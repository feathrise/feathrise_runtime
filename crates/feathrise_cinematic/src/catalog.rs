//! 启动时在后台发现独立演出文档，移动或重命名文件不影响规则和存档中的演出 ID。
use crate::Cinematic;
use bevy_ecs::prelude::Resource;
use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

pub fn is_cinematic_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".cinematic.json"))
}

#[derive(Clone, Debug, Default, Resource)]
pub struct CinematicCatalog {
    pub ready: bool,
    pub error: Option<String>,
    documents: BTreeMap<String, Arc<Cinematic>>,
}
impl CinematicCatalog {
    pub fn get(&self, id: &str) -> Option<Arc<Cinematic>> {
        self.documents.get(id).cloned()
    }
    pub fn iter(&self) -> impl Iterator<Item = &Arc<Cinematic>> {
        self.documents.values()
    }
    pub fn from_documents(documents: impl IntoIterator<Item = Cinematic>) -> Result<Self, String> {
        let mut catalog = Self {
            ready: true,
            ..Self::default()
        };
        for document in documents {
            catalog.insert(document)?;
        }
        Ok(catalog)
    }
    fn insert(&mut self, document: Cinematic) -> Result<(), String> {
        document.validate()?;
        let id = document.id.clone();
        if self.documents.contains_key(&id) {
            return Err(format!("重复的剧情演出 ID {id}"));
        }
        self.documents.insert(id, Arc::new(document));
        Ok(())
    }
    /// 只遍历当前资产根普通目录；仓外符号链接或 Windows 重解析点不属于本工程。
    pub fn load(root: &Path) -> Result<Self, String> {
        let mut result = Self {
            ready: true,
            ..Self::default()
        };
        let mut dirs = vec![root.to_path_buf()];
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir).map_err(|e| format!("{}：{e}", dir.display()))? {
                let entry = entry.map_err(|e| e.to_string())?;
                let meta = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
                if meta.is_symlink() {
                    continue;
                }
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if meta.file_attributes() & 0x400 != 0 {
                        continue;
                    }
                }
                let path = entry.path();
                if meta.is_dir() {
                    dirs.push(path);
                } else if meta.is_file() && is_cinematic_path(&path) {
                    let bytes = fs::read(&path).map_err(|e| format!("{}：{e}", path.display()))?;
                    let doc = Cinematic::from_slice(&bytes)
                        .map_err(|e| format!("{}：{e}", path.display()))?;
                    result
                        .insert(doc)
                        .map_err(|e| format!("{}：{e}", path.display()))?;
                }
            }
        }
        Ok(result)
    }
}
