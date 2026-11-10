//! 动态场景区域边界首次加载、热更新与F12未保存碰撞采用系统。

use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子场景借用独立状态，父图保留原资源。
use avian3d::prelude::{CollisionLayers, RigidBody}; // 区域空气墙使用静态Cuboid和角色过滤层。
use bevy::asset::{AssetLoadFailedEvent, LoadState}; // JSON首次加载、热更和失败事件。
use bevy::prelude::*; // ECS系统参数、父子实体、日志与场景生命周期。
use feathrise_scene_content::{ActiveDynamicScene, SceneBoundariesAsset, SceneManifestAsset}; // 运行时从目录取得正式边界和场景世界范围。
use feathrise_scene_core::state::ScenePhase; // 边界根随统一动态场景退出回收。

use super::components::{DynamicSceneBoundaryRoot, DynamicSceneBoundarySegment}; // 原子根与逐段诊断标记。
use super::derivation::{Chunk, ChunkKey}; // 后台已构造完整空间块碰撞。
use super::resources::{
    DynamicSceneBoundaryOverride, DynamicSceneBoundaryPhase, DynamicSceneBoundaryState,
    DynamicSceneBoundaryStatus,
}; // 文件状态、公开诊断和F12未保存覆盖。
use crate::scenes::dynamic_scene::resources::{
    DynamicSceneRuntimePhase, DynamicSceneRuntimeState, DynamicSceneRuntimeStatus,
};
use std::{collections::BTreeMap, sync::Arc}; // 缓存保持共享身份。 // 首载边界加入场景Ready门。

/// 进入动态场景时重置轻量状态；旧实体由DespawnOnExit统一回收。
pub(in crate::scenes::dynamic_scene) fn begin_dynamic_boundaries(
    active_scene: InstanceRes<ActiveDynamicScene>, // 当前目标场景由切换场景事务提前写入。
    mut state: InstanceResMut<DynamicSceneBoundaryState>, // 重置唯一写侧事务。
    mut status: InstanceResMut<DynamicSceneBoundaryStatus>, // 发布明确加载阶段。
) {
    state.generation = state.generation.wrapping_add(1);
    state.dirty = true; // OnEnter下一Update必须读取目录和可选覆盖。
    state.desired_scene_id = active_scene.scene_id.clone(); // 冻结当前稳定场景身份。
    state.installed_scene_id = None; // 新场景没有可复用的上一场景根。
    state.root = None;
    state.derivation.clear_installed(); // DespawnOnExit负责旧实体，状态只清理所有权。
    state.installed_from_override = false; // 新场景尚未选择文件或覆盖源。
    status.phase = DynamicSceneBoundaryPhase::Loading; // 首帧等待目录资产。
    status.scene_id = active_scene.scene_id.clone(); // F12和诊断显示真正SceneId。
    status.override_revision = None;
    status.validation_error = None;
    status.segment_count = 0; // 尚未安装任何空气墙。
    status.detail = "正在读取动态场景区域边界".to_owned(); // 不把边界等待冒充地形碰撞等待。
}

/// 只观察当前目标边界资产、目录修订和F12覆盖身份。
pub(in crate::scenes::dynamic_scene) fn observe_dynamic_boundary_assets(
    active_scene: InstanceRes<ActiveDynamicScene>, // 切换场景时更新真正目标身份。
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>, // 目录可能仍在异步加载。
    boundary_override: InstanceRes<DynamicSceneBoundaryOverride>, // 未保存作者草稿优先于文件。
    mut events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneBoundariesAsset>, // 首载和热更事件合并为一次脏标记。
    mut manifest_events: feathrise_scene_core::residency::ResidentAssetEventReader<SceneManifestAsset>, // 世界范围热更也使捕获票据失效。
    mut failures: MessageReader<AssetLoadFailedEvent<SceneBoundariesAsset>>, // 当前边界JSON失败提供明确诊断。
    mut state: InstanceResMut<DynamicSceneBoundaryState>, // 只在身份或事件变化时修改。
    mut status: InstanceResMut<DynamicSceneBoundaryStatus>, // 热更失败保留旧根但发布失败详情。
) {
    if state.desired_scene_id != active_scene.scene_id {
        state.desired_scene_id = active_scene.scene_id.clone(); // 同一GameScene内切换场景也必须换代。
        state.generation = state.generation.wrapping_add(1);
        state.dirty = true; // 下一系统原子采用新场景完整边界。
    }
    if let Some(registry) = registry.as_deref()
        && registry.revision != state.catalog_revision
    {
        state.catalog_revision = registry.revision; // 保存最新目录代次。
        state.generation = state.generation.wrapping_add(1);
        state.dirty = true; // 新增或移除boundary_path都必须重新判断。
    }
    if state.override_scene_id != boundary_override.scene_id
        || state.override_revision != boundary_override.revision
    {
        state.override_scene_id = boundary_override.scene_id.clone(); // 保存F12覆盖场景身份。
        state.override_revision = boundary_override.revision; // 保存作者专用修订。
        state.generation = state.generation.wrapping_add(1);
        state.dirty = true; // 拖动松手、撤销和保存归还文件都只合并为一次重建。
    }
    let handle_id = state
        .desired_scene_id
        .as_ref()
        .and_then(|scene_id| registry.as_deref()?.handles(scene_id)?.boundaries.as_ref())
        .map(Handle::id); // 只取得当前场景边界句柄，不扫描其他场景资产。
    let manifest_id = state
        .desired_scene_id
        .as_ref()
        .and_then(|scene_id| registry.as_deref()?.handles(scene_id))
        .map(|handles| handles.manifest.id());
    let mut changed = false;
    for event in events.read() {
        changed |= handle_id.is_some_and(|id| {
            event.is_added(id)
                || event.is_modified(id)
                || event.is_loaded_with_dependencies(id)
                || event.is_removed(id)
        });
    }
    for event in manifest_events.read() {
        changed |= manifest_id.is_some_and(|id| {
            event.is_added(id)
                || event.is_modified(id)
                || event.is_loaded_with_dependencies(id)
                || event.is_removed(id)
        });
    }
    if changed {
        state.generation = state.generation.wrapping_add(1);
        state.dirty = true; // 同帧多个资产事件只产生一个完整碰撞替换。
    }
    for failure in failures.read() {
        if Some(failure.id) != handle_id {
            continue; // 其他场景的后台失败不能污染当前场景诊断。
        }
        state.generation = state.generation.wrapping_add(1);
        state.dirty = true; // 交给下游采用系统区分首载失败与“保留上一完整墙”的热更失败。
        status.phase = DynamicSceneBoundaryPhase::Failed; // 公开边界专用失败阶段。
        status.detail = format!("区域边界加载失败：{} ({})", failure.path, failure.error); // 指出精确文件和Bevy错误。
        error!("{}", status.detail); // 每次失败事件只记录一次日志。
    }
}

/// 全部后台块完成后，一次Commands提交变更块；未变块保留实体和共享形状。
fn adopt_completed(
    commands: &mut Commands,
    state: &mut DynamicSceneBoundaryState,
    status: &mut DynamicSceneBoundaryStatus,
    runtime_state: &mut DynamicSceneRuntimeState,
    runtime_status: &mut DynamicSceneRuntimeStatus,
    completed: super::derivation::Completed,
    own_rigid_bodies: bool,
    collision_layers: CollisionLayers, // 装配方提供过滤，异步几何不携带游戏角色策略。
) {
    status.override_revision = (!completed.formal).then_some(state.override_revision);
    status.validation_error = completed.validation_error; // 预览可接受的开放草稿不自动获得关闭资格。
    let chunks = match completed.result {
        Ok(chunks) => chunks,
        Err(error) => {
            runtime_state.boundary_ready =
                state.root.is_some() && state.installed_scene_id.as_ref() == Some(&completed.scene_id);
            status.phase = DynamicSceneBoundaryPhase::Failed;
            status.detail = format!("空气墙构造失败，保留上一完整碰撞：{error}");
            if !runtime_state.boundary_ready {
                runtime_status.phase = DynamicSceneRuntimePhase::Failed;
                runtime_status.detail = status.detail.clone();
            }
            state.dirty = false; // 错误输入不逐帧重试，等下一次来源修订。
            return;
        }
    };
    let root = *state.root.get_or_insert_with(|| {
        commands
            .spawn((
                Name::new(format!("DynamicSceneBoundaries_{}", completed.scene_id)), // 场景级所有权根。
                DynamicSceneBoundaryRoot, // 根管理生命周期；子场景碰撞统一由实例批次承接。
                Transform::IDENTITY,    // 作者世界坐标无需额外偏移。
                DespawnOnExit(ScenePhase::Active), // 离图回收全部块。
            ))
            .id()
    });
    replace_chunks(
        commands,
        root,
        &mut state.derivation.entities,
        &chunks,
        &completed.changed,
        &completed.removed,
        own_rigid_bodies,
        collision_layers,
    );
    state.derivation.document = completed.document;
    state.derivation.installed = chunks; // 全部变更排入同一命令缓冲后发布唯一活动缓存。
    state.installed_scene_id = Some(completed.scene_id.clone());
    state.installed_from_override = !completed.formal;
    state.dirty = false;
    runtime_state.boundary_ready = true; // 首载完整结果前不会放行场景Ready。
    status.phase = DynamicSceneBoundaryPhase::Ready;
    status.segment_count = completed.segments;
    status.revision = status.revision.wrapping_add(1);
    status.detail = format!(
        "空气墙已应用：{}段，{}个空间碰撞块",
        completed.segments,
        state.derivation.installed.len()
    );
}

/// 发布成本仅随变更块增长；compound的重计算及精确差异比较已在后台完成。
fn replace_chunks(
    commands: &mut Commands,
    root: Entity,
    entities: &mut BTreeMap<ChunkKey, Entity>,
    chunks: &BTreeMap<ChunkKey, Arc<Chunk>>,
    changed: &[ChunkKey],
    removed: &[ChunkKey],
    own_rigid_bodies: bool,
    collision_layers: CollisionLayers, // 装配方提供过滤，异步几何不携带游戏角色策略。
) {
    for key in removed {
        if let Some(entity) = entities.remove(key) {
            commands.entity(entity).try_despawn();
        }
    }
    for key in changed {
        let chunk = &chunks[key];
        if let Some(entity) = entities.get(key) {
            commands.entity(*entity).insert(chunk.collider.clone()); // 修改块替换共享形状，不重建未变实体。
        } else {
            let mut entity = commands
                .spawn((
                    Name::new(format!("AirWallChunk_{}_{}", key.0, key.1)), // 可按空间定位诊断。
                    DynamicSceneBoundarySegment,                              // 沿用物理调试标记。
                    chunk.collider.clone(),      // Arc形状已由后台完整构建。
                    collision_layers, // 沿用应用显式声明的边界过滤。
                    Transform::IDENTITY,         // compound内采用世界坐标。
                    ChildOf(root),               // 跟随场景生命周期统一回收。
                ));
            if own_rigid_bodies {
                entity.insert(RigidBody::Static); // 根场景继续按空间块建立独立宽相包围盒。
            } // 子场景直接继承批次刚体，避免先添加再移除引发Avian迟到的自绑定警告。
            entities.insert(*key, entity.id());
        }
    }
}

/// 从F12覆盖或AssetServer文件原子安装当前动态场景空气墙。
#[allow(clippy::too_many_arguments)] // 显式资产、清单、覆盖、运行时门和诊断保持职责可见。
pub(in crate::scenes::dynamic_scene) fn prepare_dynamic_boundaries(
    frame: feathrise_scene_core::residency::InstanceFrame,
    policy: Option<Res<crate::ScenePhysicsPolicy>>, // 无应用适配时使用中性默认过滤。
    mut commands: Commands,                             // 完整规格验证后才写实体。
    asset_server: Res<AssetServer>,                     // 检查正式边界句柄的异步加载状态。
    active_scene: InstanceRes<ActiveDynamicScene>,                  // 当前动态场景稳定身份。
    registry: Option<crate::scenes::dynamic_scene::sources::SceneRuntimeCatalog>,          // 目录提供清单和边界强句柄。
    manifests: Res<Assets<SceneManifestAsset>>,           // 正式文件点位必须位于当前清单范围内。
    boundaries: Res<Assets<SceneBoundariesAsset>>,        // 已完成反序列化的边界JSON资产。
    boundary_override: InstanceRes<DynamicSceneBoundaryOverride>, // F12未保存草稿优先采用。
    mut state: InstanceResMut<DynamicSceneBoundaryState>,         // 边界唯一写侧状态。
    mut status: InstanceResMut<DynamicSceneBoundaryStatus>,       // 发布边界独立阶段。
    mut runtime_state: InstanceResMut<DynamicSceneRuntimeState>,  // 首次加载边界加入场景Ready门。
    mut runtime_status: InstanceResMut<DynamicSceneRuntimeStatus>, // 首载失败时显示明确阻断项。
) {
    let state_generation = state.generation;
    if let Some(completed) = state.derivation.poll() {
        if completed.is_current(active_scene.scene_id.as_ref(), state.generation) {
            adopt_completed(
                &mut commands,
                &mut state,
                &mut status,
                &mut runtime_state,
                &mut runtime_status,
                completed,
                frame.instance().is_none(),
                policy.as_deref().copied().unwrap_or_default().boundary_layers,
            );
        }
    }
    if !state.dirty {
        return; // 稳定帧不遍历路径、不分配规格、不修改物理实体。
    }
    let Some(scene_id) = active_scene.scene_id.as_ref() else {
        runtime_state.boundary_required = false; // 没有场景身份时不存在可满足的边界要求。
        runtime_state.boundary_ready = false; // 场景整体仍由清单缺失路径阻断。
        status.phase = DynamicSceneBoundaryPhase::Failed; // 发布精确失败原因。
        status.detail = "尚未选择动态场景，无法加载区域边界".to_owned(); // 与地形清单错误分离。
        state.dirty = false; // 等待ActiveDynamicScene变化再重试。
        return;
    };
    status.scene_id = Some(scene_id.clone()); // 所有后续等待和失败绑定当前场景。
    let Some(registry) = registry.as_deref() else {
        status.phase = DynamicSceneBoundaryPhase::Loading; // 目录异步加载尚未完成。
        status.detail = "场景目录仍在加载，区域边界等待中".to_owned(); // 不把等待记为失败。
        return; // 保持dirty，下一帧只做常数次资源查询。
    };
    let Some(handles) = registry.handles(scene_id) else {
        runtime_state.boundary_required = false; // 损坏目录由场景主运行时负责失败。
        runtime_state.boundary_ready = false; // 不能放行边界门。
        status.phase = DynamicSceneBoundaryPhase::Failed; // 边界侧同步提供诊断。
        status.detail = format!("场景目录不存在{scene_id}的区域边界入口"); // 指出目标身份。
        state.dirty = false; // 等待目录修订事件。
        return;
    };
    let Some(manifest) = manifests.get(&handles.manifest) else {
        status.phase = DynamicSceneBoundaryPhase::Loading; // 清单仍在异步加载。
        status.detail = format!("场景{scene_id}清单仍在加载，区域边界等待中"); // 点位校验需要世界范围。
        return; // 保持dirty等待同一强句柄。
    };
    let Some(world_bounds) = manifest.world_bounds() else {
        runtime_state.boundary_required = handles.boundaries.is_some(); // 保留目录声明事实。
        runtime_state.boundary_ready = false; // 无世界范围无法安全采用碰撞。
        status.phase = DynamicSceneBoundaryPhase::Failed; // 正式失败等待清单热更。
        status.detail = format!("场景{scene_id}世界范围无效，区域边界无法校验"); // 指出清单字段问题。
        state.dirty = false; // 后续清单资产事件会重新标脏主运行时与边界。
        return;
    };
    let override_document = (boundary_override.scene_id.as_ref() == Some(scene_id))
        .then(|| boundary_override.document.as_ref())
        .flatten(); // 只有同一场景的未保存草稿可以取得优先权。
    if let Some(document) = override_document {
        runtime_state.boundary_required = true; // 未保存草稿也属于当前场景安全边界要求。
        runtime_state.boundary_ready =
            state.root.is_some() && state.installed_scene_id.as_ref() == Some(scene_id); // 新场景必须等待第一套完整墙，热更继续保旧。
        state.derivation.request_shared(
            Arc::clone(document),
            scene_id,
            state_generation,
            world_bounds,
        );
        status.phase = DynamicSceneBoundaryPhase::Loading;
        status.detail = "正在后台更新空气墙；保留上一完整碰撞".to_owned();
        return;
    }
    let Some(handle) = handles.boundaries.as_ref() else {
        if state.installed_from_override
            && state.root.is_some()
            && state.installed_scene_id.as_ref() == Some(scene_id)
        {
            runtime_state.boundary_required = true; // 保存刚完成但目录热重载尚未登记新文件时，安全墙仍是必需项。
            runtime_state.boundary_ready = true; // 继续保留刚才已经完整采用的未保存碰撞，绝不制造无墙窗口。
            status.phase = DynamicSceneBoundaryPhase::Loading; // 等待catalog把新boundary_path原子发布给AssetServer。
            status.detail =
                format!("场景{scene_id}区域边界已保存，正在等待目录登记；继续保留当前静态碰撞");
            state.dirty = false; // 后续目录修订会重新标脏并切回正式文件权威。
            return;
        }
        runtime_state.boundary_required = false; // 旧场景没有声明边界文件，保持向后兼容。
        runtime_state.boundary_ready = true; // 可选边界不阻断旧场景原启动门。
        if let Some(old_root) = state.root.take() {
            commands.entity(old_root).try_despawn(); // F12保存归还后目录确实无文件时移除临时墙。
        }
        state.derivation.clear_installed(); // 已删除根不可与下次重建缓存复用。
        state.installed_scene_id = Some(scene_id.clone()); // 记录当前禁用结果所属场景。
        state.installed_from_override = false; // 当前权威回到目录的“未声明”状态。
        state.dirty = false; // 稳定帧不再重复判断。
        status.phase = DynamicSceneBoundaryPhase::Disabled; // 公开区分“无文件”和“加载失败”。
        status.override_revision = None;
        status.validation_error = None;
        status.segment_count = 0; // 当前没有正式空气墙。
        status.detail = format!("场景{scene_id}未声明数据驱动区域边界"); // 旧专用场景可继续使用自己的边界方案。
        return;
    };
    runtime_state.boundary_required = true; // 目录声明边界后必须首载成功才能放行新场景。
    match asset_server.load_state(handle.id()) {
        LoadState::Failed(error) => {
            let retaining_previous =
                state.root.is_some() && state.installed_scene_id.as_ref() == Some(scene_id); // 热更失败可以保留上一完整根。
            runtime_state.boundary_ready = retaining_previous; // 首载失败阻断，热更失败继续安全运行。
            status.phase = DynamicSceneBoundaryPhase::Failed; // 发布正式失败阶段。
            status.detail = format!("场景{scene_id}区域边界加载失败：{error}"); // 精确保留AssetServer错误。
            if !retaining_previous {
                runtime_status.phase = DynamicSceneRuntimePhase::Failed; // 首次加载失败不得放行角色。
                runtime_status.detail = status.detail.clone(); // F12底栏直接显示具体阻断项。
            }
            state.dirty = false; // 等待后续资产修复事件重试。
            return;
        }
        LoadState::Loaded => {} // 继续读取完整资产并执行正式校验。
        _ => {
            runtime_state.boundary_ready =
                state.root.is_some() && state.installed_scene_id.as_ref() == Some(scene_id); // 热更等待保留上一完整墙。
            status.phase = DynamicSceneBoundaryPhase::Loading; // 首载或热更正在异步进行。
            status.detail = format!("场景{scene_id}区域边界仍在加载"); // 等待不是失败。
            return; // 保持dirty，加载完成前只做轻量句柄查询。
        }
    }
    let Some(document) = boundaries.get(handle) else {
        runtime_state.boundary_ready =
            state.root.is_some() && state.installed_scene_id.as_ref() == Some(scene_id); // 资产表短暂不可见时保留旧根。
        status.phase = DynamicSceneBoundaryPhase::Loading; // 等待Assets登记完成。
        status.detail = format!("场景{scene_id}区域边界资产正在登记"); // 不暴露半套碰撞。
        return; // 保持dirty继续非阻塞轮询。
    };
    runtime_state.boundary_ready =
        state.root.is_some() && state.installed_scene_id.as_ref() == Some(scene_id); // 文件后台校验期间保留当前场景有效结果。
    state
        .derivation
        .request(document, scene_id, state_generation, world_bounds, true);
    status.phase = DynamicSceneBoundaryPhase::Loading;
    status.detail = "正在后台准备空气墙空间碰撞块".to_owned();
}

/// 离开动态场景时清理轻量状态；实体由DespawnOnExit递归回收。
pub(in crate::scenes::dynamic_scene) fn clear_dynamic_boundaries(
    mut state: InstanceResMut<DynamicSceneBoundaryState>, // 清理当前场景所有权。
    mut status: InstanceResMut<DynamicSceneBoundaryStatus>, // 发布离图诊断。
) {
    state.generation = state.generation.wrapping_add(1);
    state.dirty = true; // 下一次进入必须重新读取目标目录。
    state.desired_scene_id = None; // 迟到资产事件不再属于当前场景。
    state.installed_scene_id = None; // 旧根不能跨场景复用。
    state.root = None;
    state.derivation.clear_installed(); // DespawnOnExit已经拥有实体回收职责。
    state.installed_from_override = false; // 清除旧作者源身份。
    status.phase = DynamicSceneBoundaryPhase::Disabled; // 离图后没有活动边界。
    status.scene_id = None; // 清除旧场景显示身份。
    status.override_revision = None;
    status.validation_error = None;
    status.segment_count = 0; // 清除旧碰撞计数。
    status.detail = "已离开动态场景，区域边界已回收".to_owned(); // 明确生命周期完成。
}

#[cfg(test)]
mod tests {
    use super::*; // 验证正式过滤器，不启动窗口或Avian调度。

    #[test]
    fn complete_result_publishes_together_and_bad_reload_keeps_previous_root() {
        use super::super::derivation::Completed;
        use bevy::ecs::world::CommandQueue;
        use feathrise_scene_content::SceneId;
        let mut world = World::new();
        let mut queue = CommandQueue::default();
        let mut state = DynamicSceneBoundaryState::default();
        let mut status = DynamicSceneBoundaryStatus::default();
        let mut runtime = DynamicSceneRuntimeState::default();
        let mut runtime_status = DynamicSceneRuntimeStatus::default();
        let scene_id = SceneId::parse("MAP-wall-adoption").unwrap();
        let chunks: BTreeMap<_, _> = [
            (
                (0, 0),
                Arc::new(Chunk {
                    specs: vec![], // 这里只验证ECS发布边界，不重复纯几何测试。
                    collider: avian3d::prelude::Collider::cuboid(20.0, 20.0, 2.0),
                }),
            ),
            (
                (1, 0),
                Arc::new(Chunk {
                    specs: vec![],
                    collider: avian3d::prelude::Collider::cuboid(20.0, 20.0, 2.0),
                }),
            ),
        ]
        .into_iter()
        .collect();
        let completed = Completed {
            generation: 0,
            scene_id: scene_id.clone(),
            result: Ok(Arc::new(chunks)),
            segments: 2,
            formal: true,
            validation_error: None,
            document: None,
            changed: vec![(0, 0), (1, 0)],
            removed: vec![],
        };
        adopt_completed(
            &mut Commands::new(&mut queue, &world),
            &mut state,
            &mut status,
            &mut runtime,
            &mut runtime_status,
            completed,
            true,
            CollisionLayers::new(1_u32 << 28, 1_u32 << 29),
        );
        assert_eq!(
            world
                .query::<&DynamicSceneBoundarySegment>()
                .iter(&world)
                .count(),
            0
        ); // 发布前无半成品物理。
        queue.apply(&mut world);
        assert_eq!(
            world
                .query::<&DynamicSceneBoundarySegment>()
                .iter(&world)
                .count(),
            2
        ); // 同次提交出现完整块集合。
        assert!(world.query::<&CollisionLayers>().iter(&world).all(|layers|
            *layers == CollisionLayers::new(1_u32 << 28, 1_u32 << 29))); // 验证采用端真正保留应用过滤，不能回退隐藏游戏层。
        assert!(runtime.boundary_ready);
        let old_root = state.root;
        let old_entities = state.derivation.entities.clone();
        let failure = Completed {
            generation: 1,
            scene_id,
            result: Err("坏热更新".to_owned()),
            segments: 0,
            formal: true,
            validation_error: None,
            document: None,
            changed: vec![],
            removed: vec![],
        };
        adopt_completed(
            &mut Commands::new(&mut queue, &world),
            &mut state,
            &mut status,
            &mut runtime,
            &mut runtime_status,
            failure,
            true,
            CollisionLayers::new(1_u32 << 28, 1_u32 << 29),
        );
        queue.apply(&mut world);
        assert_eq!(state.root, old_root);
        assert_eq!(state.derivation.entities, old_entities);
        assert!(runtime.boundary_ready); // 热更失败仍保留保护，不能制造无墙窗口。
        assert_eq!(status.phase, DynamicSceneBoundaryPhase::Failed);
        let empty = Completed {
            generation: 2,
            scene_id: state.installed_scene_id.clone().unwrap(),
            result: Ok(Arc::default()), // 删除全部墙或全部停用均派生同一空完整结果。
            segments: 0,
            formal: false,
            validation_error: None,
            document: None,
            changed: vec![],
            removed: vec![(0, 0), (1, 0)],
        };
        adopt_completed(
            &mut Commands::new(&mut queue, &world),
            &mut state,
            &mut status,
            &mut runtime,
            &mut runtime_status,
            empty,
            true,
            CollisionLayers::new(1_u32 << 28, 1_u32 << 29),
        );
        assert_eq!(
            world
                .query::<&DynamicSceneBoundarySegment>()
                .iter(&world)
                .count(),
            2
        ); // 提交前旧保护仍完整。
        queue.apply(&mut world);
        assert_eq!(
            world
                .query::<&DynamicSceneBoundarySegment>()
                .iter(&world)
                .count(),
            0
        );
        assert!(state.derivation.entities.is_empty());
        assert!(state.derivation.installed.is_empty());
        assert!(runtime.boundary_ready); // 合法空集合不是加载失败。
        assert_eq!(status.segment_count, 0);
    }

    #[test]
    fn failed_initial_derivation_keeps_startup_gate_closed() {
        use bevy::ecs::world::CommandQueue;
        let world = World::new();
        let mut queue = CommandQueue::default();
        let mut state = DynamicSceneBoundaryState::default();
        let mut status = DynamicSceneBoundaryStatus::default();
        let mut runtime = DynamicSceneRuntimeState::default();
        let mut runtime_status = DynamicSceneRuntimeStatus::default();
        let failure = super::super::derivation::Completed {
            generation: 0,
            scene_id: feathrise_scene_content::SceneId::parse("MAP-wall-initial").unwrap(),
            result: Err("坏首载".to_owned()),
            segments: 0,
            formal: true,
            validation_error: None,
            document: None,
            changed: vec![],
            removed: vec![],
        };
        adopt_completed(
            &mut Commands::new(&mut queue, &world),
            &mut state,
            &mut status,
            &mut runtime,
            &mut runtime_status,
            failure,
            true,
            CollisionLayers::new(1_u32 << 28, 1_u32 << 29),
        );
        assert!(!runtime.boundary_ready);
        assert_eq!(runtime_status.phase, DynamicSceneRuntimePhase::Failed);
        assert!(state.root.is_none());
    }

}
