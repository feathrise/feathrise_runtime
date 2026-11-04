//! 显式验收读取实际模型线框相机、预通道和HUD布局；UI布尔值仅作为请求对照。

use super::{components::{ToolbarCamera, ToolbarRoot}, resources::PreviewToolbarState};
use avian3d::debug_render::PhysicsGizmos;
use bevy::{ecs::system::SystemParam, gizmos::config::GizmoConfigStore, prelude::*};
use bevy::core_pipeline::prepass::{DepthPrepass, NormalPrepass};
use feathrise_scene::author_view::{GridView, PrepassLease, ShadingView};
use feathrise_scene_core::view::PrimarySceneView;

#[derive(Clone, Default, PartialEq, serde::Serialize)]
pub(in crate::project::embedded) struct RuntimeEvidence {
    pub source: &'static str,
    /// 明确区分模型三角线与旧无限参考网格，旧报告不能代替本轮采用证据。
    pub grid_kind: &'static str,
    pub active: bool,
    pub requested_grid: bool,
    pub requested_collision: bool,
    pub primary_camera: Option<u64>,
    pub actual_grid: Option<[[f32; 4]; 4]>,
    pub actual_shading: Option<ShadingEvidence>,
    pub actual_prepass_lease: bool,
    pub actual_depth_prepass: bool,
    pub actual_normal_prepass: bool,
    pub actual_msaa: Option<String>,
    pub actual_collision: Option<bool>,
    pub target_camera: Option<u64>,
    pub fallback_camera_active: bool,
    pub hud_rect: Option<[f32; 4]>,
}

#[derive(Clone, PartialEq, serde::Serialize)]
pub(in crate::project::embedded) struct ShadingEvidence {
    pub options: [f32; 4],
    pub wire: [f32; 4],
}

type CameraState = (Entity, Option<GridView>, Option<ShadingView>, bool, bool, bool, Option<Msaa>);

fn snapshot(state: Option<&PreviewToolbarState>, camera: Option<CameraState>,
    store: Option<&GizmoConfigStore>, target: Option<Entity>, fallback_active: bool,
    hud_rect: Option<[f32; 4]>) -> RuntimeEvidence {
    RuntimeEvidence {
        source: "production-PrimarySceneView-MeshTriangles-and-PhysicsGizmos",
        grid_kind: "mesh_triangles",
        active: state.is_some_and(|state| state.baseline.is_some()),
        requested_grid: state.is_some_and(|state| state.grid),
        requested_collision: state.is_some_and(|state| state.collision),
        primary_camera: camera.map(|value| value.0.to_bits()),
        actual_grid: camera.and_then(|value| value.1).map(|grid| [grid.origin_stride.to_array(),
            grid.plane.to_array(), grid.axis_u.to_array(), grid.axis_v.to_array()]),
        actual_shading: camera.and_then(|value| value.2).map(|view| ShadingEvidence {
            options: view.options.to_array(), wire: view.wire.to_array(),
        }),
        actual_prepass_lease: camera.is_some_and(|value| value.3),
        actual_depth_prepass: camera.is_some_and(|value| value.4),
        actual_normal_prepass: camera.is_some_and(|value| value.5),
        actual_msaa: camera.and_then(|value| value.6).map(|msaa| format!("{msaa:?}")),
        actual_collision: store.and_then(|store| store.get_config::<PhysicsGizmos>()
            .map(|(config, _)| config.enabled)),
        target_camera: target.map(Entity::to_bits),
        fallback_camera_active: fallback_active,
        hud_rect,
    }
}

fn bounds(node: &ComputedNode, transform: &UiGlobalTransform) -> Option<[f32; 4]> {
    let rect = Rect::from_center_size(transform.translation, node.size());
    (!node.is_empty()).then_some([rect.min.x, rect.min.y, rect.max.x, rect.max.y])
}

pub(in crate::project::embedded) fn runtime_evidence(world: &mut World) -> RuntimeEvidence {
    // 唯一主相机是既有稳定标记；不读取网格资产、不扫描碰撞体，也不把UI副本冒充实际采用。
    let camera = world.query_filtered::<(Entity, Option<&GridView>, Option<&ShadingView>,
        Has<PrepassLease>, Has<DepthPrepass>, Has<NormalPrepass>, Option<&Msaa>), With<PrimarySceneView>>()
        .single(world).ok().map(|(entity, grid, shading, lease, depth, normal, msaa)|
            (entity, grid.copied(), shading.copied(), lease, depth, normal, msaa.copied()));
    let root = world.query_filtered::<(&UiTargetCamera, &ComputedNode, &UiGlobalTransform), With<ToolbarRoot>>()
        .single(world).ok().map(|(camera, node, transform)| (camera.0, bounds(node, transform)));
    let fallback_active = world.query_filtered::<&Camera, With<ToolbarCamera>>()
        .iter(world).any(|camera| camera.is_active);
    snapshot(world.get_resource::<PreviewToolbarState>(), camera, world.get_resource::<GizmoConfigStore>(),
        root.map(|value| value.0), fallback_active, root.and_then(|value| value.1))
}

#[derive(SystemParam)]
pub(in crate::project::embedded) struct RuntimeEvidenceQuery<'w, 's> {
    state: Option<Res<'w, PreviewToolbarState>>,
    cameras: Query<'w, 's, (Entity, Option<&'static GridView>, Option<&'static ShadingView>,
        Has<PrepassLease>, Has<DepthPrepass>, Has<NormalPrepass>, Option<&'static Msaa>), With<PrimarySceneView>>,
    store: Option<Res<'w, GizmoConfigStore>>,
    roots: Query<'w, 's, (&'static UiTargetCamera, &'static ComputedNode, &'static UiGlobalTransform), With<ToolbarRoot>>,
    fallback: Query<'w, 's, &'static Camera, With<ToolbarCamera>>,
}
impl RuntimeEvidenceQuery<'_, '_> {
    pub(in crate::project::embedded) fn current(&self) -> RuntimeEvidence {
        let camera = self.cameras.single().ok().map(|(entity, grid, shading, lease, depth, normal, msaa)|
            (entity, grid.copied(), shading.copied(), lease, depth, normal, msaa.copied()));
        let root = self.roots.single().ok().map(|(camera, node, transform)| (camera.0, bounds(node, transform)));
        let fallback_active = self.fallback.iter().any(|camera| camera.is_active);
        snapshot(self.state.as_deref(), camera, self.store.as_deref(), root.map(|value| value.0),
            fallback_active, root.and_then(|value| value.1))
    }
}
