//! 显式验收的实际分页高度指纹；调用者必须放在后台，不把显示模板顶点冒充作者高度。

use feathrise_scene_content::{SceneId, SparseHeightDocument, TerrainId};
use sha2::{Digest, Sha256};
use std::path::Path;

pub fn validation_height_fingerprint(root: &Path, scene: &SceneId, terrain: &TerrainId, path: &str) -> Result<String, String> {
    let mut document = SparseHeightDocument::load_source(root, scene, Some(terrain), path)?;
    let spec = document.manifest().spec.clone();
    // 验收夹具固定64m/2m；给小尺寸专项留出64格边界，超大生产地图不能借验收遍历整源。
    if spec.cells.into_iter().any(|cells| cells > 64) || document.manifest().tiles.len() > 16 {
        return Err("高度验收源超过64格/16页的固定预算".into());
    }
    let mut digest = Sha256::new();
    digest.update(serde_json::to_vec(&spec).map_err(|error| error.to_string())?);
    let keys = document.manifest().tiles.iter().map(|tile| tile.key).collect::<Vec<_>>();
    for key in keys {
        document.load_tile(key)?;
        digest.update(key.x.to_le_bytes()); digest.update(key.z.to_le_bytes());
        for code in document.tile_snapshot(key)?.height_codes(&spec) { digest.update(code.to_le_bytes()); }
        document.unload_clean_tile(key);
    }
    Ok(digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}
