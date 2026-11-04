//! 本地作者动画：分帧冻结真实骨架，后台复用正式重定向算法，完整身份仍有效才挂图。

use super::{AuthorPrefabError, AuthorPrefabReady, Preparing};
use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_scene::animation_retarget::{detect_anim_root_name, retarget_clip};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};

// 同时保留至多4份源剪辑与冻结骨架；过期任务真正返回前仍占槽，不按鼠标/热更频率启动工作。
const MAX_JOBS: usize = 4;
const MAX_NODES: usize = 65_536;
const MAX_PLAYERS: usize = 32;

struct Node {
    entity: Entity,
    parent: Option<Entity>,
    name: Option<String>,
    player: bool,
}
struct Prepared {
    player: Entity,
    clip: AnimationClip,
}
type Output = Result<VecDeque<Prepared>, String>;
#[derive(Default)]
pub(super) struct Work {
    epoch: u64,
    source: Option<AssetId<AnimationClip>>,
    nodes: Vec<Node>,
    frozen: Option<Arc<Vec<Node>>>,
    ready: Option<Output>,
    installed: bool,
}
#[derive(Clone, Copy)]
struct Key {
    root: Entity,
    epoch: u64,
    source: AssetId<AnimationClip>,
}
struct Pending {
    key: Key,
    task: Task<Output>,
}
#[derive(Resource, Default)]
pub(super) struct Jobs {
    pending: Vec<Pending>,
}

fn retire<T: Send + 'static>(value: T) {
    AsyncComputeTaskPool::get()
        .spawn(async move { drop(value) })
        .detach();
}

fn invalidate_work(work: &mut Work) {
    work.epoch = work.epoch.wrapping_add(1);
    work.source = None;
    work.installed = false;
    if let Some(ready) = work.ready.take() {
        retire(ready);
    }
}

pub(super) fn reset_skeleton(work: &mut Work) {
    invalidate_work(work);
    let retired = (std::mem::take(&mut work.nodes), work.frozen.take());
    if !retired.0.is_empty() || retired.1.is_some() {
        retire(retired);
    }
}

pub(super) fn capture(world: &World, entity: Entity, work: &mut Work) -> Result<(), String> {
    if work.nodes.len() >= MAX_NODES {
        return Err("作者模型超过65536个节点的冻结预算".into());
    }
    work.nodes.push(Node {
        entity,
        parent: world.get::<ChildOf>(entity).map(ChildOf::parent),
        name: world.get::<Name>(entity).map(ToString::to_string),
        player: world.get::<AnimationPlayer>(entity).is_some(),
    });
    Ok(())
}

/// 只有动画源发生真实变更才撤回就绪；普通Added包含本执行器派生资产，不能造成自触发重建。
pub(super) fn invalidate(
    mut gltf_events: MessageReader<AssetEvent<Gltf>>,
    mut clip_events: MessageReader<AssetEvent<AnimationClip>>,
    mut roots: Query<(Entity, &mut Preparing)>,
    mut commands: Commands,
) {
    let gltfs: HashSet<_> = gltf_events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } | AssetEvent::Removed { id } => Some(*id),
            _ => None,
        })
        .collect();
    let clips: HashSet<_> = clip_events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } | AssetEvent::Removed { id } => Some(*id),
            _ => None,
        })
        .collect();
    if gltfs.is_empty() && clips.is_empty() {
        return;
    }
    for (root, mut state) in &mut roots {
        if state
            .animation
            .as_ref()
            .is_some_and(|h| gltfs.contains(&h.id()))
            || state
                .animation_work
                .source
                .is_some_and(|id| clips.contains(&id))
        {
            invalidate_work(&mut state.animation_work);
            state.started = Instant::now();
            commands
                .entity(root)
                .remove::<(AuthorPrefabReady, AuthorPrefabError)>()
                .insert(Visibility::Hidden);
        }
    }
}

pub(super) fn poll(world: &mut World) {
    let Some(mut jobs) = world.get_resource_mut::<Jobs>() else {
        return;
    };
    let mut completed = Vec::with_capacity(MAX_JOBS);
    let mut index = 0;
    while index < jobs.pending.len() {
        if let Some(output) = check_ready(&mut jobs.pending[index].task) {
            let pending = jobs.pending.swap_remove(index);
            completed.push((pending.key, output));
        } else {
            index += 1;
        }
    }
    drop(jobs);
    for (key, output) in completed {
        accept(world, key, output);
    }
}

fn accept(world: &mut World, key: Key, output: Output) {
    let valid = world.get::<AuthorPrefabError>(key.root).is_none()
        && world.get::<Preparing>(key.root).is_some_and(|state| {
            state.animation_work.epoch == key.epoch
                && state.animation_work.source == Some(key.source)
        });
    if valid {
        world
            .get_mut::<Preparing>(key.root)
            .unwrap()
            .animation_work
            .ready = Some(output);
    } else {
        // 身体、附件、动画换代或关闭后的迟到结果不会注册Assets，也不能打开Ready门。
        retire(output);
    }
}

pub(super) fn prepare(
    world: &mut World,
    root: Entity,
    state: &mut Preparing,
) -> Result<bool, String> {
    let Some(animation) = &state.prefab.idle else {
        return Ok(true);
    };
    let work = &mut state.animation_work;
    if work.installed {
        return Ok(true);
    }
    if let Some(output) = work.ready.take() {
        let mut prepared = output?;
        // 大模型的多播放器图按4份/帧登记；整个root继续隐藏，全部安装后才原子发布Ready。
        for _ in 0..4 {
            let Some(prepared) = prepared.pop_front() else {
                break;
            };
            if world.get::<AnimationPlayer>(prepared.player).is_none() {
                return Err("重定向完成时真实动画根已失效".into());
            }
            let clip = world
                .resource_mut::<Assets<AnimationClip>>()
                .add(prepared.clip);
            let (graph, node) = AnimationGraph::from_clip(clip);
            let graph = world.resource_mut::<Assets<AnimationGraph>>().add(graph);
            world
                .entity_mut(prepared.player)
                .insert(AnimationGraphHandle(graph));
            world
                .get_mut::<AnimationPlayer>(prepared.player)
                .unwrap()
                .stop_all()
                .play(node)
                .repeat();
        }
        if prepared.is_empty() {
            work.installed = true;
            return Ok(true);
        }
        work.ready = Some(Ok(prepared));
        return Ok(false);
    }
    if state.players.is_empty() {
        return Err("完整模型没有动画播放器".into());
    }
    let jobs = world.resource::<Jobs>();
    if jobs.pending.len() >= MAX_JOBS || jobs.pending.iter().any(|p| p.key.root == root) {
        return Ok(false);
    }
    let Some(gltf) = world
        .resource::<Assets<Gltf>>()
        .get(state.animation.as_ref().expect("有动画配置必有强句柄"))
    else {
        return Ok(false);
    };
    let source = gltf
        .named_animations
        .get(animation.name.as_str())
        .cloned()
        .ok_or_else(|| format!("待机剪辑不存在：{}", animation.name))?;
    let source_root = detect_anim_root_name(gltf);
    let Some(clip) = world.resource::<Assets<AnimationClip>>().get(&source) else {
        return Ok(false);
    };
    // 与正式游戏共享剪辑缓存相同：仅首次缺失冻结一次源资产；路径展开/曲线映射全在计算池执行。
    let clip = clip.clone();
    let nodes = work
        .frozen
        .get_or_insert_with(|| Arc::new(std::mem::take(&mut work.nodes)))
        .clone();
    let key = Key {
        root,
        epoch: work.epoch,
        source: source.id(),
    };
    work.source = Some(key.source);
    let task = AsyncComputeTaskPool::get().spawn(async move { build(nodes, clip, source_root) });
    world
        .resource_mut::<Jobs>()
        .pending
        .push(Pending { key, task });
    Ok(false)
}

fn build(nodes: Arc<Vec<Node>>, clip: AnimationClip, source_root: Option<String>) -> Output {
    let lookup: HashMap<_, _> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.entity, i))
        .collect();
    let mut children: HashMap<Entity, Vec<Entity>> = HashMap::new();
    let mut players = Vec::new();
    for (index, node) in nodes.iter().enumerate() {
        if lookup[&node.entity] != index {
            continue;
        }
        if let Some(parent) = node.parent {
            children.entry(parent).or_default().push(node.entity);
        }
        if node.player {
            players.push(node.entity);
        }
    }
    if players.is_empty() || players.len() > MAX_PLAYERS {
        return Err("作者模型的动画播放器数应在1至32之间".into());
    }
    let mut result = VecDeque::with_capacity(players.len());
    for player in players {
        let model_root = nodes[lookup[&player]]
            .name
            .as_ref()
            .ok_or_else(|| "真实动画根缺少节点名称".to_string())?;
        let mut paths = Vec::new();
        let mut path = Vec::new();
        let mut stack = vec![(player, true)];
        // 迭代遍历从真实播放器开始，Scene0/作者包装不进入目标哈希，也不依赖系统栈深度。
        while let Some((entity, entering)) = stack.pop() {
            let node = &nodes[lookup[&entity]];
            if !entering {
                if node.name.is_some() {
                    path.pop();
                }
                continue;
            }
            if let Some(name) = &node.name {
                path.push(name.clone());
                paths.push(path.clone());
            }
            stack.push((entity, false));
            if let Some(children) = children.get(&entity) {
                stack.extend(children.iter().rev().map(|child| (*child, true)));
            }
        }
        let mapped = if let Some(source_root) = &source_root {
            retarget_clip(clip.clone(), model_root, &paths, source_root)
        } else {
            clip.clone()
        };
        result.push_back(Prepared {
            player,
            clip: mapped,
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::animation::animated_field;

    #[test]
    fn local_idle_uses_real_player_root_and_rejects_hot_reload_late_result() {
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let mut world = World::new();
        let root = world.spawn_empty().id();
        let wrapper = world.spawn((Name::new("Scene0"), ChildOf(root))).id();
        let player = world
            .spawn((
                Name::new("Player"),
                AnimationPlayer::default(),
                ChildOf(wrapper),
            ))
            .id();
        let bone = world
            .spawn((Name::new("mixamorig:Hips"), ChildOf(player)))
            .id();
        let mut work = Work::default();
        for entity in [root, wrapper, player, bone] {
            capture(&world, entity, &mut work).unwrap();
        }
        let source: bevy::animation::AnimationTargetId =
            ["巨剑_待机_05", "mixamorig:Hips"].into_iter().collect();
        let expected: bevy::animation::AnimationTargetId =
            ["Player", "mixamorig:Hips"].into_iter().collect();
        let wrong: bevy::animation::AnimationTargetId =
            ["Scene0", "Player", "mixamorig:Hips"].into_iter().collect();
        let mut clip = AnimationClip::default();
        clip.add_curve_to_target(
            source,
            AnimatableCurve::new(
                animated_field!(Transform::rotation),
                ConstantCurve::new(Interval::EVERYWHERE, Quat::IDENTITY),
            ),
        );
        let task = AsyncComputeTaskPool::get()
            .spawn(async move { build(Arc::new(work.nodes), clip, Some("巨剑_待机_05".into())) });
        let mut task = task;
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        let output = loop {
            if let Some(output) = check_ready(&mut task) {
                break output;
            }
            assert!(Instant::now() < deadline, "本地待机后台映射未完成");
            std::thread::yield_now();
        };
        let mapped = &output.as_ref().unwrap()[0].clip;
        assert!(mapped.curves().contains_key(&expected));
        assert!(!mapped.curves().contains_key(&wrong));
        let source = AssetId::<AnimationClip>::default();
        let prefab = Arc::new(feathrise_map_content::author_visuals::AuthorPrefab {
            model: None,
            transform: feathrise_map_content::author_visuals::AuthorPose {
                translation: [0.; 3],
                rotation: [0., 0., 0., 1.],
                scale: [1.; 3],
            },
            attachments: vec![],
            idle: None,
        });
        world.entity_mut(root).insert(Preparing {
            prefab,
            layers: bevy::camera::visibility::RenderLayers::layer(31),
            body: wrapper,
            expected: Default::default(),
            arrived: Default::default(),
            queue: Default::default(),
            names: Default::default(),
            players: vec![player],
            attachments: true,
            started: Instant::now(),
            animation: None,
            animation_work: Work {
                source: Some(source),
                ..default()
            },
        });
        let key = Key {
            root,
            epoch: 0,
            source,
        };
        reset_skeleton(&mut world.get_mut::<Preparing>(root).unwrap().animation_work);
        accept(&mut world, key, output);
        assert!(
            world
                .get::<Preparing>(root)
                .unwrap()
                .animation_work
                .ready
                .is_none()
        );
        assert!(world.get::<AuthorPrefabReady>(root).is_none());
        world.despawn(root);
        accept(&mut world, key, Ok(VecDeque::new())); // 关闭后迟到结果同样不能重建根。
        assert!(world.get_entity(root).is_err());
    }
}
