//! 从当前工程异步建立演出目录。普通游戏、作者预览和离线摄影各自只拥有自己的播放游标。
mod capture;
mod playback;
use bevy::{
    prelude::*,
    tasks::{IoTaskPool, Task, futures::check_ready},
};
use feathrise_cinematic::{
    CinematicCatalog, CinematicFrame, CinematicWorldState, movie::MOVIE_JOB_ENV,
};
use std::path::PathBuf;

#[derive(Resource)]
pub struct CatalogSource(Box<dyn FnOnce() -> Result<CinematicCatalog, String> + Send + Sync>);
impl CatalogSource {
    pub fn new(
        load: impl FnOnce() -> Result<CinematicCatalog, String> + Send + Sync + 'static,
    ) -> Self {
        Self(Box::new(load))
    }
    pub fn directory(root: PathBuf) -> Self {
        Self::new(move || CinematicCatalog::load(&root))
    }
}
#[derive(Resource)]
struct Loading(Task<Result<CinematicCatalog, String>>);

pub struct CinematicRuntimePlugin;
impl Plugin for CinematicRuntimePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CinematicFrame>()
            .init_resource::<CinematicWorldState>()
            .init_resource::<CinematicCatalog>()
            .add_systems(Startup, start_catalog)
            .add_systems(
                PreUpdate,
                receive_catalog.run_if(resource_exists::<Loading>),
            );
        if let Some(path) = std::env::var_os(MOVIE_JOB_ENV) {
            capture::install(app, PathBuf::from(path));
        } else {
            playback::install(app);
        }
    }
}
fn start_catalog(world: &mut World) {
    if let Some(source) = world.remove_resource::<CatalogSource>() {
        world.insert_resource(Loading(
            IoTaskPool::get().spawn(async move { (source.0)() }),
        ));
    } else {
        world.resource_mut::<CinematicCatalog>().ready = true;
    }
}
fn receive_catalog(
    mut commands: Commands,
    mut task: ResMut<Loading>,
    mut catalog: ResMut<CinematicCatalog>,
) {
    if let Some(result) = check_ready(&mut task.0) {
        match result {
            Ok(value) => *catalog = value,
            Err(error) => {
                error!("剧情演出目录加载失败：{error}");
                catalog.error = Some(error);
                catalog.ready = true;
            }
        }
        commands.remove_resource::<Loading>();
    }
}
