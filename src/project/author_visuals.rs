//! 中性Scene0作者外观执行器；只执行领域工厂导出的变换、名称附件和动画，不解释RPG数据。
use bevy::{
    camera::visibility::{NoFrustumCulling, RenderLayers},
    prelude::*,
    world_serialization::WorldInstanceReady,
};
use feathrise_scene_content::author_visuals::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};
mod animation;

#[derive(Component)]
pub struct AuthorPrefabReady;
#[derive(Component)]
pub struct AuthorPrefabError(pub String);
#[derive(Component)]
struct ModelOwner(Entity);
#[derive(Component)]
struct Preparing {
    prefab: Arc<AuthorPrefab>,
    layers: RenderLayers,
    body: Entity,
    expected: HashSet<Entity>,
    arrived: HashSet<Entity>,
    queue: VecDeque<Entity>,
    names: HashMap<String, Entity>,
    players: Vec<Entity>,
    attachments: bool,
    started: Instant,
    animation: Option<Handle<Gltf>>,
    animation_work: animation::Work,
}
pub struct AuthorVisualsPlugin;
impl Plugin for AuthorVisualsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<animation::Jobs>()
            .add_observer(ready)
            .add_systems(Update, (animation::invalidate, advance).chain());
    }
}
pub fn transform(p: &AuthorPose) -> Transform {
    Transform {
        translation: Vec3::from_array(p.translation),
        rotation: Quat::from_array(p.rotation),
        scale: Vec3::from_array(p.scale),
    }
}
pub fn line_mesh(lines: AuthorLines) -> Mesh {
    use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology};
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, lines.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, lines.colors)
}
fn model(
    world: &mut World,
    owner: Entity,
    parent: Entity,
    path: String,
    pose: Transform,
    layers: RenderLayers,
) -> Entity {
    let handle = world
        .resource::<AssetServer>()
        .load(GltfAssetLabel::Scene(0).from_asset(path));
    world
        .spawn((
            Name::new("外部作者完整模型"),
            WorldAssetRoot(handle),
            pose,
            Visibility::Inherited,
            layers,
            ModelOwner(owner),
            ChildOf(parent),
        ))
        .id()
}
pub fn spawn_prefab(world: &mut World, prefab: Arc<AuthorPrefab>, layers: RenderLayers) -> Entity {
    let root = world
        .spawn((
            Name::new("外部作者角色预览"),
            Transform::default(),
            Visibility::Hidden,
            layers.clone(),
        ))
        .id();
    let mut expected = HashSet::new();
    let mut arrived = HashSet::new();
    let mut queue = VecDeque::new();
    let body = if let Some(path) = &prefab.model {
        let entity = model(
            world,
            root,
            root,
            path.clone(),
            transform(&prefab.transform),
            layers.clone(),
        );
        expected.insert(entity);
        entity
    } else {
        // 该几何来自领域正式内置胶囊合同；空工程也显示真实Mesh，不请求游戏私有模型。
        let mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(Capsule3d::new(0.5, 1.));
        let material = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: Color::srgb(0.65, 0.7, 0.78),
                perceptual_roughness: 0.8,
                ..default()
            });
        let entity = world
            .spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                transform(&prefab.transform),
                Visibility::Inherited,
                layers.clone(),
                NoFrustumCulling,
                ChildOf(root),
            ))
            .id();
        expected.insert(entity);
        arrived.insert(entity);
        queue.push_back(entity);
        entity
    };
    let animation = prefab
        .idle
        .as_ref()
        .map(|a| world.resource::<AssetServer>().load(a.asset.clone()));
    world.entity_mut(root).insert(Preparing {
        prefab,
        layers,
        body,
        expected,
        arrived,
        queue,
        names: Default::default(),
        players: vec![],
        attachments: false,
        started: Instant::now(),
        animation,
        animation_work: Default::default(),
    });
    root
}

/// 只触达该作者根已经冻结的至多32个播放器；不扫描场景或安装任何玩法系统。
pub fn set_prefab_playback(world: &mut World, root: Entity, playing: bool, speed: f32) -> bool {
    if !speed.is_finite() || !(0.05..=4.0).contains(&speed)
        || world.get::<AuthorPrefabReady>(root).is_none()
    {
        return false;
    }
    let Some(state) = world.get::<Preparing>(root) else { return false; };
    if state.players.len() > 32 { return false; }
    let players = state.players.clone();
    for entity in players {
        if let Some(mut player) = world.get_mut::<AnimationPlayer>(entity) {
            for (_, animation) in player.playing_animations_mut() {
                animation.set_speed(speed);
                if playing { animation.resume(); } else { animation.pause(); }
            }
        }
    }
    true
}
fn ready(event: On<WorldInstanceReady>, owners: Query<&ModelOwner>, mut commands: Commands) {
    let Ok(owner) = owners.get(event.entity) else {
        return;
    };
    let owner = owner.0;
    let entity = event.entity;
    commands.queue(move |world: &mut World| {
        model_ready(world, owner, entity);
    });
}
fn model_ready(world: &mut World, owner: Entity, entity: Entity) {
    if world.get_entity(owner).is_err() {
        return;
    }
    let finished = world.get::<AuthorPrefabReady>(owner).is_some()
        || world.get::<AuthorPrefabError>(owner).is_some();
    let Some(mut state) = world.entity_mut(owner).take::<Preparing>() else {
        return;
    };
    if !state.expected.contains(&entity) {
        world.entity_mut(owner).insert(state);
        return;
    }
    if entity == state.body {
        // 身体热更使旧骨骼/武器和动画根失效；只撤回至多32个附件根，完整扫描仍按原128节点预算。
        for old in state.expected.iter().copied().filter(|e| *e != state.body) {
            world.despawn(old);
        }
        state.expected.clear();
        state.expected.insert(state.body);
        state.arrived.clear();
        state.queue.clear();
        state.attachments = false;
    } else if finished {
        state.queue.clear();
    }
    if entity == state.body || finished {
        animation::reset_skeleton(&mut state.animation_work);
        let retired = (
            std::mem::take(&mut state.names),
            std::mem::take(&mut state.players),
        );
        if !retired.0.is_empty() || !retired.1.is_empty() {
            bevy::tasks::AsyncComputeTaskPool::get()
                .spawn(async move {
                    drop(retired);
                })
                .detach();
        }
    }
    state.arrived.insert(entity);
    state.queue.push_back(if entity == state.body || !finished {
        entity
    } else {
        owner
    });
    state.started = Instant::now();
    world
        .entity_mut(owner)
        .remove::<(AuthorPrefabReady, AuthorPrefabError)>()
        .insert((Visibility::Hidden, state));
}
fn advance(world: &mut World) {
    animation::poll(world);
    let roots: Vec<_> = world
        .query_filtered::<Entity, (
            With<Preparing>,
            Without<AuthorPrefabReady>,
            Without<AuthorPrefabError>,
        )>()
        .iter(world)
        .take(4)
        .collect();
    for root in roots {
        let Some(mut state) = world.entity_mut(root).take::<Preparing>() else {
            continue;
        };
        let result = step(world, root, &mut state);
        match result {
            Ok(true) => {
                // 保留不可变来源及已建索引，稳定帧不查询；下一次真实WorldInstanceReady可重新进入原准备流程。
                world.entity_mut(root).insert((AuthorPrefabReady, state));
            }
            Ok(false) => {
                world.entity_mut(root).insert(state);
            }
            Err(error) => {
                world
                    .entity_mut(root)
                    .insert((AuthorPrefabError(error), state));
            }
        }
    }
}
fn step(world: &mut World, root: Entity, state: &mut Preparing) -> Result<bool, String> {
    if state.started.elapsed().as_secs() > 60 {
        return Err("作者模型、附件或动画60秒内未完整就绪".into());
    }
    // Scene0和附件后代仅在Ready后按128节点预算隔离，稳定帧完全不遍历骨架。
    for _ in 0..128 {
        let Some(entity) = state.queue.pop_front() else {
            break;
        };
        if world.get_entity(entity).is_err() {
            continue;
        }
        if state.prefab.idle.is_some() {
            animation::capture(world, entity, &mut state.animation_work)?;
        }
        if let Some(name) = world.get::<Name>(entity) {
            state.names.entry(name.to_string()).or_insert(entity);
        }
        if world.get::<AnimationPlayer>(entity).is_some() {
            state.players.push(entity);
        }
        if let Some(mut camera) = world.get_mut::<Camera>(entity) {
            camera.is_active = false;
        }
        if let Some(children) = world.get::<Children>(entity) {
            state.queue.extend(children.iter());
        }
        world
            .entity_mut(entity)
            .insert((state.layers.clone(), NoFrustumCulling));
    }
    if !state.queue.is_empty() || !state.expected.is_subset(&state.arrived) {
        return Ok(false);
    }
    if !state.attachments {
        for attachment in &state.prefab.attachments {
            let parent = *state
                .names
                .get(&attachment.parent_node)
                .ok_or_else(|| format!("完整模型缺少附件节点{}", attachment.parent_node))?;
            let entity = model(
                world,
                root,
                parent,
                attachment.model.clone(),
                transform(&attachment.transform),
                state.layers.clone(),
            );
            state.expected.insert(entity);
        }
        state.attachments = true;
        if !state.expected.is_subset(&state.arrived) {
            return Ok(false);
        }
    }
    animation::prepare(world, root, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_hot_reload_reisolates_new_nodes_and_recreates_attachment_before_ready() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<bevy::world_serialization::WorldAsset>();
        let world = app.world_mut();
        let root = world
            .spawn((
                Transform::default(),
                Visibility::Inherited,
                AuthorPrefabReady,
            ))
            .id();
        let body = world
            .spawn((Name::new("body"), Transform::default(), ChildOf(root)))
            .id();
        let old_bone = world
            .spawn((Name::new("old_socket"), Transform::default(), ChildOf(body)))
            .id();
        let old_weapon = world.spawn((Transform::default(), ChildOf(old_bone))).id();
        let bone = world
            .spawn((Name::new("socket"), Transform::default(), ChildOf(body)))
            .id();
        let prefab = Arc::new(AuthorPrefab {
            model: Some("body.glb".into()),
            transform: AuthorPose {
                translation: [0.; 3],
                rotation: [0., 0., 0., 1.],
                scale: [1.; 3],
            },
            attachments: vec![AuthorAttachment {
                model: "weapon.glb".into(),
                parent_node: "socket".into(),
                transform: AuthorPose {
                    translation: [0.; 3],
                    rotation: [0., 0., 0., 1.],
                    scale: [1.; 3],
                },
            }],
            idle: None,
        });
        world.entity_mut(root).insert(Preparing {
            prefab,
            layers: RenderLayers::layer(31),
            body,
            expected: [body, old_weapon].into_iter().collect(),
            arrived: [body, old_weapon].into_iter().collect(),
            queue: Default::default(),
            names: [("socket".into(), old_bone)].into_iter().collect(),
            players: vec![],
            attachments: true,
            started: Instant::now(),
            animation: None,
            animation_work: Default::default(),
        });
        model_ready(world, root, body);
        assert!(world.get::<AuthorPrefabReady>(root).is_none());
        assert_eq!(*world.get::<Visibility>(root).unwrap(), Visibility::Hidden);
        assert!(world.get_entity(old_weapon).is_err());
        advance(world);
        assert_eq!(
            world.get::<RenderLayers>(bone).unwrap(),
            &RenderLayers::layer(31)
        );
        assert!(
            world.get::<AuthorPrefabReady>(root).is_none(),
            "新武器尚未真实Ready不能发布"
        );
        let state = world.get::<Preparing>(root).unwrap();
        let weapon = *state.expected.iter().find(|e| **e != body).unwrap();
        assert_eq!(world.get::<ChildOf>(weapon).unwrap().parent(), bone);
        model_ready(world, root, weapon);
        advance(world);
        assert!(world.get::<AuthorPrefabReady>(root).is_some());
        assert!(world.get::<Preparing>(root).is_some());
        world.despawn(root);
        model_ready(world, root, weapon);
        assert!(world.get_entity(weapon).is_err());
    }
}
