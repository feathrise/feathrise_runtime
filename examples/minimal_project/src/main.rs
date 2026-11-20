//! 通过 SDK 的 run 入口启动最小工程，由 SDK 装配 Bevy 和工程运行时。

#[path = "../../support/mod.rs"]
mod demo;

fn main() -> Result<(), String> {
    let (project, game) = demo::load()?;
    feathrise_runtime::run(project, game)
}
