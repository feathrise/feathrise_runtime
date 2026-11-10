//! 洞内静水复用正式水位、R8遮罩、波浪与互动；水深读取水位下的真实承托面。
use super::super::super::{ClipmapGroundSnapshot, DynamicClipmapGroundSurface};
use super::{ClipmapWaterMaterial, ClipmapWaterSurface};
use crate::shared::large_scene::{
    DynamicWaterCpuInteraction, DynamicWaterSurface, PaintedWaterPurpose, PaintedWaterSurface,
    WaterSurfaceKind,
};
use bevy::{
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
};
use feathrise_scene_core::residency::{InstanceQuery, InstanceRes, InstanceResMut, SceneInstanceContextAppExt};
#[derive(Component)]
pub(crate) struct VolumeWaterSurface;
#[derive(Component)]
struct Source(Entity);
#[derive(Clone, PartialEq)]
struct Identity {
    ground: [u64; 7],
    window: [u64; 3], // 原生环境窗口跟随附近驻留；旧Clipmap外景继续使用内容身份。
    layers: Vec<(Entity, AssetId<ClipmapWaterMaterial>, usize, [u32; 12])>,
}
mod build;
use build::build;
mod runtime;
pub(super) use runtime::add_instance_systems;
pub(super) use runtime::install;

/// 不含视角吸附的几何身份：遮罩替换、水位、范围与最小水深改变均需重建。
fn layer_key(s: &ClipmapWaterSurface) -> [u32; 12] {
    [
        s.water_level.to_bits(),
        s.minimum_depth.to_bits(),
        s.resolution[0],
        s.resolution[1],
        s.world_min.x.to_bits(),
        s.world_min.y.to_bits(),
        s.world_max.x.to_bits(),
        s.world_max.y.to_bits(),
        s.coverage_bounds[0],
        s.coverage_bounds[1],
        s.coverage_bounds[2],
        s.coverage_bounds[3],
    ]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cave_water_preserves_large_coverage_without_vertex_or_sampling_cap() {
        let ground = ClipmapGroundSnapshot::stacked_for_test();
        let mut water = super::super::runtime::test_clipmap_water_surface();
        water.scene_id = ground.scene_id.clone();
        water.height_revision = ground.revision;
        water.world_max = Vec2::splat(4.);
        water.water_level = 5.;
        water = water.with_test_mask([513, 513], vec![255; 513 * 513]);
        let meshes = build(ground.clone(), vec![(Entity::from_bits(1), water)]).unwrap();
        assert!(
            meshes
                .iter()
                .map(|(_, m)| m.count_vertices())
                .sum::<usize>()
                > 262144
        );
        assert!(
            meshes
                .iter()
                .map(|(_, m)| m.indices().unwrap().len() / 3)
                .sum::<usize>()
                > 250000
        );
        assert!(meshes.iter().all(|(_, m)| m.count_vertices() <= 65 * 65)); // 只限制一次上传粒度。
        let mut shared = std::collections::BTreeMap::new();
        let mut seams = 0;
        for (_, mesh) in meshes {
            let bevy::mesh::VertexAttributeValues::Float32x3(positions) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
            else {
                panic!()
            };
            let bevy::mesh::VertexAttributeValues::Float32x2(depths) =
                mesh.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
            else {
                panic!()
            };
            for i in mesh.indices().unwrap().iter() {
                let key = positions[i].map(f32::to_bits);
                if let Some(old) = shared.insert(key, depths[i]) {
                    assert_eq!(old, depths[i]);
                    seams += 1;
                }
            }
        }
        assert!(seams > 1000); // 共用边界顶点的水位和洞底深度逐位相同。
    }
    #[test]
    fn cave_water_uses_floor_below_level_and_never_the_back_of_a_ceiling() {
        let ground = ClipmapGroundSnapshot::stacked_for_test();
        let mut water = super::super::runtime::test_clipmap_water_surface();
        water.scene_id = ground.scene_id.clone();
        water.height_revision = ground.revision;
        water.world_max = Vec2::splat(4.);
        water.water_level = 5.;
        let entity = Entity::from_bits(1);
        let meshes = build(ground.clone(), vec![(entity, water.clone())]).unwrap();
        assert_eq!(meshes.len(), 1);
        let bevy::mesh::VertexAttributeValues::Float32x2(depth) =
            meshes[0].1.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
        else {
            panic!()
        };
        assert!(
            meshes[0]
                .1
                .indices()
                .unwrap()
                .iter()
                .all(|i| depth[i] == [2., 1.])
        ); // 上传块可保留未引用角点，真正三角面必须全部有有效洞底。
        assert_eq!(water.surface_depth_at(&ground, Vec2::ONE), None); // 外环带不重复消费洞内互动。
        let cave = water.clone().volume_copy();
        assert_eq!(cave.surface_depth_at(&ground, Vec2::ONE), Some(3.));
        water.water_level = 10.;
        assert!(
            build(ground.clone(), vec![(entity, water.clone())])
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            water.volume_copy().surface_depth_at(&ground, Vec2::ONE),
            None
        );
    }
    #[test]
    fn shallow_cave_water_is_not_cut_by_wave_safety_depth() {
        let ground = ClipmapGroundSnapshot::stacked_for_test();
        let mut water = super::super::runtime::test_clipmap_water_surface();
        water.scene_id = ground.scene_id.clone();
        water.height_revision = ground.revision;
        water.world_max = Vec2::splat(4.);
        water.water_level = 2.1;
        water.minimum_depth = 0.8;
        let meshes = build(ground.clone(), vec![(Entity::from_bits(1), water.clone())]).unwrap();
        assert!(!meshes.is_empty(), "正浅水不能被波幅安全深度整片裁掉");
        for (_, mesh) in meshes {
            let bevy::mesh::VertexAttributeValues::Float32x2(depth) =
                mesh.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
            else {
                panic!()
            };
            assert!(mesh.indices().unwrap().iter().all(|index| depth[index] == [2., 1.]));
        }
        assert!(
            (water.volume_copy().surface_depth_at(&ground, Vec2::ONE).unwrap() - 0.1).abs()
                < 1.0e-5
        );
    }

    #[test]
    fn native_shore_triangles_retain_real_dry_vertices_and_fine_mask_coverage() {
        // 同一个真实斜坡在x=1.5穿过水位；只保留湿顶点会把岸线退到整格之外。
        let shape = avian3d::prelude::Collider::trimesh(
            vec![
                Vec3::new(0., 1.7, 0.),
                Vec3::new(4., 2.5, 0.),
                Vec3::new(0., 1.7, 4.),
                Vec3::new(4., 2.5, 4.),
            ],
            vec![[0, 2, 1], [1, 2, 3]],
        );
        let ground = ClipmapGroundSnapshot::from_density_mesh(
            feathrise_scene_content::SceneId::parse("MAP-native-shore").unwrap(),
            Entity::from_bits(7),
            1,
            [5; 32],
            Some(shape),
            Vec3::ZERO,
            Vec3::new(4., 2.5, 4.),
            1.,
            &[Vec3::new(2., 0., 2.)],
            4.,
        )
        .unwrap();
        let mut water = super::super::runtime::test_clipmap_water_surface();
        water.scene_id = ground.scene_id.clone();
        water.height_revision = ground.revision;
        water.world_max = Vec2::splat(4.);
        water.water_level = 2.;
        water.minimum_depth = 0.8;
        let entity = Entity::from_bits(1);
        let meshes = build(ground.clone(), vec![(entity, water.clone())]).unwrap();
        let mut crossing = 0;
        assert!(!meshes.is_empty());
        for (_, mesh) in meshes {
            let bevy::mesh::VertexAttributeValues::Float32x3(positions) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
            else {
                panic!()
            };
            let bevy::mesh::VertexAttributeValues::Float32x2(depth) =
                mesh.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
            else {
                panic!()
            };
            let indices = mesh.indices().unwrap().iter().collect::<Vec<_>>();
            for triangle in indices.chunks_exact(3) {
                let wet = triangle.iter().any(|index| depth[*index][0] < water.water_level);
                let dry = triangle.iter().any(|index| depth[*index][0] > water.water_level);
                assert!(wet, "全干三角面不能生成水面");
                crossing += usize::from(wet && dry);
                for &index in triangle {
                    let p = positions[index];
                    assert!(ground.volume_footprints().iter().any(|hole| hole.contains(Vec2::new(p[0], p[2]))));
                    assert_eq!(depth[index][1], 1., "缺少真实河床的角点不能参与三角面");
                    assert!((depth[index][0] - (1.7 + p[0] * 0.2)).abs() < 1.0e-5,
                        "干岸高度必须来自真实斜坡，不能伪造为水位");
                }
            }
        }
        assert!(crossing > 0, "跨零水深三角面必须交给片元按真实水深裁岸");

        // 细R8的非零纹素落在四个粗格点之间，顶点均为零也不能丢掉这片真实浅水。
        let mut mask = vec![0; 9 * 9];
        mask[3 * 9 + 3] = 255;
        let sparse = water.clone().with_test_mask([9, 9], mask);
        assert_eq!(sparse.coverage_at(Vec2::splat(1.5)), 1.);
        for p in [Vec2::ONE, Vec2::new(1., 2.), Vec2::new(2., 1.), Vec2::splat(2.)] {
            assert_eq!(sparse.coverage_at(p), 0.);
        }
        assert!(!build(ground.clone(), vec![(entity, sparse)]).unwrap().is_empty());

        water.water_level = 1.5;
        assert!(build(ground, vec![(entity, water)]).unwrap().is_empty(),
            "水位低于全部真实河床时不能用向上命中虚构水面");
    }

    #[test]
    fn mask_and_minimum_depth_are_part_of_water_geometry_identity() {
        let a = super::super::runtime::test_clipmap_water_surface();
        let mut b = a.clone();
        b.minimum_depth += 1.;
        assert_ne!(layer_key(&a), layer_key(&b));
        b = a.clone();
        b.world_min.x += 1.;
        assert_ne!(layer_key(&a), layer_key(&b));
        assert_eq!(a.mask_identity(), a.clone().mask_identity());
    }
    #[test]
    fn native_water_geometry_uses_a_bounded_nearby_window_and_real_bed_depth() {
        let shape = avian3d::prelude::Collider::trimesh(
            vec![
                Vec3::new(-5000., 2., -5000.),
                Vec3::new(5000., 2., -5000.),
                Vec3::new(-5000., 2., 5000.),
                Vec3::new(5000., 2., 5000.),
            ],
            vec![[0, 2, 1], [1, 2, 3]],
        );
        let ground = ClipmapGroundSnapshot::from_density_mesh(
            feathrise_scene_content::SceneId::parse("MAP-native-water").unwrap(),
            Entity::from_bits(7),
            1,
            [5; 32],
            Some(shape),
            Vec3::new(-5000., -2., -5000.),
            Vec3::new(5000., 2., 5000.),
            0.01,
            &[Vec3::ZERO],
            64.,
        )
        .unwrap();
        let mut water = super::super::runtime::test_clipmap_water_surface();
        water.scene_id = ground.scene_id.clone();
        water.height_revision = ground.revision;
        water.world_min = Vec2::splat(-5000.);
        water.world_max = Vec2::splat(5000.);
        water.water_level = 5.;
        let entity = Entity::from_bits(1);
        let meshes = build(ground.clone(), vec![(entity, water.clone())]).unwrap();
        assert!(!meshes.is_empty());
        assert!(meshes.len() <= 9);
        assert!(
            meshes
                .iter()
                .map(|(_, mesh)| mesh.count_vertices())
                .sum::<usize>()
                <= 9 * 65 * 65
        );
        for (_, mesh) in meshes {
            let bevy::mesh::VertexAttributeValues::Float32x2(depth) =
                mesh.attribute(Mesh::ATTRIBUTE_UV_1).unwrap()
            else {
                panic!()
            };
            assert!(
                mesh.indices()
                    .unwrap()
                    .iter()
                    .all(|index| depth[index] == [2., 1.])
            );
        }
        assert_eq!(
            water.volume_copy().surface_depth_at(&ground, Vec2::ZERO),
            Some(3.)
        );
    }
    #[test]
    fn empty_native_geometry_is_ready_and_does_not_invent_water_support() {
        let ground = ClipmapGroundSnapshot::from_density_mesh(
            feathrise_scene_content::SceneId::parse("MAP-native-empty").unwrap(),
            Entity::from_bits(7),
            1,
            [5; 32],
            None,
            Vec3::ZERO,
            Vec3::splat(4.),
            0.25,
            &[Vec3::ZERO],
            2.,
        )
        .unwrap();
        assert!(ground.ready);
        assert!(matches!(
            ground.sample_authoritative_surface(Vec2::ONE),
            crate::ClipmapGroundHit::Miss
        ));
        let mut water = super::super::runtime::test_clipmap_water_surface();
        water.scene_id = ground.scene_id.clone();
        water.height_revision = ground.revision;
        water.world_min = Vec2::ZERO;
        water.world_max = Vec2::splat(4.);
        assert!(
            build(ground, vec![(Entity::from_bits(1), water)])
                .unwrap()
                .is_empty()
        );
    }
}
