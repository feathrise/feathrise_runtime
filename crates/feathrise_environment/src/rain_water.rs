//! 雨丝水面裁切的跨领域契约：scene生成局部GPU高度图，atmosphere只消费完整视图范围。
use bevy::asset::uuid_handle;
use bevy::prelude::*; // 图像身份、世界范围与发布顺序。 // 固定句柄避免天气与场景之间循环依赖。

pub const RAIN_WATER_SPAN: f32 = 512.0; // 生产相机与雨丝片元共用固定米制跨度，UV通道只需传递窗口原点。

#[derive(Resource, Clone, Debug, PartialEq)] // 单一局部目标保持图像与范围同帧发布。
pub struct RainWaterMask {
    // 高度图RGBA16F：RG分段编码世界Y，B为有效水面覆盖。
    pub image: Handle<Image>, // 专用相机渲染目标；雨丝通过Hanabi材质采样。
    pub bounds: Vec4,         // minX、minZ、sizeX、sizeZ，范围外不能Clamp成水。
    pub active: bool,         // 失效、切图或停雨时立即停止采样旧图。
} // 完成共享只读数据契约。

impl Default for RainWaterMask {
    // 独立天气测试没有场景生产者时不误挡雨。
    fn default() -> Self {
        // 图像资产由水域生产端创建。
        Self {
            image: uuid_handle!("af72d5cc-f9c2-4be0-a7f2-42ae8f96e718"),
            bounds: Vec4::ZERO,
            active: false,
        } // 缺图状态明确无水。
    } // 完成默认资源构造。
} // 完成固定图像身份。

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)] // 消费者在水参数与相机范围发布完成后同步属性。
pub struct RainWaterMaskUpdate; // Render提取前完成同帧生产/消费顺序。
