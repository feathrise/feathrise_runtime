//! 把隐藏光追源的精确 Deferred 租约同步到真正可见的嵌套 Clipmap 地表。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes}; // 每个子地图借用独立状态，父图保留原资源。
use super::super::{SparseHeightPageRuntime, material::SparseClipmapSurfaceMaterial};
use super::{components::ClipmapRaytracingAnchor, resources::ClipmapRaytracingRuntime};
use crate::maps::dynamic_map::sparse_runtime::SparseClipmapDisplay;
use bevy::prelude::*;
use feathrise_environment::terrain_surface::{TerrainSurfaceMaterial, TerrainSurfaceMaterialOverride};

pub(super) fn sync_visible_material(
    pages: InstanceRes<SparseHeightPageRuntime>,
    runtime: Res<ClipmapRaytracingRuntime>,
    route: Res<super::ClipmapRaytracingInstanceRoute>,
    surfaces: Res<Assets<TerrainSurfaceMaterial>>,
    mut wrapped: ResMut<Assets<SparseClipmapSurfaceMaterial>>,
    anchors: InstanceQuery<(
        &ClipmapRaytracingAnchor,
        &MeshMaterial3d<TerrainSurfaceMaterial>,
        Option<&TerrainSurfaceMaterialOverride>,
    )>,
    displays: InstanceQuery<
        (Entity, &MeshMaterial3d<SparseClipmapSurfaceMaterial>),
        With<SparseClipmapDisplay>,
    >,
) {
    let Ok((display, handle)) = displays.single() else {
        return;
    };
    let Some(source) = pages.raytracing_material() else {
        return;
    };
    let leased = runtime
        .anchor
        .and_then(|entity| anchors.get(entity).ok())
        .filter(|(anchor, material, lease)| {
            anchor.identity.display == display
                && anchor.source.id() == source.id()
                && lease.is_some_and(|lease| lease.matches(source.id(), material.id()))
                && surfaces.get(&material.0).is_some()
        });
    let base = leased
        .and_then(|(_, material, _)| surfaces.get(&material.0))
        .or_else(|| surfaces.get(&source));
    let Some(base) = base else {
        return;
    };
    let enabled = leased.is_some();
    let grid = leased.map_or(Vec4::ZERO, |(anchor, _, _)| anchor.grid.uniform());
    let mut axes = leased.map_or_else(
        super::super::material::ClipmapProxyAxes::default,
        |(anchor, _, _)| anchor.axes,
    );
    if route.anchor == runtime.anchor {
        axes.info.w = route.index.min(15) as f32;
    }
    let Some(current) = wrapped.get(&handle.0) else {
        return;
    };
    if current.base.base.opaque_render_method == base.base.opaque_render_method
        && current
            .base
            .extension
            .dynamic_terrain_shadow_terminator_data_enabled()
            == enabled
        && current.extension.params.reserved == grid
        && current.extension.proxy_axes == axes
    {
        return;
    } // 标量核对实际字段：原地重包也会修复，稳定帧不触发 Modified 或历史 reset。
    let Some(mut material) = wrapped.get_mut(&handle.0) else {
        return;
    };
    // 只转交本会话拥有的两个字段；完整拷贝会覆盖 F12 当前平直模式及本帧作者外观。
    material.base.base.opaque_render_method = base.base.opaque_render_method;
    material
        .base
        .extension
        .set_dynamic_terrain_shadow_terminator_data(enabled);
    material.extension.params.reserved = grid; // 归还租约时清除 bit4 对应的固定代理元数据。
    material.extension.proxy_axes = axes;
}
