//! 树木运行时诊断资源。
//! 资源记录固定内容规模、资产就绪成本与上一完整PBR排队帧，不缓存逐树渲染副本。
//! 【模块所有权】主世界与RenderApp快照继续由树木子域共同维护，F3沿原公开路径读取。

use bevy::asset::AssetId; // 渲染遥测只提取七个共享Mesh的稳定资产ID。
use bevy::prelude::*; // 引入Mesh、Resource和资源提取所需基础类型。
use bevy::render::extract_resource::ExtractResource; // 把轻量资产ID表复制到RenderApp匹配真实PBR分箱。

/// 当前地图树木技术原型的轻量统计。
#[derive(Resource, Clone, Debug, Default, PartialEq)] // 非树地图保持全零，F3不会继承上一地图样本。
pub struct TreeRuntimeStats {
    pub map_code: &'static str,        // 当前树木内容所属地图编号。
    pub content_label: &'static str,   // 当前地图树木内容的可读名称。
    pub expected_instances: usize,     // 地图规格声明的稳定树位数量。
    pub spawned_instances: usize,      // 当前会话实际生成的树根数量。
    pub controlled_instances: usize,   // 已验收局部受控树位数量。
    pub density_instances: usize,      // 由全图林带密度规则离线生成的稳定树位数量。
    pub max_instances: usize,          // 当前质量档允许的全图树根固定最坏预算。
    pub render_nodes: usize,           // 四档树干、树冠和公告板渲染节点总量。
    pub max_render_nodes: usize,       // 当前质量档全部树位均存在时的渲染节点硬上限。
    pub placement_grid_spacing_m: u32, // 离线林带候选格距，零表示当前地图没有密度树位。
    pub all_map_resident: bool,        // 树量足够小时全图常驻，false保留给未来分块流送质量档。
    pub shadow_end_m: f32,             // 允许投影的最远实体LOD退出距离。
    pub collision_proxies: usize,      // 当前玩家附近实际存在的树干碰撞代理。
    pub mesh_assets: usize,            // 当前原型显式使用的唯一Mesh primitive数量。
    pub material_assets: usize,        // 当前原型显式使用的唯一材质数量。
    pub hardware_batch_budget: usize,  // 同一时刻高、中、低或公告板的固定最大材质/网格批次预算。
    pub materials_ready: bool,         // 六份GPU风材质与固定公告板完成原子安装后才为真。
    pub asset_ready_ms: f64,           // 从进入地图发起加载到七网格三材质原子就绪的会话耗时。
    pub mesh_vertex_count: u64,        // 七个唯一Mesh资产的顶点数量，不按全图实例重复累计。
    pub mesh_index_count: u64,         // 七个唯一Mesh资产的索引数量，不按可见LOD重复累计。
    pub mesh_vertex_bytes: u64, // Bevy Mesh CPU顶点缓冲的精确字节和，同时也是GPU上传有效载荷。
    pub mesh_index_bytes: u64,  // Bevy Mesh CPU索引缓冲的精确字节和，同时也是GPU上传有效载荷。
    pub referenced_image_count: usize, // 三份源PBR材质实际引用并已解码的唯一图片数量。
    pub referenced_image_bytes: u64, // 上述图片在Assets<Image>中的解码像素字节，不冒充驱动显存。
} // 完成不会随逐帧候选增长的树木诊断快照。

/// 单个PBR阶段中能够由Bevy公开分箱直接证明的树木提交量。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] // 值类型允许RenderApp无锁整份覆盖并回传主世界。
pub struct TreeRenderPhaseStats {
    pub views: u32,       // 至少包含一个树木分箱的主视图或阴影视图数量。
    pub draw_groups: u32, // 多绘制批集、普通批次或独立实体形成的实际提交组数量。
    pub bins: u32,        // 与七个树木Mesh资产ID匹配的真实Bevy渲染分箱数量。
    pub instances: u32,   // 分箱内树木实体实例数，跨多个视图时按视图累计。
} // 完成单阶段公开遥测定义。

/// 上一完整渲染帧的树木PBR排队快照。
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)] // 主世界F3只读取，RenderApp每帧覆盖。
pub struct TreeRenderStats {
    pub sampled: bool,                    // 至少完成过一次Render阶段采样后才为真。
    pub opaque: TreeRenderPhaseStats,     // 树干进入主画面Opaque3d阶段的真实分箱。
    pub alpha_mask: TreeRenderPhaseStats, // 树冠与公告板进入AlphaMask3d阶段的真实分箱。
    pub shadow: TreeRenderPhaseStats,     // 实体树进入方向光或其他灯光Shadow阶段的真实分箱。
} // 完成不会触发GPU读回的PBR排队快照。

/// 当前树木原型供RenderApp识别的唯一Mesh资产ID表。
#[derive(Resource, Clone, Debug, Default, ExtractResource)] // 每帧只提取最多七个轻量ID，不复制Mesh数据。
pub struct TreeRenderMonitorConfig {
    pub mesh_ids: Vec<AssetId<Mesh>>, // 地图进入时写入，离场时清空以截断上一地图渲染样本。
    enabled: bool, // 只有F3树木页可临时授权RenderApp扫描，普通游戏帧保持零监控成本。
} // 完成主世界到RenderApp的树木资产身份通道。

impl TreeRenderMonitorConfig {
    /// 返回当前是否允许扫描树木PBR分箱。
    pub const fn enabled(&self) -> bool {
        self.enabled // 主世界与提取后的RenderApp共享同一显式授权状态。
    }

    /// 仅在目标状态变化时更新监控授权，并返回是否发生真实边沿。
    pub fn set_enabled(&mut self, enabled: bool) -> bool {
        if self.enabled == enabled {
            return false; // F3稳态帧不制造无意义配置变化。
        }
        self.enabled = enabled; // 下一Extract阶段开启采样或截断旧快照。
        true
    }

    /// 用当前地图原型的共享Mesh句柄替换监控目标。
    pub fn replace_from_handles<'a>(
        &mut self,                                           // 原位替换上一地图资产身份。
        handles: impl IntoIterator<Item = &'a Handle<Mesh>>, // 接收地图已经持有的现有资产句柄。
    ) {
        self.mesh_ids.clear(); // 重进地图或切换原型时先截断旧Mesh身份。
        self.mesh_ids.extend(handles.into_iter().map(Handle::id)); // 只保存稳定ID，不增加新的资产强引用。
        self.mesh_ids.sort_unstable(); // 固定顺序便于测试、提取变更检测和问题截图比对。
        self.mesh_ids.dedup(); // 同一Mesh若被多个LOD误复用也只能计入一个分箱身份。
    } // 完成树木渲染监控目标替换。

    /// 离开树木地图时立即清空监控目标。
    pub fn clear(&mut self) {
        self.mesh_ids.clear(); // 下一次Render采样将发布明确全零而不是旧树木分箱。
    } // 完成监控身份清理。
} // 完成树木渲染监控配置行为。
