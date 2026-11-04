//! 默认校验并链接 FSR 预编译配套包；SDK 与 C++ 源码重建仅面向维护者/CI。

mod native_package;
mod runtime_package;

use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

const RUNTIME_DIRECTORY_NAME: &str = "fsr3-frame-generation-runtime";

fn main() {
    for name in native_package::INPUT_ENVS.into_iter().chain([
        "FEATHRISE_FSR3_BUILD_FROM_SOURCE",
        "FEATHRISE_FSR3_PATCH_ROOT",
        "FIDELITYFX_SDK",
        "VULKAN_SDK",
        "BL5_FSR3_CMAKE",
        "BL5_FSR3_PWSH",
        "BL5_FSR3_EXPECTED_MANIFEST_SHA256",
        "BL5_FSR3_EXPECTED_NATIVE_IDENTITY",
        "LOCALAPPDATA",
    ]) {
        println!("cargo:rerun-if-env-changed={name}");
    }
    for path in [
        "native_package.rs",
        "native_package",
        "runtime_package.rs",
        "prebuilt/package.lock.json",
    ]
    .into_iter()
    .chain(native_package::SOURCE_FILES)
    {
        println!("cargo:rerun-if-changed={path}");
    }
    let target = env::var("TARGET").expect("Cargo target");
    assert_eq!(
        target,
        native_package::TARGET,
        "FSR 帧生成当前只支持 Windows x64 MSVC"
    );
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo manifest directory"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR"));
    let runtime_dir = out_dir.join(RUNTIME_DIRECTORY_NAME);
    let source_mode = env::var("FEATHRISE_FSR3_BUILD_FROM_SOURCE").as_deref() == Ok("1");
    let package = if source_mode {
        assert!(
            env::var_os("BL5_FSR3_EXPECTED_NATIVE_IDENTITY").is_none(),
            "源码模式不能匹配发行预检锁定的预编译包身份，请重新预检"
        );
        source_package(&manifest_dir)
    } else {
        assert!(
            !env::var("CARGO_CFG_TARGET_FEATURE")
                .unwrap_or_default()
                .split(',')
                .any(|f| f == "crt-static"),
            "FSR 预编译桥使用动态 CRT；静态 CRT 请由维护者显式源码重建"
        );
        if let Some(path) = env::var_os("FEATHRISE_FSR3_PACKAGE") {
            println!("cargo:rerun-if-changed={}", PathBuf::from(path).display());
        }
        let native =
            native_package::acquire(&manifest_dir, &target).unwrap_or_else(|e| panic!("{e}"));
        if let Ok(expected) = env::var("BL5_FSR3_EXPECTED_NATIVE_IDENTITY") {
            assert!(
                native.identity.eq_ignore_ascii_case(&expected),
                "FSR 原生包在发行预检后改变"
            );
        }
        println!("cargo:rerun-if-changed={}", native.bridge_library.display());
        let private_bridge = out_dir.join("rpg_fsr3_frame_generation_bridge.lib");
        fs::copy(&native.bridge_library, &private_bridge)
            .unwrap_or_else(|e| panic!("复制 FSR 桥接库失败：{e}"));
        assert_eq!(
            runtime_package::sha256(&private_bridge).unwrap_or_else(|e| panic!("{e}")),
            native.bridge_sha256,
            "FSR 桥接库在校验后发生变化"
        );
        println!("cargo:rustc-link-search=native={}", out_dir.display());
        println!("cargo:rustc-link-lib=static=rpg_fsr3_frame_generation_bridge");
        println!("cargo:rustc-link-lib=dylib=msvcprt");
        native.runtime
    };
    if let Ok(expected) = env::var("BL5_FSR3_EXPECTED_MANIFEST_SHA256") {
        assert!(
            package.files[1].1.eq_ignore_ascii_case(&expected),
            "FSR revision changed after release preflight"
        );
    }
    fs::create_dir_all(&runtime_dir).unwrap_or_else(|e| panic!("创建 FSR 运行输出目录失败：{e}"));
    package
        .copy_into(&runtime_dir)
        .unwrap_or_else(|e| panic!("{e}"));
    // 发行与 cargo run 继续使用原六文件白名单；桥接静态库不会进入游戏目录。
    for (index, (source, hash)) in package.files.iter().enumerate() {
        println!("cargo:rerun-if-changed={}", source.display());
        println!(
            "cargo:rustc-env=BL5_FSR3_RUNTIME_FILE_{index}_NAME={}",
            runtime_package::FILE_NAMES[index]
        );
        println!("cargo:rustc-env=BL5_FSR3_RUNTIME_FILE_{index}_SHA256={hash}");
    }
    println!(
        "cargo:rustc-env=BL5_FSR3_RUNTIME_REVISION={}",
        runtime_package::REVISION
    );
    println!("cargo:rustc-link-lib=bcrypt");
    println!(
        "cargo:rustc-env=BL5_FSR3_FRAME_GENERATION_RUNTIME_DIR={}",
        runtime_dir.display()
    );
}

/// 维护者保留原完整来源审计和重建路径；预编译获取失败不会隐式进入此分支。
fn source_package(manifest_dir: &Path) -> runtime_package::RuntimePackage {
    // 只有显式源码模式读取独立补丁仓，普通 ZIP 消费不要求补丁材料在场。
    let patch_root = required_directory("FEATHRISE_FSR3_PATCH_ROOT");
    let sdk_root = required_directory("FIDELITYFX_SDK");
    assert_eq!(
        sdk_root.file_name(),
        Some(OsStr::new("FidelityFX-SDK-v1.1.4")),
        "FIDELITYFX_SDK 必须指向 FidelityFX-SDK-v1.1.4"
    );
    let api_include = sdk_root.join("ffx-api/include");
    for header in [
        "ffx_api/ffx_api.h",
        "ffx_api/ffx_api_types.h",
        "ffx_api/ffx_framegeneration.h",
        "ffx_api/vk/ffx_api_vk.h",
    ] {
        require_file(&api_include.join(header));
    }
    verify_sha256(
        &sdk_root.join("PrebuiltSignedDLL/amd_fidelityfx_vk.dll"),
        runtime_package::OFFICIAL_DLL_SHA256,
    );
    verify_sha256(
        &sdk_root.join("sdk/LICENSE.txt"),
        runtime_package::LICENSE_SHA256,
    );
    let vulkan = required_directory("VULKAN_SDK");
    let include = vulkan.join("Include");
    require_file(&include.join("vulkan/vulkan.h"));
    let workspace = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace/crates/FSR");
    // Cargo 跟踪与实际启动共用 scripts/ 路径，辅助脚本归位时不维护第二份字符串。
    println!(
        "cargo:rerun-if-changed={}",
        workspace.join(runtime_package::RUNTIME_BUILD_SCRIPT).display()
    );
    let built = runtime_package::ensure_runtime_built(workspace, &sdk_root, &vulkan, &patch_root)
        .unwrap_or_else(|e| panic!("{e}"));
    if built {
        println!("cargo:warning=维护者模式已重建当前 runtime 工作树的 FSR 修订运行库");
    }
    let package =
        runtime_package::inspect(workspace, &patch_root).unwrap_or_else(|e| panic!("{e}"));
    let cpp_dll_hash = format!("\"{}\"", package.files[0].1);
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        .flag_if_supported("/utf-8")
        .warnings(true)
        .define("BL5_FSR3_PATCHED_RUNTIME_SHA256", cpp_dll_hash.as_str())
        .include(api_include)
        .include(include)
        .file("src/native/fsr3_frame_generation_bridge.cpp")
        .compile("rpg_fsr3_frame_generation_bridge");
    package
}

fn required_directory(name: &str) -> PathBuf {
    let path = PathBuf::from(env::var_os(name).unwrap_or_else(|| panic!("源码重建需要 {name}")));
    assert!(path.is_dir(), "{name} 不是有效目录：{}", path.display());
    path
}
fn require_file(path: &Path) {
    assert!(path.is_file(), "缺少 SDK 输入：{}", path.display());
    println!("cargo:rerun-if-changed={}", path.display());
}
fn verify_sha256(path: &Path, expected: &str) {
    require_file(path);
    assert_eq!(
        runtime_package::sha256(path).unwrap_or_else(|e| panic!("{e}")),
        expected,
        "官方 SDK 来源 SHA-256 不匹配：{}",
        path.display()
    );
}
