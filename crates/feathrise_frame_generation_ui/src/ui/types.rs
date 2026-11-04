//! 独立UI的有限GPU记录；不包含ECS系统或原生对象所有权。
use bevy::{
    prelude::*,
    render::{render_resource::*, texture::CachedTexture},
}; // 小型GPU包装句柄和尺寸。

pub(super) struct UiBinding {
    // 一张主颜色纹理对应一个可复用采样绑定。
    pub view: TextureViewId, // 主纹理视图身份用于安全复用。
    pub group: BindGroup,    // 同一布局下的单纹理绑定。
} // UiBinding结束。

pub(super) struct UiTargets {
    // 持久强引用防止SDK借用尚未消费时外部纹理提前销毁。
    pub size: UVec2,           // 只有规格变化才替换输出集合。
    pub format: TextureFormat, // 与窗口sRGB视图逐位匹配。
    pub ui: CachedTexture,     // 真正透明背景累积出的预乘Alpha覆盖层。
} // UiTargets结束。

pub(super) struct ActiveUi {
    // begin成功后必须被同视图finish消费，不能半途丢弃普通颜色。
    pub camera: Entity,          // 唯一主世界玩法相机身份。
    pub frame: u32,              // 禁止跨帧复用捕获状态。
    pub ui_binding: BindGroup,   // UI正在写入的另一主纹理采样。
    pub encode: RenderPipeline,  // 已编译且强引用保留的sRGB输出管线。
    pub restore: RenderPipeline, // 已编译的线性预乘混合管线。
} // ActiveUi结束。
