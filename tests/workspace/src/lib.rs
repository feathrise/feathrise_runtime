//! 仅在完整 SDK 仓库执行的跨包契约；直接核对真实生产 WGSL，不复制算法或扩大生产公开接口。
//! 普通子包测试只读取自己归档内的资源；本测试包 publish=false，复用 SDK 既有构建缓存。

#[cfg(test)]
mod vegetation;
#[cfg(test)]
mod water;
#[cfg(test)]
mod terrain;
#[cfg(test)]
mod readiness_gpu;
#[cfg(test)]
#[path = "../../../crates/feathrise_environment/src/terrain_surface/wet_surface_test_support.rs"]
mod terrain_support; // 只复用测试预处理与宿主类型桩；正式地表源码仍由其所有者提供。
