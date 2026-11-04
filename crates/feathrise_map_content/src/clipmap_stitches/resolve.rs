//! 持久关系的可用性校验；移动、旋转、缩放、源替换和删除只暂停失配关系，恢复原位可重新采用。
use super::{ClipmapEdgeProfile, geometry::pair_geometry};
use crate::MapTerrainObject;
use std::{collections::BTreeSet, sync::Arc};

pub fn validate_clipmap_stitches(object: &MapTerrainObject) -> Result<(), String> {
    let mut edges = BTreeSet::new();
    let mut corners = std::collections::BTreeMap::<usize, f32>::new();
    for link in object.clipmap_stitches.iter() {
        if link.id.is_empty()
            || link.id.len() > 128
            || link.peer == object.id
            || link.edge != link.profile.edge
            || !edges.insert(link.edge)
        {
            return Err("Clipmap拼接身份、互指对象或边占用无效".into());
        }
        link.profile.validate()?;
        let ends = match link.edge {
            super::ClipmapEdge::West => [0, 2],
            super::ClipmapEdge::East => [1, 3],
            super::ClipmapEdge::South => [0, 1],
            super::ClipmapEdge::North => [2, 3],
        };
        for (corner, height) in ends.into_iter().zip([
            link.profile.samples[0],
            *link.profile.samples.last().unwrap(),
        ]) {
            if corners
                .insert(corner, height)
                .is_some_and(|old| (old - height).abs() > 0.0002)
            {
                return Err("相邻拼接边的共同角点高度冲突，请先解除原关系".into());
            }
        }
        if object.terrain.kind != crate::MapTerrainKind::SparseClipmap {
            return Err("只有Clipmap可以保存拼接关系".into());
        }
    }
    Ok(())
}

pub fn resolve_clipmap_stitches(
    object: &MapTerrainObject,
    objects: &[MapTerrainObject],
) -> Arc<[ClipmapEdgeProfile]> {
    object
        .clipmap_stitches
        .iter()
        .filter_map(|link| {
            let peer = objects.iter().find(|p| p.id == link.peer)?;
            let mate = peer.clipmap_stitches.iter().find(|m| {
                m.id == link.id
                    && m.edge == link.peer_edge
                    && m.peer == object.id
                    && m.peer_edge == link.edge
            })?;
            link.profile.validate().ok()?;
            mate.profile.validate().ok()?;
            let (reverse, _) = pair_geometry(object, link.edge, peer, link.peer_edge).ok()?;
            if object.terrain.height_path.as_deref() != Some(&link.source_path)
                || peer.terrain.height_path.as_deref() != Some(&link.peer_source_path)
                || mate.source_path != link.peer_source_path
                || mate.peer_source_path != link.source_path
                || link.profile.samples.len() != mate.profile.samples.len()
            {
                return None;
            }
            // pair_geometry 已排除倾斜姿态；水平曲线的世界Y必须一致，单独抬高或Y缩放也暂停关系。
            let count = link.profile.samples.len();
            let axis = link.edge.axis();
            let peer_axis = mate.edge.axis();
            if (link.profile.step * (count - 1) as f32
                - (object.world_max[axis] - object.world_min[axis]))
                .abs()
                > 0.0002
                || (mate.profile.step * (count - 1) as f32
                    - (peer.world_max[peer_axis] - peer.world_min[peer_axis]))
                    .abs()
                    > 0.0002
            {
                return None;
            }
            for i in 0..count {
                let j = if reverse { count - 1 - i } else { i };
                let a = object.transform.translation[1]
                    + link.profile.samples[i] * object.transform.scale[1];
                let b = peer.transform.translation[1]
                    + mate.profile.samples[j] * peer.transform.scale[1];
                if !a.is_finite() || !b.is_finite() || (a - b).abs() > 0.0002 {
                    return None;
                }
            }
            let mut profile = link.profile.clone();
            // 物理在后台补齐共同边的全部曲线节点；边界精度不随相机或对侧碰撞LOD改变。
            profile.collision_stride = 1;
            Some(profile)
        })
        .collect::<Vec<_>>()
        .into()
}
