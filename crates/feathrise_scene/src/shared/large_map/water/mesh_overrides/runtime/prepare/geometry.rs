//! 覆盖匹配收口、世界父链换算以及POSITION/UV0与三角证据的分帧验证。

use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues}; // 只读原Mesh属性和逻辑索引。
use bevy::prelude::*; // Assets、Mesh、Transform层级查询与实体去重。
use feathrise_map_content::MapPlacementWaterKind; // 非水平水必须依赖Blender作者法线而不是Shader固定轴退路。

use super::super::super::{
    DynamicWaterMeshOverridePrimitive, MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
    encoded_whole_mesh_flow_style, parse_gltf_mesh_index,
}; // 复用既有作者语义、诊断解析和根级硬上限。
use super::super::height_offset::{
    TransformChainCursor, parent_translation_for_world_height_linear,
}; // 高度和几何世界米制都由分帧父链游标提供完成矩阵。
use super::super::uv_validation::{IncrementalUvValidation, UvValidationProgress}; // 顶点和抽样三角独立预算推进。
use super::hierarchy::MESH_WATER_HIERARCHY_OPS_PER_FRAME; // 父链读取与组合共享统一明确预算。
use super::work::{
    GeometryPhase, MeshWaterRootWork, MeshWaterWorkStage, PrimitiveGeometryCursor,
    PrimitiveGeometryInput, ValidatedPrimitive,
}; // 单根工作持有全部完成前游标和小型结果。

pub(super) const MESH_WATER_UV_VERTEX_BUDGET_PER_FRAME: usize = 2_048; // 65536顶点极值至少跨32帧扫描。
pub(super) const MESH_WATER_UV_TRIANGLE_BUDGET_PER_FRAME: usize = 32; // 单primitive最多128样本至少跨四次推进。

/// 逐作者覆盖核对匹配歧义，并分帧解析该Blender对象父坐标中的世界Y位移。
pub(super) fn advance_surface_finalization(
    work: &mut MeshWaterRootWork,
    local_hierarchy: &Query<(&Transform, Option<&ChildOf>)>,
    overridden: &Query<&DynamicWaterMeshOverridePrimitive>,
) -> Result<(), String> {
    if work.finalize_surface >= work.surfaces.len() {
        work.stage = MeshWaterWorkStage::Geometry; // 全部匹配值完成后才进入任何Mesh资产扫描。
        return Ok(());
    }
    let surface_index = work.finalize_surface;
    let surface = &work.surfaces[surface_index];
    if work.finalize_chain.is_none() {
        let matches = &work.matches[surface_index];
        if matches.is_empty() {
            return Err(format!(
                "未在当前placement根内匹配节点{} / 网格{}",
                surface.node_path.join("/"),
                surface.mesh_name
            ));
        }
        let object_node = matches[0].object_node;
        if matches
            .iter()
            .any(|candidate| candidate.object_node != object_node)
        {
            return Err(format!(
                "节点路径{} / 网格{}命中多个重名Blender对象，已失败关闭",
                surface.node_path.join("/"),
                surface.mesh_name
            ));
        }
        work.finalize_object_node = Some(object_node); // 完成后同一覆盖全部primitive共享位移。
        work.finalize_chain = Some(TransformChainCursor::new(work.root, object_node));
    }
    let world_from_parent = work
        .finalize_chain
        .as_mut()
        .expect("上方已建立当前覆盖父链游标")
        .advance(
            MESH_WATER_HIERARCHY_OPS_PER_FRAME,
            local_hierarchy,
            overridden,
        )?;
    let Some(world_from_parent) = world_from_parent else {
        return Ok(()); // 父链未完成时保持当前覆盖游标，不处理下一项。
    };
    let local_translation_offset =
        parent_translation_for_world_height_linear(world_from_parent, surface.height_offset)
            .map_err(|detail| {
                format!("网格{}的世界高度偏移无法换算：{detail}", surface.mesh_name)
            })?;
    for candidate in &work.matches[surface_index] {
        if !work.selected_entities.insert(candidate.entity) {
            return Err(format!(
                "网格primitive{:?}被多项水体覆盖重复命中",
                candidate.entity
            ));
        }
        work.geometry_inputs.push(PrimitiveGeometryInput {
            candidate: candidate.clone(), // 最多六十四项固定小快照。
            surface_index,
            local_translation_offset,
        });
    }
    work.finalize_surface += 1; // 下一帧才开始下一覆盖，浅层场景也不会一帧处理全部64项。
    work.finalize_object_node = None;
    work.finalize_chain = None;
    Ok(())
}

/// 每帧推进当前唯一primitive的父链、顶点前缀和均匀三角样本。
pub(super) fn advance_geometry_validation(
    work: &mut MeshWaterRootWork,
    local_hierarchy: &Query<(&Transform, Option<&ChildOf>)>,
    overridden: &Query<&DynamicWaterMeshOverridePrimitive>,
    meshes: &Assets<Mesh>,
) -> Result<(), String> {
    if work.geometry_cursor >= work.geometry_inputs.len() {
        work.stage = MeshWaterWorkStage::Materials; // 全量几何成功后才允许创建不可见临时材质。
        return Ok(());
    }
    if work.active_geometry.is_none() {
        let input = work.geometry_inputs[work.geometry_cursor].clone();
        initialize_geometry_cursor(work, input, meshes)?; // 只做O(1)资产格式和长度门。
    }
    let cursor = work
        .active_geometry
        .as_mut()
        .expect("上方已建立当前primitive几何游标");
    if matches!(cursor.phase, GeometryPhase::ResolveWorldTransform) {
        let world_from_mesh = cursor.transform_chain.advance(
            MESH_WATER_HIERARCHY_OPS_PER_FRAME,
            local_hierarchy,
            overridden,
        )?;
        let Some(world_from_mesh) = world_from_mesh else {
            return Ok(());
        };
        let source_mesh = source_mesh_for_cursor(cursor, meshes)?;
        let surface_kind = work.surfaces[cursor.input.surface_index].kind;
        let (positions, uv0, normals, indices) =
            mesh_geometry_attributes(source_mesh, &cursor.input, surface_kind)?;
        let logical_index_count = indices.map_or(positions.len(), Indices::len);
        // 每个primitive各自增量验证；已经完成的其他primitive不能耗尽后续作者内容的额度。
        let primitive_triangle_budget = MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE;
        cursor.uv_validation = Some(IncrementalUvValidation::new(
            positions.len(),
            uv0.len(),
            normals.map(<[_]>::len),
            logical_index_count,
            world_from_mesh,
            primitive_triangle_budget,
        )?);
        cursor.phase = GeometryPhase::ValidateUv;
    }
    let source_mesh = source_mesh_for_cursor(cursor, meshes)?;
    let surface_kind = work.surfaces[cursor.input.surface_index].kind;
    let (positions, uv0, normals, indices) =
        mesh_geometry_attributes(source_mesh, &cursor.input, surface_kind)?;
    let progress = cursor
        .uv_validation
        .as_mut()
        .expect("世界变换完成后必须建立UV游标")
        .advance(
            positions,
            uv0,
            normals,
            indices,
            MESH_WATER_UV_VERTEX_BUDGET_PER_FRAME,
            MESH_WATER_UV_TRIANGLE_BUDGET_PER_FRAME,
        )
        .map_err(|detail| {
            let surface = &work.surfaces[cursor.input.surface_index];
            format!(
                "网格{} primitive{:?}的UV0水体语义无效：{detail}",
                surface.mesh_name, cursor.input.candidate.entity
            )
        })?;
    let UvValidationProgress::Complete { samples } = progress else {
        return Ok(()); // 当前primitive完整保留到下一帧。
    };
    let cursor = work
        .active_geometry
        .take()
        .expect("完成UV校验的primitive游标必须存在");
    let surface = &work.surfaces[cursor.input.surface_index];
    work.sampled_triangles = work
        .sampled_triangles
        .checked_add(samples)
        .ok_or_else(|| "整网格水体UV0三角抽样计数溢出".to_owned())?;
    let base_material = cursor
        .input
        .candidate
        .original_material
        .clone()
        .ok_or_else(|| {
            format!(
                "网格{} primitive{:?}缺少Blender StandardMaterial",
                surface.mesh_name, cursor.input.candidate.entity
            )
        })?;
    work.validated.push(ValidatedPrimitive {
        candidate: cursor.input.candidate,
        base_material,
        flow_speed: surface.flow_speed,
        flow_style_encoding: encoded_whole_mesh_flow_style(surface.kind),
        local_translation_offset: cursor.input.local_translation_offset,
    });
    work.geometry_cursor += 1; // 下一帧再开始下一primitive，固定属性门也不会批量堆叠。
    Ok(())
}

/// 为一个primitive建立父链游标并在任何顶点扫描前执行O(1)格式与总额门。
fn initialize_geometry_cursor(
    work: &mut MeshWaterRootWork,
    input: PrimitiveGeometryInput,
    meshes: &Assets<Mesh>,
) -> Result<(), String> {
    let surface = &work.surfaces[input.surface_index];
    let mesh_handle = input.candidate.mesh.as_ref().ok_or_else(|| {
        format!(
            "网格{} primitive{:?}缺少Mesh3d",
            surface.mesh_name, input.candidate.entity
        )
    })?;
    let _original_transform = input.candidate.original_transform.ok_or_else(|| {
        format!(
            "网格{} primitive{:?}缺少Transform",
            surface.mesh_name, input.candidate.entity
        )
    })?;
    match mesh_handle
        .path()
        .and_then(|path| path.label())
        .and_then(parse_gltf_mesh_index)
    {
        Some(actual) if actual != surface.mesh_index => warn!(
            "整网格水体mesh_index诊断不一致：节点{} / {} 作者={} GLB={actual}；继续以路径+网格名为准",
            surface.node_path.join("/"),
            surface.mesh_name,
            surface.mesh_index,
        ),
        None => warn!(
            "整网格水体无法从Mesh标签诊断mesh_index：节点{} / {} 作者={}",
            surface.node_path.join("/"),
            surface.mesh_name,
            surface.mesh_index,
        ),
        Some(_) => {}
    }
    let source_mesh = meshes.get(mesh_handle).ok_or_else(|| {
        format!(
            "网格{} primitive{:?}的glTF Mesh资产不可用",
            surface.mesh_name, input.candidate.entity
        )
    })?;
    if source_mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err(format!(
            "网格{} primitive{:?}不是TriangleList，无法验证UV0沿程语义",
            surface.mesh_name, input.candidate.entity
        ));
    }
    let (positions, uv0, _, _) = mesh_geometry_attributes(source_mesh, &input, surface.kind)?;
    let next_inspected = work
        .inspected_vertices
        .checked_add(uv0.len())
        .ok_or_else(|| "整网格水体UV0顶点计数溢出".to_owned())?;
    debug_assert_eq!(positions.len(), uv0.len()); // helper已执行正式失败门。
    work.inspected_vertices = next_inspected; // 只累计固定长度，不在本帧扫描属性内容。
    let primitive = input.candidate.entity;
    work.active_geometry = Some(PrimitiveGeometryCursor {
        input,
        phase: GeometryPhase::ResolveWorldTransform,
        transform_chain: TransformChainCursor::new(work.root, primitive),
        uv_validation: None,
    });
    Ok(())
}

fn source_mesh_for_cursor<'a>(
    cursor: &PrimitiveGeometryCursor,
    meshes: &'a Assets<Mesh>,
) -> Result<&'a Mesh, String> {
    let handle = cursor
        .input
        .candidate
        .mesh
        .as_ref()
        .expect("初始化已确认Mesh句柄存在");
    meshes.get(handle).ok_or_else(|| {
        format!(
            "primitive{:?}的glTF Mesh资产在增量校验期间不可用",
            cursor.input.candidate.entity
        )
    })
}

fn mesh_geometry_attributes<'a>(
    source_mesh: &'a Mesh,
    input: &PrimitiveGeometryInput,
    kind: MapPlacementWaterKind,
) -> Result<
    (
        &'a [[f32; 3]],
        &'a [[f32; 2]],
        Option<&'a [[f32; 3]]>,
        Option<&'a Indices>,
    ),
    String,
> {
    let positions = match source_mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(values)) => values.as_slice(),
        _ => {
            return Err(format!(
                "primitive{:?}缺少Float32x3 POSITION",
                input.candidate.entity
            ));
        }
    };
    let uv0 = match source_mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(values)) => values.as_slice(),
        _ => {
            return Err(format!(
                "primitive{:?}缺少Float32x2 UV0",
                input.candidate.entity
            ));
        }
    };
    if positions.len() != uv0.len() {
        return Err(format!(
            "primitive{:?}的POSITION与UV0顶点数不一致",
            input.candidate.entity
        ));
    }
    let normals =
        required_mesh_normals(source_mesh, kind, positions.len(), input.candidate.entity)?;
    Ok((positions, uv0, normals, source_mesh.indices()))
}

/// 斜坡与瀑布的顶点位移必须沿作者法线工作；水平面保持对旧GLB的缺省兼容。
fn required_mesh_normals(
    source_mesh: &Mesh,
    kind: MapPlacementWaterKind,
    expected_vertices: usize,
    entity: Entity,
) -> Result<Option<&[[f32; 3]]>, String> {
    if kind == MapPlacementWaterKind::Horizontal {
        return Ok(None); // 水平面Shader已有固定Y轴退路，不把旧平面资产升级成破坏性错误。
    }
    let normals = match source_mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(values)) => values.as_slice(),
        _ => {
            return Err(format!(
                "primitive{entity:?}的斜坡/瀑布水缺少Float32x3 NORMAL"
            ));
        }
    };
    if normals.len() != expected_vertices {
        return Err(format!("primitive{entity:?}的NORMAL与POSITION顶点数不一致"));
    }
    Ok(Some(normals))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximum_vertex_scan_requires_multiple_budgeted_advances() {
        let vertex_count = MAX_MESH_WATER_UV_VERTICES_PER_ROOT;
        let positions = (0..vertex_count)
            .map(|index| [index as f32, 0.0, (index & 1) as f32])
            .collect::<Vec<_>>();
        let uv0 = (0..vertex_count)
            .map(|index| [index as f32, (index & 1) as f32])
            .collect::<Vec<_>>(); // U保持世界米制，V在合法0..1范围内具备横向跨度。
        let indices = Indices::U32(vec![0, 1, 2]); // 单三角由0->2边提供一条有效沿程米制证据。
        let mut validation = IncrementalUvValidation::new(
            vertex_count,
            vertex_count,
            None,
            indices.len(),
            Mat3A::IDENTITY,
            MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE,
        )
        .expect("合法极值网格必须建立增量游标");

        assert_eq!(
            validation
                .advance(
                    &positions,
                    &uv0,
                    None,
                    Some(&indices),
                    MESH_WATER_UV_VERTEX_BUDGET_PER_FRAME,
                    MESH_WATER_UV_TRIANGLE_BUDGET_PER_FRAME,
                )
                .expect("首帧有限前缀必须通过"),
            UvValidationProgress::Pending
        );
        assert_eq!(
            validation.vertex_cursor(),
            MESH_WATER_UV_VERTEX_BUDGET_PER_FRAME
        ); // 65536顶点绝不能在首帧完整扫描。

        let mut advances = 1usize;
        loop {
            advances += 1;
            if matches!(
                validation
                    .advance(
                        &positions,
                        &uv0,
                        None,
                        Some(&indices),
                        MESH_WATER_UV_VERTEX_BUDGET_PER_FRAME,
                        MESH_WATER_UV_TRIANGLE_BUDGET_PER_FRAME,
                    )
                    .expect("后续有限前缀必须通过"),
                UvValidationProgress::Complete { .. }
            ) {
                break;
            }
        }
        assert_eq!(
            advances,
            vertex_count.div_ceil(MESH_WATER_UV_VERTEX_BUDGET_PER_FRAME)
        ); // 生产预算下极值顶点扫描固定需要32次Update推进。
    }

    #[test]
    fn sloped_and_waterfall_require_float32_normals_but_horizontal_stays_compatible() {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            bevy::asset::RenderAssetUsages::default(),
        );
        let entity = Entity::PLACEHOLDER;
        assert!(
            required_mesh_normals(&mesh, MapPlacementWaterKind::Horizontal, 3, entity)
                .expect("水平面允许旧资产没有NORMAL")
                .is_none()
        );
        for kind in [
            MapPlacementWaterKind::Sloped,
            MapPlacementWaterKind::Waterfall,
        ] {
            assert!(required_mesh_normals(&mesh, kind, 3, entity).is_err());
        }
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 2]);
        assert!(required_mesh_normals(&mesh, MapPlacementWaterKind::Sloped, 3, entity).is_err());
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; 3]);
        assert_eq!(
            required_mesh_normals(&mesh, MapPlacementWaterKind::Waterfall, 3, entity)
                .expect("匹配长度的Float32x3 NORMAL必须通过")
                .expect("瀑布必须返回法线切片")
                .len(),
            3
        );
    }
}

#[cfg(test)]
use super::super::super::MAX_MESH_WATER_UV_VERTICES_PER_ROOT;
