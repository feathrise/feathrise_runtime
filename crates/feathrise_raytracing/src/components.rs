//! 动态场景光追私有 ECS 组件。

use bevy::prelude::*; // Component、Entity、Vec3、Quat与UVec2。

use crate::types::CelestialLightSnapshot; // 时间历史同时跟踪天体光突变。

/// 标记只进入 Solari TLAS、没有 `Mesh3d` 的派生子代理。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RaytracingProxy {
    pub(crate) generation: u64, // 代理只属于创建它的场景代次。
    pub(crate) source: Entity,  // 对应当前完整候选中的可见源primitive。
}

/// 保存 Solari 相机上一稳定帧状态，只在真实变化时更新。
#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct SolariHistoryState {
    pub(crate) translation: Vec3,          // 上一帧传播后的相机世界坐标。
    pub(crate) rotation: Quat,             // 上一帧传播后的相机世界旋转。
    pub(crate) target_size: Option<UVec2>, // 上一帧相机目标物理尺寸。
    pub(crate) projection_bits: [u32; 16], // 上一帧投影矩阵逐浮点位快照，覆盖FOV、裁剪面与投影类型变化。
    pub(crate) scene_generation: u64,        // 光追自有场景代次变化必须丢弃上一个场景的时间历史。
    pub(crate) celestial_lights: [Option<CelestialLightSnapshot>; 3], // 固定兼容、太阳和月亮各占稳定槽位，任一活动灯变化都可失效历史。
}
