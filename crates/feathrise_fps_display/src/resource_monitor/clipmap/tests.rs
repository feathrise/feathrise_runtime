//! 真实分批采样、批次隔离与隐藏门；不创建Mesh、GPU视图或额外工作任务。

use super::{resources::*, systems::*};
use bevy::prelude::*;
use bevy::ecs::entity_disabling::Disabled;
use crate::resources::ResourceMonitorState;
use feathrise_scene_content::{SceneId, SceneInstancePath, SceneInstanceRootRequest, SceneTerrainDisplayLodStatus};
use feathrise_scene::{SubsceneActive, SubsceneInstance};
use feathrise_scene_core::residency::InstanceResourceTable;

fn fixture(count: usize) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<ClipmapMonitorState>()
        .init_resource::<InstanceResourceTable<SceneTerrainDisplayLodStatus>>()
        .insert_resource(ResourceMonitorState { visible: true, ..default() })
        .insert_resource(SceneInstanceRootRequest { scene_id: Some(SceneId::parse("MAP-monitor").unwrap()) })
        .add_systems(Update, sample_clipmaps);
    let batch = app.world_mut().spawn(SubsceneActive).id();
    let map = SceneId::parse("MAP-monitor").unwrap();
    for index in 0..count {
        let entity = app.world_mut().spawn((Name::new(format!("网格{index}")), SubsceneInstance {
            path: SceneInstancePath::root(map.clone()), source_scene_id: map.clone(), generation: 1, batch,
        })).id();
        let mut status = SceneTerrainDisplayLodStatus::default();
        status.scene_id = Some(map.clone());
        status.vertex_count = 11;
        status.triangle_count = 7;
        status.work.mesh_builds = 2;
        app.world_mut().resource_mut::<InstanceResourceTable<SceneTerrainDisplayLodStatus>>().values.insert(entity, status);
    }
    app
}

#[test]
fn many_instances_publish_one_complete_snapshot_under_a_fixed_frame_budget() {
    let mut app = fixture(73);
    app.update();
    assert!(app.world().resource::<ClipmapMonitorState>().report.is_empty());
    assert_eq!(app.world().resource::<ClipmapMonitorState>().building.count, 32);
    app.update();
    assert_eq!(app.world().resource::<ClipmapMonitorState>().building.count, 64);
    app.update();
    let report = &app.world().resource::<ClipmapMonitorState>().report;
    assert!(report.contains("73个网格") && report.contains("顶点 803 · 三角 511"));
    assert!(report.contains("累计重建/丢弃 146/0") && report.contains("另67个网格已计入总数"));
}

#[test]
fn hidden_monitor_does_no_sampling_and_new_scene_drops_partial_old_totals() {
    let mut app = fixture(73);
    app.world_mut().resource_mut::<ResourceMonitorState>().visible = false;
    app.update();
    assert!(app.world().resource::<ClipmapMonitorState>().cursor.is_none());
    app.world_mut().resource_mut::<ResourceMonitorState>().visible = true;
    app.update();
    assert_eq!(app.world().resource::<ClipmapMonitorState>().building.count, 32);
    app.world_mut().resource_mut::<SceneInstanceRootRequest>().scene_id = Some(SceneId::parse("MAP-other").unwrap());
    app.update();
    assert_eq!(app.world().resource::<ClipmapMonitorState>().building.count, 0);
    assert!(app.world().resource::<ClipmapMonitorState>().report.is_empty());
}

#[test]
fn inactive_batches_and_disabled_resident_instances_do_not_inflate_totals() {
    let mut app = fixture(3);
    let entity = *app.world().resource::<InstanceResourceTable<SceneTerrainDisplayLodStatus>>().values.keys().next().unwrap();
    app.world_mut().entity_mut(entity).insert(Disabled);
    let second = *app.world().resource::<InstanceResourceTable<SceneTerrainDisplayLodStatus>>().values.keys().nth(1).unwrap();
    let candidate = app.world_mut().spawn_empty().id();
    app.world_mut().get_mut::<SubsceneInstance>(second).unwrap().batch = candidate;
    app.update();
    assert!(app.world().resource::<ClipmapMonitorState>().report.contains("1个网格"));
}
