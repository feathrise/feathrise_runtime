//! 静态读取器专用夹具：真实共享库文件头和元数据节，无可执行入口，禁止拿它证明DLL加载成功。
use super::{PluginMetadata, encode_plugin_metadata};
use std::path::Path;

fn u16_at(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn u32_at(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn u64_at(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub fn write_metadata_fixture(path: &Path, metadata: &PluginMetadata) -> Result<(), String> {
    let payload = encode_plugin_metadata(metadata)?;
    let arm = match metadata.target_arch.as_str() {
        "x86_64" => false,
        "aarch64" => true,
        _ => return Err("静态测试夹具仅生成x86_64/aarch64头".into()),
    };
    let bytes = match metadata.target_os.as_str() {
        "windows" => pe(&payload, arm),
        "linux" => elf(&payload, arm),
        "macos" => macho(&payload, arm),
        _ => return Err("静态测试夹具平台无效".into()),
    };
    std::fs::write(path, bytes).map_err(|error| error.to_string())
}

fn pe(payload: &[u8], arm: bool) -> Vec<u8> {
    let raw_size = payload.len().div_ceil(512) * 512;
    let mut bytes = vec![0; 512 + raw_size];
    bytes[..2].copy_from_slice(b"MZ");
    u32_at(&mut bytes, 60, 128);
    bytes[128..132].copy_from_slice(b"PE\0\0");
    u16_at(&mut bytes, 132, if arm { 0xaa64 } else { 0x8664 });
    u16_at(&mut bytes, 134, 1);
    u16_at(&mut bytes, 148, 240);
    u16_at(&mut bytes, 150, 0x2022); // DLL + executable image + large-address-aware。
    u16_at(&mut bytes, 152, 0x20b);
    u64_at(&mut bytes, 176, 0x180000000);
    u32_at(&mut bytes, 184, 4096);
    u32_at(&mut bytes, 188, 512);
    u32_at(&mut bytes, 208, 8192);
    u32_at(&mut bytes, 212, 512);
    u32_at(&mut bytes, 260, 16);
    let section = 392;
    bytes[section..section + 7].copy_from_slice(b".frmeta");
    u32_at(&mut bytes, section + 8, payload.len() as u32);
    u32_at(&mut bytes, section + 12, 4096);
    u32_at(&mut bytes, section + 16, raw_size as u32);
    u32_at(&mut bytes, section + 20, 512);
    u32_at(&mut bytes, section + 36, 0x40000040);
    bytes[512..512 + payload.len()].copy_from_slice(payload);
    bytes
}

fn elf(payload: &[u8], arm: bool) -> Vec<u8> {
    let names = b"\0.shstrtab\0.frmeta\0";
    let data = (256 + names.len()).next_multiple_of(8);
    let mut bytes = vec![0; data + payload.len()];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    u16_at(&mut bytes, 16, 3); // ET_DYN，共享库。
    u16_at(&mut bytes, 18, if arm { 183 } else { 62 });
    u32_at(&mut bytes, 20, 1);
    u64_at(&mut bytes, 40, 64);
    u16_at(&mut bytes, 52, 64);
    u16_at(&mut bytes, 58, 64);
    u16_at(&mut bytes, 60, 3);
    u16_at(&mut bytes, 62, 1);
    u32_at(&mut bytes, 128, 1);
    u32_at(&mut bytes, 132, 3);
    u64_at(&mut bytes, 152, 256);
    u64_at(&mut bytes, 160, names.len() as u64);
    u64_at(&mut bytes, 176, 1);
    u32_at(&mut bytes, 192, 11);
    u32_at(&mut bytes, 196, 1);
    u64_at(&mut bytes, 200, 2);
    u64_at(&mut bytes, 216, data as u64);
    u64_at(&mut bytes, 224, payload.len() as u64);
    u64_at(&mut bytes, 240, 1);
    bytes[256..256 + names.len()].copy_from_slice(names);
    bytes[data..].copy_from_slice(payload);
    bytes
}

fn macho(payload: &[u8], arm: bool) -> Vec<u8> {
    let data = 32 + 72 + 80;
    let mut bytes = vec![0; data + payload.len()];
    u32_at(&mut bytes, 0, 0xfeedfacf);
    u32_at(&mut bytes, 4, if arm { 0x0100000c } else { 0x01000007 });
    u32_at(&mut bytes, 8, if arm { 0 } else { 3 });
    u32_at(&mut bytes, 12, 6); // MH_DYLIB。
    u32_at(&mut bytes, 16, 1);
    u32_at(&mut bytes, 20, 152);
    u32_at(&mut bytes, 32, 0x19); // LC_SEGMENT_64 + 唯一__DATA,__frmeta节。
    u32_at(&mut bytes, 36, 152);
    bytes[40..46].copy_from_slice(b"__DATA");
    u64_at(&mut bytes, 64, payload.len() as u64);
    u64_at(&mut bytes, 72, data as u64);
    u64_at(&mut bytes, 80, payload.len() as u64);
    u32_at(&mut bytes, 88, 3);
    u32_at(&mut bytes, 92, 3);
    u32_at(&mut bytes, 96, 1);
    bytes[104..112].copy_from_slice(b"__frmeta");
    bytes[120..126].copy_from_slice(b"__DATA");
    u64_at(&mut bytes, 144, payload.len() as u64);
    u32_at(&mut bytes, 152, data as u32);
    bytes[data..].copy_from_slice(payload);
    bytes
}
