//! 根和子关系增量登记；按固定实体预算暂停/恢复，不在切图帧递归扫描整张地图。

use super::{
    components::{ParkedSceneEntity, ResumeResidentAudio},
    resources::{ResidentEntityIndex, ResidentScenes},
};
use avian3d::prelude::{Collider, ColliderDisabled, RigidBody, RigidBodyDisabled};
use bevy::audio::AudioSinkPlayback;
use bevy::{
    ecs::{entity_disabling::Disabled, query::Allow},
    prelude::*,
};
use feathrise_map_content::{ActiveDynamicMap, MapId};
use feathrise_scene_core::residency::{ResidentMapEntity, SceneResidencyActivity};
use feathrise_scene_core::state::ScenePhase;

pub(super) const ENTITY_BUDGET: usize = 256;

pub(super) fn map_for_scene(scene: ScenePhase, active: Option<&ActiveDynamicMap>) -> Option<MapId> {
    match scene {
        ScenePhase::Active => active.and_then(|active| active.map_id.clone()),
        _ => None,
    }
}

pub(super) fn register_scene_scoped_root(
    insert: On<Insert, DespawnOnExit<ScenePhase>>,
    scoped: Query<&DespawnOnExit<ScenePhase>, Allow<Disabled>>,
    children: Query<&ChildOf, Allow<Disabled>>,
    owners: Query<&ResidentMapEntity, Allow<Disabled>>,
    active_dynamic: Option<Res<ActiveDynamicMap>>,
    current: Option<Res<State<ScenePhase>>>,
    mut scenes: ResMut<ResidentScenes>,
    mut activity: ResMut<SceneResidencyActivity>,
    mut commands: Commands,
) {
    if !activity.enabled {
        return;
    } // 未装配多文档的正式游戏沿用原状态作用域，不建立驻留索引。
    if let Ok(parent) = children.get(insert.entity)
        && let Ok(owner) = owners.get(parent.parent())
    {
        commands.entity(insert.entity).insert(*owner);
        return; // 后台Scene异步补出的节点必须继承父地图，不能被当前DynamicMap身份重新认领。
    }
    let Ok(scope) = scoped.get(insert.entity) else {
        return;
    };
    let Some(map) = map_for_scene(scope.0, active_dynamic.as_deref()) else {
        return;
    };
    let token = scenes.ensure(map.clone(), scope.0);
    if scenes.transfer.is_none()
        && current
            .as_ref()
            .is_some_and(|state| *state.get() == scope.0)
    {
        scenes.active = Some(map);
        activity.active = Some(token);
    }
    commands
        .entity(insert.entity)
        .insert(ResidentMapEntity(token));
}

pub(super) fn register_additional_root<T: Component>(
    insert: On<Insert, T>,
    children: Query<&ChildOf, Allow<Disabled>>,
    owners: Query<&ResidentMapEntity, Allow<Disabled>>,
    current: Option<Res<State<ScenePhase>>>,
    active_dynamic: Option<Res<ActiveDynamicMap>>,
    scenes: Option<ResMut<ResidentScenes>>,
    activity: Option<ResMut<SceneResidencyActivity>>,
    mut commands: Commands,
) {
    if let Ok(parent) = children.get(insert.entity)
        && let Ok(owner) = owners.get(parent.parent())
    {
        commands.entity(insert.entity).insert(*owner);
        return;
    }
    let (Some(mut scenes), Some(mut activity)) = (scenes, activity) else {
        return;
    };
    let Some(scene) = current.as_ref().map(|state| *state.get()) else {
        return;
    };
    let Some(map) = map_for_scene(scene, active_dynamic.as_deref()) else {
        return;
    };
    let token = scenes.ensure(map.clone(), scene);
    if scenes.transfer.is_none() {
        scenes.active = Some(map);
        activity.active = Some(token);
    }
    commands
        .entity(insert.entity)
        .insert(ResidentMapEntity(token));
}

pub(super) fn register_owner(
    insert: On<Insert, ResidentMapEntity>,
    owners: Query<&ResidentMapEntity, Allow<Disabled>>,
    mut index: ResMut<ResidentEntityIndex>,
    activity: Option<Res<SceneResidencyActivity>>,
    mut commands: Commands,
) {
    let Ok(owner) = owners.get(insert.entity) else {
        return;
    };
    index.insert(insert.entity, owner.0);
    let owned = insert.entity;
    commands.queue(move |world: &mut World| super::assets::register_owned_assets(world, owned));
    if activity
        .as_ref()
        .is_some_and(|state| state.enabled && (state.switching || state.active != Some(owner.0)))
    {
        let entity = insert.entity;
        commands.queue(move |world: &mut World| park_entity(world, entity));
    }
}

pub(super) fn unregister_owner(
    remove: On<Remove, ResidentMapEntity>,
    mut index: ResMut<ResidentEntityIndex>,
    mut assets: ResMut<super::assets::AssetOwners>,
) {
    index.remove(remove.entity);
    assets.remove(remove.entity);
}

pub(super) fn inherit_parent_owner(
    insert: On<Insert, ChildOf>,
    children: Query<&ChildOf, Allow<Disabled>>,
    owners: Query<&ResidentMapEntity, Allow<Disabled>>,
    mut commands: Commands,
) {
    let Ok(parent) = children.get(insert.entity) else {
        return;
    };
    if let Ok(owner) = owners.get(parent.parent()) {
        commands.entity(insert.entity).insert(*owner);
    }
}

/// SceneRoot可能在根归属登记前已生成子节点；只补未登记关系，单帧最多256次索引访问。
pub(super) fn enroll_existing_descendants(world: &mut World) {
    for _ in 0..ENTITY_BUDGET {
        let Some(mut cursor) = world
            .resource_mut::<ResidentEntityIndex>()
            .descendants
            .pop_front()
        else {
            break;
        };
        let owner = world.get::<ResidentMapEntity>(cursor.parent).copied();
        let child = world
            .get::<Children>(cursor.parent)
            .and_then(|children| children.get(cursor.next))
            .copied();
        if let (Some(owner), Some(child)) = (owner, child) {
            cursor.next += 1;
            world
                .resource_mut::<ResidentEntityIndex>()
                .descendants
                .push_front(cursor);
            if world.get::<ResidentMapEntity>(child) != Some(&owner) {
                if let Ok(mut entity) = world.get_entity_mut(child) {
                    entity.insert(owner);
                }
            }
        }
    }
}

pub(super) fn park_entity(world: &mut World, entity: Entity) {
    if world.get::<ParkedSceneEntity>(entity).is_some() {
        return;
    }
    let Ok(entity_ref) = world.get_entity(entity) else {
        return;
    };
    let parked = ParkedSceneEntity {
        disabled_by_us: !entity_ref.contains::<Disabled>(),
        visibility: entity_ref.get::<Visibility>().copied(),
        camera_active: entity_ref.get::<Camera>().map(|camera| camera.is_active),
        body_disabled_by_us: entity_ref.contains::<RigidBody>()
            && !entity_ref.contains::<RigidBodyDisabled>(),
        collider_disabled_by_us: entity_ref.contains::<Collider>()
            && !entity_ref.contains::<ColliderDisabled>(),
        audio_playing: entity_ref
            .get::<AudioSink>()
            .is_some_and(|sink| !sink.is_paused()),
        spatial_audio_playing: entity_ref
            .get::<SpatialAudioSink>()
            .is_some_and(|sink| !sink.is_paused()),
    };
    let entity_ref = world.entity_mut(entity);
    if let Some(active) = parked.camera_active {
        if let Some(owner) = entity_ref.get::<ResidentMapEntity>().copied() {
            world
                .resource_mut::<ResidentEntityIndex>()
                .cameras
                .entry(owner.0)
                .or_default()
                .insert(entity, active);
        }
    }
    let mut entity_ref = world.entity_mut(entity);
    if parked.body_disabled_by_us {
        entity_ref.insert(RigidBodyDisabled);
    }
    if parked.collider_disabled_by_us {
        entity_ref.insert(ColliderDisabled);
    }
    if parked.audio_playing {
        entity_ref.get::<AudioSink>().unwrap().pause();
    }
    if parked.spatial_audio_playing {
        entity_ref.get::<SpatialAudioSink>().unwrap().pause();
    }
    if let Some(mut visibility) = entity_ref.get_mut::<Visibility>() {
        *visibility = Visibility::Hidden;
    }
    if let Some(mut camera) = entity_ref.get_mut::<Camera>() {
        camera.is_active = false;
    }
    // DespawnOnExit显式允许Disabled对象；回收责任转交驻留索引，不能因同枚举切图误删。
    entity_ref.remove::<DespawnOnExit<ScenePhase>>();
    entity_ref.insert((Disabled, parked));
    world
        .resource_mut::<ResidentEntityIndex>()
        .mark_parked(entity, true);
}

pub(super) fn restore_entity(world: &mut World, entity: Entity) {
    let Ok(mut entity_ref) = world.get_entity_mut(entity) else {
        return;
    };
    let Some(parked) = entity_ref.take::<ParkedSceneEntity>() else {
        return;
    };
    if parked.body_disabled_by_us {
        entity_ref.remove::<RigidBodyDisabled>();
    }
    if parked.collider_disabled_by_us {
        entity_ref.remove::<ColliderDisabled>();
    }
    if parked.disabled_by_us {
        entity_ref.remove::<Disabled>();
    }
    if parked.audio_playing || parked.spatial_audio_playing {
        entity_ref.insert(ResumeResidentAudio {
            audio: parked.audio_playing,
            spatial: parked.spatial_audio_playing,
        });
    }
    if let Some(visibility) = parked.visibility {
        entity_ref.insert(visibility);
    }
    // 相机保持关闭，直到整图恢复完成；避免分帧期间渲染半份场景。
    // 恢复后的实体由驻留所有权回收，不再重新加入同枚举的DespawnOnExit，防止另一动态地图离开时误删。
    world
        .resource_mut::<ResidentEntityIndex>()
        .mark_parked(entity, false);
}

pub(super) fn resume_adopted_audio(
    activity: Res<SceneResidencyActivity>,
    pending: Query<(
        Entity,
        &ResidentMapEntity,
        &ResumeResidentAudio,
        Option<&AudioSink>,
        Option<&SpatialAudioSink>,
    )>,
    mut commands: Commands,
) {
    if activity.switching {
        return;
    }
    for (entity, owner, resume, sink, spatial) in pending.iter().take(ENTITY_BUDGET) {
        if activity.active != Some(owner.0) {
            continue;
        }
        if resume.audio
            && let Some(sink) = sink
        {
            sink.play();
        }
        if resume.spatial
            && let Some(sink) = spatial
        {
            sink.play();
        }
        commands.entity(entity).remove::<ResumeResidentAudio>();
    }
}

pub(super) fn pause_added_body(
    insert: On<Insert, RigidBody>,
    mut parked: Query<&mut ParkedSceneEntity, Allow<Disabled>>,
    disabled: Query<(), (With<RigidBodyDisabled>, Allow<Disabled>)>,
    mut commands: Commands,
) {
    if let Ok(mut state) = parked.get_mut(insert.entity)
        && !disabled.contains(insert.entity)
    {
        state.body_disabled_by_us = true;
        commands.entity(insert.entity).insert(RigidBodyDisabled);
    }
}

pub(super) fn pause_added_collider(
    insert: On<Insert, Collider>,
    mut parked: Query<&mut ParkedSceneEntity, Allow<Disabled>>,
    disabled: Query<(), (With<ColliderDisabled>, Allow<Disabled>)>,
    mut commands: Commands,
) {
    if let Ok(mut state) = parked.get_mut(insert.entity)
        && !disabled.contains(insert.entity)
    {
        state.collider_disabled_by_us = true;
        commands.entity(insert.entity).insert(ColliderDisabled);
    }
}
