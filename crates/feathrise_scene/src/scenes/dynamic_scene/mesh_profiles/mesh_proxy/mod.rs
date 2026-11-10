//! 数据预设驱动的景观超低模代理生成。
//! 【功能设计】后台原型任务读取已加载的最低档 CPU Mesh 一次；灌木保留分散叶片三角形，
//! 岩石从均匀采样点生成封闭凸包。结果只登记到当前场景会话的 `Assets<Mesh>`，不落盘、不逐帧重建。

// 灌木重索引和岩石输入点去重都只发生在加载阶段。
// Mesh、Vec2和Vec3提供代理构建所需基础类型。
mod attributes; // 统一读取源Mesh属性，并生成两类代理共享的边界、法线和切线。
mod rock; // 岩石使用分散采样点生成封闭QuickHull超低模。
mod shrub; // 灌木保留空间分散的真实叶片三角形和源材质属性。

pub(in crate::scenes::dynamic_scene::mesh_profiles) use rock::build_rock_ultra_proxy; // 岩石预设统一使用封闭凸包算法。
pub(in crate::scenes::dynamic_scene::mesh_profiles) use shrub::build_shrub_ultra_proxy; // 灌木预设统一保留真实叶片薄片。

#[cfg(test)] // 纯Mesh测试不启动窗口、AssetServer或RenderApp。
mod tests; // 测试独立文件锁定两类动态代理的既有输出契约。
