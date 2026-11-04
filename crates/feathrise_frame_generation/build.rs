mod identity; // 构建与发布预检共用数字身份校验。

use sha2::{Digest as _, Sha256}; // 固定SDK文件必须在编译前按字节验证，不能只相信目录名。
use std::{
    // 构建脚本只处理本机SDK、Cargo私有输出和C++桥编译。
    env,                   // 读取Cargo目标、OUT_DIR以及显式SDK环境变量。
    ffi::OsStr,            // 校验SDK根目录名，避免误选相邻版本。
    fs,                    // 创建私有运行目录并复制经过哈希验证的生产DLL。
    io::Read as _,         // 流式计算大DLL哈希，避免一次性占用额外内存。
    path::{Path, PathBuf}, // 所有输入输出都先解析成明确路径再处理。
}; // 构建脚本标准库导入结束。

const SDK_DIRECTORY_NAME: &str = "Streamline-v2.12.0"; // 本轮经官方发布页和API摘要锁定的Streamline版本。
const RUNTIME_DIRECTORY_NAME: &str = "streamline-runtime"; // Cargo私有DLL目录不得与既有NGX SR/RR白名单混用。

use feathrise_runtime_manifest::streamline::RUNTIME_FILES; // 同一白名单用于复制前后与启动前校验。

fn main() {
    // 显式FG特性进入本crate后才执行SDK预检与C++桥编译。
    println!("cargo:rerun-if-env-changed=STREAMLINE_SDK"); // SDK路径改变时必须重做哈希与桥编译。
    println!("cargo:rerun-if-env-changed=STREAMLINE_APPLICATION_ID"); // NVIDIA项目ID改变时重新嵌入运行配置。
    println!("cargo:rerun-if-env-changed=VULKAN_SDK"); // Vulkan头文件版本改变时重编C++桥。
    println!("cargo:rerun-if-changed=identity.rs"); // 身份契约改变时重新验证配置。
    println!("cargo:rerun-if-changed=src/native/nvidia_authenticode.h"); // 原生签名策略变更时必须重编桥。
    println!("cargo:rerun-if-changed=src/native/streamline_bridge.cpp"); // 桥实现改变时触发本crate重建。
    println!("cargo:rerun-if-changed=src/native/presentation_counter.h"); // 累计呈现计数契约变化时同步重编原生桥。

    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("Cargo must provide target OS"); // 读取目标而不是宿主平台。
    let target_arch =
        env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo must provide target architecture"); // DLL仅有x64版本。
    if target_os != "windows" || target_arch != "x86_64" {
        // Streamline 2.12首期范围固定Windows x64 Vulkan。
        panic!("dlss_frame_generation currently supports only x86_64-pc-windows-msvc"); // 其他目标必须明确失败，不能静默产出假功能。
    } // 目标平台校验结束。

    let sdk_root = required_directory("STREAMLINE_SDK"); // 用户显式提供SDK根，不扫描磁盘猜版本。
    if sdk_root.file_name() != Some(OsStr::new(SDK_DIRECTORY_NAME)) {
        // 目录名也是版本锁的一部分。
        panic!("STREAMLINE_SDK must point to a directory named {SDK_DIRECTORY_NAME}"); // 防止相邻版本偶然通过部分文件检查。
    } // SDK目录名校验结束。
    let include_dir = sdk_root.join("include"); // 官方公共头文件目录。
    let production_dir = sdk_root.join("bin").join("x64"); // 只允许生产DLL，development目录不得进入发布链。
    for header in [
        "sl.h",
        "sl_dlss_g.h",
        "sl_reflex.h",
        "sl_pcl.h",
        "sl_security.h",
    ] {
        // C++桥依赖的最小头文件集合。
        require_file(&include_dir.join(header)); // 任一头缺失都说明SDK不完整。
    } // 头文件预检结束。

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must provide OUT_DIR")); // Cargo专属可写输出根。
    let runtime_dir = out_dir.join(RUNTIME_DIRECTORY_NAME); // SR/RR与FG使用相互独立的私有目录。
    fs::create_dir_all(&runtime_dir)
        .unwrap_or_else(|error| panic!("failed to create {}: {error}", runtime_dir.display())); // 只创建本crate私有子目录。
    for (file_name, expected_hash) in RUNTIME_FILES {
        // 每个生产DLL都先验证源，再覆盖私有副本。
        let source = production_dir.join(file_name); // 源文件必须来自固定bin/x64目录。
        require_file(&source); // 缺少依赖时在长Rust编译前立即失败。
        verify_sha256(&source, expected_hash); // 哈希不符时拒绝未知或开发版二进制。
        let destination = runtime_dir.join(file_name); // 输出文件名保持NVIDIA加载器预期名称。
        fs::copy(&source, &destination).unwrap_or_else(|error| {
            panic!(
                "failed to copy {} to {}: {error}",
                source.display(),
                destination.display()
            )
        }); // 只复制白名单文件。
        verify_sha256(&destination, expected_hash); // 复制后再次校验，防止损坏或并发替换。
    } // 运行时白名单构建结束。

    let vulkan_root = required_directory("VULKAN_SDK"); // C++桥只使用SDK头，不链接另一套Vulkan加载器。
    let vulkan_include = vulkan_root.join("Include"); // Windows Vulkan SDK标准头路径。
    require_file(&vulkan_include.join("vulkan").join("vulkan.h")); // 原生资源结构必须与Vulkan ABI一致。

    cc::Build::new() // 使用MSVC编译单一C++动态调用桥，避免静态链接sl.interposer.lib。
        .cpp(true) // Streamline公共头是C++接口。
        .std("c++17") // SDK 2.12支持且当前桥需要的最低语言版本。
        .flag_if_supported("/utf-8") // 原生桥含中文设计注释，显式固定MSVC源与执行字符集。
        .warnings(true) // 本地桥必须接受编译器诊断。
        .include(&include_dir) // Streamline官方头文件。
        .include(&vulkan_include) // Vulkan句柄与格式定义。
        .file("src/native/streamline_bridge.cpp") // 唯一原生翻译单元，避免sl_security全局定义产生ODR冲突。
        .compile("rpg_streamline_bridge"); // 只链接本项目桥静态库，不链接NVIDIA interposer import lib。

    println!(
        "cargo:rustc-env=BL5_STREAMLINE_RUNTIME_DIR={}",
        runtime_dir.display()
    ); // Rust前置插件据此安全加载和登记同一代理。
    let raw_id = env::var("STREAMLINE_APPLICATION_ID"); // 区分缺省、显式空值与非Unicode输入。
    let raw_id = match raw_id {
        // 非Unicode环境配置不能默默回退。
        Ok(value) => Some(value), // 显式值必须通过非零数字校验。
        Err(env::VarError::NotPresent) => None, // 未设置时采用固定项目UUID。
        Err(env::VarError::NotUnicode(_)) => panic!("STREAMLINE_APPLICATION_ID must be Unicode"), // 不泄露原始值。
    }; // 环境读取结束。
    let application_id =
        identity::application_id(raw_id.as_deref()).unwrap_or_else(|error| panic!("{error}")); // 缺省产生内部零哨兵。
    println!("cargo:rustc-env=BL5_STREAMLINE_APPLICATION_ID={application_id}"); // 运行时只读取本次经过校验的构建输入。
} // 构建入口结束。

fn required_directory(name: &str) -> PathBuf {
    // 统一环境目录解析和错误信息。
    let path = PathBuf::from(
        env::var_os(name)
            .unwrap_or_else(|| panic!("{name} must be set for dlss_frame_generation builds")),
    ); // 不扫描或猜测本机路径。
    if !path.is_dir() {
        // 文件或不存在的路径都不能继续。
        panic!("{name} does not point to a directory: {}", path.display()); // 报告精确配置错误。
    } // 目录存在性校验结束。
    path // 返回明确SDK根。
} // required_directory结束。

fn require_file(path: &Path) {
    // 统一必需文件预检。
    if !path.is_file() {
        // 目录或缺失都视为SDK不完整。
        panic!(
            "required Streamline/Vulkan SDK file is missing: {}",
            path.display()
        ); // 在编译桥前给出具体路径。
    } // 文件校验结束。
} // require_file结束。

fn verify_sha256(path: &Path, expected: &str) {
    // 对官方生产二进制执行完整SHA-256固定。
    let mut file = fs::File::open(path)
        .unwrap_or_else(|error| panic!("failed to open {}: {error}", path.display())); // 只读打开明确文件。
    let mut hasher = Sha256::new(); // 每个文件使用独立哈希上下文。
    // Windows 构建脚本线程的默认栈较小；哈希缓冲放在堆上，避免预检大 DLL 时栈溢出。
    let mut chunk = vec![0_u8; 1024 * 1024]; // 1MiB有界缓冲避免大DLL峰值内存。
    loop {
        // 直到EOF覆盖文件全部字节。
        let read = file
            .read(&mut chunk)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display())); // I/O异常立即失败。
        if read == 0 {
            // 零长度读取代表EOF。
            break; // 完成该文件哈希。
        } // EOF判断结束。
        hasher.update(&chunk[..read]); // 仅加入实际读取区间。
    } // 流式读取结束。
    let digest = hasher.finalize(); // 固定三十二字节SHA-256结果。
    let actual = digest
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>(); // sha2 0.11不再为输出数组实现UpperHex，逐字节生成稳定大写文本。
    if actual != expected {
        // 任一字节不同都拒绝加载。
        panic!(
            "SHA-256 mismatch for {}: expected {expected}, got {actual}",
            path.display()
        ); // 明确暴露版本/文件异常。
    } // 哈希一致性校验结束。
} // verify_sha256结束。
