//! 显式小工程验收从RenderWorld读取真实Mesh计数；不保留或复制生产网格的CPU顶点数据。

use bevy::{
    asset::AssetId,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        mesh::{RenderMesh, RenderMeshBufferInfo},
        render_asset::RenderAssets,
    },
};
use std::sync::{Arc, Mutex};

const MAX_REQUESTS: usize = 32;

#[derive(Clone, Copy, Debug)]
pub(super) struct Counts {
    pub vertices: u32,
    /// 没有索引缓冲时为0，不以顶点数伪造索引数量。
    pub indices: u32,
}

#[derive(Default)]
struct State {
    requested: Vec<AssetId<Mesh>>,
    complete: Vec<(AssetId<Mesh>, Counts)>,
}

/// MainWorld和RenderWorld共享固定上限的小型元数据；两侧只try_lock，不阻塞帧线程。
#[derive(Resource, Clone, Default)]
struct Probe(Arc<Mutex<State>>);

/// 只属于MainWorld；ready同次取得完整回执后复制，随后snapshot读取不再与Render线程争锁。
#[derive(Resource, Default)]
struct MainReceipt(Vec<(AssetId<Mesh>, Counts)>);

pub(super) fn install(app: &mut App) -> Result<(), String> {
    if app.world().contains_resource::<Probe>() {
        return Ok(());
    }
    let probe = Probe::default();
    let Some(render) = app.get_sub_app_mut(RenderApp) else {
        return Err("专业验收需要真实RenderApp以读取GPU网格元数据".into());
    };
    render.insert_resource(probe.clone()).add_systems(
        Render,
        publish
            .in_set(RenderSystems::Cleanup)
            .after(RenderSystems::PrepareAssets),
    );
    app.insert_resource(probe).init_resource::<MainReceipt>();
    Ok(())
}

/// 全部请求属于同一有界集合才算完成；集合换代立即清空旧回执，空集与重复ID均拒绝。
pub(super) fn ready(world: &mut World, requested: &[AssetId<Mesh>]) -> bool {
    let Some(probe) = world.get_resource::<Probe>().cloned() else {
        return false;
    };
    world.resource_mut::<MainReceipt>().0.clear();
    let Ok(mut state) = probe.0.try_lock() else {
        return false;
    };
    if requested.is_empty()
        || requested.len() > MAX_REQUESTS
        || requested
            .iter()
            .enumerate()
            .any(|(index, id)| requested[..index].contains(id))
    {
        state.requested.clear();
        state.complete.clear();
        return false;
    }
    let same_set = state.requested.len() == requested.len()
        && requested.iter().all(|id| state.requested.contains(id));
    if !same_set {
        state.requested = requested.to_vec();
        state.complete.clear();
        return false;
    }
    let complete = state.complete.len() == state.requested.len()
        && state
            .requested
            .iter()
            .all(|id| state.complete.iter().any(|(ready, _)| ready == id));
    if complete {
        world.resource_mut::<MainReceipt>().0 = state.complete.clone();
    }
    complete
}

pub(super) fn get(world: &World, id: AssetId<Mesh>) -> Option<Counts> {
    world
        .get_resource::<MainReceipt>()?
        .0
        .iter()
        .find_map(|(ready, counts)| (*ready == id).then_some(*counts))
}

fn publish(probe: Res<Probe>, meshes: Res<RenderAssets<RenderMesh>>) {
    let Ok(mut state) = probe.0.try_lock() else {
        return;
    };
    // 每次用当前RenderAssets重建至多32条计数；资产缺失即撤回旧计数，不缓存已退役网格。
    let mut complete = Vec::with_capacity(state.requested.len());
    for id in &state.requested {
        let Some(mesh) = meshes.get(*id) else {
            continue;
        };
        let indices = match mesh.buffer_info {
            RenderMeshBufferInfo::Indexed { count, .. } => count,
            RenderMeshBufferInfo::NonIndexed => 0,
        };
        complete.push((
            *id,
            Counts {
                vertices: mesh.vertex_count,
                indices,
            },
        ));
    }
    state.complete = complete;
}
