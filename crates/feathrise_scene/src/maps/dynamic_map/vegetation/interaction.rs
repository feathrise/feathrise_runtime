//! 动态地图GPU叶片与玩家的世界空间交互适配。
//!
//! 【功能设计】所有动态地图Blade层共享“当前玩家 + 七个短寿命足迹”固定容量采样器，
//! 植被种类与LOD由数据预设决定，交互只依赖统一世界坐标，
//! 也不把玩家速度、逐株状态或地图局部坐标塞进植被根。

use bevy::prelude::*; // 系统参数只读取唯一玩家、时间和两份固定大小资源。
use crate::VegetationInteractionSource; // 真实玩家物理根是唯一压草来源。
use feathrise_environment::vegetation::blade::GpuBladeInteraction; // 全部Blade窗口共享固定176字节风场、相位与足迹uniform。

use crate::gpu_grass_content::trail::BladeTrailState;
use feathrise_environment::vegetation::blade::{GpuBladeInteractionSource, GPU_GRASS_INTERACTION_SOURCE_CAPACITY};

/// 进入、切离动态地图时同时清空CPU路径和GPU源，禁止相邻世界坐标之间串出旧足迹。
pub(in crate::maps::dynamic_map) fn reset_dynamic_blade_interaction(
    mut trail: ResMut<BladeTrailState>, // 七个历史槽与两份位置历史都属于当前地图会话。
    mut interaction: ResMut<GpuBladeInteraction>, // 只清局部源，保留F1设置的统一世界风。
) {
    trail.clear(); // 下一次出现唯一玩家时从其绝对世界X/Z重新建立路径。
    interaction.clear_sources(); // RenderApp下一帧不再读取上一地图玩家或足迹。
}

/// 把动态地图中的真实玩家位置接回正式Blade玩家压草链路。
pub(in crate::maps::dynamic_map) fn update_dynamic_blade_interaction(
    time: Res<Time>, // 只用游戏时间推进七个固定足迹槽的回弹寿命。
    players: Query<&Transform, With<VegetationInteractionSource>>, // 玩家物理根 X/Z 决定当前压草源和移动采样路径。
    mut interaction: ResMut<GpuBladeInteraction>, // 覆盖本帧最多八个世界空间源，不触碰逐草数据。
    mut trail: ResMut<BladeTrailState>, // 地图私有资源跨帧保存足迹，并由进入/离开系统显式清零。
) {
    let Ok(player_transform) = players.single() else {
        trail.clear(); // 玩家尚未生成或切图清理时丢弃旧路径，防止重进地图连接两处位置。
        interaction.clear_sources(); // RenderApp 下一帧只保留世界风场，不再执行局部足迹循环。
        return; // 没有唯一玩家时不猜测交互中心。
    };
    let player_xz = player_transform.translation.xz(); // 跳跃高度不改变地面草叶的水平踩踏范围。
    trail.update(player_xz, time.delta_secs()); // 固定槽按真实移动距离补点并推进一点四秒回弹寿命。
    let mut gpu_sources =
        [GpuBladeInteractionSource::default(); GPU_GRASS_INTERACTION_SOURCE_CAPACITY]; // 栈上固定八槽不会产生每帧堆分配。
    let source_count = trail.write_gpu_sources(player_xz, &mut gpu_sources); // 第一槽为当前玩家，其余为仍有效足迹。
    interaction.set_sources(&gpu_sources[..source_count]); // 更新包围圆和连续源前缀供顶点 shader 局部快速判断。
}

#[cfg(test)]
mod tests {
    use super::*; // 直接验证生产适配系统与生命周期清理函数。
    use feathrise_environment::vegetation::blade::GpuBladeInteractionSource; // 构造清理前的非零GPU源。

    #[test]
    fn dynamic_player_uploads_world_space_source_and_fixed_trail() {
        let mut app = App::new(); // 最小ECS应用即可验证CPU到固定GPU uniform的生产链。
        app.init_resource::<Time>()
            .init_resource::<GpuBladeInteraction>()
            .init_resource::<BladeTrailState>()
            .add_systems(Update, update_dynamic_blade_interaction); // 运行动态地图实际注册的适配系统。
        let player = app
            .world_mut()
            .spawn((
                VegetationInteractionSource,                         // 唯一真实玩家标记。
                Transform::from_xyz(-137.25, 6.0, 284.5), // 同时覆盖负X和非零Z，禁止误用贴图局部坐标。
            ))
            .id();

        app.update(); // 首帧应立即上传玩家当前位置，不必等待移动。
        assert_eq!(
            app.world()
                .resource::<GpuBladeInteraction>()
                .active_source_count(),
            1
        ); // 静止玩家也持续压弯脚边叶片。

        app.world_mut()
            .get_mut::<Transform>(player)
            .expect("测试玩家应仍存在")
            .translation
            .x += 1.4; // 一次移动跨过两个正式0.65米采样间距。
        app.update();
        assert_eq!(
            app.world()
                .resource::<GpuBladeInteraction>()
                .active_source_count(),
            3
        ); // 一个实时玩家源加两个历史回弹足迹。

        app.world_mut().despawn(player); // 玩家重建间隙必须立即撤销旧位置与历史路径。
        app.update();
        assert_eq!(
            app.world()
                .resource::<GpuBladeInteraction>()
                .active_source_count(),
            0
        ); // 没有唯一玩家时Shader下一帧应直接跳过局部交互。
        app.world_mut().spawn((
            VegetationInteractionSource,
            Transform::from_xyz(-136.0, 6.0, 284.5), // 故意靠近旧位置，证明清理不依赖十二米传送阈值。
        ));
        app.update();
        assert_eq!(
            app.world()
                .resource::<GpuBladeInteraction>()
                .active_source_count(),
            1
        ); // 新玩家只建立实时源，不能把旧地图或旧实体的两个足迹接回来。
    }

    #[test]
    fn dynamic_interaction_reset_clears_sources_but_keeps_world_wind() {
        let mut app = App::new(); // 独立验证进图和离图共同使用的无副作用清理边界。
        app.init_resource::<GpuBladeInteraction>()
            .init_resource::<BladeTrailState>()
            .add_systems(Update, reset_dynamic_blade_interaction);
        {
            let mut interaction = app.world_mut().resource_mut::<GpuBladeInteraction>();
            interaction.set_wind_tip_strength(0.27); // 模拟用户通过F1设置的全局草地风力。
            interaction.set_sources(&[GpuBladeInteractionSource::new(
                Vec2::new(8.0, -3.0),
                1.05,
                0.95,
            )]); // 模拟上一张地图尚有一个玩家源。
        }

        app.update(); // 进入或离开动态地图时执行同一重置。

        let interaction = app.world().resource::<GpuBladeInteraction>();
        assert_eq!(interaction.active_source_count(), 0); // 旧玩家与足迹不得串入下一张地图。
        assert!((interaction.wind_tip_strength() - 0.27).abs() < 0.000_1); // 清理局部交互不能覆盖统一世界风设置。
    }
}
