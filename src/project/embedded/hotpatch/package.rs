//! 后台核验冻结补丁包和DLL：固定字节预算、完整身份及只读租约，不在帧上读取文件。
use super::types::{Request, Target};
use serde::{Deserialize, de::{self, MapAccess, Visitor}};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt, fs::{self, File, OpenOptions}, io::Read,
    path::{Component, Path, PathBuf}, sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}}};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

const MAX_PACKAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LIBRARY_BYTES: u64 = 512 * 1024 * 1024;
const REPARSE_POINT: u32 = 0x400;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const BACKUP_SEMANTICS: u32 = 0x0200_0000;

pub(super) struct RootLease {
    pub path: PathBuf,
    _directory: File,
}

impl RootLease {
    pub fn open(path: &Path) -> Result<Arc<Self>, String> {
        if !path.is_absolute() { return Err("代码补丁目录必须是显式绝对路径".into()); }
        no_reparse(path)?;
        let path = fs::canonicalize(path).map_err(|error| format!("代码补丁目录不可用：{error}"))?;
        let directory = open_directory(&path)?;
        Ok(Arc::new(Self { path, _directory: directory }))
    }
}

pub(super) struct Verified {
    pub table: subsecond::JumpTable,
    // 包、DLL与沿途目录都不得在校验至装载之间被写入或重命名；旧DLL另由Subsecond保活。
    _leases: Vec<File>,
    _root: Arc<RootLease>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Package {
    schema: u32,
    target: Target,
    jump_table: Table,
    dll_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    lib: String,
    #[serde(deserialize_with = "addresses")]
    map: BTreeMap<u64, u64>,
    aslr_reference: u64,
    new_base_address: u64,
    ifunc_count: u64,
}

pub(super) fn verify(
    root: Arc<RootLease>, request: &Request, stop: &AtomicBool, generation: &AtomicU64,
) -> Result<Verified, String> {
    current(request, stop, generation)?;
    let mut leases = Vec::new();
    let mut package_file = open_member(&root, Path::new(&request.artifact), &mut leases)?;
    if package_file.metadata().map_err(|error| error.to_string())?.len() > MAX_PACKAGE_BYTES {
        return Err("代码补丁包超过8MiB预算".into());
    }
    let mut bytes = Vec::new();
    (&mut package_file).take(MAX_PACKAGE_BYTES + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_PACKAGE_BYTES || !sha256(&bytes).eq_ignore_ascii_case(&request.sha256) {
        return Err("代码补丁包大小或SHA256不匹配".into());
    }
    let package: Package = serde_json::from_slice(&bytes).map_err(|error| format!("代码补丁包格式无效：{error}"))?;
    if package.schema != 1 || !package.target.matches(request) {
        return Err("代码补丁包不属于当前工程、会话、World、基线或源码事务".into());
    }
    let table = package.jump_table;
    if table.map.is_empty() || table.aslr_reference != request.aslr_reference || table.new_base_address == 0 || table.ifunc_count != 0
        || table.map.get(&table.aslr_reference) != Some(&table.new_base_address) {
        return Err("代码补丁跳表缺少一致的原生main入口".into());
    }
    let library = Path::new(&table.lib);
    if !library.is_absolute() { return Err("代码补丁DLL必须是受控目录内的绝对路径".into()); }
    // Windows普通盘符路径与canonical的verbatim前缀可能不同；装载也使用受检的同一真实路径。
    let library = fs::canonicalize(library).map_err(|error| format!("代码补丁DLL不可用：{error}"))?;
    let relative = library.strip_prefix(&root.path).map_err(|_| "代码补丁DLL位于本轮受控目录之外")?;
    let mut library_file = open_member(&root, relative, &mut leases)?;
    let length = library_file.metadata().map_err(|error| error.to_string())?.len();
    if length == 0 || length > MAX_LIBRARY_BYTES { return Err("代码补丁DLL为空或超过512MiB预算".into()); }
    let mut hasher = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        current(request, stop, generation)?;
        let read = library_file.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 { break; }
        hasher.update(&chunk[..read]);
    }
    if !hex(&hasher.finalize()).eq_ignore_ascii_case(&package.dll_sha256) {
        return Err("代码补丁DLL的SHA256不匹配".into());
    }
    current(request, stop, generation)?;
    leases.push(package_file); leases.push(library_file);
    Ok(Verified { table: subsecond::JumpTable { lib: library, map: table.map.into_iter().collect(),
        aslr_reference: table.aslr_reference, new_base_address: table.new_base_address, ifunc_count: 0 },
        _leases: leases, _root: root })
}

pub(super) fn current(request: &Request, stop: &AtomicBool, generation: &AtomicU64) -> Result<(), String> {
    if stop.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != request.map_generation {
        return Err("代码补丁采用前会话已停止或地图代次已变化".into());
    }
    Ok(())
}

fn no_reparse(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| format!("代码补丁路径不可读：{error}"))?;
    if metadata.file_type().is_symlink() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err("代码补丁路径不接受符号链接或重解析点".into());
    }
    Ok(())
}

fn open_directory(path: &Path) -> Result<File, String> {
    no_reparse(path)?;
    let file = OpenOptions::new().read(true).share_mode(0x1 | 0x2)
        .custom_flags(BACKUP_SEMANTICS | OPEN_REPARSE_POINT).open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err("代码补丁目录租约不是普通目录".into());
    }
    Ok(file)
}

fn open_member(root: &RootLease, relative: &Path, leases: &mut Vec<File>) -> Result<File, String> {
    let text = relative.to_str().ok_or("代码补丁路径不是UTF-8")?;
    if relative.is_absolute() || text.is_empty() || text.len() > 2048 || text.chars().any(char::is_control)
        || text.split(['/', '\\']).any(|part| part.is_empty() || matches!(part, "." | "..")) {
        return Err("代码补丁产物路径必须是无跳转的受控相对路径".into());
    }
    let components: Vec<_> = relative.components().collect();
    let mut path = root.path.clone();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else { return Err("代码补丁路径包含非法前缀".into()); };
        let name_text = name.to_str().ok_or("代码补丁路径不是UTF-8")?;
        if name_text.ends_with([' ', '.']) || name_text.contains(['<', '>', ':', '"', '|', '?', '*']) {
            return Err("代码补丁产物包含非法Windows文件名".into());
        }
        path.push(name);
        if index + 1 != components.len() { leases.push(open_directory(&path)?); }
    }
    no_reparse(&path)?;
    let file = OpenOptions::new().read(true).share_mode(0x1).custom_flags(OPEN_REPARSE_POINT)
        .open(&path).map_err(|error| format!("不能取得代码补丁只读租约：{error}"))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err("代码补丁产物不是普通文件".into());
    }
    Ok(file)
}

fn sha256(bytes: &[u8]) -> String { hex(&Sha256::digest(bytes)) }
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes { let _ = write!(&mut value, "{byte:02x}"); }
    value
}

fn addresses<'de, D: de::Deserializer<'de>>(deserializer: D) -> Result<BTreeMap<u64, u64>, D::Error> {
    struct Addresses;
    impl<'de> Visitor<'de> for Addresses {
        type Value = BTreeMap<u64, u64>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("不重复的u64地址映射") }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut values = BTreeMap::new();
            while let Some(key) = map.next_key::<String>()? {
                let source = key.parse::<u64>().map_err(de::Error::custom)?;
                if source.to_string() != key || values.insert(source, map.next_value::<u64>()?).is_some() {
                    return Err(de::Error::custom("跳表地址不是规范u64或重复"));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(Addresses)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_table_rejects_duplicate_addresses_non_integer_and_unknown_fields() {
        for map in [r#"{"1":2,"1":3}"#, r#"{"1":1.5}"#, r#"{"01":2}"#] {
            let input = format!(r#"{{"lib":"x","map":{map},"aslr_reference":1,"new_base_address":2,"ifunc_count":0}}"#);
            assert!(serde_json::from_str::<Table>(&input).is_err());
        }
    }
    #[test]
    fn package_target_requires_every_actual_runtime_and_source_identity() {
        let request = Request { patch_id: 1, source_generation: 2, base_build: "a".repeat(64),
            artifact: "p.json".into(), sha256: "b".repeat(64), project_id: "fixture".into(), session: 3,
            map_generation: 4, pid: 5, world_id: u64::MAX, aslr_reference: 6 };
        let mut target = Target { project_id: request.project_id.clone(), session: 3, pid: 5,
            world_id: u64::MAX, base_build: request.base_build.clone(), source_generation: 2, patch_id: 1 };
        assert!(target.matches(&request));
        target.session += 1; assert!(!target.matches(&request));
        target.session -= 1; target.source_generation += 1; assert!(!target.matches(&request));
    }

    #[test]
    fn verified_package_leases_prevent_tampering_and_reject_changed_generation_or_bytes() {
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("feathrise-patch-lease-{}-{unique}",std::process::id()));
        fs::create_dir(&directory).unwrap();
        let library = directory.join("fixture.dll");
        let package_path = directory.join("package.json");
        fs::write(&library,b"owned-dll-fixture").unwrap();
        let mut request = Request { patch_id: 7, source_generation: 11, base_build: "a".repeat(64),
            artifact: "package.json".into(), sha256: String::new(), project_id: "owned-fixture".into(),
            session: 13, map_generation: 17, pid: std::process::id(), world_id: 19, aslr_reference: 23 };
        let package = serde_json::to_vec(&serde_json::json!({"schema":1,"target":{
            "project_id":request.project_id,"session":request.session,"pid":request.pid,
            "world_id":request.world_id,"base_build":request.base_build,
            "source_generation":request.source_generation,"patch_id":request.patch_id},
            "jump_table":{"lib":library,"map":{"23":29},"aslr_reference":23,"new_base_address":29,"ifunc_count":0},
            "dll_sha256":sha256(b"owned-dll-fixture")})).unwrap();
        fs::write(&package_path,&package).unwrap(); request.sha256 = sha256(&package);
        let root = RootLease::open(&directory).unwrap();
        let stop = AtomicBool::new(false); let generation = AtomicU64::new(17);
        let verified = verify(root.clone(),&request,&stop,&generation).unwrap();
        assert_eq!(verified.table.map.get(&23),Some(&29));
        assert!(OpenOptions::new().write(true).open(&library).is_err(),"DLL核验至装载之间不能写入");
        assert!(fs::rename(&package_path,directory.join("replaced.json")).is_err(),"核验中的包不能被替换");
        drop(verified);
        fs::write(&library,b"changed").unwrap();
        assert!(verify(root.clone(),&request,&stop,&generation).is_err(),"DLL摘要不符应保留旧代码");
        generation.store(18,Ordering::Release);
        assert!(current(&request,&stop,&generation).is_err());
        generation.store(17,Ordering::Release); stop.store(true,Ordering::Release);
        assert!(current(&request,&stop,&generation).is_err());
        drop(root);
        // 只移除本测试明确创建的两个文件和空目录，不递归处理用户目录。
        fs::remove_file(&library).unwrap(); fs::remove_file(&package_path).unwrap(); fs::remove_dir(directory).unwrap();
    }
}
