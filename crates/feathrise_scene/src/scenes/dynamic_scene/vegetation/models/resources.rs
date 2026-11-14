//! 模型密度任务、唯一在途身份和公开采用回执；全部源快照只共享不可变作者数据。
use crate::gpu_grass_content::RuntimeBladeProfileAssets;
use bevy::{prelude::*, tasks::Task};
use feathrise_environment::vegetation::blade::{
    GpuBladeProfileBank, GpuBladeProfileBankId, GpuBladeProfileSlot,
};
use feathrise_scene_content::{
    ModelSurfaceVegetation, SceneId, SceneInstancePath, ScenePlacementAssetTarget,
    SceneVegetationAsset, SceneVegetationLayer, VegetationPresetDescriptor,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicModelVegetationStatus {
    pub scene_id: Option<SceneId>,
    pub input_revision: Option<u64>,
    pub applied_revision: Option<u64>,
    pub applied_document: Option<usize>,
    pub runtime_revision: u64,
    pub instance_path: Option<SceneInstancePath>,
    pub ready: bool,
    pub failed: bool,
    pub surface_count: usize,
    pub blade_window_count: usize,
    pub mesh_batch_count: usize,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    pub scene: SceneId,
    pub path: SceneInstancePath,
    pub instance_generation: u64,
    pub document: usize,
    pub draft: Option<u64>,
    pub document_epoch: u64,
    pub geometry_epoch: u64,
    pub placement_revision: u64,
    pub index_revision: u64,
    pub presets: u64,
}

pub(super) struct ObjectInput {
    pub entity: Entity,
    pub id: String,
    pub path: String,
    pub target: ScenePlacementAssetTarget,
    pub density: Arc<ModelSurfaceVegetation>,
}
pub(super) struct Capture {
    pub identity: Identity,
    pub definition: Arc<SceneVegetationAsset>,
    pub cursor: usize,
    pub objects: Vec<ObjectInput>,
    pub warnings: Vec<String>,
    pub presets: Vec<VegetationPresetDescriptor>,
}
pub(super) struct LayerBuild {
    pub definition: SceneVegetationLayer,
    pub cells: Arc<[[[f32; 4]; 3]]>,
    pub image: Option<Image>,
    pub finest_spacing: f32,
    pub min: Vec3,
    pub max: Vec3,
}
pub(super) struct ObjectBuild {
    pub entity: Entity,
    pub id: String,
    pub layers: VecDeque<LayerBuild>,
}
pub(super) struct Build {
    pub objects: VecDeque<ObjectBuild>,
    pub warnings: Vec<String>,
    /// 后台去重并冻结真实输出依赖；主线程不能重新回扫全部对象和作者层。
    pub profiles: VecDeque<ProfileDependency>,
    pub banks: Vec<GpuBladeProfileBank>,
}
pub(super) struct ProfileDependency {
    pub preset: VegetationPresetDescriptor,
    pub binding: Option<(GpuBladeProfileBankId, GpuBladeProfileSlot)>,
}
pub(super) enum ProfileCheck {
    Blade(String),
    Mesh(String),
}
pub(super) struct Staging {
    pub root: Entity,
    pub source: Entity,
    pub id: String,
    pub layers: VecDeque<LayerBuild>,
    /// 每层只安装实际引用的一批，不能给每个模型复制全场外观目录。
    pub banks: BTreeMap<u32, GpuBladeProfileBankId>,
}
pub(super) struct Waiting {
    pub identity: Identity,
    pub build: Build,
    pub blades: BTreeMap<String, RuntimeBladeProfileAssets>,
    pub banks: Vec<GpuBladeProfileBank>,
    pub bindings: BTreeMap<String, (GpuBladeProfileBankId, GpuBladeProfileSlot)>,
    pub staging: Option<Staging>,
    pub retained: BTreeMap<String, Entity>,
    pub blade_count: usize,
    pub mesh_count: usize,
    pub prepared: bool,
    pub checks: VecDeque<ProfileCheck>,
    pub mesh_checks: VecDeque<(Handle<Mesh>, String)>,
    pub missing_uv: VecDeque<(Handle<Mesh>, String)>,
    pub seen_meshes: BTreeSet<bevy::asset::AssetId<Mesh>>,
}

#[derive(Resource, Default)]
pub(in crate::scenes::dynamic_scene) struct ModelVegetationState {
    pub(super) source: Option<(
        Option<bevy::asset::AssetId<SceneVegetationAsset>>,
        Option<u64>,
        Arc<SceneVegetationAsset>,
    )>,
    pub(super) document_epoch: u64,
    pub(super) capture: Option<Capture>,
    pub(super) task: Option<Task<Result<Build, String>>>,
    pub(super) building: Option<Identity>,
    pub(super) waiting: Option<Waiting>,
    pub(super) active: Option<Identity>,
    pub(super) failed: Option<Identity>,
    pub(super) roots: BTreeMap<String, Entity>,
}
impl ModelVegetationState {
    pub(in crate::scenes::dynamic_scene) fn busy(&self) -> bool {
        self.task.is_some()
    }
    pub(in crate::scenes::dynamic_scene) fn finished(&self) -> bool {
        self.task.as_ref().is_none_or(|t| t.is_finished())
    }
}

#[derive(Component)]
pub(super) struct ModelWindow {
    pub source: Entity,
}

/// 场景根与同源子场景共同受两个真实工作席位约束；任务自然结束前不归还许可。
#[derive(Resource, Default)]
pub(super) struct ModelTaskBudget(Arc<AtomicUsize>);
pub(super) struct ModelTaskPermit(Arc<AtomicUsize>);
impl ModelTaskBudget {
    pub fn acquire(&self) -> Option<ModelTaskPermit> {
        self.0
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < 2).then_some(n + 1)
            })
            .ok()
            .map(|_| ModelTaskPermit(self.0.clone()))
    }
}
impl Drop for ModelTaskPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// 有界资产消息读取器只发布小型失效代次；实例域不重复回扫全局消息。
#[derive(Resource, Default)]
pub(super) struct ModelAssetEpoch {
    pub document: u64,
    pub geometry: u64,
}
