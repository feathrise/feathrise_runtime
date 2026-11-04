//! 固定头与有界JSON的唯一编码；静态节及导出函数始终引用同一份字节。
use super::*;

pub fn encode_plugin_metadata(metadata: &PluginMetadata) -> Result<Vec<u8>, String> {
    validate(metadata)?;
    let json = serde_json::to_vec(metadata).map_err(|error| error.to_string())?;
    if json.len() > MAX_METADATA_JSON_BYTES {
        return Err("插件元数据JSON超过64KiB".into());
    }
    let mut bytes = Vec::with_capacity(METADATA_HEADER_BYTES + json.len());
    bytes.extend_from_slice(METADATA_MAGIC);
    bytes.extend_from_slice(&METADATA_FORMAT.to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&json);
    Ok(bytes)
}

pub fn decode_plugin_metadata(bytes: &[u8]) -> Result<PluginMetadata, String> {
    // PE磁盘节会补齐到文件对齐；仅允许少量零填充，不从任意二进制正文搜索魔数。
    if bytes.len() < METADATA_HEADER_BYTES || bytes.len() > MAX_METADATA_BYTES + 4095 {
        return Err("插件元数据节长度无效或超出64KiB预算".into());
    }
    if &bytes[..16] != METADATA_MAGIC {
        return Err("插件元数据标识不匹配".into());
    }
    let format = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
    let length = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
    if format != METADATA_FORMAT || length == 0 || length > MAX_METADATA_JSON_BYTES {
        return Err("插件元数据格式或JSON长度不受支持".into());
    }
    let end = METADATA_HEADER_BYTES + length;
    if end > bytes.len() || bytes[end..].iter().any(|byte| *byte != 0) {
        return Err("插件元数据截断或包含非零尾随内容".into());
    }
    let metadata: PluginMetadata = serde_json::from_slice(&bytes[METADATA_HEADER_BYTES..end])
        .map_err(|error| format!("插件元数据JSON无效：{error}"))?;
    validate(&metadata)?;
    Ok(metadata)
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && !value.contains("..")
}

pub(super) fn validate(metadata: &PluginMetadata) -> Result<(), String> {
    if metadata.format != METADATA_FORMAT || metadata.abi_major != crate::ABI_MAJOR {
        return Err("插件元数据格式或作者ABI主版本不兼容".into());
    }
    if !identifier(&metadata.id, 96) {
        return Err("插件ID必须是最多96字节的安全ASCII标识".into());
    }
    if metadata.tool.trim().is_empty() || metadata.tool.len() > 128 || metadata.tool.contains('\0')
    {
        return Err("插件名称为空或过长".into());
    }
    let core = metadata
        .version
        .split(['-', '+'])
        .next()
        .unwrap_or_default();
    if metadata.version.len() > 128
        || core.split('.').count() != 3
        || core
            .split('.')
            .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
        || !metadata
            .version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte))
    {
        return Err("插件版本必须声明major.minor.patch".into());
    }
    if !matches!(metadata.target_os.as_str(), "windows" | "linux" | "macos")
        || !matches!(
            metadata.target_arch.as_str(),
            "x86_64" | "aarch64" | "x86" | "arm"
        )
    {
        return Err("插件目标操作系统或CPU架构不受支持".into());
    }
    if metadata.runtime_dependencies.len() > 64
        || metadata
            .runtime_dependencies
            .iter()
            .any(|item| !identifier(item, 128))
    {
        return Err("插件运行依赖声明无效或超过64项".into());
    }
    if let Some(runtime) = &metadata.runtime {
        if !identifier(&runtime.crate_name, 128)
            || runtime.api == 0
            || runtime.entry.trim().is_empty()
            || runtime.entry.len() > 256
            || runtime.entry.contains('\0')
            || runtime
                .preview_kind
                .as_ref()
                .is_some_and(|kind| !identifier(kind, 128))
        {
            return Err("插件运行端说明无效".into());
        }
        if !metadata.runtime_dependencies.contains(&runtime.crate_name) {
            return Err("插件运行端未包含在运行依赖中".into());
        }
    }
    Ok(())
}
