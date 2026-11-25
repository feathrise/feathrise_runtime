//! 将导演已经呈现的主相机送往指定像素尺寸的离屏颜色目标，用 GPU 回执防止首帧拍到未就绪管线。
use super::*;
use bevy::{
    camera::RenderTarget,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
        render_resource::{PipelineCache, TextureFormat, TextureUsages},
        renderer::RenderDevice,
        view::screenshot::{Screenshot, ScreenshotCaptured},
    },
};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Component)]
struct MovieCaption;
#[derive(Resource, Default, Clone)]
struct CameraExposure {
    request: u64,
    rendered: Arc<AtomicU64>,
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<CameraExposure>()
        .add_systems(PostUpdate, hide_game_ui.before(bevy::ui::UiSystems::Layout));
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<CameraExposure>()
            .add_systems(ExtractSchedule, extract)
            .add_systems(Render, confirm.in_set(RenderSystems::Cleanup));
    }
}
fn extract(mut commands: Commands, request: Extract<Res<CameraExposure>>) {
    commands.insert_resource((**request).clone());
}
fn confirm(request: Res<CameraExposure>, cache: Res<PipelineCache>, mut previous: Local<u64>) {
    // 第一次 draw 可登记新的管线，只有同一请求的下一次 Render 仍无等待项才允许拍照。
    if request.request != 0
        && cache.pipelines().next().is_some()
        && cache.waiting_pipelines().next().is_none()
    {
        if *previous == request.request {
            request.rendered.store(request.request, Ordering::Release)
        }
        *previous = request.request;
    } else {
        *previous = 0
    }
}
fn hide_game_ui(
    mut roots: Query<&mut Node, (Without<ChildOf>, Without<MovieCaption>, Changed<Node>)>,
) {
    for mut node in &mut roots {
        if node.display != Display::None {
            node.display = Display::None
        }
    }
}
pub(super) fn shoot(world: &mut World, c: &mut Capture) -> Result<(), String> {
    if !c.issued || c.readback {
        return Ok(());
    }
    let sample = c.timeline.as_ref().unwrap().frame(c.frame).unwrap();
    let presentation = world.resource::<CinematicPresentation>();
    if let Some(error) = presentation.error(1, sample.time) {
        return Err(error.to_owned());
    }
    if !presentation.ready(1, sample.time) {
        return Ok(());
    }
    let Some(camera) = presentation.camera else {
        return Err("当前场景没有可摄影的主相机".into());
    };
    if c.target.is_none() {
        let settings = &c.job.settings;
        let max = world
            .get_resource::<RenderDevice>()
            .ok_or("缺少 GPU 渲染设备")?
            .limits()
            .max_texture_dimension_2d;
        if settings.width.max(settings.height) > max {
            return Err(format!("当前 GPU 单边纹理尺寸上限为 {max}"));
        }
        let mut texture = Image::new_target_texture(
            settings.width,
            settings.height,
            TextureFormat::Bgra8UnormSrgb,
            None,
        );
        texture.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        let handle = world.resource_mut::<Assets<Image>>().add(texture);
        world
            .entity_mut(camera)
            .insert(RenderTarget::Image(handle.clone().into()));
        world
            .get_mut::<Camera>(camera)
            .ok_or("主相机无 Camera")?
            .viewport = None;
        let entity = world
            .spawn((
                MovieCaption,
                UiTargetCamera(camera),
                Text::new(sample.subtitles.join("\n")),
                TextFont {
                    font_size: (settings.height as f32 * 0.044).into(),
                    ..default()
                },
                TextColor(Color::WHITE),
                TextShadow::default(),
                TextLayout::new(Justify::Center, bevy::text::LineBreak::WordBoundary),
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(6.),
                    width: percent(88.),
                    bottom: percent(6.),
                    ..default()
                },
                GlobalZIndex(100),
            ))
            .id();
        c.target = Some((camera, handle, entity));
    }
    let (current, handle, _) = c.target.as_ref().unwrap();
    if *current != camera {
        return Err("拍摄期间场景替换了主相机".into());
    }
    let stamp = c.frame + 1;
    let mut exposure = world.resource_mut::<CameraExposure>();
    exposure.request = stamp;
    if exposure.rendered.load(Ordering::Acquire) != stamp {
        return Ok(());
    }
    c.readback = true;
    world.spawn(Screenshot::image(handle.clone())).observe(save);
    Ok(())
}
fn save(mut event: On<ScreenshotCaptured>, mut c: ResMut<Capture>) {
    if !c.readback || c.writing.is_some() {
        return;
    }
    let path = c
        .dir
        .join("frames")
        .join(format!("frame-{:08}.png", c.frame));
    // 此观察者只接收本次摄影拥有的读回，将像素缓冲区直接移交后台，主线程不复制整帧。
    let image = std::mem::take(&mut event.event_mut().image);
    let (width, height) = (c.job.settings.width, c.job.settings.height);
    c.writing = Some(bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
        let rgb = image
            .try_into_dynamic()
            .map_err(|e| e.to_string())?
            .to_rgb8();
        if rgb.dimensions() != (width, height) {
            return Err(format!(
                "GPU 实际读回尺寸 {:?} 与请求不符",
                rgb.dimensions()
            ));
        }
        let mut file = std::io::BufWriter::new(
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|e| e.to_string())?,
        );
        let png = bevy::image::ImageFormat::Png
            .as_image_crate_format()
            .ok_or("未安装 PNG 编码器")?;
        rgb.write_to(&mut file, png).map_err(|e| e.to_string())?;
        std::io::Write::flush(&mut file).map_err(|e| e.to_string())
    }));
}
