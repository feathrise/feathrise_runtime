//! 独立网格使用真实调度和GLTF加载器取得Mesh植被原型，防止只验证草本路径。

use super::super::mesh_profiles::{
    self, DynamicMeshProfileCache, DynamicMeshProfilePhase, DynamicMeshProfileRequests,
    DynamicMeshProfileState, DynamicMeshProfileStatus,
};
use super::*;
use bevy::{
    asset::AssetPlugin,
    ecs::{
        schedule::ScheduleCleanupPolicy,
        system::{IntoSystem, System},
    },
    gltf::GltfPlugin,
};
use feathrise_environment::{
    mesh_wind::{TreeWindMaterial, TreeWindNoiseTexture},
    vegetation::{
        VegetationMeshBillboardMaterial, VegetationMeshPrototypeRegistry, VegetationPrototypeId,
    },
};
use feathrise_scene_core::residency::{
    InstanceResourceTable, MapRuntimeScope, initialize_instance_context, retire_instance_context,
};
use std::time::{Duration, Instant};

fn fixture() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: crate::test_fixtures::VEGETATION_ASSET_ROOT.into(),
            ..default()
        },
        GltfPlugin::default(),
    ))
    .init_asset::<WorldAsset>()
    .init_asset::<Mesh>()
    .init_asset::<Image>()
    .init_asset::<StandardMaterial>()
    .init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>()
    .init_asset::<TreeWindMaterial>()
    .init_asset::<VegetationMeshBillboardMaterial>()
        .init_asset::<bevy::shader::Shader>()
        .register_asset_loader(bevy::shader::ShaderLoader)
        .init_asset::<bevy::light::atmosphere::ScatteringMedium>()
        // Bevy 0.19的正式MaterialN/std由PbrPlugin登记；使用实际转换及纹理依赖链。
        .add_plugins(bevy::pbr::PbrPlugin {
            add_default_deferred_lighting_plugin: false,
            ..default()
        })
    .init_resource::<VegetationMeshPrototypeRegistry>()
    .init_resident_resource::<DynamicMeshProfileCache>()
    .init_instance_resource::<DynamicMeshProfileRequests>()
    .init_instance_resource::<DynamicMeshProfileState>()
    .init_instance_resource::<DynamicMeshProfileStatus>()
    .add_systems(PostUpdate, mesh_profiles::collect_dynamic_mesh_profiles);
    // 装配测试使用真实有效Image作为共享噪声契约，不启动RenderApp或原生窗口。
    let noise = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::default());
    app.insert_resource(TreeWindNoiseTexture(noise));
    let catalog: VegetationPresetCatalogAsset = serde_json::from_str(
        crate::test_fixtures::VEGETATION_CATALOG_JSON,
    )
    .unwrap();
    app.insert_resource(VegetationPresetRegistry {
        catalog: Handle::default(),
        revision: 1,
        resolved: ResolvedVegetationPresetCatalog::build(&catalog).unwrap(),
        last_error: None,
    });
    app.finish();
    app.cleanup();
    app
}

/// 从正式独立网格调度中保留加载步骤；若生产漏接该步骤，不能靠测试手动补注册蒙混通过。
fn profile_schedule(world: &mut World) -> Schedule {
    let keep = IntoSystem::into_system(mesh_profiles::drive_dynamic_mesh_profiles).system_type();
    let mut schedule = schedules::advance_terrain();
    schedule.initialize(world).unwrap();
    let mut found = false;
    let mut remove = Vec::new();
    for (_, system) in schedule.systems().unwrap() {
        if system.system_type() == keep {
            found = true;
        } else if system.system_type() != std::any::TypeId::of::<ApplyDeferred>() {
            // 自动插入的ApplyDeferred并非作者登记的集合，不能通过default set移除。
            remove.extend(system.default_system_sets());
        }
    }
    assert!(
        found,
        "独立网格生产调度漏掉Mesh预设加载，所有树木层都会永久等待"
    );
    // 地形/水域/碰撞不属于本测试；仅从已有调度移除它们，绝不另外添加被测加载系统。
    let mut removed = std::collections::HashSet::new();
    for set in remove.into_iter().filter(|set| removed.insert(*set)) {
        schedule
            .remove_systems_in_set(set, world, ScheduleCleanupPolicy::RemoveSystemsOnly)
            .unwrap();
    }
    schedule.initialize(world).unwrap();
    assert!(
        schedule.systems().unwrap().all(|(_, system)| {
            system.system_type() == keep
                || system.system_type() == std::any::TypeId::of::<ApplyDeferred>()
        }),
        "测试只保留正式调度中的Mesh加载，不运行其他地形域或补写替代驱动"
    );
    schedule
}

fn context(world: &mut World, profile: Option<&str>) -> Entity {
    let entity = world.spawn_empty().id();
    initialize_instance_context(world, entity);
    if let Some(profile) = profile {
        world
            .resource_mut::<InstanceResourceTable<DynamicMeshProfileRequests>>()
            .values
            .get_mut(&entity)
            .unwrap()
            .request(profile)
            .unwrap();
    }
    entity
}

fn tick(app: &mut App, schedule: &mut Schedule, scopes: &[Entity]) {
    app.update(); // 正式AssetServer推进GLTF、纹理依赖及FrameCount。
    for &scope in scopes {
        app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(scope);
        schedule.run(app.world_mut());
    }
    *app.world_mut().resource_mut::<MapRuntimeScope>() = MapRuntimeScope::default();
}

fn ready(world: &World, scope: Entity, profile: &str) -> bool {
    world
        .resource::<InstanceResourceTable<DynamicMeshProfileState>>()
        .values[&scope]
        .ready_for(
            profile,
            world.resource::<VegetationPresetRegistry>(),
            world.resource::<VegetationMeshPrototypeRegistry>(),
        )
}

fn assert_loaded_model(world: &World, profile: &str) {
    let preset = world
        .resource::<VegetationPresetRegistry>()
        .resolve(profile)
        .unwrap();
    let VegetationPresetRuntime::Mesh { lods, .. } = &preset.runtime else {
        panic!("必须测试正式Mesh预设");
    };
    let prototype = world
        .resource::<VegetationMeshPrototypeRegistry>()
        .get(VegetationPrototypeId(
            preset.resolved_prototype_id().unwrap(),
        ))
        .unwrap();
    assert_eq!(
        prototype.lods.len(),
        lods.len(),
        "不能只登记空原型冒充模型已就绪"
    );
    for (actual, declared) in prototype.lods.iter().zip(lods) {
        assert_eq!(actual.parts.len(), declared.parts.len());
        for (part, source) in actual.parts.iter().zip(&declared.parts) {
            let mesh = world.resource::<Assets<Mesh>>().get(&part.mesh).unwrap();
            assert!(
                mesh.count_vertices() > 0,
                "每档模型必须已经成为真实可用几何"
            );
            // 风/动态公告板把已转换PBR复制进扩展材质，加载结束后允许源/std句柄退役。
            // 必须核对原型实际持有的材质及base，不要求AssetServer永久保留临时源句柄。
            assert!(source.material_path.ends_with("/std"));
            let material = if source.wind.is_some() {
                assert!(part.material.is_material::<TreeWindMaterial>());
                &world.resource::<Assets<TreeWindMaterial>>()
                    .get(part.material.asset_id().typed::<TreeWindMaterial>())
                    .expect("风部件必须持有已完成PBR转换的扩展材质").base
            } else if source.billboard != feathrise_map_content::VegetationPresetBillboard::Fixed {
                assert!(part.material.is_material::<VegetationMeshBillboardMaterial>());
                &world.resource::<Assets<VegetationMeshBillboardMaterial>>()
                    .get(part.material.asset_id().typed::<VegetationMeshBillboardMaterial>())
                    .expect("动态公告板必须持有已完成PBR转换的扩展材质").base
            } else {
                assert!(part.material.is_material::<StandardMaterial>());
                world.resource::<Assets<StandardMaterial>>()
                    .get(part.material.asset_id().typed::<StandardMaterial>())
                    .expect("静态部件必须持有已完成转换的标准PBR材质")
            };
            assert_eq!(material.base_color.to_linear().to_f32_array(), [0.4, 0.6, 0.5, 1.0]);
            assert_eq!(material.perceptual_roughness, 0.7);
        }
    }
}

fn wait_ready(
    app: &mut App,
    schedule: &mut Schedule,
    scopes: &[Entity],
    targets: &[(Entity, &str)],
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !targets
        .iter()
        .all(|(scope, profile)| ready(app.world(), *scope, profile))
    {
        tick(app, schedule, scopes);
        let states = app
            .world()
            .resource::<InstanceResourceTable<DynamicMeshProfileStatus>>();
        assert!(
            targets
                .iter()
                .all(|(scope, _)| states.values[scope].phase != DynamicMeshProfilePhase::Failed),
            "真实植被GLTF加载失败：{:?}",
            targets
                .iter()
                .map(|(scope, _)| &states.values[scope])
                .collect::<Vec<_>>()
        );
        assert!(
            Instant::now() < deadline,
            "独立网格原型一直未就绪：{:?}",
            targets
                .iter()
                .map(|(scope, _)| &states.values[scope])
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn independent_terrain_mesh_profiles_load_tree_share_with_sibling_and_release_only_after_last_scope()
 {
    let mut app = fixture();
    let mut schedule = profile_schedule(app.world_mut());
    let empty = context(app.world_mut(), None);
    let first = context(app.world_mut(), Some("hn01_tree"));
    let sibling = context(app.world_mut(), Some("hn01_tree"));
    // 空网格排在前面不能消耗共享加载的每帧推进资格，从而饿死后面的树木网格。
    wait_ready(
        &mut app,
        &mut schedule,
        &[empty, first, sibling],
        &[(first, "hn01_tree"), (sibling, "hn01_tree")],
    );
    assert_loaded_model(app.world(), "hn01_tree");
    let prototype = VegetationPrototypeId(
        app.world()
            .resource::<VegetationPresetRegistry>()
            .resolve("hn01_tree")
            .unwrap()
            .resolved_prototype_id()
            .unwrap(),
    );
    let revision = app
        .world()
        .resource::<VegetationMeshPrototypeRegistry>()
        .revision(prototype)
        .unwrap();
    assert_eq!(
        revision, 1,
        "兄弟网格必须共享一次完整装配，不能互相替换同ID原型"
    );
    assert_eq!(
        app.world()
            .resource::<VegetationMeshPrototypeRegistry>()
            .len(),
        1
    );
    assert_eq!(
        app.world()
            .resource::<DynamicMeshProfileRequests>()
            .requested_count(),
        0
    );
    assert_eq!(
        app.world()
            .resource::<DynamicMeshProfileStatus>()
            .ready_profiles,
        0
    );
    assert_eq!(
        app.world()
            .resource::<InstanceResourceTable<DynamicMeshProfileStatus>>()
            .values[&empty]
            .requested_profiles,
        0
    );
    drop(retire_instance_context(app.world_mut(), first));
    for _ in 0..4 {
        tick(&mut app, &mut schedule, &[empty, sibling]);
    }
    assert!(
        ready(app.world(), sibling, "hn01_tree"),
        "删除一个网格不能删除兄弟仍借用的树木原型"
    );
    assert_eq!(
        app.world()
            .resource::<VegetationMeshPrototypeRegistry>()
            .revision(prototype),
        Some(revision)
    );
    drop(retire_instance_context(app.world_mut(), sibling));
    for _ in 0..8 {
        tick(&mut app, &mut schedule, &[empty]);
    }
    assert!(
        !app.world()
            .resource::<VegetationMeshPrototypeRegistry>()
            .contains(prototype),
        "最后一个网格退役后，公共注册表不能永久保留无人使用的完整树木原型"
    );
}

#[test]
fn independent_terrain_mesh_profiles_load_shrub_and_rock_without_overwriting_parent_requests() {
    let mut app = fixture();
    let mut schedule = profile_schedule(app.world_mut());
    let shrub = context(app.world_mut(), Some("hn01_shrub"));
    let rock = context(app.world_mut(), Some("hn01_rock"));
    wait_ready(
        &mut app,
        &mut schedule,
        &[shrub, rock],
        &[(shrub, "hn01_shrub"), (rock, "hn01_rock")],
    );
    assert_loaded_model(app.world(), "hn01_shrub");
    assert_loaded_model(app.world(), "hn01_rock");
    assert_eq!(
        app.world()
            .resource::<VegetationMeshPrototypeRegistry>()
            .len(),
        2
    );
    assert_eq!(
        app.world()
            .resource::<DynamicMeshProfileRequests>()
            .requested_count(),
        0
    );
    let statuses = app
        .world()
        .resource::<InstanceResourceTable<DynamicMeshProfileStatus>>();
    assert_eq!(statuses.values[&shrub].requested_profiles, 1);
    assert_eq!(statuses.values[&rock].requested_profiles, 1);
}

#[test]
fn independent_terrain_mesh_profiles_failed_catalog_recovers_after_hot_reload_without_new_request()
{
    let mut app = fixture();
    let mut schedule = profile_schedule(app.world_mut());
    let mut catalog: VegetationPresetCatalogAsset = serde_json::from_str(
        crate::test_fixtures::VEGETATION_CATALOG_JSON,
    )
    .unwrap();
    let original = ResolvedVegetationPresetCatalog::build(&catalog).unwrap();
    let tree = catalog
        .presets
        .iter_mut()
        .find(|preset| preset.id == "hn01_tree")
        .unwrap();
    let VegetationPresetRuntime::Mesh { lods, .. } = &mut tree.runtime else {
        unreachable!()
    };
    // 只替换测试内存目录；正式加载器仍读取自建GLTF，混合失败/成功请求也须安全收口。
    lods[0].parts[0].mesh_path =
        "gltf/tree_profile_regression_missing.glb#Mesh0/Primitive0".to_owned();
    app.world_mut()
        .resource_mut::<VegetationPresetRegistry>()
        .resolved = ResolvedVegetationPresetCatalog::build(&catalog).unwrap();
    let source = context(app.world_mut(), Some("hn01_tree"));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        tick(&mut app, &mut schedule, &[source]);
        let status = &app
            .world()
            .resource::<InstanceResourceTable<DynamicMeshProfileStatus>>()
            .values[&source];
        if status.phase == DynamicMeshProfilePhase::Failed {
            assert!(
                status
                    .detail
                    .contains("tree_profile_regression_missing.glb"),
                "释放句柄前必须保留实际损坏模型的路径和失败原因：{}",
                status.detail
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "损坏模型必须得到明确失败回执：{status:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    for _ in 0..4 {
        tick(&mut app, &mut schedule, &[source]);
    }
    assert_eq!(
        app.world()
            .resource::<DynamicMeshProfileCache>()
            .loading_profile(),
        None,
        "稳定失败不能每帧重新加载，必须等待显式重试或目录换代"
    );
    assert!(
        app.world()
            .resource::<VegetationMeshPrototypeRegistry>()
            .is_empty()
    );
    {
        let mut presets = app.world_mut().resource_mut::<VegetationPresetRegistry>();
        presets.resolved = original;
        presets.revision += 1;
    }
    // 不调用request；目录修复本身必须使原树木来源从失败恢复。
    wait_ready(&mut app, &mut schedule, &[source], &[(source, "hn01_tree")]);
    assert_loaded_model(app.world(), "hn01_tree");
    assert_eq!(
        app.world()
            .resource::<InstanceResourceTable<DynamicMeshProfileRequests>>()
            .values[&source]
            .requested_count(),
        1
    );
}
