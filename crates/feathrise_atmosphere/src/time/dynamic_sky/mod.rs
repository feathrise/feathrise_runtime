//! 动态大气天空：接管玩法与 F12 透视相机，并保留离场时精确恢复作者组件的所有权边界。

mod components; // 相机原状态快照与全局大气实体身份。
mod render_compatibility; // 在Bevy 0.19.1提取阶段清理已经失去资格的旧大气渲染状态。
mod systems; // Earth 大气单例、相机资格与作者天空盒之间的互斥让权逻辑。

pub(super) use render_compatibility::install_atmosphere_render_state_cleanup; // 插件启动时安装动态天空与离线天空盒的RenderWorld交接保护。
pub(super) use systems::sync_dynamic_sky; // WorldTimePlugin 在最终光照后同步相机大气组件。

#[cfg(test)]
mod tests; // 最小 ECS 回归验证玩法、F12 正交隔离、恢复与全局单例。
