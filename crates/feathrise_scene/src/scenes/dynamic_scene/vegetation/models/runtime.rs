//! 模型植被来源捕获、单飞后台编译和完整身份采用；几何与密度都不在输入帧重建。
use super::super::DynamicModelVegetationDraft;
use super::{index::ModelSourceIndex, resources::*};
use crate::{ScenePlacementInstance, ScenePlacementRuntimeStatus};
use bevy::{
    ecs::system::SystemParam,
    prelude::*,
    tasks::{AsyncComputeTaskPool, futures::check_ready},
};
use feathrise_scene_content::{
    ActiveDynamicScene, RuntimeAssetRoot, SceneContentRegistry, SceneId, SceneInstancePath,
    SceneVegetationAsset, SurfaceTarget, VegetationPresetRegistry,
};
use feathrise_scene_core::residency::{InstanceFrame, InstanceQuery, InstanceRes, InstanceResMut};
use std::sync::Arc;

#[derive(SystemParam)]
pub(super) struct Inputs<'w, 's> {
    active: InstanceRes<'w, ActiveDynamicScene>,
    registry: InstanceRes<'w, SceneContentRegistry>,
    definitions: Res<'w, Assets<SceneVegetationAsset>>,
    draft: Res<'w, DynamicModelVegetationDraft>,
    presets: Option<Res<'w, VegetationPresetRegistry>>,
    index: Res<'w, ModelSourceIndex>,
    epochs: Res<'w, ModelAssetEpoch>,
    placements: InstanceRes<'w, ScenePlacementRuntimeStatus>,
    roots: InstanceQuery<'w, 's, &'static ScenePlacementInstance>,
    frame: InstanceFrame<'w>,
    nodes: Query<'w, 's, &'static crate::SubsceneInstance>,
    asset_root: Res<'w, RuntimeAssetRoot>,
    budget: Res<'w, ModelTaskBudget>,
}

/// 单次最多分类256项，超额只提升失效代次，不把消息风暴转成无界主线程循环。
pub(super) fn observe(
    mut state: ResMut<ModelAssetEpoch>,
    mut documents: MessageReader<AssetEvent<SceneVegetationAsset>>,
    mut meshes: MessageReader<AssetEvent<Mesh>>,
) {
    fn invalidated<A: Asset>(reader: &mut MessageReader<AssetEvent<A>>) -> bool {
        let mut changed = false;
        for event in reader.read().take(256) {
            changed |= matches!(
                event,
                AssetEvent::Modified { .. } | AssetEvent::Removed { .. }
            );
        }
        if !reader.is_empty() {
            reader.clear();
            changed = true;
        }
        changed
    }
    if invalidated(&mut documents) {
        state.document = state.document.wrapping_add(1);
    }
    if invalidated(&mut meshes) {
        state.geometry = state.geometry.wrapping_add(1);
    }
}

impl Inputs<'_, '_> {
    fn scope(&self, scene: &SceneId) -> Option<(SceneInstancePath, u64)> {
        if let Some(entity) = self.frame.instance() {
            let node = self.nodes.get(entity).ok()?;
            // 地形上下文继续使用自己的二维/三维字段，模型只属于源场景容器。
            if node.path.terrain_id.is_some() {
                return None;
            }
            Some((node.path.clone(), node.generation))
        } else {
            Some((SceneInstancePath::root(scene.clone()), 0))
        }
    }
    fn source(
        &self,
        state: &mut ModelVegetationState,
        scene: &SceneId,
    ) -> Option<(Option<u64>, Arc<SceneVegetationAsset>)> {
        let draft = self
            .frame
            .instance()
            .is_none()
            .then(|| self.draft.frozen_for_scene(scene))
            .flatten();
        let handle = self
            .registry
            .handles(scene)
            .and_then(|handles| handles.vegetation.as_ref());
        let key = (
            handle.map(Handle::id),
            draft.as_ref().map(|(revision, _)| *revision),
        );
        if state
            .source
            .as_ref()
            .is_some_and(|(id, revision, _)| (*id, *revision) == key)
            && draft.as_ref().is_none_or(|(_, data)| {
                state
                    .source
                    .as_ref()
                    .is_some_and(|(_, _, old)| Arc::ptr_eq(old, data))
            })
            && state.document_epoch == self.epochs.document
        {
            let (_, revision, definition) = state.source.as_ref().unwrap();
            return Some((*revision, definition.clone()));
        }
        let definition = if let Some((_, definition)) = draft {
            definition
        } else {
            Arc::new(self.definitions.get(handle?)?.clone())
        };
        state.document_epoch = self.epochs.document;
        state.source = Some((key.0, key.1, definition.clone()));
        Some((key.1, definition))
    }
    fn object(&self, id: &str, scene: &SceneId) -> Option<(Entity, &ScenePlacementInstance)> {
        // 正式候选与旧作者预览短暂重叠时只取最新活动根；完整实例查询阻止同ID兄弟串位。
        self.index.by_id.get(id)?.iter().rev().find_map(|entity| {
            let root = self.roots.get(*entity).ok()?;
            (root.scene_id() == scene && root.is_active() && !root.is_terrain_patch())
                .then_some((*entity, root))
        })
    }
}

fn cancel_waiting(commands: &mut Commands, state: &mut ModelVegetationState) {
    if let Some(waiting) = state.waiting.take() {
        if let Some(staging) = &waiting.staging {
            commands.entity(staging.root).try_despawn();
        }
        for root in waiting.retained.values() {
            commands.entity(*root).try_despawn();
        }
        AsyncComputeTaskPool::get()
            .spawn(async move { drop(waiting) })
            .detach();
    }
    state.capture = None;
}
fn clear_roots(commands: &mut Commands, state: &mut ModelVegetationState) {
    for root in std::mem::take(&mut state.roots).into_values() {
        commands.entity(root).try_despawn();
    }
    state.active = None;
}
fn fail(
    state: &mut ModelVegetationState,
    status: &mut DynamicModelVegetationStatus,
    identity: Identity,
    error: String,
) {
    warn!("模型植被未采用：{error}");
    state.failed = Some(identity);
    status.failed = true;
    status.ready = false;
    status.detail = error;
}

/// 每帧只冻结固定前缀；目录热更通过外层完整Identity撤销半份捕获。
pub(super) fn capture_presets(capture: &mut Capture, presets: &VegetationPresetRegistry, budget: usize) -> Result<bool, String> {
    for _ in 0..budget {
        let Some(layer) = capture.definition.layers.get(capture.presets.len()) else { return Ok(true); };
        let preset = super::super::helpers::resolve_layer_preset(layer, presets)
            .ok_or_else(|| format!("模型植被层{}的预设不存在", layer.id))?;
        capture.presets.push(preset.clone());
    }
    Ok(capture.presets.len() == capture.definition.layers.len())
}

pub(super) fn drive(
    mut commands: Commands,
    mut state: InstanceResMut<ModelVegetationState>,
    mut status: InstanceResMut<DynamicModelVegetationStatus>,
    input: Inputs,
    mut assets: super::spawn::Assets,
) {
    let Some(scene) = input.active.scene_id.as_ref() else {
        return;
    };
    let Some((path, generation)) = input.scope(scene) else {
        return;
    };
    if status.scene_id.as_ref() != Some(scene) {
        cancel_waiting(&mut commands, &mut state);
        clear_roots(&mut commands, &mut state);
        state.source = None;
        state.failed = None;
        *status = DynamicModelVegetationStatus {
            scene_id: Some(scene.clone()),
            ..default()
        };
    }
    let Some((draft, definition)) = input.source(&mut state, scene) else {
        // 尚未读到已登记文档时保持上一完整根；真正未声明文档的空场景立即就绪。
        if input
            .registry
            .handles(scene)
            .is_some_and(|handles| handles.vegetation.is_none())
        {
            cancel_waiting(&mut commands, &mut state);
            clear_roots(&mut commands, &mut state);
            status.ready = true;
            status.failed = false;
            status.detail = "场景没有模型植被".into();
        }
        return;
    };
    // 旧二维文档并不授予模型渲染资格；空模型画布无需解析二维预设或等待它的资源。
    // 此门必须位于预设检查之前，否则普通子场景会被无关历史层永久阻塞。
    if definition.model_surfaces.is_empty() {
        if state.task.as_mut().and_then(check_ready).is_some() {
            state.task = None;
            state.building = None;
        }
        cancel_waiting(&mut commands, &mut state);
        clear_roots(&mut commands, &mut state);
        let document = Arc::as_ptr(&definition) as usize;
        if status.applied_document != Some(document) || status.applied_revision != draft {
            status.runtime_revision = status.runtime_revision.wrapping_add(1);
        }
        status.input_revision = draft;
        status.applied_revision = draft;
        status.applied_document = Some(document);
        status.instance_path = Some(path);
        status.ready = true;
        status.failed = false;
        status.surface_count = 0;
        status.blade_window_count = 0;
        status.mesh_batch_count = 0;
        status.detail = "场景没有模型植被".into();
        return;
    }
    let Some(presets) = input.presets.as_deref().filter(|p| p.revision > 0) else {
        status.detail = "等待开发者植被预设目录".into();
        return;
    };
    let identity = Identity {
        scene: scene.clone(),
        path,
        instance_generation: generation,
        document: Arc::as_ptr(&definition) as usize,
        draft,
        document_epoch: input.epochs.document,
        geometry_epoch: input.epochs.geometry,
        placement_revision: input.placements.revision,
        index_revision: input.index.revision,
        presets: presets.revision,
    };
    status.input_revision = draft;
    // 任务即使过期仍占唯一槽，直到自然完成；没有提前detach另开整图作业的取消路径。
    if let Some(task) = state.task.as_mut() {
        let Some(result) = check_ready(task) else {
            return;
        };
        state.task = None;
        let built = state.building.take();
        if built.as_ref() == Some(&identity) {
            match result {
                Ok(build) => state.waiting = Some(super::spawn::prepare(identity.clone(), build)),
                Err(error) => fail(&mut state, &mut status, identity.clone(), error),
            }
        }
    }
    if state.active.as_ref() == Some(&identity) || state.failed.as_ref() == Some(&identity) {
        return;
    }
    if state
        .waiting
        .as_ref()
        .is_some_and(|waiting| waiting.identity != identity)
    {
        cancel_waiting(&mut commands, &mut state);
    }
    if let Some(mut waiting) = state.waiting.take() {
        match super::spawn::advance(&mut commands, &mut assets, &mut waiting, presets) {
            Ok(true) => {
                // 全部对象的层/LOD/原型已齐，只有此处可撤销旧集合、一次显示新集合。
                clear_roots(&mut commands, &mut state);
                for root in waiting.retained.values() {
                    commands.entity(*root).insert(Visibility::Inherited);
                }
                state.roots = std::mem::take(&mut waiting.retained);
                status.applied_document = Some(identity.document);
                status.instance_path = Some(identity.path.clone());
                state.active = Some(identity);
                state.failed = None;
                status.runtime_revision = status.runtime_revision.wrapping_add(1);
                status.applied_revision = draft;
                status.ready = true;
                status.failed = false;
                status.surface_count = state.roots.len();
                status.blade_window_count = waiting.blade_count;
                status.mesh_batch_count = waiting.mesh_count;
                status.detail = if waiting.build.warnings.is_empty() {
                    "模型植被已完整采用".into()
                } else {
                    waiting.build.warnings.join("；")
                };
            }
            Ok(false) => state.waiting = Some(waiting),
            Err(error) => {
                state.waiting = Some(waiting);
                cancel_waiting(&mut commands, &mut state);
                fail(&mut state, &mut status, identity, error);
            }
        }
        return;
    }
    if state.task.is_some() {
        return;
    }
    if state
        .capture
        .as_ref()
        .is_none_or(|capture| capture.identity != identity)
    {
        state.capture = Some(Capture {
            identity: identity.clone(),
            definition: definition.clone(),
            cursor: 0,
            objects: Vec::new(),
            warnings: Vec::new(),
            presets: Vec::new(),
        });
        status.ready = false;
        status.failed = false;
        status.detail = "捕获模型种植面".into();
    }
    let capture = state.capture.as_mut().unwrap();
    match capture_presets(capture, presets, 32) {
        Ok(true) => {},
        Ok(false) => return,
        Err(error) => {
            fail(&mut state, &mut status, identity, error);
            return;
        }
    }
    for _ in 0..32 {
        let Some(density) = definition.model_surfaces.get(capture.cursor) else {
            break;
        };
        capture.cursor += 1;
        let SurfaceTarget::Model { object_id } = &density.target else {
            continue;
        };
        if let Some((entity, root)) = input.object(object_id, scene) {
            capture.objects.push(ObjectInput {
                entity,
                id: object_id.clone(),
                path: root.asset_path().into(),
                target: root.asset_target(),
                density: density.clone(),
            });
        } else {
            capture
                .warnings
                .push(format!("{object_id}：模型尚未就绪或已删除，作者数据已保留"));
        }
    }
    if capture.cursor < definition.model_surfaces.len() {
        return;
    }
    let Some(permit) = input.budget.acquire() else {
        return;
    };
    let capture = state.capture.take().unwrap();
    let root = input.asset_root.asset_root().to_owned();
    state.building = Some(capture.identity);
    state.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let _permit = permit;
        super::build::build_async(
            &root,
            capture.definition,
            capture.objects,
            capture.presets,
            capture.warnings,
        )
        .await
    }));
    status.detail = "后台编译模型表面植被".into();
}

pub(super) fn reset(
    mut state: InstanceResMut<ModelVegetationState>,
    mut status: InstanceResMut<DynamicModelVegetationStatus>,
) {
    let task = state.task.take();
    let building = state.building.take();
    let old = std::mem::replace(
        &mut *state,
        ModelVegetationState {
            task,
            building,
            ..default()
        },
    );
    AsyncComputeTaskPool::get()
        .spawn(async move { drop(old) })
        .detach();
    *status = default();
}
