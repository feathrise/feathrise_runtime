//! 自动遮雨完整捕获需求与主世界缓存；纹理内部携带实际完成矩阵。
use bevy::prelude::*; // 图片身份、矩阵和资源。
use bevy::render::extract_resource::ExtractResource; // 固定快照提取，无几何副本。
use feathrise_scene_content::SceneId; // 同场景枚举中的不同场景必须分开。
use feathrise_scene_core::state::ScenePhase; // 菜单和加载阶段撤销旧图。
#[derive(Resource, Clone, ExtractResource)] // 主世界需求和GPU输出通过唯一句柄对应。
#[extract_app(bevy::render::RenderApp)]
pub(crate) struct RainOcclusionMap {
    // 消费者只需要图像，矩阵给本帧生产端。
    pub image: Handle<Image>, // 空闲时绑定Bevy默认1像素图，由消费端尺寸守卫禁用。
    pub world_to_rain: Mat4,  // 本次捕获的reverse-Z clip_from_world。
    pub active: bool,         // 允许复用已完成图，不等于本帧必须重画。
} // 完整需求结束。
impl Default for RainOcclusionMap {
    // 最小ECS宿主安全无自动遮挡。
    fn default() -> Self {
        // 默认图始终已有合法Hanabi绑定。
        Self {
            image: Handle::default(),
            world_to_rain: Mat4::IDENTITY,
            active: false,
        } // 未捕获不读取旧内容。
    } // 默认值结束。
} // 默认实现结束。
#[derive(Resource, Default)] // 静态相机与材质复用深度图。
pub(super) struct CaptureState {
    // 每次只持有一张固定大小图和少量身份。
    pub casters_pending: bool, // 大父根重挂/改名未完成时不发布部分几何深度。
    pub context: Option<(ScenePhase, Option<SceneId>)>, // 场景切换废弃旧目标。
    pub target: Option<Handle<Image>>, // 保持当前GPU目标存活。
    pub warm_frames: u8,       // 新资产异步管线启动阶段连续重画。
    pub last_capture: f64,     // 有界低频刷新兜底未发出实体事件的纹理变化。
} // 缓存声明结束。
