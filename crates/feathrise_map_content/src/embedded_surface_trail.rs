//! 原地表笔迹辅助图的固定行块协议；不含PBR参数、不替代原笔刷或地形几何。
use crate::{MapId, MapTerrainKind, TerrainId};
use serde::{Deserialize, Serialize};
pub const SURFACE_TRAIL_FORMAT: &str = "feathrise-surface-trail-v1";
pub const SURFACE_TRAIL_DIM: usize = 256;
pub const SURFACE_TRAIL_ROWS: usize = 16;
pub const SURFACE_TRAIL_BLOCKS: usize = SURFACE_TRAIL_DIM / SURFACE_TRAIL_ROWS;
pub const SURFACE_TRAIL_BLOCK_BYTES: usize = SURFACE_TRAIL_DIM * SURFACE_TRAIL_ROWS * 4;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SurfaceTrailPatch {
    pub map_id: MapId,
    pub terrain_id: Option<TerrainId>,
    pub terrain_revision: u64,
    pub author_revision: u64,
    pub surface_revision: u64,
    pub source_epoch: u64,
    pub source_to_world: [f32; 16],
    pub terrain_kind: MapTerrainKind,
    pub visible: bool,
    pub reset: bool,
    pub block: u8,
    pub block_revision: u64,
    #[serde(skip)]
    pub rgba: Vec<u8>,
}
impl SurfaceTrailPatch {
    pub fn validate(&self) -> Result<(), String> {
        MapId::parse(self.map_id.as_str())?;
        if self.source_epoch == 0 || self.source_to_world.iter().any(|value| !value.is_finite()) {
            return Err("地表辅助覆盖缺少有效代次/姿态".into());
        }
        if !matches!(
            self.terrain_kind,
            MapTerrainKind::Gltf | MapTerrainKind::Heightfield | MapTerrainKind::SparseClipmap
        ) {
            return Err("地表辅助覆盖不支持此真实地形类型".into());
        }
        if self.visible {
            if usize::from(self.block) >= SURFACE_TRAIL_BLOCKS
                || self.rgba.len() != SURFACE_TRAIL_BLOCK_BYTES
            {
                return Err("地表辅助行块尺寸越界".into());
            }
        } else if !self.rgba.is_empty() || !self.reset {
            return Err("隐藏地表辅助覆盖必须可靠清空且不携像素".into());
        }
        Ok(())
    }
}
pub fn encode_surface_trail(value: &SurfaceTrailPatch) -> Result<Vec<u8>, String> {
    value.validate()?;
    let header = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if header.len() > 4096 {
        return Err("地表辅助元数据超预算".into());
    }
    let mut bytes = Vec::with_capacity(12 + header.len() + value.rgba.len());
    bytes.extend_from_slice(b"FRTRAIL1");
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&value.rgba);
    Ok(bytes)
}
pub fn decode_surface_trail(bytes: &[u8]) -> Result<SurfaceTrailPatch, String> {
    if bytes.len() < 12
        || bytes.len() > 12 + 4096 + SURFACE_TRAIL_BLOCK_BYTES
        || &bytes[..8] != b"FRTRAIL1"
    {
        return Err("地表辅助帧尺寸或格式无效".into());
    }
    let size = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if size > 4096 || 12 + size > bytes.len() {
        return Err("地表辅助元数据越界".into());
    }
    let mut value: SurfaceTrailPatch =
        serde_json::from_slice(&bytes[12..12 + size]).map_err(|error| error.to_string())?;
    value.rgba = bytes[12 + size..].to_vec();
    value.validate()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surface_trail_fixed_block_roundtrip_and_reliable_clear() {
        let mut value = SurfaceTrailPatch {
            map_id: MapId::parse("M1-TEST").unwrap(),
            terrain_id: None,
            terrain_revision: 1,
            author_revision: 2,
            surface_revision: 2,
            source_epoch: 1,
            source_to_world: bevy::prelude::Mat4::IDENTITY.to_cols_array(),
            terrain_kind: MapTerrainKind::Heightfield,
            visible: true,
            reset: true,
            block: 15,
            block_revision: 9,
            rgba: vec![47; SURFACE_TRAIL_BLOCK_BYTES],
        };
        let encoded = encode_surface_trail(&value).unwrap();
        assert_eq!(decode_surface_trail(&encoded).unwrap().rgba, value.rgba);
        value.block = 16;
        assert!(encode_surface_trail(&value).is_err());
        value.block = 0;
        value.visible = false;
        assert!(encode_surface_trail(&value).is_err());
        value.rgba.clear();
        assert!(
            !decode_surface_trail(&encode_surface_trail(&value).unwrap())
                .unwrap()
                .visible
        );
        value.reset = false;
        assert!(encode_surface_trail(&value).is_err());
    }
}
