//! Deferred光栅兼容桥的纯资产回归；不启动窗口、RenderApp或Solari GPU管线。

use bevy::{
    material::OpaqueRendererMethod, // 构造Auto、Forward与Deferred三种作者基线。
    platform::collections::{HashMap, HashSet}, // 与运行时相同的资产备份与跳过集合。
    prelude::*,                     // Assets、StandardMaterial、AlphaMode与Handle。
};

use super::{capture_assets, force_deferred, restore_assets, writes_deferred_gbuffer};
use crate::{constants::MAX_DEFERRED_RASTER_MATERIAL_COUNT, types::RaytracingPrototypeBlocker};

#[test]
fn bridge_preserves_auto_and_restores_explicit_forward_exactly() {
    let mut materials = Assets::<StandardMaterial>::default(); // 内存资产表足以验证字段所有权。
    let auto = materials.add(StandardMaterial::default()); // Auto依赖当前全局默认但字段本身必须保持Auto。
    let forward = materials.add(StandardMaterial {
        opaque_render_method: OpaqueRendererMethod::Forward, // 显式Forward在Solari期必须临时覆盖。
        ..default()                                          // 其余PBR字段不影响本测试。
    });
    let deferred = materials.add(StandardMaterial {
        opaque_render_method: OpaqueRendererMethod::Deferred, // 已兼容的P1会话副本无需再次改写。
        ..default()                                           // 其余字段保持普通PBR。
    });
    let captured = vec![
        (auto.id(), OpaqueRendererMethod::Auto), // 模拟有界初始捕获结果。
        (forward.id(), OpaqueRendererMethod::Forward), // 保存显式作者方式。
        (deferred.id(), OpaqueRendererMethod::Deferred), // 验证显式Deferred只登记回退所有权。
    ];
    let mut backups = HashMap::default(); // 回退只依据这一份明确所有权表。

    force_deferred(&mut materials, &mut backups, captured); // 应用正式兼容逻辑。

    assert_eq!(
        materials.get(auto.id()).unwrap().opaque_render_method,
        OpaqueRendererMethod::Auto
    ); // Auto字段不固化为Deferred，新建Dither克隆不会继承临时覆盖。
    assert_eq!(
        materials.get(forward.id()).unwrap().opaque_render_method,
        OpaqueRendererMethod::Deferred
    ); // 显式Forward必须被临时纠正。
    assert_eq!(
        materials.get(deferred.id()).unwrap().opaque_render_method,
        OpaqueRendererMethod::Deferred
    ); // 已Deferred资产保持原值，正式实现也不会取得可变借用制造自发Modified事件。

    restore_assets(&mut materials, &mut backups); // 模拟关闭请求或离开地图。

    assert_eq!(
        materials.get(auto.id()).unwrap().opaque_render_method,
        OpaqueRendererMethod::Auto
    ); // Auto精确恢复并由真实系统触发重新准备。
    assert_eq!(
        materials.get(forward.id()).unwrap().opaque_render_method,
        OpaqueRendererMethod::Forward
    ); // 显式作者Forward不能被会话永久改写。
    assert_eq!(
        materials.get(deferred.id()).unwrap().opaque_render_method,
        OpaqueRendererMethod::Deferred
    ); // 作者原本Deferred的资产精确恢复为同一值。
    assert!(backups.is_empty()); // 回退一次取走全部资产所有权。
}

#[test]
fn bridge_targets_only_depth_backed_opaque_and_mask_surfaces() {
    assert!(writes_deferred_gbuffer(&StandardMaterial::default())); // 默认Opaque必须提供GBuffer。
    assert!(writes_deferred_gbuffer(&StandardMaterial {
        alpha_mode: AlphaMode::Mask(0.5), // AlphaMask仍会写深度并需要同像素GBuffer。
        ..default()                       // 其余字段保持普通PBR。
    }));
    assert!(!writes_deferred_gbuffer(&StandardMaterial {
        alpha_mode: AlphaMode::Blend, // 透明主通道不进入Deferred opaque阶段。
        ..default()                   // 其余字段不改变透明分类。
    }));
}

#[test]
fn initial_capture_stops_at_the_257th_loaded_material() {
    let mut materials = Assets::<StandardMaterial>::default(); // 创建恰好硬上限加一项。
    for _ in 0..=MAX_DEFERRED_RASTER_MATERIAL_COUNT {
        materials.add(StandardMaterial::default()); // 每项都属于需要兼容的Opaque材质。
    }
    let mut visited = 0usize; // 正式捕获跨类型共享该计数器。
    let blocker = capture_assets(&materials, &HashSet::default(), &mut visited)
        .expect_err("第257项必须在产生任何材质写入前整体阻断");

    assert_eq!(visited, MAX_DEFERRED_RASTER_MATERIAL_COUNT + 1); // 不继续遍历无界精确总数。
    assert!(matches!(
        blocker,
        RaytracingPrototypeBlocker::DeferredRasterMaterialBudgetExceeded {
            actual,
            maximum,
        } if actual == MAX_DEFERRED_RASTER_MATERIAL_COUNT + 1
            && maximum == MAX_DEFERRED_RASTER_MATERIAL_COUNT
    )); // F1报告固定停止线而不是部分接入前256项。
}

// 中性测试扩展模拟应用拥有的材质；共享库测试不引入原相机、角色或游戏 shader。
#[derive(Asset, bevy::render::render_resource::AsBindGroup, Reflect, Debug, Clone)]
struct TestExtension {
    #[uniform(100)]
    value: f32,
}
impl bevy::pbr::MaterialExtension for TestExtension {}
type TestMaterial = bevy::pbr::ExtendedMaterial<StandardMaterial, TestExtension>;

fn custom_bridge_app() -> App {
    use super::super::{maintain_deferred_raster_bridge, restore_deferred_raster_bridge};
    use crate::resources::{
        DeferredRasterBridgeState, DeferredRasterBridgeTargets, RaytracingRuntime,
    };
    let mut app = App::new();
    macro_rules! assets {
        ($kind:ty) => {
            app.init_resource::<Assets<$kind>>()
                .add_message::<bevy::asset::AssetEvent<$kind>>();
        };
    }
    assets!(StandardMaterial);
    assets!(TestMaterial);
    assets!(feathrise_environment::mesh_wind::TreeWindMaterial);
    assets!(feathrise_environment::terrain_surface::TerrainSurfaceMaterial);
    assets!(feathrise_vegetation::VegetationMeshBillboardMaterial);
    assets!(feathrise_scene::SparseClipmapSurfaceMaterial);
    app.insert_resource(bevy::pbr::DefaultOpaqueRendererMethod::forward())
        .init_resource::<DeferredRasterBridgeState>()
        .init_resource::<DeferredRasterBridgeTargets>()
        .init_resource::<RaytracingRuntime>()
        .add_systems(
            Update,
            (
                maintain_deferred_raster_bridge,
                restore_deferred_raster_bridge,
            )
                .chain(),
        );
    crate::register_deferred_raster_material::<TestMaterial>(&mut app);
    app.world_mut().resource_mut::<RaytracingRuntime>().stage =
        crate::types::RaytracingPrototypeStage::WarmingSolari;
    app
}

fn test_material(method: OpaqueRendererMethod, alpha: AlphaMode) -> TestMaterial {
    TestMaterial {
        base: StandardMaterial {
            opaque_render_method: method,
            alpha_mode: alpha,
            ..default()
        },
        extension: TestExtension { value: 0.5 },
    }
}

#[test]
fn application_materials_share_real_targets_hot_reload_and_exact_rollback() {
    use crate::resources::{DeferredRasterBridgeTargets, RaytracingRuntime, RollbackDisposition};
    let mut app = custom_bridge_app();
    let forward = app
        .world_mut()
        .resource_mut::<Assets<TestMaterial>>()
        .add(test_material(
            OpaqueRendererMethod::Forward,
            AlphaMode::Opaque,
        ));
    let auto = app
        .world_mut()
        .resource_mut::<Assets<TestMaterial>>()
        .add(test_material(OpaqueRendererMethod::Auto, AlphaMode::Opaque));
    let transparent = app
        .world_mut()
        .resource_mut::<Assets<TestMaterial>>()
        .add(test_material(
            OpaqueRendererMethod::Forward,
            AlphaMode::Blend,
        ));
    app.update();
    let assets = app.world().resource::<Assets<TestMaterial>>();
    assert_eq!(
        assets.get(&forward).unwrap().base.opaque_render_method,
        OpaqueRendererMethod::Deferred
    );
    assert_eq!(
        assets.get(&auto).unwrap().base.opaque_render_method,
        OpaqueRendererMethod::Auto
    );
    assert_eq!(
        assets.get(&transparent).unwrap().base.opaque_render_method,
        OpaqueRendererMethod::Forward
    );
    assert_eq!(assets.get(&forward).unwrap().extension.value, 0.5);
    let targets = &app.world().resource::<DeferredRasterBridgeTargets>().ids;
    assert!(targets.contains(&forward.id().untyped()));
    assert!(!targets.contains(&transparent.id().untyped()));

    // 作者热更把显式Forward改为Auto；关闭时必须恢复最新作者选择而非首次旧值。
    app.world_mut()
        .resource_mut::<Assets<TestMaterial>>()
        .get_mut(&forward)
        .unwrap()
        .base
        .opaque_render_method = OpaqueRendererMethod::Auto;
    app.world_mut()
        .write_message(bevy::asset::AssetEvent::<TestMaterial>::Modified { id: forward.id() });
    app.update();
    app.world_mut()
        .resource_mut::<RaytracingRuntime>()
        .request_rollback(None, RollbackDisposition::RasterReady);
    app.update();
    assert_eq!(
        app.world()
            .resource::<Assets<TestMaterial>>()
            .get(&forward)
            .unwrap()
            .base
            .opaque_render_method,
        OpaqueRendererMethod::Auto
    );
    assert!(
        app.world()
            .resource::<DeferredRasterBridgeTargets>()
            .ids
            .is_empty()
    );
}

#[test]
fn application_material_overflow_does_not_partially_install_shared_bridge() {
    use crate::resources::{
        DeferredRasterBridgeState, DeferredRasterBridgeTargets, RaytracingRuntime,
    };
    let mut app = custom_bridge_app();
    for _ in 0..MAX_DEFERRED_RASTER_MATERIAL_COUNT {
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
    }
    let custom = app
        .world_mut()
        .resource_mut::<Assets<TestMaterial>>()
        .add(test_material(
            OpaqueRendererMethod::Forward,
            AlphaMode::Opaque,
        ));
    app.update();
    assert!(
        !app.world()
            .resource::<DeferredRasterBridgeState>()
            .installed
    );
    assert!(
        app.world()
            .resource::<DeferredRasterBridgeTargets>()
            .ids
            .is_empty()
    );
    assert!(
        app.world()
            .resource::<RaytracingRuntime>()
            .pending_rollback
            .is_some()
    );
    assert_eq!(
        app.world()
            .resource::<Assets<TestMaterial>>()
            .get(&custom)
            .unwrap()
            .base
            .opaque_render_method,
        OpaqueRendererMethod::Forward
    );
}

#[test]
fn application_added_removed_and_missing_asset_table_do_not_leave_stale_targets() {
    use crate::resources::DeferredRasterBridgeTargets;
    let mut app = custom_bridge_app();
    app.update(); // 先安装没有应用资产的桥，后续必须通过Added增量加入。
    let material = app
        .world_mut()
        .resource_mut::<Assets<TestMaterial>>()
        .add(test_material(
            OpaqueRendererMethod::Forward,
            AlphaMode::Opaque,
        ));
    app.world_mut()
        .write_message(bevy::asset::AssetEvent::<TestMaterial>::Added { id: material.id() });
    app.update();
    assert!(
        app.world()
            .resource::<DeferredRasterBridgeTargets>()
            .ids
            .contains(&material.id().untyped())
    );
    assert_eq!(
        app.world()
            .resource::<Assets<TestMaterial>>()
            .get(&material)
            .unwrap()
            .base
            .opaque_render_method,
        OpaqueRendererMethod::Deferred
    );

    app.world_mut()
        .resource_mut::<Assets<TestMaterial>>()
        .remove(material.id());
    app.world_mut()
        .write_message(bevy::asset::AssetEvent::<TestMaterial>::Removed { id: material.id() });
    app.update();
    assert!(
        app.world()
            .resource::<DeferredRasterBridgeTargets>()
            .ids
            .is_empty()
    );

    let replacement = app
        .world_mut()
        .resource_mut::<Assets<TestMaterial>>()
        .add(test_material(
            OpaqueRendererMethod::Forward,
            AlphaMode::Opaque,
        ));
    app.world_mut()
        .write_message(bevy::asset::AssetEvent::<TestMaterial>::Added {
            id: replacement.id(),
        });
    app.update();
    app.world_mut().remove_resource::<Assets<TestMaterial>>();
    app.update();
    assert!(
        app.world()
            .resource::<DeferredRasterBridgeTargets>()
            .ids
            .is_empty()
    );

    // 同类型资产表重建后，即使没有Added消息也必须通过首次快照恢复真实新目标。
    let mut assets = Assets::<TestMaterial>::default();
    let restored = assets.add(test_material(
        OpaqueRendererMethod::Forward,
        AlphaMode::Opaque,
    ));
    app.world_mut().insert_resource(assets);
    app.update();
    assert!(
        app.world()
            .resource::<DeferredRasterBridgeTargets>()
            .ids
            .contains(&restored.id().untyped())
    );
}

#[test]
fn active_application_transparent_assets_do_not_consume_member_budget() {
    use crate::resources::{DeferredRasterBridgeTargets, RaytracingRuntime};
    let mut app = custom_bridge_app();
    app.update();
    let mut handles = Vec::new();
    for _ in 0..=MAX_DEFERRED_RASTER_MATERIAL_COUNT {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<TestMaterial>>()
            .add(test_material(
                OpaqueRendererMethod::Forward,
                AlphaMode::Blend,
            ));
        app.world_mut()
            .write_message(bevy::asset::AssetEvent::<TestMaterial>::Added { id: handle.id() });
        handles.push(handle);
    }
    app.update();
    assert!(
        app.world()
            .resource::<RaytracingRuntime>()
            .pending_rollback
            .is_none()
    );
    assert!(
        app.world()
            .resource::<DeferredRasterBridgeTargets>()
            .ids
            .is_empty()
    );
}
