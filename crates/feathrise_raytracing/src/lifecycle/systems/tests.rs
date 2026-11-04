//! 生命周期超时、回退优先级与多资产原子恢复集成测试。

use bevy::{
    asset::RenderAssetUsages, // 构造主世界与渲染世界均可持有的测试Mesh。
    mesh::PrimitiveTopology,  // 测试资产不需要真实顶点，只验证句柄所有权。
};
use feathrise_environment::terrain_surface::{
    MAX_TERRAIN_SURFACE_LAYERS, TerrainSurfaceDescriptor, TerrainSurfaceExtension,
    TerrainSurfaceLayer, TerrainSurfaceMaterial, TerrainSurfaceMaterialOverride,
}; // 构造通用地表类型化测试资产。
use feathrise_settings::GraphicsRuntimeRequirements; // 构造必须随回退释放的真实租约票据。

use super::*; // 直接运行正式回退系统与超时判定。
use crate::types::{PreparedSourceMaterial, SourceCandidate}; // 构造完整材质与派生Mesh所有权事务。

#[test]
fn gameplay_camera_selection_rejects_image_targets_and_requires_one_window() {
    let first = Entity::from_raw_u32(1).expect("测试窗口相机实体合法");
    let second = Entity::from_raw_u32(2).expect("测试第二窗口相机实体合法");
    let texture = Entity::from_raw_u32(3).expect("测试纹理相机实体合法");
    let window_target = RenderTarget::Window(bevy::window::WindowRef::Primary);
    let image_target = RenderTarget::Image(Handle::<Image>::default().into());

    assert_eq!(
        unique_window_gameplay_camera(
            [(texture, &image_target), (first, &window_target)].into_iter()
        ),
        Some(first)
    ); // 误带PrimarySceneView的纹理视图不影响唯一玩法窗口选择。
    assert_eq!(
        unique_window_gameplay_camera(
            [(first, &window_target), (second, &window_target)].into_iter()
        ),
        None
    ); // 多窗口玩法身份含糊时必须阻断而不是任取一台。
    assert_eq!(
        unique_window_gameplay_camera([(texture, &image_target)].into_iter()),
        None
    );
}

/// 创建最小测试Mesh；几何内容不参与本组句柄恢复断言。
fn empty_mesh() -> Mesh {
    Mesh::new(
        PrimitiveTopology::TriangleList, // 与正式派生资产拓扑一致。
        RenderAssetUsages::all(),        // 同时保留主世界和渲染世界用途。
    )
}

/// 创建最小通用地表扩展材质，验证类型化句柄不会与StandardMaterial混用。
fn surface_material() -> TerrainSurfaceMaterial {
    let image = Handle::default(); // 测试只验证材质句柄所有权，无需加载真实纹理。
    TerrainSurfaceMaterial {
        base: StandardMaterial::default(), // 测试只关心原始/Deferred句柄恢复。
        extension: TerrainSurfaceExtension::new(TerrainSurfaceDescriptor {
            world_min: Vec2::ZERO,
            world_max: Vec2::ONE,
            weight_maps: [image.clone(), image.clone()],
            weight_map_count: 1,
            base_color_array: image.clone(),
            arm_array: image.clone(),
            normal_array: image.clone(),
            index_map: image,
            layer_table: Handle::default(),
            layers: [TerrainSurfaceLayer::default(); MAX_TERRAIN_SURFACE_LAYERS],
            layer_count: 1,
            residual_layer: 0,
            grass_layer: 0,
            grass_micro_normal_strength: 0.0,
            grass_variation_cell_meters: 8.0,
        }), // 空纹理句柄无需AssetServer。
    }
}

#[test]
fn only_cold_start_stages_are_subject_to_thirty_second_timeout() {
    assert!(stage_has_timeout(
        RaytracingPrototypeStage::DiscoveringCandidates
    ));
    assert!(stage_has_timeout(RaytracingPrototypeStage::AdaptingMeshes));
    assert!(stage_has_timeout(
        RaytracingPrototypeStage::AwaitingGpuAssets
    ));
    assert!(stage_has_timeout(RaytracingPrototypeStage::WarmingSolari));
    assert!(!stage_has_timeout(RaytracingPrototypeStage::Active));
    assert!(!stage_has_timeout(RaytracingPrototypeStage::Failed));
}

#[test]
fn timeout_starts_only_after_thirty_real_seconds() {
    assert!(!stage_timeout_exceeded(STAGE_TIMEOUT_SECONDS)); // 恰好30.0秒仍未越过硬门。
    assert!(stage_timeout_exceeded(STAGE_TIMEOUT_SECONDS + 0.1)); // 30.1秒必须立即请求回退。
}

#[test]
fn failed_rollback_overrides_a_same_frame_nonfatal_rollback() {
    let mut runtime = RaytracingRuntime::default();
    runtime.request_rollback(
        Some(RaytracingPrototypeBlocker::CameraRebuilt),
        RollbackDisposition::RasterReady,
    );
    runtime.request_rollback(
        Some(RaytracingPrototypeBlocker::AsyncTaskFailed {
            detail: "任务失败".to_string(),
        }),
        RollbackDisposition::Failed,
    );
    assert_eq!(
        runtime
            .pending_rollback
            .as_ref()
            .map(|pending| pending.disposition),
        Some(RollbackDisposition::Failed)
    );
}

#[test]
fn prepared_material_contract_rejects_session_deferred_handles_as_sources() {
    let mut standard_materials = Assets::<StandardMaterial>::default(); // 构造普通作者资产和本crate会话副本。
    let standard_source = standard_materials.add(StandardMaterial::default()); // GLB作者源句柄。
    let standard_deferred = standard_materials.add(StandardMaterial::default()); // 可见实体预热期临时句柄。
    let grass_proxy = standard_materials.add(StandardMaterial::default()); // 只供TLAS代理使用的标准材质。
    let mut surface_materials = Assets::<TerrainSurfaceMaterial>::default(); // 构造通用地表类型化资产对。
    let terrain_source = surface_materials.add(surface_material()); // 地形作者扩展材质。
    let terrain_deferred = surface_materials.add(surface_material()); // Solari会话扩展副本。
    let runtime = RaytracingRuntime {
        prepared_materials: vec![
            PreparedSourceMaterial {
                source_entity: Entity::PLACEHOLDER, // 本测试只核对类型化AssetId，不访问世界实体。
                binding: SourceMaterialBinding::Standard {
                    source: standard_source.clone(), // 热重载监视器必须继续识别作者源。
                    deferred: standard_deferred.clone(), // 会话副本的Modified事件必须被忽略。
                },
            },
            PreparedSourceMaterial {
                source_entity: Entity::PLACEHOLDER, // 草地同样不需要真实实体查询。
                binding: SourceMaterialBinding::TerrainSurface {
                    source: terrain_source.clone(), // 唯一允许触发地表源换代的作者资产。
                    deferred: terrain_deferred.clone(), // 兼容桥重新准备时不得触发整套回退。
                    proxy: grass_proxy.clone(),     // TLAS代理材质也不属于作者源。
                },
            },
        ], // 直接建立已完成原子准备的全部候选事务缩影。
        ..Default::default()
    };

    assert_eq!(
        prepared_standard_source_match(standard_source.id(), &runtime),
        Some(true)
    ); // 普通作者源热更仍必须整体重建。
    assert_eq!(
        prepared_standard_source_match(standard_deferred.id(), &runtime),
        Some(false)
    ); // 修复阶段与绑定状态来回跳的直接回归。
    assert_eq!(
        prepared_standard_source_match(grass_proxy.id(), &runtime),
        Some(false)
    ); // 代理准备事件不能污染可见源身份。
    assert_eq!(
        prepared_surface_source_match(terrain_source.id(), &runtime),
        Some(true)
    ); // 地表作者源仍保留热重载保护。
    assert_eq!(
        prepared_surface_source_match(terrain_deferred.id(), &runtime),
        Some(false)
    ); // 会话扩展副本的Modified事件不会再触发回退。
}

#[test]
fn rollback_restores_material_handles_and_releases_all_owned_assets_and_lease() {
    let mut app = App::new(); // 不安装RenderApp，直接验证主世界原子撤销边界。
    app.init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<StandardMaterial>>()
        .init_resource::<Assets<TerrainSurfaceMaterial>>()
        .init_resource::<GraphicsRuntimeLeaseRegistry>()
        .init_resource::<RaytracingRuntime>()
        .add_systems(Update, (apply_pending_rollback, ApplyDeferred).chain()); // 同帧落地全部恢复命令。

    let source_mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(empty_mesh()); // 保持源资产不被回退删除。
    let derived_mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(empty_mesh()); // 本crate拥有并应删除的派生资产。
    let standard_source = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default()); // 普通可见源作者句柄。
    let standard_deferred = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default()); // 普通可见源会话副本。
    let grass_proxy = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default()); // 草地TLAS代理专用标准材质。
    let terrain_source = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(surface_material()); // 通用地表作者扩展句柄。
    let terrain_deferred = app
        .world_mut()
        .resource_mut::<Assets<TerrainSurfaceMaterial>>()
        .add(surface_material()); // 通用地表会话Deferred副本。

    let standard_entity = app
        .world_mut()
        .spawn(MeshMaterial3d(standard_deferred.clone()))
        .id(); // 模拟活动期普通源已切到Deferred。
    let terrain_entity = app
        .world_mut()
        .spawn((
            MeshMaterial3d(terrain_deferred.clone()),
            TerrainSurfaceMaterialOverride::new(&terrain_source, &terrain_deferred),
        ))
        .id(); // 模拟活动期通用地表已切到扩展Deferred。
    let proxy_entity = app.world_mut().spawn_empty().id(); // 模拟本crate创建的TLAS代理。
    let camera = app.world_mut().spawn_empty().id(); // 租约只需稳定目标实体身份。
    let lease = app
        .world_mut()
        .resource_mut::<GraphicsRuntimeLeaseRegistry>()
        .acquire(GraphicsRuntimeRequirements::new(camera)); // 回退后注册表必须为空。

    {
        let mut runtime = app.world_mut().resource_mut::<RaytracingRuntime>(); // 一次构造完整会话所有权。
        runtime.generation = 9; // 固定当前地图代次。
        runtime.camera = Some(camera); // 保存租约目标相机。
        runtime.candidates.push(SourceCandidate {
            contract_index: 0,                        // 使用首个固定契约槽。
            mesh_name: "测试网格".to_owned(),         // 回退测试不执行GLTF身份验证。
            material_name: "测试材质".to_owned(),     // 仅补齐候选冻结身份。
            requires_source_uv: false,                // 本测试不进入分帧捕获。
            terrain_surface: false, // 普通材质恢复分支与下方独立地表事务共同覆盖。
            entity: standard_entity, // 关联可见普通源。
            mesh: source_mesh.clone(), // 源Mesh强句柄必须保留。
            mesh_id: source_mesh.id(), // 当前源资产身份。
            source_revision: 1,     // 固定测试修订。
            vertex_count: 0,        // 本测试不执行发现预算。
            triangle_count: 0,      // 本测试不执行几何转换。
            derived_bytes: 0,       // 本测试只验证资产所有权。
            adaptation_ticket: None, // 已完成采用，无在途票据。
            derived_mesh: Some(derived_mesh.clone()), // 回退必须从Assets删除。
        });
        runtime.prepared_materials = vec![
            PreparedSourceMaterial {
                source_entity: standard_entity, // 普通源恢复目标。
                binding: SourceMaterialBinding::Standard {
                    source: standard_source.clone(),     // 精确作者句柄。
                    deferred: standard_deferred.clone(), // 本crate会话句柄。
                },
            },
            PreparedSourceMaterial {
                source_entity: terrain_entity, // 通用地表恢复目标。
                binding: SourceMaterialBinding::TerrainSurface {
                    source: terrain_source.clone(),     // 精确作者扩展句柄。
                    deferred: terrain_deferred.clone(), // 本crate扩展副本。
                    proxy: grass_proxy.clone(),         // 本crate代理标准材质。
                },
            },
        ];
        runtime.created_standard_materials = vec![standard_deferred.clone(), grass_proxy.clone()]; // 两类标准副本均由本crate拥有。
        runtime.created_surface_materials = vec![terrain_deferred.clone()]; // 通用地表扩展副本由本crate拥有。
        runtime.proxy_entities = vec![proxy_entity]; // 只删除明确记录的代理。
        runtime.visible_materials_activated = true; // 启用原句柄恢复分支。
        runtime.lease = Some(lease); // 启用精确租约释放分支。
        runtime.request_rollback(None, RollbackDisposition::RasterReady); // 模拟F1关闭请求。
    }

    app.update(); // 执行正式回退并应用全部延迟命令。

    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<StandardMaterial>>(standard_entity)
            .map(|handle| handle.id()),
        Some(standard_source.id())
    ); // 普通源精确恢复作者句柄。
    assert_eq!(
        app.world()
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(terrain_entity)
            .map(|handle| handle.id()),
        Some(terrain_source.id())
    ); // 通用地表精确恢复作者扩展句柄。
    assert!(
        app.world()
            .get::<TerrainSurfaceMaterialOverride>(terrain_entity)
            .is_none()
    ); // 回退必须与原句柄恢复同批释放Deferred租约。
    assert!(app.world().get_entity(proxy_entity).is_err()); // TLAS代理已回收。
    assert!(
        app.world()
            .resource::<Assets<Mesh>>()
            .get(derived_mesh.id())
            .is_none()
    ); // 派生Mesh资产已删除。
    assert!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(standard_deferred.id())
            .is_none()
    ); // 普通Deferred副本已删除。
    assert!(
        app.world()
            .resource::<Assets<StandardMaterial>>()
            .get(grass_proxy.id())
            .is_none()
    ); // 地形光追代理标准材质已删除。
    assert!(
        app.world()
            .resource::<Assets<TerrainSurfaceMaterial>>()
            .get(terrain_deferred.id())
            .is_none()
    ); // 通用地表Deferred副本已删除。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    ); // 最后一份租约已经释放。
    let runtime = app.world().resource::<RaytracingRuntime>(); // 核对公开阶段与内部所有权集合。
    assert_eq!(runtime.stage, RaytracingPrototypeStage::RasterReady); // 回退进入完整光栅稳定态。
    assert!(runtime.candidates.is_empty()); // 不残留派生Mesh强句柄。
    assert!(runtime.prepared_materials.is_empty()); // 不残留材质事务。
    assert!(runtime.proxy_entities.is_empty()); // 不残留代理所有权。
}
