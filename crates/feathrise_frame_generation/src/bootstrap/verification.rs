//! Vulkan创建前校验固定生产DLL字节；只在启动执行有界流式读取，不进入帧关键路径。
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path}; // 不整文件分配或扫描目录。 // 与构建前置检查使用同一SHA256算法。
use feathrise_runtime_manifest::streamline::RUNTIME_FILES; // 与构建和编辑器交付共用唯一身份。
pub(super) fn verify(directory: &Path) -> Result<(), String> {
    // 失败时不得加载任何厂商模块。
    verify_files(directory, &RUNTIME_FILES) // 构建与启动同一七项FG身份。
} // FG目录验证入口结束。
pub(super) fn verify_upscaling(directory: &Path) -> Result<(), String> {
    // 与 xtask 的 DLSS 310.9.1 发行锁一致，避免 Streamline 先装载其它版本的 NGX 模块。
    verify_files(
        directory,
        &[
            (
                "nvngx_dlss.dll",
                "3975567B8943C53ACCE397F2B72380092F84F162D00B0D2C7D08A1025C563983",
            ), // 正式SR运行库。
            (
                "nvngx_dlssd.dll",
                "4BC7EA5FCB2F32CF86BC2CB072E8D2860914A0BE511CBDCB2FC206C1D9D83B80",
            ), // 正式RR运行库。
        ],
    ) // 不把FG加入超分后端自身目录。
} // SR/RR组合依赖验证结束。
fn verify_files(directory: &Path, files: &[(&str, &str)]) -> Result<(), String> {
    // 固定清单的公共有界读取。
    let mut buffer = vec![0_u8; 1024 * 1024]; // 全部文件共用固定1MiB堆缓冲。
    for &(name, expected) in files {
        // 仅固定清单，无目录递归。
        let mut file = File::open(directory.join(name))
            .map_err(|error| format!("无法读取DLSS帧生成运行库{name}：{error}"))?; // 保留精确文件及I/O阶段。
        let mut hasher = Sha256::new(); // 每个文件独立摘要。
        loop {
            // 启动期覆盖全部字节。
            let length = file
                .read(&mut buffer)
                .map_err(|error| format!("读取DLSS帧生成运行库{name}失败：{error}"))?; // 读取失败不能跳过。
            if length == 0 {
                break;
            } // EOF才完成验证。
            hasher.update(&buffer[..length]); // 只加入实际字节。
        } // 固定缓冲读取结束。
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect(); // 固定32字节摘要格式。
        if actual != expected {
            return Err(format!("DLSS帧生成运行库{name}的SHA256不匹配"));
        } // 即使其他版本仍有合法签名也拒绝加载。
    } // 全部生产身份完成。
    Ok(()) // 随后原生层继续校验签名与签名者。
} // 启动身份校验结束。
