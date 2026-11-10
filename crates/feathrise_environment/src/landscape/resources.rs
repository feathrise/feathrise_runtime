//! 固定景观运行时诊断资源。
//! 【功能设计】只记录固定实例、共享资产有效载荷与碰撞数量，不缓存逐帧渲染副本。
//! 【模块所有权】诊断快照继续由固定景观子域定义，F3仍通过原公开路径只读消费。

use bevy::prelude::*; // Resource派生与默认值类型。

/// 一类固定景观的轻量运行时统计。
#[derive(Clone, Debug, Default, PartialEq)] // 灌木和岩石各保存一份相同口径快照。
pub struct LandscapeKindRuntimeStats {
    pub content_label: &'static str, // F3显示“灌木”或“岩石”的稳定中文名。
    pub expected_instances: usize,   // 离线清单声明的完整稳定落点数量。
    pub spawned_instances: usize,    // 当前会话实际生成的根实体数量。
    pub max_instances: usize,        // 当前质量档允许的固定最坏实例预算。
    pub render_nodes: usize,         // 当前四档LOD子节点总量。
    pub max_render_nodes: usize,     // 硬上限全部实例存在时的四档节点预算。
    pub placement_grid_spacing_m: u32, // 离线候选网格米制间距。
    pub all_scene_resident: bool,      // 本轮固定数量足够小，全部随HN-01常驻。
    pub shadow_end_m: f32,           // 当前类别最远投影LOD的退出距离。
    pub collision_proxies: usize,    // 玩家附近实际驻留的简化碰撞数量。
    pub collision_eligible_instances: usize, // 离线标记为中大型可阻挡岩石的总数。
    pub mesh_assets: usize,          // 三个源primitive加一份动态代理的唯一Mesh数量。
    pub material_assets: usize,      // 当前四档实际共享材质数量；第四档可复用最低源材质。
    pub hardware_batch_budget: usize, // 四档共享网格/材质的理论主渲染批次上限。
    pub materials_ready: bool,       // 四档网格与材质全部解码并完成校准后才为真。
    pub asset_ready_ms: f64,         // 从进入场景发起加载到四档原子就绪的会话耗时。
    pub generated_ultra_proxy: bool, // 第四档是否由加载期代码从最低源网格生成而非外部GLB。
    pub lod_vertex_counts: [u64; 4], // 高、中、低、超低四档唯一Mesh的真实顶点数。
    pub lod_index_counts: [u64; 4],  // 高、中、低、超低四档唯一Mesh的真实索引数。
    pub mesh_vertex_count: u64,      // 四个唯一Mesh的真实顶点数量。
    pub mesh_index_count: u64,       // 四个唯一Mesh的真实索引数量。
    pub mesh_vertex_bytes: u64,      // Bevy Mesh公开顶点缓冲有效载荷。
    pub mesh_index_bytes: u64,       // Bevy Mesh公开索引缓冲有效载荷。
    pub referenced_image_count: usize, // 三份源PBR材质去重后的图片数量。
    pub referenced_image_bytes: u64, // Assets<Image>中已解码的像素字节。
    pub source_bundle_bytes: u64,    // 直接复用的GLB整包磁盘字节和，用于暴露未拆包风险。
    pub geometry_lod_reduces: bool,  // 四档顶点量是否真实递减，动态代理成功后两类均应为true。
} // 完成单类固定景观诊断快照。

/// 当前场景灌木与岩石的聚合诊断资源。
#[derive(Resource, Clone, Debug, Default, PartialEq)] // 非HN-01场景保持全零并防止继承旧样本。
pub struct LandscapeRuntimeStats {
    pub scene_code: &'static str,           // 当前固定景观所属场景编号。
    pub shrub: LandscapeKindRuntimeStats, // 灌木固定实例、风材质和资产成本。
    pub rock: LandscapeKindRuntimeStats,  // 岩石固定实例、碰撞与资产成本。
} // 完成场景级固定景观诊断资源。
