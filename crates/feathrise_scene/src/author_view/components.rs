//! 跨作者宿主与工程的固定相机Uniform；句柄、管线和实际GPU生命周期由原实现管理。
use bevy::{
    prelude::*,
    render::{extract_component::ExtractComponent, render_resource::ShaderType},
};

#[derive(Component, Clone, Copy, PartialEq, ExtractComponent, ShaderType)]
#[extract_app(bevy::render::RenderApp)]
pub struct ShadingView {
    pub options: Vec4, // 线框/实体/材质/渲染、轮廓增强、法线颜色、显示背景。
    pub wire: Vec4,    // 穿透、工作室光源、强制显示模型三角拓扑；末字段保留16字节对齐。
}

impl ShadingView {
    /// 游戏预览检查真实模型三角网格；不隐藏作者多边形面内对角线，也不附加无限参考网格。
    pub fn mesh_triangles() -> Self {
        Self { options: Vec4::new(0.0, 1.0, 0.0, 1.0), wire: Vec4::new(0.0, 0.0, 1.0, 0.0) }
    }
}

#[derive(Component, Clone, Copy, PartialEq, ExtractComponent, ShaderType)]
#[extract_app(bevy::render::RenderApp)]
pub struct GridView {
    pub origin_stride: Vec4,
    pub plane: Vec4,
    pub axis_u: Vec4,
    pub axis_v: Vec4,
}

impl GridView {
    /// 固定世界y=0的水平参考面，不依赖当前资产、相对吸附原点或观察目标。
    pub fn horizontal_world(step: f32) -> Self {
        Self {
            origin_stride: Vec3::ZERO.extend(if step.is_finite() && step > 0.0 { step } else { 1.0 }),
            plane: Vec3::Y.extend(0.0),
            axis_u: Vec4::X,
            axis_v: Vec4::Z,
        }
    }

    /// 两种作者视口共用世界参考网格：相对吸附原点与手柄高度都不能移动网格或主轴。
    pub fn world_anchored(mut self) -> Self {
        self.origin_stride = Vec3::ZERO.extend(self.origin_stride.w);
        self.plane.w = 0.0;
        self
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    #[test]
    fn world_grid_plane_and_main_axes_stay_at_the_world_origin_for_all_views() {
        for (normal, axis_u, axis_v) in [
            (Vec3::Y, Vec3::X, Vec3::Z),
            (Vec3::Z, Vec3::X, Vec3::Y),
            (Vec3::X, Vec3::Z, Vec3::Y),
        ] {
            let point = axis_u * 12.0 + axis_v * -3.0;
            for focus in [Vec3::new(0.3, 4.25, -0.7), Vec3::new(1000.2, -91.0, 207.8)] {
                let view = GridView {
                    origin_stride: focus.extend(0.5),
                    plane: normal.extend(focus.dot(normal)),
                    axis_u: axis_u.extend(0.0),
                    axis_v: axis_v.extend(0.0),
                }.world_anchored();
                // 用不同相机射线击中同一世界点；网格坐标及两条主轴均不得继承对象或观察目标偏移。
                let camera = focus + normal * 200.0;
                let ray = point - camera;
                let distance = (view.plane.w - camera.dot(normal)) / ray.dot(normal);
                let hit = camera + ray * distance;
                let offset = hit - view.origin_stride.truncate();
                let coordinate = Vec2::new(offset.dot(axis_u), offset.dot(axis_v)) / view.origin_stride.w;
                assert!(coordinate.distance(Vec2::new(24.0, -6.0)) < 0.001);
                assert_eq!(view.origin_stride, Vec4::new(0.0, 0.0, 0.0, 0.5));
                assert_eq!(view.plane, normal.extend(0.0));
                assert!(view.world_anchored() == view);
            }
        }
    }

    #[test]
    fn horizontal_world_fallback_keeps_valid_density_and_rejects_invalid_spacing() {
        for step in [0.1, 1.0, 100.0, 0.0, -1.0, f32::NAN, f32::INFINITY] {
            let grid = GridView::horizontal_world(step);
            assert_eq!(grid.origin_stride.truncate(), Vec3::ZERO);
            assert_eq!(grid.plane, Vec3::Y.extend(0.0));
            assert_eq!(grid.axis_u, Vec4::X);
            assert_eq!(grid.axis_v, Vec4::Z);
            assert_eq!(grid.origin_stride.w, if step.is_finite() && step > 0.0 { step } else { 1.0 });
        }
    }
}

#[derive(Component, Clone)]
pub(super) struct AmbientLease(pub Option<AmbientLight>);

/// 附件所有权保留原作者语义，离开该视图时只归还本模块取得的附件。
#[derive(Component, Clone, Copy)]
#[component(on_remove = release_prepass_graphics)]
pub struct PrepassLease {
    pub(super) msaa: Msaa,
    pub(super) depth: bool,
    pub(super) normal: bool,
    pub(super) graphics: Option<feathrise_settings::GraphicsRuntimeLeaseId>,
}

fn release_prepass_graphics(
    mut world: bevy::ecs::world::DeferredWorld,
    context: bevy::ecs::lifecycle::HookContext,
) {
    let ticket = world
        .get::<PrepassLease>(context.entity)
        .and_then(|lease| lease.graphics);
    if let Some(ticket) = ticket
        && let Some(mut registry) =
            world.get_resource_mut::<feathrise_settings::GraphicsRuntimeLeaseRegistry>()
    {
        // 正常归还与相机直接销毁都释放同一票据，不能在换相机后泄漏窗口覆盖。
        registry.release(ticket);
    }
}
