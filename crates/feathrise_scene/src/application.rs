//! 场景只消费应用显式声明的观察者和领域交接；没有安装适配器的场景不产生玩法对象。

use bevy::prelude::*;
use feathrise_scene_content::{ExpandedSceneInstance, SceneId, SceneInstancePath};
use std::{any::Any, sync::Arc};

/// 应用在场景装配时声明物理过滤；共享场景不识别玩家、阵营或旧游戏层号。
/// 运行期间保持不变，场景的异步派生结果仍只按原场景/修订身份采用。
#[derive(Resource, Clone, Copy)]
pub struct ScenePhysicsPolicy {
    pub boundary_layers: avian3d::prelude::CollisionLayers,
    pub surface_query_mask: avian3d::prelude::LayerMask,
}
impl Default for ScenePhysicsPolicy {
    fn default() -> Self {
        Self {
            boundary_layers: avian3d::prelude::CollisionLayers::default(),
            surface_query_mask: avian3d::prelude::LayerMask::ALL,
        }
    }
}

/// 场景创建视图与天体光时采用应用给定的层；默认只显示场景、天气和传送提示。
#[derive(Resource, Clone)]
pub struct SceneViewLayers {
    pub camera: bevy::camera::visibility::RenderLayers,
    pub light: bevy::camera::visibility::RenderLayers,
}
impl Default for SceneViewLayers {
    fn default() -> Self {
        use feathrise_scene_core::constants::render_layers::{AIR_WALL_GAMEPLAY_LAYER,
            CELESTIAL_GAMEPLAY_LAYER, PORTAL_EFFECT_LAYER, WEATHER_EFFECT_LAYER};
        use bevy::camera::visibility::RenderLayers;
        Self {
            camera: RenderLayers::from_layers(&[0, PORTAL_EFFECT_LAYER, WEATHER_EFFECT_LAYER,
                CELESTIAL_GAMEPLAY_LAYER, AIR_WALL_GAMEPLAY_LAYER]),
            light: RenderLayers::layer(0),
        }
    }
}

/// 申请附近物理和流送的唯一观察锚点；相机本身不自动取得此资格，也不隐式产生交互。
#[derive(Component, Default)]
pub struct SceneRuntimeObserver;

/// 显式允许当前位置产生压草轨迹；流送观察者和编辑相机不会自动踩草。
#[derive(Component, Default)]
pub struct VegetationInteractionSource;

/// 水面接触需要真实脚底偏移与动作序号，由应用控制器在其动作发布后同步。
#[derive(Component, Default, Clone, Copy, PartialEq)]
pub struct WaterInteractionSource {
    pub takeoff_sequence: u64,
    pub foot_offset: f32,
}

/// 动态物理参与者只声明所属完整场景实例，不携带角色、阵营或生命字段。
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct SceneCollisionParticipant {
    pub source_scene: Arc<str>,
    pub instance_path: Option<SceneInstancePath>,
    pub instance_owner: Option<Entity>,
    pub instance_generation: Option<u64>,
}

/// 所有可选应用来源都在场景几何发布后执行，最后才能释放公共启动门。
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SceneApplicationSet {
    PublishSource,
    PublishReadiness,
    CompleteStartup,
}

/// 场景宿主同步调用的小型交接表。领域自行持有资产、任务和结果，不把 DTO 塞入共享库。
/// 实例 Entity 带 Bevy generation，node 带完整 root/path/source/transform；实现必须再核对自己的输入代次。
#[derive(Resource, Clone, Copy)]
pub struct SceneApplicationHooks {
    pub startup: fn(&World, &feathrise_scene_core::state::SceneStartupGate) -> SceneApplicationStartupStatus,
    pub configure_instance: fn(&mut Schedule),
    pub instance_ready: fn(&World, Entity) -> Result<bool, String>,
    pub publish_instance: fn(&mut World, Entity, &ExpandedSceneInstance),
    pub retire_instance: fn(&mut World, Entity, &SceneInstancePath) -> Option<Box<dyn Any + Send + Sync>>,
    pub reset_instances: fn(&mut World, Option<SceneId>),
    pub expect_instances: fn(&mut World, usize),
}

/// 注册过的应用领域必须报告实际已完成项；缺少自身输入时应返回未完成，不能省略检查。
pub struct SceneApplicationStartupStatus {
    pub completed: usize,
    pub total: usize,
    pub error: Option<String>,
}

#[cfg(test)]
#[path = "application_tests.rs"]
mod tests;
