//! 构建与发行共用的 FSR Vulkan 源码修订身份链；运行时只信编译固化的文件哈希。

use sha2::{Digest as _, Sha256}; // 官方来源与修订产物采用同一 SHA-256 算法。
use std::{
    // 标准库同时承担构建期磁盘校验与受控脚本启动。
    env,           // 手动解析 PowerShell 7 路径，避免 Cargo 环境中的 PATH 大小写条目冲突。
    fs,            // 源码重建写入显式独占输出，发布归档本身保持只读。
    io::Read as _, // 大 DLL 使用固定缓冲流式计算身份。
    path::{Path, PathBuf}, // 所有来源和目标都通过明确路径传递。
    process::Command, // 缺少当前输出的运行包时启动所属 crate 内的唯一构建入口。
}; // 标准库导入结束。

pub const REVISION: &str = "bl5-vulkan-lifetime-ui-1"; // 本项目修订号，明确区别 AMD 官方签名原件。
pub const OFFICIAL_DLL_SHA256: &str =
    "A1624CC4238FEF046F30C4D80CE3F47BE63FC5F5373F49E3EE9EDB9960F54C78"; // 官方来源字节身份保持不变。
pub const LICENSE_SHA256: &str = "C93D509CD69D50698796C9C3274247352736EF122A3197ACE2010F0978E21AA6"; // 原 MIT 许可不得改写。
const RUNTIME_RELATIVE_DIRECTORY: &str = ".local/fsr3-sdk-patched/runtime"; // 保留维护者旧工作区预检入口，不作为发布包构建的目录推断。
pub const RUNTIME_BUILD_SCRIPT: &str = "scripts/构建FSR修订运行库.ps1"; // 路径只相对当前 crate，解包后不需要完整 SDK 目录。

/// Cargo 默认使用自身私有 OUT_DIR；显式绝对目录可复用维护者已有源码构建缓存。
pub fn source_build_root(out_dir: &Path, configured: Option<&Path>) -> Result<PathBuf, String> {
    let directory = configured
        .map(Path::to_path_buf)
        .unwrap_or_else(|| out_dir.join("fsr3-source"));
    if !directory.is_absolute() {
        return Err("FEATHRISE_FSR3_SOURCE_BUILD_ROOT 必须是绝对目录".into());
    }
    Ok(directory)
}
#[cfg(test)] // 生产 build.rs 不需要测试临时目录预算。
const TEST_DIRECTORY_ATTEMPTS: u32 = 1024; // 并行测试使用有限唯一目录，避免异常残留造成无界循环。
pub const FILE_NAMES: [&str; 6] = [
    // 开发和发行使用完全相同的私有目录白名单。
    "amd_fidelityfx_vk.dll", // 本项目从 AMD SDK 源码构建的未签名 Vulkan DLL。
    "manifest.json",         // 含构建工具、来源、修订与产物身份的可追溯记录。
    "LICENSE-FidelityFX-SDK.txt", // 原 AMD MIT 许可。
    "v1.1.4-vulkan-lifetime.patch", // 对官方源码施加的完整补丁。
    "source-manifest.json",  // 官方输入源码和修订源码的文件身份。
    "README.md",             // 人类可读来源、修改内容和复现命令。
]; // 六项全部与主程序固定身份匹配后才允许加载。

#[derive(Clone, Debug)] // 发行预检持有不可变快照，长构建后仍按同一身份复验。
pub struct RuntimePackage {
    // 不依赖 Bevy，供 build.rs 与 xtask 复用。
    pub files: Vec<(PathBuf, String)>, // 顺序严格对应 FILE_NAMES，每项保存原路径和精确哈希。
} // RuntimePackage 结束。

#[allow(dead_code)] // 既有维护工具仍可预检工作区 .local 产物，Cargo 单包模式使用显式运行目录。
pub fn inspect(workspace: &Path, patch_root: &Path) -> Result<RuntimePackage, String> {
    inspect_runtime(&workspace.join(RUNTIME_RELATIVE_DIRECTORY), patch_root)
}

/// 来源校验只消费显式运行目录与外部补丁目录，不从包的父级推断任何输入。
pub fn inspect_runtime(runtime: &Path, patch_root: &Path) -> Result<RuntimePackage, String> {
    // 所有失败在加载或写入发行包之前返回。
    // 原生补丁归独立补丁仓；调用者显式传入材料目录，不搜索 SDK 或私有父目录。
    let manifest_path = runtime.join(FILE_NAMES[1]); // 不搜索其他 SDK 版本或旁路目录。
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|error| {
            format!(
                "读取修订 manifest 失败：{}（{error}）；请运行 {RUNTIME_BUILD_SCRIPT}",
                manifest_path.display()
            )
        })?)
        .map_err(|error| {
            format!(
                "解析修订 manifest 失败：{}（{error}）",
                manifest_path.display()
            )
        })?; // 拒绝无法追溯的裸 DLL。
    validate_manifest(&manifest)?; // 先核对固定协议、SDK、修订和未签名声明。
    let paths = [
        runtime.join(FILE_NAMES[0]),
        manifest_path,
        patch_root.join("LICENSE.txt"),
        patch_root.join(FILE_NAMES[3]),
        patch_root.join(FILE_NAMES[4]),
        patch_root.join(FILE_NAMES[5]),
    ]; // 来源资料不能被运行目录的同名文件替换。
    let hashes = [
        "dll_sha256",
        "",
        "license_sha256",
        "patch_sha256",
        "source_manifest_sha256",
        "",
    ]; // manifest 自身和补丁仓 README 使用完整原文字节哈希。
    let mut files = Vec::with_capacity(FILE_NAMES.len()); // 构建期固定六项小清单。
    for (index, path) in paths.into_iter().enumerate() {
        // 按固定白名单逐项核验。
        let actual = sha256(&path)?; // 流式计算真实字节身份。
        if !hashes[index].is_empty()
            && !actual.eq_ignore_ascii_case(field(&manifest, hashes[index])?)
        {
            // 不允许 manifest 只描述另一份产物。
            return Err(format!(
                "{} 与 manifest 的 {} 不匹配",
                path.display(),
                hashes[index]
            )); // 输出明确失败阶段。
        } // manifest 比较结束。
        files.push((path, actual)); // 保留已确认身份供构建与发行再次复验。
    } // 全部六项完成。
    let actual_size = fs::metadata(&files[0].0)
        .map_err(|error| error.to_string())?
        .len(); // DLL 长度与构建清单互证。
    if manifest["dll_size"].as_u64() != Some(actual_size) {
        // 拒绝截断或错误产物的记录。
        return Err("修订 DLL 长度与 manifest 不匹配".into()); // 不继续复制不一致产物。
    } // 长度核验结束。
    Ok(RuntimePackage { files }) // 不写入官方 SDK 或发布目录。
} // inspect 结束。

#[derive(Clone, Copy, Debug, Eq, PartialEq)] // 两态只表示可直接复用或可安全构建，任何半包都通过错误返回。
enum RuntimePairState {
    // 自动编译不接受第三种模糊状态。
    Complete, // DLL 与 manifest 都是普通文件，随后仍执行完整六文件身份校验。
    Missing,  // 专用目录不存在或为空，可以启动当前工作树构建脚本。
} // RuntimePairState 结束。

#[allow(dead_code)] // 保留 RPG 维护入口的四参合同；发布包构建不走完整工作区布局。
pub fn ensure_runtime_built(
    workspace: &Path,
    sdk_root: &Path,
    vulkan_root: &Path,
    patch_root: &Path,
) -> Result<bool, String> {
    ensure_runtime_built_at(
        &workspace.join("crates/feathrise_fsr3_frame_generation"),
        &workspace.join(".local/fsr3-sdk-patched"),
        sdk_root,
        vulkan_root,
        patch_root,
    )
}

/// 当前包与写入目录各自独立，支持 Cargo registry 的只读解包布局。
pub fn ensure_runtime_built_at(
    crate_root: &Path,
    build_root: &Path,
    sdk_root: &Path,
    vulkan_root: &Path,
    patch_root: &Path,
) -> Result<bool, String> {
    // 脚本来自当前包；缓存和外部输入分别显式传入，二者不再共享“工作区根”假设。
    ensure_runtime_built_with(build_root, || {
        invoke_runtime_build_script(crate_root, build_root, sdk_root, vulkan_root, patch_root)
    })
} // ensure_runtime_built 结束。

fn ensure_runtime_built_with<F>(build_root: &Path, mut build: F) -> Result<bool, String>
where
    F: FnMut() -> Result<(), String>,
{
    // 闭包隔离外部进程，单元测试不需要真实 SDK。
    let runtime = build_root.join("runtime"); // 状态检查只观察本次显式输出，不读取源码包或其他检出的运行库。
    if runtime_pair_state(&runtime)? == RuntimePairState::Complete {
        // 成对文件存在时直接进入后续身份校验。
        return Ok(false); // 后续构建不重复运行耗时的 SDK 源码编译。
    } // 已有运行包快速路径结束。
    build().map_err(|error| format!("编译当前包 FSR 修订运行库失败：{error}"))?; // 只有完全缺失状态才能执行一次构建器。
    match runtime_pair_state(&runtime)? {
        // 子进程成功退出后仍以磁盘事实确认固定文件对。
        RuntimePairState::Complete => Ok(true), // 调用方可输出仅本次首次构建提示。
        RuntimePairState::Missing => Err(format!(
            "FSR 构建脚本成功退出，但没有生成 {} 与 {}",
            runtime.join(FILE_NAMES[0]).display(),
            runtime.join(FILE_NAMES[1]).display()
        )), // 禁止把空成功当作可加载运行包。
    } // 构建后文件对确认结束。
} // ensure_runtime_built_with 结束。

fn invoke_runtime_build_script(
    crate_root: &Path,
    build_root: &Path,
    sdk_root: &Path,
    vulkan_root: &Path,
    patch_root: &Path,
) -> Result<(), String> {
    let powershell = resolve_powershell_7()?;
    let mut command = runtime_build_command(
        &powershell,
        crate_root,
        build_root,
        sdk_root,
        vulkan_root,
        patch_root,
    )?;
    let status = command.status().map_err(|error| {
        format!(
            "无法启动 PowerShell 7 {} 执行包内 FSR 构建脚本（{error}）",
            powershell.display()
        )
    })?;
    if !status.success() {
        return Err(format!(
            "{} 退出状态为 {status}；可手动运行该脚本查看完整诊断",
            crate_root.join(RUNTIME_BUILD_SCRIPT).display()
        ));
    }
    Ok(()) // 文件完整性由调用方状态检查与随后 inspect_runtime 双重确认。
}

/// 构造真实子进程参数，测试可在无 SDK 的独立解包布局中核对脚本和输出位置。
fn runtime_build_command(
    powershell: &Path,
    crate_root: &Path,
    build_root: &Path,
    sdk_root: &Path,
    vulkan_root: &Path,
    patch_root: &Path,
) -> Result<Command, String> {
    let script = crate_root.join(RUNTIME_BUILD_SCRIPT);
    let script_metadata = fs::metadata(&script)
        .map_err(|error| format!("读取包内 FSR 构建脚本失败：{}（{error}）", script.display()))?;
    if !script_metadata.is_file() {
        return Err(format!("FSR 构建脚本不是普通文件：{}", script.display()));
    }
    let mut command = Command::new(powershell);
    command
        .arg("-NoLogo")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(script)
        .arg("-SdkRoot")
        .arg(sdk_root)
        .arg("-VulkanSdk")
        .arg(vulkan_root)
        .arg("-PatchRoot")
        .arg(patch_root)
        .arg("-BuildRoot")
        .arg(build_root)
        .current_dir(crate_root); // Git apply 不需要主仓；相对进程位置只归当前包。
    Ok(command)
}

fn resolve_powershell_7() -> Result<PathBuf, String> {
    // 显式配置优先，随后手动遍历所有大小写形式的 PATH。
    if let Some(configured) = env::var_os("BL5_FSR3_PWSH") {
        // CI 或非标准安装可固定 PowerShell 7 完整路径。
        let configured_path = PathBuf::from(configured); // 显式值不作为命令字符串交给 shell。
        if configured_path.is_file() {
            return Ok(configured_path);
        } // 普通文件才能成为自动执行入口。
        return Err(format!(
            "BL5_FSR3_PWSH 不是有效文件：{}",
            configured_path.display()
        )); // 显式错误不得被静默回退掩盖。
    } // 显式 PowerShell 7 解析结束。
    for (key, value) in env::vars_os() {
        // Windows 宿主可能注入 PATH 与 Path 两个大小写不同的条目。
        if !key.to_string_lossy().eq_ignore_ascii_case("PATH") {
            continue;
        } // 其他环境值不得参与可执行文件搜索。
        for directory in env::split_paths(&value) {
            // 使用平台路径列表规则拆分每个候选条目。
            let candidate = directory.join("pwsh.exe"); // PowerShell 7 的 Windows 应用名固定。
            if candidate.is_file() {
                return Ok(candidate);
            } // 首个真实普通文件与当前进程搜索顺序一致。
        } // 单个 PATH 变量目录遍历结束。
    } // 全部大小写 PATH 条目遍历结束。
    for candidate in [
        PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe"),
        PathBuf::from(r"C:\Program Files\PowerShell\7-preview\pwsh.exe"),
    ] {
        // 标准安装目录作为无 PATH 环境的有限后备。
        if candidate.is_file() {
            return Ok(candidate);
        } // 不递归扫描磁盘或猜测用户目录。
    } // 标准 PowerShell 7 安装检查结束。
    Err("未找到 PowerShell 7；请安装 pwsh 或把完整路径设置到 BL5_FSR3_PWSH".into()) // 不使用 Windows PowerShell 5.1 改变脚本语义。
} // resolve_powershell_7 结束。

fn runtime_pair_state(runtime: &Path) -> Result<RuntimePairState, String> {
    // 把完全缺失与危险的部分存在状态明确分开。
    let runtime_metadata = match fs::metadata(runtime) {
        // 跟随正常目录链接，但保留具体 I/O 失败。
        Ok(metadata) => metadata, // 已存在路径继续检查其类型与固定文件。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RuntimePairState::Missing);
        } // 新工作树通常尚未创建私有目录。
        Err(error) => {
            return Err(format!(
                "读取 FSR 运行目录失败：{}（{error}）",
                runtime.display()
            ));
        } // 权限或介质错误不能冒充缺失。
    }; // 运行目录元数据检查结束。
    if !runtime_metadata.is_dir() {
        // 专用位置必须保持目录语义。
        return Err(format!(
            "FSR 运行路径不是目录，拒绝自动覆盖：{}",
            runtime.display()
        )); // 固定目录被普通文件占用时要求人工处理。
    } // 运行目录类型检查结束。
    let dll_exists = required_runtime_file_exists(&runtime.join(FILE_NAMES[0]))?; // DLL 必须和自己的 manifest 成对出现。
    let manifest_exists = required_runtime_file_exists(&runtime.join(FILE_NAMES[1]))?; // manifest 不能作为孤立占位文件存在。
    if dll_exists && manifest_exists {
        // 完整文件对不触发重新编译或覆盖。
        return Ok(RuntimePairState::Complete); // 字节身份由随后 inspect 继续核验。
    } // 完整文件对快速路径结束。
    if dll_exists || manifest_exists {
        // 单边存在说明上次中断或人为修改。
        return Err(format!(
            "FSR 运行目录仅部分存在，拒绝自动覆盖：{}；请手动运行 {} 修复或移走 {} 与 {}",
            runtime.display(),
            RUNTIME_BUILD_SCRIPT,
            FILE_NAMES[0],
            FILE_NAMES[1]
        )); // 自动入口保留现场。
    } // 半包拒绝结束。
    let mut entries = fs::read_dir(runtime)
        .map_err(|error| format!("读取 FSR 运行目录失败：{}（{error}）", runtime.display()))?; // 只有真正空目录才等价于完整缺失。
    if entries.next().is_some() {
        // 没有文件对但存在其他内容时不推测所有权。
        return Err(format!(
            "FSR 运行目录缺少固定文件但含有其他内容，拒绝自动覆盖：{}",
            runtime.display()
        )); // 人工确认后可显式运行脚本修复。
    } // 未知内容拒绝结束。
    Ok(RuntimePairState::Missing) // 空专用目录可以由脚本原子发布完整结果。
} // runtime_pair_state 结束。

fn required_runtime_file_exists(path: &Path) -> Result<bool, String> {
    // 固定运行文件必须是可访问的普通文件。
    match fs::metadata(path) {
        // 不使用 exists 吞掉权限和介质错误。
        Ok(metadata) if metadata.is_file() => Ok(true), // 正常文件可进入哈希校验。
        Ok(_) => Err(format!("FSR 运行文件路径不是普通文件：{}", path.display())), // 目录或设备节点都不能作为运行库输入。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false), // 真正缺失交由成对状态判断。
        Err(error) => Err(format!(
            "读取 FSR 运行文件失败：{}（{error}）",
            path.display()
        )), // 其他 I/O 错误必须阻断。
    } // 文件状态判断结束。
} // required_runtime_file_exists 结束。

pub fn validate_manifest(manifest: &serde_json::Value) -> Result<(), String> {
    // 单独校验方便覆盖错误版本、签名和哈希输入。
    if manifest["schema_version"].as_u64() != Some(1) {
        return Err("未知 FSR 修订 manifest schema".into());
    } // 不猜测新协议。
    for (key, expected) in [
        ("sdk_version", "1.1.4"),
        ("revision", REVISION),
        ("official_dll_sha256", OFFICIAL_DLL_SHA256),
        ("license_sha256", LICENSE_SHA256),
        ("authenticode", "NotSigned"),
        ("build_configuration", "Release"),
    ] {
        // 明确承认源码修订产物不具有 AMD 签名。
        if !field(manifest, key)?.eq_ignore_ascii_case(expected) {
            return Err(format!("FSR 修订 manifest 的 {key} 必须为 {expected}"));
        } // 固定来源和构建模式不允许漂移。
    } // 固定字段结束。
    for key in ["patch_sha256", "source_manifest_sha256", "dll_sha256"] {
        // 可变产物身份也必须满足完整 SHA-256 表达。
        let value = field(manifest, key)?; // 缺失或非字符串立即拒绝。
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("FSR 修订 manifest 的 {key} 不是 SHA-256"));
        } // 拒绝空值或缩写。
    } // 可变哈希格式结束。
    for key in ["cmake_version", "generator"] {
        if field(manifest, key)?.trim().is_empty() {
            return Err(format!("FSR 修订 manifest 缺少 {key}"));
        }
    } // 记录实际复现工具，不伪造版本。
    Ok(()) // 字节哈希和长度由 inspect 进一步核验。
} // validate_manifest 结束。

fn field<'a>(manifest: &'a serde_json::Value, key: &str) -> Result<&'a str, String> {
    // 所有字符串字段使用相同拒绝策略。
    manifest[key]
        .as_str()
        .ok_or_else(|| format!("FSR 修订 manifest 缺少字符串字段 {key}")) // 类型错误也不隐式转换。
} // field 结束。

pub fn sha256(path: &Path) -> Result<String, String> {
    // 有界缓冲避免大 DLL 整文件载入。
    let mut file =
        fs::File::open(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?; // 路径纳入诊断。
    let mut hasher = Sha256::new(); // 每项独立计算。
    let mut buffer = vec![0_u8; 1024 * 1024]; // 1 MiB 固定堆预算。
    loop {
        // 直到完整文件 EOF。
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?; // 读取失败不会返回部分哈希。
        if count == 0 {
            break;
        } // 文件结束。
        hasher.update(&buffer[..count]); // 仅处理实际读到的字节。
    } // 文件读取结束。
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()) // 统一大写方便嵌入 Rust 与 C++。
} // sha256 结束。

impl RuntimePackage {
    // 构建与发行共享复制和复验，避免白名单漂移。
    pub fn verify(&self) -> Result<(), String> {
        // 长编译之后仍只接受预检时的同一产物。
        for (path, expected) in &self.files {
            if sha256(path)? != *expected {
                return Err(format!("FSR 预检后文件已改变：{}", path.display()));
            }
        } // 文件被替换时不发布。
        Ok(()) // 快照仍有效。
    } // verify 结束。
    pub fn copy_into(&self, directory: &Path) -> Result<(), String> {
        // 只写调用方拥有的私有运行目录。
        self.verify()?; // 在复制前复验原始文件。
        fs::create_dir_all(directory).map_err(|error| error.to_string())?; // 创建开发或发行专用目录。
        for (name, (source, expected)) in FILE_NAMES.iter().zip(&self.files) {
            // 每项使用固定发布名。
            let target = directory.join(name); // 不允许 manifest 提供任意路径。
            fs::copy(source, &target)
                .map_err(|error| format!("复制 {} 失败：{error}", source.display()))?; // 来源读取错误不吞掉。
            if sha256(&target)? != *expected {
                return Err(format!("复制后的 {} 身份不匹配", target.display()));
            } // 拒绝损坏或并发替换。
        } // 六项复制结束。
        let mut count = 0; // 审计目录既不缺失也不含白名单外文件。
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            // 只审计当前专用目录。
            let entry = entry.map_err(|error| error.to_string())?; // I/O 错误终止发布。
            if !entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_file()
                || !FILE_NAMES.iter().any(|name| entry.file_name() == *name)
            {
                return Err(format!(
                    "FSR 运行目录存在白名单外文件：{}",
                    entry.path().display()
                ));
            } // 不分发隐藏代理或子目录。
            count += 1; // 只记录已通过白名单的普通文件。
        } // 目录条目结束。
        if count != FILE_NAMES.len() {
            return Err("FSR 运行目录缺少身份链文件".into());
        } // 不允许不完整副本。
        Ok(()) // 返回时目录内容与固定清单一致。
    } // copy_into 结束。
} // RuntimePackage 实现结束。

#[cfg(test)] // 由 xtask 测试执行共用来源协议，build.rs 不运行测试。
mod tests {
    // 覆盖缺包构建、安全拒绝与修订身份错误。
    use super::*; // 直接验证生产协议。
    use std::cell::Cell; // 测试通过局部计数确认构建器是否被调用。

    static TEST_ROOT_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0); // 并行测试用进程内序号取得不同目录。

    fn create_test_root(label: &str) -> PathBuf {
        // 每个测试只删除自己成功创建的精确临时根。
        for _ in 0..TEST_DIRECTORY_ATTEMPTS {
            // 遗留目录或进程号复用时有限尝试下一序号。
            let sequence = TEST_ROOT_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed); // 唯一性不承担线程同步语义。
            let root = std::env::temp_dir().join(format!(
                "rpg_fsr3_runtime_{label}_{}_{sequence}",
                std::process::id()
            )); // 名称同时标识测试职责与进程。
            match fs::create_dir(&root) {
                // 原子取得临时根所有权。
                Ok(()) => return root, // 只有成功创建的目录才会在测试末尾递归清理。
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue, // 不碰触上次异常遗留的同名目录。
                Err(error) => panic!("创建测试临时目录失败：{}（{error}）", root.display()), // 其他 I/O 错误不能伪装成冲突。
            } // 单个临时名尝试结束。
        } // 有限临时名搜索结束。
        panic!("测试临时目录冲突超过 {TEST_DIRECTORY_ATTEMPTS} 次"); // 异常环境要求显式失败。
    } // create_test_root 结束。

    fn write_runtime_pair(build_root: &Path) {
        // 小字节夹具只验证构建触发状态，不冒充完整 manifest 身份。
        let runtime = build_root.join("runtime"); // 与生产入口使用同一显式输出下的固定位置。
        fs::create_dir_all(&runtime).expect("应创建模拟运行目录"); // 测试根已独占，可以安全创建父目录。
        fs::write(runtime.join(FILE_NAMES[0]), b"runtime").expect("应写入模拟 DLL"); // 普通文件满足成对状态检查。
        fs::write(runtime.join(FILE_NAMES[1]), b"manifest").expect("应写入模拟 manifest"); // 内容解析不属于本组触发测试。
    } // write_runtime_pair 结束。

    fn valid_manifest() -> serde_json::Value {
        // 构造只用于协议测试的合法结构。
        serde_json::json!({"schema_version": 1, "sdk_version": "1.1.4", "revision": REVISION, "official_dll_sha256": OFFICIAL_DLL_SHA256, "license_sha256": LICENSE_SHA256, "patch_sha256": "A".repeat(64), "source_manifest_sha256": "B".repeat(64), "dll_sha256": "C".repeat(64), "authenticode": "NotSigned", "cmake_version": "test", "generator": "test", "build_configuration": "Release"}) // 字节匹配由 inspect 的实际文件测试覆盖。
    } // valid_manifest 结束。

    #[test]
    fn source_build_defaults_to_cargo_output_and_rejects_relative_overrides() {
        let root = create_test_root("source_output");
        let out_dir = root.join("cargo-output");
        assert_eq!(
            source_build_root(&out_dir, None).expect("Cargo 输出应为绝对路径"),
            out_dir.join("fsr3-source")
        );
        let existing_cache = root.join("existing-cache");
        assert_eq!(
            source_build_root(&out_dir, Some(&existing_cache)).expect("允许显式复用既有缓存"),
            existing_cache
        );
        assert!(source_build_root(&out_dir, Some(Path::new("relative-cache"))).is_err());
        fs::remove_dir_all(&root).expect("应清理测试独占根目录");
    }

    #[test]
    fn unpacked_crate_uses_its_own_script_and_explicit_build_root() {
        // 模拟 registry 独立包布局，复制实际随包脚本；没有 SDK/crates 祖先或主仓。
        let root = create_test_root("unpacked_script");
        let crate_root = root.join("registry cache/feathrise_fsr3_frame_generation-0.1.0");
        let script = crate_root.join(RUNTIME_BUILD_SCRIPT);
        fs::create_dir_all(script.parent().expect("脚本应有父目录")).expect("应创建独立包目录");
        fs::write(&script, include_bytes!("scripts/构建FSR修订运行库.ps1"))
            .expect("应写入实际随包脚本");
        let output = root.join("cargo-output/fsr3-source");
        let sdk = root.join("external/FidelityFX-SDK-v1.1.4");
        let vulkan = root.join("external/Vulkan");
        let patches = root.join("external/fidelityfx-sdk-patches");
        let command = runtime_build_command(
            Path::new("pwsh.exe"),
            &crate_root,
            &output,
            &sdk,
            &vulkan,
            &patches,
        )
        .expect("独立包应能定位自己的脚本");
        let arguments: Vec<_> = command.get_args().collect();
        for (flag, value) in [
            ("-File", script.as_os_str()),
            ("-BuildRoot", output.as_os_str()),
            ("-SdkRoot", sdk.as_os_str()),
            ("-VulkanSdk", vulkan.as_os_str()),
            ("-PatchRoot", patches.as_os_str()),
        ] {
            assert!(
                arguments
                    .windows(2)
                    .any(|pair| { pair[0] == std::ffi::OsStr::new(flag) && pair[1] == value }),
                "应将 {flag} 的空格及中文路径作为独立参数传递"
            );
        }
        assert_eq!(command.get_current_dir(), Some(crate_root.as_path()));
        assert!(!output.exists(), "构造命令不应向源码包或输出写入文件");

        // 即使另有旧 SDK 布局脚本，也不能在当前包缺件后偷偷改用它。
        let legacy = root
            .join("crates/feathrise_fsr3_frame_generation")
            .join(RUNTIME_BUILD_SCRIPT);
        fs::create_dir_all(legacy.parent().expect("旧脚本应有父级")).expect("应创建旧目录夹具");
        fs::rename(&script, &legacy).expect("应把脚本移到旧布局");
        assert!(
            runtime_build_command(
                Path::new("pwsh.exe"),
                &crate_root,
                &output,
                &sdk,
                &vulkan,
                &patches,
            )
            .is_err()
        );
        fs::remove_dir_all(&root).expect("应清理测试独占根目录");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn cargo_target_inside_unpacked_crate_reaches_external_input_preflight() {
        // Cargo 对单独解包包默认使用 <crate>/target；这条真实 PS 回归不能误把它当源码写入。
        let root = create_test_root("package_target");
        let crate_root = root.join("package/feathrise_fsr3_frame_generation-0.1.0");
        let script = crate_root.join(RUNTIME_BUILD_SCRIPT);
        fs::create_dir_all(script.parent().expect("脚本应有父目录")).expect("应创建解包目录");
        fs::write(&script, include_bytes!("scripts/构建FSR修订运行库.ps1"))
            .expect("应复制真实脚本");
        let powershell = resolve_powershell_7().expect("Windows 源码定位回归需要 PowerShell 7");
        let sdk = root.join("external/FidelityFX-SDK-v1.1.4");
        let vulkan = root.join("external/Vulkan");
        let patches = root.join("external/fidelityfx-sdk-patches");
        for (relative, matching_manifest, should_reach_inputs) in [
            ("target/debug/build/feathrise-fsr3-test/out", true, true),
            (
                "custom-cache/debug/build/feathrise-fsr3-test/out",
                true,
                true,
            ),
            ("src/build/feathrise-fsr3-test/out", false, false),
            ("target/non-cargo/out", false, false),
        ] {
            let out_dir = crate_root.join(relative);
            fs::create_dir_all(&out_dir).expect("应建立模拟 Cargo 已创建的 OUT_DIR");
            let build_root = out_dir.join("fsr3-source");
            let mut command = runtime_build_command(
                &powershell,
                &crate_root,
                &build_root,
                &sdk,
                &vulkan,
                &patches,
            )
            .expect("应定位本包脚本");
            let output = command
                .env("OUT_DIR", &out_dir)
                // 只有 Cargo 确认当前包的 OUT_DIR 才能例外；单独伪造输出路径不能授权源码写入。
                .env(
                    "CARGO_MANIFEST_DIR",
                    if matching_manifest {
                        &crate_root
                    } else {
                        &root
                    },
                )
                .output()
                .expect("应执行真实脚本的路径与外部材料预检");
            let detail = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            assert!(!output.status.success(), "本回归没有提供外部 SDK 或补丁");
            let expected = if should_reach_inputs {
                "补丁目录缺少来源材料"
            } else {
                "BuildRoot 必须与包源码目录分离"
            };
            assert!(detail.contains(expected), "{relative}: {detail}");
            assert!(!build_root.exists(), "预检失败不应产生源码副本或运行库");
        }
        // 即使两项 Cargo 环境匹配，也不能把包根或包含包根的祖先当作可重建输出根。
        let ancestor_out = root.join("ancestor-cargo-output");
        let protected_build_root = ancestor_out.join("fsr3-source");
        for suffix in ["", "nested-package"] {
            let protected_crate = protected_build_root.join(suffix);
            let script = protected_crate.join(RUNTIME_BUILD_SCRIPT);
            fs::create_dir_all(script.parent().expect("脚本应有父级")).expect("应创建受保护包");
            fs::write(&script, include_bytes!("scripts/构建FSR修订运行库.ps1"))
                .expect("应复制真实脚本");
            let output = runtime_build_command(
                &powershell,
                &protected_crate,
                &protected_build_root,
                &sdk,
                &vulkan,
                &patches,
            )
            .expect("应定位受保护包的脚本")
            .env("OUT_DIR", &ancestor_out)
            .env("CARGO_MANIFEST_DIR", &protected_crate)
            .output()
            .expect("应执行真实包根保护检查");
            let detail = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            assert!(!output.status.success());
            assert!(detail.contains("不能等于或包含包源码目录"), "{detail}");
        }
        fs::remove_dir_all(&root).expect("应清理测试独占根目录");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn shader_header_path_budget_rejects_long_roots_before_copy_or_compile() {
        // 执行真实脚本的提前预检：少量 accessor 与四个补丁占位已足够，不安装或调用编译器。
        let root = create_test_root("path");
        let crate_root = root.join("crate");
        let script = crate_root.join(RUNTIME_BUILD_SCRIPT);
        fs::create_dir_all(script.parent().expect("脚本应有父级")).expect("应创建包目录");
        fs::write(&script, include_bytes!("scripts/构建FSR修订运行库.ps1"))
            .expect("应复制真实脚本");
        let sdk = root.join("FidelityFX-SDK-v1.1.4");
        let accessors = sdk.join("sdk/src/backends/shared/blob_accessors");
        fs::create_dir_all(&accessors).expect("应创建最小 accessor 夹具");
        let patches = root.join("patches");
        fs::create_dir(&patches).expect("应创建补丁占位目录");
        for name in [
            "LICENSE.txt",
            "README.md",
            "v1.1.4-vulkan-lifetime.patch",
            "source-manifest.json",
        ] {
            fs::write(patches.join(name), b"fixture").expect("应创建存在性预检占位");
        }
        let powershell = resolve_powershell_7().expect("Windows 路径预检需要 PowerShell 7");
        let shader =
            "ffx_frameinterpolation_compute_game_vector_field_inpainting_pyramid_pass_wave64_16bit";
        let header = format!("{shader}_{}.h", "0".repeat(32));
        let projected = |directory: &Path| {
            directory
                .join("build/sdk/src/backends/shaders/vk")
                .join(&header)
                .to_string_lossy()
                .encode_utf16()
                .count()
        };
        let base_length = projected(&root.join("b"));
        assert!(
            base_length < 259,
            "测试独占临时目录过长，无法建立短路径对照"
        );
        for (open, close) in [("<", ">"), ("\"", "\"")] {
            fs::write(
                accessors.join("fixture.cpp"),
                format!("#include {open}{shader}_permutations.h{close}\n"),
            )
            .expect("应覆盖尖括号和引号 include");
            for length in [259, 260, 269] {
                let build_root = root.join("b".repeat(length - base_length + 1));
                assert_eq!(projected(&build_root), length);
                let mut command = runtime_build_command(
                    &powershell,
                    &crate_root,
                    &build_root,
                    &sdk,
                    &root.join("Vulkan"),
                    &patches,
                )
                .expect("应定位真实包内脚本");
                let output = command
                    // 只解析现成可执行文件路径；缺失官方 DLL 会在任何 CMake 执行前停止短路径对照。
                    .arg("-CmakePath")
                    .arg(&powershell)
                    .output()
                    .expect("应执行真实路径预算预检");
                let detail = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                );
                assert!(
                    !output.status.success(),
                    "本回归不提供官方 DLL，不能执行编译"
                );
                if length >= 260 {
                    assert!(detail.contains(&format!("{length} 个字符")), "{detail}");
                    assert!(
                        detail.contains("FEATHRISE_FSR3_SOURCE_BUILD_ROOT"),
                        "{detail}"
                    );
                    assert!(detail.contains("ffx_frameinterpolation"), "{detail}");
                } else {
                    assert!(detail.contains("PrebuiltSignedDLL"), "{detail}");
                    assert!(!detail.contains("FSR 源码构建路径过长"), "{detail}");
                }
                assert!(!build_root.exists(), "提前预检不得复制 SDK 或生成构建目录");
            }
        }
        fs::remove_dir_all(&root).expect("应清理测试独占根目录");
    }

    #[test] // 全新工作树必须只启动一次构建器并观察到成对结果。
    fn missing_runtime_builds_once() {
        // 不启动真实 PowerShell 或 SDK 编译。
        let root = create_test_root("missing"); // 本测试独占的可删除根目录。
        let calls = Cell::new(0_u32); // 记录闭包真实执行次数。
        let built = ensure_runtime_built_with(&root, || {
            // 生产状态机仍负责构建前后检查。
            calls.set(calls.get() + 1); // 本次缺包只能触发一次。
            write_runtime_pair(&root); // 模拟脚本原子发布 DLL 与 manifest。
            Ok(()) // 模拟子进程成功退出。
        })
        .expect("缺包时应成功构建"); // 状态机必须接受完整结果。
        assert!(built); // 返回值向调用方标记本次确实发生首次构建。
        assert_eq!(calls.get(), 1); // 一个入口调用不得重复启动耗时编译。
        fs::remove_dir_all(&root).expect("应清理测试独占根目录"); // 精确删除 create_test_root 的目录。
    } // 缺包构建测试结束。

    #[test] // 已有文件对必须直接复用当前工作树结果。
    fn complete_runtime_skips_builder() {
        // 后续 cargo check 不应重复编译 SDK。
        let root = create_test_root("complete"); // 本测试独占的可删除根目录。
        write_runtime_pair(&root); // 先建立当前工作树完整文件对。
        let calls = Cell::new(0_u32); // 构建器应保持未调用。
        let built = ensure_runtime_built_with(&root, || {
            // 若状态判断错误，计数会暴露回归。
            calls.set(calls.get() + 1); // 不应执行该分支。
            Ok(()) // 保持闭包类型与生产入口一致。
        })
        .expect("完整运行包应直接复用"); // 完整身份仍由调用方随后 inspect。
        assert!(!built); // 返回值明确没有发生新编译。
        assert_eq!(calls.get(), 0); // 不允许重复启动脚本。
        fs::remove_dir_all(&root).expect("应清理测试独占根目录"); // 精确删除 create_test_root 的目录。
    } // 完整运行包复用测试结束。

    #[test] // 半包和未知内容必须保留现场并拒绝自动覆盖。
    fn unsafe_runtime_directories_do_not_start_builder() {
        // 自动编译只处理全空状态。
        for (label, file_name) in [("partial", FILE_NAMES[0]), ("unknown", "foreign.txt")] {
            // 分别覆盖孤立 DLL 与未知文件。
            let root = create_test_root(label); // 每个场景使用独立可删除根目录。
            let runtime = root.join("runtime"); // 使用生产显式输出下的运行目录。
            fs::create_dir_all(&runtime).expect("应创建不安全运行目录"); // 随后写入精确现场。
            fs::write(runtime.join(file_name), b"keep").expect("应写入现场文件"); // 自动入口不得修改该字节。
            let calls = Cell::new(0_u32); // 构建器必须保持未调用。
            let error = ensure_runtime_built_with(&root, || {
                // 任一危险状态都应在闭包前返回。
                calls.set(calls.get() + 1); // 不应执行该分支。
                Ok(()) // 保持闭包签名完整。
            })
            .expect_err("不安全目录必须拒绝自动覆盖"); // 错误由状态审计产生。
            assert!(error.contains("拒绝自动覆盖")); // 诊断必须明确安全边界。
            assert_eq!(calls.get(), 0); // 不允许以脚本覆盖方式掩盖现场。
            assert_eq!(
                fs::read(runtime.join(file_name)).expect("现场文件应保留"),
                b"keep"
            ); // 原有字节不得改变。
            fs::remove_dir_all(&root).expect("应清理测试独占根目录"); // 精确删除 create_test_root 的目录。
        } // 两类危险目录测试结束。
    } // 危险目录拒绝测试结束。

    #[test] // 构建器空成功不能绕过实际文件检查。
    fn builder_success_without_runtime_pair_is_rejected() {
        // 防止脚本提前退出被 Cargo 当成成功。
        let root = create_test_root("empty_success"); // 本测试独占的可删除根目录。
        let error = ensure_runtime_built_with(&root, || Ok(())).expect_err("缺少产物时必须失败"); // 闭包不写任何文件。
        assert!(error.contains("没有生成")); // 诊断应直接指出固定文件对缺失。
        fs::remove_dir_all(&root).expect("应清理测试独占根目录"); // 精确删除 create_test_root 的目录。
    } // 空成功拒绝测试结束。

    #[test] // 源码修订绝不能被表述为 AMD 签名原件。
    fn modified_runtime_cannot_claim_official_signature_or_unknown_revision() {
        // 验证协议关键身份字段。
        let original = valid_manifest(); // 合法基线。
        validate_manifest(&original).expect("完整源码修订身份应通过结构检查"); // 避免反向测试因不相关缺字段而通过。
        for (key, value) in [
            ("authenticode", "Valid"),
            ("revision", "unknown"),
            ("sdk_version", "2.0"),
            ("dll_sha256", "A"),
        ] {
            // 覆盖伪称签名、未知修订、错 SDK 和截断哈希。
            let mut modified = original.clone(); // 每个场景只变更一个字段。
            modified[key] = value.into(); // 模拟损坏或伪造的 manifest。
            assert!(
                validate_manifest(&modified).is_err(),
                "应拒绝无效字段 {key}"
            ); // 每种错误都必须阻止构建。
        } // 错误身份场景结束。
    } // 结构拒绝测试结束。

    #[test] // 长构建之间的替换不能发布为预检时的产物。
    fn release_snapshot_rejects_changed_file_bytes() {
        // 使用小文件验证与 DLL 相同的快照规则。
        let root = create_test_root("snapshot"); // 本测试独占的可删除根目录。
        let path = root.join("runtime.json"); // 文件严格位于测试独占根内。
        fs::write(&path, b"original").expect("应创建预检输入"); // 模拟预检之前的构建记录。
        let package = RuntimePackage {
            files: vec![(path.clone(), sha256(&path).expect("应计算初始身份"))],
        }; // 保存真实字节快照。
        package.verify().expect("未变更输入应通过复验"); // 确认正向路径。
        fs::write(&path, b"replaced").expect("应模拟预检后的输入替换"); // 模拟长编译期间并发替换。
        let result = package.verify(); // 使用原预检身份复验。
        assert!(result.is_err()); // 不允许把替换后的产物装入当前发行包。
        fs::remove_dir_all(&root).expect("应清理测试独占根目录"); // 精确删除 create_test_root 的目录。
    } // 快照拒绝测试结束。
} // tests 结束。
