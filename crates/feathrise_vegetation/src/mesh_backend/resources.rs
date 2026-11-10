//! 通用Mesh原型登记、私有活动实体表和公开只读采用状态。
//! 【功能设计】注册表只保存内容无关原型；私有运行表证明自动根所有权，公开状态不反向驱动迁移。

use std::collections::BTreeMap; // 原型和活动实例按稳定数字身份保持确定性顺序。

use bevy::prelude::{Entity, Resource, Transform}; // 运行表只保存根实体与最终变换。

use crate::{VegetationLayerId, VegetationPrototypeId}; // 状态不声明树木、灌木或岩石类别。

use super::types::{VegetationMeshPrototype, VegetationMeshRenderTargetStats}; // 注册表保存完整原型，渲染监控保存按技术身份隔离的只读快照。

/// 通用Mesh公告板朝向适配所处阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VegetationMeshBillboardPhase {
    #[default]
    NoBillboards, // 当前世界没有通用Mesh公告板节点。
    FixedOnly,              // 只有固定交叉impostor，不需要相机或逐帧旋转。
    WaitingForCamera,       // 兼容保留的旧CPU阶段；GPU逐视图路径不再发布该值。
    AmbiguousCameras,       // 兼容保留的旧CPU阶段；多视图现在分别在各自顶点Shader中朝向。
    InvalidCameraTransform, // 兼容保留的旧CPU阶段；GPU直接读取有效渲染视图Uniform。
    Active,                 // 全部动态公告板都已绑定GPU逐视图扩展材质。
    ActiveWithFallback,     // 至少一个动态节点缺少GPU扩展材质并保持作者姿态。
}

/// 通用Mesh公告板在节点拓扑变化时发布的只读诊断快照。
#[derive(Clone, Copy, Debug, Resource, PartialEq, Eq)]
pub struct VegetationMeshBillboardStatus {
    pub phase: VegetationMeshBillboardPhase, // F3区分空、固定、GPU活动或材质回退。
    pub camera: Option<Entity>, // 兼容字段；GPU逐视图模式始终为None，不绑定单一主相机。
    pub active_marked_cameras: usize, // 兼容字段；旧相机标记不再扫描，因此始终为零。
    pub total_nodes: usize,     // 固定与动态公告板节点总数。
    pub fixed_nodes: usize,     // 保持作者姿态的交叉impostor数量。
    pub camera_facing_nodes: usize, // 需要GPU逐视图朝向的动态节点数量。
    pub oriented_nodes: usize,  // 已绑定GPU公告板材质的动态节点数量。
    pub fallback_nodes: usize,  // 缺少GPU公告板材质并保持作者姿态的动态节点数量。
}

impl Default for VegetationMeshBillboardStatus {
    fn default() -> Self {
        Self {
            phase: VegetationMeshBillboardPhase::NoBillboards, // 空世界不会伪报等待相机。
            camera: None,                                      // 默认没有选中相机。
            active_marked_cameras: 0,                          // 默认没有相机争用。
            total_nodes: 0,                                    // 默认没有公告板节点。
            fixed_nodes: 0,                                    // 默认没有固定交叉面。
            camera_facing_nodes: 0,                            // 默认没有动态朝向节点。
            oriented_nodes: 0,                                 // 默认没有GPU材质就绪节点。
            fallback_nodes: 0,                                 // 默认没有回退节点。
        } // 返回完整零状态供插件初始化与拓扑变化后重建。
    } // 完成公告板状态默认值。
} // 完成公告板状态默认实现。

/// 公告板节点增删时推进的私有拓扑代次。
///
/// 初始从一开始，保证插件晚于既有节点安装时首帧仍会执行一次完整聚合；之后
/// 稳定帧只比较一个整数，不再遍历全部公告板节点。
#[derive(Resource)]
pub(super) struct VegetationMeshBillboardTopology {
    generation: u64, // 节点插入、替换或释放都会推进该值。
}

impl Default for VegetationMeshBillboardTopology {
    fn default() -> Self {
        Self { generation: 1 } // 零保留给状态系统尚未发布过任何拓扑的本地游标。
    } // 完成首帧强制聚合的默认代次。
} // 完成公告板拓扑默认实现。

impl VegetationMeshBillboardTopology {
    pub(super) fn generation(&self) -> u64 {
        self.generation // 状态系统只读当前代次，不暴露外部推进能力。
    } // 完成公告板拓扑代次查询。

    pub(super) fn advance(&mut self) {
        self.generation = self.generation.wrapping_add(1); // 长会话溢出仍保持定义良好的回绕。
        if self.generation == 0 {
            self.generation = 1; // 零继续保留给尚未发布状态。
        } // 完成零值保留。
    } // 完成公告板拓扑推进。
} // 完成公告板拓扑行为。

/// 上一完整渲染帧的通用Mesh PBR分箱快照。
#[derive(Clone, Debug, Default, Resource, PartialEq, Eq)]
pub struct VegetationMeshRenderStats {
    pub sampled: bool, // RenderApp至少完成一次官方PBR排队后才为真。
    pub targets: Vec<VegetationMeshRenderTargetStats>, // 按布局层和原型稳定升序保存精确目标统计。
} // 主世界F3只读取，独立监控插件未安装时该资源根本不存在。

impl VegetationMeshRenderStats {
    /// 精确读取一个布局层与原型组合的上一完整帧快照。
    pub fn target(
        &self,
        layer: VegetationLayerId,         // 项目诊断公开的候选布局层身份。
        prototype: VegetationPrototypeId, // 项目诊断公开的候选原型身份。
    ) -> Option<&VegetationMeshRenderTargetStats> {
        self.targets
            .binary_search_by_key(&(layer, prototype), |stats| {
                (stats.target.layer, stats.target.prototype) // 与RenderApp固定排序口径保持一致。
            })
            .ok()
            .map(|index| &self.targets[index]) // 只返回完全匹配目标，禁止按Mesh句柄或内容名称猜测。
    } // 完成精确目标快照查询。
} // 完成通用Mesh渲染快照行为。

/// 通用Mesh精确PBR分箱监控的主世界按需开关。
///
/// 【功能设计】默认关闭以保证普通游戏帧不提取数千个诊断身份，也不遍历PBR公开分箱；
/// 诊断界面只有在显示树木、灌木或岩石页时才临时开启，关闭后下一提取帧立即截断快照。
#[derive(Clone, Copy, Debug, Default, Resource, PartialEq, Eq)]
pub struct VegetationMeshRenderMonitorConfig {
    enabled: bool, // 私有字段阻止消费者绕过显式方法伪造未审计的常驻监控模式。
}

impl VegetationMeshRenderMonitorConfig {
    /// 返回当前是否允许提取身份并扫描PBR分箱。
    pub const fn enabled(&self) -> bool {
        self.enabled // RenderApp与F3使用同一布尔契约，不从统计快照反推启用状态。
    }

    /// 仅在目标状态变化时更新监控开关，并返回本次是否发生切换。
    pub fn set_enabled(&mut self, enabled: bool) -> bool {
        if self.enabled == enabled {
            return false; // 稳态F3帧不污染资源变更检测。
        }
        self.enabled = enabled; // 下一Extract阶段按新状态启用采样或清除旧快照。
        true // 调用方可在测试和后续遥测中确认真实边沿。
    }
}

/// 通用Mesh根身份或托管标记发生结构变化时推进的私有代次。
#[derive(Default, Resource)]
pub(super) struct VegetationMeshOwnerTopology {
    generation: u64, // 组件钩子只推进代次，稳定帧不扫描全部根。
}

impl VegetationMeshOwnerTopology {
    pub(super) fn generation(&self) -> u64 {
        self.generation // 生命周期只读当前身份拓扑代次。
    } // 完成拓扑代次查询。

    pub(super) fn advance(&mut self) {
        self.generation = self.generation.wrapping_add(1); // 根出现、替换或移除都触发下一次所有权核对。
    } // 完成拓扑代次推进。
}

/// 一份已登记原型及其热替换revision。
#[derive(Clone)]
struct RegisteredVegetationMeshPrototype {
    revision: u64,                      // 同ID再次登记时递增，活动根据此确定性重建。
    prototype: VegetationMeshPrototype, // Mesh与标准材质句柄由消费侧加载后登记。
} // 注册表内部包装不进入公共API。

/// 内容无关的通用Mesh原型注册表。
#[derive(Default, Resource)]
pub struct VegetationMeshPrototypeRegistry {
    generation: u64, // 任意登记、替换、移除或清空都会推进全局变化代次。
    prototypes: BTreeMap<VegetationPrototypeId, RegisteredVegetationMeshPrototype>, // 稳定ID唯一拥有当前原型。
} // 空注册表保证安装Mesh插件不会自动接管旧可见对象。

impl VegetationMeshPrototypeRegistry {
    /// 登记或原子替换一个通用Mesh原型，并返回旧版本。
    pub fn register(
        &mut self,
        prototype: VegetationMeshPrototype, // 消费侧已经完成资产加载和原型装配。
    ) -> Option<VegetationMeshPrototype> {
        let revision = self.advance_generation(); // 本次原型版本与注册表变化共享单调身份。
        self.prototypes
            .insert(
                prototype.id, // 相同稳定ID只保留最新完整原型。
                RegisteredVegetationMeshPrototype {
                    revision,  // 生命周期用该值识别同ID热替换。
                    prototype, // 完整值一次进入注册表，活动根不读取部分更新。
                },
            )
            .map(|registered| registered.prototype) // 调用方可显式释放或检查旧强句柄。
    } // 完成原型登记或替换。

    /// 移除一个原型；下一生命周期同步会回收其全部自动托管根。
    pub fn remove(
        &mut self,
        id: VegetationPrototypeId, // 精确移除一个内容无关原型身份。
    ) -> Option<VegetationMeshPrototype> {
        let removed = self
            .prototypes
            .remove(&id)
            .map(|registered| registered.prototype); // 先取得旧原型及其强句柄所有权。
        if removed.is_some() {
            self.advance_generation(); // 只有真实移除才触发生命周期回收。
        } // 不存在的ID保持注册表代次稳定。
        removed // 返回旧原型供场景适配层按需处理。
    } // 完成单原型移除。

    /// 清空全部原型；空表不重复推进变化代次。
    pub fn clear(&mut self) {
        if self.prototypes.is_empty() {
            return; // 重复离图清理不制造无意义生命周期同步。
        } // 完成空表快速路径。
        self.prototypes.clear(); // 先释放全部已登记原型强句柄。
        self.advance_generation(); // 再触发全部自动托管根确定性回收。
    } // 完成注册表清空。

    /// 读取当前原型而不暴露其内部revision包装。
    pub fn get(&self, id: VegetationPrototypeId) -> Option<&VegetationMeshPrototype> {
        self.prototypes
            .get(&id)
            .map(|registered| &registered.prototype) // 消费者只能读取完整当前版本。
    } // 完成原型只读查询。

    /// 判断一个稳定原型身份是否已经登记。
    pub fn contains(&self, id: VegetationPrototypeId) -> bool {
        self.prototypes.contains_key(&id) // 不要求消费者克隆共享资产句柄。
    } // 完成原型存在性查询。

    /// 读取单个原型的正式登记版本，供共享所有者精确释放自己的版本。
    pub fn revision(&self, id: VegetationPrototypeId) -> Option<u64> {
        self.prototypes.get(&id).map(|registered| registered.revision)
    }

    /// 返回当前登记原型数量。
    pub fn len(&self) -> usize {
        self.prototypes.len() // F3可显示真实登记数而不解析内容名称。
    } // 完成注册表长度查询。

    /// 判断注册表是否为空。
    pub fn is_empty(&self) -> bool {
        self.prototypes.is_empty() // 空表明确表示Mesh生命周期不会接管任何层。
    } // 完成空表查询。

    /// 返回任意原型变化都会推进的注册表代次。
    pub fn generation(&self) -> u64 {
        self.generation // 生命周期和诊断只读该值，不允许外部伪造变化。
    } // 完成注册表代次查询。

    pub(super) fn entry(
        &self,
        id: VegetationPrototypeId, // 层配置引用的稳定原型身份。
    ) -> Option<(u64, &VegetationMeshPrototype)> {
        self.prototypes
            .get(&id)
            .map(|registered| (registered.revision, &registered.prototype)) // 内部差分同时取得版本和完整原型。
    } // 完成生命周期原型查询。

    fn advance_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1); // 长会话溢出时保持定义良好的回绕。
        if self.generation == 0 {
            self.generation = 1; // 零保留给从未登记过任何原型的默认状态。
        } // 完成零值保留。
        self.generation // 本次变化代次同时作为新原型revision。
    } // 完成注册表代次推进。
}

/// 通用Mesh自动采用所处阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VegetationMeshAdoptionPhase {
    #[default]
    CoreUnavailable, // 只安装Mesh插件但没有布局核心时保持无副作用。
    Unbound,                 // 没有可管理层或对应原型尚未登记。
    WaitingForLayout,        // 配置generation尚未获得同代活动布局提交。
    FallbackRetained,        // 至少一个稳定身份已有旧根所有者，通用后端没有重复生成。
    CurrentLayoutAdopted,    // 当前同代布局中的全部可管理候选都由通用后端一致表达。
    RejectedKeepingFallback, // 候选源或稳定身份非法时保留上一托管/旧实现。
}

/// 阻止一层完整采用的内容无关原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VegetationMeshAdoptionBlocker {
    MissingPrototype,               // 层引用的原型尚未登记或已经离图移除。
    LayoutGenerationPending,        // 活动提交仍属于上一配置与源字段代次。
    UnsupportedCandidateSource,     // 当前小阶段只接管HN迁移所需的显式稳定落点。
    DuplicateInstanceId,            // 同层重复稳定ID会破坏差分与单一所有者。
    ExplicitPlacementIndexOverflow, // 显式清单超出公共候选索引可表达范围。
    ExistingOwner,                  // 同层同ID已经存在非托管旧根，通用后端主动让出所有权。
}

/// 一层通用Mesh采用的完整只读快照。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VegetationMeshLayerAdoptionStatus {
    pub layer: VegetationLayerId,           // 内容名称由项目侧常量解释。
    pub prototype: VegetationPrototypeId,   // 当前层声明的稳定原型身份。
    pub phase: VegetationMeshAdoptionPhase, // 本层独立迁移阶段。
    pub blocker: Option<VegetationMeshAdoptionBlocker>, // 未完整采用时的结构化原因。
    pub candidate_instances: usize,         // 显式清单中的全部候选数量。
    pub selected_instances: usize,          // 当前活动分配场实际选中的稳定候选数量。
    pub managed_instances: usize,           // 通用生命周期当前独占管理的根数量。
    pub fallback_instances: usize,          // 因旧根占用而继续保留的稳定身份数量。
} // F3无需扫描数千节点即可展示迁移分子分母。

/// 通用Mesh全部可管理层的只读采用状态。
#[derive(Clone, Debug, Resource, PartialEq, Eq)]
pub struct VegetationMeshAdoptionStatus {
    pub phase: VegetationMeshAdoptionPhase, // 按拒绝、回退、等待、采用顺序聚合最保守阶段。
    pub layout_revision: u64,               // 状态对应的活动布局revision。
    pub configuration_generation: u64,      // 状态对应的配置与源字段generation。
    pub registry_generation: u64,           // 状态对应的原型注册表generation。
    pub managed_instances: usize,           // 全部自动托管根总数。
    pub layers: Vec<VegetationMeshLayerAdoptionStatus>, // 层ID稳定升序保存独立状态。
} // 该资源只提供证据，不自动切换项目迁移状态。

impl Default for VegetationMeshAdoptionStatus {
    fn default() -> Self {
        Self {
            phase: VegetationMeshAdoptionPhase::CoreUnavailable, // 没有布局核心时不得伪报采用。
            layout_revision: 0,                                  // 默认尚无活动布局。
            configuration_generation: 0,                         // 默认尚无同代配置提交。
            registry_generation: 0,                              // 默认尚未登记原型。
            managed_instances: 0,                                // 默认没有自动托管根。
            layers: Vec::new(),                                  // 默认没有可管理层快照。
        }
    } // 完成只读状态默认值。
}

/// 一个由自动生命周期实际拥有的活动根记录。
#[derive(Clone, Debug)]
pub(super) struct ActiveVegetationMeshRoot {
    pub(super) entity: Entity, // 根实体及Children由Bevy linked-spawn语义一并回收。
    pub(super) prototype: VegetationPrototypeId, // 同层配置换原型时必须替换根。
    pub(super) prototype_revision: u64, // 同ID原型热替换时必须替换根。
    pub(super) transform: Transform, // 显式落点变换变化时必须替换根。
} // 私有记录不接受旧树木/景观根冒充自动所有者。

/// 通用Mesh生命周期私有运行表。
#[derive(Default, Resource)]
pub(super) struct VegetationMeshRuntime {
    pub(super) active: BTreeMap<(VegetationLayerId, u64), ActiveVegetationMeshRoot>, // 层与稳定ID构成唯一根身份。
    last_layout_revision: Option<u64>, // None保证安装后至少同步一次默认状态。
    last_configuration_generation: Option<u64>, // 配置或源改变时等待同代活动提交。
    last_registry_generation: Option<u64>, // 原型登记、替换或移除触发差分。
    last_owner_topology_generation: Option<u64>, // 旧根或托管根结构变化触发单一所有权复核。
} // 稳态四个身份不变时不扫描候选或现有根。

impl VegetationMeshRuntime {
    pub(super) fn needs_sync(
        &self,
        layout_revision: u64,           // 当前活动布局revision。
        configuration_generation: u64,  // 当前配置与源字段generation。
        registry_generation: u64,       // 当前原型注册表generation。
        owner_topology_generation: u64, // 当前根身份与托管标记结构generation。
    ) -> bool {
        self.last_layout_revision != Some(layout_revision)
            || self.last_configuration_generation != Some(configuration_generation)
            || self.last_registry_generation != Some(registry_generation)
            || self.last_owner_topology_generation != Some(owner_topology_generation) // 四类输入任一变化才进入差分。
    } // 完成生命周期变化检测。

    pub(super) fn mark_synced(
        &mut self,
        layout_revision: u64,           // 已完成差分的活动布局revision。
        configuration_generation: u64,  // 已完成差分的配置与源字段generation。
        registry_generation: u64,       // 已完成差分的注册表generation。
        owner_topology_generation: u64, // 已完成核对的根身份拓扑generation。
    ) {
        self.last_layout_revision = Some(layout_revision); // 锁定当前活动布局。
        self.last_configuration_generation = Some(configuration_generation); // 锁定当前配置代次。
        self.last_registry_generation = Some(registry_generation); // 锁定当前原型代次。
        self.last_owner_topology_generation = Some(owner_topology_generation); // 锁定当前所有权拓扑。
    } // 完成生命周期同步身份更新。
}
