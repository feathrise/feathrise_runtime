//! 应用显式提供的场景视图资格；只描述渲染消费者需要的身份，不包含相机控制或角色跟随。

mod components;

pub use components::{
    CameraRaycastIgnore, DeferredRasterAuxiliaryCamera, FrameGenerationUi, PrimarySceneView,
};
