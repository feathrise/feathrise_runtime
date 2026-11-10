//! 场景、传送和环境渲染的固定通道；层号属于现有渲染协议，不承载游戏角色分类。
//!
//! 保留既有编号，使主视图、作者视图和离屏捕获在拆分后仍互相隔离。
//! 名称中的 GAMEPLAY 表示原主场景通道，不要求安装玩家、战斗或游戏菜单。

/// 传送点世界特效层：主场景与作者视口可见，辅助离屏视图不默认接收。
pub const PORTAL_EFFECT_LAYER: usize = 10;

/// 天气云幕与降水专属层：由接收天气的主视图显式加入。
pub const WEATHER_EFFECT_LAYER: usize = 11;

/// 可见太阳与月亮的主场景远景层，与作者通道隔离。
pub const CELESTIAL_GAMEPLAY_LAYER: usize = 12;

/// 可见太阳与月亮的作者远景层，由共享天体系统和作者视口共同使用。
pub const CELESTIAL_AUTHORING_LAYER: usize = 13;

/// 运行时区域空气墙调试层；作者辅助线使用自己的通道。
pub const AIR_WALL_GAMEPLAY_LAYER: usize = 15;

/// 区域空气墙的保留型地表轮廓层；完整版本就绪后再让主视图接收。
pub const AIR_WALL_OUTLINE_GAMEPLAY_LAYER: usize = 16;

/// 雨丝局部水面高度图专用层，普通场景及作者相机均不默认接收。
pub const RAIN_WATER_MASK_LAYER: usize = 17;

/// 自动屋顶遮雨的纯深度视图层，仅实际世界标准模型参加。
pub const RAIN_OCCLUSION_LAYER: usize = 19;
