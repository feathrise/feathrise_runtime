//! GPU 叶片植被质量的主世界应用系统。

use bevy::prelude::*; // Query、Res 与 Vec3 等 ECS 常用类型。
use feathrise_environment::vegetation::blade::GpuBladeView; // 只接管明确标记的玩法 GPU 叶片视图。
use feathrise_settings::RenderSettings; // 读取持久化植被档及其三个固定倍率。

/// 幂等同步植被密度、LOD 阈值与作者范围倍率。
pub(super) fn apply_vegetation_quality(
    settings: Res<RenderSettings>, // 当前 UI 与设置文件共享的权威画质资源。
    mut views: Query<&mut GpuBladeView>, // 场景内有界的唯一玩法视图；纹理相机没有该标记。
) {
    let (density, lod_threshold, view_distance) =
        settings.graphics.vegetation_quality.runtime_multipliers(); // 每帧只推导三个栈上常量，不扫描场景、资产或逐草实例。
    let target = Vec3::new(density, lod_threshold, view_distance); // 组成可一次比较的目标向量。
    for mut view in &mut views {
        if view.quality_multipliers() == target {
            continue; // 稳定帧不触发组件变更检测或额外 RenderApp 同步。
        } // 完成当前玩法视图的幂等快速路径。
        view.set_quality_multipliers(density, lod_threshold, view_distance); // 新相机与切档都只更新三个有限倍率。
    } // 完成当前场景全部明确植被视图同步，正常场景只有一个。
} // 完成不会重建候选缓冲或密度纹理的运行时应用。

#[cfg(test)]
mod tests {
    use super::*; // 复用正式系统和组件类型构造最小 ECS 回归。
    use feathrise_settings::VegetationQuality; // 测试显式切换低、中、高和极高档。

    #[test]
    fn current_and_new_views_inherit_quality_without_stable_frame_rewrites() {
        let mut app = App::new(); // 最小主世界足以验证纯组件同步，不创建 RenderApp 或 GPU。
        app.init_resource::<RenderSettings>(); // 建立与正式插件一致的统一设置资源。
        app.add_systems(Update, apply_vegetation_quality); // 测试调度无需依赖完整场景插件。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .vegetation_quality = VegetationQuality::Low; // 首帧直接模拟已加载的低档设置。
        let first_view = app.world_mut().spawn(GpuBladeView::legacy()).id(); // 兼容视图也必须应用密度与范围倍率。
        app.update(); // 首帧同步现有玩法视图。
        assert_eq!(
            app.world()
                .get::<GpuBladeView>(first_view)
                .expect("首个玩法植被视图应仍存在")
                .quality_multipliers(),
            Vec3::new(0.55, 1.35, 0.60),
        ); // 低档必须精确命中设置模型声明的 GPU 倍率。

        app.world_mut().clear_trackers(); // 清除首帧合法改写产生的组件变更标记。
        app.update(); // 设置和视图均未变化的稳定帧再次运行同步。
        assert!(
            !app.world()
                .entity(first_view)
                .get_ref::<GpuBladeView>()
                .expect("稳定帧首个玩法植被视图应仍存在")
                .is_changed(),
            "稳定帧不得重复标记 GpuBladeView 变更",
        ); // 锁定不会持续触发提取或 GPU uniform 无意义更新的幂等契约。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .vegetation_quality = VegetationQuality::Ultra; // 场景运行中切换到极高档。
        let new_view = app.world_mut().spawn(GpuBladeView::legacy()).id(); // 模拟切换场景后同帧生成的新玩法相机。
        app.update(); // 同帧更新旧视图并接管新视图。
        let expected = Vec3::new(1.0, 0.8, 1.0); // 极高保持作者密度范围并延伸精细 LOD。
        assert_eq!(
            app.world()
                .get::<GpuBladeView>(first_view)
                .expect("旧玩法植被视图应继续存在")
                .quality_multipliers(),
            expected,
        ); // 已存在视图必须响应运行时切档。
        assert_eq!(
            app.world()
                .get::<GpuBladeView>(new_view)
                .expect("新玩法植被视图应完成接管")
                .quality_multipliers(),
            expected,
        ); // 后续生成视图必须直接继承当前持久植被档。
    } // 完成现有、新增与稳定帧三类 ECS 行为覆盖。
} // 完成植被质量主世界系统测试。
