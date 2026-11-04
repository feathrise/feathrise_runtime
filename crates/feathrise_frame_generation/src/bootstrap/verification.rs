//! Vulkan创建前校验固定生产DLL字节；只在启动执行有界流式读取，不进入帧关键路径。
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path}; // 不整文件分配或扫描目录。 // 与构建前置检查使用同一SHA256算法。
use feathrise_runtime_manifest::streamline::RUNTIME_FILES; // 与构建和编辑器交付共用唯一身份。
pub(super) fn verify(directory: &Path) -> Result<(), String> {
    // 失败时不得加载任何厂商模块。
    verify_files(directory, &RUNTIME_FILES) // 构建与启动同一七项FG身份。
} // FG目录验证入口结束。
pub(super) fn verify_upscaling(directory: &Path) -> Result<(), String> {
    // 与xtask DLSS 310.5.3生产发行锁一致。
    verify_files(
        directory,
        &[
            (
                "nvngx_dlss.dll",
                "8707E53B26C68C606B98BF31C223485FF30D310A261B1A36D48B2EAABC1507EC",
            ), // 正式SR运行库。
            (
                "nvngx_dlssd.dll",
                "E1D1B8D8D07E7210B5FEAFC42F118FE3BF0144B9E7F2768E44C6F652FAA6D7B8",
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
