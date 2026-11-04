//! 仅F3总览可见时采样；每帧至多读取32份状态，换地图或批次立即废弃未完成快照。

use super::resources::*;
use bevy::prelude::*;
use feathrise_map_content::{MapInstanceRootRequest, MapTerrainDisplayLodStatus};
use feathrise_scene::{SubMapActive, SubMapInstance};
use feathrise_scene_core::residency::InstanceResourceTable;
use crate::resources::{ResourceMonitorPage, ResourceMonitorState};
use std::ops::Bound::{Excluded, Unbounded};

pub(in crate::resource_monitor) fn sample_clipmaps(
    time: Res<Time<Real>>,
    mut monitor: ResMut<ResourceMonitorState>,
    request: Option<Res<MapInstanceRootRequest>>,
    root: Option<Res<MapTerrainDisplayLodStatus>>,
    instances: Option<Res<InstanceResourceTable<MapTerrainDisplayLodStatus>>>,
    batches: Query<Entity, With<SubMapActive>>,
    contexts: Query<(&SubMapInstance, &Name)>,
    mut state: ResMut<ClipmapMonitorState>,
) {
    if !monitor.visible || monitor.page != ResourceMonitorPage::Overview {
        state.sampling = false;
        state.next_sample = 0.0;
        return; // 隐藏不扫描；重新打开从完整新一轮开始。
    }
    let identity = request.as_deref().and_then(|request| request.map_id.clone())
        .map(|map| (map, batches.single().ok())); // Bevy默认查询排除已Disabled的驻留地图。
    if state.identity != identity {
        *state = ClipmapMonitorState { identity, ..default() };
        monitor.force_refresh = true;
    }
    let Some((map, batch)) = state.identity.clone() else { return; };
    let now = time.elapsed_secs_f64();
    if !state.sampling {
        if now < state.next_sample { return; }
        state.sampling = true;
        state.cursor = None;
        state.building = Snapshot::default();
        if let Some(root) = root.as_deref().filter(|status| status.map_id.as_ref() == Some(&map)) {
            state.building.observe("地图基础网格", root);
        }
    }
    let mut visited = 0;
    if let Some(instances) = instances {
        let start = state.cursor.map_or(Unbounded, Excluded);
        for (&entity, status) in instances.values.range((start, Unbounded)).take(32) {
            visited += 1;
            state.cursor = Some(entity);
            let Ok((instance, name)) = contexts.get(entity) else { continue; };
            if Some(instance.batch) == batch && instance.path.root_map_id == map {
                state.building.observe(name.as_str(), status);
            } // 未采用候选、旧批次和其他地图不进入总数。
        }
    }
    if visited < 32 {
        state.report = state.building.report(); // 完整一轮才发布；至多格式化六行，不深拷贝作者列表。
        state.building = Snapshot::default();
        state.sampling = false;
        state.next_sample = now + 0.5;
        monitor.force_refresh = true;
    }
}
