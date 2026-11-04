//! 世界 UI 独立透明绘制的同帧 GPU 输出；界面合成域只消费完整、当前帧结果。
use bevy::{prelude::*, render::texture::CachedTexture}; // 使用 Bevy 纹理所有权，避免原生句柄悬空。
use std::sync::atomic::{AtomicBool, Ordering}; // Core3d 绘制只读 World 中可发布固定大小完成事实。

#[derive(Component)] // 仅挂在本次被授权的唯一玩法渲染视图。
pub struct WorldUiLayer {
    // 原主深度负责遮挡，输出分辨率UI纹理负责清晰显示与去抖动。
    pub texture: CachedTexture, // 全附件尺寸与主深度一致，独立颜色层使用 render_size 确定有效绘制范围。
    pub mapped_depth: CachedTexture, // 输出分辨率无抖动遮挡附件，不能直接复用内部viewport。
    pub depth_drawn: AtomicBool, // 本帧深度映射完整后才可绘制名牌。
    pub internal_size: UVec2,   // 本帧主视图真正绘制的低分辨率 viewport，失败回退时仍使用它。
    pub render_size: UVec2, // 独立颜色层实际绘制尺寸；头顶UI固定使用完整输出分辨率，避免再次放大低清纹理。
    pub output_size: UVec2, // 最终透明 UI 与窗口的物理尺寸。
    // 独立颜色层中的所有 FrameGenerationUi 材质使用无抖动投影，合成阶段不再做二次位移。
    pub camera: Entity,     // 主世界玩法相机身份，辅助视图不能使用本层。
    pub frame: u32,         // Layer 不允许跨帧复用旧内容。
    pub drawn: AtomicBool,  // 只有本帧独立 phase 成功录制后才置真。
    pub failed: AtomicBool, // 已分流后绘制失败必须让本帧 FG 回退，不能重新把世界 UI 送去插值。
} // WorldUiLayer 结束。

impl WorldUiLayer {
    // 给合成域提供明确的非阻塞完成查询。
    pub fn is_drawn(&self) -> bool {
        self.drawn.load(Ordering::Acquire)
    } // 不等待 GPU，只确认同队列命令已经完整记录。
    pub fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    } // begin_ui 读取当前帧事务失败事实。
} // WorldUiLayer 方法结束。
