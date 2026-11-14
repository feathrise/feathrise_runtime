//! 实窗验收的分页高度单飞IO；主帧仅冻结小身份、非阻塞轮询并采用完整有效指纹。

mod resources;
mod types;
use resources::SourceState;
use types::Identity;
use bevy::{prelude::*, ecs::system::RunSystemOnce, tasks::{IoTaskPool, futures::check_ready}};
use feathrise_scene::TerrainObjectQueries;
use feathrise_scene_content::{SceneTerrainKind, TerrainId};

pub(super) fn install(app: &mut App) { app.init_resource::<SourceState>(); }

fn identity(world: &mut World) -> Option<Identity> {
    let scene = world.resource::<crate::project::startup::SavedGameStartup>().scene_id.clone();
    let terrain = TerrainId::parse("starter-ground").ok()?;
    world.run_system_once(move |query: TerrainObjectQueries| {
        let (instance, object, _) = query.instance_source(&scene, &terrain)?;
        (object.terrain.kind == SceneTerrainKind::SparseClipmap && query.collision_is_ready(&scene, &terrain)).then(|| Identity {
            scene: scene.clone(), terrain: terrain.clone(), instance, path: object.terrain.height_path.clone().unwrap_or_default(),
        })
    }).ok().flatten()
}

pub(super) fn ready(world: &mut World) -> bool {
    let Some(desired) = identity(world) else { return false; };
    let root = world.resource::<crate::project::ProjectContext>().asset_root().to_owned();
    let mut state = world.resource_mut::<SourceState>();
    if let Some(result) = state.task.as_mut().and_then(|(_, task)| check_ready(task)) {
        let (identity, _) = state.task.take().unwrap();
        if identity == desired { state.completed = Some((identity, result)); }
    }
    if state.completed.as_ref().is_some_and(|(identity, _)| identity == &desired) { return true; }
    state.completed = None;
    if state.task.is_none() {
        let input = desired.clone();
        let task = IoTaskPool::get().spawn(async move {
            crate::project::validation_height_fingerprint(&root, &input.scene, &input.terrain, &input.path)
        });
        state.task = Some((desired, task));
    }
    false
}

pub(super) fn fingerprint(world: &mut World) -> Result<(String, String), String> {
    let desired = identity(world).ok_or("高度验收活动分页实例已换代")?;
    let completed = world.resource::<SourceState>().completed.as_ref()
        .filter(|(identity, _)| *identity == desired).ok_or("高度验收分页指纹尚未就绪")?;
    Ok((desired.path, completed.1.clone()?))
}
