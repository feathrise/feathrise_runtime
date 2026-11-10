//! 高度图仅使用独立相机与真实水面的子代理，不增加普通玩法实体。
use bevy::prelude::*; // 独立标记组件。

#[derive(Component)] // 唯一顶视相机，与玩法和作者相机查询互斥。
pub(super) struct RainMaskCamera; // 没有PrimarySceneView或天气预览标记。

#[derive(Component)] // 子代理随源实体despawn自动清理。
pub(super) struct RainMaskProxy; // 不安装DynamicWaterSurface、碰撞或遮雨组件。
