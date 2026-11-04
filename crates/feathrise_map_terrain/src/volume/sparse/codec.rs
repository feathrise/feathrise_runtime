//! GLB v2包含有界块目录与共享作者源；旧v1继续只读/编辑兼容，校验后才重建缓存。
use super::{brush::rebuild_all, types::*};
use crate::{
    TerrainMeshInput, build_white_terrain_glb,
    volume::{decode_volume_glb, encode_volume_glb},
};
use serde_json::{Value, json};
use std::sync::Arc;
const KEY: &str = "rpg_editor_volume_v2";
const MAX_DIRECTORY_BYTES: usize = 256 * 1024;

pub fn encode_author_volume_glb(
    source: &VolumeSource,
    mesh: &TerrainMeshInput,
) -> Result<Vec<u8>, String> {
    if let VolumeSource::Paged(data) = source {
        return super::paged::encode_paged_volume(data);
    }
    let VolumeSource::Sparse(data) = source else {
        let VolumeSource::Dense(v) = source else {
            unreachable!()
        };
        return encode_volume_glb(v, mesh);
    };
    data.validate()?;
    if mesh.positions.len() > MAX_SPARSE_VERTICES || mesh.indices.len() / 3 > MAX_SPARSE_TRIANGLES {
        return Err("分块三维输出超过几何预算".into());
    }
    // 大小预检和精确预留避免向BIN追加作者块时倍增容量；编码期间最多两份32MiB文件缓冲。
    let binary_bytes = mesh.positions.len() * 24 + mesh.indices.len() * 4 + data.memory_bytes();
    if binary_bytes + MAX_DIRECTORY_BYTES + 28 > MAX_AUTHOR_VOLUME_GLB_BYTES {
        return Err("密度与可见网格合计超过32MiB文件预算，保留上一完整地形".into());
    }
    let (mut document, mut binary) = {
        let glb = build_white_terrain_glb(mesh)?;
        let (document, bin) = crate::glb::parse_glb_chunks(&glb)?;
        let mut binary = Vec::with_capacity(binary_bytes + 3);
        binary.extend_from_slice(bin);
        (document, binary)
    };
    let mut columns = Vec::new();
    let mut blocks = Vec::new();
    for (key, values) in &data.columns {
        let start = binary.len();
        for v in values.iter() {
            binary.extend(v.to_le_bytes());
        }
        let view = add_view(&mut document, start, binary.len() - start)?;
        columns.push(json!([key[0], key[1], view]));
    }
    for (key, values) in &data.blocks {
        let start = binary.len();
        for v in values.iter() {
            binary.extend(v.to_le_bytes());
        }
        let view = add_view(&mut document, start, binary.len() - start)?;
        blocks.push(json!([key[0], key[1], key[2], view]));
    }
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    document["buffers"][0]["byteLength"] = json!(binary.len());
    document["asset"]["extras"][KEY] =
        json!({"cell_size":data.cell_size,"columns":columns,"blocks":blocks});
    if data.mesh_recipe == MeshRecipe::CompactV1 {
        document["asset"]["extras"][KEY]["mesh_recipe"] = json!(1);
    }
    document["nodes"][0]["name"] = json!("分块体积地形原型");
    if serde_json::to_vec(&document)
        .map_err(|e| e.to_string())?
        .len()
        > MAX_DIRECTORY_BYTES
    {
        return Err("分块作者目录超过256KiB预算".into());
    }
    super::super::codec::pack(document, &binary, MAX_AUTHOR_VOLUME_GLB_BYTES)
}

fn add_view(document: &mut Value, offset: usize, len: usize) -> Result<usize, String> {
    let views = document["bufferViews"]
        .as_array_mut()
        .ok_or("生成GLB缺少缓冲目录")?;
    let id = views.len();
    views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":len}));
    Ok(id)
}

pub fn decode_author_volume_glb(
    bytes: &[u8],
    budget: Arc<VolumeMemoryBudget>,
) -> Result<VolumeSource, String> {
    if bytes.len() > MAX_AUTHOR_VOLUME_GLB_BYTES {
        return Err("三维作者GLB超过32MiB文件预算".into());
    }
    validate_directory_envelope(bytes)?;
    if let Some(data) = super::paged::decode_paged_volume(bytes)? {
        return Ok(VolumeSource::Paged(data));
    }
    let (document, binary) = crate::glb::parse_glb_chunks(bytes)?;
    let meta = &document["asset"]["extras"][KEY];
    if meta.is_null() {
        return Ok(VolumeSource::Dense(decode_volume_glb(bytes)?));
    }
    let step = meta["cell_size"].as_f64().ok_or("三维块目录缺少格距")? as f32;
    let columns = meta["columns"]
        .as_array()
        .ok_or("三维块目录缺少基础高度列")?;
    let blocks = meta["blocks"].as_array().ok_or("三维块目录缺少密度块")?;
    if columns.is_empty() || columns.len() > MAX_COLUMNS || blocks.len() > MAX_BLOCKS {
        return Err("三维块目录容量无效".into());
    }
    let mut data = super::brush::new_sparse_volume(step, budget)?;
    data.mesh_recipe = match meta.get("mesh_recipe") {
        None => MeshRecipe::Legacy,
        Some(v) if v.as_u64() == Some(1) => MeshRecipe::CompactV1,
        _ => return Err("三维作者网格配方未知，不能用另一种提取结果覆盖原文件".into()),
    };
    for entry in columns {
        let a = entry
            .as_array()
            .filter(|a| a.len() == 3)
            .ok_or("三维高度列目录无效")?;
        let key = [coordinate(&a[0])?, coordinate(&a[1])?];
        let raw = read_view(&document, binary, &a[2], 33usize.pow(2) * 4)?;
        let values = raw
            .chunks_exact(4)
            .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
            .collect::<Vec<_>>();
        if data
            .columns
            .insert(key, data.budget.retain(values, raw.len())?)
            .is_some()
        {
            return Err("三维块目录包含重复高度列".into());
        }
    }
    for entry in blocks {
        let a = entry
            .as_array()
            .filter(|a| a.len() == 4)
            .ok_or("三维密度块目录无效")?;
        let key = [coordinate(&a[0])?, coordinate(&a[1])?, coordinate(&a[2])?];
        let raw = read_view(&document, binary, &a[3], 32usize.pow(3) * 2)?;
        let values = raw
            .chunks_exact(2)
            .map(|v| i16::from_le_bytes(v.try_into().unwrap()))
            .collect::<Vec<_>>();
        if data
            .blocks
            .insert(key, data.budget.retain(values, raw.len())?)
            .is_some()
        {
            return Err("三维块目录包含重复密度块".into());
        }
    }
    data.validate()?; // 在任何缓存重建和整数格点运算前验证坐标、长度及高差。
    rebuild_all(&mut data)?;
    Ok(VolumeSource::Sparse(data))
}

/// 先限制原始JSON载荷，再交通用GLB解析器；小文件里的巨大零数组也不能放大解码工作域。
fn validate_directory_envelope(bytes: &[u8]) -> Result<(), String> {
    let mut offset = 12usize;
    while offset < bytes.len() {
        let header = bytes
            .get(offset..offset.saturating_add(8))
            .ok_or("三维GLB块头被截断")?;
        let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        let kind = u32::from_le_bytes(header[4..].try_into().unwrap());
        if kind == 0x4e4f534a && len > MAX_DIRECTORY_BYTES {
            return Err("三维作者JSON目录超过256KiB预算".into());
        }
        offset = offset
            .checked_add(8)
            .and_then(|n| n.checked_add(len))
            .filter(|n| *n <= bytes.len())
            .ok_or("三维GLB块长度无效")?;
    }
    Ok(())
}
fn coordinate(v: &Value) -> Result<i32, String> {
    v.as_i64()
        .and_then(|v| i32::try_from(v).ok())
        .filter(|v| v.abs_diff(0) <= 1_000_000)
        .ok_or_else(|| "三维块坐标无效".into())
}
fn read_view<'a>(
    document: &Value,
    binary: &'a [u8],
    id: &Value,
    expected: usize,
) -> Result<&'a [u8], String> {
    let id = id
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("三维块缓冲索引无效")?;
    let v = document["bufferViews"].get(id).ok_or("三维块缓冲不存在")?;
    if v["buffer"].as_u64() != Some(0) || v["byteLength"].as_u64() != Some(expected as u64) {
        return Err("三维块缓冲长度无效".into());
    }
    let offset = v["byteOffset"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("三维块偏移无效")?;
    let end = offset.checked_add(expected).ok_or("三维块偏移溢出")?;
    binary
        .get(offset..end)
        .ok_or_else(|| "三维块缓冲被截断".into())
}
