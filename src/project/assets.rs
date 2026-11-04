//! 工程运行时的资产来源；沿用Bevy文件读取与监听，仅校验当前工程来源，不套原型内容预算。

use crate::project::ProjectContext;
use bevy::{
    asset::{
        AssetApp, AssetMode, AssetPlugin,
        io::{
            AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, PathStream, Reader,
            file::FileAssetReader,
        },
    },
    prelude::App,
    tasks::IoTaskPool,
};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

/// 每个工程的 Bevy App 固定自身资产来源；嵌入会话可在默认根覆盖完整作者快照，普通运行只读工程文件。
/// 两种入口都不向SDK或其它工程回退同名文件。
pub(super) fn install(app: &mut App, project: &ProjectContext) -> Result<AssetPlugin, String> {
    let overlay = app.world().get_resource::<crate::project::embedded::asset_overlay::AssetOverlay>().cloned();
    let first = project.asset_roots.first().ok_or("工程未登记默认资产根")?;
    for (index, root) in project.asset_roots.iter().enumerate() {
        let root = root
            .canonicalize()
            .map_err(|error| format!("工程资产根不可访问：{}：{error}", root.display()))?;
        checked_location(&root, Path::new(""))
            .map_err(|error| format!("工程资产根不可用：{error}"))?;
        // 保留生产FileWatcher，替换的reader只负责来源归属，不再检查图片尺寸/GLB节点/文件容量。
        let overlay = (index == 0).then(|| overlay.clone()).flatten();
        let mut source = AssetSourceBuilder::platform_default(&root.to_string_lossy(), None)
            .with_reader(move || Box::new(ProductionProjectReader { root: root.clone(),
                inner: FileAssetReader::new(&root), overlay: overlay.clone() }));
        source.writer = None;
        let id = if index == 0 {
            AssetSourceId::Default
        } else {
            AssetSourceId::from(format!("root{index}"))
        };
        app.register_asset_source(id, source);
    }
    Ok(AssetPlugin {
        file_path: first.to_string_lossy().into_owned(),
        mode: AssetMode::Unprocessed,
        watch_for_changes_override: Some(true),
        ..Default::default()
    })
}

struct ProductionProjectReader {
    root: PathBuf,
    inner: FileAssetReader,
    overlay: Option<crate::project::embedded::asset_overlay::AssetOverlay>,
}

impl ProductionProjectReader {
    async fn check(&self, path: &Path) -> Result<(), AssetReaderError> {
        let root = self.root.clone();
        let path = path.to_owned();
        // 文件系统归属检查放在IO池；正式解码、格式支持和资源大小仍由生产loader与GPU决定。
        IoTaskPool::get()
            .spawn(async move { checked_location(&root, &path) })
            .await
    }
}

fn checked_location(root: &Path, relative: &Path) -> Result<(), AssetReaderError> {
    let denied = || {
        AssetReaderError::from(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "资产必须属于当前工程登记根，不能经过父跳转、符号链接或junction",
        ))
    };
    if relative
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(denied());
    }
    let metadata = fs::symlink_metadata(root)?;
    if !metadata.is_dir() || !crate::project::context::ordinary(&metadata) {
        return Err(denied());
    }
    if root.canonicalize()? != root {
        return Err(denied());
    }
    let full = root.join(relative);
    let canonical = match full.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AssetReaderError::NotFound(full));
        }
        Err(error) => return Err(error.into()),
    };
    if !canonical.starts_with(root) {
        return Err(denied());
    }
    let mut current = root.to_owned();
    for part in relative.components() {
        current.push(part);
        if !crate::project::context::ordinary(&fs::symlink_metadata(&current)?) {
            return Err(denied());
        }
    }
    Ok(())
}

impl AssetReader for ProductionProjectReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        if let Some(reader) = self.overlay.as_ref().and_then(|overlay| overlay.read(path)) {
            return Ok(Box::new(reader) as Box<dyn Reader>);
        }
        self.check(path).await?;
        self.inner.read(path).await.map(|reader| Box::new(reader) as Box<dyn Reader>)
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let mut meta = path.as_os_str().to_owned();
        meta.push(".meta");
        self.check(Path::new(&meta)).await?;
        self.inner.read_meta(path).await
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        self.check(path).await?;
        self.inner.read_directory(path).await
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        self.check(path).await?;
        self.inner.is_directory(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::{RenderAssetUsages, io::AssetSource, io::AssetSourceBuilders},
        prelude::Image,
        render::render_resource::{Extent3d, TextureDimension, TextureFormat},
        tasks::{TaskPoolBuilder, block_on},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "production-game-assets-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }

        fn project(&self, name: &str, color: [u8; 4]) -> ProjectContext {
            let root = self.0.join(name);
            let assets = root.join("assets");
            fs::create_dir_all(&assets).unwrap();
            fs::create_dir(root.join("secondary")).unwrap();
            Image::new_fill(
                Extent3d {
                    width: 4096,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                &color,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::MAIN_WORLD,
            )
            .try_into_dynamic()
            .unwrap()
            .save(assets.join("same.png"))
            .unwrap();
            ProjectContext {
                id: name.into(),
                name: name.into(),
                root: root.clone(),
                asset_roots: vec![assets, root.join("secondary")],
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let temporary = std::env::temp_dir().canonicalize().unwrap();
            assert!(self.0.starts_with(temporary));
            assert!(
                self.0
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("production-game-assets-")
            );
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn source(project: &ProjectContext, id: AssetSourceId<'static>) -> AssetSource {
        let mut app = App::new();
        let assets = install(&mut app, project).unwrap();
        assert_eq!(assets.watch_for_changes_override, Some(true));
        let mut sources = app.world_mut().resource_mut::<AssetSourceBuilders>();
        assert!(sources.get_mut("hot").is_none());
        let source = sources.get_mut(&id).unwrap();
        assert!(source.watcher.is_some());
        assert!(source.writer.is_none());
        source.build(id, false, false) // CPU回归不实际启动监听线程。
    }

    fn read(source: &AssetSource, path: &str) -> Result<Vec<u8>, AssetReaderError> {
        block_on(async {
            let mut reader = source.reader().read(Path::new(path)).await?;
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;
            Ok(bytes)
        })
    }

    #[test]
    fn production_sources_read_large_images_from_own_project_without_prototype_or_snapshot() {
        IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(1).build());
        let fixture = Fixture::new();
        let a = fixture.project("A", [255, 0, 0, 255]);
        let b = fixture.project("B", [0, 0, 255, 255]);
        let own_a = source(&a, AssetSourceId::Default);
        let own_b = source(&b, AssetSourceId::Default);
        let bytes_a = read(&own_a, "same.png").unwrap();
        let bytes_b = read(&own_b, "same.png").unwrap();
        assert_eq!(
            bytes_a,
            fs::read(a.asset_roots[0].join("same.png")).unwrap()
        );
        assert_eq!(
            bytes_b,
            fs::read(b.asset_roots[0].join("same.png")).unwrap()
        );
        assert_ne!(bytes_a, bytes_b);
        assert_eq!(
            u32::from_be_bytes(bytes_a[16..20].try_into().unwrap()),
            4096
        );
        let secondary = source(&a, AssetSourceId::from("root1"));
        assert!(matches!(
            read(&secondary, "same.png"),
            Err(AssetReaderError::NotFound(_))
        ));
        assert!(read(&own_a, "../secondary/same.png").is_err());
    }
}
