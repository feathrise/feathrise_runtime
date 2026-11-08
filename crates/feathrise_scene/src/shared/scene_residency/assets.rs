//! 后台地图的资产通知保留到恢复；只处理变化和固定预算，不逐帧扫描完整地图。

use super::resources::ResidentScenes;
use bevy::{
    asset::{AssetId, UntypedAssetId},
    ecs::message::MessageCursor,
    prelude::*,
};
use feathrise_map_content::*;
use feathrise_scene_core::residency::{
    ResidentAssetEvent, ResidentMapEntity, ResidentMapToken, ResidentSceneActivated,
    SceneResidencyActivity,
};
use std::collections::{HashMap, HashSet, VecDeque, hash_set};

const EVENT_BUDGET: usize = 128;

#[cfg(test)]
#[path = "assets_tests.rs"]
mod tests;

#[derive(Resource, Default)]
pub(super) struct AssetOwners {
    assets: HashMap<UntypedAssetId, HashMap<ResidentMapToken, usize>>,
    entities: HashMap<Entity, (ResidentMapToken, Vec<UntypedAssetId>)>,
}
impl AssetOwners {
    pub fn remove(&mut self, entity: Entity) {
        let Some((token, ids)) = self.entities.remove(&entity) else {
            return;
        };
        for id in ids {
            let owners = self.assets.get_mut(&id).unwrap();
            let count = owners.get_mut(&token).unwrap();
            *count -= 1;
            if *count == 0 {
                owners.remove(&token);
            }
            if owners.is_empty() {
                self.assets.remove(&id);
            }
        }
    }
}

struct MapChanges<A: Asset> {
    known: Vec<AssetId<A>>,
    indexed: HashMap<AssetId<A>, usize>, // swap_remove同步更新位置，释放资产不扫描整张地图。
    pending: HashMap<AssetId<A>, AssetEvent<A>>,
    overrun: u64,
    shared_seen: u64, // 原运行时已消费到的共享资产版本；不由切图输入帧提前推进。
}
impl<A: Asset> Default for MapChanges<A> {
    fn default() -> Self {
        Self {
            known: Vec::new(),
            indexed: HashMap::new(),
            pending: HashMap::new(),
            overrun: 0,
            shared_seen: 0,
        }
    }
}

struct SharedAssetChange<A: Asset> {
    id: AssetId<A>,
    changed: u64,
}

#[derive(Clone, Copy)]
struct Replay {
    ticket: u64,
    token: ResidentMapToken,
    local_cursor: usize,
    local_done: bool,
    shared_cursor: usize,
    shared_end: usize, // 采用时冻结登记范围，当前地图新生成的资产不能让回放永远追不上。
    all: bool,
}

/// 通知发布不是消费完成；最多保留一帧的固定预算批次，到Last再确认游标和局部通知。
struct AssetDelivery<A: Asset> {
    token: ResidentMapToken,
    native: Vec<AssetEvent<A>>,
    pending_ids: Vec<AssetId<A>>,
    replay_ticket: Option<u64>,
    next_replay: Option<Replay>,
    completed_overrun: Option<u64>,
}
impl<A: Asset> AssetDelivery<A> {
    fn new(token: ResidentMapToken) -> Self {
        Self {
            token,
            native: Vec::new(),
            pending_ids: Vec::new(),
            replay_ticket: None,
            next_replay: None,
            completed_overrun: None,
        }
    }
}
#[derive(Resource)]
struct AssetChanges<A: Asset> {
    maps: HashMap<ResidentMapToken, MapChanges<A>>,
    replay: Option<Replay>,
    replay_ticket: u64,
    delivery: Option<AssetDelivery<A>>,
    dependents: HashMap<AssetId<A>, HashSet<ResidentMapToken>>,
    unused: VecDeque<(AssetId<A>, hash_set::IntoIter<ResidentMapToken>)>,
    retired: VecDeque<(ResidentMapToken, Vec<AssetId<A>>)>,
    overrun: u64,
    shared: Vec<SharedAssetChange<A>>,
    shared_index: HashMap<AssetId<A>, usize>,
    shared_serial: u64, // 非maps目录下的高度/密度/材质源也必须保留真实变更，而非只识别场景根。
}
impl<A: Asset> Default for AssetChanges<A> {
    fn default() -> Self {
        Self {
            maps: HashMap::new(),
            replay: None,
            replay_ticket: 0,
            delivery: None,
            dependents: HashMap::new(),
            unused: VecDeque::new(),
            retired: VecDeque::new(),
            overrun: 0,
            shared: Vec::new(),
            shared_index: HashMap::new(),
            shared_serial: 0,
        }
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<AssetOwners>()
        .add_observer(register_root_asset)
        .add_observer(register_mesh_asset);
    watch::<MapManifestAsset>(app);
    watch::<MapPlacementsAsset>(app);
    watch::<MapPortalsAsset>(app);
    watch::<MapBoundariesAsset>(app);
    watch::<MapSurfaceAsset>(app);
    watch::<MapVegetationAsset>(app);
    watch::<MapWaterAsset>(app);
    watch::<MapWaterMaskAsset>(app);
    watch::<MapSurfaceSourceAsset>(app);
    watch::<SparseHeightManifestAsset>(app);
    watch::<Image>(app);
    watch::<Mesh>(app);
    watch::<WorldAsset>(app);
    watch::<StandardMaterial>(app);
    watch::<feathrise_environment::terrain_surface::TerrainSurfaceMaterial>(app);
    watch::<crate::DynamicWaterMaterial>(app);
    watch::<crate::maps::ClipmapWaterMaterial>(app);
    watch::<Gltf>(app);
    watch::<AnimationClip>(app);
}

fn watch<A: Asset>(app: &mut App) {
    app.init_resource::<AssetChanges<A>>()
        .add_message::<ResidentAssetEvent<A>>()
        .add_observer(begin_replay::<A>)
        .add_systems(PreUpdate, (collect::<A>, replay::<A>).chain())
        .add_systems(
            Last,
            acknowledge_active::<A>.before(super::systems::advance_scene_residency),
        );
}

/// 应用声明自身资产需要同一驻留热更回放与关闭回收，公共场景不枚举领域 DTO。
#[derive(Resource, Default)]
struct ApplicationAssetCleanup(Vec<fn(&mut World, ResidentMapToken, &mut ResidentScenes)>);

pub fn watch_scene_application_asset<A: Asset>(app: &mut App) {
    if app.world().contains_resource::<AssetChanges<A>>() { return; }
    watch::<A>(app);
    app.init_resource::<ApplicationAssetCleanup>();
    app.world_mut().resource_mut::<ApplicationAssetCleanup>().0.push(forget_type::<A>);
}

fn event_id<A: Asset>(event: &AssetEvent<A>) -> AssetId<A> {
    match *event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::LoadedWithDependencies { id }
        | AssetEvent::Unused { id } => id,
    }
}

fn remember<A: Asset>(changes: &mut AssetChanges<A>, token: ResidentMapToken, id: AssetId<A>) {
    let map = changes.maps.entry(token).or_default();
    if !map.indexed.contains_key(&id) {
        map.indexed.insert(id, map.known.len());
        map.known.push(id);
    }
    changes.dependents.entry(id).or_default().insert(token);
}

fn retain_pending<A: Asset>(map: &mut MapChanges<A>, event: AssetEvent<A>) {
    let id = event_id(&event);
    // Loaded不能覆盖尚未消费的Modified；同一资产只保留最新的有效通知。
    if !matches!(event, AssetEvent::LoadedWithDependencies { .. })
        || !map.pending.get(&id).is_some_and(|event| {
            matches!(event, AssetEvent::Modified { .. } | AssetEvent::Removed { .. })
        })
    {
        map.pending.insert(id, event);
    }
}

/// 反向索引只访问真正依赖该资产的地图；Unused及关闭地图的清理共享每类128次访问预算。
fn retire_dependencies<A: Asset>(changes: &mut AssetChanges<A>) {
    for _ in 0..EVENT_BUDGET {
        if let Some((id, tokens)) = changes.unused.front_mut() {
            let id = *id;
            let Some(token) = tokens.next() else {
                changes.unused.pop_front();
                continue;
            };
            if changes
                .dependents
                .get(&id)
                .is_some_and(|tokens| tokens.contains(&token))
            {
                continue; // 同一ID在Unused之后被重新登记，旧释放通知不得删除新的有效依赖。
            }
            if let Some(map) = changes.maps.get_mut(&token) {
                map.pending.remove(&id);
                if let Some(index) = map.indexed.remove(&id) {
                    map.known.swap_remove(index);
                    if let Some(swapped) = map.known.get(index) {
                        map.indexed.insert(*swapped, index);
                    }
                    if let Some(replay) = changes.replay.as_mut()
                        && replay.token == token
                    {
                        replay.local_cursor = replay.local_cursor.min(index);
                    } // 被移到前面的活依赖仍需复核，不能因回收跳过。
                }
            }
        } else if let Some((token, ids)) = changes.retired.front_mut() {
            let token = *token;
            let Some(id) = ids.pop() else {
                changes.retired.pop_front();
                continue;
            };
            if let Some(tokens) = changes.dependents.get_mut(&id) {
                tokens.remove(&token);
                if tokens.is_empty() {
                    changes.dependents.remove(&id);
                }
            }
        } else {
            break;
        }
    }
}

/// 归属登记与资产根登记可能先后到达，两条观察器都补登记相同的O(1)依赖。
pub(super) fn register_owned_assets(world: &mut World, entity: Entity) {
    let Some(owner) = world.get::<ResidentMapEntity>(entity).copied() else {
        return;
    };
    let mut ids = Vec::new(); // 大部分归属节点没有独立资产；不为每个空节点分配堆内存。
    if let Some(id) = world.get::<WorldAssetRoot>(entity).map(|root| root.0.id()) {
        ids.push(id.untyped());
        remember(
            &mut world.resource_mut::<AssetChanges<WorldAsset>>(),
            owner.0,
            id,
        );
    }
    if let Some(id) = world.get::<Mesh3d>(entity).map(|mesh| mesh.0.id()) {
        ids.push(id.untyped());
        remember(&mut world.resource_mut::<AssetChanges<Mesh>>(), owner.0, id);
    }
    let mut owners = world.resource_mut::<AssetOwners>();
    owners.remove(entity);
    for id in &ids {
        *owners
            .assets
            .entry(*id)
            .or_default()
            .entry(owner.0)
            .or_default() += 1;
    }
    if !ids.is_empty() {
        owners.entities.insert(entity, (owner.0, ids));
    }
}

fn register_root_asset(event: On<Insert<WorldAssetRoot>>, mut commands: Commands) {
    let entity = event.entity;
    commands.queue(move |world: &mut World| register_owned_assets(world, entity));
}
fn register_mesh_asset(event: On<Insert<Mesh3d>>, mut commands: Commands) {
    let entity = event.entity;
    commands.queue(move |world: &mut World| register_owned_assets(world, entity));
}

fn collect<A: Asset>(
    messages: Option<Res<Messages<AssetEvent<A>>>>,
    mut cursor: Local<MessageCursor<AssetEvent<A>>>,
    server: Option<Res<AssetServer>>,
    owners: Res<AssetOwners>,
    scenes: Res<ResidentScenes>,
    activity: Res<SceneResidencyActivity>,
    mut changes: ResMut<AssetChanges<A>>,
) {
    if !activity.enabled {
        return;
    } // 无驻留入口时不维护通知副本；原热更新读者继续直接消费。
    let Some(messages) = messages else {
        retire_dependencies(&mut changes);
        return;
    };
    if cursor.missed_messages(&messages) != 0 {
        changes.overrun = changes.overrun.wrapping_add(1); // 突发超预算时暖恢复分帧复核全部已登记依赖，不静默丢失热更。
    }
    for event in cursor.read(&messages).take(EVENT_BUDGET) {
        let id = event_id(event);
        if matches!(event, AssetEvent::Unused { .. }) {
            // 强句柄已经全部释放，不可能还有驻留图依赖；局部退役，避免动态Mesh世代积累。
            if let Some(tokens) = changes.dependents.remove(&id) {
                changes.unused.push_back((id, tokens.into_iter()));
            }
            if let Some(index) = changes.shared_index.remove(&id) {
                changes.shared.swap_remove(index);
                if let Some(swapped) = changes.shared.get(index).map(|entry| entry.id) {
                    changes.shared_index.insert(swapped, index);
                }
                let len = changes.shared.len();
                if let Some(replay) = changes.replay.as_mut() {
                    replay.shared_cursor = replay.shared_cursor.min(index);
                    replay.shared_end = replay.shared_end.min(len);
                }
            }
            continue;
        }
        let source_path = server.as_ref().and_then(|server| server.get_path(id));
        let modified = matches!(
            event,
            AssetEvent::Modified { .. } | AssetEvent::Removed { .. }
        );
        // 文件依赖先登记，供突发丢通知后的复核。无路径的临时Mesh只记真实Modified，忽略大量Added/Removed。
        if matches!(event, AssetEvent::Modified { .. })
            || changes.shared_index.contains_key(&id)
            || (source_path.is_some() && !matches!(event, AssetEvent::Removed { .. }))
        {
            let index = if let Some(index) = changes.shared_index.get(&id) {
                *index
            } else {
                let index = changes.shared.len();
                changes.shared.push(SharedAssetChange { id, changed: 0 });
                changes.shared_index.insert(id, index);
                index
            };
            if modified {
                changes.shared_serial += 1;
                changes.shared[index].changed = changes.shared_serial;
            }
        }
        let path_token = source_path.and_then(|path| {
            let path = path.path().to_string_lossy().replace('\\', "/");
            let folder = path.strip_prefix("maps/")?.split('/').next()?;
            let map = MapId::parse(folder).ok()?;
            scenes.maps.get(&map).map(|entry| entry.token)
        });
        let explicit = owners.assets.get(&id.untyped());
        let path_token =
            path_token.filter(|token| explicit.is_none_or(|owners| !owners.contains_key(token)));
        let explicit = explicit
            .into_iter()
            .flat_map(|owners| owners.keys())
            .copied();
        for token in explicit.chain(path_token) {
            remember(&mut changes, token, id);
            if activity.switching || activity.active != Some(token) {
                let map = changes.maps.get_mut(&token).unwrap();
                retain_pending(map, *event);
            } else {
                // PreUpdate尚不能证明本帧能消费：Update中的标签请求可能先冻结所有地图系统。
                changes
                    .delivery
                    .get_or_insert_with(|| AssetDelivery::new(token))
                    .native
                    .push(*event);
            }
        }
    }
    retire_dependencies(&mut changes);
}

fn begin_replay<A: Asset>(event: On<ResidentSceneActivated>, mut changes: ResMut<AssetChanges<A>>) {
    let all = changes
        .maps
        .get(&event.0)
        .is_some_and(|map| map.overrun != changes.overrun);
    changes.maps.entry(event.0).or_default();
    changes.replay_ticket = changes.replay_ticket.wrapping_add(1);
    changes.replay = Some(Replay {
        ticket: changes.replay_ticket,
        token: event.0,
        local_cursor: 0,
        local_done: false,
        shared_cursor: 0,
        shared_end: changes.shared.len(),
        all,
    });
}

/// 只在原运行时确实有资格消费通知的帧末记录版本；输入帧先开启切图锁时不能丢掉本帧更新。
fn acknowledge_active<A: Asset>(
    activity: Res<SceneResidencyActivity>,
    mut changes: ResMut<AssetChanges<A>>,
) {
    if let Some(delivery) = changes.delivery.take() {
        if !activity.enabled || activity.switching || activity.active != Some(delivery.token) {
            if let Some(map) = changes.maps.get_mut(&delivery.token) {
                for event in delivery.native {
                    if map.indexed.contains_key(&event_id(&event)) {
                        retain_pending(map, event);
                    }
                }
            }
            return; // 未确认的回放仍保留原pending和游标；真实通知也转存，下一次恢复不会漏收。
        }
        if let Some(ticket) = delivery.replay_ticket
            && changes
                .replay
                .as_ref()
                .is_some_and(|replay| replay.ticket == ticket)
        {
            let map = changes.maps.get_mut(&delivery.token).unwrap();
            for id in delivery.pending_ids {
                map.pending.remove(&id);
            }
            if let Some(overrun) = delivery.completed_overrun {
                map.overrun = overrun;
            }
            changes.replay = delivery.next_replay;
        } // 同帧再次激活产生新票据时，只能推进新事务自己的通知，不能提交旧批次。
    }
    if !activity.enabled || activity.switching || changes.replay.is_some() {
        return;
    }
    if let Some(token) = activity.active {
        let serial = changes.shared_serial;
        let overrun = changes.overrun;
        let map = changes.maps.entry(token).or_default();
        map.shared_seen = serial;
        map.overrun = overrun; // 活动地图原读者已消费本帧资产，不把其正常冷载突发误判为后台丢通知。
    }
}

fn replay<A: Asset>(
    mut changes: ResMut<AssetChanges<A>>,
    mut messages: Option<ResMut<Messages<ResidentAssetEvent<A>>>>,
    assets: Option<Res<Assets<A>>>,
    activity: Res<SceneResidencyActivity>,
) {
    let Some(mut replay) = changes.replay else {
        return;
    };
    let token = replay.token;
    if activity.switching || activity.active != Some(token) {
        return;
    }
    let overrun = changes.overrun;
    let Some(messages) = messages.as_mut() else {
        return;
    };
    let AssetChanges {
        maps, shared, delivery, ..
    } = &mut *changes;
    let map = maps.get_mut(&token).unwrap();
    let delivery = delivery.get_or_insert_with(|| AssetDelivery::new(token));
    delivery.replay_ticket = Some(replay.ticket);
    let mut remaining = EVENT_BUDGET;
    if !replay.local_done && replay.all {
        for id in map.known.iter().skip(replay.local_cursor).take(remaining) {
            messages.write(ResidentAssetEvent(
                if assets.as_ref().is_some_and(|assets| assets.contains(*id)) {
                    AssetEvent::Modified { id: *id }
                } else {
                    AssetEvent::Removed { id: *id }
                },
            ));
            delivery.pending_ids.push(*id);
            replay.local_cursor += 1;
            remaining -= 1;
        }
        replay.local_done = replay.local_cursor >= map.known.len();
    } else if !replay.local_done {
        for (id, event) in map.pending.iter().take(remaining) {
            messages.write(ResidentAssetEvent(*event));
            delivery.pending_ids.push(*id);
            remaining -= 1;
        }
        replay.local_done = delivery.pending_ids.len() == map.pending.len();
    }
    // 按原AssetId回放共享源；原消费者仍做完整句柄过滤。已属本地图的通知不重复发布。
    if replay.local_done {
        for change in shared[..replay.shared_end]
            .iter()
            .skip(replay.shared_cursor)
            .take(remaining)
        {
            replay.shared_cursor += 1;
            if map.indexed.contains_key(&change.id)
                || (!replay.all && change.changed <= map.shared_seen)
            {
                continue;
            }
            let event = if assets
                .as_ref()
                .is_some_and(|assets| assets.contains(change.id))
            {
                AssetEvent::Modified { id: change.id }
            } else {
                AssetEvent::Removed { id: change.id }
            };
            messages.write(ResidentAssetEvent(event));
        }
    }
    if !replay.local_done || replay.shared_cursor < replay.shared_end {
        delivery.next_replay = Some(replay);
    } else {
        delivery.completed_overrun = Some(overrun);
    }
    // 游标、pending和丢消息复核版本只在本帧所有地图系统仍有资格运行时一起提交。
}

fn forget_type<A: Asset>(world: &mut World, token: ResidentMapToken, scenes: &mut ResidentScenes) {
    let mut changes = world.resource_mut::<AssetChanges<A>>();
    if let Some(mut map) = changes.maps.remove(&token) {
        if !map.known.is_empty() {
            changes
                .retired
                .push_back((token, std::mem::take(&mut map.known)));
        } // 大表只移动所有权；反向索引后续按相同固定预算退役。
        scenes.retired_resources.push_back(Box::new(map));
    }
    if changes
        .replay
        .as_ref()
        .is_some_and(|replay| replay.token == token)
    {
        changes.replay = None;
    }
    if changes
        .delivery
        .as_ref()
        .is_some_and(|delivery| delivery.token == token)
    {
        changes.delivery = None;
    }
}

/// 固定类型数的回收，不留下关闭地图的通知和依赖表。
pub(super) fn forget(world: &mut World, token: ResidentMapToken, scenes: &mut ResidentScenes) {
    macro_rules! forget { ($($ty:ty),*) => { $(forget_type::<$ty>(world, token, scenes);)* }; }
    forget!(
        MapManifestAsset,
        MapPlacementsAsset,
        MapPortalsAsset,
        MapBoundariesAsset,
        MapSurfaceAsset,
        MapVegetationAsset,
        MapWaterAsset,
        MapWaterMaskAsset,
        MapSurfaceSourceAsset,
        SparseHeightManifestAsset,
        Image,
        Mesh,
        WorldAsset,
        StandardMaterial,
        feathrise_environment::terrain_surface::TerrainSurfaceMaterial,
        crate::DynamicWaterMaterial,
        crate::maps::ClipmapWaterMaterial,
        Gltf,
        AnimationClip
    );
    let callbacks = world.get_resource::<ApplicationAssetCleanup>()
        .map(|value| value.0.clone()).unwrap_or_default();
    for cleanup in callbacks { cleanup(world, token, scenes); }
    // 根/网格依赖已经随固定预算的实体Remove观察器局部回收。
}
