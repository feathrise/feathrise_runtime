//! 缩放Clipmap首次安装与地表保存热替换的真实包装回归；作者纹素与共享源材质保持不变。

use super::*;
use feathrise_environment::terrain_surface::{TerrainSurfaceExtension, TerrainSurfaceIndexedDescriptor};

fn source() -> TerrainSurfaceMaterial {
    TerrainSurfaceMaterial {
        base: StandardMaterial::default(),
        extension: TerrainSurfaceExtension::new_indexed(TerrainSurfaceIndexedDescriptor {
            world_min: Vec2::splat(-50.0), world_max: Vec2::splat(50.0),
            weight_map: Handle::default(), index_map: Handle::default(),
            base_color_array: Handle::default(), normal_array: Handle::default(),
            arm_height_array: Handle::default(), layer_table: Handle::default(),
            layer_count: 1, zero_weight: Handle::default(),
        }),
    }
}

#[test]
fn scaled_clipmap_wraps_new_and_saved_surface_with_the_current_height_source_frame() {
    let mut app = App::new();
    app.init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<TerrainSurfaceMaterial>>()
        .init_resource::<Assets<ClipmapHeightMaterial>>()
        .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
        .init_resource::<SparseHeightPageRuntime>()
        .add_message::<AssetEvent<TerrainSurfaceMaterial>>()
        .add_systems(Update, sync_sparse_clipmap_surface);
    let matrix = Transform {
        translation: Vec3::new(63.0, 11.0, -27.0),
        rotation: Quat::from_euler(EulerRot::YXZ, 0.6, 0.2, -0.1),
        scale: Vec3::splat(42.362328),
    }.to_matrix();
    let mut extension = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::splat(-50.0), Vec2::splat(50.0)).unwrap(),
        UVec2::splat(800),
        &ClipmapFlatHeightPage { heights: Handle::default(), page_table: Handle::default() },
    ).unwrap();
    extension.set_source_frame(matrix);
    {
        let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>();
        pages.extension = Some(extension);
        pages.extension_revision = 1;
    }
    let surface = app.world_mut().resource_mut::<Assets<TerrainSurfaceMaterial>>().add(source());
    let display = app.world_mut().spawn((DynamicSparseClipmapPrimitive, MeshMaterial3d(surface.clone()))).id();
    app.update();
    let wrapper = app.world().get::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>(display).unwrap().0.clone();
    {
        let materials = app.world().resource::<Assets<SparseClipmapSurfaceMaterial>>();
        let material = materials.get(&wrapper).unwrap();
        assert_eq!(material.extension.params.source_to_world, matrix);
        assert_eq!(material.base.extension.source_frame(), matrix,
            "首次包装必须直接带完整姿态，不等待高度矩阵再次变化才修复地表");
    }
    assert_eq!(app.world().resource::<Assets<TerrainSurfaceMaterial>>().get(&surface).unwrap()
        .extension.source_frame(), Mat4::IDENTITY, "包装只改实例副本，不能污染共享作者材质");

    // 保存/刷涂发布新PBR句柄时，高度页和修订完全不变；必须复用外层并正确绑定新内层。
    let replacement = app.world_mut().resource_mut::<Assets<TerrainSurfaceMaterial>>().add(source());
    app.world_mut().entity_mut(display).insert(MeshMaterial3d(replacement.clone()));
    app.update();
    assert_eq!(app.world().get::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>(display).unwrap().0, wrapper);
    let material = app.world().resource::<Assets<SparseClipmapSurfaceMaterial>>().get(&wrapper).unwrap();
    assert_eq!(material.extension.params.source_to_world, matrix);
    assert_eq!(material.base.extension.source_frame(), matrix,
        "高度已是42倍时，保存后新地表仍应覆盖整张源图");
    assert_eq!(app.world().resource::<SparseHeightPageRuntime>().wrapped_source.as_ref(), Some(&replacement));
    let tick = app.world().resource_ref::<Assets<SparseClipmapSurfaceMaterial>>().last_changed();
    app.update();
    assert_eq!(app.world().resource_ref::<Assets<SparseClipmapSurfaceMaterial>>().last_changed(), tick);
}

#[test]
fn scaled_clipmap_display_candidate_adopts_height_and_surface_in_the_same_source_frame() {
    use crate::maps::dynamic_map::sparse_runtime::DisplayBuildIdentity;
    let mut app = App::new();
    app.init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<TerrainSurfaceMaterial>>()
        .init_resource::<Assets<ClipmapHeightMaterial>>()
        .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<DynamicMapRuntimeState>()
        .init_resource::<MapTerrainDisplayLodStatus>()
        .init_resource::<SparseClipmapRuntime>()
        .init_resource::<SparseHeightPageRuntime>()
        .add_systems(Update, drive_sparse_display_gpu_transaction);
    let surface = app.world_mut().resource_mut::<Assets<TerrainSurfaceMaterial>>().add(source());
    let display = app.world_mut().spawn((DynamicSparseClipmapPrimitive, Mesh3d(Handle::default()),
        MeshMaterial3d(surface))).id();
    let manifest = SparseHeightManifest::empty(MapId::parse("MAP-surface-frame").unwrap(),
        SparseHeightSpec::new([-50.0; 2], [50.0; 2], 0.125, -8.0, 16.0).unwrap()).unwrap();
    let plan = feathrise_map_content::compile_display_lod_plan(&manifest.spec,
        &feathrise_map_content::default_display_lods_for_spec(&manifest.spec).unwrap()).unwrap();
    let pages_identity = desired_identity(&manifest, display, 1, [400; 2], &plan, None, None).unwrap();
    let display_identity = DisplayBuildIdentity { map_id: manifest.map_id.clone(), root: display, plan };
    {
        let mut runtime = app.world_mut().resource_mut::<SparseClipmapRuntime>();
        runtime.desired = Some(display_identity.clone());
        runtime.applied = Some(display_identity);
    }
    let matrix = Transform::from_scale(Vec3::splat(42.362328)).to_matrix();
    let mut height = ClipmapHeightExtension::new_flat(
        ClipmapBounds::new(Vec2::splat(-50.0), Vec2::splat(50.0)).unwrap(), UVec2::splat(800),
        &ClipmapFlatHeightPage { heights: Handle::default(), page_table: Handle::default() },
    ).unwrap();
    height.set_source_frame(matrix);
    {
        let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>();
        pages.desired = Some(pages_identity.clone());
        pages.candidate = Some(RegisteredPageCandidate { identity: pages_identity, extension: height,
            atlas: None, cpu: None, registered_at: Instant::now() });
    }
    app.update(); // 先检查尚未采用的真实离屏候选，不能靠后续bind修补测试结果。
    let wrapper = match &app.world().resource::<SparseHeightPageRuntime>().transaction.as_ref().unwrap().material {
        DisplayCandidateMaterial::Surface { material, .. } => material.clone(),
        _ => panic!("真实地表来源必须创建组合PBR候选"),
    };
    {
        let materials = app.world().resource::<Assets<SparseClipmapSurfaceMaterial>>();
        let material = materials.get(&wrapper).unwrap();
        assert_eq!(material.extension.params.source_to_world, matrix);
        assert_eq!(material.base.extension.source_frame(), matrix);
    }
    app.update();
    app.update(); // 无RenderApp夹具仍通过生产两帧候选门，原子安装完整组合。
    assert_eq!(app.world().get::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>(display).unwrap().0, wrapper);
    let material = app.world().resource::<Assets<SparseClipmapSurfaceMaterial>>().get(&wrapper).unwrap();
    assert_eq!(material.base.extension.source_frame(), material.extension.params.source_to_world);
}
