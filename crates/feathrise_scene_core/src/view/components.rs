//! 场景画质、天气和辅助渲染的最小组件合同；应用自行决定哪台相机取得资格。

use bevy::prelude::*;

/// 主要场景视图资格：环境、画质与帧生成仅接管显式声明的窗口场景相机。
///
/// 此标记不创建相机、不选择投影或控制方式，也不意味着存在玩家。
/// 消费者仍需执行原有唯一相机、活动状态及目标窗口检查，不能由标记绕过租约。
#[derive(Component, Default)]
pub struct PrimarySceneView;

/// 共享材质切到 Deferred 时，允许离屏 PBR 视图取得最小兼容租约。
/// 此资格不授权 Solari、DLSS、HDR、运动历史或窗口画质设置传播到辅助视图。
#[derive(Component)]
pub struct DeferredRasterAuxiliaryCamera;

/// 纯视觉网格不参与观察射线遮挡；是否执行避障以及射线算法仍由应用拥有。
#[derive(Component)]
pub struct CameraRaycastIgnore;

/// 显式声明的世界界面颜色网格可由帧生成后端独立合成；不隐式包含其父级或其他网格。
#[derive(Component, Clone, Copy, Debug)]
pub struct FrameGenerationUi;
