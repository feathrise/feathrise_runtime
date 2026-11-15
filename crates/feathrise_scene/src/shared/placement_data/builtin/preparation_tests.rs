//! 大几何安装复用正式后台队列，核对待更新、迟到拒收和真实碰撞。
use super::{ScenePlacementInstance, builtin_placement_visual_ready, builtin_visual_for_root, install_preparation, spawn_builtin_placement_visual};
use avian3d::prelude::Collider;
use bevy::{camera::primitives::Aabb, ecs::system::RunSystemOnce, prelude::*};
use feathrise_scene_content::{SceneId, ScenePlacementAssetTarget, ScenePlacementRecord};

#[path = "../../../../tests/fixtures/editable-chunk-lod-v1.rs"]
mod fixture;

fn install(app: &mut App, root: Entity, target: ScenePlacementAssetTarget) {
    let mut source: ScenePlacementRecord = serde_json::from_value(serde_json::json!({
        "id": "prepared-cube", "translation": [0,0,0], "yaw_degrees": 0, "scale": [2,1,3]
    })).unwrap();
    source.asset_target = target.clone();
    app.world_mut().entity_mut(root).insert(ScenePlacementInstance::new(&SceneId::parse("SCENE-builtins").unwrap(), &source));
    app.world_mut().run_system_once(move |mut commands: Commands| {
        assert!(spawn_builtin_placement_visual(&mut commands, root, target.clone()));
    }).unwrap();
}

#[test]
fn full_builtin_appearance_survives_small_and_async_geometry_swaps() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>();
    install_preparation(&mut app);
    let scale = Vec3::new(2.0, 1.0, 3.0);
    let root = app.world_mut().spawn((Transform::from_scale(scale), Visibility::Inherited)).id();
    let small = fixture::geometry();
    assert_eq!((small.vertex_count(), small.edges().len(), small.face_count()), (56, 108, 54));
    install(&mut app, root, ScenePlacementAssetTarget::EditableMesh {
        geometry: small, color_srgb: [160; 3], roughness_milli: 700, collision: true,
    });
    let original = builtin_visual_for_root(app.world(), root).expect("56点网格必须存在完整对象画面");
    assert!(builtin_placement_visual_ready(app.world(), root));
    assert!(app.world().get::<Aabb>(original).is_some());
    let target = |geometry, color_srgb| ScenePlacementAssetTarget::EditableMesh {
        geometry, color_srgb, roughness_milli: 700, collision: true,
    };
    install(&mut app, root, target(fixture::cube(12), [220, 90, 30]));
    app.update(); // 启动第一个真实worker，不提前替换待准备的原画面。
    assert_eq!(builtin_visual_for_root(app.world(), root), Some(original));
    assert!(!builtin_placement_visual_ready(app.world(), root));
    let author = fixture::cube(101); // 每边100次切割后的闭合立方体。
    assert_eq!((author.vertex_count(), author.face_count()), (61_208, 61_206));
    install(&mut app, root, target(author.clone(), [30, 140, 220]));
    let timeout = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !builtin_placement_visual_ready(app.world(), root) {
        app.update();
        if builtin_placement_visual_ready(app.world(), root) { break; }
        assert_eq!(builtin_visual_for_root(app.world(), root), Some(original), "未完成或迟到结果不能替换当前画面");
        assert!(std::time::Instant::now() < timeout);
        std::thread::sleep(std::time::Duration::from_millis(1)); // 真实后台编排，无伪造完成结果。
    }
    let world = app.world();
    let visual = builtin_visual_for_root(world, root).unwrap();
    assert_ne!(visual, original);
    assert!(world.get_entity(original).is_err());
    let mesh = world.resource::<Assets<Mesh>>().get(&world.get::<Mesh3d>(visual).unwrap().0).unwrap();
    let material_handle = world.get::<MeshMaterial3d<StandardMaterial>>(visual).unwrap();
    let material = world.resource::<Assets<StandardMaterial>>().get(&material_handle.0).unwrap();
    let collider = world.get::<Collider>(visual).unwrap();
    assert_eq!(material.base_color, Color::srgb_u8(30, 140, 220));
    assert_eq!(mesh.indices().unwrap().len(), author.triangles().len() * 3);
    assert_eq!(collider.shape().as_trimesh().unwrap().indices().len(), author.triangles().len());
    assert_eq!(collider.scale(), scale);
    assert_eq!(world.get::<Children>(root).unwrap().len(), 1);
}
