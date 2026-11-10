//! 相邻同身份上传块归入一个隐藏水体根，共用材质和唯一互动组件。
use super::*;

pub fn stage_water_surface_chunk(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<DynamicWaterMaterial>,
    staged: &mut Vec<StagedWaterSurface>,
    build: super::super::BuiltWaterSurface,
    elapsed_seconds: f32,
) -> usize {
    let piece = stage_water_surface_assets(
        meshes,
        materials,
        build.descriptor,
        build.mesh,
        elapsed_seconds,
    );
    let triangles = piece.triangle_count;
    if let Some(owner) = staged.last_mut().filter(|s| {
        s.descriptor.id == piece.descriptor.id && s.descriptor.kind == piece.descriptor.kind
    }) {
        let root = owner.candidate_root.expect("分块水面已经创建隐藏根");
        let child = commands
            .spawn((
                crate::shared::large_scene::WaterRenderChunk,
                Mesh3d(piece.mesh.clone()),
                MeshMaterial3d(owner.material.clone()),
                Transform::IDENTITY,
                Visibility::Inherited,
                ChildOf(root),
                NoFrustumCulling,
                CameraRaycastIgnore,
                NotShadowCaster,
            ))
            .id(); // 只有根拥有水体身份和互动，不在每个子块重新发射玩家波。
        if let Some(bounds) = piece.local_aabb {
            commands.entity(child).insert(bounds);
        }
        if let Some(b) = piece.local_aabb {
            owner.local_aabb = Some(match owner.local_aabb {
                Some(a) => Aabb::from_min_max(
                    (a.center - a.half_extents)
                        .min(b.center - b.half_extents)
                        .into(),
                    (a.center + a.half_extents)
                        .max(b.center + b.half_extents)
                        .into(),
                ),
                None => b,
            });
        }
        owner.additional_meshes.push(piece.mesh.clone());
        owner.additional_entities.push(child);
        owner.triangle_count += triangles;
        materials.remove(piece.material.id());
    } else {
        let root = commands
            .spawn((
                Mesh3d(piece.mesh.clone()),
                MeshMaterial3d(piece.material.clone()),
                Transform::from_xyz(0., piece.descriptor.translation_y, 0.),
                Visibility::Hidden,
                NoFrustumCulling,
                CameraRaycastIgnore,
                NotShadowCaster,
                DespawnOnExit(feathrise_scene_core::state::ScenePhase::Active),
            ))
            .id();
        staged.push(StagedWaterSurface {
            candidate_root: Some(root),
            ..piece
        });
    }
    triangles
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::CommandQueue;
    use feathrise_scene_content::{SceneWaterRiver, SceneWaterSplinePoint};

    #[test]
    fn water_chunks_share_one_material_and_interaction_then_retire_incrementally() {
        let point = |id: &str, x| SceneWaterSplinePoint {
            id: id.into(),
            position: [x, 5., 0.],
            width: 4.,
            depth: 2.,
            flow_speed: 1.,
            outgoing: default(),
        };
        let build = crate::shared::large_scene::build_river_water_surface(&SceneWaterRiver {
            id: "chunked".into(),
            label: "分块长河".into(),
            points: vec![point("a", 0.), point("b", 20_000.)],
            source_layer_id: None,
            target_layer_id: None,
            base_color_srgba: [0.2, 0.3, 0.4, 0.7],
            perceptual_roughness: 0.06,
        })
        .unwrap();
        let pieces = crate::shared::large_scene::chunk_water_surfaces(vec![build]).unwrap();
        let count = pieces.len();
        assert!(count > 1);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        crate::shared::large_scene::install_water_retirement(&mut app);
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<DynamicWaterMaterial>::default();
        let mut staged = Vec::new();
        let mut queue = CommandQueue::default();
        for piece in pieces {
            stage_water_surface_chunk(
                &mut Commands::new(&mut queue, app.world()),
                &mut meshes,
                &mut materials,
                &mut staged,
                piece,
                0.,
            );
        }
        queue.apply(app.world_mut());
        assert_eq!(staged.len(), 1);
        assert_eq!(meshes.len(), count);
        assert_eq!(materials.len(), 1);
        let candidate = staged[0].candidate_root.unwrap();
        assert_eq!(
            *app.world().get::<Visibility>(candidate).unwrap(),
            Visibility::Hidden
        );
        assert!(
            app.world()
                .get::<DynamicWaterCpuInteraction>(candidate)
                .is_none()
        );
        let spawn = spawn_staged_water_surface(
            &mut Commands::new(&mut queue, app.world()),
            SceneId::parse("MAP-water-chunks").unwrap(),
            PaintedWaterPurpose::Runtime,
            staged.pop().unwrap(),
        );
        queue.apply(app.world_mut());
        assert_eq!(
            app.world_mut()
                .query::<&DynamicWaterCpuInteraction>()
                .iter(app.world())
                .count(),
            1
        );
        for &child in &spawn.additional_entities {
            assert_eq!(
                app.world().get::<ChildOf>(child).unwrap().parent(),
                candidate
            );
            assert_eq!(
                app.world()
                    .get::<MeshMaterial3d<DynamicWaterMaterial>>(child)
                    .unwrap()
                    .0,
                spawn.material
            );
            assert!(
                app.world()
                    .get::<crate::shared::large_scene::WaterRenderChunk>(child)
                    .is_some()
            );
        }
        spawn.retire(&mut Commands::new(&mut queue, app.world()));
        queue.apply(app.world_mut());
        assert!(
            app.world()
                .get::<DynamicWaterCpuInteraction>(candidate)
                .is_none()
        );
        let garbage = app.world().get::<ChildOf>(candidate).unwrap().parent();
        assert_eq!(
            *app.world().get::<Visibility>(garbage).unwrap(),
            Visibility::Hidden
        );
        app.insert_resource(meshes).insert_resource(materials);
        for remaining in (0..count).rev() {
            app.update();
            assert_eq!(app.world().resource::<Assets<Mesh>>().len(), remaining);
        }
        assert!(app.world().get_entity(candidate).is_err());
        assert_eq!(
            app.world().resource::<Assets<DynamicWaterMaterial>>().len(),
            0
        );
    }
}
