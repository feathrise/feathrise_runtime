//! 同一个GLB保存常规可见网格和原型密度作者源；禁止无源网格冒充可继续雕刻的数据。
use super::types::*;
use crate::{TerrainMeshInput, build_white_terrain_glb};
use serde_json::{Value, json};

const SOURCE_KEY: &str = "rpg_editor_volume_v1";

pub fn encode_volume_glb(data: &VolumeData, mesh: &TerrainMeshInput) -> Result<Vec<u8>, String> {
    data.validate()?;
    if mesh.positions.len() > MAX_VOLUME_VERTICES || mesh.indices.len() / 3 > MAX_VOLUME_TRIANGLES {
        return Err("三维GLB输入超过固定顶点/三角预算".into()); // 编码前拒绝，不能先分配超预算法线和BIN再检查文件长度。
    }
    let glb = build_white_terrain_glb(mesh)?;
    let (mut document, bin) = crate::glb::parse_glb_chunks(&glb)?;
    let mut binary = bin.to_vec();
    let offset = binary.len();
    for value in &data.base_heights {
        binary.extend(value.to_le_bytes());
    }
    for value in &data.density {
        binary.extend(value.to_le_bytes());
    }
    let length = binary.len() - offset;
    while !binary.len().is_multiple_of(4) {
        binary.push(0);
    }
    let views = document["bufferViews"]
        .as_array_mut()
        .ok_or("生成GLB缺少bufferViews")?;
    let view = views.len();
    views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":length}));
    document["buffers"][0]["byteLength"] = json!(binary.len());
    document["asset"]["extras"][SOURCE_KEY] =
        json!({"cells":data.cells,"cell_size":data.cell_size,"buffer_view":view});
    document["nodes"][0]["name"] = json!("体积地形原型");
    pack(document, &binary, MAX_VOLUME_GLB_BYTES)
}

pub fn decode_volume_glb(bytes: &[u8]) -> Result<VolumeData, String> {
    if bytes.len() > MAX_VOLUME_GLB_BYTES {
        return Err("三维作者GLB超过8MiB原型门".into());
    }
    let (document, binary) = crate::glb::parse_glb_chunks(bytes)?;
    let metadata = &document["asset"]["extras"][SOURCE_KEY];
    let cells = metadata["cells"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or("GLB不含有效三维雕刻作者源")?;
    let step = metadata["cell_size"].as_f64().ok_or("三维作者源缺少格距")? as f32;
    if !(4..=VOLUME_CELLS).contains(&cells) || !cells.is_multiple_of(4) {
        return Err("三维作者源单元数无效".into());
    }
    let side = cells as usize + 1;
    let view_id = metadata["buffer_view"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("三维作者源缓冲索引无效")?;
    let view = document["bufferViews"]
        .get(view_id)
        .ok_or("三维作者源缓冲不存在")?;
    if view["buffer"].as_u64() != Some(0) {
        return Err("三维作者源必须使用内嵌缓冲".into());
    }
    let offset = view["byteOffset"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("三维作者源偏移无效")?;
    let length = view["byteLength"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("三维作者源长度无效")?;
    let expected = side.pow(2) * 4 + side.pow(3) * 2;
    let end = offset.checked_add(length).ok_or("三维作者源偏移溢出")?;
    if length != expected || end > binary.len() {
        return Err("三维作者源字节长度不匹配".into());
    }
    let raw = &binary[offset..end];
    let data = VolumeData {
        cells,
        cell_size: step,
        base_heights: raw[..side.pow(2) * 4]
            .chunks_exact(4)
            .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
            .collect(),
        density: raw[side.pow(2) * 4..]
            .chunks_exact(2)
            .map(|v| i16::from_le_bytes(v.try_into().unwrap()))
            .collect(),
    };
    data.validate()?;
    Ok(data)
}

pub(super) fn pack(document: Value, binary: &[u8], max_bytes: usize) -> Result<Vec<u8>, String> {
    let mut json = serde_json::to_vec(&document).map_err(|e| e.to_string())?;
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let length = 12 + 8 + json.len() + 8 + binary.len();
    if length > max_bytes {
        return Err("三维生成GLB超过当前格式文件预算".into());
    }
    let mut out = Vec::with_capacity(length);
    for value in [
        0x46546c67u32,
        2,
        length as u32,
        json.len() as u32,
        0x4e4f534a,
    ] {
        out.extend(value.to_le_bytes());
    }
    out.extend(json);
    for value in [binary.len() as u32, 0x004e4942] {
        out.extend(value.to_le_bytes());
    }
    out.extend(binary);
    Ok(out)
}
