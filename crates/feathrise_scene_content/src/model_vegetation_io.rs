//! 模型植被的分块持久源；仅由作者保存/加载或运行时唯一后台任务调用。
use crate::{ModelSurfaceVegetationSample, SceneId, SceneVegetationAsset, SurfaceTarget};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::{Component, Path}, sync::Arc};

pub(crate) const BLOCK_SAMPLES: usize = 4096;
pub(crate) const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(bevy::asset::Asset, bevy::reflect::TypePath, Serialize, Deserialize)]
pub(crate) struct Block {
    pub(crate) format: u32,
    pub(crate) scene_id: SceneId,
    pub(crate) target: SurfaceTarget,
    pub(crate) topology: [u8; 32],
    pub(crate) ordinal: usize,
    pub(crate) samples: Vec<ModelSurfaceVegetationSample>,
    #[serde(skip)]
    pub(crate) source_bytes: usize,
}

/// IO加载器和作者磁盘读取使用同一块身份，路径不能借场景相邻目录串入其他作者数据。
pub(crate) fn validate_block_path(folder: &str, relative: &str) -> Result<(), String> {
    let path = Path::new(relative);
    if !path.components().all(|p| matches!(p, Component::Normal(_)))
        || !path.starts_with(folder) || relative.contains(['\\',':']) {
        return Err("模型植被块必须位于所属场景私有目录".into());
    }
    Ok(())
}

fn private_path(root: &Path, folder: &str, relative: &str) -> Result<std::path::PathBuf, String> {
    validate_block_path(folder, relative)?;
    let path = Path::new(relative);
    let full = root.join(path);
    if full.exists() && !full.canonicalize().map_err(|e| e.to_string())?
        .starts_with(root.join(folder).canonicalize().map_err(|e| e.to_string())?) {
        return Err("模型植被块的实际路径越过所属场景私有目录".into());
    }
    Ok(full)
}

/// 读取完整块集合后再发布；丢块、身份错配或预算溢出均不返回半份密度。
pub fn load_model_vegetation_blocks(root: &Path, folder: &str, asset: &mut SceneVegetationAsset) -> Result<(), String> {
    let mut bytes = 0usize;
    let mut targets = BTreeSet::new();
    for shared in Arc::make_mut(&mut asset.model_surfaces) {
        let surface = Arc::make_mut(shared);
        surface.validate()?;
        if !targets.insert(serde_json::to_string(&surface.target).map_err(|e| e.to_string())?) {
            return Err("模型植被有重复目标".into());
        }
        if surface.sample_blocks.is_empty() { continue; }
        if !surface.samples.is_empty() { return Err("模型植被不能同时声明内联样本和分块源".into()); }
        let mut samples = Vec::new();
        for (ordinal, relative) in surface.sample_blocks.iter().enumerate() {
            let path = private_path(root, folder, relative)?;
            let length = std::fs::metadata(&path).map_err(|e| format!("读取模型植被块{}：{e}", path.display()))?.len();
            bytes = bytes.checked_add(usize::try_from(length).map_err(|_| "模型植被块过大")?)
                .filter(|n| *n <= MAX_BYTES).ok_or("模型植被超过64MiB读取预算")?;
            let block: Block = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            if block.format != 1 || block.scene_id != asset.scene_id || block.target != surface.target
                || block.topology != surface.topology || block.ordinal != ordinal || block.samples.len() > BLOCK_SAMPLES {
                return Err("模型植被块身份、顺序或样本数量不一致".into());
            }
            samples.extend(block.samples);
        }
        surface.samples = samples;
        surface.sample_blocks.clear(); // 内存采用后只有一个权威载体；下一次保存重新派生内容寻址块。
        surface.validate()?;
    }
    Ok(())
}

/// 生成内容寻址块和小型文档；调用方把两者加入原有同批回滚保存事务。
pub fn prepare_model_vegetation_blocks(folder: &str, asset: &SceneVegetationAsset)
    -> Result<(SceneVegetationAsset, Vec<(String, Vec<u8>)>), String> {
    let mut metadata = asset.clone();
    let mut writes = Vec::new();
    let mut total = 0usize;
    for shared in Arc::make_mut(&mut metadata.model_surfaces) {
        let surface = Arc::make_mut(shared);
        surface.validate()?;
        if !surface.sample_blocks.is_empty() { return Err("模型植被分块尚未完整载入，不能保存".into()); }
        for (ordinal, samples) in surface.samples.chunks(BLOCK_SAMPLES).enumerate() {
            let block = Block { format: 1, scene_id: asset.scene_id.clone(), target: surface.target.clone(),
                topology: surface.topology, ordinal, samples: samples.to_vec(), source_bytes: 0 };
            let bytes = serde_json::to_vec(&block).map_err(|e| e.to_string())?;
            total = total.checked_add(bytes.len()).filter(|n| *n <= MAX_BYTES).ok_or("模型植被超过64MiB保存预算")?;
            let hash = crate::authoring_io::bytes_hash(&bytes);
            let relative = format!("{}/vegetation/models/{hash}.json", folder.trim_end_matches('/'));
            surface.sample_blocks.push(relative.clone());
            writes.push((relative, bytes));
        }
        surface.samples.clear();
    }
    Ok((metadata, writes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelSurfaceAnchor, ModelSurfaceVegetation};

    #[test]
    fn model_blocks_roundtrip_and_missing_or_wrong_identity_cannot_load() {
        let parent = std::env::temp_dir();
        let root = parent.join(format!("feathrise-model-vegetation-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir(&root).unwrap(); // 独占夹具，不能复用普通作者资产目录。
        let folder = "scenes/model-fixture";
        std::fs::create_dir_all(root.join(folder)).unwrap();
        let mut surface = ModelSurfaceVegetation::empty("plane", [3;32], 0.25);
        surface.samples = (0..4100).map(|triangle|ModelSurfaceVegetationSample {
            anchor: ModelSurfaceAnchor { primitive:0, triangle, barycentric:[0.2,0.3] },
            layers: vec![("grass".into(), 200)],
        }).collect();
        let original = SceneVegetationAsset { scene_id:SceneId::parse("MAP-model-fixture").unwrap(),
            model_surfaces:vec![Arc::new(surface)].into(), ..Default::default() };
        let (metadata, writes) = prepare_model_vegetation_blocks(folder, &original).unwrap();
        assert_eq!(writes.len(),2, "4096样本一块，尾块不能丢失");
        assert!(metadata.model_surfaces[0].samples.is_empty());
        assert_eq!(original.model_surfaces[0].samples.len(),4100, "保存编码不得清掉作者数据");
        assert!(load_model_vegetation_blocks(&root, folder, &mut metadata.clone()).is_err());
        for (relative, bytes) in &writes {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path,bytes).unwrap();
        }
        let mut loaded = metadata.clone();
        load_model_vegetation_blocks(&root, folder, &mut loaded).unwrap();
        assert_eq!(loaded.model_surfaces, original.model_surfaces);
        let mut wrong_scene = metadata.clone();
        wrong_scene.scene_id = SceneId::parse("MAP-wrong-owner").unwrap();
        assert!(load_model_vegetation_blocks(&root, folder, &mut wrong_scene).is_err());
        let mut wrong_topology = metadata;
        Arc::make_mut(&mut Arc::make_mut(&mut wrong_topology.model_surfaces)[0]).topology = [4;32];
        assert!(load_model_vegetation_blocks(&root, folder, &mut wrong_topology).is_err());
        assert_eq!(root.parent(),Some(parent.as_path()));
        std::fs::remove_dir_all(root).unwrap(); // 只清理由本测试独占创建的临时根。
    }
}
