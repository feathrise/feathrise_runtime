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
            &GpuBladeClipmapHeight,
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
    mut applied: Local<Option<(u64, bool, Option<Entity>)>>,
) {
    let identity = (state.boundary_revision, state.valid, vegetation.active_root);
    if *applied == Some(identity) && changed_inputs.is_empty() && added_windows.is_empty() {
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
        if height != Some(next_height) {
            commands.entity(entity).insert(next_height.clone());
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
}
