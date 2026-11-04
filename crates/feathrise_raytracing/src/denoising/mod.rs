//! NVIDIA DLSS Ray Reconstruction 的能力门、目标相机接管、历史同步与精确回退功能域。

mod di_stability; // 固化已验Solari DI同帧spatial reservoir稳定策略。
mod gi_stability; // 编译期覆盖固化全屏GI temporal稳定策略，不改变DI、cache或RR历史。
mod shadow_terminator; // 动态低模地形保留平滑受光法线，并为主射线建立虚拟平滑面起点。
mod systems; // 固定O(1)系统分别维护挂接、回退与正常reset同步。

pub(crate) use di_stability::install_di_stability_shader; // 只在插件build期替换DI root，不新增逐帧CPU系统。
pub(crate) use gi_stability::install_gi_stability_shader; // shader覆盖只在插件build期发生，不写Solari或RR reset。
pub(crate) use shadow_terminator::install_dynamic_terrain_shadow_terminator_shaders; // 一份独立库与一份root覆盖只在插件build期注册。
pub(crate) use systems::{
    maintain_dlss_ray_reconstruction, restore_dlss_ray_reconstruction_on_rollback,
    sync_dlss_antialiasing_requirement, sync_dlss_ray_reconstruction_reset,
}; // 生命周期只通过固定系统入口协调厂商降噪。

#[cfg(all(test, feature = "dlss_rr"))]
// 本机DLSS/Vulkan/clang SDK就绪后验证组件所有权，测试本身不初始化GPU。
mod tests; // 降噪域的能力门、重置和精确回退回归。
