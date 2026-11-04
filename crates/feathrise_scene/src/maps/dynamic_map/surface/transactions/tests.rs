//! 动态地表源转交、稳定材质仲裁与预热取消事务回归测试。

use feathrise_scene_core::residency::{InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use bevy::ecs::system::RunSystemOnce; // 最小World执行真实Commands并在返回前应用延迟组件变更。
use bevy::pbr::ExtendedMaterial; // 构造资产表中真实存在的扩展材质句柄。
use feathrise_environment::terrain_surface::{
    TerrainSurfaceDescriptor, TerrainSurfaceExtension, TerrainSurfaceLayer,
    TerrainSurfaceMaterialOverride,
}; // 测试活动与候选材质必须使用正式固定布局类型。
use feathrise_map_content::{MapId, MapSurfaceAsset}; // 预热重试只需要一份冻结定义身份。

use super::*; // 直接覆盖正式源转交和统一预热取消事务。

fn test_surface_material() -> TerrainSurfaceMaterial {
    ExtendedMaterial {
        base: StandardMaterial::default(), // 测试只关心资产句柄有效性，不绑定真实纹理内容。
        extension: TerrainSurfaceExtension::new(TerrainSurfaceDescriptor {
            world_min: Vec2::ZERO,                               // 最小测试世界范围。
            world_max: Vec2::ONE,                                // 保持非零范围。
            weight_maps: [Handle::default(), Handle::default()], // 无RenderApp测试使用空图片句柄。
            weight_map_count: 1, // 与动态地图当前单RGBA权重图路径一致。
            base_color_array: Handle::default(), // 不启动纹理加载。
            arm_array: Handle::default(), // 不启动纹理加载。
            normal_array: Handle::default(), // 不启动纹理加载。
            index_map: Handle::default(), // 不启动纹理加载。
            layer_table: Handle::default(), // 无 RenderApp 事务测试只需类型正确。
            layers: [TerrainSurfaceLayer::default(); 8], // 固定八槽满足正式材质布局。
            layer_count: 1,      // 至少保留一个有效层。
            residual_layer: 0,   // 唯一层接收残余权重。
            grass_layer: 0,      // 唯一层作为草土层。
            grass_micro_normal_strength: 0.0, // 关闭无关微法线。
            grass_variation_cell_meters: 1.0, // 保持合法正尺度。
        }),
    }
}

fn reconcile_once(
    mut commands: Commands,
    materials: Res<Assets<TerrainSurfaceMaterial>>,
    clipmap_materials: Res<Assets<SparseClipmapSurfaceMaterial>>,
    mut state: InstanceResMut<DynamicSurfaceState>,
    mut runtime: InstanceResMut<DynamicMapRuntimeState>,
    mut stats: ResMut<TerrainSurfaceRenderStats>,
    current: Query<
        (
            Entity,
            Option<&MeshMaterial3d<StandardMaterial>>,
            Option<&MeshMaterial3d<ClipmapHeightMaterial>>,
            Option<&MeshMaterial3d<TerrainSurfaceMaterial>>,
            Option<&MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
            Option<&TerrainSurfaceCarrier>,
            Option<&TerrainSurfaceMaterialOverride>,
            Option<&SparseClipmapSurfaceLease>,
        ),
        With<DynamicMapTerrainPrimitive>,
    >,
) {
    reconcile_current_surface_source(
        &mut commands,
        &materials,
        &clipmap_materials,
        current.single().ok().map(
            |(
                entity,
                white,
                height_fallback,
                surface,
                clipmap,
                carrier,
                material_override,
                clipmap_lease,
            )| {
                (
                    entity,
                    white,
                    height_fallback,
                    surface,
                    clipmap,
                    carrier.is_some(),
                    material_override,
                    clipmap_lease,
                )
            },
        ), // 测试输入与生产只读源快照保持相同字段。
        &mut state,
        &mut runtime,
        &mut stats,
    ); // 使用与生产系统相同的唯一正式源输入。
}

#[test]
fn active_surface_is_transferred_to_replaced_terrain_without_white_or_stale_material() {
    let mut world = World::new(); // 最小主世界足以验证组件原子转交。
    let active = {
        let mut materials = Assets::<TerrainSurfaceMaterial>::default();
        let active = materials.add(test_surface_material()); // 句柄必须在资产表中真实有效。
        world.insert_resource(materials); // 生产逻辑通过这一唯一资产表验证可复用性。
        active
    };
    world.insert_resource(Assets::<SparseClipmapSurfaceMaterial>::default()); // 旧地形测试使用空外层资产表。
    let old_source = world
        .spawn((
            MeshMaterial3d(active.clone()), // 旧正式地形当前使用活动RGBA材质。
            TerrainSurfaceCarrier,          // 旧RenderApp承接身份必须被回收。
            DynamicSurfaceSourceWarmup,     // 遗留预热标记也不能留在旧Scene。
        ))
        .id();
    let new_source = world
        .spawn((
            DynamicMapTerrainPrimitive, // Scene换代后重新登记的唯一正式primitive。
            MeshMaterial3d::<StandardMaterial>(Handle::default()), // GLB重新插入的白模路径。
        ))
        .id();
    world.insert_resource(DynamicSurfaceState {
        source_entity: Some(old_source),       // 状态仍指向换代前实体。
        active_material: Some(active.clone()), // 已预热活动材质可安全跨稳定布局复用。
        ..default()
    });
    world.insert_resource(DynamicMapRuntimeState::default()); // 初始未就绪用于证明本次转交发布成功。
    world.insert_resource(TerrainSurfaceRenderStats::default()); // 初始阶段由事务推进到Active。

    world
        .run_system_once(reconcile_once)
        .expect("唯一正式源转交系统应成功执行");

    assert!(
        world
            .get::<MeshMaterial3d<StandardMaterial>>(new_source)
            .is_none()
    ); // 新白模必须同帧移除。
    assert_eq!(
        world
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(new_source)
            .expect("新地形应立即取得活动地表材质")
            .0
            .id(),
        active.id(),
    ); // 新实体只保留原活动扩展材质。
    assert!(world.get::<TerrainSurfaceCarrier>(new_source).is_some()); // 新实体恢复RenderApp承接身份。
    assert!(
        world
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(old_source)
            .is_none()
    ); // 旧实体不留下第二条draw路径。
    assert!(world.get::<TerrainSurfaceCarrier>(old_source).is_none()); // 旧Carrier随材质一并回收。
    assert!(world.resource::<DynamicMapRuntimeState>().surface_ready); // 当前会话无需重新预热即可继续显示RGBA地表。
    assert_eq!(
        world.resource::<TerrainSurfaceRenderStats>().phase,
        TerrainSurfacePhase::Active,
    ); // 仅几何换代保持正式Active阶段。
}

#[test]
fn stable_active_source_does_not_enter_the_write_reconciliation_path() {
    let mut materials = Assets::<TerrainSurfaceMaterial>::default(); // 构造真实有效活动材质表。
    let active = materials.add(test_surface_material()); // 稳定句柄必须能从资产表读取。
    let installed = MeshMaterial3d(active.clone()); // 当前正式实体已经安装同一活动材质。
    let entity = Entity::from_bits(42); // 纯只读决定只比较稳定实体身份。
    let state = DynamicSurfaceState {
        source_entity: Some(entity),   // 状态与当前唯一正式源一致。
        active_material: Some(active), // 状态持有同一有效活动材质。
        ..default()
    };
    let runtime = DynamicMapRuntimeState {
        surface_ready: true, // 正式运行时已发布完整地表证据。
        ..default()
    };
    let mut stats = TerrainSurfaceRenderStats::default(); // 初始统计随后推进为稳定Active。
    stats.set_phase(TerrainSurfacePhase::Active); // 没有真实资源修订在途。

    assert!(!surface_source_needs_reconcile(
        &materials,
        &Assets::<SparseClipmapSurfaceMaterial>::default(),
        Some((entity, None, None, Some(&installed), None, true, None, None,)),
        &state,
        &runtime,
        &stats,
    )); // 稳定帧只能常数成本读取，不能重复写材质、Carrier或资源状态。
}

#[test]
fn exact_raytracing_override_is_stable_but_unowned_deferred_material_is_reclaimed() {
    let mut materials = Assets::<TerrainSurfaceMaterial>::default(); // 两份资产模拟正式地表和Solari会话副本。
    let active = materials.add(test_surface_material());
    let deferred = materials.add(test_surface_material());
    let installed = MeshMaterial3d(deferred.clone());
    let material_override = TerrainSurfaceMaterialOverride::new(&active, &deferred);
    let entity = Entity::from_bits(43);
    let state = DynamicSurfaceState {
        source_entity: Some(entity),
        active_material: Some(active),
        ..default()
    };
    let runtime = DynamicMapRuntimeState {
        surface_ready: true,
        ..default()
    };
    let mut stats = TerrainSurfaceRenderStats::default();
    stats.set_phase(TerrainSurfacePhase::Active);

    assert!(!surface_source_needs_reconcile(
        &materials,
        &Assets::<SparseClipmapSurfaceMaterial>::default(),
        Some((
            entity,
            None,
            None,
            Some(&installed),
            None,
            true,
            Some(&material_override),
            None,
        )),
        &state,
        &runtime,
        &stats,
    )); // 精确租约在稳定帧保持只读，不能把Solari Deferred句柄换回作者句柄。
    assert!(surface_source_needs_reconcile(
        &materials,
        &Assets::<SparseClipmapSurfaceMaterial>::default(),
        Some((entity, None, None, Some(&installed), None, true, None, None,)),
        &state,
        &runtime,
        &stats,
    )); // 同一个Deferred句柄失去租约后必须由地图系统回收，不能接受任意外部材质。
}

#[test]
fn cancelled_warmup_releases_every_identity_and_requests_one_retry() {
    let mut world = World::new(); // 最小世界验证统一取消的ECS与资产副作用。
    let candidate = {
        let mut materials = Assets::<TerrainSurfaceMaterial>::default();
        let candidate = materials.add(test_surface_material()); // 候选必须真实存在才能证明被回收。
        world.insert_resource(materials);
        candidate
    };
    let source = world.spawn(DynamicSurfaceSourceWarmup).id(); // 模拟绑定即将消失旧源的single-flight标记。
    let warmup = world.spawn_empty().id(); // 模拟离屏预热实体。
    let map_id = MapId::parse("MAP-surface-cancel").expect("测试地图ID合法");
    world.insert_resource(DynamicSurfaceState {
        definition: Some(MapSurfaceAsset {
            schema_version: 0, // 取消事务不重新校验定义内容，只以存在性决定是否重试。
            map_id,            // 冻结同一地图身份。
            weight_source_path: String::new(), // 测试不访问纹理路径。
            weight_texture_path: String::new(), // 测试不访问纹理路径。
            index_source_path: String::new(), // 测试不访问索引作者源。
            index_texture_path: String::new(), // 测试不加载索引纹理。
            resolution: [0, 0], // 测试不创建像素画布。
            layers: Vec::new(), // 测试不构造候选，只验证已存在候选回收。
        }),
        candidate_material: Some(candidate.clone()), // 当前唯一候选资产。
        warmup_entity: Some(warmup),                 // 当前唯一预热事务实体。
        warmup_source_entity: Some(source),          // 即使正式查询失效也保留旧源身份。
        revision: 7,                                 // 取消后应只增加一次重试修订。
        ..default()
    });
    world.insert_resource(Assets::<ShaderBuffer>::default()); // 候选参数表与候选材质共享取消生命周期。

    world
        .run_system_once(
            move |mut commands: Commands,
                  mut materials: ResMut<Assets<TerrainSurfaceMaterial>>,
                  mut layer_tables: ResMut<Assets<ShaderBuffer>>,
                  mut state: InstanceResMut<DynamicSurfaceState>| {
                cancel_surface_warmup(
                    &mut commands,
                    &mut materials,
                    &mut layer_tables,
                    &mut state,
                    warmup,
                    source,
                    true,
                ); // 使用生产统一取消入口，不直接篡改测试状态。
            },
        )
        .expect("预热取消系统应成功执行");

    assert!(world.get::<DynamicSurfaceSourceWarmup>(source).is_none()); // 旧源single-flight标记已释放。
    assert!(world.get_entity(warmup).is_err()); // 离屏预热实体已销毁。
    let state = world.resource::<DynamicSurfaceState>();
    assert!(state.warmup_entity.is_none() && state.warmup_source_entity.is_none()); // 两个世界身份同时清空。
    assert!(state.candidate_material.is_none()); // 状态不再持有孤立候选句柄。
    assert!(state.reload_requested && state.revision == 8); // 当前定义只安排一次新修订重试。
    assert!(
        world
            .resource::<Assets<TerrainSurfaceMaterial>>()
            .get(candidate.id())
            .is_none()
    ); // 未采用候选已从资产表回收。
}

#[test]
fn author_surface_replaces_white_source_and_restores_file_source_without_stealing_lease() {
    let mut world = World::new(); // 最小正式源事务测试。
    let mut materials = Assets::<TerrainSurfaceMaterial>::default(); // 所有句柄真实存在。
    let file = materials.add(test_surface_material()); // 保留文件活动版本。
    let author = materials.add(test_surface_material()); // 已完整预热作者版本。
    let source = world
        .spawn((
            DynamicMapTerrainPrimitive,
            MeshMaterial3d::<StandardMaterial>(Handle::default()),
        ))
        .id(); // 正式白模起点。
    let state = DynamicSurfaceState {
        source_entity: Some(source),
        author_material: Some(author.clone()),
        ..default()
    }; // 首次还没有文件活动材质。
    assert!(surface_source_needs_reconcile(
        &materials,
        &Assets::<SparseClipmapSurfaceMaterial>::default(),
        Some((
            source,
            Some(&MeshMaterial3d(Handle::default())),
            None,
            None,
            None,
            false,
            None,
            None,
        )),
        &state,
        &DynamicMapRuntimeState::default(),
        &TerrainSurfaceRenderStats::default()
    )); // 白模稳定不能吞掉首次作者接管。
    world.insert_resource(materials); // 正式资产表。
    world.insert_resource(Assets::<SparseClipmapSurfaceMaterial>::default()); // 普通源不安装高度外层。
    world.insert_resource(state); // 当前源所有权。
    world.insert_resource(DynamicMapRuntimeState::default()); // 就绪证据。
    world.insert_resource(TerrainSurfaceRenderStats::default()); // 统计阶段。
    world.run_system_once(reconcile_once).unwrap(); // 使用真实命令边界。
    assert_eq!(
        world
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(source)
            .unwrap()
            .0,
        author
    ); // 正式地形实际采用作者源。
    assert!(
        world
            .get::<MeshMaterial3d<StandardMaterial>>(source)
            .is_none()
    ); // 单一draw。
    {
        let mut state = world.resource_mut::<DynamicSurfaceState>(); // 模拟丢弃草稿。
        state.active_material = Some(file.clone()); // 文件源一直保留。
        state.author_material = None; // 只撤销覆盖。
    }
    world.run_system_once(reconcile_once).unwrap(); // 复用正式租约仲裁归还文件。
    assert_eq!(
        world
            .get::<MeshMaterial3d<TerrainSurfaceMaterial>>(source)
            .unwrap()
            .0,
        file
    ); // 不需重新读取文件即可恢复。
    assert!(
        world
            .resource::<Assets<TerrainSurfaceMaterial>>()
            .get(&author)
            .is_some()
    ); // 场景不能删除编辑器持有资产。
}
