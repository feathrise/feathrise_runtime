//! 接收玩家或怪物的技能地面命中，只对当前稀疏场景发布会话级浅坑。

use super::clipmap::SparseHeightPageRuntime; // 必须先取得受检稀疏规格。
use super::resources::DynamicSceneRuntimeState; // 只在当前稀疏地形根准备后受理。
use crate::components::{DynamicSceneTerrainPrimitive, DynamicSparseTerrainCollider};
use avian3d::prelude::{SpatialQuery, SpatialQueryFilter}; // 真实碰撞射线避免把施法原点当作地面落点。
use bevy::asset::{AssetEvent, AssetId, LoadState};
use bevy::prelude::*; // 消息、资源和世界空间射线。
use feathrise_scene_core::residency::TerrainCollisionRegion; // 外扩碰撞块不能赋予范围外技能物理地面。
use feathrise_scene_content::{
    ActiveDynamicScene, SceneId, SceneSurfaceAsset, SceneSurfaceSourceAsset, SparseHeightCraterRequest,
    SparseHeightSessionCrater, SurfaceCraterResponse, SurfacePresetRegistry,
    SurfaceSourceDefinitionHasher,
};
use feathrise_scene_core::residency::{
    EntitySceneScope, InstanceFrame, InstanceRes, InstanceResMut, SceneSourceFrame,
}; // 每个子场景借用独立状态，父图保留原资源。
use std::sync::Arc; // 运行时覆盖与作者草稿独立。 // 建筑和角色不得冒充可破坏地面。

use super::surface::DynamicSurfaceAuthorPreview;

const MAX_PENDING_CRATERS: usize = 32; // 首载源尚未就绪时不丢施法，也不能无限积压。

fn belongs_to_current_terrain_root(
    entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
) -> bool {
    let mut cursor = entity;
    for _ in 0..256 {
        if cursor == root {
            return true;
        }
        let Ok(parent) = parents.get(cursor) else {
            return false;
        };
        cursor = parent.parent();
    }
    false // 退役根、同Owner残留几何及异常深层级不能取得当前实例写入资格。
}

fn soil_surface(spec: &feathrise_scene_content::SparseHeightSpec) -> Arc<SurfaceCraterResponse> {
    Arc::new(
        SurfaceCraterResponse::new(
            Vec2::from_array(spec.world_min), // 测试快照覆盖整张稀疏场景。
            Vec2::from_array(spec.world_max), // 单像素足以表达统一土壤响应。
            [1, 1],                           // 固定单像素，不需要磁盘作者图层。
            Arc::from([0_u8; 16]),            // 第一地表槽选择土壤系数。
            Arc::from([255_u8, 0, 0, 0]),     // 第一槽权重为满值。
            vec![1.0],                        // 土壤浅坑使用完整深度。
        )
        .unwrap(),
    )
}

#[cfg(test)]
pub(crate) fn soil_surface_for_test(
    spec: &feathrise_scene_content::SparseHeightSpec,
) -> Arc<SurfaceCraterResponse> {
    soil_surface(spec)
}

#[derive(Clone, PartialEq, Eq)]
struct CraterSourceIdentity {
    scene_id: SceneId,
    definition: Option<AssetId<SceneSurfaceAsset>>,
    preset_revision: u64,
    bounds: [u32; 4],
}

struct RoutedCrater {
    root_scene: SceneId,
    source_scene: SceneId,
    root: Entity,
    frame: SceneSourceFrame,
    position: Vec3,
    radius: f32,
    depth: f32,
}

/// 四槽作者源的强句柄与已验证CPU快照独立于GPU KTX2，稳定帧不复制画布。
#[derive(Resource, Default)]
pub(super) struct CraterSurfaceSource {
    identity: Option<CraterSourceIdentity>,
    indices: Option<Handle<SceneSurfaceSourceAsset>>,
    weights: Option<Handle<SceneSurfaceSourceAsset>>,
    snapshot: Option<Arc<SurfaceCraterResponse>>,
    pending: Vec<RoutedCrater>,
    last_error: Option<String>,
    saved_source_ticket: u64, // 一次保存只复核一次当前完整源，不能借用上次校验缓存。
    verified_saved_source_ticket: Option<u64>, // 当前CPU快照已匹配该保存内容；任何源失效立即撤销。
}

/// AssetServer 在IO线程有界读取两张源；定义、预设或raw热更都换代旧CPU快照。
pub(super) fn prepare_crater_surface(
    active: InstanceRes<ActiveDynamicScene>,
    registry: crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog,
    presets: Res<SurfacePresetRegistry>,
    definitions: Res<Assets<SceneSurfaceAsset>>,
    sources: Res<Assets<SceneSurfaceSourceAsset>>,
    server: Res<AssetServer>,
    pages: InstanceRes<SparseHeightPageRuntime>,
    mut definition_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneSurfaceAsset>,
    mut source_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneSurfaceSourceAsset>,
    mut state: InstanceResMut<CraterSurfaceSource>,
    mut author: crate::scenes::dynamic_scene::author_sources::TerrainAuthorResMut<
        DynamicSurfaceAuthorPreview,
    >,
    ownership: (
        InstanceFrame,
        Query<&super::instances::TerrainObjectInstance>,
    ),
) {
    let definition_changed = definition_events.read().fold(false, |changed, event| {
        changed
            || state.identity.as_ref().is_some_and(|identity| match event {
                AssetEvent::Added { id }
                | AssetEvent::Modified { id }
                | AssetEvent::Removed { id }
                | AssetEvent::LoadedWithDependencies { id }
                | AssetEvent::Unused { id } => Some(*id) == identity.definition,
            })
    });
    let Some((scene_id, handles)) = active
        .scene_id
        .as_ref()
        .and_then(|scene_id| registry.handles(scene_id).map(|handles| (scene_id, handles)))
    else {
        source_events.read().for_each(drop);
        *state = CraterSurfaceSource::default();
        return;
    };
    let terrain_id = ownership
        .0
        .instance()
        .and_then(|entity| ownership.1.get(entity).ok())
        .map(|object| &object.definition.id);
    let checking_saved_source = author.saving
        && author.scene_id.as_ref() == Some(scene_id)
        && author.matches_terrain(terrain_id)
        && (author.expected_source.is_some() || author.pending_sources != 0);
    if checking_saved_source {
        author.pending_sources = 3; // 当前快照在本帧重新经过下列完整源门后才恢复为零。
        if state.saved_source_ticket != author.save_ticket {
            state.saved_source_ticket = author.save_ticket;
            state.verified_saved_source_ticket = None;
            state.snapshot = None;
            state.last_error = None; // 新票据复核已经到达的当前内容，不清Reader，早到新事件也不丢。
        }
    }
    let Some(spec) = pages.spec_for(scene_id) else {
        source_events.read().for_each(drop);
        state.snapshot = None;
        return;
    };
    let bounds = [
        spec.world_min[0].to_bits(),
        spec.world_min[1].to_bits(),
        spec.world_max[0].to_bits(),
        spec.world_max[1].to_bits(),
    ];
    let Some(definition_handle) = handles.surface.as_ref() else {
        source_events.read().for_each(drop);
        let identity = CraterSourceIdentity {
            scene_id: scene_id.clone(),
            definition: None,
            preset_revision: 0,
            bounds,
        };
        if state.identity.as_ref() != Some(&identity) || state.snapshot.is_none() {
            if state
                .identity
                .as_ref()
                .is_some_and(|old| old.scene_id != *scene_id)
            {
                state.pending.clear();
            }
            state.identity = Some(identity);
            state.indices = None;
            state.weights = None;
            state.last_error = None;
            state.snapshot = Some(soil_surface(spec)); // 白模/裸地沿用默认1的土壤响应，不能因没涂PBR而禁用技能地形。
        }
        return;
    };
    let Some(definition) = definitions
        .get(definition_handle)
        .filter(|source| source.scene_id == *scene_id)
    else {
        source_events.read().for_each(drop);
        state.snapshot = None;
        return;
    };
    if presets.revision == 0 && !definition.layers.is_empty() {
        source_events.read().for_each(drop);
        state.snapshot = None;
        return;
    }
    let identity = CraterSourceIdentity {
        scene_id: scene_id.clone(),
        definition: Some(definition_handle.id()),
        preset_revision: presets.revision,
        bounds,
    };
    if state.identity.as_ref() != Some(&identity) || definition_changed {
        if state
            .identity
            .as_ref()
            .is_some_and(|previous| previous.scene_id != *scene_id)
        {
            state.pending.clear(); // 跨场景未处理技能不可落到新图。
        }
        let indices = server.load(definition.index_source_path.clone());
        let weights = server.load(definition.weight_source_path.clone());
        state.indices = Some(indices);
        state.weights = Some(weights);
        state.identity = Some(identity);
        state.snapshot = None;
        state.verified_saved_source_ticket = None;
        state.last_error = None;
    }
    let source_changed = source_events.read().fold(false, |changed, event| {
        let id = match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::Removed { id }
            | AssetEvent::LoadedWithDependencies { id }
            | AssetEvent::Unused { id } => *id,
        };
        let index = state
            .indices
            .as_ref()
            .is_some_and(|handle| handle.id() == id);
        let weight = state
            .weights
            .as_ref()
            .is_some_and(|handle| handle.id() == id);
        changed || index || weight
    }); // 原通道、驻留回放和旧journal只让缓存失效，不具备新保存的内容证据。
    if source_changed {
        state.snapshot = None; // raw源改写后新笔必须使用新画布，旧笔持有旧Arc。
        state.verified_saved_source_ticket = None;
        state.last_error = None; // 文件变动才允许重试此前损坏的源。
    }
    if state.snapshot.is_some() {
        if checking_saved_source && state.verified_saved_source_ticket == Some(author.save_ticket) {
            author.pending_sources = 0; // 稳定帧只比较小票据，不重新比较全部定义或扫描像素。
        }
        return;
    }
    if state.last_error.is_some() {
        return; // 损坏作者源没有新修订时不每帧重复构建和报警。
    }
    let (Some(index_handle), Some(weight_handle)) = (&state.indices, &state.weights) else {
        return;
    };
    let source_ids = [index_handle.id(), weight_handle.id()];
    let (Some(indices), Some(weights)) = (sources.get(index_handle), sources.get(weight_handle))
    else {
        for id in source_ids {
            if let LoadState::Failed(error) = server.load_state(id) {
                let detail = format!("地表浅坑作者源加载失败：{error}");
                if state.last_error.as_deref() != Some(&detail) {
                    warn!("{detail}");
                    state.last_error = Some(detail);
                }
            }
        }
        return;
    };
    let mut definition_hash = SurfaceSourceDefinitionHasher::new(definition);
    let coefficients = definition
        .layers
        .iter()
        .map(|layer| {
            definition_hash.push_layer(layer); // 复用原有冻结循环，正式采用只比摘要，不另扫层表或像素。
            presets
                .resolve(&layer.preset_id)
                .map(|preset| preset.crater_response)
                .ok_or_else(|| format!("地表层{}缺少预设{}", layer.id, layer.preset_id))
        })
        .collect::<Result<Vec<_>, _>>();
    let definition_hash = definition_hash.finish();
    let result = coefficients.and_then(|coefficients| {
        SurfaceCraterResponse::new(
            Vec2::from_array(spec.world_min),
            Vec2::from_array(spec.world_max),
            definition.resolution,
            Arc::clone(indices.bytes()),
            Arc::clone(weights.bytes()),
            coefficients,
        )
    });
    match result {
        Ok(snapshot) => {
            state.snapshot = Some(Arc::new(snapshot)); // 两张文件与当前场景/预设身份完整才发布。
            state.last_error = None;
            if checking_saved_source
                && author
                    .expected_source
                    .as_ref()
                    .is_some_and(|expected| expected.matches(definition_hash, indices, weights))
            {
                state.verified_saved_source_ticket = Some(author.save_ticket);
                author.pending_sources = 0; // 两份实际raw摘要及完整定义匹配本批保存，旧事件再多也不能冒充。
            }
        }
        Err(error) if state.last_error.as_deref() != Some(&error) => {
            warn!("地表浅坑源未采用：{error}");
            state.last_error = Some(error);
        }
        Err(_) => {}
    }
}

pub(super) fn receive_crater_requests(
    mut requests: MessageReader<SparseHeightCraterRequest>, // 消费当前帧玩家/怪物的单次地形请求。
    active_scene: Res<ActiveDynamicScene>, // 消息只携带玩法父图身份，不能与独立网格的源图混淆。
    runtime: InstanceRes<DynamicSceneRuntimeState>,
    pages: InstanceRes<SparseHeightPageRuntime>,
    spatial: SpatialQuery, // 读取真实已采用的世界地形碰撞。
    region: InstanceRes<TerrainCollisionRegion>,
    terrain: Query<
        (),
        Or<(
            With<DynamicSparseTerrainCollider>,
            With<DynamicSceneTerrainPrimitive>,
            With<super::systems::DynamicSceneTerrainRoot>, // 原始GLB的完整Collider在根，Primitive只在可见Mesh子节点。
        )>,
    >, // 旧GLB承托也挡住射线，不能穿过它破坏下层Clipmap。
    owners: EntitySceneScope,
    contexts: Query<(&super::instances::SubsceneInstance, &SceneSourceFrame)>,
    active_batches: Query<(), With<super::instances::SubsceneActive>>,
    parents: Query<&ChildOf>,
    mut surface: InstanceResMut<CraterSurfaceSource>,
) {
    for request in requests.read().take(MAX_PENDING_CRATERS) {
        if active_scene.scene_id.as_ref() != Some(&request.scene_id)
            || !request.origin.is_finite()
            || !request.radius.is_finite()
            || !(0.0..=8.0).contains(&request.radius)
            || request.radius == 0.0
            || !request.depth.is_finite()
            || !(0.0..=2.0).contains(&request.depth)
            || request.depth == 0.0
        {
            continue;
        }
        let filter = SpatialQueryFilter::from_mask(avian3d::prelude::LayerMask(request.ground_query_mask)); // 调用方提供地面过滤，场景只验证真实场景落点。
        let ray_start = request.origin + Vec3::Y * 3.0; // 允许角色身高和有限坡面起伏。
        let Some(hit) =
            spatial.cast_ray_predicate(ray_start, Dir3::NEG_Y, 12.0, true, &filter, &|entity| {
                if !terrain.contains(entity) {
                    return false;
                }
                let owner = owners.instance(entity);
                if owner.is_some_and(|owner| {
                    !contexts.get(owner).is_ok_and(|(context, _)| {
                        context.path.root_scene_id == request.scene_id
                            && active_batches.contains(context.batch)
                    })
                }) {
                    return false;
                }
                runtime
                    .for_instance(owner)
                    .and_then(|runtime| runtime.root)
                    .is_some_and(|root| belongs_to_current_terrain_root(entity, root, &parents))
            })
        else {
            continue;
        }; // 过滤角色/建筑，只取当前采用的共享地面。
        if hit.normal.y <= 0.0 {
            continue;
        } // 只处理真实朝上的地形面。
        let position = ray_start + Vec3::NEG_Y * hit.distance; // 以实际命中世界位置计算圆形范围。
        let owner = owners.instance(hit.entity);
        let (source_scene, frame) = if let Some(owner) = owner {
            let Ok((context, frame)) = contexts.get(owner) else {
                continue;
            };
            (context.source_scene_id.clone(), *frame)
        } else {
            (request.scene_id.clone(), SceneSourceFrame::default())
        };
        let Some(runtime) = runtime.for_instance(owner).filter(|runtime| {
            runtime.active_scene_id.as_ref() == Some(&source_scene)
                && runtime.sparse_height_path.is_some()
        }) else {
            continue;
        };
        let Some(root) = runtime.root else {
            continue;
        };
        if pages
            .for_instance(owner)
            .and_then(|pages| pages.spec_for(&source_scene))
            .is_none()
            || region
                .for_instance(owner)
                .is_none_or(|region| !region.supports(frame.to_local(request.origin)))
        {
            continue;
        }
        let Some(target) = surface.for_instance_mut(owner) else {
            continue;
        };
        if target.pending.len() < MAX_PENDING_CRATERS {
            target.pending.push(RoutedCrater {
                root_scene: request.scene_id.clone(),
                source_scene,
                root,
                frame,
                position: frame.to_local(position),
                radius: request.radius,
                depth: request.depth,
            });
            // 根容器只投递一次，真正命中的唯一对象在自己的调度中消费，不向所有同SceneId网格广播。
        } else {
            warn!("浅坑等待队列已满，拒绝后续技能形变");
        }
    }
    if !requests.is_empty() {
        requests.clear();
        warn!("本帧浅坑请求超过32次，拒绝超出预算的技能形变");
    }
}

/// 独立网格与旧单地形共用采用端，候选仍由现有高度页与碰撞单飞任务按同会话修订重建。
pub(super) fn apply_pending_craters(
    root_scene: Res<ActiveDynamicScene>,
    active: InstanceRes<ActiveDynamicScene>,
    runtime: InstanceRes<DynamicSceneRuntimeState>,
    pages: InstanceRes<SparseHeightPageRuntime>,
    frame: InstanceFrame,
    region: InstanceRes<TerrainCollisionRegion>,
    mut session: InstanceResMut<SparseHeightSessionCrater>,
    mut surface: InstanceResMut<CraterSurfaceSource>,
    author: crate::scenes::dynamic_scene::author_sources::TerrainAuthorRes<DynamicSurfaceAuthorPreview>,
    objects: Query<&super::instances::TerrainObjectInstance>,
) {
    let map = active.scene_id.as_ref();
    if session.scene_id() != map {
        session.clear(map.cloned());
    }
    surface.pending.retain(|pending| {
        Some(&pending.root_scene) == root_scene.scene_id.as_ref()
            && Some(&pending.source_scene) == map
            && Some(pending.root) == runtime.root
            && pending.frame == frame.frame()
    });
    let Some(spec) = map.and_then(|map| pages.spec_for(map)) else {
        return;
    };
    let terrain_id = frame
        .instance()
        .and_then(|entity| objects.get(entity).ok())
        .map(|object| &object.definition.id);
    let snapshot = if author.scene_id.as_ref() == map
        && author.matches_terrain(terrain_id)
        && author.material.is_some()
    {
        author.crater_response.clone() // 关闭F12或保存交接期间仍按实际可见作者像素及预设系数挖真坑。
    } else {
        surface.snapshot.clone()
    };
    let Some(snapshot) = snapshot else {
        return;
    }; // 作者CPU候选尚缺失时等待，不误用保存前文件或猜测土壤。
    let matrix = frame.frame().source_to_world;
    let scale = Vec3::new(
        matrix.x_axis.truncate().length(),
        matrix.y_axis.truncate().length(),
        matrix.z_axis.truncate().length(),
    );
    for pending in std::mem::take(&mut surface.pending) {
        if !region.supports(pending.position) {
            continue;
        }
        if let Err(error) = session.apply_world_metric(
            spec,
            pending.position.xz(),
            pending.radius,
            pending.depth,
            scale,
            snapshot.clone(),
        ) {
            warn!("地形技能浅坑未采用：{error}");
        }
    }
}

pub(super) fn clear_crater_surface(mut state: InstanceResMut<CraterSurfaceSource>) {
    *state = CraterSurfaceSource::default(); // 释放raw强句柄和跨场景等待消息。
}

pub(super) fn clear_session_craters(mut session: InstanceResMut<SparseHeightSessionCrater>) {
    session.clear(None); // 切换场景/重进清空全部技能破坏，旧后台结果依靠修订丢弃。
}

#[cfg(test)]
mod tests;
