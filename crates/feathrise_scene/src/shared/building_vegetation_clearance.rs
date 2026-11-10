//! 正式建筑摆放与 F12 未保存草稿共用的植被净空快照。
//!
//! 【功能设计】正式摆放批次和作者建筑索引分别发布完整不可变四边形数组；
//! 作者快照存在时优先用于同图预览，清除后自动回到最近一次正式批次，始终不改写植被作者 R8。

use std::sync::Arc; // Mesh 分块与 Blade 遮罩后台任务只浅克隆完整净空数组。

use bevy::prelude::*; // Resource 为场景和 F12 提供同一主世界交换边界。
use feathrise_scene_content::{BuildingWorldQuad, SceneId}; // 净空使用建筑域唯一世界四边形公式。

/// F12 建筑索引发布净空时携带的完整作者身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildingVegetationClearanceAuthoringIdentity {
    pub scene_id: SceneId,           // 切换场景后的旧索引不得覆盖新场景。
    pub author_revision: u64,    // 移动、删除和撤销都会推进作者修订。
    pub structure_revision: u64, // 增删 placement 后数组身份立即换代。
    pub catalog_revision: u64,   // 预设净空参数热更后必须重建派生结果。
}

/// 一个可安全交给后台任务的当前建筑植被净空快照。
#[derive(Clone, Debug)]
pub struct BuildingVegetationClearanceSnapshot {
    pub scene_id: SceneId,                     // 快照只对这一个场景生效。
    pub revision: u64,                     // 每次真正换源后单调递增。
    pub regions: Arc<[BuildingWorldQuad]>, // 完整采用前不暴露部分四边形前缀。
    pub floor_heights: Arc<[f32]>, // 与四边形一一对应的地基Y；空数组为旧二维来源，由消费者按外表面解释。
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FormalIdentity {
    scene_id: SceneId,           // 正式批次所属场景。
    placement_revision: u64, // 显式摆放运行时的原子采用修订。
    catalog_revision: u64,   // 生成四边形时使用的建筑目录修订。
}

#[derive(Clone, Debug)]
struct StoredClearance {
    scene_id: SceneId,                     // 查询时拒绝跨场景复用。
    revision: u64,                     // 存入时分配的全局换代号。
    regions: Arc<[BuildingWorldQuad]>, // 后台消费者只复制 Arc 句柄。
    floor_heights: Arc<[f32]>,         // 三维净空从建筑所在层向下查找，不误清除同XZ的另一层。
}

/// 建筑净空的正式值与可选 F12 作者覆盖。
#[derive(Resource, Default)]
pub struct BuildingVegetationClearanceState {
    next_revision: u64,                      // 零保留给“从未发布”，实际快照从一开始。
    formal_identity: Option<FormalIdentity>, // 相同正式输入不会在稳定帧重复发布。
    formal: Option<StoredClearance>,         // 保存/热更失败期间继续保留上一完整正式值。
    authoring_identity: Option<BuildingVegetationClearanceAuthoringIdentity>, // F12 BVH 联合身份。
    authoring: Option<StoredClearance>,      // 存在时只覆盖同一场景的正式净空。
}

impl BuildingVegetationClearanceState {
    /// 发布已经完整采用的正式摆放批次净空。
    #[cfg(test)]
    pub(crate) fn set_formal(
        &mut self,
        scene_id: SceneId,                     // 候选批次采用时冻结的场景身份。
        placement_revision: u64,           // 公开 placement 原子采用修订。
        catalog_revision: u64,             // 候选规划使用的建筑目录修订。
        regions: Arc<[BuildingWorldQuad]>, // 完整候选净空数组。
    ) {
        self.set_formal_layered(
            scene_id,
            placement_revision,
            catalog_revision,
            regions,
            Arc::default(),
        );
    }

    /// 正式摆放同时冻结地基高度；高度和四边形必须在同一批次原子发布。
    pub(crate) fn set_formal_layered(
        &mut self,
        scene_id: SceneId,
        placement_revision: u64,
        catalog_revision: u64,
        regions: Arc<[BuildingWorldQuad]>,
        floor_heights: Arc<[f32]>,
    ) {
        debug_assert!(floor_heights.is_empty() || floor_heights.len() == regions.len());
        let identity = FormalIdentity {
            scene_id: scene_id.clone(), // 组成稳定正式来源键。
            placement_revision,     // 同摆放修订只发布一次。
            catalog_revision,       // 净空参数换代也必须形成新来源。
        };
        if self.formal_identity.as_ref() == Some(&identity) {
            return; // 稳定 Ready 帧不推进净空修订或触发植被重算。
        }
        let revision = self.allocate_revision(); // 先取得唯一换代号再写入完整值。
        self.formal_identity = Some(identity); // 身份和值在同一主线程调用中更新。
        self.formal = Some(StoredClearance {
            scene_id,   // 正式来源场景。
            revision, // 动态植被据此拒收迟到分块和遮罩。
            regions,  // Arc 所有权直接移入，不复制四边形。
            floor_heights,
        });
    }

    /// 离开正式场景时撤销其净空；不影响另一张图的作者覆盖清理。
    pub(crate) fn clear_formal(&mut self) {
        if self.formal.take().is_some() || self.formal_identity.take().is_some() {
            self.allocate_revision(); // 即使当前有作者覆盖，也保留正式来源换代的全局顺序。
        }
    }

    /// F12 在建筑 BVH 与完整保存审计同时就绪后发布作者覆盖。
    pub fn set_authoring(
        &mut self,
        identity: BuildingVegetationClearanceAuthoringIdentity, // 场景、作者、结构和目录联合身份。
        regions: Vec<BuildingWorldQuad>, // 从已完成不可变 BVH 一次复制固定大小四边形。
    ) {
        self.set_authoring_layered(identity, Arc::from(regions), Arc::default());
    }

    /// 作者BVH在后台提前冻结净空，采用帧仅浅克隆两份Arc，不扫描完整建筑数组。
    pub fn set_authoring_layered(
        &mut self,
        identity: BuildingVegetationClearanceAuthoringIdentity,
        regions: Arc<[BuildingWorldQuad]>,
        floor_heights: Arc<[f32]>,
    ) {
        debug_assert!(floor_heights.is_empty() || floor_heights.len() == regions.len());
        if self.authoring_identity.as_ref() == Some(&identity) {
            return; // 稳定 F12 帧不重复分配 Arc 或推进修订。
        }
        let scene_id = identity.scene_id.clone(); // 值与身份共享同一场景来源。
        let revision = self.allocate_revision(); // 迟到旧作者任务不能取得当前换代号。
        self.authoring_identity = Some(identity); // 先登记完整来源身份。
        self.authoring = Some(StoredClearance {
            scene_id,   // 只覆盖当前作者场景。
            revision, // Mesh 与 Blade 共用这一个换代门。
            regions,
            floor_heights,
        });
    }

    /// 关闭 F12 或作者索引失效时回到上一完整正式快照。
    pub fn clear_authoring(&mut self) {
        if self.authoring.take().is_some() || self.authoring_identity.take().is_some() {
            self.allocate_revision(); // 回退正式值也必须使当前消费者观察到来源变化。
        }
    }

    /// 返回指定场景当前应使用的完整净空；同图作者值优先于正式值。
    pub fn snapshot_for(&self, scene_id: &SceneId) -> Option<BuildingVegetationClearanceSnapshot> {
        let stored = self
            .authoring
            .as_ref()
            .filter(|snapshot| snapshot.scene_id == *scene_id)
            .or_else(|| {
                self.formal
                    .as_ref()
                    .filter(|snapshot| snapshot.scene_id == *scene_id)
            })?; // 跨场景来源不得被当作空净空误用。
        Some(BuildingVegetationClearanceSnapshot {
            scene_id: stored.scene_id.clone(),   // 小型稳定身份供后台结果复核。
            revision: stored.revision,       // 来源切换、移动和目录热更都会改变。
            regions: stored.regions.clone(), // 大数组只增加一个 Arc 引用。
            floor_heights: stored.floor_heights.clone(), // 完整同源层高度不在帧路径复制。
        })
    }

    /// 空场景仍发布可区分的零区域正式快照，因此删除最后建筑能够恢复植被。
    pub(crate) fn empty_regions() -> Arc<[BuildingWorldQuad]> {
        Arc::from(Vec::<BuildingWorldQuad>::new()) // 显式空切片代表已完成而不是尚未就绪。
    }

    fn allocate_revision(&mut self) -> u64 {
        self.next_revision = self.next_revision.wrapping_add(1).max(1); // 回绕时跳过未发布哨兵零。
        self.next_revision // 返回本次完整来源换代号。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 纯资源回归不需要启动渲染或资产服务器。

    fn quad(x: f32) -> BuildingWorldQuad {
        BuildingWorldQuad {
            corners: [Vec2::splat(x); 4], // 资源测试只比较来源和数组，不执行几何运算。
        }
    }

    #[test]
    fn authoring_snapshot_overrides_and_then_restores_formal_regions() {
        let scene_id = SceneId::parse("MAP-clearance").expect("测试场景 ID 合法");
        let mut state = BuildingVegetationClearanceState::default(); // 从无快照开始。
        state.set_formal(scene_id.clone(), 3, 5, Arc::from(vec![quad(1.0)])); // 发布正式建筑。
        let formal = state.snapshot_for(&scene_id).expect("正式快照应可查询");
        state.set_authoring(
            BuildingVegetationClearanceAuthoringIdentity {
                scene_id: scene_id.clone(), // 作者覆盖属于同图。
                author_revision: 7,     // 完整作者修订。
                structure_revision: 11, // 完整结构修订。
                catalog_revision: 5,    // 与正式目录一致也仍是独立来源。
            },
            vec![quad(2.0)], // 未保存移动后的净空。
        );
        let authoring = state.snapshot_for(&scene_id).expect("作者快照应覆盖正式值");
        assert_ne!(authoring.revision, formal.revision); // 来源换代必须拒收旧植被结果。
        assert_eq!(authoring.regions[0].corners[0], Vec2::splat(2.0)); // 当前返回作者几何。
        state.clear_authoring(); // 模拟关闭 F12 或索引失效。
        let restored = state.snapshot_for(&scene_id).expect("应恢复上一正式快照");
        assert_eq!(restored.revision, formal.revision); // 正式快照本身没有被作者操作改写。
        assert_eq!(restored.regions[0].corners[0], Vec2::splat(1.0)); // 原始正式几何恢复。
    }
}
