//! 向维护组包工具公开本程序实际编入的六文件身份；不读取磁盘清单，也不启动帧生成。

/// 只读 ASCII 身份标记。宿主保留此字符串后，组包可与固定预编译包逐项核对。
/// 不提供加载／卸载或修改编译身份的入口；运行时仍由原身份门再次检查完整六文件。
pub fn distribution_build_identity() -> &'static str {
    concat!(
        "FEATHRISE_FSR3_RUNTIME_IDENTITY_V1\0",
        env!("BL5_FSR3_RUNTIME_FILE_0_NAME"), "=", env!("BL5_FSR3_RUNTIME_FILE_0_SHA256"), "\0",
        env!("BL5_FSR3_RUNTIME_FILE_1_NAME"), "=", env!("BL5_FSR3_RUNTIME_FILE_1_SHA256"), "\0",
        env!("BL5_FSR3_RUNTIME_FILE_2_NAME"), "=", env!("BL5_FSR3_RUNTIME_FILE_2_SHA256"), "\0",
        env!("BL5_FSR3_RUNTIME_FILE_3_NAME"), "=", env!("BL5_FSR3_RUNTIME_FILE_3_SHA256"), "\0",
        env!("BL5_FSR3_RUNTIME_FILE_4_NAME"), "=", env!("BL5_FSR3_RUNTIME_FILE_4_SHA256"), "\0",
        env!("BL5_FSR3_RUNTIME_FILE_5_NAME"), "=", env!("BL5_FSR3_RUNTIME_FILE_5_SHA256"),
        "\0FEATHRISE_FSR3_RUNTIME_IDENTITY_END",
    )
}
