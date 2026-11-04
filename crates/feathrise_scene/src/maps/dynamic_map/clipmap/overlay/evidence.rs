//! 验收只读真实覆盖实体及其已安装材质；输入笔刷存在不能充当实际渲染绑定完成。

use super::*;

/// 固定大小的场景输出证据；仍须配合真实窗口截图核对最终像素。
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ClipmapBrushOverlayEvidence {
    pub entity: u64,
    pub source_entity: Option<u64>,
    pub parent: Option<u64>,
    /// 实体的材质、网格、局部变换和父级均与当前正式地形配对，且高度绑定完整相同。
    pub bound: bool,
    /// 已绑定、刷圈非空且通过真实层/层级/视图可见性；不声明GPU片元已经出现在截图中。
    pub visible: bool,
    pub render_layer: bool,
    pub hierarchy_visible: bool,
    pub view_visible: bool,
    pub brush: [f32; 4],
    pub color: [f32; 4],
    pub missing_pages: [[u32; 4]; 16],
    pub source_to_world: [[f32; 4]; 4],
}

/// 不读取DynamicClipmapVegetationOverlay输入，不创建实体、不授予高度或可见性信用。
/// 此接口没有逐帧系统，普通工程无需启用可写runtime_test_support夹具特性。
pub fn clipmap_brush_overlay_evidence(world: &World) -> Option<ClipmapBrushOverlayEvidence> {
    let state = world.get_resource::<OverlayState>()?;
    let entity = state.entity?;
    world.get::<ClipmapOverlayEntity>(entity)?;
    let handle = world.get::<MeshMaterial3d<OverlayMaterial>>(entity)?;
    let material = world.get_resource::<Assets<OverlayMaterial>>()?.get(&handle.0)?;
    let target = world.get_resource::<crate::TerrainObjectAuthoringTarget>();
    let parent = target.and_then(|target| target.instance());
    let source_entity = if let Some(parent) = parent {
        world.get_resource::<InstanceResourceTable<super::super::super::sparse_runtime::SparseClipmapRuntime>>()?
            .values.get(&parent)?.display
    } else {
        world.get_resource::<super::super::super::sparse_runtime::SparseClipmapRuntime>()?.display
    };
    let source_height = source_entity.and_then(|source| {
        world.get::<DynamicSparseClipmapPrimitive>(source)?;
        world.get::<MeshMaterial3d<SparseClipmapSurfaceMaterial>>(source)
            .and_then(|handle| world.get_resource::<Assets<SparseClipmapSurfaceMaterial>>()?.get(&handle.0))
            .map(|material| &material.extension)
            .or_else(|| world.get::<MeshMaterial3d<ClipmapHeightMaterial>>(source)
                .and_then(|handle| world.get_resource::<Assets<ClipmapHeightMaterial>>()?.get(&handle.0))
                .map(|material| &material.extension))
    });
    let actual_parent = world.get::<ChildOf>(entity).map(ChildOf::parent);
    let bound = state.material.as_ref() == Some(&handle.0)
        && actual_parent == parent
        && source_entity.is_some_and(|source| {
            world.get::<Mesh3d>(source).is_some_and(|mesh| world.get::<Mesh3d>(entity) == Some(mesh))
                && world.get::<Transform>(source).is_some_and(|pose| world.get::<Transform>(entity) == Some(pose))
        })
        && source_height.is_some_and(|height| material.base.extension.same_height_binding(height));
    let render_layer = world.get::<RenderLayers>(entity).is_some_and(|layers|
        layers.intersects(&RenderLayers::layer(CLIPMAP_VEGETATION_OVERLAY_LAYER)));
    let hierarchy_visible = world.get::<Visibility>(entity).is_some_and(|visibility| *visibility != Visibility::Hidden)
        && world.get::<InheritedVisibility>(entity).is_some_and(|visibility| visibility.get());
    let view_visible = world.get::<ViewVisibility>(entity).is_some_and(|visibility| visibility.get());
    let params = &material.extension.params;
    Some(ClipmapBrushOverlayEvidence {
        entity: entity.to_bits(), source_entity: source_entity.map(Entity::to_bits),
        parent: actual_parent.map(Entity::to_bits), bound,
        visible: bound && render_layer && hierarchy_visible && view_visible
            && params.brush.is_finite() && params.brush.z > 0.0 && params.color.w > 0.0,
        render_layer, hierarchy_visible, view_visible,
        brush: params.brush.to_array(), color: params.color.to_array(),
        missing_pages: params.missing_pages.map(|page| page.to_array()),
        source_to_world: material.base.extension.params.source_to_world.to_cols_array_2d(),
    })
}
