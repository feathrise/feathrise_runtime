//! F3 按素材预设显示通用 Mesh 原型与已发布的运行时统计。
//! 只读现有完整快照，不在监控刷新帧遍历场景实例或网格。

use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_environment::vegetation::{
    VegetationMeshPrototypeRegistry, VegetationMeshRenderStats, VegetationPrototypeId,
};
use feathrise_scene_content::VegetationPresetRegistry;
use feathrise_scene::DynamicVegetationStatus;

#[derive(SystemParam)]
pub(super) struct MeshMonitorQueries<'w> {
    runtime: Option<Res<'w, DynamicVegetationStatus>>,
    prototypes: Option<Res<'w, VegetationMeshPrototypeRegistry>>,
}

/// 稳定字符串只用于保持现有素材引用，原型编号始终由当前目录解析。
pub(super) fn preset_prototype(
    registry: Option<&VegetationPresetRegistry>,
    preset_id: &str,
) -> Option<VegetationPrototypeId> {
    registry?
        .resolve(preset_id)?
        .resolved_prototype_id()
        .map(VegetationPrototypeId)
}

pub(super) fn build_mesh_report(
    label: &str,
    preset_id: &str,
    queries: &MeshMonitorQueries,
    _meshes: &Assets<Mesh>,
    registry: Option<&VegetationPresetRegistry>,
    render: Option<&VegetationMeshRenderStats>,
) -> String {
    let Some(prototype) = preset_prototype(registry, preset_id) else {
        return format!("{label}：素材预设尚未就绪");
    };
    let lod_count = queries
        .prototypes
        .as_deref()
        .and_then(|registry| registry.get(prototype))
        .map(|prototype| prototype.lods.len());
    let readiness = lod_count.map_or_else(
        || "原型等待加载".to_owned(),
        |count| format!("原型就绪 | {count}档LOD"),
    );
    // 总数明确标为全图共享快照，不能冒充单个素材的逐株统计；各作者层精确值由F4既有缓存显示。
    let runtime = queries.runtime.as_deref().map_or_else(
        || "当前场景植被未激活".to_owned(),
        |status| {
            format!(
                "运行时全图汇总：Mesh实例 {} | 空间批次 {} | 渲染节点 {} | 近场碰撞 {}\n{}",
                status.mesh_instance_count,
                status.mesh_batch_count,
                status.mesh_render_node_count,
                status.mesh_collision_proxy_count,
                status.detail
            )
        },
    );
    let render = render.filter(|stats| stats.sampled).map_or_else(
        || "渲染分箱等待采样".to_owned(),
        |stats| format!("全图渲染统计目标 {}组", stats.targets.len()),
    );
    format!(
        "{label} | {preset_id} | {readiness}\n{runtime}\n{render}\n当前作者层数量与LOD详情见F4植被页"
    )
}
