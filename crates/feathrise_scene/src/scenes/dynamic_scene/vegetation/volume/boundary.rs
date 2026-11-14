//! 轻量同步两种草窗的跨界输入；只交换完整资产句柄，不复制密度或重新散布。
use super::{State, VolumeVegetationLayer, VolumeWindow};
use bevy::prelude::*;
use feathrise_scene_core::residency::{InstanceRes,InstanceQuery};
use feathrise_environment::vegetation::blade::{
    GpuBladeClipmapHeight, GpuBladeDensityMap, GpuBladeVolumeSource,
};

/// 父格的密度取样点与实际草根可以落在边界两侧，两种窗口必须读到同代输入。
/// 几何失效时撤销室外的旧三维查询源；显示LOD/字段换代等待期间继续用上一完整源。
pub(super) fn sync_inputs(
    mut commands: Commands,
    state: InstanceRes<State>,
    vegetation: InstanceRes<super::super::DynamicVegetationState>,
    exterior: InstanceQuery<
        (
            Entity,
            &VolumeVegetationLayer,
            &ChildOf,
            &GpuBladeDensityMap,
            Option<&GpuBladeClipmapHeight>,
            Option<&GpuBladeVolumeSource>,
        ),
        Without<VolumeWindow>,
    >,
    volume: InstanceQuery<
        (
            Entity,
            &VolumeVegetationLayer,
            Option<&GpuBladeDensityMap>,
            Option<&GpuBladeClipmapHeight>,
        ),
        With<VolumeWindow>,
    >,
    changed_inputs: InstanceQuery<
        (),
        (
            Without<VolumeWindow>,
            With<VolumeVegetationLayer>,
            Or<(
                Changed<GpuBladeDensityMap>,
                Changed<GpuBladeClipmapHeight>,
                Changed<ChildOf>,
            )>,
        ),
    >,
    added_windows: InstanceQuery<(), Added<VolumeWindow>>,
    mut removed_heights: RemovedComponents<GpuBladeClipmapHeight>,
    mut applied: Local<Option<(u64, bool, Option<Entity>)>>,
) {
    let identity = (state.boundary_revision, state.valid, vegetation.active_root);
    let height_removed = removed_heights.read().count() != 0;
    if *applied == Some(identity) && changed_inputs.is_empty() && added_windows.is_empty() && !height_removed {
        return;
    }
    *applied = Some(identity);
    for (entity, layer, parent, _, _, bound) in &exterior {
        let source = (state.valid && Some(parent.parent()) == vegetation.active_root)
            .then(|| state.blade_sources.get(&layer.0))
            .flatten();
        if source != bound.map(|b| &b.0) {
            if let Some(source) = source {
                commands
                    .entity(entity)
                    .insert(GpuBladeVolumeSource(source.clone()));
            } else {
                commands.entity(entity).remove::<GpuBladeVolumeSource>();
            }
        }
    }
    for (entity, layer, density, height) in &volume {
        // 原生体素的隐藏模板只有密度，没有二维高度；高度不能成为三维草窗取输入的资格门。
        let Some((_, _, _, next_density, next_height, _)) =
            exterior.iter().find(|(_, id, parent, _, _, _)| {
                id.0 == layer.0 && Some(parent.parent()) == vegetation.active_root
            })
        else {
            continue;
        };
        if density != Some(next_density) {
            commands.entity(entity).insert(next_density.clone());
        }
        if height != next_height {
            if let Some(next_height) = next_height {
                commands.entity(entity).insert(next_height.clone());
            } else {
                commands.entity(entity).remove::<GpuBladeClipmapHeight>();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundary_windows_share_complete_sources_and_follow_input_revisions() {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<super::super::super::DynamicVegetationState>()
            .add_systems(Update, sync_inputs);
        let outdoor_root = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<super::super::super::DynamicVegetationState>()
            .active_root = Some(outdoor_root);
        let source = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        let density = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        app.insert_resource(State {
            valid: true,
            blade_sources: std::collections::BTreeMap::from([("grass".into(), source.clone())]),
            ..default()
        });
        let mut height = GpuBladeClipmapHeight {stitches: Default::default(),
            holes: [Vec4::ZERO; 32],
            hole_count: 1,
            atlas: Handle::default(),
            page_table: Handle::default(),
            world_min: Vec2::ZERO,
            world_max: Vec2::splat(8.),
            cell_size: Vec2::splat(0.25),
            logical_cells: UVec2::splat(32),
            max_mip: 0,
            fine_center: Vec2::splat(4.),
            bands: [default(); 16],
            band_count: 1,
            height_min: 0.,
            height_max: 8.,
            revision: [1, 2, 3],
            missing_pages: [UVec4::ZERO; 16],
            ready: true,
        };
        height.holes[0] = Vec4::new(0.07, 0., 4., 4.);
        let exterior = app
            .world_mut()
            .spawn((
                VolumeVegetationLayer("grass".into()),
                ChildOf(outdoor_root),
                GpuBladeDensityMap::new(density.clone()),
                height.clone(),
            ))
            .id();
        let windows = [0, 1, 2, 3].map(|_| {
            app.world_mut()
                .spawn((VolumeWindow, VolumeVegetationLayer("grass".into())))
                .id()
        });
        let retired_root = app.world_mut().spawn_empty().id();
        let retired = app
            .world_mut()
            .spawn((
                VolumeVegetationLayer("grass".into()),
                ChildOf(retired_root),
                GpuBladeDensityMap::new(density.clone()),
                height.clone(),
                GpuBladeVolumeSource(source.clone()),
            ))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<GpuBladeVolumeSource>(exterior).unwrap().0,
            source
        );
        assert!(
            app.world().get::<GpuBladeVolumeSource>(retired).is_none(),
            "退役根不能接管当前三维密度"
        );
        for window in windows {
            assert_eq!(
                app.world().get::<GpuBladeDensityMap>(window),
                Some(&GpuBladeDensityMap::new(density.clone()))
            );
            assert_eq!(
                app.world().get::<GpuBladeClipmapHeight>(window),
                Some(&height)
            );
        }
        let new_density = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());
        height.revision[2] += 1;
        height.missing_pages[0].x = 1;
        app.world_mut()
            .entity_mut(exterior)
            .insert((GpuBladeDensityMap::new(new_density.clone()), height.clone()));
        app.update();
        for window in windows {
            assert_eq!(
                app.world().get::<GpuBladeDensityMap>(window),
                Some(&GpuBladeDensityMap::new(new_density.clone()))
            );
            assert_eq!(
                app.world().get::<GpuBladeClipmapHeight>(window),
                Some(&height)
            );
        }
        // 从Clipmap切为原生模板时不能留下旧二维高度，让三维源意外等待已退役的页图。
        app.world_mut().entity_mut(exterior).remove::<GpuBladeClipmapHeight>();
        app.update();
        for window in windows {
            assert!(app.world().get::<GpuBladeClipmapHeight>(window).is_none());
            assert_eq!(app.world().get::<GpuBladeDensityMap>(window),
                Some(&GpuBladeDensityMap::new(new_density.clone())));
        }
        app.world_mut().resource_mut::<State>().valid = false;
        app.update();
        assert!(app.world().get::<GpuBladeVolumeSource>(exterior).is_none());
        app.world_mut().resource_mut::<State>().valid = true;
        app.update();
        assert_eq!(
            app.world().get::<GpuBladeVolumeSource>(exterior).unwrap().0,
            source,
            "恢复仍应复用完整源"
        );
    }

    #[test]
    fn native_voxel_templates_without_height_publish_real_density_to_volume_windows() {
        use feathrise_environment::vegetation::blade::{GpuBladePatch, GpuBladeSourceRevision};
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<super::super::super::DynamicVegetationState>()
            .add_systems(Update, sync_inputs);
        let outdoor_root = app.world_mut().spawn_empty().id();
        app.world_mut().resource_mut::<super::super::super::DynamicVegetationState>()
            .active_root = Some(outdoor_root);
        let density = app.world_mut().resource_mut::<Assets<Image>>().add(Image::default());
        let source = app.world_mut().resource_mut::<Assets<Image>>().add(Image::default());
        app.insert_resource(State { valid: true,
            blade_sources: std::collections::BTreeMap::from([("grass".into(), source.clone())]),
            ..default() });
        let template = app.world_mut().spawn((
            VolumeVegetationLayer("grass".into()), ChildOf(outdoor_root), Visibility::Hidden,
            GpuBladeDensityMap::new(density.clone()), GpuBladeSourceRevision(19),
        )).id();
        let patch = GpuBladePatch::square(Vec3::ZERO, 8., 33, 7)
            .with_density_field(Vec2::ZERO, Vec2::splat(8.))
            .with_volume_cell_source(25, 0.25);
        assert!(patch.uses_density_field(), "三维候选仍必须等待真实密度纹理");
        let window = app.world_mut().spawn((VolumeWindow, VolumeVegetationLayer("grass".into()),
            patch, GpuBladeSourceRevision(3))).id();
        app.update();
        assert_eq!(app.world().get::<GpuBladeDensityMap>(window),
            Some(&GpuBladeDensityMap::new(density.clone())), "没有二维高度也要绑定真实R8，否则RenderApp永久清零候选");
        assert!(app.world().get::<GpuBladeClipmapHeight>(window).is_none());
        assert_eq!(app.world().get::<GpuBladeVolumeSource>(template).unwrap().0, source);
        assert_eq!(app.world().get::<GpuBladeSourceRevision>(window).unwrap().0, 3,
            "模板新R8修订不能冒充三维worker完整采用代");
        let next_density = app.world_mut().resource_mut::<Assets<Image>>().add(Image::default());
        app.world_mut().entity_mut(template).insert(GpuBladeDensityMap::new(next_density.clone()));
        app.update();
        assert_eq!(app.world().get::<GpuBladeDensityMap>(window),
            Some(&GpuBladeDensityMap::new(next_density)), "原生模板换代继续同步真实密度");
    }
}
