//! 动态地形高度缓存、公开状态和内部有界任务类型。

use std::sync::Arc; // 完成值在水域、植被与F12之间只克隆Arc身份。

use avian3d::prelude::Collider; // 后台完成值携带已构建Trimesh，主线程不再遍历整图。
use bevy::asset::AssetId; // Mesh资产身份参与迟到结果拒收。
use bevy::math::Affine3A; // 捕获冻结完整节点到世界变换。
use bevy::prelude::*; // Resource、Entity、Handle、Image与Mesh。
use bevy::tasks::Task; // 同时最多一个不可协作取消的整图派生任务。
use feathrise_map_content::MapId; // 动态地图稳定身份阻止跨图采用。
use feathrise_map_terrain::{
    // 共享纯算法输入、完成值与F12编辑索引。
    TerrainDerivedData,
    TerrainEditingIndex,
    TerrainSampleSpec,
}; // 完成地形算法类型导入。

/// GLB 地形到内存高度派生的公开阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicTerrainHeightPhase {
    /// 正在等待清单、唯一primitive或Mesh资产。
    #[default]
    Waiting,
    /// 主线程正在按固定预算冻结源POSITION与索引。
    Capturing,
    /// 完整输入已进入唯一后台任务。
    Building,
    /// 当前目标代次的CPU高度、GPU Image和Trimesh已一起采用。
    Ready,
    /// 当前输入无效；热更时仍可保留上一完整成功结果。
    Failed,
} // 结束派生阶段枚举。

/// F12与诊断面板可读取的动态地形派生状态。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicTerrainHeightStatus {
    /// 当前阶段。
    pub phase: DynamicTerrainHeightPhase, // 不以任务启动冒充可用。
    /// 当前目标地图。
    pub map_id: Option<MapId>, // 与缓存完成值共同提供稳定身份。
    /// 当前期望输入代次。
    pub desired_generation: u64, // GLB或Mesh热更都会推进。
    /// 最近一次成功原子采用代次。
    pub applied_generation: u64, // 仅Ready且等于期望值时证明当前输入落稳。
    /// 当前等待、成功预算或失败原因。
    pub detail: String, // UI不需要解析日志文本。
} // 结束公开派生状态。

/// 一次完整成功 GLB 派生，可被运行时和 F12 只读共享。
#[derive(Clone)]
pub struct DynamicTerrainHeightData {
    /// 完成值所属动态地图。
    pub map_id: MapId, // 防止两个动态地图坐标重叠时串用。
    /// GLB/Mesh输入代次。
    pub generation: u64, // 热更与F12冲突判断的轻量版本证据。
    /// 产生完成值的正式primitive实体。
    pub source_entity: Entity, // F12接管与热更诊断使用。
    /// 产生完成值的Mesh资产身份。
    pub source_mesh: AssetId<Mesh>, // 同实体Mesh换代时旧结果失效。
    /// 清单采样规格。
    pub spec: TerrainSampleSpec, // 水域与植被无需再次读取或克隆清单。
    /// 世界空间顶点，保持Blender导出顺序。
    pub positions: Arc<[[f32; 3]]>, // F12打开时只O(1)克隆，首次写再复制。
    /// 重算平滑法线，保持与顶点等长。
    pub normals: Arc<[[f32; 3]]>, // 预览和保存使用同一算法结果。
    /// U32 TriangleList索引，保持三角顺序和连接关系。
    pub indices: Arc<[u32]>, // Collider、保存和Solari代理共享同序拓扑。
    /// 规则CPU最高表面高度网格；完整洞穴几何由positions/indices与Trimesh表达。
    pub heights: Arc<[u16]>, // 水域与Mesh植被不再解码KTX2。
    /// 从最高表面高度打包的内存RGBA16高度单元Image。
    pub height_image: Handle<Image>, // GPU Blade直接绑定该强句柄。
    /// F12局部笔刷空间桶和拓扑邻接。
    pub editing_index: Arc<TerrainEditingIndex>, // 平滑不再使用规则3×3邻域。
} // 结束完整地形完成值。

/// 最多保存当前运行地图和上一份完整结果；热更失败继续提供旧结果。
#[derive(Resource, Default)]
pub struct DynamicTerrainHeightCache {
    pub(super) ready: Option<DynamicTerrainHeightData>, // 当前唯一玩法地图的完整采用值。
    pub(super) pending: Option<DynamicTerrainHeightData>, // 新GLB派生完成但仍等待水域和植被的候选值。
} // 结束动态地形缓存。

impl DynamicTerrainHeightCache {
    /// 返回当前缓存的完整地形；调用方必须继续核对MapId。
    pub fn ready(&self) -> Option<&DynamicTerrainHeightData> {
        self.ready.as_ref() // 只借用完成值，不复制大型数组。
    } // 结束完成值读取。

    /// 返回指定地图的完整地形，其他地图缓存视为缺席。
    pub fn ready_for(&self, map_id: &MapId) -> Option<&DynamicTerrainHeightData> {
        self.ready.as_ref().filter(|ready| &ready.map_id == map_id) // 一次常数身份比较。
    } // 结束按地图读取。

    /// 返回下游本轮应构建的地形；候选存在时优先使用候选，否则使用已采用值。
    pub(crate) fn target_for(&self, map_id: &MapId) -> Option<&DynamicTerrainHeightData> {
        self.pending
            .as_ref()
            .filter(|pending| &pending.map_id == map_id)
            .or_else(|| self.ready_for(map_id)) // 热更计算尚未完成时下游继续绑定上一完整代次。
    } // 结束目标值读取。

    /// 返回尚未全局提交的GLB派生候选。
    pub(crate) fn pending_for(&self, map_id: &MapId) -> Option<&DynamicTerrainHeightData> {
        self.pending
            .as_ref()
            .filter(|pending| &pending.map_id == map_id) // 全局提交门只接受当前地图候选。
    } // 结束候选读取。
} // 结束缓存公开接口。

/// 地形、水域和植被同帧切换使用的轻量提交令牌。
#[derive(Resource, Default)]
pub(crate) struct DynamicTerrainCommitState {
    map_id: Option<MapId>, // 令牌只属于一个当前玩法地图。
    generation: u64,       // 三个子系统必须精确匹配同一GLB派生代次。
} // 结束全局提交状态。

impl DynamicTerrainCommitState {
    pub(crate) fn arm(&mut self, map_id: MapId, generation: u64) {
        self.map_id = Some(map_id); // 候选全部就绪后才建立一帧提交令牌。
        self.generation = generation; // 零不会成为合法派生代次。
    }

    pub(crate) fn matches(&self, map_id: &MapId, generation: u64) -> bool {
        self.map_id.as_ref() == Some(map_id) && self.generation == generation
    }

    pub(crate) fn is_armed(&self) -> bool {
        self.map_id.is_some()
    }

    pub(crate) fn clear(&mut self) {
        self.map_id = None; // 同一令牌最多消费一个Update提交边界。
        self.generation = 0;
    }
} // 结束提交令牌实现。

/// 捕获任务的当前属性阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DynamicTerrainCapturePhase {
    Positions, // 逐片复制并应用节点世界变换。
    Indices,   // U16/U32或无索引TriangleList统一冻结为U32。
    Complete,  // 只有该阶段允许启动后台纯计算。
} // 结束捕获阶段枚举。

/// 当前目标输入的完整身份。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DynamicTerrainIdentity {
    pub(super) map_id: MapId,              // 地图稳定身份。
    pub(super) generation: u64,            // 期望输入代次。
    pub(super) source_entity: Entity,      // 正式primitive实体。
    pub(super) source_mesh: AssetId<Mesh>, // Mesh资产身份。
} // 结束派生身份。

/// 主线程跨帧固定预算冻结中的源网格。
pub(super) struct DynamicTerrainCaptureJob {
    pub(super) identity: DynamicTerrainIdentity, // 完成后启动任务仍携带同一身份。
    pub(super) transform: Affine3A,              // 节点到世界变换在捕获开始时冻结。
    pub(super) transform_bits: [u32; 12],        // 稳定比较捕获期间层级是否换代。
    pub(super) phase: DynamicTerrainCapturePhase, // 当前属性游标。
    pub(super) offset: usize,                    // 当前属性内连续元素偏移。
    pub(super) positions: Vec<Vec<[f32; 3]>>,    // 每片不超过单帧预算，避免扩容时搬移整图前缀。
    pub(super) indices: Vec<Vec<u32>>,           // 索引同样按固定小片保存，后台再合并。
    pub(super) captured_vertices: usize,         // 真实已冻结位置总数供进度显示。
    pub(super) captured_indices: usize,          // 真实已冻结逻辑索引总数。
    pub(super) expected_vertices: usize,         // 采用前再次核对完整数量。
    pub(super) expected_indices: usize,          // 无索引时等于顶点数。
    pub(super) indexed: bool,                    // 决定索引阶段读取源表或生成顺序值。
} // 结束分帧捕获任务。

/// 后台任务完整成功或失败的拥有结果。
pub(super) struct DynamicTerrainTaskResult {
    pub(super) identity: DynamicTerrainIdentity, // 主线程采用前再次核对。
    pub(super) result: Result<DynamicTerrainTaskData, String>, // 任一派生失败整批拒绝。
} // 结束后台任务结果。

/// 后台已经一起完成的纯数据、编辑索引与Avian碰撞。
pub(super) struct DynamicTerrainTaskData {
    pub(super) derived: TerrainDerivedData, // 位置、法线、索引、高度和RGBA16字节。
    pub(super) editing_index: TerrainEditingIndex, // F12笔刷索引与同一拓扑修订绑定。
    pub(super) render_mesh: Mesh,           // 后台准备完整显示Mesh，主线程采用时只登记资产句柄。
    pub(super) collider: Collider,          // 主线程常数操作安装已构建Trimesh。
} // 结束后台完成数据。

/// 已登记主世界资产、但尚未与水域和植被共同提交的地形候选。
pub(super) struct DynamicTerrainInstallCandidate {
    pub(super) identity: DynamicTerrainIdentity, // 采用前再次核对当前GLB目标。
    pub(super) root: Entity,                     // 只允许安装到捕获时仍有效的动态地图根。
    pub(super) render_mesh: Handle<Mesh>,        // 后台完成网格已登记，提交只替换句柄。
    pub(super) collider: Collider,               // 同拓扑Trimesh在同一提交帧安装。
} // 结束地形安装候选。

/// 当前动态地图唯一派生写侧，捕获和CPU任务都实施single-flight背压。
#[derive(Resource, Default)]
pub(crate) struct DynamicTerrainDerivationState {
    pub(super) attempted: Option<DynamicTerrainIdentity>, // 同一完整输入最多派生一次；候选、已采用及失败都不重启。
    pub(super) desired: Option<DynamicTerrainIdentity>,   // 当前唯一目标源身份。
    pub(super) desired_transform: Option<Affine3A>,       // 当前源完整世界变换。
    pub(super) desired_transform_bits: Option<[u32; 12]>, // O(1)变换换代比较。
    pub(super) spec: Option<TerrainSampleSpec>,           // 当前清单采样规格。
    pub(super) generation: u64, // 新地图、primitive、Mesh或变换变化时推进。
    pub(super) capture: Option<DynamicTerrainCaptureJob>, // 主线程固定预算捕获游标。
    pub(super) retire_task: Option<Task<()>>, // 旧输入/候选按异步任务链顺序释放；完成前禁止新捕获。
    pub(super) task: Option<Task<DynamicTerrainTaskResult>>, // 唯一不可协作取消后台任务。
    pub(super) dirty_after_task: bool, // 在途期间的新输入合并为一次后继捕获。
    pub(super) pending_install: Option<DynamicTerrainInstallCandidate>, // 完整派生等待全局同代提交。
} // 结束动态地形派生状态。

#[cfg(test)]
mod tests {
    use super::*; // 只验证双代缓存和一次性提交身份，不启动渲染世界。
    use feathrise_map_terrain::{TerrainMeshInput, build_terrain_editing_index};

    fn terrain_data(map_id: &MapId, generation: u64) -> DynamicTerrainHeightData {
        let mesh = TerrainMeshInput {
            positions: vec![[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [-1.0, 0.0, 1.0]],
            indices: vec![0, 2, 1],
        };
        DynamicTerrainHeightData {
            map_id: map_id.clone(),
            generation,
            source_entity: Entity::PLACEHOLDER,
            source_mesh: Handle::<Mesh>::default().id(),
            spec: TerrainSampleSpec {
                resolution: [2, 2],
                world_min: [-1.0, -1.0],
                world_max: [1.0, 1.0],
                base_height: -1.0,
                height_scale: 2.0,
            },
            positions: mesh.positions.clone().into(),
            normals: vec![[0.0, 1.0, 0.0]; 3].into(),
            indices: mesh.indices.clone().into(),
            heights: vec![32_768; 4].into(),
            height_image: Handle::default(),
            editing_index: Arc::new(
                build_terrain_editing_index(&mesh).expect("测试三角形必须可建立编辑索引"),
            ),
        }
    }

    #[test]
    fn pending_generation_is_used_for_builds_without_replacing_ready_queries() {
        let map_id = MapId::parse("MAP-terrain-double-cache").expect("测试MapId必须合法");
        let mut cache = DynamicTerrainHeightCache::default();
        cache.ready = Some(terrain_data(&map_id, 4));
        cache.pending = Some(terrain_data(&map_id, 5));

        assert_eq!(
            cache.ready_for(&map_id).map(|value| value.generation),
            Some(4)
        );
        assert_eq!(
            cache.target_for(&map_id).map(|value| value.generation),
            Some(5)
        );
        assert_eq!(
            cache.pending_for(&map_id).map(|value| value.generation),
            Some(5)
        );
    }

    #[test]
    fn commit_token_requires_exact_map_and_generation_and_can_be_cleared() {
        let map_id = MapId::parse("MAP-terrain-commit").expect("测试MapId必须合法");
        let other = MapId::parse("MAP-terrain-other").expect("测试MapId必须合法");
        let mut commit = DynamicTerrainCommitState::default();
        commit.arm(map_id.clone(), 9);
        assert!(commit.matches(&map_id, 9));
        assert!(!commit.matches(&map_id, 8));
        assert!(!commit.matches(&other, 9));
        commit.clear();
        assert!(!commit.is_armed());
    }
}

// 在途任务真正完成后才释放实例并发席位。
impl DynamicTerrainDerivationState {
    pub(in crate::maps::dynamic_map) fn background_busy(&self) -> bool { self.task.is_some() || self.retire_task.is_some() }
    pub(in crate::maps::dynamic_map) fn background_finished(&self) -> bool { self.task.as_ref().is_none_or(|task| task.is_finished()) && self.retire_task.as_ref().is_none_or(|task| task.is_finished()) }
}
