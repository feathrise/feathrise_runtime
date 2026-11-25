//! 由组件安装和移除维护运行根。只检查当前轨道指向的候选，不在播放时遍历整张地图。
use super::FrameIssue;
use bevy::ecs::{entity_disabling::Disabled, query::Allow};
use bevy::prelude::*;
use feathrise_scene::{
    ScenePlacementInstance, ScenePlacementRuntimePhase, ScenePlacementRuntimeStatus,
    SubsceneActive, SubsceneInstance, SubsceneRuntimeStatus,
};
use feathrise_scene_content::{
    ActiveDynamicScene, SceneId, SceneInstanceGraph, SceneInstanceGraphPhase,
};
use feathrise_scene_core::{
    cinematic::CinematicActor,
    residency::{InstanceResourceTable, SceneInstanceOwner, SceneSourceFrame},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Resource, Default)]
pub(crate) struct PlacementIndex(pub(crate) BTreeMap<(SceneId, String), BTreeSet<Entity>>);

fn forget<K: Ord>(index: &mut BTreeMap<K, BTreeSet<Entity>>, key: &K, entity: Entity) {
    if let Some(entities) = index.get_mut(key) {
        entities.remove(&entity);
        if entities.is_empty() {
            index.remove(key);
        }
    }
}

pub(crate) fn install_placements(app: &mut App) {
    if app.world().contains_resource::<PlacementIndex>() {
        return;
    }
    app.init_resource::<PlacementIndex>()
        .add_observer(
            |e: On<Insert<ScenePlacementInstance>>,
             q: Query<&ScenePlacementInstance, Allow<Disabled>>,
             mut index: ResMut<PlacementIndex>| {
                if let Ok(p) = q.get(e.entity) {
                    index
                        .0
                        .entry((p.scene_id().clone(), p.id().into()))
                        .or_default()
                        .insert(e.entity);
                }
            },
        )
        .add_observer(
            |e: On<Discard<ScenePlacementInstance>>,
             q: Query<&ScenePlacementInstance, Allow<Disabled>>,
             mut index: ResMut<PlacementIndex>| {
                if let Ok(p) = q.get(e.entity) {
                    forget(
                        &mut index.0,
                        &(p.scene_id().clone(), p.id().into()),
                        e.entity,
                    );
                }
            },
        );
}

#[derive(Resource, Default)]
pub(super) struct Cast {
    actors: BTreeMap<(Option<String>, String), BTreeSet<Entity>>,
    instances: BTreeMap<String, BTreeSet<Entity>>,
}

pub(super) fn install(app: &mut App) {
    install_placements(app);
    app.init_resource::<Cast>()
        .add_observer(
            |e: On<Insert<CinematicActor>>,
             q: Query<&CinematicActor, Allow<Disabled>>,
             mut cast: ResMut<Cast>| {
                if let Ok(a) = q.get(e.entity) {
                    cast.actors
                        .entry((a.instance.clone(), a.target.clone()))
                        .or_default()
                        .insert(e.entity);
                }
            },
        )
        .add_observer(
            |e: On<Discard<CinematicActor>>,
             q: Query<&CinematicActor, Allow<Disabled>>,
             mut cast: ResMut<Cast>| {
                if let Ok(a) = q.get(e.entity) {
                    forget(
                        &mut cast.actors,
                        &(a.instance.clone(), a.target.clone()),
                        e.entity,
                    );
                }
            },
        )
        .add_observer(
            |e: On<Insert<SubsceneInstance>>,
             q: Query<&SubsceneInstance, Allow<Disabled>>,
             mut cast: ResMut<Cast>| {
                if let Ok(s) = q.get(e.entity) {
                    cast.instances
                        .entry(s.path.to_string())
                        .or_default()
                        .insert(e.entity);
                }
            },
        )
        .add_observer(
            |e: On<Discard<SubsceneInstance>>,
             q: Query<&SubsceneInstance, Allow<Disabled>>,
             mut cast: ResMut<Cast>| {
                if let Ok(s) = q.get(e.entity) {
                    forget(&mut cast.instances, &s.path.to_string(), e.entity);
                }
            },
        )
        // 候选根先获得地址、后被采纳。当前源坐标或活动代际变化时再投上一次结算的姿态。
        .add_observer(|_: On<Insert<SubsceneActive>>, mut cast: ResMut<Cast>| cast.set_changed())
        .add_observer(|_: On<Discard<SubsceneActive>>, mut cast: ResMut<Cast>| cast.set_changed())
        .add_observer(|_: On<Insert<SceneSourceFrame>>, mut cast: ResMut<Cast>| cast.set_changed());
}

pub(super) struct Scope {
    scene: SceneId,
    owner: Option<Entity>,
    instance: Option<String>,
    pub frame: SceneSourceFrame,
}

pub(super) fn scope(world: &World, address: Option<&str>) -> Result<Scope, FrameIssue> {
    let root = world
        .get_resource::<ActiveDynamicScene>()
        .and_then(|s| s.scene_id.as_ref())
        .ok_or(FrameIssue::Waiting)?;
    let Some(path) = address.filter(|path| *path != root.as_str()) else {
        return Ok(Scope {
            scene: root.clone(),
            owner: None,
            instance: None,
            frame: Default::default(),
        });
    };
    if path.split('/').next() != Some(root.as_str()) {
        return Err(format!("演出实例 {path} 不属于当前场景 {root}").into());
    }
    let Some(entity) = world
        .resource::<Cast>()
        .instances
        .get(path)
        .and_then(|nodes| {
            nodes.iter().copied().find(|e| {
                world.get::<SubsceneInstance>(*e).is_some_and(|s| {
                    &s.path.root_scene_id == root
                        && world.get::<SubsceneActive>(s.batch).is_some()
                        && world.get::<Disabled>(s.batch).is_none()
                })
            })
        })
    else {
        // 子图按整批采用。地址还未出现时，只在当前来源图与准备批次都已发布后判定缺失。
        let graph = world.get_resource::<SceneInstanceGraph>();
        if let Some(graph) = graph {
            if graph.root_scene_id.as_ref() != Some(root)
                || matches!(
                    graph.phase,
                    SceneInstanceGraphPhase::Waiting | SceneInstanceGraphPhase::Building
                )
            {
                return Err(FrameIssue::Waiting);
            }
            if graph.phase == SceneInstanceGraphPhase::Failed {
                return Err(format!(
                    "场景实例 {path} 加载失败：{}",
                    graph.last_error.as_deref().unwrap_or("来源图错误")
                )
                .into());
            }
        }
        if let Some(status) = world.get_resource::<SubsceneRuntimeStatus>() {
            if status.root_scene_id.as_ref() != Some(root) {
                return Err(FrameIssue::Waiting);
            }
            if let Some(error) = &status.last_error {
                return Err(format!("场景实例 {path} 加载失败：{error}").into());
            }
            if !status.ready || graph.is_some_and(|g| status.applied_revision != g.revision) {
                return Err(FrameIssue::Waiting);
            }
        }
        return Err(format!("场景实例 {path} 当前未加载").into());
    };
    let frame = *world
        .get::<SceneSourceFrame>(entity)
        .ok_or_else(|| format!("场景实例 {path} 缺少坐标系"))?;
    Ok(Scope {
        scene: world
            .get::<SubsceneInstance>(entity)
            .unwrap()
            .source_scene_id
            .clone(),
        owner: Some(entity),
        instance: Some(path.into()),
        frame,
    })
}

impl Scope {
    pub fn target(&self, world: &World, name: &str, actor: bool) -> Result<Entity, FrameIssue> {
        let candidates = if actor {
            // 玩家贯穿同一世界。位于子地图的剧情使用该子地图坐标，仍指向真正的玩家根。
            let actors = &world.resource::<Cast>().actors;
            let owner = (name != "player").then(|| {
                self.instance
                    .clone()
                    .unwrap_or_else(|| self.scene.to_string())
            });
            actors.get(&(owner, name.to_owned())).or_else(|| {
                // 工程自行生成的无地址演员属于当前根世界，已登记来源地址的角色优先。
                (self.owner.is_none() && name != "player")
                    .then(|| actors.get(&(None, name.to_owned())))
                    .flatten()
            })
        } else {
            world
                .resource::<PlacementIndex>()
                .0
                .get(&(self.scene.clone(), name.to_owned()))
        };
        let mut found = candidates.into_iter().flatten().copied().filter(|&e| {
            world.get::<Disabled>(e).is_none()
                && (actor && world.get::<CinematicActor>(e).is_some()
                    || (world
                        .get::<ScenePlacementInstance>(e)
                        .is_some_and(|p| p.is_active())
                        && world.get::<SceneInstanceOwner>(e).map(|o| o.0) == self.owner))
        });
        let Some(entity) = found.next() else {
            let address = self.instance.as_deref().unwrap_or(self.scene.as_str());
            if actor
                && let Some(gate) =
                    world.get_resource::<feathrise_scene_core::state::SceneStartupGate>()
            {
                if let Some(error) = &gate.application_error {
                    return Err(format!("场景 {address} 演员 {name} 加载失败：{error}").into());
                }
                if !gate.ready {
                    return Err(FrameIssue::Waiting);
                }
            }
            if !actor {
                // 场景启动与摆放候选分别结算；同源的两个实例也各自发布就绪，不能借用根图的状态。
                let status = match self.owner {
                    Some(owner) => world
                        .get_resource::<InstanceResourceTable<ScenePlacementRuntimeStatus>>()
                        .and_then(|t| t.values.get(&owner)),
                    None => world.get_resource::<ScenePlacementRuntimeStatus>(),
                };
                if let Some(status) = status {
                    if status.scene_id.as_ref() != Some(&self.scene)
                        || matches!(
                            status.phase,
                            ScenePlacementRuntimePhase::Waiting
                                | ScenePlacementRuntimePhase::Building
                        )
                    {
                        return Err(FrameIssue::Waiting);
                    }
                    if status.phase == ScenePlacementRuntimePhase::Failed {
                        return Err(format!(
                            "场景 {address} 摆放 {name} 加载失败：{}",
                            status.detail
                        )
                        .into());
                    }
                }
            }
            return Err(format!("场景 {address} 中没有活动目标 {name}").into());
        };
        if found.next().is_some() {
            return Err(format!("演出目标 {name} 在同一场景实例中重复").into());
        }
        Ok(entity)
    }
}
