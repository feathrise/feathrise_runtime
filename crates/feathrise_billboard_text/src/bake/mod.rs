// bake/mod.rs — 固定离屏相机池功能域入口。

mod helpers; // 纯状态推进和空闲槽位选择逻辑。
mod systems; // 相机池创建、任务分配、布局同步和槽位回收系统。

pub use systems::sync_billboard_layouts;
pub(crate) use systems::{
    // 只向crate插件层暴露需要注册的系统。
    assign_billboard_bake_jobs,  // 将FIFO等待任务分配给空闲固定相机。
    finish_billboard_bakes,      // 最终纹理稳定后释放2D源并归还槽位。
    release_orphaned_bake_slots, // PostUpdate兜底回收业务层中途删除的活跃任务。
    setup_billboard_bake_pool,   // Startup阶段创建固定数量Camera2d。
}; // 保留原有公开系统API，避免调用方路径失效。
