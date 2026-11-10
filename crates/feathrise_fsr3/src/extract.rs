//! 从主世界提取合格 FSR 相机和同帧投影参数。

use crate::types::{ExtractedFsr3View, Fsr3RenderCleanup, Fsr3Upscaling}; // 公开请求与私有渲染快照。
use bevy::{
    camera::{Camera, Hdr, Projection}, // 活动状态、HDR 与透视参数。
    ecs::query::Has,                   // 不用 With<Hdr>，确保失去 HDR 时仍能清理旧状态。
    prelude::*,                        // Commands、Query、Time。
    render::{MainWorld, sync_world::RenderEntity}, // 主/渲染实体稳定映射。
    time::Real,                        // 不受游戏暂停缩放影响的真实帧间隔。
};

/// 提取当前活动透视 HDR 相机；主世界 reset 在成功复制后立即消费。
pub(crate) fn extract_fsr3_views(
    mut commands: Commands,            // 只对相机对应渲染实体增删固定数量组件。
    mut main_world: ResMut<MainWorld>, // Render Extract 的唯一主世界访问入口。
    cleanup_query: Query<Has<ExtractedFsr3View>>, // 识别上一帧仍有 FSR 状态的渲染实体。
) {
    let frame_time_ms = (main_world.resource::<Time<Real>>().delta_secs() * 1000.0).max(1.0); // 上游拒绝小于1ms。
    let mut cameras = main_world.query::<(
        RenderEntity,
        &Camera,
        &Projection,
        Has<Hdr>,
        Option<&mut Fsr3Upscaling>,
    )>(); // 枚举有限相机集合；HDR 是显式资格位而非过滤器，避免关闭帧遗漏清理。

    for (render_entity, camera, projection, hdr, mut fsr) in cameras.iter_mut(&mut main_world) {
        let mut entity = commands
            .get_entity(render_entity)
            .expect("FSR 相机实体必须已同步到 RenderApp"); // 对齐 Bevy 官方 DLSS 提取前提。
        let perspective = match projection {
            Projection::Perspective(value)
                if value.near.is_finite()
                    && value.near >= 0.075
                    && value.fov.is_finite()
                    && value.fov > 0.0
                    && value.fov <= std::f32::consts::PI =>
            {
                Some(value) // 先过滤上游确定会拒绝的深度与视野参数，避免低分辨率主通道后才失败。
            }
            _ => None, // 正交、非法投影、UI、场景和预览相机保持作者路径。
        };
        if camera.is_active
            && hdr
            && let (Some(fsr), Some(perspective)) = (fsr.as_deref_mut(), perspective)
        {
            entity.insert(ExtractedFsr3View {
                settings: fsr.clone(),         // reset 必须与本帧其他参数原子提取。
                camera_near: f32::INFINITY,    // DEPTH_INVERTED + DEPTH_INFINITE 的 FSR 验证契约。
                camera_far: perspective.near, // 必须与真实投影一致；过小近裁面交给上游验证并安全回退。
                camera_fov_y: perspective.fov, // 使用 Bevy 实际垂直视野。
                frame_time_ms,                // 传入真实毫秒而不是游戏时间倍率。
            });
            fsr.reset = false; // reset 只影响已提取的这一帧，稳定帧不得重复清历史。
        } else if cleanup_query.get(render_entity) == Ok(true) {
            entity.remove::<Fsr3RenderCleanup>(); // 关闭、停用或投影改变时归还全部 RenderApp 状态。
        }
    }
}
