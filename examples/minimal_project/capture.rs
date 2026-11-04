//! 示例的显式有限截图模式；普通交互运行不安装截图或自动退出系统。

use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use std::{fs::OpenOptions, path::PathBuf};

#[derive(Resource)]
struct Capture {
    path: PathBuf,
    requested: bool,
}

pub(super) fn parse_arguments() -> Result<Option<PathBuf>, String> {
    let mut args = std::env::args_os().skip(1);
    match args.next() {
        None => Ok(None),
        Some(flag) if flag == "--capture" => {
            let path = args.next().ok_or("--capture 后需要 PNG 输出路径")?;
            if args.next().is_some() {
                return Err(
                    "用法：cargo run --example minimal_project -- [--capture 输出.png]".into(),
                );
            }
            // run 会切换到工程目录，输出路径提前绑定调用者的 cwd；不往示例资产目录写验收文件。
            let path = std::path::absolute(path).map_err(|error| error.to_string())?;
            if path.extension().is_none_or(|extension| extension != "png")
                || !path.parent().is_some_and(|parent| parent.is_dir())
            {
                return Err("截图需要已有目录内的 .png 文件路径".into());
            }
            if path.exists() {
                return Err("截图文件已存在，请指定新的文件名，避免覆盖已有结果".into());
            }
            Ok(Some(path))
        }
        Some(_) => Err("用法：cargo run --example minimal_project -- [--capture 输出.png]".into()),
    }
}

pub(super) fn install(app: &mut App, path: PathBuf) {
    app.insert_resource(Capture {
        path,
        requested: false,
    })
    .add_systems(Update, request_capture);
}

fn request_capture(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut capture: ResMut<Capture>,
    mut frames: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    *frames = frames.saturating_add(1);
    if time.elapsed_secs() >= 45.0 {
        error!("示例截图超时；未取得实际 GPU 回读，不算验收通过");
        exit.write(AppExit::error());
        return;
    }
    // 固定小样本预热后只请求一次；实际截图事件到达后才正常退出。
    if !capture.requested && time.elapsed_secs() >= 3.0 && *frames >= 12 {
        capture.requested = true;
        commands
            .spawn(Screenshot::primary_window())
            .observe(finish_capture);
    }
}

fn finish_capture(
    event: On<ScreenshotCaptured>,
    capture: Res<Capture>,
    mut exit: MessageWriter<AppExit>,
) {
    // 回读完成不等于文件保存成功；单个观察者检查编码与写入结果，再决定退出状态。
    let save = || -> Result<(), String> {
        let image = event
            .image
            .clone()
            .try_into_dynamic()
            .map_err(|error| error.to_string())?
            .to_rgb8(); // HDR 回读的 alpha 可储存亮度，和 Bevy 默认截图一样只保存 RGB。
        let format = bevy::image::ImageFormat::Png
            .as_image_crate_format()
            .ok_or("当前构建不支持 PNG 编码")?;
        // 再用原子 create_new 拒绝覆盖，防止启动后同名文件被其他任务创建。
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&capture.path)
            .map_err(|error| error.to_string())?;
        image
            .write_to(&mut file, format)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())
    };
    match save() {
        Ok(()) => {
            info!(
                width = event.image.width(),
                height = event.image.height(),
                "FEATHRISE_MINIMAL_CAPTURED"
            );
            exit.write(AppExit::Success);
        }
        Err(error) => {
            error!("示例截图保存失败：{error}");
            exit.write(AppExit::error());
        }
    }
}
