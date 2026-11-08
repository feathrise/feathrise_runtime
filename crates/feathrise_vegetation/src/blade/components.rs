//! GPU 草地批次组件。

use bevy::ecs::query::QueryItem; // ExtractComponent 使用主世界查询结果类型。
use bevy::prelude::*; // Component 与基础反射/调试类型。
use bevy::render::batching::NoAutomaticBatching; // 每个草环带拥有独立动态uniform，禁止Bevy按共享原型网格自动合并阶段项。
use bevy::render::extract_component::ExtractComponent; // 将轻量批次描述同步到渲染世界。
use bevy::render::render_resource::Buffer;
use bevy::render::render_resource::ShaderType; // 按 WGSL 对齐规则把每块几十字节参数写入动态 uniform。 // 渲染世界组件持有跨帧复用的紧凑实例与间接状态GPU缓冲。
use bevy::render::sync_component::SyncComponent; // Bevy 0.19 要求显式声明主世界组件在渲染世界中的目标Bundle。

use crate::types::{
    GpuBladeConfig, GpuBladeProfileSlot, GpuBladeUltraLodBudget, GpuBladeViewLodConfig,
}; // 消费项目显式传入外观槽、保守包围、超远固定预算和玩法相机投影规格。

/// 可选作者源票据，只用于把实际GPU回读绑定到同一已采用修订；不改变着色器ABI。
#[derive(Component, Clone, Copy, Debug)]
pub struct GpuBladeSourceRevision(pub u64);

impl ExtractComponent<bevy::render::RenderApp> for GpuBladeSourceRevision {
    type QueryData = &'static Self;
    type QueryFilter = ();
    type Out = Self;
    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> { Some(*item) }
}
impl SyncComponent<bevy::render::RenderApp> for GpuBladeSourceRevision { type Target = Self; }

/// 一个 Blade 窗口选择的运行时 profile 批次。
///
/// 批次零兼容既有全局四槽；数据驱动目录从批次一开始递增。该值只选择绑定组，
/// 实例 `packed_appearance` 仍只保存批次内部的两位局部槽号。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GpuBladeProfileBankId(u32); // 私有字段防止调用方把任意整数误当成已登记批次。

impl GpuBladeProfileBankId {
    /// 既有场景和最小样例继续使用的兼容批次零。
    pub const LEGACY: Self = Self(0); // 零值默认保证旧生成代码无需插入新组件。

    /// 从数据驱动批次数组下标构造公开批次身份。
    pub fn for_dynamic_index(index: usize) -> Option<Self> {
        let index = u32::try_from(index).ok()?; // GPU/组件身份只接受可无损表达的实际驻留下标。
        index.checked_add(1).map(Self) // 零保留给兼容批次，动态第零项映射为一。
    }

    /// 返回渲染绑定组数组使用的零起始下标。
    pub const fn index(self) -> u32 {
        self.0 // 内容稳定 ID 不写入这里，该值只在当前会话内有效。
    }
}

impl ExtractComponent<bevy::render::RenderApp> for GpuBladeProfileBankId {
    type QueryData = &'static GpuBladeProfileBankId; // GpuBladePatch 的 require 契约保证每个窗口都已有明确批次。
    type QueryFilter = (); // 不附加场景语义过滤条件。
    type Out = Self; // RenderApp 只保存四字节会话身份。

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        Some(*item) // 每帧只复制四字节会话身份，不复制目录或纹理句柄。
    }
}

impl SyncComponent<bevy::render::RenderApp> for GpuBladeProfileBankId {
    type Target = Self; // 批次身份直接同步到渲染实体，不派生额外 Bundle。
}

/// 一个植被根持有的绘制槽引用；候选与旧根共存，退役只归还本根引用。
#[derive(Component)]
#[component(on_discard = release_profile_bank_allocation)]
pub struct GpuBladeProfileBankAllocation {
    pub(crate) epoch: std::sync::Arc<()>,
    pub(crate) ids: std::sync::Arc<[GpuBladeProfileBankId]>,
}
impl GpuBladeProfileBankAllocation {
    /// 将本次打包的局部批次下标映射到全世界稳定槽位，不改变四槽内部profile编号。
    pub fn bank(&self, local_index: usize) -> Option<GpuBladeProfileBankId> {
        self.ids.get(local_index).copied()
    }
}

fn release_profile_bank_allocation(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    let allocation = world.get::<GpuBladeProfileBankAllocation>(context.entity)
        .map(|allocation| (allocation.epoch.clone(), allocation.ids.clone()));
    if let Some((epoch, ids)) = allocation && !ids.is_empty()
        && let Some(mut banks) = world.get_resource_mut::<super::resources::GpuBladeProfileBanks>() {
        banks.release(&epoch, &ids); // Arc只浅克隆；移除/替换本根都不扫描其他地图或草窗。
    }
}

/// `grid.w`最低位表示当前窗口启用GPU密度概率采样。
const DENSITY_FIELD_ENABLED_FLAG: u32 = 1 << 0;

/// 几何档号占用第 1–3 位，支持 0..=7 共八档，与目录 MAX_BLADE_PRESET_LODS 一致。
const LOD_LEVEL_SHIFT: u32 = 1;

/// 从`grid.w`中安全清除或读取三个几何档号位，不影响密度纹理启用标志。
const LOD_LEVEL_MASK: u32 = 0b111 << LOD_LEVEL_SHIFT;

/// 渲染世界临时设置该位时，本帧继续消费上一份有效GPU输出而不覆盖间接绘制状态。
const RETAIN_RESIDENT_OUTPUT_FLAG: u32 = 1 << 4;

/// 正式层级LOD使用该位声明同源父簇层级；相机数据暂不可用时只保留最远祖先代理，禁止多档重叠。
const HIERARCHICAL_LOD_FLAG: u32 = 1 << 5;

/// `grid.w`第六位之后的两个标志位保存通用叶片内容profile槽号。
const VEGETATION_PROFILE_SHIFT: u32 = 6;

/// 两位内容编号支持四份固定profile，共用二十字节紧凑实例的外观字段。
const VEGETATION_PROFILE_MASK: u32 = 0b11 << VEGETATION_PROFILE_SHIFT;

/// 启用最粗档超远层级树；未设置该位时只保留兼容短链路径。
const ULTRA_LOD_CHAIN_FLAG: u32 = 1 << 8;

/// 最粗档超远预算块单边指数存放在第九至十位，二、四、八分别编码为零、一、二。
const ULTRA_LOD_BUDGET_SHIFT: u32 = 9;

/// 两位预算编码覆盖 High、Dense、Medium、Low 四档固定预算。
const ULTRA_LOD_BUDGET_MASK: u32 = 0b11 << ULTRA_LOD_BUDGET_SHIFT;

/// 几何档号最高合法值；三位字段支持 0..=7。
const MAX_LOD_LEVEL: u32 = 7;

/// 单个 GPU 草地绘制窗口。
///
/// 【功能设计】CPU只提交窗口中心、常驻半径和实例数量，不创建逐草实体，也不生成逐草位置数组；
/// Compute按绝对世界源草格生成位置、整簇LOD、朝向和高度，绘制shader再稳定重建宽度、倾斜、柔韧度和颜色；窗口跟随玩家时重叠区域不会重新洗牌。
#[derive(Component, Clone, Copy, Debug, PartialEq, ShaderType)]
#[repr(C)] // 含数组的扩展区也固定声明顺序，CPU布局与ShaderType字段顺序一致。
#[require(NoAutomaticBatching, GpuBladeProfileBankId)] // 每个窗口都禁止自动合批，并默认选择兼容批次零。
pub struct GpuBladePatch {
    pub(crate) origin_extent: Vec4, // xyz 为草块世界原点，w 为正方形边长。
    pub(crate) grid: UVec4, // x 为单边候选数，y 为活动实例数，z 为随机种子，w 保存绘制标志位。
    pub(crate) focus_radius: Vec4, // xyz为连续玩家常驻中心，w为当前窗口完全消失的外半径；视觉LOD可改由相机uniform负责。
    pub(crate) height_world: Vec4, // xy 为高度纹理世界最小 X/Z，zw 为世界最大 X/Z；无效范围表示使用固定平面。
    pub(crate) height_decode: Vec4, // x为编码最低米制高度，y为跨度，z为启用标记，w为全局激活覆盖率。
    pub(crate) density_world: Vec4, // xy 为密度纹理世界最小X/Z，zw为世界最大X/Z；启用位单独保存在grid.w。
    pub(crate) lod_density: Vec4,   // x为内圈空洞半径，y/z为内外过渡宽度，w为非零密度概率增益。
    pub(crate) vegetation_bounds: Vec4, // x为最高叶尖，y为水平扫掠半径，z为投影参考高度，w为根部下扎深度。
    pub(crate) density_control_world: Vec4, // xy/zw为可选受控种植区世界X/Z最小角与最大角；零范围表示不追加区域裁剪。
    pub(crate) view_lod_pixel_thresholds: Vec4, // 内容级前两对像素阈值；是否继承玩法视图由metadata档数决定。
    pub(crate) view_lod_ultra_fade: Vec4,       // xy为内容级超远转远阈值，zw为近镜头整株避让距离。
    pub(crate) view_lod_residency_radii: Vec4, // 内容级前四档常驻半径；局部配置由独立metadata档数启用。
    pub(crate) view_lod_residency_transitions: Vec4, // xyz为内容级近中远回退宽度，w为共用安全边距。
    pub(crate) view_lod_pixel_thresholds_ext: [Vec4; 2], // 后四对相邻阈值；保持前四档字段偏移兼容。
    pub(crate) view_lod_residency_radii_ext: Vec4, // 第五至八档常驻半径。
    pub(crate) view_lod_residency_transitions_ext: Vec4, // 第四至七段交接宽度。
    pub(crate) view_lod_metadata: Vec4,        // x为实际档数，yz为参考高度和母格；x=0继承视图。
    pub(crate) source_to_world: Mat4,
    pub(crate) world_to_source: Mat4,
}

/// 标记被 F3/F4 性能隔离临时关闭的单个 GPU 叶片植被 LOD 窗口。
///
/// 【功能设计】该标记只存在于主世界且不持久化；提取系统看到它后主动移除渲染世界中的
/// `GpuBladePatch`，从而让对应档位同时退出 Compute、主视图绘制和方向光阴影排队。
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GpuBladeLodDebugDisabled;

/// 渲染世界中单个草圈持久复用的GPU压缩输出。
///
/// 【功能设计】缓冲只在容量变化时重建；每帧计算着色器覆盖存活前缀并更新间接实例数，
/// 禁止把数十MiB实例缓冲退化成逐帧创建或CPU上传。
#[derive(Component, Clone)]
pub(crate) struct GpuBladePreparedBuffers {
    pub(crate) instances: Buffer, // STORAGE阶段写入、VERTEX阶段读取的二十字节紧凑实例数组。
    pub(crate) draw_state: Buffer, // STORAGE阶段更新并供异步诊断读取的八十字节聚合状态块。
    pub(crate) depth_bucket_draws: Buffer, // 八个等面积径向桶的间接参数、分区容量和原子预留状态。
    pub(crate) active_tiles: Buffer, // 粗剔除仅追加通过环带与视锥测试的瓦片线性编号。
    pub(crate) tile_dispatch: Buffer, // GPU写入活跃瓦片数并由候选阶段直接执行间接派发。
    pub(crate) classify_dispatch_scratch: Buffer, // 分类与绘制收尾阶段替代binding7，避免真实派发缓冲在同一dispatch兼任可写Storage与Indirect。
    pub(crate) capacity: u32,                     // 当前实例缓冲最多可容纳的存活草株数量。
    pub(crate) tile_count: u32, // 当前候选正方形切分后的瓦片总数，变化时必须重建列表容量。
    pub(crate) generation: u32, // 容量变化时递增，供后续绑定组缓存和异步回读识别旧缓冲。
    pub(crate) clipmap_missing_pages: [UVec4; 16], // 局部缺页掩码也是完整输出身份，变化时禁止复用旧候选集合。
    pub(crate) clipmap_revision: Option<[u64; 3]>, // GPU 输出对应的完整地图/高度/视图修订。
    pub(crate) output_valid: bool, // 四阶段已就绪且本帧作业会在相机前完成时，才允许最终Draw消费持久间接参数；Queue仅准备候选项。
    pub(crate) source_revision: u64, // 最后真正排入完整Compute作业的作者来源；纹理等待复用旧输出时保持旧值。
}

mod clipmap; // 已采用分页高度的公共组件契约。
mod clipmap_stitches;
pub use clipmap_stitches::{GpuBladeClipmapStitches,clipmap_stitch_shader_source};
pub use clipmap::{GPU_BLADE_CLIPMAP_MAX_BANDS, GpuBladeClipmapBand, GpuBladeClipmapHeight}; // 场景只传通用高度资源和固定上限环带，不反向依赖。
mod patch_fields;
mod patch_lod;

/// 单个草场窗口绑定的GPU高度单元纹理。
///
/// 每个texel以RGBA16依次保存地形单元的NW、NE、SW、SE四角；shader一次textureLoad后按真实三角面插值。
#[derive(Component, Clone, Debug)]
pub struct GpuBladeHeightMap {
    pub(crate) image: Handle<Image>, // 强句柄让地图存活期间保留对应GPU纹理，离场后允许统一释放。
}

/// 单个草场窗口绑定的GPU草地密度概率纹理。
///
/// 红通道是零到一的生草概率；shader单次读取最近texel，并用稳定世界草格随机值生成不会随窗口移动洗牌的草株集合。
#[derive(Component, Clone, Debug, PartialEq)]
pub struct GpuBladeDensityMap {
    pub(crate) image: Handle<Image>, // 强句柄让地图存活期间保留约2048平方密度资产，离场后允许统一释放。
}

/// 三维作者样本输入：位置/概率、法线/格距、稳定作者中心，以及可选的有界密度查找索引。
/// 上传的是有界作者格，不是CPU生成的逐株实例；候选与压缩仍由GPU完成。
#[derive(Component, Clone)]
pub struct GpuBladeVolumeSource(pub Handle<Image>);
impl ExtractComponent<bevy::render::RenderApp> for GpuBladeVolumeSource {
    type QueryData = &'static Self;
    type QueryFilter = ();
    type Out = Self;
    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self> {
        Some(item.clone())
    }
}
impl SyncComponent<bevy::render::RenderApp> for GpuBladeVolumeSource {
    type Target = Self;
}

impl GpuBladeDensityMap {
    /// 绑定已经按线性数值红通道概率契约生成的图片资产。
    pub fn new(image: Handle<Image>) -> Self {
        Self { image } // 不复制纹理像素，只保存Bevy资产句柄。
    }
}

impl ExtractComponent<bevy::render::RenderApp> for GpuBladeDensityMap {
    type QueryData = &'static GpuBladeDensityMap; // 主世界只读密度纹理句柄。
    type QueryFilter = (); // 所有带该组件的草场窗口都需要提取。
    type Out = Self; // 渲染世界保存同一强句柄直到实体同步清理。

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        Some(item.clone()) // 每帧只克隆句柄，不复制约0.61MiB压缩PNG或16MiB RGBA8 GPU像素。
    }
}

impl SyncComponent<bevy::render::RenderApp> for GpuBladeDensityMap {
    type Target = Self; // 密度句柄直接同步到同一渲染组件，不派生额外Bundle。
}

impl GpuBladeHeightMap {
    /// 绑定已经按高度单元契约生成的图片资产。
    pub fn new(image: Handle<Image>) -> Self {
        Self { image } // 不复制纹理像素，只保存Bevy资产句柄。
    }
}

impl ExtractComponent<bevy::render::RenderApp> for GpuBladeHeightMap {
    type QueryData = &'static GpuBladeHeightMap; // 主世界只读高度纹理句柄。
    type QueryFilter = (); // 所有带该组件的草场窗口都需要提取。
    type Out = Self; // 渲染世界保存同一强句柄直到实体同步清理。

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        Some(item.clone()) // 每帧只克隆句柄，不复制约1.3MiB PNG或2MiB GPU像素。
    }
}

impl SyncComponent<bevy::render::RenderApp> for GpuBladeHeightMap {
    type Target = Self; // 高度句柄直接同步到同一渲染组件，不派生额外Bundle。
}

impl ExtractComponent<bevy::render::RenderApp> for GpuBladePatch {
    type QueryData = (
        &'static GpuBladePatch,        // 读取主世界中不可变的批次描述。
        Has<GpuBladeLodDebugDisabled>, // 同时读取F3是否临时关闭当前内容档位。
        Option<&'static Transform>,
        Option<&'static GlobalTransform>,
    ); // 保持查询覆盖已关闭窗口，才能显式清理上一帧残留的渲染组件。
    type QueryFilter = (); // 不在查询过滤器排除关闭项，否则旧渲染组件无法收到移除命令。
    type Out = Self; // 渲染世界保存同一轻量组件。

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        let (patch, debug_disabled, local, global) = item; // 解构草窗参数和非持久化调试关闭状态。
        if debug_disabled { return None; }
        let mut extracted = *patch;
        // 跟随已传播的父层级，移动整图无需重写逐草位置或生成新高度页。
        if let (Some(local), Some(global)) = (local, global) {
            extracted.source_to_world = global.to_matrix() * local.to_matrix().inverse();
            extracted.world_to_source = extracted.source_to_world.inverse();
        }
        Some(extracted) // 关闭时返回None，让Bevy移除旧组件并停止后续Compute、Draw和阴影查询。
    }
}

impl SyncComponent<bevy::render::RenderApp> for GpuBladePatch {
    type Target = Self; // 调试禁用时Extract返回None，Bevy据此移除同一目标组件。
}

/// 标记允许项目自有草地进入的玩法相机，并保存该视图专属的GPU视觉LOD配置。
///
/// 【功能设计】玩家仍负责草窗常驻中心；开启屏幕空间LOD后，渲染世界只把相机投影阈值和近镜头整株避让参数
/// 打包进共享视图uniform，不在主线程生成、筛选或上传任何逐草数据。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub struct GpuBladeView {
    screen_space_lod_enabled: bool, // false保留既有玩家径向LOD，true允许Compute按玩法相机投影尺寸选档。
    lod_pixel_thresholds: Vec4,     // xy为远转中开始/结束像素，zw为中转近开始/结束像素。
    lod_ultra_pixel_thresholds: Vec4, // xy为超远转远级子树开始/结束像素，zw保留。
    camera_fade_lod_params: Vec4, // xy为近镜头整株避让范围，z为投影参考草高，w为统一最细世界草格。
    lod_residency_radii: Vec4,    // xyzw为近中远超远相机驱动常驻外半径。
    lod_residency_transition_widths: Vec4, // xyz为近/中/远子级过渡宽度，w为统一安全边距。
    quality_multipliers: Vec4,    // xyz为最终密度、LOD像素阈值和作者范围倍率，w保留并固定为一。
    lod_pixel_thresholds_ext: [Vec4; 2], // 第四至七对屏幕阈值。
    lod_residency_radii_ext: Vec4, // 第五至八档常驻半径。
    lod_residency_transitions_ext: Vec4, // 第四至七段交接宽度。
    lod_metadata: Vec4,           // x为实际档数，yz为内容投影参考和母格。
}

impl GpuBladeView {
    /// 构造保守兼容视图：只启用既有六平面剔除，不启用屏幕空间LOD。
    pub const fn legacy() -> Self {
        Self {
            screen_space_lod_enabled: false, // 未显式启用的地图与最小样例继续使用GpuBladePatch中的玩家径向环带。
            lod_pixel_thresholds: Vec4::ZERO, // 禁用模式不得向后续WGSL暴露看似有效的像素阈值。
            lod_ultra_pixel_thresholds: Vec4::ZERO, // 兼容模式不得触发第四级超远交接。
            camera_fade_lod_params: Vec4::ZERO, // 禁用模式不得误触发近镜头整株避让。
            lod_residency_radii: Vec4::ZERO, // 旧径向草块没有四级跨窗口祖先回退契约。
            lod_residency_transition_widths: Vec4::ZERO, // 旧径向草块不执行跨窗口LOD过渡。
            quality_multipliers: Vec4::ONE,  // 未接管画质时严格保持作者密度、范围与既有LOD。
            lod_pixel_thresholds_ext: [Vec4::ZERO; 2],
            lod_residency_radii_ext: Vec4::ZERO,
            lod_residency_transitions_ext: Vec4::ZERO,
            lod_metadata: Vec4::ZERO,
        } // 完成不会改变当前画面的保守配置。
    }

    /// 使用消费者显式规格构造玩法相机投影LOD配置。
    ///
    /// 前四档继续占用既有 Vec4 槽位，第五至八档写入扩展槽；`lod_count` 由配置携带。
    pub const fn screen_space_lod_with(config: GpuBladeViewLodConfig) -> Self {
        // 细→粗 pixel_thresholds 映射到既有打包顺序：
        // [0]中近 → zw，[1]远中 → xy，[2]超远转远 → ultra.xy。
        let t0 = config.pixel_thresholds[0];
        let t1 = config.pixel_thresholds[1];
        let t2 = config.pixel_thresholds[2];
        Self {
            screen_space_lod_enabled: true, // 唯一玩法视图准备成功后使用明确的屏幕空间模式值。
            lod_pixel_thresholds: Vec4::new(t1.x, t1.y, t0.x, t0.y),
            lod_ultra_pixel_thresholds: Vec4::new(t2.x, t2.y, 0.0, 0.0),
            camera_fade_lod_params: Vec4::new(
                config.camera_fade_distances.x, // 镜头进入草叶运动包围体后的整株隐藏距离。
                config.camera_fade_distances.y, // 草株完全恢复距离。
                config.reference_grass_height,  // 当前草种屏幕投影使用的参考高度。
                config.finest_grid_spacing,     // 当前父簇层级使用的统一最细母格。
            ), // 所有参数只进入共享视图uniform，不生成任何CPU逐草数据。
            lod_residency_radii: Vec4::new(
                config.residency_radii[0],
                config.residency_radii[1],
                config.residency_radii[2],
                config.residency_radii[3],
            ), // 前四档常驻半径；扩展档见 lod_residency_radii_ext。
            lod_residency_transition_widths: Vec4::new(
                config.residency_transition_widths[0], // 近级在安全边界内逐父簇交还中级。
                config.residency_transition_widths[1], // 中级在安全边界内逐父簇交还远级。
                config.residency_transition_widths[2], // 远级在安全边界内逐父簇交还超远级。
                config.residency_margin,               // 尾槽保存全部子级共用的外沿安全余量。
            ), // 固定世界哈希乘连续权重，静止镜头不会让同一父簇逐帧换级。
            quality_multipliers: Vec4::ONE, // 地图作者配置默认使用完整高档，运行时设置可另行幂等覆盖。
            lod_pixel_thresholds_ext: [
                Vec4::new(
                    config.pixel_thresholds[3].x,
                    config.pixel_thresholds[3].y,
                    config.pixel_thresholds[4].x,
                    config.pixel_thresholds[4].y,
                ),
                Vec4::new(
                    config.pixel_thresholds[5].x,
                    config.pixel_thresholds[5].y,
                    config.pixel_thresholds[6].x,
                    config.pixel_thresholds[6].y,
                ),
            ],
            lod_residency_radii_ext: Vec4::new(
                config.residency_radii[4],
                config.residency_radii[5],
                config.residency_radii[6],
                config.residency_radii[7],
            ),
            lod_residency_transitions_ext: Vec4::new(
                config.residency_transition_widths[3],
                config.residency_transition_widths[4],
                config.residency_transition_widths[5],
                config.residency_transition_widths[6],
            ),
            lod_metadata: Vec4::new(
                config.lod_count as f32,
                config.reference_grass_height,
                config.finest_grid_spacing,
                0.0,
            ),
        } // 完成只包含视图级常量的Copy配置，不持有资产或GPU资源。
    }

    /// 链式应用最终密度、LOD 阈值与作者范围三个画质倍率。
    pub fn with_quality_multipliers(
        mut self,           // 按值接收玩法视图，便于场景构造时继续链式插入。
        density: f32,       // 最终密度概率倍率，零至一不会让内容超过作者预算。
        lod_threshold: f32, // 屏幕阈值倍率，越低越晚切换到粗模。
        view_distance: f32, // 作者最远范围倍率，最多一防止越过既有候选窗口。
    ) -> Self {
        self.set_quality_multipliers(density, lod_threshold, view_distance); // 复用原子校验与钳制逻辑。
        self // 返回已经写入有限安全倍率的玩法视图。
    }

    /// 更新三个 GPU 画质倍率；非法输入完整保留上一份有效设置。
    pub fn set_quality_multipliers(
        &mut self,          // 只更新独立质量槽，不改写地图作者LOD规格。
        density: f32,       // 最终密度概率倍率。
        lod_threshold: f32, // 屏幕LOD阈值倍率。
        view_distance: f32, // 草窗与常驻半径倍率。
    ) {
        if !density.is_finite() || !lod_threshold.is_finite() || !view_distance.is_finite() {
            return; // 任一异常值都不得部分污染同一GPU质量向量。
        } // 完成三个输入的原子有限性校验。
        self.quality_multipliers = Vec4::new(
            density.clamp(0.0, 1.0), // 允许未来关闭叶片，但禁止设置把作者密度向上放大。
            lod_threshold.clamp(0.5, 2.0), // 给未来档位保留两倍区间并避免阈值退化为零。
            view_distance.clamp(0.25, 1.0), // 最低保留四分之一范围，最高严格止于作者窗口。
            1.0,                     // 尾槽保持确定值，供未来扩展前锁定当前GPU ABI。
        ); // 完成不会触发候选缓冲重建的三个纯倍率打包。
    }

    /// 返回最终密度、LOD 阈值和作者范围三个当前画质倍率。
    pub fn quality_multipliers(self) -> Vec3 {
        self.quality_multipliers.truncate() // 主世界设置系统只比较三个有业务含义的槽位。
    }

    /// 返回当前玩法视图是否请求屏幕空间草地LOD。
    pub(crate) const fn uses_screen_space_lod(self) -> bool {
        self.screen_space_lod_enabled // 渲染世界仍会在投影无效时保守回退兼容模式。
    }

    /// 返回远中与中近两段屏幕像素阈值。
    pub(crate) const fn lod_pixel_thresholds(self) -> Vec4 {
        self.lod_pixel_thresholds // 四项阈值由渲染世界原样写入追加的视图uniform。
    }

    /// 返回超远代理到既有远级子树的屏幕像素阈值。
    pub(crate) const fn lod_ultra_pixel_thresholds(self) -> Vec4 {
        self.lod_ultra_pixel_thresholds // 只有显式第四级层级窗口会在WGSL中读取xy。
    }

    /// 返回近镜头整株避让、参考草高与统一母格参数。
    pub(crate) const fn camera_fade_lod_params(self) -> Vec4 {
        self.camera_fade_lod_params // 参数只供后续Compute分类读取，不参与CPU逐草计算。
    }

    /// 返回近中远常驻半径与子级可用性安全边距。
    pub const fn lod_residency_radii(self) -> Vec4 {
        self.lod_residency_radii // Compute在目标子级不驻留时必须选择仍可用的祖先代理。
    }

    /// 返回近级与中级常驻边界的稳定整簇过渡宽度。
    pub const fn lod_residency_transition_widths(self) -> Vec4 {
        self.lod_residency_transition_widths // Compute按固定世界父格概率渐退，消除玩家半径上的LOD硬环。
    }

    /// 渲染世界原样传递扩展档，不能仅扩大目录数组而截断 GPU 配置。
    pub(crate) const fn lod_pixel_thresholds_ext(self) -> [Vec4; 2] {
        self.lod_pixel_thresholds_ext
    }
    pub(crate) const fn lod_residency_radii_ext(self) -> Vec4 {
        self.lod_residency_radii_ext
    }
    pub(crate) const fn lod_residency_transitions_ext(self) -> Vec4 {
        self.lod_residency_transitions_ext
    }
    pub(crate) const fn lod_metadata(self) -> Vec4 {
        self.lod_metadata
    }
}

impl Default for GpuBladeView {
    fn default() -> Self {
        Self::legacy() // 未显式升级的地图必须保持当前玩家径向LOD行为。
    }
}

impl ExtractComponent<bevy::render::RenderApp> for GpuBladeView {
    type QueryData = &'static GpuBladeView; // 读取主世界玩法相机标记及其三组Copy配置。
    type QueryFilter = (); // 不附加其他相机过滤条件。
    type Out = Self; // 渲染世界保留同一轻量配置供共享相机uniform准备。

    fn extract_component(item: QueryItem<'_, '_, Self::QueryData>) -> Option<Self::Out> {
        Some(*item) // 每帧只复制视图常量与三个画质倍率，不复制任何草株或资产数据。
    }
}

impl SyncComponent<bevy::render::RenderApp> for GpuBladeView {
    type Target = Self; // 玩法相机配置按值同步到同一渲染组件。
}

#[cfg(test)]
#[path = "components_tests.rs"]
mod tests; // 草窗数据契约测试独立存放，避免生产组件与测试细节共同超过单文件职责上限。
