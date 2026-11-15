//! 工程视口直接安装原作者相机渲染器；预览归还附件租约，临时网格与碰撞开关归预览工具条。

use super::{EmbeddedProjectState, resources::Runtime};
use bevy::prelude::*;
use feathrise_editor_protocol::{AuthorTool, Mode};
use feathrise_scene::{
    ScenePlacementInstance, ScenePlacementPrimitiveVisual,
    author_view::{GridView, SelectionOutline, ShadingView, apply_shading, supports_selection_outline},
};
use feathrise_scene_content::ActiveDynamicScene;

pub(super) fn install(app: &mut App) {
    app.add_plugins((feathrise_scene::author_view::ShadingRenderPlugin,
        feathrise_scene::author_view::GridRenderPlugin));
    if !app.is_plugin_added::<avian3d::debug_render::PhysicsDebugPlugin>() {
        app.add_plugins(avian3d::debug_render::PhysicsDebugPlugin);
    }
    // 首条作者参数到达前保持关闭，不能短暂显示工程全部碰撞线框。
    if let Some(mut store) = app.world_mut().get_resource_mut::<bevy::gizmos::config::GizmoConfigStore>() {
        store.config_mut::<avian3d::debug_render::PhysicsGizmos>().0.enabled = false;
    }
    app.add_systems(PostUpdate, sync.after(super::systems::camera)
        .before(feathrise_settings::GraphicsRuntimeLeaseSystems::RestoreReleased)
        .before(bevy::transform::TransformSystems::Propagate));
}

pub(super) fn release(world: &mut World, runtime: &mut Runtime) {
    runtime.outlined_selection = None;
    if let Some(entity) = runtime.author_camera.take() {
        apply_shading(world, entity, None);
        if let Ok(mut camera) = world.get_entity_mut(entity) {
            camera.remove::<(GridView, SelectionOutline)>();
        }
    }
}

fn whole_placement<'a>(target: &'a str, scene_id: &str) -> Option<&'a str> {
    let (scene, placement) = target.strip_prefix("placement:")?.split_once('/')?;
    let id = placement.strip_suffix("//")?;
    (scene == scene_id && !id.is_empty() && !id.contains('/')).then_some(id)
}

fn builtin_selection(world: &World, runtime: &Runtime) -> Option<(usize, Entity)> {
    // 宿主只传稳定选择身份；工程内通过活动根索引取得自己的正式网格，避免扫描全图。
    let state = world.resource::<EmbeddedProjectState>();
    let preview = super::tools::visible(state, runtime)?;
    let scene = world.get_resource::<ActiveDynamicScene>()?.scene_id.as_ref()?;
    let (index, id) = preview.items.iter().enumerate().find_map(|(index, item)| {
        let AuthorTool::Selection(selection) = item else { return None; };
        whole_placement(&selection.target, scene.as_str()).map(|id| (index, id))
    })?;
    let root = super::preview::active_root(world, scene, id).ok()?;
    if !world.get::<ScenePlacementInstance>(root)?.asset_target().is_builtin()
        || world.get::<Visibility>(root) == Some(&Visibility::Hidden)
    {
        return None;
    }
    // 内置立方体及细分网格共享唯一的直接可视子节点。只在完整匹配时替换本条选框。
    let children = world.get::<Children>(root)?;
    if children.len() > 64 { return None; }
    let mut visuals = children.iter().filter(|child| world.get::<ScenePlacementPrimitiveVisual>(*child).is_some());
    let visual = visuals.next()?;
    if visuals.next().is_some() || !world.get::<InheritedVisibility>(visual)?.get() { return None; }
    let mesh = world.get_resource::<Assets<Mesh>>()?.get(&world.get::<Mesh3d>(visual)?.0)?;
    let material = world.get_resource::<Assets<StandardMaterial>>()?
        .get(&world.get::<MeshMaterial3d<StandardMaterial>>(visual)?.0)?;
    supports_selection_outline(mesh, material).then_some((index, visual))
}

pub(super) fn sync(world: &mut World, mut primary: Local<Option<QueryState<Entity, With<feathrise_scene_core::view::PrimarySceneView>>>>) {
    let Some(mut runtime) = world.remove_non_send::<Runtime>() else { return; };
    if let Some(view) = runtime.author_view {
        let editing = world.resource::<EmbeddedProjectState>().mode == Mode::Editing;
        if let Some(mut shading) = world.get_resource_mut::<feathrise_scene::TerrainShadingPreview>()
            && shading.flat != view.terrain_flat { shading.flat = view.terrain_flat; }
        if let Some(mut grid) = world.get_resource_mut::<feathrise_scene::TerrainGridPreview>()
            && grid.visible != view.terrain_grid { grid.visible = view.terrain_grid; }
        if let Some(mut backfaces) = world.get_resource_mut::<feathrise_scene::TerrainBackfacesPreview>()
            && backfaces.enabled != view.terrain_backfaces { backfaces.enabled = view.terrain_backfaces; }
        // 试玩碰撞开关不被宿主旧显示快照逐帧覆盖；回到编辑后仍采用最新作者显示选择。
        if editing && let Some(mut store) = world.get_resource_mut::<bevy::gizmos::config::GizmoConfigStore>()
            && store.get_config::<avian3d::debug_render::PhysicsGizmos>()
                .is_some_and(|(config, _)| config.enabled != view.physics_debug) {
            store.config_mut::<avian3d::debug_render::PhysicsGizmos>().0.enabled = view.physics_debug;
        }
        // 缓存查询原型，只追踪新增/移除相机，不为每帧重建遍历整个场景的QueryState。
        let camera = primary.get_or_insert_with(||world.query_filtered::<Entity,
            With<feathrise_scene_core::view::PrimarySceneView>>()).single(world).ok();
        if runtime.author_camera != camera || !editing { release(world, &mut runtime); }
        if editing && let Some(entity) = camera {
            let shading = ShadingView { options: Vec4::from_array(view.shading), wire: Vec4::from_array(view.wire) };
            apply_shading(world, entity, Some(shading));
            let selected = if shading.uses_work_background() { builtin_selection(world, &runtime) } else { None };
            let outline = selected.map(|(_, visual)| SelectionOutline::new(vec![visual]));
            if world.get::<SelectionOutline>(entity) != outline.as_ref() {
                if let Some(outline) = outline { world.entity_mut(entity).insert(outline); }
                else { world.entity_mut(entity).remove::<SelectionOutline>(); }
            }
            runtime.outlined_selection = selected.map(|(index, _)| index);
            let grid = view.grid.map(|[origin_stride, plane, axis_u, axis_v]| GridView {
                origin_stride:Vec4::from_array(origin_stride), plane:Vec4::from_array(plane),
                axis_u:Vec4::from_array(axis_u), axis_v:Vec4::from_array(axis_v) }.world_anchored());
            if world.get::<GridView>(entity).copied() != grid {
                if let Some(grid) = grid { world.entity_mut(entity).insert(grid); }
                else { world.entity_mut(entity).remove::<GridView>(); }
            }
            runtime.author_camera = Some(entity);
        }
    }
    world.insert_non_send(runtime);
}

#[cfg(test)]
mod tests {
    use super::whole_placement;

    #[test]
    fn outline_selects_only_whole_placements_of_the_current_scene() {
        assert_eq!(whole_placement("placement:SCENE-1/item//", "SCENE-1"), Some("item"));
        assert_eq!(whole_placement("placement:SCENE-2/item//", "SCENE-1"), None);
        assert_eq!(whole_placement("placement:SCENE-1/item/Node/Mesh", "SCENE-1"), None);
        assert_eq!(whole_placement("placement:SCENE-1/item/Node//", "SCENE-1"), None);
        assert_eq!(whole_placement("placement:SCENE-1///", "SCENE-1"), None);
        assert_eq!(whole_placement("water:SCENE-1/item//", "SCENE-1"), None);
    }
}

/// 验收读取真正参与提取的相机组件和生产资源，不能只回显宿主命令作为采用证据。
#[cfg(feature = "validation")]
pub(super) fn evidence(world: &mut World, requested: Option<feathrise_editor_protocol::AuthorViewState>) -> serde_json::Value {
    use bevy::core_pipeline::prepass::{DepthPrepass, NormalPrepass};
    let cameras = world.query_filtered::<Entity, With<feathrise_scene_core::view::PrimarySceneView>>()
        .iter(world).take(2).collect::<Vec<_>>();
    let camera = (cameras.len() == 1).then(|| {
        let entity = cameras[0];
        let shading = world.get::<ShadingView>(entity).map(|view|serde_json::json!({
            "options":view.options.to_array(), "wire":view.wire.to_array()}));
        let grid = world.get::<GridView>(entity).map(|view|[view.origin_stride.to_array(), view.plane.to_array(),
            view.axis_u.to_array(), view.axis_v.to_array()]);
        serde_json::json!({"entity":entity.to_bits(),"shading":shading,"grid":grid,
            "prepass_lease":world.get::<feathrise_scene::author_view::PrepassLease>(entity).is_some(),
            "msaa":world.get::<Msaa>(entity).map(|msaa|format!("{msaa:?}")),
            "depth_prepass":world.get::<DepthPrepass>(entity).is_some(),
            "normal_prepass":world.get::<NormalPrepass>(entity).is_some()})
    });
    serde_json::json!({"requested":requested,"camera":camera,
        "terrain_flat":world.get_resource::<feathrise_scene::TerrainShadingPreview>().map(|value|value.flat),
        "terrain_grid":world.get_resource::<feathrise_scene::TerrainGridPreview>().map(|value|value.visible),
        "terrain_backfaces":world.get_resource::<feathrise_scene::TerrainBackfacesPreview>().map(|value|value.enabled),
        "physics_debug":world.get_resource::<bevy::gizmos::config::GizmoConfigStore>()
            .and_then(|store|store.get_config::<avian3d::debug_render::PhysicsGizmos>().map(|(config,_)|config.enabled)),
        "source":"production-PrimarySceneView-components"})
}
