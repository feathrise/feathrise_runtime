//! 启动边界复验本项目 FSR Vulkan 源码修订；所有期望哈希来自编译产物，绝不来自旁边 JSON。

use sha2::{Digest as _, Sha256}; // 运行时与构建使用相同完整字节哈希。
use std::{fs, io::Read as _, path::Path}; // 文件读取只发生在插件前置引导，不进入帧关键路径。

const FILES: [(&str, &str); 6] = [
    // 与构建和发行共用的六项产物协议保持一致。
    (
        env!("BL5_FSR3_RUNTIME_FILE_0_NAME"),
        env!("BL5_FSR3_RUNTIME_FILE_0_SHA256"),
    ), // 修订 DLL 的名称与期望身份都由共用白名单固化。
    (
        env!("BL5_FSR3_RUNTIME_FILE_1_NAME"),
        env!("BL5_FSR3_RUNTIME_FILE_1_SHA256"),
    ), // manifest 必须与该主程序构建时完全相同。
    (
        env!("BL5_FSR3_RUNTIME_FILE_2_NAME"),
        env!("BL5_FSR3_RUNTIME_FILE_2_SHA256"),
    ), // 原 AMD MIT 许可。
    (
        env!("BL5_FSR3_RUNTIME_FILE_3_NAME"),
        env!("BL5_FSR3_RUNTIME_FILE_3_SHA256"),
    ), // 本项目所用源码修改记录。
    (
        env!("BL5_FSR3_RUNTIME_FILE_4_NAME"),
        env!("BL5_FSR3_RUNTIME_FILE_4_SHA256"),
    ), // 官方输入源码身份记录。
    (
        env!("BL5_FSR3_RUNTIME_FILE_5_NAME"),
        env!("BL5_FSR3_RUNTIME_FILE_5_SHA256"),
    ), // 复现与署名说明。
]; // 不从可替换 manifest 动态获取期望值。

pub(crate) fn verify_runtime_identity(directory: &Path) -> Result<(), String> {
    // 在任何 LoadLibrary 调用前执行。
    for (name, expected) in FILES {
        // 必须同时存在完整、匹配的来源记录。
        verify_file(&directory.join(name), expected)?; // 任一字节变化立即阻止候选加载。
    } // 六项文件核验结束。
    for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
        // 发布与开发目录采用同一白名单。
        let entry = entry.map_err(|e| e.to_string())?; // 不忽略读目录失败。
        if !entry.file_type().map_err(|e| e.to_string())?.is_file()
            || !FILES.iter().any(|(name, _)| entry.file_name() == *name)
        {
            // 子目录和额外 DLL 都不属于已审计包。
            return Err(format!(
                "FSR 源码修订运行目录存在白名单外条目：{}",
                entry.path().display()
            )); // 报告明确来源身份错误。
        } // 当前条目验证结束。
    } // 全目录审计结束。
    Ok(()) // C++ 将再次复验 DLL 并持有只读锁直至加载完成。
} // verify_runtime_identity 结束。

fn verify_file(path: &Path, expected: &str) -> Result<(), String> {
    // 大 DLL 不整体读入内存。
    let mut file = fs::File::open(path)
        .map_err(|e| format!("FSR 身份文件不可读取：{}（{e}）", path.display()))?; // 缺少来源资料也禁止加载。
    let mut hash = Sha256::new(); // 每个文件独立上下文。
    let mut buffer = vec![0_u8; 1024 * 1024]; // 启动期固定 1 MiB 堆预算。
    loop {
        // 完整覆盖每一字节。
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?; // 读取错误不返回部分身份。
        if count == 0 {
            break;
        } // 到达 EOF。
        hash.update(&buffer[..count]); // 使用有效读取范围。
    } // 哈希读取结束。
    let actual: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect(); // 与构建固定的大写格式一致。
    if actual != expected {
        return Err(format!(
            "FSR 源码修订文件身份不匹配：{}，期望 {expected}，实际 {actual}",
            path.display()
        ));
    } // 不允许同名 DLL 或可替换 JSON 自证身份。
    Ok(()) // 单文件字节身份有效。
} // verify_file 结束。

#[cfg(test)] // 测试只在开发执行，不引入运行路径。
mod tests {
    // 验证已部署包与核心篡改拒绝行为。
    use super::*; // 测试同一启动入口。
    #[test] // Cargo 私有副本必须具备全部来源资料。
    fn cargo_private_runtime_passes_pinned_source_revision_gate() {
        // 不再测试旧 AMD 签名 DLL 常量。
        verify_runtime_identity(Path::new(env!("BL5_FSR3_FRAME_GENERATION_RUNTIME_DIR")))
            .expect("构建时锁定的完整源码修订包必须通过启动身份门"); // 覆盖完整白名单与六项哈希。
    } // 正向身份测试结束。
    #[test] // 任意文件不能用旁边 manifest 的新值绕过二进制固化身份。
    fn changed_bytes_fail_even_with_a_valid_hash_shape() {
        // 小文件验证真实字节比较，而不复制大 DLL。
        let path =
            std::env::temp_dir().join(format!("fsr3_identity_tamper_{}.json", std::process::id())); // 测试专用明确路径。
        fs::write(&path, b"{\"dll_sha256\":\"changed\"}").expect("应写入模拟被替换的 manifest"); // 模拟旁边来源记录被修改。
        let result = verify_file(&path, env!("BL5_FSR3_RUNTIME_FILE_1_SHA256")); // 期望值始终来自主程序构建。
        fs::remove_file(&path).expect("应移除测试创建的单一临时文件"); // 不递归操作未知目录。
        assert!(result.is_err()); // 篡改不能通过身份门。
    } // 反向身份测试结束。
} // tests 结束。
