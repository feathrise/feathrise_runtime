//! Feathrise 公共 SDK：工程运行入口与公共运行装配。
//!
//! project 管理工程启动和运行协作，app 装配已有公共能力；具体能力由同仓子库提供。
//! 模块归并保持系统注册和功能开关，编辑器作者界面继续由编辑器拥有。

pub mod app;
pub mod project;

#[cfg(feature = "default_font")]
pub use app::default_font::{DefaultFontPlugin, DEFAULT_FONT_BYTES, DEFAULT_FONT_LICENSE};

pub use app::{game_plugins, install_vulkan_surface_guard, prepare_vulkan_environment};
pub use project::{
    GameView, ProjectContext, run, validate_saved_map,
    validate_saved_map_with_check,
    AuthorRuntimePreview, AuthorRuntimePreviewScene, AuthorRuntimePreviewStatus, AuthorRuntimePreviewEvidence,
    AUTHOR_RUNTIME_PREVIEW_LAYER,
};
