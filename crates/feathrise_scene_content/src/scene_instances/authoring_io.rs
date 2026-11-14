//! 作者保存与删除的引用保护；完整文件遍历只能由已有的有界IO任务调用。

use std::{collections::{BTreeMap, BTreeSet}, io::Read, path::Path, sync::Arc};
use crate::{GENERATED_SCENE_ID_PREFIX, SCENE_SCHEMA_VERSION, SceneCatalogAsset, SceneManifestAsset, SceneId};
use super::{SceneInstanceSource, ExpandedSceneInstance, expand_scene_instances, MAX_EXPANDED_SCENE_INSTANCES};

/// 读取前先限制字节数，并核对真实路径仍在资产根内，不信任清单里的文件夹文本。
fn read_manifest(root: &Path, relative: &str) -> Result<SceneManifestAsset, String> {
    let root = root.canonicalize().map_err(|e| format!("无法读取资产根：{e}"))?;
    let path = root.join(relative).canonicalize().map_err(|e| format!("子场景清单{relative}无法读取：{e}"))?;
    if !path.starts_with(&root) { return Err(format!("子场景清单路径超出资产根：{relative}")); }
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    std::fs::File::open(&path).map_err(|e| e.to_string())?.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES { return Err(format!("子场景清单{relative}超过8MiB预算")); }
    serde_json::from_slice(&bytes).map_err(|e| format!("子场景清单{relative}格式错误：{e}"))
}

fn source(manifest: &SceneManifestAsset) -> Result<Arc<SceneInstanceSource>, String> {
    if manifest.schema_version != SCENE_SCHEMA_VERSION {
        return Err(format!("场景{}清单需要版本 {SCENE_SCHEMA_VERSION}", manifest.id));
    }
    if !manifest.is_data_driven() {
        return Err(format!("场景{}使用不支持的运行类型：{}", manifest.id, manifest.runtime_scene));
    }
    crate::validate_terrain_objects(manifest)?;
    super::validate_scene_instances(&manifest.id, &manifest.subscenes)?;
    Ok(Arc::new(SceneInstanceSource { terrains: manifest.terrains.clone(), scene_id: manifest.id.clone(), revision: 0, instances: manifest.subscenes.clone(),
        world_bounds: manifest.world_bounds().ok_or_else(|| format!("场景{}范围无效", manifest.id))? }))
}

/// 根使用本次待保存草稿，子场景只读取已保存来源；在任何暂存创建前拒绝间接循环和缺失来源。
pub fn validate_saved_scene_instances(root: &Path, catalog: &SceneCatalogAsset, manifest: &SceneManifestAsset) -> Result<Vec<ExpandedSceneInstance>, String> {
    let entries: BTreeMap<_, _> = catalog.scenes.iter().map(|entry| (&entry.id, entry)).collect();
    let mut sources = BTreeMap::from([(manifest.id.clone(), source(manifest)?)]);
    let mut pending: BTreeSet<_> = manifest.subscenes.iter().map(|child| child.source_scene_id.clone()).collect();
    while let Some(id) = pending.pop_first() {
        if sources.contains_key(&id) { continue; }
        if sources.len() >= MAX_EXPANDED_SCENE_INSTANCES { return Err("子场景来源数超过完整展开预算".to_owned()); }
        let entry = entries.get(&id).ok_or_else(|| format!("子场景来源{id}已不在正式目录"))?;
        let saved = read_manifest(root, &entry.manifest_path())?;
        if saved.id != id { return Err(format!("子场景{id}清单身份不匹配")); }
        // 新建场景以 terrains 持有地形对象；显式空列表也是可实例化的场景容器。
        if saved.terrains.is_none() && !matches!(saved.terrain.kind,
            crate::SceneTerrainKind::Gltf | crate::SceneTerrainKind::SparseClipmap) {
            return Err(format!("场景{id}缺少可实例化的场景容器或作者地形"));
        }
        let snapshot = source(&saved)?;
        pending.extend(snapshot.instances.iter().map(|child| child.source_scene_id.clone()));
        sources.insert(id, snapshot);
    }
    expand_scene_instances(&manifest.id, &sources)
}

/// 删除来源前检查全目录直接引用；间接引用必然包含一条直接边，错误清单必须修复后再删。
pub fn saved_scene_referrers(root: &Path, catalog: &SceneCatalogAsset, target: &SceneId) -> Result<Vec<SceneId>, String> {
    let mut referrers = Vec::new();
    for entry in &catalog.scenes {
        if entry.id == *target { continue; }
        let private_folder = format!("scenes/{}", entry.id);
        if entry.id.as_str().starts_with(GENERATED_SCENE_ID_PREFIX) && entry.folder.trim_end_matches('/') == private_folder
            && std::fs::symlink_metadata(root.join(&private_folder)).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound) { continue; }
        let manifest = read_manifest(root, &entry.manifest_path())?;
        if manifest.id != entry.id { return Err(format!("场景{}清单身份不匹配，暂不能安全删除来源", entry.id)); }
        if manifest.subscenes.iter().any(|child| &child.source_scene_id == target) { referrers.push(entry.id.clone()); }
    }
    Ok(referrers)
}
