//! 构建脚本与发布预检共用的 Streamline 身份校验，缺省数字 ID 使用项目 UUID。

pub fn application_id(raw: Option<&str>) -> Result<u32, &'static str> {
    // 缺失与显式无效配置必须区分。
    let Some(raw) = raw else {
        return Ok(0);
    }; // 零只作为内部选择 Project ID 路径的哨兵。
    raw.parse::<u32>().ok().filter(|value| *value != 0).ok_or(
        // 显式数字 ID 必须属于非零 u32。
        "STREAMLINE_APPLICATION_ID must be a non-zero u32 assigned by NVIDIA", // 不输出身份值。
    ) // 格式错误不允许静默回退项目身份。
} // 身份校验结束。

#[cfg(test)] // 无需 SDK 或 GPU 即可验证身份边界。
mod tests {
    // 构建和发布共同执行同一套契约。
    use super::application_id; // 使用实际校验函数。
    #[test] // 未配置数字身份是合法官方路径。
    fn absent_id_uses_project_identity() {
        // 项目 UUID 由运行时固定提供。
        assert_eq!(application_id(None), Ok(0)); // 不要求开发者申请数字 ID。
    } // 缺省身份测试结束。
    #[test] // 显式无效输入不得绕过配置错误。
    fn invalid_explicit_id_is_rejected() {
        // 覆盖空值、零、溢出、非数字和空白。
        for raw in ["", "0", "4294967296", "project-guid", " 1", "1 ", "-1"] {
            // 边界输入集合。
            assert!(application_id(Some(raw)).is_err()); // 均保持硬失败。
        } // 输入检查结束。
    } // 无效身份测试结束。
    #[test] // 仅验证数字边界，不用于初始化或借用样例身份。
    fn numeric_id_preserves_value() {
        // NVIDIA 分配值交给原生层选择数字路径。
        assert_eq!(application_id(Some("4294967295")), Ok(u32::MAX)); // u32 上界不得被截断。
    } // 数字身份测试结束。
} // 身份测试结束。
