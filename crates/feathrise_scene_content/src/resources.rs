//! 已加载场景目录的句柄注册表。

use std::collections::BTreeMap; // 场景按稳定ID排序，UI与测试不会受哈希顺序影响。

use bevy::prelude::*; // Handle与Resource。

use crate::{
    BuildingPresetCatalogAsset, CourtyardPresetCatalogAsset, SceneBoundariesAsset, SceneCatalogAsset,
    SceneId, SceneManifestAsset, ScenePlacementsAsset, ScenePortalsAsset, SceneSurfaceAsset,
    SceneVegetationAsset, SceneWaterAsset, ResolvedBuildingPresetCatalog,
    ResolvedCourtyardPresetCatalog, ResolvedSurfacePresetCatalog, ResolvedVegetationPresetCatalog,
    SurfacePresetCatalogAsset, VegetationPresetCatalogAsset,
};

/// 全局建筑预设的强句柄、当前原子快照与热更诊断。
#[derive(Resource, Debug)]
pub struct BuildingPresetRegistry {
    /// 启动时固定持有的目录资产句柄。
    pub catalog: Handle<BuildingPresetCatalogAsset>,
    /// 只有整份目录校验并采用成功后才递增。
    pub revision: u64,
    /// 最近一次完整有效的可查询建筑目录。
    pub resolved: ResolvedBuildingPresetCatalog,
    /// 最近一次加载或校验错误；失败期间继续保留上一快照。
    pub last_error: Option<String>,
}

impl BuildingPresetRegistry {
    /// 按规范 ID 或历史别名读取当前建筑预设。
    pub fn resolve(&self, id: &str) -> Option<&crate::BuildingPresetDescriptor> {
        self.resolved.resolve(id) // 场景旧引用和 F12 当前选择共享规范化结果。
    }

    /// 按开发者声明顺序借用全部当前建筑预设。
    pub fn presets(&self) -> &[crate::BuildingPresetDescriptor] {
        self.resolved.presets() // 稳定帧列表不复制任意数量描述。
    }
}

/// 全局院落组合预设的强句柄、完整快照及热更诊断。
#[derive(Resource, Debug)]
pub struct CourtyardPresetRegistry {
    /// 启动时固定持有的唯一全局目录句柄。
    pub catalog: Handle<CourtyardPresetCatalogAsset>,
    /// 每次完整校验并采用成功后才递增。
    pub revision: u64,
    /// 最近一次有效的院落组合目录与稳定 ID 索引。
    pub resolved: ResolvedCourtyardPresetCatalog,
    /// 最近一次加载或校验错误；坏热更期间不清空已采用快照。
    pub last_error: Option<String>,
}

impl CourtyardPresetRegistry {
    /// 通过规范稳定 ID 读取当前完整组合。
    pub fn resolve(&self, id: &str) -> Option<&crate::CourtyardPresetDescriptor> {
        self.resolved.resolve(id) // 场景和 F12 均借用同一修订。
    }

    /// 按作者声明顺序借用当前组合列表。
    pub fn presets(&self) -> &[crate::CourtyardPresetDescriptor] {
        self.resolved.presets() // 稳定帧不复制任意数量的成员模板。
    }
}

/// 全局 PBR 地表预设的强句柄、当前原子快照与热更诊断。
#[derive(Resource, Debug)]
pub struct SurfacePresetRegistry {
    pub catalog: Handle<SurfacePresetCatalogAsset>,
    /// 整份目录通过后才递增，后台派生和动态场景可拒收旧身份结果。
    pub revision: u64,
    pub resolved: ResolvedSurfacePresetCatalog,
    pub last_error: Option<String>,
}

impl SurfacePresetRegistry {
    pub fn resolve(&self, id: &str) -> Option<&crate::SurfacePresetDescriptor> {
        self.resolved.resolve(id)
    }

    pub fn presets(&self) -> &[crate::SurfacePresetDescriptor] {
        self.resolved.presets()
    }
}

/// 全局开发者植被预设的强句柄、当前原子快照与热更诊断。
#[derive(Resource, Debug)]
pub struct VegetationPresetRegistry {
    /// 启动时固定持有的目录资产句柄。
    pub catalog: Handle<VegetationPresetCatalogAsset>,
    /// 成功采用次数；场景和编辑器可据此拒收旧目录派生任务。
    pub revision: u64,
    /// 最近一次完整校验通过的可查询快照。
    pub resolved: ResolvedVegetationPresetCatalog,
    /// 最近一次加载或校验失败原因；失败时继续保留上一快照。
    pub last_error: Option<String>,
}

impl VegetationPresetRegistry {
    /// 按规范 ID 或历史别名读取当前预设。
    pub fn resolve(&self, id: &str) -> Option<&crate::VegetationPresetDescriptor> {
        self.resolved.resolve(id) // 所有消费者共享同一别名规范化结果。
    }

    /// 按开发者声明顺序读取全部当前预设。
    pub fn presets(&self) -> &[crate::VegetationPresetDescriptor] {
        self.resolved.presets() // F12连续列表借用快照，不复制任意数量描述。
    }
}

/// 单个场景三个私有热加载资产的强句柄。
#[derive(Clone, Debug)]
pub struct SceneContentHandles {
    /// 原游戏兼容路径只原样传递；共享场景不注册、加载或解释角色文件。
    pub characters_path: Option<String>,
    /// 作者清单句柄。
    pub manifest: Handle<SceneManifestAsset>,
    /// 传送图句柄。
    pub portals: Handle<ScenePortalsAsset>,
    /// 显式场景实例作者文件句柄。
    pub placements: Handle<ScenePlacementsAsset>,
    /// 可选地表权重作者文件句柄。
    pub surface: Option<Handle<SceneSurfaceAsset>>,
    /// 可选植被密度作者文件句柄。
    pub vegetation: Option<Handle<SceneVegetationAsset>>,
    /// 可选可绘制水域作者文件句柄。
    pub water: Option<Handle<SceneWaterAsset>>,
    /// 可选区域边界与空气墙作者文件句柄。
    pub boundaries: Option<Handle<SceneBoundariesAsset>>,
}

/// 全局场景目录及其当前入口句柄。
#[derive(Resource, Debug, Default)]
pub struct SceneContentRegistry {
    /// 启动时固定持有的目录资产。
    pub catalog: Handle<SceneCatalogAsset>,
    /// 目录成功应用次数；消费者据此拒收过期后台任务。
    pub revision: u64,
    /// 当前目录中每个场景的私有资产句柄。
    pub scenes: BTreeMap<SceneId, SceneContentHandles>,
    /// 最近一次目录校验失败原因。
    pub last_error: Option<String>,
}

/// 当前由目录驱动的动态场景选择；F12编辑器和传送系统在切场景前写入。
#[derive(Resource, Clone, Debug, Default)]
pub struct ActiveDynamicScene {
    /// None表示尚未选择动态场景，进入DynamicSceneRoot会保持失败白模。
    pub scene_id: Option<SceneId>,
}

impl ActiveDynamicScene {
    /// 切换到指定场景ID。
    pub fn select(&mut self, scene_id: SceneId) {
        self.scene_id = Some(scene_id);
    }
}

impl SceneContentRegistry {
    /// 返回指定场景私有资产的强句柄。
    pub fn handles(&self, scene_id: &SceneId) -> Option<&SceneContentHandles> {
        self.scenes.get(scene_id)
    }
}

/// F12已完整采用的积水遮罩；Some场景加None图像表示显式禁止积水。
#[derive(Resource, Default, Clone)] // 消费者只复制轻量句柄和世界边界。
pub struct ScenePuddleLiveOverride {
    // 退出作者模式后清除并归还正式纹理。
    pub scene_id: Option<crate::SceneId>, // None表示没有作者覆盖，Some必须严格匹配场景。
    pub revision: u64,                // 每次完整GPU候选采用递增。
    pub mask: Option<Handle<Image>>,  // 线性R8潜力；零值禁止积水，不表达水深。
    pub world_min: Vec2,              // 首纹素对应的世界XZ下界。
    pub world_max: Vec2,              // 末纹素对应的世界XZ上界。
    pub preview: bool,                // 积水工具激活时显示蓝色潜力诊断色。
} // 结束公开作者覆盖协议。
/// 最近一次积水保存等待正式纹理采用的内容证明；正式场景后台比对基级R8后清除场景身份作为回执。
#[derive(Resource, Default, Clone)] // 大画布只共享Arc，保存完成帧不得深拷贝。
pub struct ScenePuddleSaveHandoff {
    // 不以同规格、文件时间戳或旧Image事件冒充本次保存。
    pub scene_id: Option<crate::SceneId>, // None表示没有待交接保存；Some限定所属场景。
    pub texture_path: Option<String>, // 本次已经原子落盘的精确KTX2路径。
    pub expected_mask: Option<std::sync::Arc<Vec<u8>>>, // 本次保存原始R8；候选基级必须逐字节一致。
    pub revision: u64,                // 每次成功保存递增，拒绝前一保存的迟到比较任务。
} // 正式场景完成采用后归还F12覆盖，外部热更新继续正常工作。
/// 多层碰撞的已保存与实际采用版本；编辑器不得把保存成功冒充物理完成。
#[derive(bevy::prelude::Resource, Clone, Debug, Default, PartialEq)] // 只承载轻量诊断数据。
pub struct SceneCollisionLodStatus {
    // 场景运行时发布，编辑器只读。
    pub scene_id: Option<crate::SceneId>, // 切换场景时隔离旧诊断。
    pub saved_lods: Vec<crate::SceneCollisionLod>, // 当前清单中的已保存配置。
    pub applied_lods: Vec<crate::SceneCollisionLod>, // 完整物理步边界已采用配置。
    pub detail: String,               // 后台排队、采用或失败说明。
    pub resident_blocks: usize,       // 当前实际驻留块数。
    pub rebuilt_blocks: usize,        // 上一次候选真实新建块数。
    pub paused_entities: usize,       // 范围外暂停但未销毁的实体数。
    pub build_milliseconds: f32,      // 最近后台高度采样与形状构建耗时，不含布局、IO或排队。
    pub layout_milliseconds: f32,     // 最近采用批次后台规划、拓扑与依赖准备耗时。
    pub request_to_adopt_milliseconds: f32, // 请求冻结到采用的总墙钟延迟，包含排队、布局、IO与构建。
    pub adopted_center_lag_meters: f32, // 当前玩家与已采用碰撞中心的水平距离，不等同于实际缺承托距离。
    pub layout_rebuilt_blocks: usize,   // 最近批次真正生成拓扑及页依赖的块数。
    pub layout_reused_blocks: usize,    // 最近批次直接共享缓存拓扑及页依赖的块数。
    pub applied_spacing_meters: Vec<[f32; 2]>, // 每层实际X/Z格距，不以倍率冒充米数。
    pub resident_source_pages: usize,   // 已采用形状依赖的高度页数。
    pub peak_shape_count: usize,        // 会话内旧驻留加完整候选形状的峰值数量，不冒充总内存字节。
} // 状态不写入场景资产。

/// 原运行时事务边沿累计的实际工作，不遍历Mesh或读取GPU；换场景/根后重新计数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SceneTerrainDisplayLodWork {
    pub mesh_builds: u64,
    pub mesh_discarded: u64,
    pub mesh_uploads: u64,
    pub page_uploads: u64,
    pub display_adoptions: u64,
    pub mesh_upload_bytes: u64,
    pub page_upload_bytes: u64,
}

/// Geometry Clipmap显示LOD的已保存、已采用及完整候选资源诊断。
#[derive(bevy::prelude::Resource, Clone, Debug, Default, PartialEq)] // 只保存小型作者列表和计数，不持有Mesh或图集。
pub struct SceneTerrainDisplayLodStatus {
    // 场景运行时写入，F12只读展示保存与采用差异。
    pub scene_id: Option<crate::SceneId>, // 切换场景后旧状态不得显示在新场景资料弹框。
    pub saved_lods: Option<Vec<crate::SceneTerrainDisplayLod>>, // None明确表示清单仍处于兼容自动模式。
    pub applied_lods: Vec<crate::SceneTerrainDisplayLod>, // 当前正式Mesh解析后的作者列表，旧自动模式也回显等价值。
    pub phase: crate::SceneTerrainDisplayLodPhase,        // UI不得解析detail字符串判断状态机阶段。
    pub detail: String,                                 // 排队、失败和兼容模式的中文说明。
    pub actual_half_extents_meters: Vec<[f32; 2]>,      // 每个有效环带量化后的X/Z累计半径。
    pub effective_band_count: usize, // 包含倍率跳级产生的内部2:1过渡带，不冒充作者行数。
    pub resident_height_pages: usize, // 当前正式候选实际上传的非平地高度页数。
    pub vertex_count: usize,         // 当前正式组合Mesh的顶点数量。
    pub triangle_count: usize,       // 当前正式组合Mesh的三角形数量。
    pub mesh_bytes: u64,             // 位置、法线、两套UV及u32索引的原始上传字节。
    pub build_milliseconds: f32,     // 最近完整后台拓扑构建耗时。
    pub upload_wait_milliseconds: f32, // 最近资产登记至GPU预热完成的墙钟时间。
    pub work: SceneTerrainDisplayLodWork, // 累计次数与字节区分同档复用、重复构建和页窗口上传。
} // 诊断不参与场景保存或运行时选择。
