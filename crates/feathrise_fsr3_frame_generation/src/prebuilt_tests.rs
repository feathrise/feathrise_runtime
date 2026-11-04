//! 原生分发回归：实际链接预编译 C ABI 并引导固定 DLL，不以 cargo check 代替链接验证。

#[test]
fn linked_bridge_bootstraps_the_pinned_runtime_without_a_gpu() {
    let directory = std::path::Path::new(env!("BL5_FSR3_FRAME_GENERATION_RUNTIME_DIR"));
    let result = crate::native::bootstrap(directory);
    let detail = crate::native::last_error();
    let status = crate::native::status();
    // 此时没有 Vulkan 设备和窗口；测试仅确认加载/ABI，不声称帧生成画面正确。
    unsafe {
        crate::native::bl5_fsr3_fg_shutdown();
    }
    assert_eq!(
        result,
        crate::native::RESULT_OK,
        "真实预编译桥引导失败：{detail}"
    );
    assert_eq!(status.runtime_loaded, 1);
    assert_eq!(status.attached, 0);
}
