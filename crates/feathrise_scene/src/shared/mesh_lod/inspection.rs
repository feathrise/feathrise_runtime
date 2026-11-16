//! 按所选根读取已安装的显示网格和原始网格。只读资源用于编辑器显示差异和比较。
use super::{components::{MeshLodAuxiliary, StaticMeshChunkLodStatus}, resources::*};
use bevy::{mesh::PrimitiveTopology, prelude::*};

/// 只在明确打开预览时读取一棵有限子树。超出单模型预算时不显示部分合计。
const MAX_INSPECT_NODES: usize = 256;

pub(super) fn sample(
    world: &World,
    runtime: &MeshLodRuntime,
    request: MeshLodInspectionRequest,
    enabled: bool,
) -> Option<MeshLodInspectionSample> {
    let meshes = world.resource::<Assets<Mesh>>();
    let mut visit = vec![request.root];
    let mut result = MeshLodInspectionSample::default();
    let mut found_mesh = false;
    let mut visited = 0;
    while let Some(entity) = visit.pop() {
        visited += 1;
        let node = world.get_entity(entity).ok()?;
        if node.contains::<MeshLodAuxiliary>() || node.get::<Visibility>() == Some(&Visibility::Hidden)
            || (entity != request.root && node.contains::<crate::ScenePlacementInstance>())
        {
            continue;
        }
        if let Some(children) = node.get::<Children>() {
            if visited + visit.len() + children.len() > MAX_INSPECT_NODES {
                return None;
            }
            visit.extend(children.iter());
        }
        let Some(visual) = node.get::<Mesh3d>() else { continue };
        let mesh = meshes.get(&visual.0)?;
        if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
            continue;
        }
        found_mesh = true;
        let target = runtime.targets.get(&entity).filter(|target| target.owner == request.root);
        result.displayed_triangles += triangle_count(mesh)?;
        if let Some(target) = target {
            result.original_triangles += triangle_count(meshes.get(&target.original)?)?;
            if visual.0 != target.original
                && let Some(chunks) = node.get::<StaticMeshChunkLodStatus>()
                && super::chunks::is_current(target, &visual.0)
            {
                for &level in &chunks.levels {
                    *result.levels.get_mut(level)? = true;
                }
            } else {
                let actual_level = if visual.0 == target.original {
                    0
                } else {
                    runtime.cache.get(&target.key)?.levels.iter()
                        .position(|level| level.mesh == visual.0)? + 1
                };
                *result.levels.get_mut(actual_level)? = true;
            }
            let cache_pending = runtime.requested.contains(&target.key)
                || runtime.upload.as_ref().is_some_and(|upload| upload.key == target.key);
            result.pending |= enabled && !request.original && (cache_pending
                || (!target.chunk_desired.is_empty()
                    && target.chunk_selection.as_ref().is_none_or(|value| value.levels != target.chunk_desired)
                    && target.chunk_retry_credit.is_none()));
        } else {
            result.original_triangles += triangle_count(mesh)?;
            result.levels[0] = true;
        }
    }
    found_mesh.then_some(result)
}

fn triangle_count(mesh: &Mesh) -> Option<usize> {
    let count = if let Some(indices) = mesh.try_indices_option().ok()? {
        indices.len()
    } else {
        mesh.try_attribute_option(Mesh::ATTRIBUTE_POSITION).ok()??.len()
    };
    Some(count / 3)
}
