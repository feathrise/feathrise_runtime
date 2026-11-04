// 时钟功能域：组织日期算法、公开时钟方法与自动推进系统。

mod helpers; // 日期规范化、WorldClock 方法和存档恢复逻辑。
mod systems; // Bevy自动推进系统。

pub(super) use systems::advance_world_clock; // 插件层只注册系统，不暴露内部实现。
