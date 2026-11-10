//! 连续植被预览的唯一字节合同：小型描述头加连续R8，不传ECS句柄，不重编码整个场景。

use crate::{SceneId, SceneVegetationAsset, TerrainId};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet}, sync::Arc};

pub const VEGETATION_PREVIEW_FORMAT: &str = "feathrise-vegetation-preview-v1";
pub const MAX_VEGETATION_PREVIEW_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_VEGETATION_PREVIEW_CANVASES: usize = 4;
const MAX_HEADER_BYTES: usize = 1024 * 1024;
const MAGIC: &[u8; 8] = b"FRVEG001";

pub struct VegetationPreviewBatch {
    pub scene_id: SceneId,
    /// 作者图层结构/画布来源身份；同来源像素完成值允许落后当前持续刷印。
    pub structure_revision: u64,
    pub canvases: Vec<VegetationPreviewCanvas>,
}

pub struct VegetationPreviewCanvas {
    pub terrain_id: Option<TerrainId>,
    pub source_revision: u64,
    pub layout_revision: u64,
    pub definition: SceneVegetationAsset,
    pub densities: BTreeMap<String, Arc<[u8]>>,
    pub nonzero_layers: BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct Header {
    scene_id: SceneId,
    structure_revision: u64,
    canvases: Vec<CanvasHeader>,
}
#[derive(Serialize, Deserialize)]
struct CanvasHeader {
    terrain_id: Option<TerrainId>,
    source_revision: u64,
    layout_revision: u64,
    definition: SceneVegetationAsset,
    /// raw区偏移和长度只由本模块顺序生成；解码拒绝重叠、间隙、尾随字节。
    densities: BTreeMap<String, [usize; 2]>,
    nonzero_layers: BTreeSet<String>,
}

fn validate(batch: &VegetationPreviewBatch) -> Result<(), String> {
    if batch.canvases.is_empty() || batch.canvases.len() > MAX_VEGETATION_PREVIEW_CANVASES {
        return Err("植被预览必须包含1到4个完整变化画布".into());
    }
    let mut terrains = BTreeSet::new();
    let mut total = 0_usize;
    for canvas in &batch.canvases {
        if !terrains.insert(canvas.terrain_id.clone()) || canvas.definition.scene_id != batch.scene_id
            || canvas.definition.authoring.is_some() || canvas.definition.layers.len() > 64
            || canvas.densities.len() != canvas.definition.layers.len()
        {
            return Err("植被预览存在重复画布、跨场景来源、作者图或不完整正式层".into());
        }
        let mut layers = BTreeSet::new();
        for layer in &canvas.definition.layers {
            let size = (layer.resolution[0] as usize).checked_mul(layer.resolution[1] as usize)
                .filter(|size| *size > 0).ok_or("植被预览密度尺寸无效")?;
            let pixels = canvas.densities.get(&layer.id).ok_or("植被预览缺少正式层R8")?;
            if !layers.insert(&layer.id) || pixels.len() != size {
                return Err("植被预览密度尺寸或层身份不符".into());
            }
            total = total.checked_add(size).filter(|size| *size <= MAX_VEGETATION_PREVIEW_BYTES)
                .ok_or("植被预览超过64MiB批次预算")?;
            if canvas.nonzero_layers.contains(&layer.id) != pixels.iter().any(|pixel| *pixel != 0) {
                return Err("植被预览非零层证据与原始R8不符".into());
            }
        }
        if canvas.nonzero_layers.iter().any(|id| !canvas.densities.contains_key(id)) {
            return Err("植被预览非零集合包含未知层".into());
        }
    }
    Ok(())
}

/// 仅由后台调用。大像素直接写raw区，避免逐像素JSON数字放大IPC。
pub fn encode_vegetation_preview(batch: &VegetationPreviewBatch) -> Result<Vec<u8>, String> {
    validate(batch)?;
    let mut raw_len = 0_usize;
    let mut canvases = Vec::new();
    for canvas in &batch.canvases {
        let mut densities = BTreeMap::new();
        for (id, pixels) in &canvas.densities {
            densities.insert(id.clone(), [raw_len, pixels.len()]);
            raw_len += pixels.len();
        }
        canvases.push(CanvasHeader { terrain_id: canvas.terrain_id.clone(),
            source_revision: canvas.source_revision, layout_revision: canvas.layout_revision,
            definition: canvas.definition.clone(), densities, nonzero_layers: canvas.nonzero_layers.clone() });
    }
    let metadata = serde_json::to_vec(&Header { scene_id: batch.scene_id.clone(),
        structure_revision: batch.structure_revision, canvases }).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_HEADER_BYTES || 12 + metadata.len() + raw_len > MAX_VEGETATION_PREVIEW_BYTES {
        return Err("植被预览编码超过批次预算".into());
    }
    let mut bytes = Vec::with_capacity(12 + metadata.len() + raw_len);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&metadata);
    for canvas in &batch.canvases {
        for pixels in canvas.densities.values() { bytes.extend_from_slice(pixels); }
    }
    Ok(bytes)
}

/// 背景线程完成完整格式/身份/像素验证；调用方在主线程仍须核对会话、场景和来源代次。
pub fn decode_vegetation_preview(bytes: &[u8]) -> Result<VegetationPreviewBatch, String> {
    if bytes.len() < 12 || bytes.len() > MAX_VEGETATION_PREVIEW_BYTES || &bytes[..8] != MAGIC {
        return Err("植被预览帧头或总长度无效".into());
    }
    let metadata_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if metadata_len > MAX_HEADER_BYTES || metadata_len > bytes.len() - 12 {
        return Err("植被预览描述头越界".into());
    }
    let header: Header = serde_json::from_slice(&bytes[12..12 + metadata_len])
        .map_err(|error| error.to_string())?;
    if header.canvases.is_empty() || header.canvases.len() > MAX_VEGETATION_PREVIEW_CANVASES {
        return Err("植被预览画布数量无效".into());
    }
    let raw = &bytes[12 + metadata_len..];
    let mut cursor = 0_usize;
    let mut canvases = Vec::new();
    for canvas in header.canvases {
        let mut densities = BTreeMap::new();
        for (id, [offset, length]) in canvas.densities {
            if offset != cursor || length > raw.len().saturating_sub(cursor) {
                return Err("植被预览R8范围存在重叠、缺口或越界".into());
            }
            densities.insert(id, Arc::from(&raw[cursor..cursor + length]));
            cursor += length;
        }
        canvases.push(VegetationPreviewCanvas { terrain_id: canvas.terrain_id,
            source_revision: canvas.source_revision, layout_revision: canvas.layout_revision,
            definition: canvas.definition, densities, nonzero_layers: canvas.nonzero_layers });
    }
    if cursor != raw.len() { return Err("植被预览存在未声明尾随R8".into()); }
    let batch = VegetationPreviewBatch { scene_id: header.scene_id,
        structure_revision: header.structure_revision, canvases };
    validate(&batch)?;
    Ok(batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_r8_bytes_roundtrip_without_json_pixel_expansion_and_validate_nonzero_evidence() {
        let map = SceneId::parse("MAP-preview-r8").unwrap();
        let layer = crate::SceneVegetationLayer {
            id: "blade".into(), label: "草".into(), kind: crate::SceneVegetationKind::Blade,
            runtime_profile: Some("ordinary".into()), runtime_layer_id: Some(1), prototype_id: None,
            asset_path: "grass.glb".into(), density_source_path: "grass.r8".into(),
            density_texture_path: "grass.ktx2".into(), resolution: [64, 64], spacing_meters: 1.0,
            scale_range: [1.0, 1.0], max_instances: 4096, seed: 1,
        };
        let pixels: Arc<[u8]> = Arc::from(vec![237; 4096]);
        let mut batch = VegetationPreviewBatch { scene_id: map.clone(), structure_revision: 2,
            canvases: vec![VegetationPreviewCanvas { terrain_id: None, source_revision: 4, layout_revision: 1,
                definition: SceneVegetationAsset { schema_version: crate::SCENE_SCHEMA_VERSION, scene_id: map, layers: vec![layer], authoring: None },
                densities: BTreeMap::from([("blade".into(), pixels.clone())]),
                nonzero_layers: BTreeSet::from(["blade".into()]) }] };
        let encoded = encode_vegetation_preview(&batch).unwrap();
        assert!(encoded.len() < 6000, "R8不能变成逐像素JSON数字");
        assert_eq!(decode_vegetation_preview(&encoded).unwrap().canvases[0].densities["blade"], pixels);
        batch.canvases[0].nonzero_layers.clear();
        assert!(encode_vegetation_preview(&batch).is_err());
    }
    #[test]
    fn empty_formal_output_roundtrips_and_bad_framing_or_duplicate_canvas_is_rejected() {
        let map = SceneId::parse("MAP-preview").unwrap();
        let mut batch = VegetationPreviewBatch { scene_id: map.clone(), structure_revision: 9,
            canvases: vec![VegetationPreviewCanvas { terrain_id: None, source_revision: 12,
                layout_revision: 3, definition: SceneVegetationAsset { scene_id: map, ..Default::default() },
                densities: BTreeMap::new(), nonzero_layers: BTreeSet::new() }] };
        let encoded = encode_vegetation_preview(&batch).unwrap();
        let decoded = decode_vegetation_preview(&encoded).unwrap();
        assert_eq!(decoded.structure_revision, 9);
        assert_eq!(decoded.canvases[0].source_revision, 12);
        let mut tail = encoded.clone(); tail.push(1);
        assert!(decode_vegetation_preview(&tail).is_err());
        assert!(decode_vegetation_preview(&encoded[..encoded.len()-1]).is_err());
        batch.canvases.extend(decoded.canvases);
        assert!(encode_vegetation_preview(&batch).is_err());
    }
}
