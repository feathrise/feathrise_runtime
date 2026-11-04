//! 子地图复用正式六域后端，系统游标独立；相机、天空、玩家与世界时钟由父图拥有。

use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use super::super::{boundary, clipmap, layered_collision, mesh_profiles, session_crater, sparse_runtime, surface, systems, terrain_derived, terrain_topology, volume_stream, vegetation, water};

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct Advance;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct Begin;
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct Follow;

pub(super) fn initialize(world: &mut World) {
    let mut schedule = Schedule::new(Begin);
    schedule.add_systems((boundary::begin_dynamic_boundaries, surface::begin_dynamic_surface,
        mesh_profiles::begin_dynamic_mesh_profiles, vegetation::begin_dynamic_vegetation,
        water::begin_dynamic_water, water::begin_clipmap_water).chain());
    schedule.run(world);
}

pub(super) fn advance(hooks: Option<crate::SceneApplicationHooks>) -> Schedule {
    let mut schedule = Schedule::new(Advance);
    schedule.add_systems((
        (systems::observe_dynamic_map_assets, systems::prepare_dynamic_map_terrain,
            session_crater::prepare_crater_surface, session_crater::apply_pending_craters,
            sparse_runtime::drive_sparse_clipmap,
            terrain_topology::collect_terrain_topology, layered_collision::drive_layered_collision,
            terrain_topology::publish_terrain_topology,
            clipmap::drive_sparse_height_pages).chain(),
        (systems::mark_dynamic_map_terrain_primitive, terrain_derived::observe_dynamic_terrain_source,
            terrain_derived::capture_dynamic_terrain_source, terrain_derived::poll_dynamic_terrain_derivation)
            .chain().run_if(systems::dynamic_map_uses_glb_terrain),
        (boundary::observe_dynamic_boundary_assets, boundary::prepare_dynamic_boundaries).chain(),
        (surface::observe_dynamic_surface_assets, surface::prepare_dynamic_surface_definition,
            surface::install_dynamic_surface, clipmap::sync_sparse_clipmap_surface,
            clipmap::drive_sparse_display_gpu_transaction, sparse_runtime::follow_sparse_clipmap).chain(),
        mesh_profiles::drive_dynamic_mesh_profiles,
        (vegetation::observe_dynamic_vegetation_assets, vegetation::prepare_dynamic_vegetation_definition,
            vegetation::start_dynamic_vegetation_build, vegetation::install_dynamic_vegetation,
            vegetation::sync_dynamic_blade_clipmap, clipmap::drive_authoritative_height_pages,
            clipmap::sync_clipmap_ground_surface, vegetation::drive_dynamic_blade_clearance,
            vegetation::drive_dynamic_mesh_residency).chain(),
        (water::observe_dynamic_water_assets, water::prepare_dynamic_water, water::install_dynamic_water,
            water::drive_clipmap_water, water::follow_clipmap_water).chain(),
        (terrain_derived::prepare_dynamic_terrain_commit, terrain_derived::commit_dynamic_terrain_derivation,
            vegetation::commit_dynamic_vegetation_for_terrain, water::commit_dynamic_water_for_terrain,
            terrain_derived::finish_dynamic_terrain_commit).chain().run_if(systems::dynamic_map_uses_glb_terrain),
        (crate::shared::portal_data::observe_portal_data_events,
            crate::shared::portal_data::rebuild_data_driven_portals,
            crate::shared::portal_data::follow_local_portal_arrivals).chain(),
    ).chain());
    crate::shared::placement_data::add_instance_systems(&mut schedule);
    super::super::glb_objects::add_systems(&mut schedule);
    schedule.configure_sets(crate::shared::placement_data::InstancePlacementAdvance
        .after(systems::prepare_dynamic_map_terrain).before(vegetation::start_dynamic_vegetation_build));
    schedule.configure_sets(crate::SceneApplicationSet::PublishSource.after(crate::shared::portal_data::rebuild_data_driven_portals));
    if let Some(hooks) = hooks { (hooks.configure_instance)(&mut schedule); }
    schedule.add_systems(super::material_frames::bind.after(systems::prepare_dynamic_map_terrain)
        .after(surface::install_dynamic_surface).after(clipmap::drive_sparse_display_gpu_transaction)
        .after(water::follow_clipmap_water).after(crate::shared::surface_weather::resolve_puddle_mask));
    schedule.add_systems(crate::shared::surface_weather::resolve_puddle_mask);
    add_terrain_dependencies(&mut schedule);
    schedule
}

pub(super) fn follow() -> Schedule {
    let mut schedule = Schedule::new(Follow);
    schedule.add_systems((vegetation::follow_dynamic_blade_windows, water::follow_clipmap_water,
        crate::shared::portal_data::follow_local_portal_arrivals, super::material_frames::bind).chain());
    #[cfg(any(feature = "test_panel", feature = "author_view"))]
    clipmap::add_instance_grid_systems(&mut schedule, super::material_frames::bind);
    schedule
}

/// 地形域复用正式后端；地图级摆放、传送、角色和环境不在每个地形上重复执行。
pub(super) fn advance_terrain() -> Schedule {
    let mut schedule = Schedule::new(Advance);
    schedule.add_systems((
        (systems::observe_dynamic_map_assets, systems::prepare_dynamic_map_terrain,
            session_crater::prepare_crater_surface, session_crater::apply_pending_craters,
            sparse_runtime::drive_sparse_clipmap,
            terrain_topology::collect_terrain_topology, layered_collision::drive_layered_collision,
            terrain_topology::publish_terrain_topology,
            clipmap::drive_sparse_height_pages).chain(),
        super::super::voxel_objects::drive.after(systems::prepare_dynamic_map_terrain),
        (systems::mark_dynamic_map_terrain_primitive, terrain_derived::observe_dynamic_terrain_source,
            terrain_derived::capture_dynamic_terrain_source, terrain_derived::poll_dynamic_terrain_derivation)
            .chain().run_if(systems::dynamic_map_uses_glb_terrain),
        (surface::observe_dynamic_surface_assets, surface::prepare_dynamic_surface_definition,
            surface::install_dynamic_surface, clipmap::sync_sparse_clipmap_surface,
            clipmap::drive_sparse_display_gpu_transaction, sparse_runtime::follow_sparse_clipmap).chain(),
        // 独立网格须消费自己的正式Mesh预设请求；Blade不经过这个加载门。
        mesh_profiles::drive_dynamic_mesh_profiles,
        (vegetation::observe_dynamic_vegetation_assets, vegetation::prepare_dynamic_vegetation_definition,
            vegetation::start_dynamic_vegetation_build, vegetation::install_dynamic_vegetation,
            vegetation::sync_dynamic_blade_clipmap, clipmap::drive_authoritative_height_pages,
            clipmap::sync_clipmap_ground_surface, vegetation::drive_dynamic_blade_clearance,
            vegetation::drive_dynamic_mesh_residency).chain(),
        (water::observe_dynamic_water_assets, water::prepare_dynamic_water, water::install_dynamic_water,
            water::drive_clipmap_water, water::follow_clipmap_water).chain(),
        (terrain_derived::prepare_dynamic_terrain_commit, terrain_derived::commit_dynamic_terrain_derivation,
            vegetation::commit_dynamic_vegetation_for_terrain, water::commit_dynamic_water_for_terrain,
            terrain_derived::finish_dynamic_terrain_commit).chain().run_if(systems::dynamic_map_uses_glb_terrain),
        super::material_frames::bind,
    ).chain());
    crate::shared::placement_data::add_instance_systems(&mut schedule);
    super::super::glb_objects::add_systems(&mut schedule);
    add_terrain_dependencies(&mut schedule);
    schedule.add_systems(crate::shared::surface_weather::resolve_puddle_mask);
    schedule
}

fn add_terrain_dependencies(schedule: &mut Schedule) {
    // 独立网格和子地图不在根地图InstanceQuery中；沿原有有界实例调度同步各自的真实高度线框。
    #[cfg(any(feature = "test_panel", feature = "author_view"))]
    clipmap::add_instance_grid_systems(schedule, super::material_frames::bind);
    vegetation::add_volume_instance_systems(schedule);
    water::add_volume_instance_systems(schedule);
    schedule.configure_sets(crate::shared::placement_data::InstancePlacementAdvance
        .after(systems::prepare_dynamic_map_terrain).before(terrain_topology::collect_terrain_topology));
    volume_stream::add_instance_systems(schedule);
    schedule.add_systems((terrain_topology::sync_topology_materials, terrain_topology::sync_volume_surface)
        .chain().after(clipmap::drive_sparse_display_gpu_transaction).after(terrain_topology::publish_terrain_topology));
}
