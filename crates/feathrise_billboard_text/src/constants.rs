// constants.rs — Billboard 文字默认参数常量
//
// 这些是「内置默认值」，调用方可通过 `BillboardText` 字段逐个覆盖，
// 因此本 crate 自带一套合理默认，不依赖外部常量 crate（保持可独立开源）。

/// 默认离屏纹理宽度（像素）。越宽可容纳越长文字而不裁切。
pub const DEFAULT_TEXTURE_WIDTH: u32 = 1024;

/// 默认离屏纹理高度（像素）。
pub const DEFAULT_TEXTURE_HEIGHT: u32 = 128;

/// 默认烘焙字号（仅影响纹理清晰度，不影响世界尺寸）。
pub const DEFAULT_FONT_SIZE: f32 = 80.0;

/// 默认 Billboard 世界空间高度（米）。世界宽度由文字实际像素宽按比例推导。
pub const DEFAULT_WORLD_HEIGHT: f32 = 0.24;

/// 默认水平内边距（像素），避免阴影/抗锯齿被裁掉。
pub const DEFAULT_HORIZONTAL_PADDING_PX: f32 = 24.0;

/// 默认「参考距离」：相机距离小于该值时不再放大（保持原始尺寸）。
pub const DEFAULT_REF_DISTANCE: f32 = 8.0;

/// 默认最大可见距离（米）：超过则隐藏 Billboard。
pub const DEFAULT_MAX_DISTANCE: f32 = 50.0;

/// 固定烘焙相机池大小；再多的 Billboard 请求只排队，不再继续创建相机。
pub const DEFAULT_BAKE_POOL_SIZE: usize = 4;

/// 固定烘焙相机池的渲染层起始编号；每个相机槽位长期复用一个隔离层。
pub const DEFAULT_BASE_LAYER: usize = 200;
