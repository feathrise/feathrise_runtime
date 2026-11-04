//! 有界IO读取及后台驻留聚合；版本载体为空场景，不参与普通GLB实例化。
use super::types::*;
use feathrise_map_content::MapTerrainCutout;
use feathrise_map_terrain::volume::sparse::paged::*;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

fn source_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if path.len() > 4096
        || p.is_absolute()
        || path.contains('\\')
        || p.components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || p.parent()
            .and_then(Path::file_name)
            .and_then(|p| p.to_str())
            != Some("volume_sculpt")
    {
        return Err("体积流送版本必须在地图私有volume_sculpt目录".into());
    }
    Ok(root.join(p))
}
pub(super) async fn build(asset_root: feathrise_map_content::RuntimeAssetRoot, identity: Identity, progress: Arc<AtomicU64>) -> Result<Built, String> {
    build_with_progress(
        asset_root.asset_root(), // 任务拥有启动时的工程来源，不读取宿主cwd或SDK同名数据。
        identity,
        Some(progress),
    )
    .await
}
#[cfg(test)]
pub(super) async fn build_at(root: &Path, identity: Identity) -> Result<Built, String> {
    build_with_progress(root, identity, None).await
}
async fn build_with_progress(
    root: &Path,
    identity: Identity,
    progress: Option<Arc<AtomicU64>>,
) -> Result<Built, String> {
    if let Some(p) = &progress {
        p.fetch_add(1, Ordering::Relaxed);
    }
    let path = source_path(root, &identity.record.asset_path)?;
    let file = std::fs::File::open(&path).map_err(|e| format!("体积版本读取失败：{e}"))?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 4096 {
        return Err("体积版本超过4KiB预算".into());
    }
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if path.file_name().and_then(|n| n.to_str()) != Some(format!("v-{hash}.glb").as_str()) {
        return Err("体积版本与摘要路径不一致".into());
    }
    let source = decode_paged_volume(&bytes)?.ok_or("流送记录不含v3作者根")?;
    let patch = identity
        .record
        .terrain_patch
        .as_ref()
        .ok_or("流送作者声明缺失")?;
    if !patch.streamed_volume
        || patch.volume_cell_size != Some(source.cell_size)
        || patch.cutout.is_some()
        || patch
            .cutouts
            .iter()
            .map(|h| [h.min, h.max])
            .collect::<Vec<_>>()
            != source.footprint()
    {
        return Err("流送作者包围范围与版本根不一致".into());
    }
    let objects = path.parent().unwrap().join("objects");
    let focus = identity.center.map(|v| (v as f32 + 0.5) * source.extent());
    let mut foci = vec![focus];
    if let Some(p) = identity.physics_center.filter(|p| *p != identity.center) {
        foci.push(p.map(|v| (v as f32 + 0.5) * source.extent()));
    }
    let resident = super::display::build_volume_display_with_progress(
        &objects,
        &source,
        &foci,
        identity.lods,
        progress.clone(),
    )
    .await?;
    let mut record = identity.record;
    record.terrain_patch.as_mut().unwrap().cutouts = resident
        .footprint
        .into_iter()
        .map(|h| MapTerrainCutout {
            min: h[0],
            max: h[1],
        })
        .collect();
    let meshes = resident.meshes;
    let fields = resident.fields;
    if let Some(p) = &progress {
        p.fetch_add(1, Ordering::Relaxed);
    }
    Ok(Built {
        record,
        meshes,
        fields,
    })
}
