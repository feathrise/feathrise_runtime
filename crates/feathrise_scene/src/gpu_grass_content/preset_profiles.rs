//! 开发者植被预设到通用 GPU Blade 运行时规格的适配层。
//!
//! 【功能设计】地图只保存目录中的稳定预设 ID；本模块在当前资源修订内解析四级网格、
//! 外观、风或水流驱动和包围参数。GPU 批次及局部槽由动态地图随后派生，绝不写回地图。

use bevy::prelude::*; // AssetServer、Mesh/Image 强句柄和 GPU 配置向量。
use feathrise_environment::vegetation::blade::{
    GpuBladeConfig, GpuBladeProfile, GpuBladeProfileSlot, GpuBladeUltraLodBudget,
    GpuBladeViewLodConfig,
}; // 公共后端纯值类型保持项目内容与渲染 ABI 解耦。
use feathrise_map_content::{
    BladePresetAppearance, BladePresetLod, BladePresetMotion, BladePresetUltraLodBudget,
    BladePresetViewLod, VegetationPresetDescriptor, VegetationPresetRuntime,
}; // 开发者 JSON 的完整 Blade 描述由地图内容 crate 唯一定义。

/// 当前地图修订使用的一档拥有名称所有权的 Blade 候选窗口。
#[derive(Clone, Debug)]
pub struct RuntimeBladeLodSpec {
    pub name: String,          // F3 与实体名使用目录提供的可读名称。
    pub level: u32,            // 零至三身份已经在目录加载时完整校验。
    pub grid_side: u32,        // 正方形候选网格单边数量。
    pub spacing: f32,          // 世界稳定候选格距，单位为米。
    pub inner_radius: f32,     // 当前档径向内洞；屏幕层级内容通常为零。
    pub outer_radius: f32,     // 当前档窗口安全外半径。
    pub inner_fade_width: f32, // 与更细档交接的稳定宽度。
    pub outer_fade_width: f32, // 窗口边缘整株退出宽度。
    pub seed: u32,             // 当前档内容盐。
    pub snap_cells: u32,       // 窗口每次重定位跨越的完整候选格数。
}

impl RuntimeBladeLodSpec {
    /// 返回端点式正方形候选窗口边长。
    pub fn patch_extent(&self) -> f32 {
        self.grid_side.saturating_sub(1) as f32 * self.spacing // 与公共 GpuBladePatch 构造规则保持一致。
    }
}

/// 当前地图修订使用的一种完整 Blade 纯值规格。
#[derive(Clone, Debug)]
pub struct RuntimeBladeProfileSpec {
    pub id: String,                     // 当前目录规范 ID；旧别名在进入本结构前已迁移。
    pub label: String,                  // 作者可读名称。
    pub lods: Vec<RuntimeBladeLodSpec>, // 可变档数网格身份和候选窗口参数，按下标对应几何 LOD。
    pub ultra_lod_budget: GpuBladeUltraLodBudget, // 最粗档世界稳定固定预算。
    pub view_lod: GpuBladeViewLodConfig, // 内容级屏幕阈值、避让与常驻边界。
    pub density_gain: f32,              // 非零 R8 概率增益。
    pub vegetation_config: GpuBladeConfig, // 局部 profile 槽与保守运动包围。
    pub ultra_horizontal_sweep: f32,    // 复用作物远模时的额外水平扫掠。
}

/// 一个 Blade 预设在当前 AssetServer 中已经请求的完整强资产集合。
#[derive(Clone)]
pub struct RuntimeBladeProfileAssets {
    pub spec: RuntimeBladeProfileSpec, // 窗口创建所需的全部拥有型纯值。
    pub meshes: Vec<Handle<Mesh>>,     // 与 lods 同长的强网格句柄，下标严格等于几何 LOD。
    pub appearance: GpuBladeProfile,   // 当前批次局部槽写入的外观与运动常量。
    pub base_color_texture_path: Option<String>, // 批次打包按同一纹理页分组。
    pub base_color_texture: Option<Handle<Image>>, // None 明确使用公共白色回退。
}

/// 解析并请求一个开发者 Blade 预设；Mesh 预设由通用 Mesh 注册链处理。
pub fn load_blade_preset(
    asset_server: &AssetServer, // 所有网格和纹理继续走 Bevy 异步资产管线。
    preset: &VegetationPresetDescriptor, // 已由全局目录原子校验通过的规范项。
) -> Result<RuntimeBladeProfileAssets, String> {
    match &preset.runtime {
        VegetationPresetRuntime::Blade {
            lods,
            appearance,
            base_color_texture_path,
            motion,
            bounds,
            view_lod,
            density_gain,
            ultra_lod_budget,
            ultra_horizontal_sweep,
        } => {
            let ordered_lods = ordered_data_lods(lods)?; // JSON 数组顺序不承担 LOD 身份，显式按 level 归一化。
            let meshes = ordered_lods
                .iter()
                .map(|lod| asset_server.load::<Mesh>(lod.mesh_path.clone())) // 每档强类型路径可指向独立 GLB primitive。
                .collect::<Vec<_>>();
            let runtime_lods = ordered_lods.into_iter().map(runtime_lod_from_data).collect(); // 网格句柄和纯值列表保持同一 level 下标。
            let appearance = apply_motion(appearance_from_data(appearance), motion)?; // 外观和运动都完全来自目录。
            let texture_path = base_color_texture_path.clone(); // None 明确使用公共白色回退。
            Ok(RuntimeBladeProfileAssets {
                spec: RuntimeBladeProfileSpec {
                    id: preset.id.clone(),       // 保存规范 ID 而不是地图引用的历史别名。
                    label: preset.label.clone(), // 诊断使用开发者目录名称。
                    lods: runtime_lods,          // 已按 level 排序的可变档列表。
                    ultra_lod_budget: ultra_budget_from_data(*ultra_lod_budget), // 固定预算枚举一一映射。
                    view_lod: view_lod_from_data(view_lod), // 完整屏幕和常驻参数不套用 HN-01 默认值。
                    density_gain: *density_gain,            // 目录校验已保证有限非负。
                    vegetation_config: GpuBladeConfig::new(
                        GpuBladeProfileSlot::SLOT_0, // 真正局部槽由当前地图批次打包阶段替换。
                        bounds.max_blade_height,     // 最高叶尖参与视锥包围。
                        bounds.max_horizontal_sweep, // 静态、运动和原型共同水平包围。
                        bounds.reference_height,     // 屏幕 LOD 典型高度。
                        bounds.root_penetration,     // 根部下扎参与纵向包围。
                    ),
                    ultra_horizontal_sweep: *ultra_horizontal_sweep, // 只在最粗档显式正值时生效。
                },
                meshes,     // 全部强句柄已发起异步请求。
                appearance, // 十个 vec4 固定内容常量不随实例量增长。
                base_color_texture: texture_path
                    .as_ref()
                    .map(|path| asset_server.load(path.clone())), // 显式纹理页异步请求一次。
                base_color_texture_path: texture_path, // 稳定路径用作批次分组键。
            })
        }
        VegetationPresetRuntime::Mesh { .. } => Err(format!(
            "植被预设{}不是 Blade 类型，不能创建 GPU 叶片窗口",
            preset.id
        )), // 后端类别分叉必须保持显式，禁止错误资产悄然走另一条管线。
    }
}

/// 把数据目录的可变档按显式 level 排序，并验证转换后 level 恰好覆盖 0..n-1。
fn ordered_data_lods(lods: &[BladePresetLod]) -> Result<Vec<BladePresetLod>, String> {
    let mut ordered = lods.to_vec(); // 只复制小描述，资产路径所有权随当前修订保持。
    ordered.sort_by_key(|lod| lod.level); // 数组顺序不影响运行时网格身份。
    if ordered
        .iter()
        .enumerate()
        .any(|(index, lod)| lod.level != index as u32)
    {
        return Err("数据 Blade 的 LOD level 必须恰好覆盖 0..n-1".to_owned()); // 防御绕过目录校验的直接测试调用。
    }
    Ok(ordered) // 下标现在可安全用于 Mesh3d 和 N 窗配对。
}

/// 把一个目录 LOD 转成不再借用目录资源的当前修订纯值。
fn runtime_lod_from_data(lod: BladePresetLod) -> RuntimeBladeLodSpec {
    RuntimeBladeLodSpec {
        name: lod.name,                         // 转移名称所有权避免重复分配。
        level: lod.level,                       // 已排序的零至三级身份。
        grid_side: lod.grid_side,               // 候选网格规模。
        spacing: lod.spacing,                   // 世界格距。
        inner_radius: lod.inner_radius,         // 径向内洞。
        outer_radius: lod.outer_radius,         // 安全外沿。
        inner_fade_width: lod.inner_fade_width, // 内过渡带。
        outer_fade_width: lod.outer_fade_width, // 外淡出带。
        seed: lod.seed,                         // 当前档稳定盐。
        snap_cells: lod.snap_cells,             // 重定位完整格步长。
    }
}

/// 把八组目录外观向量转成公共 GPU profile；运动字段随后由唯一函数写入。
fn appearance_from_data(value: &BladePresetAppearance) -> GpuBladeProfile {
    GpuBladeProfile::new(
        Vec4::from_array(value.geometry),          // 株高、根深与静态倾斜。
        Vec4::from_array(value.width_flexibility), // 宽度和柔韧度范围。
        Vec4::from_array(value.bending),           // 交互、总弯曲与响应倍率。
        Vec4::from_array(value.atlas_uv),          // 当前纹理页 UV 切片。
        Vec4::from_array(value.root_color),        // 线性根部综合色。
        Vec4::from_array(value.tip_color),         // 线性尖端综合色。
        Vec4::from_array(value.cool_tint),         // 冷色个体变化与粗级覆盖标志。
        Vec4::from_array(value.warm_tint),         // 暖色个体变化与作物远模标志。
    )
}

/// 把目录中的唯一运动来源原子写入 GPU 外观。
fn apply_motion(
    appearance: GpuBladeProfile, // 当前数据预设已经确定的几何和颜色外观。
    motion: &BladePresetMotion,  // 每个数据Blade都必须明确声明世界风或水流。
) -> Result<GpuBladeProfile, String> {
    match motion {
        BladePresetMotion::Wind => Ok(appearance.with_wind()), // 风驱动清除外观中可能残留的水流参数。
        BladePresetMotion::WaterFlow {
            direction,
            speed,
            tip_strength,
            spatial_scale,
        } => {
            let direction = Vec2::from_array(*direction); // 先转成 GPU 世界 X/Z 向量。
            let length = direction.length(); // 目录已校验，仍防御直接调用传入零或非有限向量。
            if !length.is_finite() || length <= 0.000_001 {
                return Err("Blade 水流方向必须是有限非零向量".to_owned()); // 不允许 WGSL normalize 产生 NaN。
            }
            Ok(appearance.with_water_flow(
                direction / length, // CPU 归一化一次，数百万草叶无需重复处理内容错误。
                *speed,             // 独立水流动画速度。
                *tip_strength,      // 独立草尖水流摆幅。
                *spatial_scale,     // 独立波长倍率。
            ))
        }
    }
}

/// 把目录超远预算枚举映射到公共 GPU 后端。
const fn ultra_budget_from_data(value: BladePresetUltraLodBudget) -> GpuBladeUltraLodBudget {
    match value {
        BladePresetUltraLodBudget::High => GpuBladeUltraLodBudget::High, // 二乘二父格一个代理。
        BladePresetUltraLodBudget::Dense => GpuBladeUltraLodBudget::Dense, // 三乘三父格一个代理。
        BladePresetUltraLodBudget::Medium => GpuBladeUltraLodBudget::Medium, // 四乘四父格一个代理。
        BladePresetUltraLodBudget::Low => GpuBladeUltraLodBudget::Low,   // 八乘八父格一个代理。
    }
}

/// 把目录完整视图 LOD 转成公共纯值配置。
fn view_lod_from_data(value: &BladePresetViewLod) -> GpuBladeViewLodConfig {
    GpuBladeViewLodConfig::from_slice(
        &value
            .pixel_thresholds
            .iter()
            .map(|pair| Vec2::from_array(*pair))
            .collect::<Vec<_>>(), // 目录细→粗阈值对齐公共配置顺序。
        Vec2::from_array(value.camera_fade_distances), // 近镜头整株避让。
        value.reference_grass_height,                  // 投影估算参考株高。
        value.finest_grid_spacing,                     // 各档共享最细世界母格。
        &value.residency_radii,                        // 每档相机安全半径。
        value.residency_margin,                        // 子级请求额外安全边距。
        &value.residency_transition_widths,            // 相邻档向祖先代理交还宽度。
    )
    .expect("目录校验已保证视图 LOD 与档数对齐") // 防御绕过目录校验的直接测试调用。
}

#[cfg(test)]
mod tests {
    use super::*; // 纯运动转换测试不启动 RenderApp 或请求真实资产。

    #[test]
    fn water_flow_is_normalized_and_does_not_use_world_wind() {
        let motion = BladePresetMotion::WaterFlow {
            direction: [3.0, 4.0], // 使用可精确验证的三四五向量。
            speed: 0.7,            // 独立动画速度。
            tip_strength: 0.12,    // 独立米制摆幅。
            spatial_scale: 0.8,    // 独立空间频率。
        };
        let profile = apply_motion(GpuBladeProfile::DEFAULT, &motion).expect("合法水流可转换");
        assert!(profile.uses_water_flow()); // 驱动位必须选择水流分支。
        assert_eq!(profile.motion.y, 0.6); // X 流向被归一化。
        assert_eq!(profile.motion.z, 0.8); // Z 流向被归一化。
        assert_eq!(profile.motion.w, 0.7); // 速度不读取世界风。
        assert_eq!(profile.motion_params.x, 0.12); // 摆幅不读取 F1 风力滑块。
    }
}
