//! 小型v3版本载体；几何来自驻留块，不能交普通GLB实例化路径显示。
use super::types::*;
use serde_json::json;
const KEY: &str = "rpg_editor_volume_v3";
pub fn encode_paged_volume(volume: &PagedVolume) -> Result<Vec<u8>, String> {
    volume.validate()?;
    // 保留合法空场景GLB以兼容路径/保存事务；即使误用普通模型入口也不会显示标记三角。
    let mut doc = json!({"asset":{"version":"2.0","generator":"rpg_volume_stream_v3"},"scene":0,"scenes":[{"name":"体积流送版本载体"}],"buffers":[{"byteLength":4}]});
    doc["asset"]["extras"][KEY] = serde_json::to_value(volume).map_err(|e| e.to_string())?;
    super::super::super::codec::pack(doc, &[0; 4], 4096)
}
pub fn decode_paged_volume(bytes: &[u8]) -> Result<Option<PagedVolume>, String> {
    let (doc, _) = crate::glb::parse_glb_chunks(bytes)?;
    let Some(meta) = doc["asset"]["extras"].get(KEY) else {
        return Ok(None);
    };
    if bytes.len() > 4096 {
        return Err("流送体积版本载体超过4KiB".into());
    }
    let volume: PagedVolume =
        serde_json::from_value(meta.clone()).map_err(|e| format!("流送体积版本无效：{e}"))?;
    volume.validate()?;
    Ok(Some(volume))
}
