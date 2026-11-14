//! 显式实例隐藏候选根和World实例完成标记。

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
}; // 整批根共享一个O(1)三态门，采用或退役帧不逐实体改作者身份。

use bevy::prelude::*;
use crate::shared::authoring_version::AuthoringVersion; // 作者重施和正式批次采用是两个独立身份。
use feathrise_scene_content::{
    SceneId, ScenePlacementAssetTarget, ScenePlacementRecord, ScenePlacementWaterSurface,
}; // 根冻结场景、文件内目标与完整作者记录的非水结构证据，水数组由当前覆盖组件共同证明。

/// 共享批次门的内部状态；数值只写入同一`AtomicU8`，不得持久化或跨进程解释。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum PlacementInstanceState {
    Pending = 0, // 当前隐藏正式候选可以参与同快照保存等待，但不能接受F12写回。
    Active = 1,  // 已原子采用正式根或当前作者会话精确预览可以接受作者写回。
    Retired = 2, // 已切换场景、换代或取消的根仅等待有界清理，不能再提供保存证据。
}

/// 一批placement根共享的作者可见性门。
///
/// 【功能设计】物理`PlacementPendingInstance`只负责场景内部隔离与分帧清理；
/// F12是否能写入必须由本门整批切换，避免8192根逐项解除标记时出现半批可编辑。
#[derive(Clone, Debug)]
pub(crate) struct PlacementInstanceGate(Arc<AtomicU8>);

impl PlacementInstanceGate {
    pub(super) fn new(active: bool) -> Self {
        let state = if active {
            PlacementInstanceState::Active // 编辑器精确预览创建后立即属于当前作者会话。
        } else {
            PlacementInstanceState::Pending // 正式候选在整批采用前保持隐藏等待态。
        };
        Self(Arc::new(AtomicU8::new(state as u8))) // 每批只分配一个共享原子标量。
    }

    pub(super) fn activate(&self) {
        self.0
            .store(PlacementInstanceState::Active as u8, Ordering::Release); // 可见容器发布前后以一次Release切换整批作者身份。
    }

    pub(super) fn retire(&self) {
        self.0
            .store(PlacementInstanceState::Retired as u8, Ordering::Release); // 切换场景、取消或替换时永久撤销本批保存与作者资格。
    }

    fn is_active(&self) -> bool {
        self.state() == PlacementInstanceState::Active // F12读取时与采用侧Release建立完整身份边界。
    }

    fn is_pending(&self) -> bool {
        self.state() == PlacementInstanceState::Pending // 只有仍由当前隐藏候选拥有的根可参与首次保存等待。
    }

    fn state(&self) -> PlacementInstanceState {
        match self.0.load(Ordering::Acquire) {
            value if value == PlacementInstanceState::Pending as u8 => {
                PlacementInstanceState::Pending // 初建候选保持等待。
            }
            value if value == PlacementInstanceState::Active as u8 => {
                PlacementInstanceState::Active // 整批已正式采用。
            }
            _ => PlacementInstanceState::Retired, // 未知值也失败关闭为不可保存、不可编辑的退役态。
        }
    }
}

/// 标记一个已保存显式摆放记录对应的唯一Scene0或直接primitive根。
///
/// 【功能设计】F12以稳定摆放ID直接定位正式实例根，不通过名称推断、
/// 不逐帧扫描世界；ID字段保持私有，外部只能读取而不能破坏运行时身份。
#[derive(Component, Clone, Debug)] // 字符串逐根保存，活动门则由同批全部根共享。
pub struct ScenePlacementInstance {
    scene_id: SceneId,      // 同一稳定placement ID只在所属场景内有效，切换场景残留根不能串图证明。
    id: String,         // 私有字段阻止实例生命期中被属性面板意外改名。
    asset_path: String, // 冻结创建该可视根的GLB/GLTF作者来源。
    asset_target: ScenePlacementAssetTarget, // Scene0或Mesh/Primitive/Material数字组合必须参与保存证明。
    building_preset_id: Option<String>,    // 建筑身份变化必须重建或重施根证据，普通资产保持None。
    terrain_patch: Option<feathrise_scene_content::ScenePlacementTerrainPatch>, // 洞口与真实碰撞一起参与保存证明。
    mesh_lod: Option<feathrise_scene_content::MeshLodPolicy>, // 显式网格策略属于作者身份，换策略不能借用旧根证明。
    translation: [f32; 3], // 保存证明比较原始JSON平移，不用归一化后的Transform猜作者值。
    yaw_degrees: f32,      // 保存证明比较原始有限Y轴角度快照。
    scale: [f32; 3],       // 保存证明比较原始三轴缩放快照。
    gate: PlacementInstanceGate, // 采用/退役只需O(1)切换整批，不扫描世界或逐根改组件。
    previous_gate: Option<PlacementInstanceGate>, // 未变根跨候选保留旧活动信用；最多两票据，禁止递归门链。
    authoring_version: Option<AuthoringVersion>, // 初始版本浅借gate；同批热改独立换票据而不拆正式采用门。
} // 结束显式摆放根身份组件。

impl ScenePlacementInstance {
    /// 为当前作者会话的一条已校验摆放记录建立活动预览根身份。
    pub fn new(scene_id: &SceneId, record: &ScenePlacementRecord) -> Self {
        Self {
            scene_id: scene_id.clone(),                // 预览根只证明创建它的当前草稿场景。
            id: record.id.clone(),                 // 稳定记录ID与其余结构字段来自同一不可变快照。
            asset_path: record.asset_path.clone(), // 后续换资产必须重建预览，不能原地漂移证据。
            asset_target: record.asset_target,     // 保存、撤销与重新进图都必须恢复同一个子资产。
            building_preset_id: record.building_preset_id.clone(), // 预览根冻结创建时的规范建筑身份。
            terrain_patch: record.terrain_patch.clone(),
            mesh_lod: record.mesh_lod,
            translation: record.translation, // 数组是Copy，不产生额外堆分配。
            yaw_degrees: record.yaw_degrees, // 保留作者原始角度值供精确比较。
            scale: record.scale,             // 保留作者原始缩放值供精确比较。
            gate: PlacementInstanceGate::new(true), // 编辑器独立预览根创建后立即属于作者活动集合。
            previous_gate: None,
            authoring_version: None,
        }
    } // 完成摆放根身份构造。

    /// 为隐藏正式候选建立共享批次身份；只有整批原子采用后才会变为活动。
    pub(super) fn new_pending(
        scene_id: &SceneId,
        record: &ScenePlacementRecord,
        gate: PlacementInstanceGate,
    ) -> Self {
        Self {
            scene_id: scene_id.clone(), // 正式候选的场景身份来自同一完整JSON修订。
            id: record.id.clone(),  // 根ID与场景句柄、变换和覆盖共享同一计划快照。
            asset_path: record.asset_path.clone(), // 保存门拒绝同ID但不同GLB来源的旧根。
            asset_target: record.asset_target, // 直接primitive不能借用同文件Scene0旧根的Ready信用。
            building_preset_id: record.building_preset_id.clone(), // 正式候选保留同修订建筑语义。
            terrain_patch: record.terrain_patch.clone(),
            mesh_lod: record.mesh_lod,
            translation: record.translation, // 精确保留计划生成前的作者平移。
            yaw_degrees: record.yaw_degrees, // 精确保留计划生成前的作者角度。
            scale: record.scale,             // 精确保留计划生成前的作者缩放。
            gate,                            // 全候选批次共享Pending、Active、Retired三态。
            previous_gate: None,
            authoring_version: None,
        }
    }

    /// 返回与`ScenePlacementRecord.id`完全一致的只读身份。
    pub fn id(&self) -> &str {
        &self.id // 借用读取避免F12枚举实例时为每项分配新字符串。
    } // 完成摆放ID读取。

    /// 返回创建该Scene0根时冻结的场景身份。
    pub fn scene_id(&self) -> &SceneId {
        &self.scene_id // 保存与诊断只借用稳定身份，不复制字符串。
    }

    /// 返回创建该根时冻结的Scene0或直接primitive目标。
    pub fn asset_target(&self) -> ScenePlacementAssetTarget {
        self.asset_target // 视口只读取不可变目标来区分Scene层级命中与直接子资产命中。
    }

    /// 通用表面后台读取与显示根共用同一来源路径，避免碰撞关闭后丢失种植几何。
    pub fn asset_path(&self) -> &str { &self.asset_path }

    /// 同Entity上的后来作者快照必须使旧保存失去回收权；克隆仍表示同一版本。
    pub fn authoring_version(&self) -> AuthoringVersion {
        self.authoring_version.clone().unwrap_or_else(||
            AuthoringVersion::from_shared_state(self.gate.0.clone()))
    }

    /// 更新同一内置摆放的作者快照，保留所属批次的采用与退役门。
    /// 【功能设计】属性热改不能用独立活动门替换正式共享门，否则旧批次退役后仍会冒充可编辑根。
    pub fn with_updated_record(&self, record: &ScenePlacementRecord) -> Option<Self> {
        if self.id != record.id || self.asset_path != record.asset_path
            || !self.asset_target.is_builtin() || !record.asset_target.is_builtin()
        {
            return None; // 切换记录或外部资产必须重建正式身份，不能借用本根原有信用。
        }
        let mut updated = Self::new_pending(&self.scene_id, record, self.gate.clone());
        updated.previous_gate = self.previous_gate.clone();
        updated.authoring_version = Some(AuthoringVersion::default());
        Some(updated) // 保留正式采用/退役门，同时撤销旧作者版本的保存回收租约。
    }

    /// 作者属性预览只更新LOD策略，保持原根的批次门和其他来源字段；正式保存仍走完整候选。
    pub fn with_author_preview_mesh_lod_policy(
        &self, policy: Option<feathrise_scene_content::MeshLodPolicy>,
    ) -> Result<Self, String> {
        if let Some(policy) = policy { policy.validate()?; }
        let mut updated = self.clone();
        if updated.mesh_lod != policy {
            updated.mesh_lod = policy;
            updated.authoring_version = Some(AuthoringVersion::default());
        } // 只撤销真实策略变化前的保存租约，不能把正式根换成永久活动的独立预览门。
        Ok(updated)
    }

    /// 仅变换热改复用真实运行根；资产、碰撞、赋水与LOD结构变化仍走正式候选。
    pub fn with_updated_transform_record(&self, record: &ScenePlacementRecord) -> Option<Self> {
        if self.id != record.id || self.asset_path != record.asset_path
            || self.asset_target != record.asset_target || self.building_preset_id != record.building_preset_id
            || self.terrain_patch != record.terrain_patch || self.mesh_lod != record.mesh_lod {
            return None;
        }
        let mut updated = Self::new_pending(&self.scene_id, record, self.gate.clone());
        updated.previous_gate = self.previous_gate.clone();
        updated.authoring_version = Some(AuthoringVersion::default());
        Some(updated)
    }

    /// 同一作者记录跨批次沿用实体；候选失败时仍借旧活动门，采用后只借新门。
    pub(super) fn carried_to(&self, gate: PlacementInstanceGate) -> Option<Self> {
        let previous = if self.gate.is_active() { self.gate.clone() }
            else { self.previous_gate.as_ref().filter(|gate| gate.is_active())?.clone() };
        let mut carried = self.clone();
        carried.authoring_version = Some(self.authoring_version()); // 换容器不是作者修改，不撤销当前保存版本。
        carried.gate = gate;
        carried.previous_gate = Some(previous);
        Some(carried)
    }

    /// 返回该根是否必须等待静态网格碰撞完成。
    pub(crate) fn is_building(&self) -> bool {
        self.building_preset_id.is_some() || self.uses_async_mesh_collision()
    }

    pub(crate) fn uses_async_mesh_collision(&self) -> bool {
        self.is_terrain_patch() || self.asset_target.has_primitive_collision()
    }

    pub fn is_terrain_patch(&self) -> bool {
        self.terrain_patch.is_some()
    }

    /// 显示与碰撞独立继承全局策略；建筑显示也使用该契约。
    pub fn mesh_lod_policy(&self) -> feathrise_scene_content::MeshLodPolicy {
        self.mesh_lod.unwrap_or_default()
    }

    pub(crate) fn terrain_patch(&self) -> Option<&feathrise_scene_content::ScenePlacementTerrainPatch> {
        self.terrain_patch.as_ref()
    }

    /// 当前根与草稿记录及实际水覆盖是否组成同一个完整作者快照。
    pub fn matches_record(
        &self,
        scene_id: &SceneId,
        record: &ScenePlacementRecord,
        water_surfaces: &[ScenePlacementWaterSurface],
    ) -> bool {
        self.scene_id == *scene_id // 跨场景同名placement首先失败关闭。
            && self.id == record.id // 同图内稳定ID必须一致。
            && self.asset_path == record.asset_path // GLB/GLTF文件来源必须与草稿完全一致。
            && self.asset_target == record.asset_target // 文件内Scene0或子资产目标同样必须一致。
            && self.building_preset_id == record.building_preset_id // 建筑分类和空间语义不能借用普通资产旧根证明。
            && self.terrain_patch == record.terrain_patch
            && self.mesh_lod == record.mesh_lod
            && self.translation == record.translation // 平移变化必须由对应预览或正式候选证明。
            && self.yaw_degrees == record.yaw_degrees // 旋转变化不能借用旧根Ready。
            && self.scale == record.scale // 缩放变化不能借用旧根Ready。
            && water_surfaces == record.water_surfaces // 当前覆盖组件补齐可热改的水数组证据。
    }

    /// 当前根是否属于已原子采用批次或编辑器自己的独立预览。
    pub fn is_active(&self) -> bool {
        self.gate.is_active() || self.previous_gate.as_ref().is_some_and(PlacementInstanceGate::is_active)
    }

    /// 当前根是否仍属于尚未采用、也尚未取消的正式隐藏候选。
    pub fn is_pending(&self) -> bool {
        !self.is_active() && self.gate.is_pending() // 仍在旧画面中的复用根不能被误判为不可见候选。
    }
} // 结束显式摆放根身份公开接口。

impl PartialEq for ScenePlacementInstance {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id // 公开值相等语义保持为稳定记录ID，不把运行时批次指针泄漏给作者域。
    }
}

impl Eq for ScenePlacementInstance {}

#[cfg(test)]
mod builtin_record_update_tests {
    use super::*;

    fn record() -> ScenePlacementRecord {
        let mut record: ScenePlacementRecord = serde_json::from_str(r#"{"id":"cube","asset_path":"","translation":[0,1,0],"yaw_degrees":0,"scale":[1,1,1]}"#).unwrap();
        record.asset_target = ScenePlacementAssetTarget::DEFAULT_CUBOID;
        record
    }

    #[test]
    fn mesh_lod_author_preview_keeps_batch_and_carried_gates_until_retirement() {
        let map = SceneId("MAP-lod-preview-gate".into());
        let mut expected = record();
        expected.mesh_lod = Some(feathrise_scene_content::MeshLodPolicy {
            display: feathrise_scene_content::MeshLodMode::Disabled, ..Default::default()
        });
        for active in [false, true] {
            let gate = PlacementInstanceGate::new(active);
            let original = ScenePlacementInstance::new_pending(&map, &record(), gate.clone());
            let updated = original.with_author_preview_mesh_lod_policy(expected.mesh_lod).unwrap();
            assert_eq!(updated.is_active(), active);
            assert_eq!(updated.is_pending(), !active);
            assert!(updated.matches_record(&map, &expected, &[]));
            assert_ne!(updated.authoring_version(), original.authoring_version());
            assert_eq!(updated.authoring_version(), updated.with_author_preview_mesh_lod_policy(expected.mesh_lod).unwrap().authoring_version());
            gate.activate(); assert!(updated.is_active());
            gate.retire(); assert!(!updated.is_active() && !updated.is_pending());
        }
        let old = PlacementInstanceGate::new(true);
        let candidate = PlacementInstanceGate::new(false);
        let carried = ScenePlacementInstance::new_pending(&map, &record(), old.clone()).carried_to(candidate.clone()).unwrap();
        let updated = carried.with_author_preview_mesh_lod_policy(expected.mesh_lod).unwrap();
        candidate.retire(); assert!(updated.is_active()); // 失败候选仍借旧批次，不能丢掉previous_gate。
        old.retire(); assert!(!updated.is_active() && !updated.is_pending());
    }

    #[test]
    fn builtin_record_update_keeps_the_formal_batch_gate_through_adoption_and_retirement() {
        let map = SceneId("MAP-cube".into());
        for active in [false, true] {
            let gate = PlacementInstanceGate::new(active);
            let original = ScenePlacementInstance::new_pending(&map, &record(), gate.clone());
            assert_eq!(original.authoring_version(), original.clone().authoring_version());
            assert_ne!(original.authoring_version(), ScenePlacementInstance::new(&map, &record()).authoring_version());
            let mut changed = record();
            changed.translation = [2.0, 3.0, 4.0];
            changed.asset_target = ScenePlacementAssetTarget::Cuboid {
                size_mm: [1000, 2000, 3000], color_srgb: [255, 64, 32],
                roughness_milli: 350, collision: false,
            };
            let refreshed = original.with_updated_record(&changed).unwrap();
            assert_ne!(original.authoring_version(), refreshed.authoring_version(), "同正式批次的后来作者参数也须撤销旧保存租约");
            assert_eq!(refreshed.authoring_version(), refreshed.clone().authoring_version());
            assert!(refreshed.matches_record(&map, &changed, &[]));
            assert_eq!(refreshed.is_active(), active);
            assert_eq!(refreshed.is_pending(), !active);
            gate.activate();
            assert!(refreshed.is_active());
            gate.retire();
            assert!(!refreshed.is_active());
            assert!(!refreshed.is_pending());
            assert!(!refreshed.with_updated_record(&changed).unwrap().is_active());
        }
    }

    #[test]
    fn builtin_record_update_rejects_other_records_and_external_asset_targets() {
        let source = record();
        let instance = ScenePlacementInstance::new(&SceneId("MAP-cube".into()), &source);
        let mut changed = source.clone();
        changed.id = "other-cube".into();
        assert!(instance.with_updated_record(&changed).is_none());
        changed = source.clone();
        changed.asset_path = "other.glb".into();
        assert!(instance.with_updated_record(&changed).is_none());
        changed = source;
        changed.asset_target = ScenePlacementAssetTarget::Scene0;
        assert!(instance.with_updated_record(&changed).is_none());
    }

    #[test]
    fn independent_preview_versions_cannot_borrow_same_id_ownership() {
        let map = SceneId("MAP-preview-versions".into());
        let first = ScenePlacementInstance::new(&map, &record());
        let next = ScenePlacementInstance::new(&map, &record());
        assert_eq!(first, next, "既有公开值相等语义仍只比较稳定记录ID");
        assert_ne!(first.authoring_version(), next.authoring_version(), "同记录同值的新作者快照不能继承旧回收权");
        assert_eq!(first.authoring_version(), first.clone().authoring_version());
    }

    #[test]
    fn carried_placement_keeps_old_activity_on_cancel_and_drops_it_after_adoption() {
        let map = SceneId("MAP-carry-gate".into());
        let old = PlacementInstanceGate::new(true);
        let source = ScenePlacementInstance::new_pending(&map, &record(), old.clone());
        let failed = PlacementInstanceGate::new(false);
        let carried = source.carried_to(failed.clone()).unwrap();
        failed.retire();
        assert!(carried.is_active() && !carried.is_pending());
        assert_eq!(source.authoring_version(), carried.authoring_version());
        let next = PlacementInstanceGate::new(false);
        let adopted = carried.carried_to(next.clone()).unwrap();
        next.activate();
        old.retire();
        assert!(adopted.is_active());
        assert!(!carried.is_active());
        next.retire();
        assert!(!adopted.is_active() && !adopted.is_pending());
        assert!(adopted.carried_to(PlacementInstanceGate::new(false)).is_none());
    }
}

#[cfg(test)] // 使用真实私有批次门验证遮雨消费者，不为测试扩大生产写接口。
mod rain_shelter_gate_tests {
    // 测试位于门所有权模块内。
    use super::*; // 直接使用正式pending/active/retired门。
    use feathrise_environment::rain_shelter::{RainShelterBox, RainShelterState}; // 正式CPU受雨数据。
    use feathrise_scene_content::ActiveDynamicScene; // 场景桥需要完整动态身份。
    use feathrise_scene_core::state::ScenePhase; // 当前正式场景。

    #[test] // 原子门变更没有Changed<Component>也必须即时反映。
    fn pending_active_retired_placement_controls_shelter_without_component_changes() {
        // 坏候选不能影响旧有效批次。
        let mut app = App::new(); // 不安装渲染或物理插件。
        app.insert_resource(State::new(ScenePhase::Active)); // 活动场景与记录一致。
        app.insert_resource(ActiveDynamicScene { scene_id: Some(SceneId("HN-01".into())) }); // 当前场景与两个placement批次身份一致。
        app.add_plugins(crate::shared::rain_shelter::RainShelterPlugin); // 正式完整作者桥。
        let record: ScenePlacementRecord = serde_json::from_str(r#"{"id":"roof","asset_path":"scenes/HN-01/roof.glb","translation":[0,0,0],"yaw_degrees":0,"scale":[1,1,1]}"#).unwrap(); // 正式serde数据而非复制构造所有私有细节。
        let old_gate = PlacementInstanceGate::new(true); // 旧有效屋顶继续拥有活动权。
        let new_gate = PlacementInstanceGate::new(false); // 热更候选尚未原子采用。
        let old_root = app
            .world_mut()
            .spawn(ScenePlacementInstance::new_pending(
                &SceneId("HN-01".into()),
                &record,
                old_gate.clone(),
            ))
            .id(); // 保留旧有效根。
        let new_root = app
            .world_mut()
            .spawn(ScenePlacementInstance::new_pending(
                &SceneId("HN-01".into()),
                &record,
                new_gate.clone(),
            ))
            .id(); // 新隐藏候选根。
        app.world_mut().spawn((
            RainShelterBox {
                center: Vec3::ZERO,
                half_extents: Vec3::ONE,
            },
            GlobalTransform::from(Transform::from_xyz(0.0, 4.0, 0.0)),
            ChildOf(old_root),
        )); // 旧有效遮雨几何。
        let candidate = app
            .world_mut()
            .spawn((
                RainShelterBox {
                    center: Vec3::ZERO,
                    half_extents: Vec3::NAN,
                },
                GlobalTransform::from(Transform::from_xyz(0.0, 4.0, 0.0)),
                ChildOf(new_root),
            ))
            .id(); // 坏候选不能让旧活动批次关闭。
        app.update(); // 检查同场景两根活动门。
        assert_eq!(
            app.world()
                .resource::<RainShelterState>()
                .exposure(Vec3::ZERO),
            0.0
        ); // pending坏源被整根跳过，旧源继续遮雨。
        app.world_mut()
            .entity_mut(candidate)
            .insert(RainShelterBox {
                center: Vec3::ZERO,
                half_extents: Vec3::ONE,
            }); // 修复候选但尚未采用。
        app.update(); // 候选仍不增加正式体数量。
        assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 1.0); // 只有旧完整版本。
        old_gate.retire(); // 原子撤销旧批次。
        new_gate.activate(); // 同帧原子采用新完整批次。
        app.update(); // 不依赖任何Changed<Instance>通知。
        assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 1.0); // 正式代理没有双批叠加。
        new_gate.retire(); // 取消后即使实体尚未删除也不能遮雨。
        app.update(); // 单根gate检查即时撤销。
        assert_eq!(app.world().resource::<RainShelterState>().gpu.header.x, 0.0); // retired实体不留下幽灵代理。
    } // 完成共享门生命周期回归。
} // 完成门内部集成测试。

/// 标记直接由一条primitive摆放记录生成的唯一可视子实体。
///
/// 【功能设计】直接primitive没有Scene0对象层级和`GltfMeshName`，中央视口只能
/// 通过本标记命中其可视网格，再从祖先`ScenePlacementInstance`恢复整条摆放记录；
/// 该路径不会伪造Blender节点、网格水绑定或Scene0展开需求。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ScenePlacementPrimitiveVisual;

/// 内置参数网格的唯一可视子节点；父根只通过精确地址替换或切换它的碰撞。
#[derive(Component)]
pub(super) struct BuiltinPlacementVisual;

/// 正式根和作者预览共用单子节点地址，附加作者子树再宽也不扫描Children。
/// 地址使用前仍须核验可视标记和真实ChildOf，失效或被重挂的节点不再属于原根。
#[derive(Component, Clone, Copy)]
pub(super) struct BuiltinPlacementVisualRoot(pub(super) Entity);

/// 当前完整JSON修订的隐藏候选根。
#[derive(Component)]
pub(super) struct PlacementStagingRoot;

/// 标记仍属于隐藏原子候选树的显式摆放根。
///
/// 【内部协调约定】场景运行时独占本组件的插入和移除；编辑器等外部系统
/// 不得修改候选内容或自行改变其生命周期。该物理标记在单根结算后仍保留，
/// 整批采用后按固定预算移除；外部作者系统必须改用`ScenePlacementInstance::is_active`，
/// 不能以本标记是否存在推断正式身份。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PlacementPendingInstance;

/// 标记当前根的`DynamicWaterMeshOverrides`含至少一项作者绑定。
///
/// 保存预检只查询本标记与覆盖组件的交集，因此8192个普通placement不会在等待帧被反复扫描；
/// 正式候选、编辑器预览及F12热改写侧必须与完整覆盖数组同批维护本标记。
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ScenePlacementWaterEvidence;

/// 标记根在整个隐藏候选生命期中所属的代次/修订。
///
/// 根暂时Ready后仍保留身份，因此GLB热更再次发出`WorldInstanceReady`时
/// 能撤销该根的旧Ready信用；只在整批原子采用或候选销毁时移除。
#[derive(Component)]
pub(super) struct PlacementPendingRevision {
    pub(super) generation: u32,
    pub(super) revision: u32,
}

/// 水体增量准备向所属候选根汇报一次真实游标/阶段推进。
///
/// 事件直接定向到根上注册的observer，不需要全局消息资源；旧代次、已采用或已退役根
/// 即使迟到触发，也会被observer按当前`PlacementPendingRevision`失败关闭。
#[derive(EntityEvent, Clone, Copy, Debug)]
pub(crate) struct ScenePlacementProgressPulse {
    pub entity: Entity,
}

/// 标记Bevy已完整实例化候选根，等待可选整网格水体接管结果。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)] // 瞬时门标记只在隐藏候选事务内存在。
pub(super) struct PlacementWorldInstanceReady(pub(super) u64); // 资产epoch让同InstanceId热重载也不能复用旧WorldReady。

/// 当前建筑根收到的 World 与 Avian 层级碰撞完成代次。
///
/// 【功能设计】Scene0 热更可能复用同一根与同一 World instance ID，因此不能让旧碰撞
/// Ready 标记给新层级授信。两个真实完成边沿各推进一次计数，只有计数相等时才属于同一代。
#[derive(Component, Clone, Copy, Debug, Default)]
pub(super) struct PlacementBuildingCollisionGeneration {
    pub(super) world_ready: u64, // 每次真实 WorldInstanceReady 推进，新 Scene 不能借旧碰撞。
    pub(super) collision_ready: u64, // 每次 ColliderConstructorHierarchyReady 推进。
}

/// Avian 已完成某一代建筑可见网格的层级碰撞构造。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlacementBuildingCollisionReady(pub(super) u64); // 数值必须追平当前 World 完成代次。
