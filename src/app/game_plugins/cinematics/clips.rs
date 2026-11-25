//! 给同一模型的真实骨架临时使用单剪辑图。显式 seek 不推进游戏时间也不执行动画事件。
use super::FrameIssue;
use bevy::{
    animation::AnimatedBy, asset::LoadState, gltf::GltfAssetLabel, mesh::morph::MorphWeights,
    prelude::*,
};
use feathrise_scene_core::cinematic::{ActorClip, CinematicActor, CinematicClipProvider};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Clips(HashMap<Entity, Model>);

// ActorScene0 和整份 Gltf 是不同资产。演员仍在世界时保留已取用的目录，连续试播不重新导入骨架。
#[derive(Component)]
struct ActorModel {
    path: String,
    gltf: Handle<Gltf>,
}

struct Model {
    path: String,
    clip: Option<PreparedClip>,
}

struct PreparedClip {
    player: Entity,
    name: String,
    graph: Handle<AnimationGraph>,
    node: AnimationNodeIndex,
    duration: f32,
    born: u64,
}

#[derive(Clone)]
pub(super) struct ShotClip {
    pub player: Entity,
    pub graph: Handle<AnimationGraph>,
    pub node: AnimationNodeIndex,
    pub time: f32,
}

impl Clips {
    pub fn forget(&mut self, entity: Entity) {
        self.0.remove(&entity);
    }
    pub fn prepare(
        &mut self,
        world: &mut World,
        entity: Entity,
        name: &str,
        time: f32,
        pass: u64,
    ) -> Result<ShotClip, FrameIssue> {
        let actor = world.get::<CinematicActor>(entity).unwrap();
        let path = actor
            .model
            .as_ref()
            .ok_or_else(|| FrameIssue::Error(format!("角色 {} 缺少 GLB 模型", actor.target)))?
            .clone();
        let provider = world.get::<CinematicClipProvider>(entity).copied();
        let Some(player) = actor.player else {
            // 正在实例化的身体允许稍后绑定。只读宿主已请求的 Scene0，损坏或静态模型不能永久卡住导演。
            if let Some(server) = world.get_resource::<AssetServer>()
                && let Some(scene) = server
                    .get_handle::<WorldAsset>(GltfAssetLabel::Scene(0).from_asset(path.clone()))
            {
                if let LoadState::Failed(error) = server.load_state(scene.id()) {
                    return Err(format!("角色 GLB 加载失败 {path}：{error}").into());
                }
                if world
                    .get_resource::<Assets<WorldAsset>>()
                    .and_then(|a| a.get(&scene))
                    .is_some_and(|a| a.world.component_id::<AnimationPlayer>().is_none())
                {
                    return Err(format!("模型 {path} 没有可播放动作 {name} 的骨架").into());
                }
            }
            return Err(FrameIssue::Waiting);
        };
        if world.get::<AnimationPlayer>(player).is_none() {
            return Err(if provider.is_some() {
                FrameIssue::Waiting
            } else {
                FrameIssue::Error("角色的骨架播放器已卸载".into())
            });
        }
        let model = self.0.entry(entity).or_insert_with(|| Model {
            path: path.clone(),
            clip: None,
        });
        if let Some(clip) = model
            .clip
            .as_ref()
            .filter(|c| c.name == name && c.player == player && model.path == path)
        {
            return clip.sample(player, time, pass);
        }
        model.path = path.clone();
        // 已登记的外部动作由演员自己的正式动画图解析；只取该动作，不重载已实例化的身体 Scene0。
        let registered = if let Some(provider) = provider {
            (provider.0)(world, entity, name)?
        } else {
            ActorClip::Unregistered
        };
        let handle = match registered {
            ActorClip::Ready(handle) => handle,
            ActorClip::Waiting => return Err(FrameIssue::Waiting),
            ActorClip::Unregistered => native_clip(world, entity, &path, name)?,
        };
        let Some(duration) = world
            .resource::<Assets<AnimationClip>>()
            .get(&handle)
            .map(AnimationClip::duration)
        else {
            return Err(FrameIssue::Waiting);
        };
        let (graph, node) = AnimationGraph::from_clip(handle);
        let graph = world.resource_mut::<Assets<AnimationGraph>>().add(graph);
        model.clip = Some(PreparedClip {
            player,
            name: name.to_owned(),
            graph,
            node,
            duration,
            born: pass,
        });
        Err(FrameIssue::Waiting)
    }
}

fn native_clip(
    world: &mut World,
    entity: Entity,
    path: &str,
    name: &str,
) -> Result<Handle<AnimationClip>, FrameIssue> {
    let model = if let Some(model) = world.get::<ActorModel>(entity).filter(|m| m.path == path) {
        model.gltf.clone()
    } else {
        let server = world
            .get_resource::<AssetServer>()
            .ok_or_else(|| FrameIssue::Error("演出宿主没有资源服务器".into()))?;
        let asset = server.load(path.to_owned());
        world.entity_mut(entity).insert(ActorModel {
            path: path.to_owned(),
            gltf: asset.clone(),
        });
        asset
    };
    if let LoadState::Failed(error) = world.resource::<AssetServer>().load_state(model.id()) {
        return Err(FrameIssue::Error(format!(
            "角色 GLB 加载失败 {path}：{error}"
        )));
    }
    let gltfs = world
        .get_resource::<Assets<Gltf>>()
        .ok_or_else(|| FrameIssue::Error("工程未安装 glTF 资产插件".into()))?;
    let gltf = gltfs.get(&model).ok_or(FrameIssue::Waiting)?;
    gltf.named_animations
        .get(name)
        .or_else(|| {
            let (source, index) = name.rsplit_once("#Animation")?;
            if source != path {
                return None;
            }
            gltf.animations.get(index.parse::<usize>().ok()?)
        })
        .cloned()
        .ok_or_else(|| FrameIssue::Error(format!("模型 {path} 中没有动作 {name}")))
}

impl PreparedClip {
    fn sample(&self, player: Entity, time: f32, pass: u64) -> Result<ShotClip, FrameIssue> {
        // Bevy 先发布 AssetEvent 再构建用于实际骨骼的线程化动画图，两次 PostUpdate 后才取帧。
        if pass.saturating_sub(self.born) < 2 {
            return Err(FrameIssue::Waiting);
        }
        Ok(ShotClip {
            player,
            graph: self.graph.clone(),
            node: self.node,
            time: time.clamp(0., self.duration),
        })
    }
}

pub(super) struct OriginalPlayer {
    entity: Entity,
    original: AnimationPlayer,
    graph: Option<AnimationGraphHandle>,
    bones: Vec<(Entity, Transform, Option<MorphWeights>)>,
}

impl OriginalPlayer {
    pub fn entity(&self) -> Entity {
        self.entity
    }
    pub fn capture(world: &World, entity: Entity) -> Self {
        // 只访问这个 GLB 动画根下真正绑定到它的节点。原图没有覆盖的骨骼和形变也能准确回滚。
        let mut queue = vec![entity];
        let mut bones = Vec::new();
        while let Some(node) = queue.pop() {
            if world
                .get::<AnimatedBy>(node)
                .is_some_and(|owner| owner.0 == entity)
                && let Some(pose) = world.get::<Transform>(node)
            {
                bones.push((node, *pose, world.get::<MorphWeights>(node).cloned()));
            }
            if let Some(children) = world.get::<Children>(node) {
                queue.extend(children.iter());
            }
        }
        Self {
            entity,
            original: world.get::<AnimationPlayer>(entity).unwrap().clone(),
            graph: world.get::<AnimationGraphHandle>(entity).cloned(),
            bones,
        }
    }
    pub fn restore(self, world: &mut World) {
        if let Ok(mut e) = world.get_entity_mut(self.entity) {
            e.insert(self.original);
            if let Some(graph) = self.graph {
                e.insert(graph);
            } else {
                e.remove::<AnimationGraphHandle>();
            }
        }
        for (bone, pose, morph) in self.bones {
            if let Ok(mut e) = world.get_entity_mut(bone) {
                e.insert(pose);
                if let Some(weights) = morph {
                    e.insert(weights);
                }
            }
        }
    }
}

impl ShotClip {
    pub fn apply(&self, world: &mut World) {
        let mut e = world.entity_mut(self.player);
        if e.get::<AnimationGraphHandle>()
            .is_none_or(|g| g.0 != self.graph)
        {
            e.insert(AnimationGraphHandle(self.graph.clone()));
            e.get_mut::<AnimationPlayer>().unwrap().stop_all();
        }
        e.get_mut::<AnimationPlayer>()
            .unwrap()
            .play(self.node)
            .set_seek_time(self.time)
            .pause();
    }
}
