//! 在后台用真实地表采样每个源节点，按 GLB 的父子顺序换回相对位置。

use super::{Node, SourceMeshHeight, Surface, TerrainObjectRaySnapshot};
use crate::ClipmapGroundHit;
use bevy::prelude::*;

pub(super) fn positions(
    root: Mat4,
    nodes: &[Node],
    sources: &[Surface],
) -> Option<Vec<(Entity, Vec3)>> {
    let surfaces: Vec<_> = sources
        .iter()
        .flatten()
        .filter_map(|(surface, _)| {
            surface
                .mesh_bounds()
                .map(|(min, max)| Ground { surface, min, max })
        })
        .collect();
    resolve_positions(root, nodes, &surfaces)
}

fn resolve_positions(
    root: Mat4,
    nodes: &[Node],
    surfaces: &[Ground<'_>],
) -> Option<Vec<(Entity, Vec3)>> {
    let mut worlds = Vec::with_capacity(nodes.len());
    let mut translations = Vec::new();
    let mut pending = false;
    for node in nodes {
        let parent = node.parent.map_or(root, |parent| worlds[parent]);
        let original = parent.transform_point3(node.local.translation);
        let offset = match node.height {
            Some(SourceMeshHeight::FollowTerrain { offset }) => Some(offset),
            Some(SourceMeshHeight::SnapToGround) => Some(0.0),
            Some(SourceMeshHeight::FixedHeight) | None => None,
        };
        let mut local = node.local;
        if let Some(offset) = offset {
            match ground_at(original, surfaces) {
                ClipmapGroundHit::Hit(hit) => {
                    // 世界 Y 是落点方向；父节点任意旋转或缩放时，局部三个平移分量都可能改变。
                    let translation = parent.inverse().transform_point3(Vec3::new(
                        original.x,
                        hit.y + offset,
                        original.z,
                    ));
                    if translation.is_finite() && translation != local.translation {
                        local.translation = translation;
                        translations.push((node.entity, translation));
                    }
                }
                ClipmapGroundHit::WaitingForHeight => pending = true,
                ClipmapGroundHit::Miss | ClipmapGroundHit::Failed => {}
            }
        }
        worlds.push(parent * local.to_matrix());
    }
    (!pending).then_some(translations)
}

struct Ground<'a> {
    surface: &'a TerrainObjectRaySnapshot,
    min: Vec3,
    max: Vec3,
}

fn ground_at(original: Vec3, surfaces: &[Ground<'_>]) -> ClipmapGroundHit {
    let mut closest_below: Option<Vec3> = None;
    let mut closest_above: Option<Vec3> = None;
    let mut pending = false;
    for ground in surfaces.iter().filter(|ground| {
        ground.min.xz().cmple(original.xz()).all() && ground.max.xz().cmpge(original.xz()).all()
    }) {
        // 留出浮点求交余量，零高度的薄地表也有有效的射线区间。
        let margin = ground.max.y.abs().max(ground.min.y.abs()).max(1.0) * (8.0 * f32::EPSILON);
        let top = ground.max.y + margin;
        let bottom = ground.min.y - margin;
        let from = (original.y + margin).min(top);
        let ray = |y| {
            ground
                .surface
                .support_below_status(original.with_y(y), y - bottom)
        };
        let mut hit = if from > bottom {
            ray(from)
        } else {
            ClipmapGroundHit::Miss
        };
        if hit == ClipmapGroundHit::Miss && top > from {
            // 新放置原点可能在整个地表下方，允许在真实表面上落点。
            hit = ray(top);
        }
        match hit {
            ClipmapGroundHit::Hit(point) if point.y <= original.y + margin => {
                if closest_below.is_none_or(|old| point.y > old.y) {
                    closest_below = Some(point);
                }
            }
            ClipmapGroundHit::Hit(point) => {
                if closest_above.is_none_or(|old| point.y < old.y) {
                    closest_above = Some(point);
                }
            }
            ClipmapGroundHit::WaitingForHeight => pending = true,
            ClipmapGroundHit::Miss | ClipmapGroundHit::Failed => {}
        }
    }
    if pending {
        return ClipmapGroundHit::WaitingForHeight;
    }
    closest_below
        .or(closest_above)
        .map_or(ClipmapGroundHit::Miss, ClipmapGroundHit::Hit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::prelude::Collider;
    use feathrise_scene_core::residency::SceneSourceFrame;

    fn source() -> TerrainObjectRaySnapshot {
        let vertices = vec![
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, 2.0, 8.0),
            Vec3::new(8.0, 6.0, 0.0),
        ];
        TerrainObjectRaySnapshot::Mesh(
            Collider::trimesh(vertices, vec![[0, 1, 2]]),
            SceneSourceFrame::default(),
        )
    }

    #[test]
    fn references_share_local_authoring_but_resolve_their_own_terrain_height() {
        let surface = source();
        let (min, max) = surface.mesh_bounds().unwrap();
        let ground = [Ground {
            surface: &surface,
            min,
            max,
        }];
        let entity = Entity::from_bits(1);
        let local = Transform::from_xyz(0.0, 0.0, 1.0);
        let node = Node {
            entity,
            local,
            parent: None,
            height: Some(SourceMeshHeight::FollowTerrain { offset: 1.0 }),
        };
        let solve = |x| {
            resolve_positions(
                Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
                &[node],
                &ground,
            )
            .unwrap()
        };
        assert!((solve(2.0)[0].1.y - 4.0).abs() < 1e-4);
        assert!((solve(4.0)[0].1.y - 5.0).abs() < 1e-4);
        assert!(solve(12.0).is_empty(), "无本地承托面时保留原姿态");
        assert!(
            resolve_positions(Mat4::IDENTITY, &[node], &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn nested_snap_is_world_vertical_and_fixed_pose_inherits_its_parent() {
        let surface = source();
        let (min, max) = surface.mesh_bounds().unwrap();
        let ground = [Ground {
            surface: &surface,
            min,
            max,
        }];
        let parent = Transform::from_xyz(2.0, 8.0, 1.0)
            .with_rotation(Quat::from_rotation_z(0.5))
            .with_scale(Vec3::new(2.0, 1.0, 0.5));
        let node = |id, local, parent, height| Node {
            entity: Entity::from_bits(id),
            local,
            parent,
            height,
        };
        let nodes = [
            node(1, parent, None, None),
            node(
                2,
                Transform::IDENTITY,
                Some(0),
                Some(SourceMeshHeight::SnapToGround),
            ),
            node(
                3,
                Transform::from_xyz(0.0, 1.0, 0.0),
                Some(1),
                Some(SourceMeshHeight::FixedHeight),
            ),
        ];
        let translations = resolve_positions(Mat4::IDENTITY, &nodes, &ground).unwrap();
        assert_eq!(translations.len(), 1);
        assert_eq!(translations[0].0, nodes[1].entity);
        let snapped = parent.transform_point(translations[0].1);
        assert!(snapped.abs_diff_eq(Vec3::new(2.0, 3.0, 1.0), 1e-4));
    }

    #[test]
    fn zero_height_surface_accepts_nodes_above_or_below() {
        let collider = Collider::trimesh(
            vec![Vec3::ZERO, Vec3::Z * 4.0, Vec3::X * 4.0],
            vec![[0, 1, 2]],
        );
        let surface = TerrainObjectRaySnapshot::Mesh(collider, SceneSourceFrame::default());
        let (min, max) = surface.mesh_bounds().unwrap();
        let ground = [Ground {
            surface: &surface,
            min,
            max,
        }];
        for y in [-10.0, 10.0] {
            assert!(
                matches!(ground_at(Vec3::new(1.0, y, 1.0), &ground), ClipmapGroundHit::Hit(point) if point.y.abs() < 1e-5)
            );
        }
    }
}
