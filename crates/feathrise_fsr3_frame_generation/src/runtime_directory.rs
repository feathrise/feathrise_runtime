//! FSR 运行目录只在启动期选择；移动或残缺安装不得借用编译机 OUT_DIR 掩盖缺件。

use std::path::{Path, PathBuf};

pub(super) fn resolve(executable: &Path, output: &Path, cargo_runtime: &Path) -> Result<PathBuf, String> {
    let directory = executable.parent().ok_or("FSR 主程序缺少父目录")?
        .canonicalize().map_err(|error| format!("无法检查 FSR 主程序目录：{error}"))?;
    let deployed = directory.join("fsr3-frame-generation-runtime");
    // 目录存在即归安装所有；缺 DLL、缺资料、错版均由既有六文件身份门明确拒绝。
    let installed = deployed.try_exists().map_err(|error| format!("无法检查 FSR 安装目录：{error}"))?
        || directory.join("package-manifest.json").try_exists()
            .map_err(|error| format!("无法检查程序分发清单：{error}"))?;
    let profile = output.ancestors().nth(3).and_then(|path| path.canonicalize().ok());
    let cargo_location = profile.as_deref().is_some_and(|profile| {
        directory == profile || directory == profile.join("deps")
    });
    Ok(select(&deployed, cargo_runtime, installed, cargo_location))
}

fn select(deployed: &Path, cargo_runtime: &Path, installed: bool, cargo_location: bool) -> PathBuf {
    if !installed && cargo_location {
        cargo_runtime.to_owned() // 原 Cargo 输出与测试 deps 才保留开发私有副本入口。
    } else {
        deployed.to_owned() // 移动后的裸程序也要求完整邻近包，绝不回退编译机器。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_output_keeps_development_runtime() {
        assert_eq!(select(Path::new("deployed"), Path::new("cargo"), false, true), Path::new("cargo"));
    }

    #[test]
    fn moved_executable_requires_adjacent_runtime_even_when_missing() {
        assert_eq!(select(Path::new("deployed"), Path::new("cargo"), false, false), Path::new("deployed"));
    }

    #[test]
    fn partial_install_never_falls_back_to_cargo() {
        for cargo_location in [false, true] {
            assert_eq!(select(Path::new("deployed"), Path::new("cargo"), true, cargo_location), Path::new("deployed"));
        }
    }
}
