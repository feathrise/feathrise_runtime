//! 工程共享运行入口；复用场景与渲染及工程数据，可显式接入编辑器常驻视口，不包含作者界面。

mod app;
pub mod author_preview;
pub mod author_visuals;
mod assets;
mod config;
mod context;
pub mod embedded;
// 运行验收需显式编译开启，普通工程不带截图/作者探测实现。
#[cfg(feature = "validation")]
mod smoke;
mod startup;

pub use app::run;
pub use context::ProjectContext;
pub use author_preview::{AuthorRuntimePreview, AuthorRuntimePreviewScene, AuthorRuntimePreviewEvidence,
    AuthorRuntimePreviewStatus, AUTHOR_RUNTIME_PREVIEW_LAYER};
pub use feathrise_scene_core::view::PrimarySceneView;
pub use feathrise_scene::SceneRuntimeObserver;

/// 本次显式保存场景的起始参考点；工程可据此选择自己的视图，不代表角色实例。
#[derive(bevy::prelude::Resource)]
pub struct ProjectViewOrigin(pub bevy::prelude::Vec3);
/// 工程的初始观察角度；两者均为3D，后续导航由工程自己的相机控制器决定。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameView {
    #[default]
    ThirdPerson,
    FixedTopDown,
}
impl GameView {
    pub fn label(self) -> &'static str {
        match self {
            Self::ThirdPerson => "斜向视角 3D",
            Self::FixedTopDown => "俯视视角 3D",
        }
    }
    pub fn load(project: &ProjectContext) -> Result<Self, String> {
        let value = context::read_small_json(&project.root.join("editor-project.json"), 64 * 1024)?;
        value
            .get("game_view")
            .map(|v| serde_json::from_value(v.clone()).map_err(|e| format!("工程视角无效：{e}")))
            .unwrap_or(Ok(Self::default()))
    }
}
pub use startup::{
    validate_saved_scene, validate_saved_scene_with_check, validate_saved_startup_with_check,
};
