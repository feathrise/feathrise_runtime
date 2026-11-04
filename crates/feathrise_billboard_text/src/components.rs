// components.rs — Billboard 文字 ECS 组件

use bevy::prelude::*;
use bevy::sprite::Text2dShadow;

use crate::constants::{
    DEFAULT_FONT_SIZE, DEFAULT_HORIZONTAL_PADDING_PX, DEFAULT_MAX_DISTANCE, DEFAULT_REF_DISTANCE,
    DEFAULT_TEXTURE_HEIGHT, DEFAULT_TEXTURE_WIDTH, DEFAULT_WORLD_HEIGHT,
};
use crate::materials::{BillboardColorMaterial, BillboardTemporalGuideMaterial};

/// 主相机标记组件。
///
/// 调用方需把该组件加到自己的主 3D 相机上，Billboard 才能朝向它并做距离裁剪。
/// 这样本 crate 无需依赖任何游戏专用的相机标记，保持完全解耦。
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct BillboardCamera;

/// 相机把文字交给独立界面合成时，暂停仅用于场景时间重建的字形引导。
#[derive(Component, Debug, Clone, Copy)] // 通用显式契约，不依赖任何特定超分或帧生成实现。
pub struct BillboardTemporalGuideSuppressed; // 移除后既有相机资格逻辑自动恢复原引导。

/// Billboard 装饰元素：在文字「背后」额外烘焙的一个 2D 圆盘。
///
/// 用于实现「彩色底盘 + 描边圈 + 中心文字」这类复合徽标：把多个圆盘按 `z` 由小到大叠放，
/// 文字始终渲染在所有装饰之上。坐标系与离屏纹理一致（像素，原点居中）。
#[derive(Clone, Debug)]
pub struct BillboardDecoration {
    /// 圆盘半径（像素，基于离屏纹理坐标）。
    pub radius_px: f32,
    /// 圆盘颜色。
    pub color: Color,
    /// 叠放层级（越大越靠上，但始终低于文字）。
    pub z: f32,
}

/// Billboard 文字描边配置。
///
/// 通过在离屏纹理里生成多方向黑色底字来模拟描边，比给名牌加整块背景更干净。
#[derive(Clone, Debug)]
pub struct BillboardTextOutline {
    /// 描边颜色。
    pub color: Color,
    /// 描边像素宽度。
    pub width_px: f32,
}

/// Billboard 文字「请求/规格」组件。
///
/// 把该组件插入到任意实体上：
///   - 若设置 `follow = true`，Billboard 会跟随该实体头顶（owner 即被插入实体）；
///   - 该组件在被处理后会自动从实体上移除（消费语义），生成出独立的 Billboard 实体。
///   - 同一实体再次插入请求会完整替换旧实例；未激活实体上的请求保留到激活后消费。
///
/// 几乎所有字段都有合理默认值（见 `constants.rs`），调用方只需设置 `text` 与 `font`。
#[derive(Component, Clone)]
pub struct BillboardText {
    /// 要显示的文字内容。
    pub text: String,
    /// 字体句柄。
    pub font: Handle<Font>,
    /// 烘焙字号（仅影响纹理清晰度）。
    pub font_size: f32,
    /// 文字颜色。
    pub color: Color,
    /// 可选文字阴影；`None` 表示无阴影。
    pub shadow: Option<Text2dShadow>,
    /// 可选文字描边；用于草地等复杂背景里的头顶名牌清晰度。
    pub outline: Option<BillboardTextOutline>,
    /// 离屏纹理尺寸（宽, 高），像素。
    pub texture_size: UVec2,
    /// Billboard 世界空间高度（米）；世界宽度按文字像素宽推导。
    pub world_height: f32,
    /// 头顶/锚点的 Y 轴偏移（米）。
    pub y_offset: f32,
    /// 整体世界尺寸缩放倍率（在距离缩放基础上再乘）。
    pub scale: f32,
    /// 是否跟随宿主实体（true=头顶 UI；false=固定在生成时的世界坐标）。
    pub follow: bool,
    /// 可选的跟随目标实体覆盖。
    ///
    /// - `None`（默认）：跟随「被插入 `BillboardText` 的那个实体」。
    /// - `Some(e)`：改为跟随实体 `e`。便于用一个独立的「锚点/追踪实体」承载请求，
    ///   而让 Billboard 跟随另一个游戏对象（如 NPC）。
    pub follow_target: Option<Entity>,
    /// 是否启用「近大远小」的距离缩放。
    pub distance_scaling: bool,
    /// 参考距离：小于该值不再放大。
    pub ref_distance: f32,
    /// 最大可见距离：超过则隐藏。
    pub max_distance: f32,
    /// 存活时长（秒）；`Some(t)` 表示 t 秒后自动销毁，`None` 表示常驻。
    pub ttl: Option<f32>,
    /// 水平内边距（像素），避免阴影被裁切。
    pub padding_px: f32,
    /// 文字背后的装饰圆盘（按 `z` 由低到高叠放，文字在最上）。
    pub decorations: Vec<BillboardDecoration>,
    /// 是否按文字真实宽度裁切 UV/网格。
    ///
    /// - `true`（默认）：常规头顶文字，等待首帧布局后按内容宽度裁切（窄而紧凑）。
    /// - `false`：固定整张纹理映射（用于正方形徽标等内容已填满纹理的场景），
    ///   立即可见、不做 UV 裁切，世界宽度按纹理宽高比由 `world_height` 推导。
    pub crop_to_text: bool,
}

impl Default for BillboardText {
    fn default() -> Self {
        Self {
            text: String::new(),
            font: Handle::default(),
            font_size: DEFAULT_FONT_SIZE,
            color: Color::WHITE,
            shadow: Some(Text2dShadow {
                offset: Vec2::new(2.0, -2.0),
                color: Color::srgba(0.0, 0.0, 0.0, 0.9),
            }),
            outline: None,
            texture_size: UVec2::new(DEFAULT_TEXTURE_WIDTH, DEFAULT_TEXTURE_HEIGHT),
            world_height: DEFAULT_WORLD_HEIGHT,
            y_offset: 0.0,
            scale: 1.0,
            follow: true,
            follow_target: None,
            distance_scaling: true,
            ref_distance: DEFAULT_REF_DISTANCE,
            max_distance: DEFAULT_MAX_DISTANCE,
            ttl: None,
            padding_px: DEFAULT_HORIZONTAL_PADDING_PX,
            decorations: Vec::new(),
            crop_to_text: true,
        }
    }
}

/// 指向已生成 Billboard 实体的回写句柄。
///
/// `spawn_billboard_texts` 消费 `BillboardText` 后，会把该组件插回到「承载请求的实体」上，
/// 让调用方能够拿到生成出的 Billboard 实体（例如用于后续整体销毁/重建）。
#[derive(Component, Debug, Clone, Copy)]
pub struct BillboardTextHandle(pub Entity);

/// 时间重建引导子实体标记。
///
/// 最终 Blend 文字不会写入预通道；该子实体只在具备 Deferred、Motion 与时间抖动的
/// 主 Billboard 相机中显示，以同一字形补齐 DLSS RR 所需的表面引导。
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct BillboardTemporalGuide;

/// Billboard 跟踪器组件（由插件内部生成在独立的 Billboard 实体上）。
///
/// 链接宿主、最终纹理和烘焙期2D源实体，并保存跟随/缩放/销毁所需的运行时状态。
#[derive(Component)]
pub struct BillboardTextLink {
    /// 宿主实体（被跟随对象）；`follow=false` 时仅用于销毁判定可忽略。
    pub owner: Entity,
    /// 当前烘焙任务的离屏2D文字实体；排队或烘焙完成后为None。
    pub text_entity: Option<Entity>,
    /// 离屏描边文字实体列表（随 Billboard 一起销毁）。
    pub outline_entities: Vec<Entity>,
    /// 离屏装饰圆盘实体列表（随 Billboard 一起销毁）。
    pub decoration_entities: Vec<Entity>,
    /// Billboard 四边形 mesh 句柄；销毁时同步移除动态资产，避免临时文字堆积 GPU 资源。
    pub mesh: Handle<Mesh>,
    /// Billboard 3D 材质句柄；销毁时同步移除动态资产，避免材质资产持续增长。
    pub material: Handle<BillboardColorMaterial>,
    /// 时间重建引导子实体；销毁或业务重建时与最终文字一起回收。
    pub temporal_guide_entity: Entity,
    /// 每个 Billboard 独占的时间重建引导材质。
    pub temporal_guide_material: Handle<BillboardTemporalGuideMaterial>,
    /// 离屏渲染目标图片句柄；销毁时同步移除动态纹理，避免 Image 资产持续增长。
    pub image: Handle<Image>,
    /// 装饰圆盘 mesh 句柄列表；随装饰实体一起释放动态资产。
    pub decoration_meshes: Vec<Handle<Mesh>>,
    /// 装饰圆盘材质句柄列表；随装饰实体一起释放动态资产。
    pub decoration_materials: Vec<Handle<ColorMaterial>>,
    /// 头顶 Y 偏移。
    pub y_offset: f32,
    /// 世界尺寸缩放倍率。
    pub scale: f32,
    /// 是否跟随宿主。
    pub follow: bool,
    /// 是否启用距离缩放。
    pub distance_scaling: bool,
    /// 参考距离。
    pub ref_distance: f32,
    /// 最大可见距离。
    pub max_distance: f32,
    /// 离屏纹理宽度（像素），用于 UV 裁切计算。
    pub texture_width: f32,
    /// 离屏纹理高度（像素），用于世界宽度换算。
    pub texture_height: f32,
    /// Billboard 世界高度（米）。
    pub world_height: f32,
    /// 水平内边距（像素）。
    pub padding_px: f32,
    /// 剩余存活时长（秒）；`None` 表示常驻。
    pub ttl: Option<f32>,
    /// 等待队列和烘焙期保留的原始规格；完成后释放字体、文字和装饰配置的额外引用。
    pub(crate) bake_spec: Option<BillboardText>,
    /// 是否已经完成纹理烘焙；只有就绪后才显示并开始计算TTL。
    pub(crate) bake_ready: bool,
    /// 是否仍在等待首次文本布局同步（同步前隐藏，避免错误宽度闪烁）。
    pub(crate) needs_layout_sync: bool,
    /// 离屏内容完成布局后继续保留的渲染帧数；归零后释放2D源并归还相机槽位。
    pub(crate) bake_frames_remaining: u8,
}

impl BillboardTextLink {
    /// 返回最终文字纹理是否已经完整烘焙，可供其他相机呈现系统安全接管可见性。
    pub fn is_ready(&self) -> bool {
        self.bake_ready // 排队和烘焙阶段必须继续隐藏，不能暴露透明或旧目标内容。
    }
}
