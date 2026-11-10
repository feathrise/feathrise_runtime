//! 碰撞系统注册边界；具体驱动、生命周期与发布各自独立。
pub(crate) use super::driver::drive_layered_collision; // 唯一动态场景碰撞驱动。
pub(crate) use super::lifecycle::{clear_layered_collision, reap_layered_collision}; // 离场和后台回收保留原注册接口。
