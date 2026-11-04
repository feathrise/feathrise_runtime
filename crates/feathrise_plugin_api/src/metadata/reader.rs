//! 只读取可执行文件头、节表和有界元数据节；永不LoadLibrary/dlopen。
use super::{MAX_METADATA_BYTES, PluginMetadata, decode_plugin_metadata};
use object::{Architecture, BinaryFormat, Object, ObjectKind, ObjectSection, read::ReadCacheOps};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const MAX_BINARY_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_TOTAL_READ_BYTES: usize = 4 * 1024 * 1024;
const MAX_SECTIONS: usize = 4096;

struct BoundedReader {
    file: File,
    length: u64,
    read: usize,
}

impl ReadCacheOps for BoundedReader {
    fn len(&mut self) -> Result<u64, ()> {
        Ok(self.length)
    }
    fn seek(&mut self, pos: u64) -> Result<u64, ()> {
        if pos > self.length {
            return Err(());
        }
        Seek::seek(&mut self.file, SeekFrom::Start(pos)).map_err(|_| ())
    }
    fn read(&mut self, bytes: &mut [u8]) -> Result<usize, ()> {
        self.budget(bytes.len())?;
        Read::read(&mut self.file, bytes).map_err(|_| ())
    }
    fn read_exact(&mut self, bytes: &mut [u8]) -> Result<(), ()> {
        self.budget(bytes.len())?;
        Read::read_exact(&mut self.file, bytes).map_err(|_| ())
    }
}

impl BoundedReader {
    fn budget(&mut self, length: usize) -> Result<(), ()> {
        // ReadCache在分配前调用读操作，但还会按请求长度分配；外层ReadRef另行截断单次请求。
        if length > MAX_READ_BYTES || self.read.saturating_add(length) > MAX_TOTAL_READ_BYTES {
            return Err(());
        }
        self.read += length;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct LimitedCache<'a>(&'a object::read::ReadCache<BoundedReader>);

impl<'a> object::read::ReadRef<'a> for LimitedCache<'a> {
    fn len(self) -> Result<u64, ()> {
        object::read::ReadRef::len(self.0)
    }
    fn read_bytes_at(self, offset: u64, size: u64) -> Result<&'a [u8], ()> {
        // 必须在ReadCache分配前拒绝伪造的大节表/字符串表，不能只在底层read时检查。
        if size > MAX_READ_BYTES as u64 {
            return Err(());
        }
        object::read::ReadRef::read_bytes_at(self.0, offset, size)
    }
    fn read_bytes_at_until(
        self,
        range: std::ops::Range<u64>,
        delimiter: u8,
    ) -> Result<&'a [u8], ()> {
        let end = range.start.checked_add(4096).ok_or(())?.min(range.end);
        object::read::ReadRef::read_bytes_at_until(self.0, range.start..end, delimiter)
    }
}

pub fn read_plugin_metadata(path: &Path) -> Result<PluginMetadata, String> {
    let file = File::open(path).map_err(|error| format!("无法读取插件文件：{error}"))?;
    let info = file.metadata().map_err(|error| error.to_string())?;
    if !info.is_file() || info.len() == 0 || info.len() > MAX_BINARY_BYTES {
        return Err("插件必须是非空普通文件，且不超过1GiB".into());
    }
    let cache = object::read::ReadCache::new(BoundedReader {
        file,
        length: info.len(),
        read: 0,
    });
    let object = object::File::parse(LimitedCache(&cache))
        .map_err(|error| format!("插件二进制文件头/节表无效或超出读取预算：{error}"))?;
    let target_os = match object.format() {
        BinaryFormat::Pe => "windows",
        BinaryFormat::Elf => "linux",
        BinaryFormat::MachO => "macos",
        _ => return Err("插件只接受PE DLL、ELF SO或Mach-O DYLIB".into()),
    };
    if object.kind() != ObjectKind::Dynamic || !object.is_little_endian() {
        return Err("插件必须是受支持的小端共享动态库，不能安装可执行程序或目标文件".into());
    }
    let target_arch = match object.architecture() {
        Architecture::X86_64 => "x86_64",
        Architecture::Aarch64 => "aarch64",
        Architecture::I386 => "x86",
        Architecture::Arm => "arm",
        _ => return Err("插件二进制CPU架构不受支持".into()),
    };
    let expected_suffix = match target_os {
        "windows" => "dll",
        "linux" => "so",
        _ => "dylib",
    };
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case(expected_suffix))
    {
        return Err(format!(
            "插件二进制格式与扩展名不匹配，应使用.{expected_suffix}"
        ));
    }
    let mut metadata = None;
    for (index, section) in object.sections().enumerate() {
        if index >= MAX_SECTIONS {
            return Err("插件节数量超过4096项".into());
        }
        let name = section
            .name()
            .map_err(|error| format!("插件节名无效：{error}"))?;
        let is_metadata = if target_os == "macos" {
            name == "__frmeta"
                && section.segment_name().map_err(|e| e.to_string())? == Some("__DATA")
        } else {
            name == ".frmeta"
        };
        if !is_metadata {
            continue;
        }
        if metadata.is_some() {
            return Err("插件存在重复元数据节".into());
        }
        if section.size() > (MAX_METADATA_BYTES + 4095) as u64 {
            return Err("插件元数据节超过64KiB预算".into());
        }
        let bytes = section
            .data()
            .map_err(|error| format!("插件元数据节截断：{error}"))?;
        metadata = Some(decode_plugin_metadata(bytes)?);
    }
    let metadata = metadata.ok_or("插件缺少静态元数据节；旧目录清单包需要重新构建")?;
    if metadata.target_os != target_os || metadata.target_arch != target_arch {
        return Err(format!(
            "插件元数据声明{}-{}，实际二进制为{target_os}-{target_arch}",
            metadata.target_os, metadata.target_arch
        ));
    }
    Ok(metadata)
}
