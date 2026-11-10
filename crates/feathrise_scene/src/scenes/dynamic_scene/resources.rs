//! 动态场景旧白模与新稀疏高度源共用的加载事务与公开状态。

use bevy::prelude::*; // Entity与Resource。
use feathrise_scene_content::SceneId; // 动态场景以稳定ID区分会话。

/// 动态场景运行时阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicSceneRuntimePhase {
    /// 场景目录、清单或GLB仍在异步加载。
    #[default]
    Waiting,
    /// 白模根已生成，正在等待GLB层级与Avian碰撞完成。
    BuildingCollision,
    /// 白模、法线和碰撞均已就绪。
    Ready,
    /// 当前场景数据失败，保留全屏编辑器错误提示而不放行玩法。
    Failed,
}

/// F12编辑器和F3可读取的动态场景加载证据。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicSceneRuntimeStatus {
    /// 当前阶段。
    pub phase: DynamicSceneRuntimePhase,
    /// 当前选择的SceneId。
    pub scene_id: Option<SceneId>,
    /// 最近一次成功碰撞就绪的修订。
    pub revision: u64,
    /// 当前具体等待项或失败原因。
    pub detail: String,
}

/// 动态场景唯一写侧状态。
#[derive(Resource, Debug)]
pub(super) struct DynamicSceneRuntimeState {
    /// 清单或场景选择改变后请求重新核对。
    pub(super) dirty: bool,
    /// 当前已生成白模根所属场景。
    pub(super) active_scene_id: Option<SceneId>,
    /// 当前白模GLB路径。
    pub(super) glb_path: Option<String>,
    /// 只有旧规则白模需要高度派生；原始GLB保持所有primitive与材质。
    pub(super) derive_glb_height: bool,
    /// 白模派生的真实显示/碰撞实体；原始输入primitive始终隐藏。
    pub(super) terrain_display: Option<Entity>,
    /// 新场景稀疏高度清单资产路径；与旧 GLB 路径互斥。
    pub(super) sparse_height_path: Option<String>,
    /// 当前白模根实体。
    pub(super) root: Option<Entity>,
    /// 当前动态场景唯一玩法相机；清单环境只向该实体发布天空盒请求。
    pub(super) camera: Option<Entity>,
    /// 当前动态场景太阳方向光；全局与场景覆盖和月亮灯一起写入。
    pub(super) celestial_sun_light: Option<Entity>,
    /// 当前动态场景月亮方向光；独立轨迹避免晨昏时把太阳灯经天顶旋到东方。
    pub(super) celestial_moon_light: Option<Entity>,
    /// 上次观察到的场景目录修订。
    pub(super) catalog_revision: u64,
    /// 当前白模层级碰撞是否完成。
    pub(super) collision_ready: bool,
    /// 稀疏场景固定 Clipmap 显示网格是否已经完整安装。
    pub(super) sparse_clipmap_ready: bool,
    /// 稀疏场景首组 GPU 高度页或确定性全平回退是否已经完整采用。
    pub(super) sparse_height_pages_ready: bool,
    /// 当前目录是否声明数据驱动地表。
    pub(super) surface_required: bool,
    /// 地表未声明或已通过离屏预热并正式接管。
    pub(super) surface_ready: bool,
    /// 当前目录或F12覆盖是否要求区域边界。
    pub(super) boundary_required: bool,
    /// 区域边界未声明、已完整加载或已保留上一完整热更结果。
    pub(super) boundary_ready: bool,
    /// 启动门是否已经为当前场景代次放行。
    pub(super) ready_released: bool,
    /// 场景清单环境专用修订；迟到天空Image事件不能跨清单或切换场景采用。
    pub(super) environment_revision: u64,
}

impl Default for DynamicSceneRuntimeState {
    fn default() -> Self {
        Self {
            dirty: true,
            active_scene_id: None,
            glb_path: None,
            derive_glb_height: false,
            terrain_display: None,
            sparse_height_path: None,
            root: None,
            camera: None,
            celestial_sun_light: None,  // 进入动态场景后登记明确太阳实体。
            celestial_moon_light: None, // 进入动态场景后登记明确月亮实体。
            catalog_revision: 0,
            collision_ready: false,
            sparse_clipmap_ready: false,
            sparse_height_pages_ready: false,
            surface_required: false,
            surface_ready: false,
            boundary_required: false,
            boundary_ready: false,
            ready_released: false,
            environment_revision: 0,
        }
    }
}
