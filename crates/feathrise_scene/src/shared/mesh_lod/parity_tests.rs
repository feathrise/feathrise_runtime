//! 旧正式LOD用共同几何和相机取证；测试执行成功与新旧效果对等分别判断。
use super::*;
// 中性几何夹具随SDK自身保存，公共仓库测试不反向读取私有验证目录。
#[path = "../../../tests/fixtures/editor-lod-parity-v1.rs"]
mod fixture;

#[test]
fn integrated_lod_records_shared_parity_fixture() {
    let mut app = tests::app();
    app.world_mut().resource_mut::<feathrise_settings::RenderSettings>().mesh_lod.max_error_pixels = 16.0;
    app.world_mut().resource_mut::<feathrise_settings::RenderSettings>().mesh_lod.hysteresis_ratio = 0.15;
    app.world_mut().resource_mut::<feathrise_settings::RenderSettings>().mesh_lod.display_enabled = true;
    let source = app.world_mut().resource_mut::<Assets<Mesh>>().add(fixture::mesh());
    let root = tests::owner_root(app.world_mut(), false);
    let entity = tests::visual(app.world_mut(), root, source.clone());
    app.world_mut().entity_mut(entity).insert(fixture::bounds());
    fixture::camera(app.world_mut());
    tests::until(&mut app, |world| world.get::<Mesh3d>(entity).unwrap().0 != source);
    let mut rows = vec![fixture::sample(app.world(), entity, &source, "game-enabled")];
    app.world_mut().resource_mut::<feathrise_settings::RenderSettings>().mesh_lod.display_enabled = false;
    app.update(); rows.push(fixture::sample(app.world(), entity, &source, "game-disabled"));
    app.world_mut().resource_mut::<feathrise_settings::RenderSettings>().mesh_lod.display_enabled = true;
    app.update(); rows.push(fixture::sample(app.world(), entity, &source, "game-reenabled"));
    app.world_mut().resource_mut::<MeshLodAuthoringState>().active = true;
    app.update(); rows.push(fixture::sample(app.world(), entity, &source, "authoring"));
    assert!(app.world().resource::<MeshLodRuntimeStatus>().authoring_ready);
    app.world_mut().resource_mut::<MeshLodAuthoringState>().active = false;
    app.update(); rows.push(fixture::sample(app.world(), entity, &source, "game-resumed"));
    fixture::report("integrated-production-lod", rows);
}
