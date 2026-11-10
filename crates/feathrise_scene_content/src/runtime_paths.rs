//! 非AssetServer后台数据读取的工程根；运行时不依赖编辑器或进程当前目录。

use bevy::prelude::Resource;
use std::path::{Component, Path, PathBuf};

/// 组合根在启动时安装一次；体素、体积和开发校验将该值克隆进有界后台任务。
///
/// 显式工程根即使缺文件也不会回退SDK同名文件。运行期间不替换该资源；切工程使用新App。
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAssetRoot(PathBuf);

impl RuntimeAssetRoot {
    /// 工程管理应先校验真实目录；这里只做固定路径校验，不在帧关键路径访问磁盘。
    pub fn new(asset_root: impl Into<PathBuf>) -> Result<Self, String> {
        let asset_root = asset_root.into();
        if !asset_root.is_absolute()
            || asset_root
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            return Err(format!(
                "运行时资产根必须是无父级跳转的绝对路径：{}",
                asset_root.display()
            ));
        }
        Ok(Self(asset_root))
    }

    pub fn asset_root(&self) -> &Path {
        &self.0
    }
}

impl Default for RuntimeAssetRoot {
    fn default() -> Self {
        // 未装配工程的独立测试只使用SDK内资产；正式游戏和编辑器入口必须显式注入自己的根。
        Self(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .and_then(Path::parent)
                .expect("scene_content必须位于SDK的crates目录")
                .join("assets"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_runtime_root_rejects_relative_paths_and_parent_traversal() {
        let root = std::env::temp_dir().join("runtime-root-project/assets");
        assert!(RuntimeAssetRoot::new("assets").is_err());
        assert!(RuntimeAssetRoot::new(root.join("../other-project")).is_err());
        let snapshot = RuntimeAssetRoot::new(&root).unwrap();
        assert_eq!(snapshot.asset_root(), root);
        assert_ne!(snapshot, RuntimeAssetRoot::default());
    }
}
