//! 未保存稀疏高度的共享局部覆盖；F12 发布块快照，场景只按场景与修订读取。

use std::collections::BTreeMap; // 仅当前受检发布批次临时按键去重，不保留随全图增长的可变镜像。
use std::sync::Arc; // 编辑器、GPU页任务和碰撞任务共享不可变块，不深拷贝高度数组。

use bevy::prelude::Resource; // 资源可由 scene_content、scene 与独立 F12 幂等初始化。

use super::{SparseHeightTile, SparseHeightTileKey}; // 覆盖使用与磁盘完全相同的受检块结构。
use crate::SceneId; // 覆盖不能跨场景沿用。

mod snapshot;
pub use snapshot::SparseHeightLiveSnapshot;
use snapshot::LiveTileIndex;

/// 当前场景作者会话尚未保存或仍需在玩法中保持的局部高度块。
#[derive(Resource, Default, Clone)]
pub struct SparseHeightLiveOverride {
    scene_id: Option<SceneId>, // None 表示正式磁盘资产拥有全部高度权威。
    revision: u64,         // 编辑器 terrain_revision，迟到页任务必须精确核对。
    index: LiveTileIndex, // 唯一完整块表及Mip占用；Clone只共享根，后续雕刻只复制触及键的32层路径。
}

impl SparseHeightLiveOverride {
    /// 沿确定性四叉顺序推进捕获；返回键只作不透明游标，每次固定深度定位下一块，不扫描全树。
    pub fn tile_after(&self, after: Option<SparseHeightTileKey>) -> Option<(SparseHeightTileKey, &Arc<SparseHeightTile>)> {
        self.index.tile_after(after)
    }

    /// 当前覆盖所属场景；场景切换场景时不允许猜测或沿用旧块。
    pub fn scene_id(&self) -> Option<&SceneId> {
        self.scene_id.as_ref()
    }

    /// 当前完整覆盖修订；每次真实笔刷、撤销或重做后推进。
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 当前实际覆盖块数，只随作者触及范围增长，不按整个场景预分配。
    pub fn tile_count(&self) -> usize {
        self.index.len()
    }

    /// 只读根节点缓存；捕获相机/草地范围不得在主帧扫描所有实时页。
    pub fn height_bounds(&self) -> Option<[f32; 2]> {
        self.index.height_bounds()
    }

    /// 取得不可变局部块强引用；显式平地块也返回 Some 以覆盖磁盘旧凸起。
    pub fn tile(&self, key: SparseHeightTileKey) -> Option<&Arc<SparseHeightTile>> {
        self.index.tile(key)
    }

    /// 场景只查询实际窗口内的层级页；像素更新不会改变占用，显式全平块仍须覆盖旧磁盘高度。
    pub fn has_mip_tiles(&self, level: u8, key: SparseHeightTileKey) -> bool {
        self.index.has_mip_tiles(level, key)
    }

    /// 只捕获小型场景/修订与持久根；相关块的遍历和收集必须在调用方已有的后台任务中执行。
    pub fn snapshot(&self) -> SparseHeightLiveSnapshot {
        self.index.snapshot(self.scene_id.clone(), self.revision)
    }

    /// 遍历全部当前覆盖，供场景在场景/修订变化时构建局部页候选。
    pub fn tiles(
        &self,
    ) -> impl ExactSizeIterator<Item = (SparseHeightTileKey, &Arc<SparseHeightTile>)> {
        self.index.tiles()
    }

    /// 切换场景或草稿真正失效时清除所有强引用，并发布调用方当前地形修订。
    /// 帧内切换场景应先把非空旧资源整体移交既有后台回收槽，再对空资源调用；同场景更新修订不会清索引。
    pub fn retarget(&mut self, scene_id: Option<SceneId>, revision: u64) {
        if self.scene_id != scene_id {
            self.index = LiveTileIndex::default(); // 旧图平地覆盖也不能进入新图。
        }
        self.scene_id = scene_id;
        self.revision = revision;
    }

    /// 原子发布一批触及块；同图旧修订和块身份不一致均被拒绝。
    pub fn publish_tiles(
        &mut self,
        scene_id: &SceneId,
        revision: u64,
        tiles: impl IntoIterator<Item = Arc<SparseHeightTile>>,
    ) -> Result<usize, String> {
        SceneId::parse(scene_id.as_str())?;
        if self.scene_id.as_ref() != Some(scene_id) {
            return Err("稀疏高度实时覆盖场景身份不一致".into());
        }
        if revision < self.revision {
            return Err("稀疏高度实时覆盖修订已经过期".into());
        }
        let mut pending = BTreeMap::new();
        for tile in tiles {
            if tile.meter_bounds().is_none() {
                return Err("实时高度覆盖必须来自已绑定米制高度的作者页".into());
            }
            if pending.insert(tile.key, tile).is_some() {
                return Err("同一批稀疏高度实时覆盖包含重复块".into());
            }
        }
        let count = pending.len();
        for (key, tile) in pending {
            self.index.insert(key, tile); // 受检批次只更新固定地址路径；旧实例和worker保有完整独立版本。
        }
        self.revision = revision;
        Ok(count)
    }

    /// 草稿失效、显式放弃或切换场景时归还正式磁盘权威；清空后仍推进修订以拒收迟到结果。
    /// 此便捷接口同步交还所有权；帧关键路径应移动整份资源到既有回收槽，避免最后一个根Arc递归释放全树。
    pub fn clear(&mut self, revision: u64) {
        self.scene_id = None;
        self.revision = revision;
        self.index = LiveTileIndex::default();
    }
}

#[cfg(test)]
mod occupancy_tests {
    use super::*;
    use crate::SparseHeightSpec;

    #[test]
    fn live_mip_occupancy_is_incremental_atomic_and_cleared_with_its_scene() {
        let spec = SparseHeightSpec::new([-512.0; 2], [512.0; 2], 0.125, -8.0, 16.0).unwrap();
        let map = SceneId::parse("MAP-live-occupancy").unwrap();
        let mut live = SparseHeightLiveOverride::default();
        live.retarget(Some(map.clone()), 1);
        let a = SparseHeightTileKey { x: 3, z: 5 };
        let b = SparseHeightTileKey { x: 6, z: 7 };
        let a_tile = Arc::new(SparseHeightTile::blank(&spec, a).unwrap());
        let b_tile = Arc::new(SparseHeightTile::blank(&spec, b).unwrap());
        live.publish_tiles(&map, 2, [a_tile.clone()]).unwrap();
        for level in 0..=spec.max_mip_level() {
            assert!(live.has_mip_tiles(level, spec.mip_ancestor_key(a, level).unwrap()));
        }
        assert!(!live.has_mip_tiles(0, b));
        let count = live.tile_count();
        live.publish_tiles(&map, 3, [a_tile]).unwrap();
        assert_eq!(live.tile_count(), count, "重复雕刻与全平覆盖不能增大占用索引");
        let before = live.snapshot();
        assert!(live.publish_tiles(&map, 4, [b_tile.clone(), b_tile.clone()]).is_err());
        assert_eq!(live.revision(), before.revision(), "坏批次不能提前污染任何祖先");
        assert!(!live.has_mip_tiles(0, b));
        assert!(live.publish_tiles(&map, 2, [b_tile.clone()]).is_err());
        assert_eq!(live.tile_count(), before.tile_count());
        assert!(!live.has_mip_tiles(0, b));
        live.publish_tiles(&map, 4, [b_tile]).unwrap();
        assert!(live.has_mip_tiles(0, b));
        live.retarget(Some(SceneId::parse("MAP-another-source").unwrap()), 5);
        assert!(!live.has_mip_tiles(32, SparseHeightTileKey { x: 0, z: 0 }));
        assert_eq!(live.tile_count(), 0);
        live.retarget(Some(map), 6);
        live.clear(7);
        assert!(!live.has_mip_tiles(2, SparseHeightTileKey { x: 0, z: 1 }));
    }
}
