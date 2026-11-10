//! 整体地形/子场景姿态直接进入原生产覆盖源；列表只在单飞后台改一项，原实例图负责真实根采用。

use bevy::{
    asset::AssetId,
    ecs::system::SystemState,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_scene_content::{
    ActiveDynamicScene, SceneContentRegistry, SceneId, SceneInstanceAuthorPreview, SceneInstanceId,
    SceneInstancePlacement, SceneInstanceTransform, SceneManifestAsset, SceneTerrainObject, TerrainId,
    TerrainObjectTransform,
};
use feathrise_scene::{
    TerrainObjectInstance, TerrainObjectQueries, TerrainObjectTransformPreview,
};
use std::sync::Arc;

#[cfg(test)]
#[path = "object_pose_tests.rs"]
mod tests;

pub(super) enum Target {
    Subscene {
        id: SceneInstanceId,
        pose: Option<SceneInstanceTransform>,
    },
    Terrain {
        id: TerrainId,
        pose: Option<TerrainObjectTransform>,
    },
}
pub(super) struct Prepared {
    pub scene_id: SceneId,
    pub revision: u64,
    pub active: bool,
    pub target: Target,
}

/// 两个不可变数组足以验证实际清单来源，不复制目录或整个场景正文。
#[derive(Clone)]
struct Sources {
    handle: AssetId<SceneManifestAsset>,
    instances: Arc<[SceneInstancePlacement]>,
    terrains: Option<Arc<[SceneTerrainObject]>>,
}
fn sources(world: &World, map: &SceneId) -> Result<Sources, String> {
    if world
        .get_resource::<ActiveDynamicScene>()
        .and_then(|active| active.scene_id.as_ref())
        != Some(map)
    {
        return Err("整体姿态预览已不属于活动场景".into());
    }
    let handles = world
        .get_resource::<SceneContentRegistry>()
        .and_then(|registry| registry.handles(map))
        .ok_or("整体姿态预览缺少正式场景来源")?;
    let manifest = world
        .get_resource::<Assets<SceneManifestAsset>>()
        .and_then(|assets| assets.get(&handles.manifest))
        .ok_or("整体姿态预览的正式清单尚未采用")?;
    Ok(Sources {
        handle: handles.manifest.id(),
        instances: manifest.subscenes.clone(),
        terrains: manifest.terrains.clone(),
    })
}
fn same_terrains(a: &Option<Arc<[SceneTerrainObject]>>, b: &Option<Arc<[SceneTerrainObject]>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}
impl Sources {
    fn same(&self, other: &Self) -> bool {
        self.handle == other.handle
            && Arc::ptr_eq(&self.instances, &other.instances)
            && same_terrains(&self.terrains, &other.terrains)
    }
}
struct Published {
    scene_id: SceneId,
    source: Sources,
    revision: u64,
    instances: Arc<[SceneInstancePlacement]>,
    terrains: Option<Arc<[SceneTerrainObject]>>,
}
impl Published {
    fn owns(&self, preview: &SceneInstanceAuthorPreview) -> bool {
        preview.scene_id.as_ref() == Some(&self.scene_id)
            && preview.revision == self.revision
            && Arc::ptr_eq(&preview.instances, &self.instances)
            && same_terrains(&preview.terrains, &self.terrains)
    }
}
struct Completion {
    value: Prepared,
    source: Sources,
    instances: Arc<[SceneInstancePlacement]>,
    terrains: Option<Arc<[SceneTerrainObject]>>,
    publish: bool,
}
fn prepare_lists(
    value: Prepared,
    source: Sources,
    mut instances: Arc<[SceneInstancePlacement]>,
    mut terrains: Option<Arc<[SceneTerrainObject]>>,
) -> Result<Completion, String> {
    let publish = match &value.target {
        Target::Subscene {
            id,
            pose: Some(pose),
        } => {
            pose.validate()?;
            let rows = Arc::make_mut(&mut instances);
            let row = rows
                .iter_mut()
                .find(|row| &row.id == id)
                .ok_or("子场景姿态目标尚未进入当前正式来源")?;
            row.transform = *pose;
            true
        }
        Target::Terrain {
            id,
            pose: Some(pose),
        } => {
            pose.validate()?;
            let rows = terrains
                .as_ref()
                .ok_or("独立地形姿态目标尚未进入当前正式来源")?;
            if !rows.iter().any(|row| &row.id == id && row.enabled) {
                return Err("地形姿态目标不存在或已禁用".into());
            }
            // 按住时只发原单目标Transform覆盖；结束后才让原实例图接回权威姿态。
            // 这样下一目标可继续拖动，不必等待完整场景快照，也不每次鼠移复制地形定义。
            if !value.active {
                let rows = Arc::make_mut(terrains.as_mut().unwrap());
                rows.iter_mut().find(|row| &row.id == id).unwrap().transform = *pose;
            }
            !value.active
        }
        Target::Subscene { pose: None, .. } | Target::Terrain { pose: None, .. } => {
            if value.active {
                return Err("活动整体姿态不能缺少原作者姿态".into());
            }
            false
        }
    };
    Ok(Completion {
        value,
        source,
        instances,
        terrains,
        publish,
    })
}

struct TerrainCredit {
    scene_id: SceneId,
    id: TerrainId,
    pose: TerrainObjectTransform,
    source: Sources,
    /// 发布时记住实际旧定义；清单观察晚于实例采用时也能识别接回，不能误记新定义。
    definition: Arc<SceneTerrainObject>,
}
impl TerrainCredit {
    fn owns(&self, preview: &TerrainObjectTransformPreview) -> bool {
        preview.pose.as_ref() == Some(&(self.scene_id.clone(), self.id.clone(), self.pose))
    }
}
#[derive(Default)]
pub(super) struct State {
    task: Option<Task<Result<Completion, String>>>,
    ready: Option<Completion>,
    published: Option<Published>,
    terrain: Option<TerrainCredit>,
    retired_terrain: Option<TerrainCredit>,
    garbage: Option<Task<()>>,
    queries: Option<SystemState<TerrainObjectQueries<'static, 'static>>>,
    #[cfg(feature = "validation")]
    last: Option<(SceneId, u64, bool, String)>,
    #[cfg(feature = "validation")]
    actual: Option<Entity>,
}
impl State {
    pub(super) fn busy(&self) -> bool {
        self.task.is_some()
            || self.ready.is_some()
            || self.garbage.is_some()
            || self.retired_terrain.is_some()
    }
    pub(super) fn start(&mut self, world: &World, value: Prepared) -> Result<(), String> {
        if self.busy() || value.revision == 0 {
            return Err("整体姿态超过单飞准备预算或缺少有效修订".into());
        }
        let source = sources(world, &value.scene_id)?;
        let existing = world
            .get_resource::<SceneInstanceAuthorPreview>()
            .ok_or("原实例作者覆盖资源未安装")?;
        let owned = self.published.as_ref().filter(|credit| {
            credit.scene_id == value.scene_id && credit.source.same(&source) && credit.owns(existing)
        });
        if existing.scene_id.is_some() && owned.is_none() {
            return Err("实例作者覆盖已由另一来源接管".into());
        }
        let (instances, terrains) = owned.map_or_else(
            || (source.instances.clone(), source.terrains.clone()),
            |credit| (credit.instances.clone(), credit.terrains.clone()),
        );
        self.task = Some(
            AsyncComputeTaskPool::get()
                .spawn(async move { prepare_lists(value, source, instances, terrains) }),
        );
        Ok(())
    }
    fn terrain_source(
        &mut self,
        world: &mut World,
        map: &SceneId,
        id: &TerrainId,
    ) -> Result<Option<(Entity, Arc<SceneTerrainObject>)>, String> {
        let query = self.queries.get_or_insert_with(|| SystemState::new(world));
        let query = query
            .get(world)
            .map_err(|error| format!("整体地形活动根查询失败：{error}"))?;
        Ok(query.instance(map, id).and_then(|entity| {
            world
                .get::<TerrainObjectInstance>(entity)
                .map(|object| (entity, object.definition.clone()))
        }))
    }
    fn target_is_active(&mut self, world: &mut World) -> Result<bool, String> {
        let Some(value) = self.ready.as_ref() else { return Ok(true) };
        let query = self.queries.get_or_insert_with(|| SystemState::new(world));
        let query = query.get(world).map_err(|error| format!("整体姿态活动根查询失败：{error}"))?;
        Ok(match &value.value.target {
            Target::Terrain { id, pose: Some(_) } => query.instance(&value.value.scene_id, id).is_some(),
            Target::Subscene { id, pose: Some(_) } => query.subscene_instance(&value.value.scene_id, id).is_some(),
            _ => true,
        })
    }
    fn clear_terrain(&mut self, world: &mut World) {
        if let Some(credit) = self.terrain.take() {
            if let Some(mut preview) = world.get_resource_mut::<TerrainObjectTransformPreview>()
                && credit.owns(&preview)
            {
                preview.pose = None;
            }
            self.retired_terrain = Some(credit); // 切换场景后可能持有旧清单最后一份Arc，下一帧复用唯一回收槽。
        }
    }
    fn clear_published(&mut self, world: &mut World) {
        let Some(credit) = self.published.take() else {
            return;
        };
        let old = if let Some(mut preview) = world.get_resource_mut::<SceneInstanceAuthorPreview>()
            && credit.owns(&preview)
        {
            let revision = preview.revision.wrapping_add(1);
            Some(std::mem::replace(
                &mut *preview,
                SceneInstanceAuthorPreview {
                    revision,
                    ..default()
                },
            ))
        } else {
            None
        };
        self.garbage = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop((credit, old));
        }));
    }
    pub(super) fn advance(&mut self, world: &mut World) -> Result<bool, String> {
        if let Some(task) = self.garbage.as_mut() {
            if check_ready(task).is_none() {
                return Ok(false);
            }
            self.garbage = None;
        }
        if let Some(retired) = self.retired_terrain.take() {
            self.garbage = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(retired);
            }));
            return Ok(false);
        }
        self.reconcile(world)?;
        #[cfg(feature = "validation")]
        self.refresh_evidence(world)?;
        if self.garbage.is_some() || self.retired_terrain.is_some() {
            return Ok(false);
        }
        if let Some(task) = self.task.as_mut() {
            let Some(result) = check_ready(task) else {
                return Ok(false);
            };
            self.task = None;
            self.ready = Some(result?);
        }
        let Some(value) = self.ready.as_ref() else {
            return Ok(true);
        };
        if !sources(world, &value.value.scene_id).is_ok_and(|now| value.source.same(&now)) {
            let retired = self.ready.take();
            self.garbage = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(retired);
            }));
            return Err("整体姿态后台结果的来源已换代，拒绝迟到采用".into());
        }
        // 来源含有该ID仍不代表根已激活；保留单个结果并只查原稳定地址，不等未来快照。
        // 非法ID已经由后台正式清单检查报错，缺根只等待本次已发布来源的原消费者。
        if !self.target_is_active(world)? { return Ok(false); }
        let value = self.ready.as_ref().unwrap();
        // 旧目标的End已发布给实例图，等该消费者接回再交出唯一地形覆盖槽。
        // 等的是已发送的原列表来源，不等待会被本次正文挡住的完整快照。
        if let Target::Terrain { id, .. } = &value.value.target {
            let previous = self
                .terrain
                .as_ref()
                .filter(|old| old.scene_id != value.value.scene_id || &old.id != id)
                .map(|old| (old.scene_id.clone(), old.id.clone(), old.pose));
            if let Some((map, id, pose)) = previous {
                if self
                    .terrain_source(world, &map, &id)?
                    .is_some_and(|(_, object)| object.transform != pose)
                {
                    return Ok(false);
                }
                self.clear_terrain(world);
            }
        }
        let value = self.ready.take().unwrap();
        #[cfg(feature = "validation")]
        {
            self.last = Some((
                value.value.scene_id.clone(),
                value.value.revision,
                value.value.active,
                match &value.value.target {
                    Target::Subscene { id, .. } => format!("subscene:{}", id.as_str()),
                    Target::Terrain { id, .. } => format!("terrain:{}", id.as_str()),
                },
            ));
        }
        match &value.value.target {
            Target::Terrain {
                id,
                pose: Some(pose),
            } => {
                let (_, definition) = self.terrain_source(world, &value.value.scene_id, id)?
                    .ok_or("整体地形活动根在同一采用帧内失去身份")?;
                world.resource_mut::<TerrainObjectTransformPreview>().pose =
                    Some((value.value.scene_id.clone(), id.clone(), *pose));
                if let Some(old) = self.terrain.replace(TerrainCredit {
                    scene_id: value.value.scene_id.clone(),
                    id: id.clone(),
                    pose: *pose,
                    source: value.source.clone(),
                    definition,
                }) {
                    self.retired_terrain = Some(old);
                }
            }
            Target::Terrain { pose: None, .. } => {
                self.clear_terrain(world);
                self.clear_published(world);
            }
            Target::Subscene { pose: None, .. } => self.clear_published(world),
            _ => {}
        }
        if value.publish {
            let mut preview = world.resource_mut::<SceneInstanceAuthorPreview>();
            if preview.scene_id.is_some()
                && self
                    .published
                    .as_ref()
                    .is_none_or(|credit| !credit.owns(&preview))
            {
                return Err("整体姿态采用时原实例覆盖所有权已变化".into());
            }
            let revision = preview.revision.wrapping_add(1).max(1);
            let old = std::mem::replace(
                &mut *preview,
                SceneInstanceAuthorPreview {
                    scene_id: Some(value.value.scene_id.clone()),
                    revision,
                    instances: value.instances.clone(),
                    terrains: value.terrains.clone(),
                },
            );
            let previous = self.published.replace(Published {
                scene_id: value.value.scene_id.clone(),
                source: value.source.clone(),
                revision,
                instances: value.instances.clone(),
                terrains: value.terrains.clone(),
            });
            self.garbage = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop((value, old, previous));
            }));
            return Ok(false);
        }
        // 数组的最后一份引用也必须在后台释放；每域复用这个唯一回收席位。
        if self.garbage.is_none() {
            self.garbage = Some(AsyncComputeTaskPool::get().spawn(async move {
                drop(value);
            }));
        }
        Ok(false)
    }
    fn reconcile(&mut self, world: &mut World) -> Result<(), String> {
        let changed = self.published.as_ref().is_some_and(|credit| {
            !sources(world, &credit.scene_id).is_ok_and(|now| credit.source.same(&now))
        });
        if changed { self.clear_published(world); }
        if let Some((map, id, pose, definition, source_changed)) =
            self.terrain.as_ref().map(|credit| {
                (
                    credit.scene_id.clone(),
                    credit.id.clone(),
                    credit.pose,
                    credit.definition.clone(),
                    !sources(world, &credit.scene_id).is_ok_and(|now| credit.source.same(&now)),
                )
            })
        {
            let active = world
                .get_resource::<ActiveDynamicScene>()
                .and_then(|active| active.scene_id.as_ref())
                == Some(&map);
            let object = self.terrain_source(world, &map, &id)?;
            if !active
                || object.is_none()
                || object
                    .as_ref()
                    .is_some_and(|(_, object)| object.transform == pose)
                || source_changed && object.as_ref().is_some_and(|(_, object)| !Arc::ptr_eq(&definition, object))
            {
                self.clear_terrain(world);
            }
        }
        Ok(())
    }
    pub(super) fn snapshot_started(&self) -> Result<(), String> {
        if self.task.is_some() || self.ready.is_some() {
            return Err("完整快照不能越过尚未采用的整体姿态".into());
        }
        Ok(()) // 已显示覆盖继续由真正的清单/实例采用接回，收到消息本身不撤回它。
    }
    #[cfg(feature = "validation")]
    fn refresh_evidence(&mut self, world: &mut World) -> Result<(), String> {
        let Some((map, _, _, target)) = &self.last else {
            return Ok(());
        };
        let query = self.queries.get_or_insert_with(|| SystemState::new(world));
        let query = query
            .get(world)
            .map_err(|error| format!("整体姿态实际根取证失败：{error}"))?;
        self.actual = if let Some(id) = target.strip_prefix("terrain:") {
            TerrainId::parse(id)
                .ok()
                .and_then(|id| query.instance(map, &id))
        } else if let Some(id) = target.strip_prefix("subscene:") {
            SceneInstanceId::parse(id)
                .ok()
                .and_then(|id| query.subscene_instance(map, &id))
        } else {
            None
        };
        Ok(())
    }
    #[cfg(feature = "validation")]
    pub(super) fn evidence(&self, world: &World) -> serde_json::Value {
        // 采用时沿原稳定地址表记住根；截图只读这一个实体，不为验收扫描完整游戏World。
        let actual=self.actual.and_then(|entity|{
            let instance=world.get::<feathrise_scene::SubsceneInstance>(entity)?;
            if self.last.as_ref().is_none_or(|(map,_,_,_)|map!=&instance.path.root_scene_id)
                || world.get::<feathrise_scene::SubsceneActive>(instance.batch).is_none(){return None;}
            Some(serde_json::json!({"entity":entity.to_bits(),"path":instance.path.to_string(),
                "local":world.get::<Transform>(entity).map(|pose|pose.to_matrix().to_cols_array()),
                "world":world.get::<GlobalTransform>(entity).map(|pose|pose.to_matrix().to_cols_array())}))
        });
        serde_json::json!({"source":"production-SceneInstanceAuthorPreview-and-TerrainObjectTransformPreview","last":self.last,"actual":actual})
    }
}
