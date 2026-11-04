//! 用真实预编译包验证普通消费、缓存并发与拒绝路径；不修改全局环境或启动 SDK 构建。

use super::*;
use serde_json::Value;
use sha2::Digest as _;
use std::{
    io::{Read, Write},
    sync::Barrier,
    thread,
};

/// 临时根只通过独占 create_dir 取得，回收前再次核对规范路径，避免误删陌生目录。
struct TestDirectory {
    path: PathBuf,
    canonical: PathBuf,
    parent: PathBuf,
}

impl TestDirectory {
    fn new() -> Self {
        let parent = fs::canonicalize(env::temp_dir()).expect("应解析测试临时目录");
        for _ in 0..128 {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!(
                "feathrise-fsr3-native-test-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    let canonical = fs::canonicalize(&path).expect("应解析新建的专属测试根");
                    assert_eq!(canonical.parent(), Some(parent.as_path()));
                    return Self {
                        path,
                        canonical,
                        parent,
                    };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("无法创建专属测试目录：{error}"),
            }
        }
        panic!("专属测试目录碰撞超过固定次数");
    }

    fn options(&self) -> Options {
        Options {
            cache: self.path.join("cache"),
            local: Some(configured_archive()),
        }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let Ok(actual) = fs::canonicalize(&self.path) else {
            return;
        };
        let regular_directory = fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
        if regular_directory
            && actual == self.canonical
            && actual.parent() == Some(self.parent.as_path())
        {
            let _ = fs::remove_dir_all(&actual);
        }
    }
}

fn crate_root() -> PathBuf {
    // 单测由外部 xtask 通过 #[path] 编译；按真实源码位置定位 SDK，避免绑定游戏目录层级。
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find_map(|root| {
            let source = root.join("feathrise_runtime/crates/feathrise_fsr3_frame_generation");
            source
                .join("native_package/tests.rs")
                .is_file()
                .then_some(source)
        })
        .expect("获取器测试应能定位真实 SDK 源码")
}

fn lock(root: &Path) -> manifest::Lock {
    manifest::Lock::read(root, TARGET).expect("仓库预编译包锁应有效")
}

fn configured_archive() -> PathBuf {
    PathBuf::from(
        env::var_os("FEATHRISE_FSR3_PACKAGE").expect("测试须显式提供真实 Windows FSR ZIP"),
    )
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("应读取测试 JSON")).expect("测试 JSON 应有效")
}

fn write_json(path: &Path, value: &Value) {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("应编码测试 JSON"),
    )
    .expect("应写入测试 JSON");
}

fn fixture(directory: &TestDirectory) -> PathBuf {
    let source = crate_root();
    let root = directory.path.join("crate");
    fs::create_dir(&root).expect("应创建测试 crate 根");
    fs::create_dir(root.join("prebuilt")).expect("应创建测试预编译目录");
    for name in SOURCE_FILES {
        let destination = root.join(name);
        fs::create_dir_all(destination.parent().expect("源码应有父目录"))
            .expect("应创建测试源码目录");
        fs::copy(source.join(name), destination).expect("应复制真实桥接源码");
    }
    fs::copy(
        source.join("prebuilt/package.lock.json"),
        root.join("prebuilt/package.lock.json"),
    )
    .expect("应复制可信锁");
    root
}

fn assert_not_published(options: &Options, identity: &str) {
    let root = options.cache.join("fsr3-fg");
    assert!(!root.join(identity).exists(), "失败包不能发布为完整缓存");
    if root.exists() {
        assert!(
            fs::read_dir(root).expect("应能读取专属缓存").all(|entry| {
                !entry
                    .expect("应能读取缓存目录项")
                    .file_name()
                    .to_string_lossy()
                    .contains(".staging-")
            }),
            "失败获取必须回收本次暂存目录"
        );
    }
}

fn entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path).expect("应打开测试 ZIP"))
        .expect("真实 ZIP 应有效");
    (0..archive.len())
        .map(|index| {
            let mut entry = archive.by_index(index).expect("应读取 ZIP 条目");
            let name = entry.name().to_owned();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).expect("应读取完整 ZIP 条目");
            (name, bytes)
        })
        .collect()
}

fn write_archive(path: &Path, entries: &[(String, Vec<u8>)]) {
    let mut archive = zip::ZipWriter::new(fs::File::create(path).expect("应创建测试 ZIP"));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in entries {
        archive
            .start_file(name, options)
            .expect("应开始测试 ZIP 条目");
        archive.write_all(bytes).expect("应写入测试 ZIP 条目");
    }
    archive.finish().expect("应完成测试 ZIP");
}

#[test]
fn source_inspection_uses_only_the_explicit_external_patch_directory() {
    // 使用真实包建立来源链；任意目录布局均可，不依赖 SDK 内或私有父目录的副本。
    let directory = TestDirectory::new();
    let root = crate_root();
    let native = acquire_with(&root, TARGET, &directory.options()).expect("应验证真实原生包");
    let workspace = directory.path.join("sdk");
    let runtime = workspace.join(".local/fsr3-sdk-patched/runtime");
    let patches = directory.path.join("external/native-inputs");
    fs::create_dir_all(&runtime).expect("应创建源码产物目录");
    fs::create_dir_all(&patches).expect("应创建外部补丁目录");
    for (index, (source, _)) in native.runtime.files.iter().enumerate() {
        let target = match index {
            0 | 1 => runtime.join(runtime_package::FILE_NAMES[index]),
            2 => patches.join("LICENSE.txt"),
            _ => patches.join(runtime_package::FILE_NAMES[index]),
        };
        fs::copy(source, target).expect("应复制真实来源链输入");
    }
    let inspected =
        runtime_package::inspect(&workspace, &patches).expect("应从显式材料目录完成预检");
    for ((_, actual), (_, expected)) in inspected.files.iter().zip(&native.runtime.files) {
        assert_eq!(actual, expected, "外部材料与真实原生包必须具有同一身份");
    }
    let legacy = workspace.join("third_party/fidelityfx-sdk-patches");
    fs::create_dir_all(legacy.parent().expect("旧目录应有父级")).expect("应创建旧目录夹具");
    fs::rename(&patches, &legacy).expect("应模拟材料只残留在旧位置");
    assert!(
        runtime_package::inspect(&workspace, &patches).is_err(),
        "显式来源缺失时不能回退旧目录"
    );
    fs::rename(&legacy, &patches).expect("应恢复显式来源");
    fs::write(
        patches.join(runtime_package::FILE_NAMES[3]),
        b"changed patch",
    )
    .expect("应模拟外部补丁变更");
    let error = runtime_package::inspect(&workspace, &patches).expect_err("补丁变更必须拒绝");
    assert!(error.contains("patch_sha256"), "{error}");
}

#[test]
fn explicit_package_works_from_cold_cache_and_reuses_verified_cache() {
    let directory = TestDirectory::new();
    let options = directory.options();
    let root = crate_root();
    let first = acquire_with(&root, TARGET, &options).expect("冷缓存应直接使用显式 ZIP");
    assert_eq!(first.identity, lock(&root).sha256);
    assert_eq!(first.runtime.files.len(), runtime_package::FILE_NAMES.len());
    assert!(first.bridge_library.is_file());
    for ((path, _), name) in first.runtime.files.iter().zip(runtime_package::FILE_NAMES) {
        assert_eq!(path.file_name().and_then(|name| name.to_str()), Some(name));
    }

    let second = acquire_with(&root, TARGET, &options).expect("已验证缓存应支持离线复用");
    assert_eq!(second.bridge_library, first.bridge_library);
    assert_eq!(second.identity, first.identity);
}

#[test]
fn missing_package_never_uses_a_bundled_archive_or_warm_cache() {
    let directory = TestDirectory::new();
    let root = fixture(&directory);
    fs::copy(
        configured_archive(),
        root.join("prebuilt").join(lock(&root).archive),
    )
    .expect("模拟旧版遗留在源码树的 ZIP");
    let mut options = directory.options();
    options.local = None;
    let error = acquire_with(&root, TARGET, &options).expect_err("本地 ZIP 路径必须显式提供");
    assert!(error.contains("FEATHRISE_FSR3_PACKAGE"), "{error}");
    assert!(!options.cache.exists());

    options.local = Some(configured_archive());
    acquire_with(&root, TARGET, &options).expect("显式包应能准备缓存");
    options.local = None;
    assert!(acquire_with(&root, TARGET, &options).is_err());
    options.local = Some(directory.path.join("missing.zip"));
    assert!(acquire_with(&root, TARGET, &options).is_err());
}

#[test]
fn concurrent_consumers_publish_one_complete_cache() {
    let directory = TestDirectory::new();
    let options = directory.options();
    let root = crate_root();
    let barrier = Barrier::new(4);
    let paths = thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    acquire_with(&root, TARGET, &options)
                        .expect("共享冷缓存的并发消费者应全部取得完整包")
                        .bridge_library
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("消费者线程不能崩溃"))
            .collect::<Vec<_>>()
    });
    assert!(paths.iter().all(|path| path == &paths[0]));
    let children: Vec<_> = fs::read_dir(options.cache.join("fsr3-fg"))
        .expect("应创建共享缓存")
        .map(|entry| entry.expect("目录项应有效").file_name())
        .collect();
    assert_eq!(
        children.len(),
        2,
        "只允许完整包目录与同身份锁文件，不能遗留暂存包"
    );
}

#[test]
fn cached_dll_bridge_and_native_manifest_tampering_is_never_repaired_silently() {
    for name in [
        runtime_package::FILE_NAMES[0],
        manifest::BRIDGE,
        manifest::NATIVE_MANIFEST,
    ] {
        let directory = TestDirectory::new();
        let options = directory.options();
        let root = crate_root();
        let package = acquire_with(&root, TARGET, &options).expect("应先取得真实完整缓存");
        let changed = package
            .bridge_library
            .parent()
            .expect("桥接库应在包根目录")
            .join(name);
        let mut bytes = fs::read(&changed).expect("应读取待篡改测试文件");
        bytes[0] ^= 1;
        fs::write(&changed, &bytes).expect("应修改测试缓存");
        let error = acquire_with(&root, TARGET, &options).expect_err("篡改缓存必须明确拒绝");
        assert!(
            error.contains("缓存校验失败") && error.contains("SHA-256"),
            "{error}"
        );
        assert_eq!(
            fs::read(changed).expect("应保留损坏现场"),
            bytes,
            "不能静默覆盖损坏现场"
        );
    }
}

#[test]
fn source_line_endings_are_normalized_but_changed_source_invalidates_warm_cache() {
    let directory = TestDirectory::new();
    let root = fixture(&directory);
    let source = root.join(SOURCE_FILES[0]);
    let original = fs::read_to_string(&source).expect("桥接源码应是 UTF-8");
    fs::write(
        &source,
        original.replace("\r\n", "\n").replace('\n', "\r\n"),
    )
    .expect("应构造 CRLF 源码副本");
    let options = directory.options();
    acquire_with(&root, TARGET, &options).expect("Git 换行差异不能使相同源码失效");
    fs::write(&source, format!("{original}\n// 测试真实接口变化\n")).expect("应构造源码变化");
    let error = acquire_with(&root, TARGET, &options).expect_err("旧 ABI 缓存不能配合变化后的源码");
    assert!(
        error.contains(SOURCE_FILES[0]) && error.contains("旧 ABI"),
        "{error}"
    );
}

#[test]
fn unsupported_target_and_lock_crt_fail_before_cache_creation() {
    let directory = TestDirectory::new();
    let options = directory.options();
    acquire_with(&crate_root(), "aarch64-pc-windows-msvc", &options)
        .expect_err("不得把 x64 桥接用于其他目标");
    assert!(!options.cache.exists());

    let root = fixture(&directory);
    let path = root.join("prebuilt/package.lock.json");
    let mut value = json(&path);
    value["crt"] = Value::String("static".into());
    write_json(&path, &value);
    let error = acquire_with(&root, TARGET, &options).expect_err("不能使用静态 CRT 的可信锁");
    assert!(error.contains("dynamic CRT"));
    assert!(!options.cache.exists());
}

#[test]
fn wrong_bridge_abi_is_rejected_even_when_fixture_archive_and_manifest_are_trusted() {
    let directory = TestDirectory::new();
    let root = fixture(&directory);
    let path = directory.path.join("wrong-abi.zip");
    fs::copy(configured_archive(), &path).expect("应复制真实测试 ZIP");
    let mut contents = entries(&path);
    let native = contents
        .iter_mut()
        .find(|(name, _)| name == manifest::NATIVE_MANIFEST)
        .expect("真实包应带原生清单");
    let mut value: Value = serde_json::from_slice(&native.1).expect("原生清单应有效");
    value["bridge_abi"] = Value::from(2);
    native.1 = serde_json::to_vec_pretty(&value).expect("应编码错误 ABI 清单");
    let manifest_hash: String = sha2::Sha256::digest(&native.1)
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect();
    write_archive(&path, &contents);
    let lock_path = root.join("prebuilt/package.lock.json");
    let mut trusted = json(&lock_path);
    trusted["sha256"] = Value::String(runtime_package::sha256(&path).expect("应计算测试 ZIP 哈希"));
    trusted["size"] = Value::from(fs::metadata(&path).expect("应读取测试 ZIP 长度").len());
    trusted["native_manifest_sha256"] = Value::String(manifest_hash);
    write_json(&lock_path, &trusted);
    let mut options = directory.options();
    options.local = Some(path);
    let error =
        acquire_with(&root, TARGET, &options).expect_err("可信哈希不能代替桥接 ABI 协议检查");
    assert!(error.contains("桥接 ABI"), "{error}");
    assert_not_published(&options, &lock(&root).sha256);
}

#[test]
fn corrupted_or_truncated_zip_never_publishes_partial_cache() {
    let root = crate_root();
    let original = fs::read(configured_archive()).expect("应读取真实归档");
    for truncated in [false, true] {
        let directory = TestDirectory::new();
        let mut bytes = original.clone();
        if truncated {
            bytes.pop();
        } else {
            bytes[0] ^= 1;
        }
        let path = directory.path.join("invalid.zip");
        fs::write(&path, bytes).expect("应写入损坏测试 ZIP");
        let mut options = directory.options();
        options.local = Some(path);
        let error = acquire_with(&root, TARGET, &options).expect_err("损坏或截断归档必须失败");
        assert!(
            error.contains(if truncated { "长度" } else { "SHA-256" }),
            "{error}"
        );
        assert_not_published(&options, &lock(&root).sha256);
    }
}

#[test]
fn extraction_rejects_unsafe_paths_physical_duplicates_and_expansion_over_budget() {
    let base: Vec<_> = manifest::names()
        .map(|name| (name.to_owned(), Vec::new()))
        .collect();
    for scenario in ["path", "duplicate", "oversize"] {
        let directory = TestDirectory::new();
        let path = directory.path.join("invalid.zip");
        let mut contents = base.clone();
        if scenario == "path" {
            contents[0].0 = "../outside.dll".into();
        } else if scenario == "duplicate" {
            // ZipWriter 主动拒绝重名；先写同长度占位条目，再修改两处 ZIP 文件名。
            contents.push(("FAKE00.md".into(), Vec::new()));
        }
        write_archive(&path, &contents);
        if scenario == "duplicate" {
            let mut bytes = fs::read(&path).expect("应读取重复条目测试 ZIP");
            let positions: Vec<_> = bytes
                .windows(9)
                .enumerate()
                .filter_map(|(index, window)| (window == b"FAKE00.md").then_some(index))
                .collect();
            assert_eq!(positions.len(), 2, "占位文件名应同时在本地头和中央目录出现");
            for position in positions {
                bytes[position..position + 9].copy_from_slice(b"README.md");
            }
            fs::write(&path, bytes).expect("应写入物理重复条目");
        } else if scenario == "oversize" {
            // 仅扩大中央目录声明即可在解压前测试预算，避免测试本身分配大块内容。
            let mut bytes = fs::read(&path).expect("应读取超预算测试 ZIP");
            let position = bytes
                .windows(4)
                .position(|window| window == b"PK\x01\x02")
                .expect("应存在中央目录记录");
            bytes[position + 24..position + 28]
                .copy_from_slice(&((manifest::MAX_EXPANDED + 1) as u32).to_le_bytes());
            fs::write(&path, bytes).expect("应写入超预算声明");
        }
        let destination = directory.path.join("payload");
        fs::create_dir(&destination).expect("应创建专属解压目录");
        transport::extract(&path, &destination).expect_err(&format!("必须拒绝 {scenario} ZIP"));
        assert!(
            !directory.path.join("outside.dll").exists(),
            "不允许 ZIP 逃出解压根"
        );
    }
}
