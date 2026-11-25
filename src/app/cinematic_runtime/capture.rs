//! 独立游戏进程拍摄真实主相机。GPU 回读和后台 PNG 写盘都确认后才提交下一个整数帧。
mod image;
use super::super::game_plugins::cinematics::{CinematicPresentation, CinematicPresentationSet};
use crate::project::ProjectContext;
use bevy::{
    prelude::*,
    tasks::{IoTaskPool, Task, futures::check_ready},
    time::TimeUpdateStrategy,
};
use feathrise_cinematic::{
    Cinematic, CinematicFrame, FrameData, FrameMode, MovieDialogue, MovieYarnProvider, movie::*,
};
use feathrise_scene_core::state::{LoadingProgress, LoadingStage, ScenePhase, SceneStartupGate};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Resource)]
struct Capture {
    dir: PathBuf,
    job: MovieJob,
    doc: Arc<Cinematic>,
    timeline: Option<MovieTimeline>,
    frame: u64,
    target: Option<(Entity, Handle<Image>, Entity)>,
    writing: Option<Task<Result<(), String>>>,
    readback: bool,
    issued: bool,
}
#[derive(Resource)]
struct Source(Task<Result<(MovieJob, Arc<Cinematic>), String>>, PathBuf);
#[derive(Resource)]
struct JobPath(PathBuf);

pub(super) fn install(app: &mut App, path: PathBuf) {
    println!("FEATHRISE_MOVIE_PROTOCOL={PROTOCOL}");
    app.insert_resource(JobPath(path))
        .add_systems(Startup, open)
        .add_systems(PreUpdate, loaded.run_if(resource_exists::<Source>))
        .add_systems(Update, prepare_frame.run_if(resource_exists::<Capture>))
        .add_systems(
            PostUpdate,
            photograph
                .after(CinematicPresentationSet)
                .after(TransformSystems::Propagate)
                .run_if(resource_exists::<Capture>),
        );
    image::install(app);
}
fn open(mut commands: Commands, path: Res<JobPath>, project: Option<Res<ProjectContext>>) {
    let file = path.0.clone();
    let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    let project = project.map(|p| p.clone());
    commands.insert_resource(Source(
        IoTaskPool::get().spawn(async move {
            let owner = file.parent().ok_or("摄影任务没有目录")?;
            if file.file_name().and_then(|n| n.to_str()) != Some("job.json")
                || !owner.join("frames").is_dir()
            {
                return Err("无效的摄影任务目录".into());
            }
            fs::write(owner.join("started"), PROTOCOL.to_string()).map_err(|e| e.to_string())?;
            let job: MovieJob =
                serde_json::from_slice(&fs::read(&file).map_err(|e| e.to_string())?)
                    .map_err(|e| format!("摄影任务：{e}"))?;
            let project = project.ok_or("摄影需要工程上下文")?;
            if job.version != PROTOCOL || job.project_id != project.id {
                return Err("摄影协议或工程身份不匹配".into());
            }
            job.settings.validate()?;
            let root = project.asset_roots.first().ok_or("工程无主资产根")?;
            let path = crate::project::checked_existing(root, &job.asset, false)?;
            let bytes = fs::read(path).map_err(|e| e.to_string())?;
            if Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                != job.asset_sha256
            {
                return Err("演出在启动摄影后发生变化，请重新导出".into());
            }
            let doc = Cinematic::from_slice(&bytes)?;
            if doc.id != job.cinematic {
                return Err("演出稳定 ID 已变化".into());
            }
            Ok((job, Arc::new(doc)))
        }),
        dir,
    ));
}
fn finish(world: &mut World, dir: &Path, id: &str, frames: u64, error: Option<String>) {
    let failed = error.is_some();
    let receipt = MovieReceipt {
        choice: world
            .get_resource_mut::<MovieDialogue>()
            .and_then(|mut dialogue| dialogue.choice.take()),
        version: PROTOCOL,
        cinematic: id.into(),
        frames,
        error,
    };
    let result = serde_json::to_vec(&receipt)
        .map_err(|e| e.to_string())
        .and_then(|bytes| fs::write(dir.join("result.json"), bytes).map_err(|e| e.to_string()));
    if let Some(error) = receipt.error {
        error!("摄影失败：{error}")
    }
    if let Err(error) = &result {
        error!("摄影回执失败：{error}")
    }
    world.remove_resource::<Capture>();
    world.write_message(if failed || result.is_err() {
        AppExit::error()
    } else {
        AppExit::Success
    });
}
fn loaded(world: &mut World) {
    let mut src = world.resource_mut::<Source>();
    let Some(result) = check_ready(&mut src.0) else {
        return;
    };
    let dir = world.remove_resource::<Source>().unwrap().1;
    match result {
        Err(error) => finish(world, &dir, "", 0, Some(error)),
        Ok((job, doc)) => {
            let yarn = doc.dialogue.iter().any(|c| c.yarn.is_some());
            if yarn && !world.contains_resource::<MovieYarnProvider>() {
                finish(
                    world,
                    &dir,
                    &doc.id,
                    0,
                    Some("本工程尚未安装 Yarn 导出适配".into()),
                );
                return;
            }
            world.insert_resource(MovieDialogue {
                choice: None,
                document: doc.clone(),
                choices: job.settings.choices.clone(),
                lines: (!yarn).then(|| Ok(BTreeMap::new())),
            });
            world.insert_resource(Capture {
                dir,
                job,
                doc,
                timeline: None,
                frame: 0,
                target: None,
                writing: None,
                readback: false,
                issued: false,
            });
        }
    }
}
fn prepare_frame(world: &mut World) {
    // 资源从 World 临时取出，成功才交回。错误收口始终带真实写完的帧数。
    let mut c = world.remove_resource::<Capture>().unwrap();
    match prepare(world, &mut c) {
        Ok(true) => finish(world, &c.dir, &c.doc.id, c.frame, None),
        Err(error) => finish(world, &c.dir, &c.doc.id, c.frame, Some(error)),
        Ok(false) => world.insert_resource(c),
    }
}
fn prepare(world: &mut World, c: &mut Capture) -> Result<bool, String> {
    if let Some(progress) = world.get_resource::<LoadingProgress>()
        && progress.stage == LoadingStage::Failed
    {
        return Err(format!("摄影场景加载失败：{}", progress.label));
    }
    if c.issued {
        world.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    }
    if let Some(task) = &mut c.writing {
        let Some(done) = check_ready(task) else {
            return Ok(false);
        };
        done?;
        c.writing = None;
        c.readback = false;
        c.issued = false;
        c.frame += 1;
        if c.frame % u64::from(c.job.settings.fps) == 0
            && let Some(timeline) = &c.timeline
        {
            println!("FEATHRISE_MOVIE_FRAMES {}/{}", c.frame, timeline.frames);
        }
    }
    if c.timeline.is_none() {
        let Some(lines) = world.resource_mut::<MovieDialogue>().lines.take() else {
            return Ok(false);
        };
        c.timeline = Some(MovieTimeline::new(&c.doc, &c.job.settings, &lines?)?);
    }
    let timeline = c.timeline.as_ref().unwrap();
    if c.frame >= timeline.frames {
        return Ok(true);
    }
    // Active 允许工程异步实例化演员和摆放。打开正式的场景启动门后，才可把缺失绑定判为错误。
    if crate::project::has_saved_scene(world)
        && let Some(gate) = world.get_resource::<SceneStartupGate>()
    {
        if let Some(reason) = gate
            .application_error
            .as_ref()
            .or(gate.subscenes_error.as_ref())
        {
            return Err(reason.clone());
        }
        if !gate.ready {
            return Ok(false);
        }
    }
    if c.issued
        || (crate::project::has_saved_scene(world)
            && world
                .get_resource::<State<ScenePhase>>()
                .is_some_and(|s| s.get() != &ScenePhase::Active))
    {
        return Ok(false);
    }
    let f = timeline.frame(c.frame).unwrap();
    let previous = world
        .resource::<CinematicFrame>()
        .active
        .as_ref()
        .map(|f| f.time);
    // 在读回和写盘的等待帧中保持模拟时钟静止；到下一时间点只推进两次采样间的差值。
    let step = previous.map_or(0., |t| (f.time - t).max(0.));
    world.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
        step,
    )));
    world.resource_mut::<CinematicFrame>().active = Some(FrameData {
        document: c.doc.clone(),
        time: f.time,
        ticket: 1,
        instance: c.job.settings.instance.clone(),
        mode: FrameMode::Export,
    });
    if let Some((_, _, entity)) = &c.target {
        world.get_mut::<Text>(*entity).ok_or("摄影字幕已移除")?.0 = f.subtitles.join("\n");
    }
    c.issued = true;
    Ok(false)
}
fn photograph(world: &mut World) {
    let mut c = world.remove_resource::<Capture>().unwrap();
    let result = image::shoot(world, &mut c);
    if let Err(error) = result {
        finish(world, &c.dir, &c.doc.id, c.frame, Some(error))
    } else {
        world.insert_resource(c)
    }
}
