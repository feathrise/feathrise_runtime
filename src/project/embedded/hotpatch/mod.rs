//! 显式函数体补丁入口；复用工程控制管道，不连接dx的自动装载服务。

mod resources;
#[cfg(all(debug_assertions, windows))]
mod package;
#[cfg(all(debug_assertions, windows))]
mod systems;
#[cfg(all(debug_assertions, windows))]
mod types;
#[cfg(all(debug_assertions, windows))]
mod worker;

pub use resources::FunctionHotpatchState;
#[cfg(all(debug_assertions, windows))]
pub(super) use systems::{busy, prepare, receive};

pub(super) fn install(app: &mut bevy::prelude::App) {
    #[cfg(all(debug_assertions, windows))]
    if app.world().contains_non_send::<resources::active::Runtime>() {
        // 在Update执行器检查热补丁标记之前发布通知，确保实际调用从新跳表刷新。
        app.add_systems(bevy::prelude::First, systems::advance);
    }
}

pub(super) fn stop(world: &bevy::prelude::World) {
    #[cfg(all(debug_assertions, windows))]
    if let Some(runtime) = world.get_non_send::<resources::active::Runtime>() {
        runtime.stop.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(not(all(debug_assertions, windows)))]
pub(super) fn prepare(_: &mut bevy::prelude::App, _: &super::Configuration, _: &str) -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(all(debug_assertions, windows)))]
pub(super) fn busy(_: &bevy::prelude::World) -> bool { false }

#[cfg(not(all(debug_assertions, windows)))]
pub(super) fn receive(world: &mut bevy::prelude::World, runtime: &mut super::resources::Runtime,
    patch_id: u64, source_generation: u64, base_build: String, _: String, _: String) {
    // 可选能力未开放时以独立失败回执拒绝，不能把普通补丁拒绝冒充视口断链。
    let _ = runtime.transport.send(feathrise_editor_protocol::Message::CodePatchResult {
        patch_id, source_generation, base_build, pid: std::process::id(),
        world_id: world.resource::<super::EmbeddedProjectState>().world_id,
        epoch: 0, applied: false, reason: Some("当前构建未开放函数体热更新".into()),
    });
}
