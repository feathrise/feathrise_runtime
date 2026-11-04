//! GPU竞争输入固定模式和预算ABI回归。

use super::*; // 私有常量与uniform只在功能域内部接受结构回归。

#[test]
fn competition_modes_and_group_budget_remain_stable() {
    assert_eq!(GPU_COMPETITION_MODE_ADDITIVE, 0); // WGSL零值固定表示完全叠加。
    assert_eq!(GPU_COMPETITION_MODE_WEIGHTED, 1); // WGSL一值固定表示加权容量分区。
    assert_eq!(GPU_COMPETITION_MODE_PRIORITY, 2); // WGSL二值固定表示优先覆盖。
    assert_eq!(GPU_COMPETITION_MAX_LAYERS, 32); // 单组u32活动掩码固定容纳三十二层。
    assert_eq!(GPU_COMPETITION_MAX_BUFFER_BYTES, 128 * 1024 * 1024); // 正式加载尖峰不得静默突破固定单缓冲预算。
    assert!(GPU_COMPOSE_SHADER.contains("fn compose_density")); // 测试常量必须继续指向生产正式入口而不是第二份算法副本。
}

#[test]
fn production_competition_shader_is_valid_wgsl() {
    let module = wgpu::naga::front::wgsl::parse_str(GPU_COMPOSE_SHADER)
        .expect("production competition shader must parse as WGSL"); // 默认单测立即阻断保留字或语法错误。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(), // 覆盖函数返回、循环、存储缓冲和纹理写入合法性。
        wgpu::naga::valid::Capabilities::all(),    // 使用完整标准能力避免误拒绝正式桌面后端语法。
    )
    .validate(&module)
    .expect("production competition shader must pass Naga validation"); // 不再等到显卡门控烟雾才发现控制流错误。
}
