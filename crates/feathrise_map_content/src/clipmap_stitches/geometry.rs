//! 完整边配对和吸附姿态；用变换后的真实格距检查二次幂比例，支持相对90度旋转。
use super::{CLIPMAP_STITCH_SEGMENTS, ClipmapEdge, ClipmapEdgeProfile, ClipmapStitch};
use crate::{MapTerrainKind, MapTerrainObject, TerrainObjectTransform};
use bevy::prelude::*;
use std::sync::Arc;

pub fn clipmap_edge_geometry(
    object: &MapTerrainObject,
    edge: ClipmapEdge,
) -> Result<(Vec3, Vec3, Vec3, f32), String> {
    if object.terrain.kind != MapTerrainKind::SparseClipmap || !object.enabled {
        return Err("拼接只支持启用的矩形Clipmap".into());
    }
    object.transform.validate()?;
    // 拼接曲线仍以世界竖直高度表示；倾斜网格保留关系记录但暂停采用，恢复水平可复用。
    let up = object.transform.rotation() * Vec3::Y;
    if up.dot(Vec3::Y) < 0.999999 || up.xz().length_squared() > 0.000000000001 {
        return Err("倾斜Clipmap暂不支持边缘拼接，请先恢复水平姿态".into());
    }
    let min = Vec2::from_array(object.world_min);
    let max = Vec2::from_array(object.world_max);
    let (a, b, normal) = match edge {
        ClipmapEdge::West => (min, Vec2::new(min.x, max.y), Vec3::NEG_X),
        ClipmapEdge::East => (Vec2::new(max.x, min.y), max, Vec3::X),
        ClipmapEdge::South => (min, Vec2::new(max.x, min.y), Vec3::NEG_Z),
        ClipmapEdge::North => (Vec2::new(min.x, max.y), max, Vec3::Z),
    };
    let matrix = object.transform.as_transform().to_matrix();
    let point = |p: Vec2| matrix.transform_point3(Vec3::new(p.x, 0.0, p.y));
    let axis = edge.axis();
    let cells = object.terrain.resolution[axis]
        .checked_sub(1)
        .filter(|v| *v > 0)
        .ok_or("Clipmap边缺少有效分辨率")?;
    let (a, b) = (point(a), point(b));
    Ok((
        a,
        b,
        matrix.transform_vector3(normal).normalize(),
        a.distance(b) / cells as f32,
    ))
}

pub(super) fn dyadic_ratio(a: f32, b: f32) -> bool {
    let ratio = a.max(b) / a.min(b);
    ratio.is_finite() && a > 0.0 && b > 0.0 && (ratio.log2() - ratio.log2().round()).abs() <= 0.0001
}

pub(super) fn pair_geometry(
    a: &MapTerrainObject,
    ae: ClipmapEdge,
    b: &MapTerrainObject,
    be: ClipmapEdge,
) -> Result<(bool, f32), String> {
    if a.id == b.id {
        return Err("网格不能与自身拼接".into());
    }
    let (a0, a1, an, astep) = clipmap_edge_geometry(a, ae)?;
    let (b0, b1, bn, bstep) = clipmap_edge_geometry(b, be)?;
    if !dyadic_ratio(astep, bstep) {
        return Err("两侧实际世界格距必须为二次幂比例".into());
    }
    let tolerance = (a0.distance(a1) * 0.000001).max(0.0002);
    let reverse = a0.xz().distance(b1.xz()) <= tolerance && a1.xz().distance(b0.xz()) <= tolerance;
    let forward = a0.xz().distance(b0.xz()) <= tolerance && a1.xz().distance(b1.xz()) <= tolerance;
    if (!reverse && !forward) || an.dot(bn) > -0.99999 {
        return Err("完整边未重合或两网格位于边界同侧".into());
    }
    Ok((reverse, astep.max(bstep)))
}

/// 仅吸附距离两米内的完整边；先取相对90度朝向，再计算平移，不重采样源网格。
pub fn nearest_clipmap_stitch_pose(
    moving: &MapTerrainObject,
    peers: &[MapTerrainObject],
) -> Result<(usize, ClipmapEdge, ClipmapEdge, TerrainObjectTransform), String> {
    clipmap_edge_geometry(moving, ClipmapEdge::West)?;
    let mut best: Option<(f32, usize, ClipmapEdge, ClipmapEdge, TerrainObjectTransform)> = None;
    for (index, peer) in peers.iter().enumerate().filter(|(_, p)| {
        p.id != moving.id && p.enabled && p.terrain.kind == MapTerrainKind::SparseClipmap
    }) {
        let mut candidate = moving.clone();
        candidate.transform.yaw_degrees = peer.transform.yaw_degrees
            + ((moving.transform.yaw_degrees - peer.transform.yaw_degrees) / 90.0).round() * 90.0;
        for ae in ClipmapEdge::ALL {
            for be in ClipmapEdge::ALL {
                if moving.clipmap_stitches.iter().any(|s| s.edge == ae)
                    || peer.clipmap_stitches.iter().any(|s| s.edge == be)
                {
                    continue;
                }
                let Ok((a0, a1, an, astep)) = clipmap_edge_geometry(&candidate, ae) else {
                    continue;
                };
                let Ok((b0, b1, bn, bstep)) = clipmap_edge_geometry(peer, be) else {
                    continue;
                };
                if an.dot(bn) > -0.99999
                    || !dyadic_ratio(astep, bstep)
                    || (a0.distance(a1) - b0.distance(b1)).abs()
                        > 0.0002_f32.max(b0.distance(b1) * 0.000001)
                {
                    continue;
                }
                let target = if (a1 - a0).dot(b1 - b0) > 0.0 { b0 } else { b1 };
                let delta = target.xz() - a0.xz();
                let score = delta.length();
                if score > 2.0 || best.as_ref().is_some_and(|v| v.0 <= score) {
                    continue;
                }
                let mut pose = candidate.transform;
                pose.translation[0] += delta.x;
                pose.translation[2] += delta.y;
                best = Some((score, index, ae, be, pose));
            }
        }
    }
    best.map(|(_, i, a, b, p)| (i, a, b, p))
        .ok_or("两米内未找到等长完整边；请检查格距比例、朝向和边长".into())
}

/// reference提供边界高度；IO方只需按返回格距采样这一条边，而不是加载整图。
pub fn clipmap_stitch_pair(
    a: &MapTerrainObject,
    ae: ClipmapEdge,
    b: &MapTerrainObject,
    be: ClipmapEdge,
    id: String,
    mut reference_height: impl FnMut(f32) -> Result<f32, String>,
) -> Result<(ClipmapStitch, ClipmapStitch), String> {
    let (reverse, world_step) = pair_geometry(a, ae, b, be)?;
    let (a0, a1, _, _) = clipmap_edge_geometry(a, ae)?;
    let length = a0.distance(a1);
    // 统一为二次幂段数，反向边和任意长度尾端也有完全相同的格点相位。
    let count = ((length / world_step).ceil() as usize)
        .max(1)
        .next_power_of_two()
        .min(CLIPMAP_STITCH_SEGMENTS);
    let step = length / count as f32;
    let mut ah = Vec::with_capacity(count + 1);
    let mut bh = Vec::with_capacity(count + 1);
    for i in 0..=count {
        let distance = (i as f32 * step).min(length);
        let source_b = reference_height(if reverse { length - distance } else { distance })?;
        let world_y = b.transform.translation[1] + source_b * b.transform.scale[1];
        ah.push((world_y - a.transform.translation[1]) / a.transform.scale[1]);
        bh.push(source_b);
    }
    // 反向配对统一按各自源轴递增保存。
    if reverse {
        bh.reverse();
    }
    let source = |o: &MapTerrainObject| {
        o.terrain
            .height_path
            .clone()
            .ok_or("Clipmap缺少高度源".to_string())
    };
    let make = |o: &MapTerrainObject,
                e: ClipmapEdge,
                peer: &MapTerrainObject,
                pe: ClipmapEdge,
                values: Vec<f32>|
     -> Result<ClipmapStitch, String> {
        let axis: usize = e.axis();
        let scale = if axis == 0 {
            o.transform.scale[0]
        } else {
            o.transform.scale[2]
        };
        let normal_scale = o.transform.scale[if axis == 0 { 2 } else { 0 }];
        Ok(ClipmapStitch {
            id: id.clone(),
            edge: e,
            peer: peer.id.clone(),
            peer_edge: pe,
            source_path: source(o)?,
            peer_source_path: source(peer)?,
            profile: ClipmapEdgeProfile {
                edge: e,
                step: step / scale,
                blend_width: (step * 4.0 / normal_scale)
                    .min((o.world_max[1 - axis] - o.world_min[1 - axis]) * 0.25),
                collision_stride: 1,
                samples: Arc::from(values),
            },
        })
    };
    Ok((make(a, ae, b, be, ah)?, make(b, be, a, ae, bh)?))
}
