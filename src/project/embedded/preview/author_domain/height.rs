//! 局部高度在后台生成本地来源；只有真实发布成功才续接作者时钟到本地GPU票据。
use super::super::super::{systems, tool_brush::HeightSourceBindings, EmbeddedProjectState};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_scene_content::{
    SceneId, SparseHeightLiveOverride, SparseHeightTile, TerrainId,
    embedded_snapshot::SparseHeightSnapshotTile,
};
use std::{collections::BTreeSet, sync::Arc};
pub(super) struct Prepared {
    scene_id: SceneId,
    terrain: Option<TerrainId>,
    revision: u64,
    reset: bool,
    tiles: Vec<Arc<SparseHeightTile>>,
}
pub(super) fn prepare(
    scene_id: SceneId,
    terrain: Option<TerrainId>,
    revision: u64,
    reset: bool,
    input: Vec<SparseHeightSnapshotTile>,
) -> Result<Prepared, String> {
    let mut keys = BTreeSet::new();
    let mut tiles = Vec::with_capacity(input.len());
    for tile in input {
        if !keys.insert(tile.key) {
            return Err("高度预览包含重复瓦片".into());
        }
        let tile = SparseHeightTile::decode_asset_bytes(tile.key, &tile.bytes)?;
        if tile.meter_bounds().is_none() {
            return Err("高度预览缺少受检米制范围".into());
        }
        tiles.push(Arc::new(tile));
    }
    Ok(Prepared {
        scene_id,
        terrain,
        revision,
        reset,
        tiles,
    })
}
struct Pending {
    scene_id: SceneId,
    terrain: Option<TerrainId>,
    revision: u64,
    snapshot_revision: Option<u64>,
    task: Task<Result<SparseHeightLiveOverride, String>>,
}
#[derive(Default)]
pub(super) struct State {
    pending: Option<Pending>,
    #[cfg(feature = "validation")]
    last: Option<(SceneId, Option<TerrainId>, u64, u64)>,
}
impl State {
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }
    pub(super) fn start(&mut self, world: &World, value: Prepared) -> Result<(), String> {
        let mut base = systems::height_preview_source(world, &value.scene_id, value.terrain.as_ref())
            .unwrap_or_default();
        let ticket = systems::next_height_source_revision()?;
        let map = value.scene_id.clone();
        self.pending = Some(Pending {
            scene_id: value.scene_id,
            terrain: value.terrain,
            revision: value.revision,
            snapshot_revision: world.get_resource::<EmbeddedProjectState>().map(|state| state.revision),
            task: AsyncComputeTaskPool::get().spawn(async move {
                if value.reset {
                    base = SparseHeightLiveOverride::default();
                }
                base.retarget(Some(map.clone()), ticket);
                base.publish_tiles(&map, ticket, value.tiles)?;
                Ok(base)
            }),
        });
        Ok(())
    }
    pub(super) fn advance(
        &mut self,
        world: &mut World,
    ) -> Result<Option<Option<SparseHeightLiveOverride>>, String> {
        if self.pending.as_ref().is_some_and(|pending| pending.snapshot_revision.is_some_and(|revision|
            world.get_resource::<EmbeddedProjectState>().is_none_or(|state| state.revision != revision))) {
            // 先拒绝采用、保留后台结果所有权；不能取出迟到的大高度树再在帧内析构。
            return Err("局部高度完成时所属完整快照已经改变".into());
        }
        let Some(result) = self
            .pending
            .as_mut()
            .and_then(|pending| check_ready(&mut pending.task))
        else {
            return Ok(None);
        };
        let pending = self.pending.take().unwrap();
        let source = result?;
        let ticket = source.revision();
        let old = systems::publish_height_preview(
            world,
            &pending.scene_id,
            pending.terrain.clone(),
            source,
        )?;
        if let Some(snapshot_revision) = pending.snapshot_revision
            && let Some(mut bindings) = world.get_resource_mut::<HeightSourceBindings>() {
            bindings.published_preview(&pending.scene_id, pending.terrain.as_ref(), snapshot_revision, pending.revision, ticket);
        } // 映射推进不能早于真实来源发布，失败不得给刷圈授予尚未采用的高度信用。
        #[cfg(feature = "validation")]
        { self.last = Some((pending.scene_id.clone(), pending.terrain.clone(), pending.revision, ticket)); }
        Ok(Some(old))
    }
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World) -> serde_json::Value {
        self.last.as_ref().map_or(serde_json::Value::Null,|(map,terrain,revision,ticket)| {
            let source=systems::height_preview_source(world,map,terrain.as_ref());
            serde_json::json!({"scene_id":map,"terrain":terrain,"author_revision":revision,"ticket":ticket,
                "adopted_ticket":source.as_ref().map(SparseHeightLiveOverride::revision),"tiles":source.as_ref().map(SparseHeightLiveOverride::tile_count)})
        })
    }
}

#[cfg(test)]
mod tests;
