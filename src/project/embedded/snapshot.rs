//! 快照在管道线程完整校验和准备；帧线程只发布不可变资产源，生产 loader 继续拥有派生与就绪门。

use super::{
    SceneSnapshot,
    asset_overlay::{Files, ReadEvidence, valid_path},
};
use feathrise_scene_content::*;
use std::{collections::BTreeMap, sync::Arc};

pub(super) struct PreparedSnapshot {
    pub transaction: u64,
    pub revision: u64,
    pub generation: u64,
    pub scene_id: SceneId,
    pub name: String,
    pub spawn: [f32; 3],
    pub files: Arc<Files>,
    pub paths: Vec<String>,
    pub reload_paths: Vec<String>,
    pub reads: Arc<ReadEvidence>,
    pub heights: SparseHeightLiveOverride,
    pub height_overrides: BTreeMap<TerrainId, SparseHeightLiveOverride>,
    pub height_bindings: super::tool_brush::HeightSourceBindings,
    pub placements: Option<super::placement_update::Prepared>,
    pub clear_vegetation: bool,
    pub clear_water: bool,
}

/// 只允许在后台调用；完整 JSON、所有页和路径通过以后才返回一个可发布值。
pub(super) fn prepare(
    bytes: &[u8],
    map: &str,
    transaction: u64,
    revision: u64,
    project: &crate::project::ProjectContext,
    previous: &Files,
    generation: u64,
) -> Result<PreparedSnapshot, String> {
    let snapshot: SceneSnapshot =
        serde_json::from_slice(bytes).map_err(|e| format!("作者快照无效：{e}"))?;
    let id = SceneId::parse(map)?;
    if snapshot.entry.id != id
        || snapshot.manifest.id != id
        || snapshot.placements.scene_id != id
        || snapshot.portals.scene_id != id
        || snapshot.manifest.schema_version != SCENE_SCHEMA_VERSION
        || !snapshot.manifest.is_data_driven()
    {
        return Err("作者快照的场景、版本或运行类型不一致".into());
    }
    if !snapshot
        .manifest
        .default_spawn
        .iter()
        .all(|value| value.is_finite())
    {
        return Err("作者快照起始位置无效".into());
    }
    let mut files = snapshot.asset_overrides;
    // 未保存目录只存在内存，保留本工程其他场景条目和正式路径。读取/编码均在后台。
    let path = project.asset_root().join(SCENE_CATALOG_ASSET_PATH);
    let mut catalog: SceneCatalogAsset =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if let Some(entry) = catalog.scenes.iter_mut().find(|entry| entry.id == id) {
        *entry = snapshot.entry.clone();
    } else {
        catalog.scenes.push(snapshot.entry.clone());
    }
    add_json(&mut files, SCENE_CATALOG_ASSET_PATH.to_owned(), &catalog)?;
    add_json(
        &mut files,
        snapshot.entry.manifest_path(),
        &snapshot.manifest,
    )?;
    add_json(
        &mut files,
        snapshot.entry.placements_path(),
        &snapshot.placements,
    )?;
    add_json(&mut files, snapshot.entry.portals_path(), &snapshot.portals)?;
    if let Some(value) = snapshot.boundaries {
        if value.scene_id != id {
            return Err("边界快照属于另一场景".into());
        }
        add_json(
            &mut files,
            snapshot
                .entry
                .boundary_path
                .clone()
                .ok_or("边界快照缺少目录路径")?,
            &value,
        )?;
    }
    if let Some(value) = snapshot.surface {
        if value.scene_id != id {
            return Err("地表快照属于另一场景".into());
        }
        add_json(
            &mut files,
            snapshot
                .entry
                .surface_path
                .clone()
                .ok_or("地表快照缺少目录路径")?,
            &value,
        )?;
    }
    if let Some(value) = snapshot.vegetation {
        if value.scene_id != id {
            return Err("植被快照属于另一场景".into());
        }
        // 密度纹理由作者的生产 KTX2 编码器提供；原始 R8 也交给同一个只读来源。
        for layer in &value.layers {
            if let Some(source) = snapshot.vegetation_densities.get(&layer.id) {
                validate_plane(layer.resolution, source.len())?;
                files.insert(layer.density_source_path.clone(), source.clone());
                if !files.contains_key(&layer.density_texture_path) {
                    return Err(format!("植被层 {} 缺少同批运行纹理", layer.id));
                }
            }
        }
        add_json(
            &mut files,
            snapshot
                .entry
                .vegetation_path
                .clone()
                .ok_or("植被快照缺少目录路径")?,
            &value,
        )?;
    }
    if let Some(value) = snapshot.water {
        if value.scene_id != id {
            return Err("水域快照属于另一场景".into());
        }
        for layer in &value.layers {
            if let Some(source) = snapshot.water_masks.get(&layer.id) {
                validate_plane(layer.resolution, source.len())?;
                files.insert(layer.mask_source_path.clone(), source.clone());
                if !files.contains_key(&layer.mask_texture_path) {
                    return Err(format!("水域层 {} 缺少同批运行纹理", layer.id));
                }
            }
        }
        add_json(
            &mut files,
            snapshot
                .entry
                .water_path
                .clone()
                .ok_or("水域快照缺少目录路径")?,
            &value,
        )?;
    }
    let mut heights = SparseHeightLiveOverride::default();
    let height_revision = super::systems::next_height_source_revision()?;
    let mut height_bindings = super::tool_brush::HeightSourceBindings::new(id.clone(), revision);
    if let Some(path) = sparse_height_path(&snapshot.manifest.terrain)? {
        height_bindings.declare(None, path)?;
    }
    if let Some(terrains) = &snapshot.manifest.terrains {
        for object in terrains.iter() {
            if let Some(path) = sparse_height_path(&object.terrain)? {
                height_bindings.declare(Some(object.id.clone()), path)?;
            }
        }
    } // 在后台声明稳定来源；缺席live与首次空live都为0时，不能等待一次未必发生的完整快照重建。
    if let Some(author_revision) = snapshot.authoring_height_revision
        && let Some(path) = height_path(&snapshot.manifest, snapshot.terrain_id.as_ref())? {
        height_bindings.insert(snapshot.terrain_id.clone(), path,
            author_revision, height_revision)?;
    }
    if snapshot.authoring_height_revision.is_some() || !snapshot.sparse_height_tiles.is_empty() {
        heights.retarget(Some(id.clone()), height_revision);
        let tiles = snapshot
            .sparse_height_tiles
            .into_iter()
            .map(|tile| SparseHeightTile::decode_asset_bytes(tile.key, &tile.bytes).map(Arc::new))
            .collect::<Result<Vec<_>, _>>()?;
        heights.publish_tiles(&id, height_revision, tiles)?;
    }
    let mut height_overrides = BTreeMap::new();
    if let Some(terrain) = snapshot.terrain_id {
        TerrainId::parse(terrain.as_str())?;
        height_overrides.insert(terrain, std::mem::take(&mut heights));
    }
    for value in snapshot.height_overrides {
        TerrainId::parse(value.terrain_id.as_str())?;
        if let Some(author_revision) = value.authoring_height_revision
            && let Some(path) = height_path(&snapshot.manifest, Some(&value.terrain_id))? {
            height_bindings.insert(Some(value.terrain_id.clone()), path,
                author_revision, height_revision)?;
        }
        let mut height = SparseHeightLiveOverride::default();
        height.retarget(Some(id.clone()), height_revision);
        let tiles = value
            .tiles
            .into_iter()
            .map(|tile| SparseHeightTile::decode_asset_bytes(tile.key, &tile.bytes).map(Arc::new))
            .collect::<Result<Vec<_>, _>>()?;
        height.publish_tiles(&id, height_revision, tiles)?;
        if height_overrides.insert(value.terrain_id, height).is_some() {
            return Err("高度快照重复绑定相同地形".into());
        }
    }
    let total = files.iter().try_fold(0usize, |total, (path, bytes)| {
        if !valid_path(path) {
            return Err(format!("快照资产路径越界：{path}"));
        }
        total
            .checked_add(bytes.len())
            .ok_or_else(|| "快照资产字节数溢出".to_owned())
    })?;
    if total > 128 * 1024 * 1024 {
        return Err("完整作者资产超过 128 MiB 传输预算；未采用部分结果".into());
    }
    // 仅内容真正改变的路径触发生产热更；相机移动和纯摆放编辑不能让全部场景资源重载。
    let (paths, reload_paths) = changed_paths(&files, previous);
    let clear_vegetation = reload_paths.iter().any(|path| path == &snapshot.entry.manifest_path()
        || path == VEGETATION_PRESET_CATALOG_ASSET_PATH)
        || vegetation_sources_changed(&files, previous, &reload_paths);
    let clear_water = reload_paths.iter().any(|path| path == &snapshot.entry.manifest_path()
        || path == WATER_PRESET_CATALOG_ASSET_PATH)
        || water_sources_changed(&files, previous, &reload_paths);
    let placement_path = snapshot.entry.placements_path();
    // 仅摆放JSON变化时才允许无重建采用；其它来源变动可能撤销模型/碰撞身份。
    let placements = if reload_paths.len() == 1 && reload_paths[0] == placement_path {
        previous.get(&placement_path).and_then(|old| super::placement_update::prepare(
            placement_path, old, snapshot.placements, snapshot.manifest.world_bounds()?))
    } else { None };
    let reads = Arc::new(
        paths
            .iter()
            .map(|path| (path.clone(), Default::default()))
            .collect(),
    );
    Ok(PreparedSnapshot {
        transaction,
        revision,
        generation,
        scene_id: id,
        name: snapshot.manifest.display_name,
        spawn: snapshot.manifest.default_spawn,
        paths,
        reload_paths,
        reads,
        placements,
        clear_vegetation,
        clear_water,
        files: Arc::new(files),
        heights,
        height_overrides,
        height_bindings,
    })
}

/// 绑定路径只从同批完整清单取得；任意笔刷路径不能授权其它地形或工程资产。
fn height_path(manifest: &SceneManifestAsset, terrain: Option<&TerrainId>) -> Result<Option<String>, String> {
    let descriptor = if let Some(id) = terrain {
        &manifest.terrains.as_ref().and_then(|terrains| terrains.iter().find(|terrain| terrain.id == *id))
            .ok_or("高度来源没有对应的地形对象")?.terrain
    } else { &manifest.terrain };
    sparse_height_path(descriptor)
}

fn sparse_height_path(descriptor: &SceneTerrainDescriptor) -> Result<Option<String>, String> {
    if descriptor.kind != SceneTerrainKind::SparseClipmap { return Ok(None); }
    descriptor.height_path.as_ref().filter(|path| valid_path(path)).cloned().map(Some)
        .ok_or_else(|| "作者高度来源缺少规范的清单路径".into())
}

/// 被移除的草稿也需要失效旧缓存，但其临时文件可能本来就没有磁盘副本。
/// 它们不属于新快照的就绪依赖；实际新场景的依赖仍由生产场景就绪门核对。
fn vegetation_sources_changed(files: &Files, previous: &Files, changed: &[String]) -> bool {
    let changed: std::collections::BTreeSet<_> = changed.iter().map(String::as_str).collect();
    for (path, bytes) in files.iter().chain(previous.iter()).filter(|(path, _)| path.ends_with(".scene_vegetation.json")) {
        if changed.contains(path.as_str()) { return true; }
        if let Ok(definition) = serde_json::from_slice::<SceneVegetationAsset>(bytes) {
            if definition.layers.iter().any(|layer| changed.contains(layer.density_source_path.as_str())
                || changed.contains(layer.density_texture_path.as_str())) { return true; }
        }
    }
    false
}

fn water_sources_changed(files: &Files, previous: &Files, changed: &[String]) -> bool {
    let changed: std::collections::BTreeSet<_> = changed.iter().map(String::as_str).collect();
    for (path, bytes) in files.iter().chain(previous.iter()).filter(|(path, _)| path.ends_with(".scene_water.json")) {
        if changed.contains(path.as_str()) { return true; }
        if let Ok(definition) = serde_json::from_slice::<SceneWaterAsset>(bytes) {
            if definition.layers.iter().any(|layer| changed.contains(layer.mask_source_path.as_str())
                || changed.contains(layer.mask_texture_path.as_str())) { return true; }
        }
    }
    false
}

pub(super) fn changed_paths(files: &Files, previous: &Files) -> (Vec<String>, Vec<String>) {
    let paths: Vec<_> = files
        .iter()
        .filter(|(path, bytes)| {
            previous
                .get(*path)
                .is_none_or(|old| old.as_ref() != bytes.as_ref())
        })
        .map(|(path, _)| path.clone())
        .collect();
    let reload_paths = paths
        .iter()
        .cloned()
        .chain(
            previous
                .keys()
                .filter(|path| !files.contains_key(*path))
                .cloned(),
        )
        .collect();
    (paths, reload_paths)
}

fn add_json<T: serde::Serialize>(
    files: &mut BTreeMap<String, Arc<Vec<u8>>>,
    path: String,
    value: &T,
) -> Result<(), String> {
    files.insert(
        path,
        Arc::new(serde_json::to_vec(value).map_err(|e| e.to_string())?),
    );
    Ok(())
}

fn validate_plane(size: [u32; 2], bytes: usize) -> Result<(), String> {
    if size.contains(&0) || u64::from(size[0]) * u64::from(size[1]) != bytes as u64 {
        Err("快照 R8 画布尺寸与完整字节数不一致".into())
    } else {
        Ok(())
    }
}
