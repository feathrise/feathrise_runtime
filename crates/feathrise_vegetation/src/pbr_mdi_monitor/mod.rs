//! Bevy 0.19 PBR 多绘制分箱的项目自有只读排队镜像。
//! 【功能设计】只在调用方授权诊断时重建当前可见 MDI 实体，不修改官方阶段、绘制命令或 GPU 资源。

mod helpers; // 把公开普通分箱与镜像MDI实体合并为旧版一致的提交统计。
mod systems; // 复现官方PBR主画面和阴影排队门，仅记录实际存在的MDI批集。
mod types; // 三阶段镜像条目和跨crate共享的紧凑统计值。

pub use systems::PbrMdiQueueMirrorParams; // 消费监控系统通过SystemParam取得官方排队所需只读资源。
pub use types::{PbrMdiQueueMirror, PbrPhaseStats}; // 树木与通用Mesh监控复用同一统计口径。
