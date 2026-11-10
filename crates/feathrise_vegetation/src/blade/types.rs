//! GPU 草地纯值配置、内部数据布局与每帧计算作业描述。

use bevy::prelude::{Vec2, Vec3, Vec4}; // 玩法相机LOD配置保存一至八档屏幕阈值、米制范围和常驻半径。

/// GPU叶片植被与状态回读公开支持的最大几何档数。
pub const GPU_GRASS_LOD_CAPACITY: usize = 8; // 四个外观槽各占八个几何位，完整覆盖一个 u32 诊断掩码。

/// 第四级超远草的确定性固定预算档。
///
/// 【功能设计】该档位不是运行时随机降采样：GPU按世界稳定的超远父格分块，每块只允许一个代理，
/// 因而相机静止或窗口重定位时不会重新洗牌；未由三维代理覆盖的远景综合色继续由地表材质承接。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GpuBladeUltraLodBudget {
    High,  // 每二乘二个超远父格保留一个三维代理，质量优先且显存压力最高。
    Dense, // 每三乘三个超远父格保留一个三维代理，提供比默认中档约一点七八倍的温和增密。
    #[default]
    Medium, // 每四乘四个超远父格保留一个三维代理，保留为目标硬件平衡回退档。
    Low,   // 每八乘八个超远父格保留一个三维代理，面向最低硬件或回退验证。
}

impl GpuBladeUltraLodBudget {
    /// 返回世界稳定预算块的单边超远父格数。
    pub const fn block_side(self) -> u32 {
        match self {
            Self::High => 2,   // 四个超远父格中最多保留一个代理。
            Self::Dense => 3,  // 九个超远父格中最多保留一个代理。
            Self::Medium => 4, // 十六个超远父格中最多保留一个代理。
            Self::Low => 8,    // 六十四个超远父格中最多保留一个代理。
        } // 完成无需运行时分配的固定预算换算。
    }
}
use bevy::render::render_resource::{BindGroup, Buffer}; // 每个草圈作业绑定全部输入输出并保留GPU间接瓦片派发缓冲。

/// 绘制端固定保留的叶片植被 profile 槽数量。
pub const GPU_GRASS_PROFILE_CAPACITY: usize = 4; // 两位实例字段和固定小 uniform 共同锁定四个并发外观槽。

/// 一条 GPU 叶片植被窗口使用的受检 profile 槽。
///
/// 【功能设计】公共运行时只承诺零至三号通用槽，不为槽位附加普通草、麦田或场景语义；
/// 消费者在内容适配层为槽位命名，Compute 把两位索引写入二十字节实例记录的外观字段。
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct GpuBladeProfileSlot(u32); // 私有字段禁止构造超出两位 GPU ABI 的非法编号。

impl GpuBladeProfileSlot {
    /// 第一个通用 profile 槽。
    pub const SLOT_0: Self = Self(0); // 默认槽保留旧零值 ABI，未显式配置的窗口仍能安全工作。

    /// 第二个通用 profile 槽。
    pub const SLOT_1: Self = Self(1); // 公开通用编号而不绑定任何项目内容名称。

    /// 第三个通用 profile 槽。
    pub const SLOT_2: Self = Self(2); // 公开通用编号而不绑定任何项目内容名称。

    /// 第四个通用 profile 槽。
    pub const SLOT_3: Self = Self(3); // 公开通用编号而不绑定任何项目内容名称。

    /// 尝试从公开整数构造一个合法 profile 槽。
    pub const fn new(index: u32) -> Option<Self> {
        if index < GPU_GRASS_PROFILE_CAPACITY as u32 {
            Some(Self(index)) // 零至三可无损写入草窗和紧凑实例的两位字段。
        } else {
            None // 越界值必须由调用方显式处理，公共接口不静默映射到其他外观。
        }
    }

    /// 从 GPU 标志位或内部诊断索引恢复槽位，损坏值安全回退零号槽。
    pub(crate) const fn from_gpu_index(index: u32) -> Self {
        match Self::new(index) {
            Some(slot) => slot,   // 合法两位编号保持原样。
            None => Self::SLOT_0, // 损坏或未来未知编号不得造成数组越界。
        }
    }

    /// 返回可直接写入两位 GPU 标志的稳定索引。
    pub const fn index(self) -> u32 {
        self.0 // 私有构造保证值始终位于零至三。
    }
}

/// 单个叶片植被窗口参与GPU保守剔除和屏幕LOD估算的米制规格。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuBladeConfig {
    pub profile_slot: GpuBladeProfileSlot, // 两位受检槽号决定绘制端使用哪份profile。
    pub max_blade_height: f32,             // 从地表根部到最高随机叶尖的保守高度上界。
    pub max_horizontal_sweep: f32,         // 原型宽度、静态倾斜、风摆和交互共同形成的水平包围半径。
    pub reference_height: f32,             // 屏幕空间LOD换算使用的典型植株高度而非极端最大值。
    pub root_penetration: f32,             // 根部向地表下方延伸的深度，用于剔除包围与防止悬浮。
}

impl GpuBladeConfig {
    /// 无内容语义的安全默认叶片包围规格。
    pub const DEFAULT: Self = Self {
        profile_slot: GpuBladeProfileSlot::SLOT_0, // 未显式配置的窗口使用第一个通用槽。
        max_blade_height: 0.90,                    // 覆盖公共默认 profile 的最高九十厘米叶尖。
        max_horizontal_sweep: 0.78, // 覆盖默认静态倾斜、世界风、局部交互和常见宽代理。
        reference_height: 0.70,     // 屏幕空间 LOD 使用七十厘米中性参考株高。
        root_penetration: 0.03,     // 根部下扎三厘米以压住轻微地表接缝。
    };

    /// 用有限正值构造一份消费者可调的叶片植被窗口规格。
    pub const fn new(
        profile_slot: GpuBladeProfileSlot,
        max_blade_height: f32,
        max_horizontal_sweep: f32,
        reference_height: f32,
        root_penetration: f32,
    ) -> Self {
        Self {
            profile_slot,         // 保留调用方选择的固定四槽profile身份。
            max_blade_height,     // 运行时写入草窗前仍执行有限值和下界保护。
            max_horizontal_sweep, // 运行时写入草窗前仍执行有限值和下界保护。
            reference_height,     // 运行时写入草窗前仍执行有限值和下界保护。
            root_penetration,     // 运行时写入草窗前仍执行有限值和范围保护。
        }
    }
}

impl Default for GpuBladeConfig {
    fn default() -> Self {
        Self::DEFAULT // 公共默认与零号 profile 的中性叶片包围保持一致。
    }
}

/// Blade 视图 LOD 支持的最大几何档数；与紧凑实例三位几何档号一致。
pub const GPU_BLADE_MAX_VIEW_LODS: usize = 8;

/// 消费者玩法相机使用的屏幕空间草地LOD配置。
///
/// 消费者显式提供投影阈值、叶片高度、母格和常驻半径，渲染后端只负责打包；
/// 这样不同消费者或场景可以重新标定内容，而不会继承隐藏在通用构造器里的魔数。
/// 数组按「细 → 粗」存放，有效长度由 `lod_count` 决定，尾部槽位忽略。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuBladeViewLodConfig {
    /// 相邻档投影像素阈值对，有效长度 = LOD 数 - 1；索引 0 连接最细两档。
    pub pixel_thresholds: [Vec2; GPU_BLADE_MAX_VIEW_LODS - 1],
    pub camera_fade_distances: Vec2, // X/Y分别表示近镜头整株隐藏和完全恢复距离。
    pub reference_grass_height: f32, // 投影LOD换算使用的当前草种参考高度，单位为米。
    pub finest_grid_spacing: f32,    // 层级父簇统一使用的最细世界草格间距，单位为米。
    /// 每档相对观察源的安全外半径，有效长度 = LOD 数。
    pub residency_radii: [f32; GPU_BLADE_MAX_VIEW_LODS],
    pub residency_margin: f32, // 请求子级前预留的常驻安全边距，避免4K或窄FOV选中窗外子级。
    /// 相邻档在安全边界内回退祖先的稳定过渡宽度，有效长度 = LOD 数 - 1。
    pub residency_transition_widths: [f32; GPU_BLADE_MAX_VIEW_LODS - 1],
    /// 当前配置实际使用的档数，1..=GPU_BLADE_MAX_VIEW_LODS。
    pub lod_count: u32,
}

impl GpuBladeViewLodConfig {
    /// 校验全部有效槽；无效更新必须原子拒绝，尾槽不参与作者语义。
    pub(crate) fn is_valid(self) -> bool {
        let count = self.lod_count();
        (1..=GPU_BLADE_MAX_VIEW_LODS).contains(&count)
            && self.pixel_thresholds[..count - 1]
                .iter()
                .all(|pair| pair.is_finite() && pair.x >= 0.0 && pair.y >= pair.x)
            && self.camera_fade_distances.is_finite()
            && self.camera_fade_distances.x >= 0.0
            && self.camera_fade_distances.y >= self.camera_fade_distances.x
            && self.reference_grass_height.is_finite()
            && self.reference_grass_height > 0.0
            && self.finest_grid_spacing.is_finite()
            && self.finest_grid_spacing > 0.0
            && self.residency_radii[..count]
                .iter()
                .all(|v| v.is_finite() && *v > 0.0)
            && self.residency_transition_widths[..count - 1]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
            && self.residency_margin.is_finite()
            && self.residency_margin >= 0.0
    }

    /// 返回当前配置描述的 LOD 档数。
    pub const fn lod_count(self) -> usize {
        self.lod_count as usize
    }

    /// 用完整的可变档规格构造不可变相机LOD配置；多余尾部槽位填安全零值。
    pub fn from_slice(
        pixel_thresholds: &[Vec2],
        camera_fade_distances: Vec2,
        reference_grass_height: f32,
        finest_grid_spacing: f32,
        residency_radii: &[f32],
        residency_margin: f32,
        residency_transition_widths: &[f32],
    ) -> Result<Self, String> {
        let lod_count = residency_radii.len();
        if lod_count == 0
            || lod_count > GPU_BLADE_MAX_VIEW_LODS
            || pixel_thresholds.len() + 1 != lod_count
            || residency_transition_widths.len() + 1 != lod_count
        {
            return Err(format!(
                "Blade 视图 LOD 档数必须在 1..={GPU_BLADE_MAX_VIEW_LODS}，且相邻数组长度为档数减一"
            ));
        }
        let mut config = Self {
            pixel_thresholds: [Vec2::ZERO; GPU_BLADE_MAX_VIEW_LODS - 1],
            camera_fade_distances,
            reference_grass_height,
            finest_grid_spacing,
            residency_radii: [0.0; GPU_BLADE_MAX_VIEW_LODS],
            residency_margin,
            residency_transition_widths: [0.0; GPU_BLADE_MAX_VIEW_LODS - 1],
            lod_count: lod_count as u32,
        };
        config.pixel_thresholds[..pixel_thresholds.len()].copy_from_slice(pixel_thresholds);
        config.residency_radii[..lod_count].copy_from_slice(residency_radii);
        config.residency_transition_widths[..residency_transition_widths.len()]
            .copy_from_slice(residency_transition_widths);
        Ok(config)
    }

    /// 用消费者的完整三级规格构造不可变相机LOD配置（兼容旧场景/专图）。
    #[allow(clippy::too_many_arguments)] // 固定纯值配置需要在const上下文一次性写明全部LOD维度。
    pub const fn new(
        far_to_middle_pixels: Vec2,  // 远景草簇过渡到中景草簇的投影像素区间。
        middle_to_near_pixels: Vec2, // 中景草簇过渡到近景单叶的投影像素区间。
        camera_fade_distances: Vec2, // 镜头进入草叶时的整株隐藏与恢复距离。
        reference_grass_height: f32, // 当前草种用于屏幕尺寸估计的参考高度。
        finest_grid_spacing: f32,    // 当前层级草簇共享的最细母格间距。
        residency_radii: Vec3,       // 近中远三个持久窗口的观察源安全半径。
        residency_margin: f32,       // 子级常驻边缘额外保留的回退余量。
        residency_transition_widths: Vec2, // 近级和中级分别使用的常驻边界过渡宽度。
    ) -> Self {
        let residency = residency_radii.to_array(); // const上下文通过显式数组读取SIMD向量，避免非const解引用。
        let transitions = residency_transition_widths.to_array(); // 同样显式读取两段兼容过渡宽度。
        // const 构造用数组字面量，避免 IndexMut 尚不稳定。
        let pixel_thresholds = [
            middle_to_near_pixels, // 细→粗：中近、远中。
            far_to_middle_pixels,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::ZERO,
        ];
        let radii = [
            residency[0],
            residency[1],
            residency[2],
            residency[2], // 兼容配置把第四半径钳到远级且不会被三级窗口读取。
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        let widths = [
            transitions[0],
            transitions[1],
            0.0, // 三级兼容配置不声明远级子树常驻过渡。
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        Self {
            pixel_thresholds,
            camera_fade_distances,
            reference_grass_height,
            finest_grid_spacing,
            residency_radii: radii,
            residency_margin,
            residency_transition_widths: widths,
            lod_count: 3, // 兼容三级规格实际只消费前三档。
        }
    }

    /// 构造包含最粗超远代理的四档相机驱动LOD规格（兼容旧专图）。
    #[allow(clippy::too_many_arguments)] // 每一项都是场景标定后的独立视觉或固定驻留边界，合并会隐藏单位。
    pub const fn new_with_ultra(
        ultra_to_far_pixels: Vec2,         // 超远代理进入远级子树的投影像素区间。
        far_to_middle_pixels: Vec2,        // 远景转中景的投影像素区间。
        middle_to_near_pixels: Vec2,       // 中景转近景的投影像素区间。
        camera_fade_distances: Vec2,       // 近镜头整株避让范围。
        reference_grass_height: f32,       // 当前草种投影参考高度。
        finest_grid_spacing: f32,          // 四级父簇共享的最细世界格距。
        residency_radii: Vec4,             // 近、中、远、超远四个相机驱动常驻半径。
        residency_margin: f32,             // 子级窗口外沿前的统一安全边距。
        residency_transition_widths: Vec3, // 近、中、远子级向祖先交还的稳定宽带。
    ) -> Self {
        let radii = residency_radii.to_array();
        let transitions = residency_transition_widths.to_array();
        let pixel_thresholds = [
            middle_to_near_pixels, // 细→粗。
            far_to_middle_pixels,
            ultra_to_far_pixels,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::ZERO,
            Vec2::ZERO,
        ];
        let residency = [radii[0], radii[1], radii[2], radii[3], 0.0, 0.0, 0.0, 0.0];
        let widths = [
            transitions[0],
            transitions[1],
            transitions[2],
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        Self {
            pixel_thresholds,
            camera_fade_distances,
            reference_grass_height,
            finest_grid_spacing,
            residency_radii: residency,
            residency_margin,
            residency_transition_widths: widths,
            lod_count: 4,
        }
    }
}

/// GPU紧凑实例的CPU镜像，只用于锁定顶点步长和单元测试，不做逐草CPU上传。
#[repr(C)] // 与WGSL的三个坐标、外观和法线五个32位字段保持二十字节一致。
pub(crate) struct GpuBladeCompactInstance {
    pub(crate) root_world: [f32; 3], // xyz保存已经贴地的单根草世界根位置。
    pub(crate) packed_appearance: u32, // 前三字节保存cos、sin、缩放，最高字节保存内容profile、四位覆盖份额和几何LOD。
    pub(crate) surface_normal: u32,    // snorm8保存三维承托法线；零值保持室外草的默认朝上姿态。
}

/// RenderGraph在主相机绘制前执行的一次草圈计算作业。
pub(crate) struct GpuBladeComputeJob {
    pub(crate) tile_bind_group: BindGroup, // 粗剔除与瓦片收尾绑定真实可写派发状态。
    pub(crate) classify_bind_group: BindGroup, // 分类与绘制收尾把binding7切到占位Storage，真实派发缓冲只作为Indirect参数。
    pub(crate) uniform_offset: u32,            // 共享GpuBladePatch动态uniform中的当前记录偏移。
    pub(crate) tile_cull_groups: u32,          // 每六十四个瓦片执行一个粗距离与视锥测试工作组。
    pub(crate) tile_dispatch: Buffer,          // 前十二字节由GPU生成并被细候选阶段直接间接消费。
    pub(crate) lod_priority: u32, // 零至二让作业在每个Compute阶段稳定按近、中、远顺序提交。
    pub(crate) vegetation_profile: u32, // 零至三让诊断和后续分内容计时稳定归属消费者定义的外观槽。
}

#[cfg(test)]
mod tests;
