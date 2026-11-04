//! F3通用地表阶段、纹理、管线、承接实体与Opaque排队报告。

use bevy::ecs::system::SystemParam; // 查询单独封装，植被报告不再直接依赖地表组件。
use bevy::prelude::Query; // 主世界只统计明确承接标记的固定少量实体。
use feathrise_environment::terrain_surface::{
    // 读取通用环境模块公开组件与上一完整RenderApp帧快照。
    TerrainSurfaceCarrier,
    TerrainSurfaceRenderStats,
};

/// F3通用地表功能域所需的主世界查询。
#[derive(SystemParam)] // 查询与草窗、树木和景观查询保持独立职责。
pub(super) struct TerrainSurfaceMonitorQueries<'w, 's> {
    carriers: Query<'w, 's, &'static TerrainSurfaceCarrier>, // 正式地形稳定为一，预热期间可短暂为一或二。
} // 完成地表查询参数定义。

/// 构建一行独立地表报告，不把纹理或材质资产计入草地驻留。
pub(super) fn build_terrain_surface_report(
    // 仅F3半秒刷新且当前植被页可见时执行。
    queries: &TerrainSurfaceMonitorQueries<'_, '_>, // 读取主世界承接实体数量。
    stats: Option<&TerrainSurfaceRenderStats>,      // 插件缺失时允许显示明确未启用状态。
) -> String {
    // 返回可嵌入任一叶片分页的独立报告行。
    let stats = stats.copied().unwrap_or_default(); // 无资源时使用Inactive全零快照。
    let main_carriers = queries.carriers.iter().count(); // 查询规模固定为零、一个或预热期两个。
    format!(
        // 所有首次失败和热更失败都可从同一行直接读出具体项。
        "地表材质 阶段 {:?} | 代次 {} | 失败 {:?} | 纹理组 就绪/总数/等待/错误 {}/{}/{}/{} | PBR管线 就绪/等待/错误 {}/{}/{} | 承接 主世界/RenderApp {}/{} | Opaque排队 {} | 独立代理 0",
        stats.phase,             // 白模、加载、预热、活动或失败生命周期。
        stats.generation,        // 当前地图代次用于排除迟到预热结果。
        stats.failure,           // 权重、基础色数组、ARM数组或Shader具体失败项。
        stats.texture_ready,     // 已经上传GpuImage的正式纹理数量。
        stats.texture_expected,  // HN-01固定为三，动态索引PBR地表为五。
        stats.texture_waiting,   // 尚未上传且未失败的纹理数量。
        stats.texture_error,     // 任一正式纹理失败时为一。
        stats.pipeline_ready,    // 全部地表专用管线可执行时为一。
        stats.pipeline_waiting,  // 提取、专用化或异步创建中时为一。
        stats.pipeline_error,    // WGSL或设备创建失败时为一。
        main_carriers,           // 主世界当前正式或预热承接标记数量。
        stats.carrier_instances, // 上一完整RenderApp帧准确承接数量。
        stats.phase_queued,      // 至少一个准确管线已进入Opaque3d分箱。
    ) // 返回独立地表报告文本。
} // 完成通用地表报告格式化。
