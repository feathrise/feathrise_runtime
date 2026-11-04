//! 场景 crate 的全局 ECS 资源。

use std::collections::HashMap; // 场景细节节点增删使用 O(1) 索引，切图不得退化为平方复杂度。

use bevy::prelude::*; // 引入 Resource 派生宏。
use feathrise_settings::SceneDetailQuality; // 记录上次实际应用的固定景观质量档。

/// F12 当前地形真实三角拓扑的进程内显示状态；切图恢复由编辑器会话负责。
#[cfg(any(feature = "test_panel", feature = "author_view"))] // 正式构建不注册线框预览资源。
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)] // 默认关闭使每次重启都从无网格开始。
pub struct TerrainGridPreview {
    pub visible: bool, // 关闭F12后继续用于玩法视角，菜单和加载页由会话强制清零。
} // 该资源不参与settings、地图清单或存档序列化。

/// 场景细节稳定帧快速路径与有界切档队列。
#[derive(Resource, Default)] // 空注册表在无固定景观地图安全完成，新增观察器随后增量登记节点。
pub(crate) struct SceneDetailQualityState {
    pub(crate) last_applied: Option<SceneDetailQuality>, // 相同档位稳定帧直接 O(1) 返回。
    pub(crate) pending_quality: Option<SceneDetailQuality>, // Some 表示仍有登记节点等待本轮切档。
    pub(crate) cursor: usize,                            // 下一批待处理节点在稳定向量中的起始下标。
    pub(crate) entities: Vec<Entity>, // 只保存已接管 LOD 实体，不在切档时重新扫描整个 World。
    pub(crate) indices: HashMap<Entity, usize>, // 实体销毁时 O(1) swap-remove，避免切图平方退化。
} // 完成场景细节运行时状态定义。
