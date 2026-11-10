//! 动态植被待加载资产、后台作业和公开状态。

use std::collections::{BTreeMap, BTreeSet, VecDeque}; // 分块身份使用有序集合保证同焦点下请求顺序可复现。
use std::sync::Arc; // 每个后台分块任务浅克隆密度与GLB派生高度快照，禁止按块复制整图。

use bevy::prelude::*;
use bevy::tasks::Task;
use feathrise_environment::vegetation::blade::{GpuBladeProfileBank, GpuBladeProfileBankId};
use feathrise_scene_content::{
    SceneId, SceneManifestAsset, SceneVegetationAsset, SceneVegetationInstanceTransform, SceneVegetationLayer,
}; // 待采用修订同时持有自己的任意数量四槽批次和每层会话身份。

use crate::gpu_grass_content::RuntimeBladeProfileSpec; // 数据预设统一转成当前修订拥有型四级规格。
use crate::scenes::dynamic_scene::clipmap::{ClipmapGroundContentRevision, ClipmapGroundQueryRevision}; // Mesh分块只保存高度内容身份与局部查询等待身份，不复制分页数据。

/// 动态场景植被运行阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicVegetationPhase {
    /// JSON、KTX2或场景资产仍在异步加载。
    #[default]
    Waiting,
    /// CPU在后台冻结当前Mesh密度与高度源。
    Building,
    /// 修订源与Blade窗口已经采用，Mesh按视野继续常驻完整空间块。
    Ready,
    /// 当前修订无效；首载保持空植被，热更保留上一完整修订。
    Failed,
}

/// F3和F12可直接读取的植被热加载与空间常驻证据。
#[derive(Resource, Clone, Debug, Default)]
pub struct DynamicVegetationStatus {
    pub input_draft_revision: Option<u64>, // 当前构建或失败对应的草稿发布身份。
    pub applied_draft_revision: Option<u64>, // 仅可见根完整提交后写入，旧Ready不能冒充新笔划。
    /// 已完整替换当前活动Blade窗口的像素发布代次；与只在布局变化时更新的draft修订分开。
    pub applied_density_revision: Option<u64>,
    pub phase: DynamicVegetationPhase,
    pub scene_id: Option<SceneId>,
    pub revision: u64,
    pub layer_count: usize,
    pub blade_layer_count: usize,
    /// 正式GPU叶片窗口数量；每个受检层固定为近、中、远、超远四窗。
    pub blade_window_count: usize,
    pub mesh_instance_count: usize,
    pub mesh_batch_count: usize,
    pub mesh_render_node_count: usize,
    /// 玩家近场当前实际存在的简化静态碰撞代理数。
    pub mesh_collision_proxy_count: usize,
    /// 当前 Mesh 常驻块采用的 Clipmap 作者高度身份；GLB 场景保持 `None`。
    pub clipmap_ground_revision: Option<[u64; 5]>,
    pub detail: String,
}

/// 一层已请求但尚未采用的强资产句柄。
#[derive(Clone)]
pub(super) struct DynamicVegetationLayerAssets {
    pub(super) source_density: Option<Arc<[u8]>>, // 草稿已在后台冻结的CPU源与图片同代。
    pub(super) in_memory_density: bool, // Assets直接登记的草稿图片没有AssetServer文件加载状态。
    pub(super) definition: SceneVegetationLayer,
    pub(super) density: Handle<Image>,
    /// 叶片层只接受稳定profile解析出的完整可变档规格。
    pub(super) blade_profile: Option<RuntimeBladeProfileSpec>,
    /// 与规格同长的网格句柄；作者asset_path不会替换其中任一档。
    pub(super) blade_meshes: Option<Vec<Handle<Mesh>>>,
    /// 当前修订派生的四槽批次身份；从不写入场景 JSON。
    pub(super) blade_profile_bank: Option<GpuBladeProfileBankId>,
}

/// 一个资源修订的不可拆分输入集合。
#[derive(Clone)]
pub(super) struct DynamicVegetationPending {
    pub(super) scene_id: SceneId,
    pub(super) manifest: SceneManifestAsset,
    pub(super) clipmap_height: Option<feathrise_environment::vegetation::blade::GpuBladeClipmapHeight>, // 分页高度独立于GLB缓存。
    pub(super) height_cells: Option<Handle<Image>>,
    pub(super) heights: Arc<[u16]>, // Mesh散布直接读取共享CPU高度，不再从RGBA16 Image反解。
    pub(super) terrain_generation: u64, // GPU Image与CPU Arc必须来自同一完整GLB派生代次。
    pub(super) layers: Vec<DynamicVegetationLayerAssets>,
    /// 当前修订实际引用的 Blade 预设按纹理页和每四项打包；数量不设置产品上限。
    pub(super) blade_profile_banks: Vec<GpuBladeProfileBank>,
    pub(super) generation: u32,
    pub(super) revision: u32,
    /// 只在当前资源修订扫描一次Mesh密度，避免等待大资源期间逐帧遍历整图。
    pub(super) mesh_profiles_scanned: bool,
    /// 非零Mesh层需要的正式四级profile；空白层不触发大GLB加载。
    pub(super) required_mesh_profiles: BTreeSet<String>,
    /// 请求集合的显式重试语义要求每个植被修订只写一次，稳定等待帧不能推进加载代次。
    pub(super) mesh_profiles_requested: bool,
}

/// 一层Mesh资产的不可变散布源；每个空间分块任务只浅克隆这些CPU快照。
#[derive(Clone, Debug)]
pub(super) struct DynamicMeshLayerBuild {
    pub(super) layer_index: usize,
    pub(super) layer: SceneVegetationLayer, // 作者格距、稳定种子和姿态范围始终来自完整正式层。
    pub(super) density: Arc<[u8]>,        // R8基础级在当前修订内只保存一份，不随常驻块数量增长。
}

/// 一层 Blade 的原始基础级 R8；建筑净空每次都从该不可变作者值重新派生。
#[derive(Clone, Debug)]
pub(super) struct DynamicBladeLayerBuild {
    pub(super) runtime_layer_id: u64, // 四个LOD窗口通过正式层身份共同切换密度句柄。
    pub(super) resolution: [u32; 2],  // 派生单Mip图片严格保持作者基础级尺寸。
    pub(super) density: Arc<[u8]>,    // 原始R8只冻结一次，净空任务只浅克隆。
}

/// Mesh 分块的高度后端；Clipmap 只标记能力，实际局部页快照在提交单块任务时取得。
#[derive(Clone, Debug)]
pub(super) enum DynamicVegetationGroundSource {
    Heightfield {
        heights: Arc<[u16]>,
        resolution: [u32; 2],
        base_height: f32,
        height_scale: f32,
    },
    Clipmap,
}

/// 当前完整修订的不可变Mesh散布源；它不预生成全图逐株数组。
#[derive(Clone, Debug)]
pub(super) struct DynamicVegetationBuild {
    pub(super) volume_mesh_layers: Arc<[SceneVegetationLayer]>, // 三维种植保留完整正式定义，不能因室外R8为空丢掉原型来源。
    pub(super) generation: u32,
    pub(super) revision: u32,
    pub(super) mesh_layers: Vec<DynamicMeshLayerBuild>,
    pub(super) blade_layers: Vec<DynamicBladeLayerBuild>, // 不改写原KTX2的Blade基础级快照。
    pub(super) ground: DynamicVegetationGroundSource,     // GLB整图或Clipmap局部页二选一。
    pub(super) world_min: [f32; 2], // 全图最小角也是所有分块共用的稳定候选原点。
    pub(super) world_max: [f32; 2], // 分块边界在此范围内钳制，最外块独占闭边界。
}

/// 一层已经在后台完成建筑净空栅格化的临时 Blade 密度。
pub(super) struct DynamicBladeClearanceLayerBuild {
    pub(super) runtime_layer_id: u64, // 主线程只替换同一正式层的四个窗口。
    pub(super) resolution: [u32; 2],  // 创建内存Image所需的完整二维尺寸。
    pub(super) density: Vec<u8>,      // 从原始R8复制并清零净空纹素的完成值。
}

/// 一次 Blade 建筑净空后台任务的完整原子结果。
pub(super) struct DynamicBladeClearanceBuild {
    pub(super) generation: u32,         // 切换场景后的迟到结果不得采用。
    pub(super) revision: u32,           // 植被热更后的迟到结果不得采用。
    pub(super) clearance_revision: u64, // 建筑移动、删除或撤销后的旧结果不得采用。
    pub(super) draft_revision: Option<u64>, // 最新完整密度即使没有建筑变化也必须更新全部草窗。
    pub(super) layers: Vec<DynamicBladeClearanceLayerBuild>, // 全部Blade层共同完成后才替换窗口。
}

/// Blade窗口和全部Mesh空间块共用的当前修订父根。
pub(super) struct DynamicVegetationRootBuild {
    pub(super) root: Entity,              // 原子替换时只切换这一棵修订根。
    pub(super) blade_layer_count: usize,  // 状态面板无需重新遍历正式层。
    pub(super) blade_window_count: usize, // 每个Blade层固定四窗的真实创建数。
}

/// 一个Mesh层空间块的稳定身份；层索引和全图块坐标共同决定唯一内容。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct DynamicMeshChunkKey {
    pub(super) layer_index: usize, // 同一坐标的树木、灌木和岩石分别拥有独立常驻生命周期。
    pub(super) x: i32,             // 以场景世界原点对齐的固定块列，不随相机移动改变散布。
    pub(super) z: i32,             // 以场景世界原点对齐的固定块行，不随相机移动改变散布。
}

/// 单个空间块的后台精确散布结果。
pub(super) struct DynamicMeshChunkBuild {
    pub(super) generation: u32,         // 切换场景后迟到结果必须由代次拒收。
    pub(super) revision: u32,           // 密度或高度热更后迟到结果必须由修订拒收。
    pub(super) clearance_revision: u64, // 建筑移动、删除或撤销后旧块不得覆盖新净空。
    pub(super) ground_revision: Option<ClipmapGroundContentRevision>, // 视角换页不改变，雕刻换代必须拒收。
    pub(super) key: DynamicMeshChunkKey, // 完成值只能安装到请求时的同一层和同一块。
    pub(super) transforms: Vec<SceneVegetationInstanceTransform>, // 仅包含当前半开块内被作者密度选中的实例。
    /// 后台冻结真实作者根点的三维包围盒；空块为None，倾斜后的近场粗筛不能丢弃根点Y。
    pub(super) source_root_bounds: Option<[Vec3; 2]>,
}

/// 单块涉及未到达高度页时不发布部分实例，等待局部查询身份变化后再试。
pub(super) struct DynamicMeshChunkGroundWait {
    pub(super) generation: u32,
    pub(super) revision: u32,
    pub(super) clearance_revision: u64,
    pub(super) ground_revision: ClipmapGroundContentRevision,
    pub(super) query_revision: ClipmapGroundQueryRevision,
    pub(super) key: DynamicMeshChunkKey,
}

pub(super) enum DynamicMeshChunkOutcome {
    Ready(DynamicMeshChunkBuild),
    WaitingForGround(DynamicMeshChunkGroundWait),
}

/// 单块作者结果在隐藏父根下转成共享实例缓冲，完整后只切换一个父根的可见性。
pub(super) struct DynamicMeshChunkStaging {
    pub(super) root: Entity,
    pub(super) build: DynamicMeshChunkBuild, // 完成值仅在一次批次安装期间保持拥有。
}

/// 一个已经完整可见的常驻空间块。
pub(super) struct DynamicMeshResidentChunk {
    pub(super) root: Entity,            // 回收时只需精确销毁该块父根。
    pub(super) clearance_revision: u64, // 同坐标旧净空块保留到新完整块原子替换。
    pub(super) ground_revision: Option<ClipmapGroundContentRevision>, // Clipmap高度换代后逐块替换。
    pub(super) instance_count: usize,   // 状态面板无需遍历ECS即可汇总当前常驻实例数。
    pub(super) render_node_count: usize, // 非空块有限LOD/部件承接节点数；空块为零。
    /// 当前块完整作者变换；近场碰撞只在玩家跨小网格时扫描与其半径相交的块。
    pub(super) transforms: Arc<[SceneVegetationInstanceTransform]>,
    /// 直接采用后台根点包围盒；主线程只做固定成本矩阵变盒，不重新遍历逐株数据求范围。
    pub(super) source_root_bounds: Option<[Vec3; 2]>,
    /// 按全图稳定实例 ID 索引当前真实 Avian 代理；数量只由玩家附近内容决定。
    pub(super) collision_proxies: BTreeMap<u64, Entity>,
}

/// 动态场景唯一植被写侧状态。
#[derive(Resource, Default)]
pub(crate) struct DynamicVegetationState {
    pub(super) draft_revision: Option<u64>, // None表示当前场景使用磁盘Blade定义。
    pub(super) generation: u32,
    pub(super) revision: u32,
    pub(super) catalog_revision: u64,
    pub(super) preset_catalog_revision: u64, // 开发者预设热更独立触发完整植被修订。
    pub(super) scene_id: Option<SceneId>,
    pub(super) terrain_generation: Option<u64>, // 完整GLB高度换代时建立一次植被修订。
    pub(super) definition_handle: Option<Handle<SceneVegetationAsset>>,
    pub(super) pending: Option<DynamicVegetationPending>,
    pub(super) task: Option<Task<Result<DynamicVegetationBuild, String>>>,
    pub(super) blade_clearance_task: Option<Task<Result<DynamicBladeClearanceBuild, String>>>, // 单在途CPU遮罩任务限制瞬时内存。
    pub(super) blade_clearance_task_identity: Option<(u32, u32, u64, Option<u64>)>, // 根、净空与密度来源共同绑定单飞任务。
    pub(super) blade_clearance_applied_revision: Option<(u64, Option<u64>)>, // 密度刷印也使遮罩更新，窗口实体保持稳定。
    pub(super) active_root: Option<Entity>,
    pub(super) mesh_source: Option<Arc<DynamicVegetationBuild>>, // 主线程与每帧常驻系统只浅克隆当前修订源，不保存全图逐株数组。
    /// 新GLB高度代次对应的完整Blade根；保持隐藏直到地形、碰撞、水域与植被共同提交。
    pub(super) candidate_root: Option<DynamicVegetationRootBuild>,
    /// 与隐藏根同修订的Mesh分块输入；共同提交前不能驱动可见常驻块。
    pub(super) candidate_mesh_source: Option<Arc<DynamicVegetationBuild>>,
    pub(super) mesh_chunk_task: Option<Task<Result<DynamicMeshChunkOutcome, String>>>, // single-flight区域散布槽提供固定后台背压。
    pub(super) mesh_chunk_task_identity: Option<(
        u32,
        u32,
        u64,
        Option<ClipmapGroundContentRevision>,
        DynamicMeshChunkKey,
    )>, // 代次、植被、净空、高度内容和块身份共同拒绝迟到结果。
    pub(super) mesh_chunk_staging: Option<DynamicMeshChunkStaging>, // 主线程同一时间只安装一个完整共享实例块。
    pub(super) queued_mesh_chunks: VecDeque<DynamicMeshChunkKey>, // 相机范围内缺失块按距离近到远等待生成。
    pub(super) desired_mesh_chunks: BTreeSet<DynamicMeshChunkKey>, // 当前画质与焦点真正需要的块，不是作者容量上限。
    pub(super) mesh_chunk_height_waits: BTreeMap<DynamicMeshChunkKey, ClipmapGroundQueryRevision>, // 同一缺页窗口不忙重试，窗口恢复后逐块继续。
    pub(super) resident_mesh_chunks: BTreeMap<DynamicMeshChunkKey, DynamicMeshResidentChunk>, // 已完整显示块可精确差分回收。
    pub(super) residency_focus_chunk: Option<[i32; 2]>, // 相机未跨固定块时稳定帧不重建期望集合。
    pub(super) residency_range_bits: Option<u32>, // 场景细节距离倍率变化时才重新计算空间常驻。
    pub(super) residency_clearance_revision: Option<u64>, // 建筑净空换代时重排当前所需块，但保留旧完整块到替换完成。
    pub(super) residency_ground_revision: Option<ClipmapGroundContentRevision>, // 视角修订不进入此字段。
    pub(super) collision_focus_cell: Option<[i32; 2]>, // 玩家未跨十六米网格且块集合未变时不扫描近场实例。
    pub(super) collision_frame: Option<feathrise_scene_core::residency::SceneSourceFrame>, // 玩家静止时网格移动/旋转也须重核对近场。
    pub(super) dirty: bool,
    /// 已启动或已明确失败的当前身份，防止稳定帧重复提交同一后台任务。
    pub(super) attempted_revision: Option<(u32, u32)>,
    pub(super) failure_logged_revision: Option<u32>,
}

/// 宿主只把并发席位归还给真正结清后台工作的实例。
impl DynamicVegetationState {
    pub(in crate::scenes::dynamic_scene) fn background_busy(&self) -> bool {
        self.task.is_some() || self.blade_clearance_task.is_some() || self.mesh_chunk_task.is_some()
    }
    pub(in crate::scenes::dynamic_scene) fn background_finished(&self) -> bool {
        self.task.as_ref().is_none_or(|task| task.is_finished()) && self.blade_clearance_task.as_ref().is_none_or(|task| task.is_finished()) && self.mesh_chunk_task.as_ref().is_none_or(|task| task.is_finished())
    }
}

impl DynamicVegetationState {
    /// 全局地形提交门查询当前植被是否已为指定GLB候选建立完整隐藏根。
    pub(crate) fn candidate_ready_for_terrain(
        &self,
        scene_id: &SceneId,
        terrain_generation: u64,
        required: bool,
    ) -> bool {
        if !required {
            return true; // 未声明植被定义的场景没有待切换根。
        }
        let Some(pending) = self.pending.as_ref() else {
            return false;
        };
        let Some(source) = self.candidate_mesh_source.as_ref() else {
            return false;
        };
        pending.scene_id == *scene_id
            && pending.terrain_generation == terrain_generation
            && self.candidate_root.is_some()
            && source.generation == pending.generation
            && source.revision == pending.revision
    }

    pub(super) fn begin(&mut self) {
        let generation = self.generation.wrapping_add(1).max(1);
        let task = self.task.take(); // CPU任务没有协作取消点时继续作为唯一在途槽，重进后先回收迟到结果。
        let blade_clearance_task = self.blade_clearance_task.take(); // 旧遮罩任务自然结束，再由联合身份丢弃。
        let blade_clearance_task_identity = self.blade_clearance_task_identity.take(); // 保留在途身份到完成边界。
        let mesh_chunk_task = self.mesh_chunk_task.take(); // 区域任务同样自然结束，再由新代次身份门直接丢弃。
        let mesh_chunk_task_identity = self.mesh_chunk_task_identity.take(); // 保留旧任务完整身份直到完成，防止跨修订误去重。
        *self = Self {
            generation,
            revision: 1,
            dirty: true,
            task,
            blade_clearance_task,
            blade_clearance_task_identity,
            mesh_chunk_task,
            mesh_chunk_task_identity,
            ..default()
        };
    }

    /// 合并同帧多个资产事件，只创建一个新修订；旧任务保留在唯一槽内直到自然结束。
    pub(super) fn request_rebuild(&mut self) {
        if !self.dirty {
            self.revision = self.revision.wrapping_add(1).max(1);
            self.dirty = true;
            self.pending = None;
            self.attempted_revision = None;
            self.failure_logged_revision = None;
        }
    }

    pub(super) fn current(&self, generation: u32, revision: u32) -> bool {
        self.generation == generation && self.revision == revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undeclared_vegetation_does_not_block_a_terrain_commit() {
        let scene_id = SceneId::parse("MAP-vegetation-optional").expect("测试SceneId必须合法");
        let state = DynamicVegetationState::default();
        assert!(state.candidate_ready_for_terrain(&scene_id, 7, false));
        assert!(!state.candidate_ready_for_terrain(&scene_id, 7, true));
    }
}
