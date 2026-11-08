//! 父/子地图正式材质身份、临时背面覆盖和有界开关恢复回归。

use super::{components::TerrainBackfacesRenderMaterial, resources::TerrainBackfacesPreview};
use crate::TerrainGridTarget;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

fn app() -> App {
    let mut app = App::new();
    app.init_resource::<Assets<StandardMaterial>>();
    app.add_message::<AssetEvent<StandardMaterial>>();
    super::install(&mut app);
    app
}

#[test]
fn terrain_backfaces_parent_child_share_preview_and_restore_original_identity() {
    let mut app = app();
    let original = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let parent = app
        .world_mut()
        .spawn((TerrainGridTarget, MeshMaterial3d(original.clone())))
        .id();
    let child = app
        .world_mut()
        .spawn((TerrainGridTarget, MeshMaterial3d(original.clone())))
        .id();
    let prop = app.world_mut().spawn(MeshMaterial3d(original.clone())).id();
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = true;
    app.update();
    let world = app.world();
    let overlay = world.get::<TerrainBackfacesRenderMaterial>(parent).unwrap();
    assert_eq!(
        overlay.displayed,
        world
            .get::<TerrainBackfacesRenderMaterial>(child)
            .unwrap()
            .displayed
    );
    assert!(world.get::<TerrainBackfacesRenderMaterial>(prop).is_none());
    assert_eq!(
        world
            .get::<MeshMaterial3d<StandardMaterial>>(parent)
            .unwrap()
            .0,
        original
    );
    let materials = world.resource::<Assets<StandardMaterial>>();
    assert!(!materials.get(&original).unwrap().double_sided);
    let temporary = materials
        .get(overlay.displayed.typed::<StandardMaterial>())
        .unwrap();
    assert!(temporary.double_sided);
    assert!(temporary.cull_mode.is_none());
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = false;
    app.update();
    assert!(
        !app.world()
            .resource::<TerrainBackfacesPreview>()
            .is_restoring()
    );
    assert_eq!(
        app.world()
            .get::<TerrainBackfacesRenderMaterial>(parent)
            .unwrap()
            .displayed,
        original.id().untyped()
    );
}

#[test]
fn terrain_backfaces_late_targets_and_material_replacement_follow_current_source() {
    let mut app = app();
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = true;
    app.update();
    let old = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let target = app
        .world_mut()
        .spawn((TerrainGridTarget, MeshMaterial3d(old)))
        .id();
    app.update();
    let new = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            double_sided: true,
            cull_mode: None,
            ..default()
        });
    app.world_mut()
        .entity_mut(target)
        .insert(MeshMaterial3d(new.clone()));
    app.update();
    assert_eq!(
        app.world()
            .get::<TerrainBackfacesRenderMaterial>(target)
            .unwrap()
            .original,
        new.id().untyped()
    );
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = false;
    app.update();
    assert_eq!(
        app.world()
            .get::<TerrainBackfacesRenderMaterial>(target)
            .unwrap()
            .displayed,
        new.id().untyped()
    );
    assert!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(&new)
            .unwrap()
            .double_sided
    );
    app.world_mut().despawn(target);
    app.update();
    assert!(
        !app.world()
            .resource::<TerrainBackfacesPreview>()
            .is_restoring()
    );
}

#[test]
fn terrain_backfaces_toggle_off_restores_with_bounded_work() {
    let mut app = app();
    let original = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    for _ in 0..150 {
        app.world_mut()
            .spawn((TerrainGridTarget, MeshMaterial3d(original.clone())));
    }
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = true;
    for _ in 0..6 {
        app.update();
    }
    assert_eq!(
        app.world()
            .resource::<TerrainBackfacesPreview>()
            .active_targets,
        150
    );
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = false;
    app.update();
    assert_eq!(
        app.world()
            .resource::<TerrainBackfacesPreview>()
            .active_targets,
        86
    );
    assert!(
        app.world()
            .resource::<TerrainBackfacesPreview>()
            .is_restoring()
    );
    for _ in 0..3 {
        app.update();
    }
    assert!(
        !app.world()
            .resource::<TerrainBackfacesPreview>()
            .is_restoring()
    );
}


#[test]
fn terrain_backfaces_only_refreshes_render_inputs_and_restores_temporary_tags() {
    let mut app = app();
    let original = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let tagged = app
        .world_mut()
        .spawn((
            TerrainGridTarget,
            Mesh3d(Handle::default()),
            MeshMaterial3d(original.clone()),
            MeshTag::new(42),
        ))
        .id();
    let untagged = app
        .world_mut()
        .spawn((
            TerrainGridTarget,
            Mesh3d(Handle::default()),
            MeshMaterial3d(original.clone()),
        ))
        .id();
    app.update();
    let mesh_tick = app
        .world()
        .entity(tagged)
        .get_ref::<Mesh3d>()
        .unwrap()
        .last_changed();
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = true;
    app.update();
    assert_eq!(
        app.world()
            .entity(tagged)
            .get_ref::<Mesh3d>()
            .unwrap()
            .last_changed(),
        mesh_tick
    );
    assert_eq!(app.world().get::<MeshTag>(tagged).unwrap().value, 42);
    assert_eq!(app.world().get::<MeshTag>(untagged).unwrap().value, 0);
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = false;
    app.update();
    assert_eq!(app.world().get::<MeshTag>(tagged).unwrap().value, 42);
    assert!(app.world().get::<MeshTag>(untagged).is_none());
    assert_eq!(
        app.world()
            .entity(tagged)
            .get_ref::<Mesh3d>()
            .unwrap()
            .last_changed(),
        mesh_tick
    );
    // 关闭期间不刷新临时资产；再次开启使用最新源颜色。
    app.world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .get_mut(&original)
        .unwrap()
        .base_color = Color::srgb(0.8, 0.1, 0.2);
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<StandardMaterial>>>()
        .write(AssetEvent::Modified { id: original.id() });
    let changed = app
        .world()
        .resource_ref::<Assets<StandardMaterial>>()
        .last_changed();
    app.update();
    assert_eq!(
        app.world()
            .resource_ref::<Assets<StandardMaterial>>()
            .last_changed(),
        changed
    );
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = true;
    app.update();
    let displayed = app
        .world()
        .get::<TerrainBackfacesRenderMaterial>(tagged)
        .unwrap()
        .displayed
        .typed::<StandardMaterial>();
    assert_eq!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(displayed)
            .unwrap()
            .base_color,
        Color::srgb(0.8, 0.1, 0.2)
    );
}

#[test]
fn terrain_backfaces_hot_updates_survive_message_expiry_and_stable_frames_do_not_copy_assets() {
    let mut app = app();
    let mut originals = Vec::new();
    let mut entities = Vec::new();
    for _ in 0..130 {
        let original = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        entities.push(
            app.world_mut()
                .spawn((TerrainGridTarget, MeshMaterial3d(original.clone())))
                .id(),
        );
        originals.push(original);
    }
    app.world_mut()
        .resource_mut::<TerrainBackfacesPreview>()
        .enabled = true;
    for _ in 0..6 {
        app.update();
    }
    for original in &originals {
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(original)
            .unwrap()
            .base_color = Color::srgb(0.7, 0.2, 0.1);
        app.world_mut()
            .resource_mut::<Messages<AssetEvent<StandardMaterial>>>()
            .write(AssetEvent::Modified { id: original.id() });
    }
    app.update(); // 本帧只复制64份；其余通知必须已合并到持久队列。
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<StandardMaterial>>>()
        .update();
    app.world_mut()
        .resource_mut::<Messages<AssetEvent<StandardMaterial>>>()
        .update();
    for _ in 0..4 {
        app.update();
    }
    for entity in entities {
        let displayed = app
            .world()
            .get::<TerrainBackfacesRenderMaterial>(entity)
            .unwrap()
            .displayed
            .typed::<StandardMaterial>();
        assert_eq!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(displayed)
                .unwrap()
                .base_color,
            Color::srgb(0.7, 0.2, 0.1)
        );
    }
    let changed = app
        .world()
        .resource_ref::<Assets<StandardMaterial>>()
        .last_changed();
    for _ in 0..8 {
        app.update();
    }
    assert_eq!(
        app.world()
            .resource_ref::<Assets<StandardMaterial>>()
            .last_changed(),
        changed
    );
}
