//! 同步Bevy 0.19.1的视图管线失效通知，避免线框继续使用相机换代前的资源布局。

use bevy::{
    pbr::{check_views_need_specialization, wireframe::specialize_wireframes},
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems,
        camera::{DirtySpecializations, DirtyWireframeSpecializations},
    },
};

pub(super) struct WireframeViewSpecializationPlugin;

impl Plugin for WireframeViewSpecializationPlugin {
    fn build(&self, app: &mut App) {
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return; // 无渲染器的逻辑测试不创建RenderApp或GPU资源。
        };
        render_app.add_systems(
            Render,
            sync_wireframe_view_specializations
                .in_set(RenderSystems::Specialize)
                .after(check_views_need_specialization)
                .before(specialize_wireframes),
        ); // 必须在当前视图键计算后、线框缓存清理及重新排队前完成，不能晚一帧。
    }
}

fn sync_wireframe_view_specializations(
    dirty_meshes: Res<DirtySpecializations>,
    mut dirty_wireframes: ResMut<DirtyWireframeSpecializations>,
) {
    if dirty_meshes.views.is_empty() {
        return; // 稳定帧不写线框资源，也不触发额外的变更检测。
    }
    // Bevy分别维护普通网格和线框的缓存，但0.19.1的视图键更新只通知前者。
    // 仅同步按RetainedViewEntity登记的变更视图；实体材质变更仍由各自插件管理。
    dirty_wireframes
        .views
        .extend(dirty_meshes.views.iter().copied());
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        app::SubApp,
        core_pipeline::prepass::DepthPrepass,
        light::atmosphere::ScatteringMedium,
        pbr::{DistanceFog, ExtractedAtmosphere, MeshPipelineKey, ViewKeyCache},
        render::{
            camera::{clear_dirty_specializations, clear_dirty_wireframe_specializations},
            render_resource::TextureFormat,
            sync_world::MainEntity,
            view::{ExtractedView, RenderVisibleEntitiesClass, RetainedViewEntity},
        },
    };

    fn fixture() -> App {
        let mut app = App::new();
        let mut render_app = SubApp::new();
        render_app
            .add_schedule(Render::base_schedule())
            .init_resource::<ViewKeyCache>()
            .init_resource::<DirtySpecializations>()
            .init_resource::<DirtyWireframeSpecializations>()
            .add_systems(
                Render,
                check_views_need_specialization.in_set(RenderSystems::PrepareAssets),
            );
        app.insert_sub_app(RenderApp, render_app);
        app.add_plugins(WireframeViewSpecializationPlugin);
        app
    }

    fn frame(app: &mut App) {
        let world = app.sub_app_mut(RenderApp).world_mut();
        world
            .run_system_cached(clear_dirty_specializations)
            .unwrap();
        world
            .run_system_cached(clear_dirty_wireframe_specializations)
            .unwrap();
        world.run_schedule(Render);
    }

    fn view(app: &mut App, main: Entity, subview: u32) -> (Entity, RetainedViewEntity) {
        let retained = RetainedViewEntity::new(main.into(), None, subview);
        let entity = app
            .sub_app_mut(RenderApp)
            .world_mut()
            .spawn((
                ExtractedView {
                    retained_view_entity: retained,
                    clip_from_view: Mat4::IDENTITY,
                    world_from_view: GlobalTransform::IDENTITY,
                    clip_from_world: None,
                    target_format: TextureFormat::Rgba8UnormSrgb,
                    viewport: UVec4::new(0, 0, 32, 32),
                    color_grading: default(),
                    invert_culling: false,
                },
                Msaa::Sample4,
            ))
            .id();
        (entity, retained)
    }

    fn atmosphere() -> ExtractedAtmosphere {
        ExtractedAtmosphere {
            inner_radius: 6_360_000.0,
            outer_radius: 6_460_000.0,
            ground_albedo: Vec3::splat(0.3),
            medium: Handle::<ScatteringMedium>::default().id(),
            world_to_atmosphere: Mat4::IDENTITY,
        }
    }

    #[test]
    fn camera_layout_additions_and_removals_invalidate_wireframes_in_the_same_frame() {
        let mut app = fixture();
        let main = app.world_mut().spawn_empty().id();
        let (entity, retained) = view(&mut app, main, 0);
        // 覆盖日志中的普通布局、天空布局和深度/天空/雾布局，并检查反向移除。
        let layouts = [0, 1, 3, 7, 5, 4, 6, 2, 0];
        for mask in layouts {
            let world = app.sub_app_mut(RenderApp).world_mut();
            let mut camera = world.entity_mut(entity);
            camera.remove::<(DepthPrepass, DistanceFog, ExtractedAtmosphere)>();
            if mask & 1 != 0 {
                camera.insert(atmosphere());
            }
            if mask & 2 != 0 {
                camera.insert(DepthPrepass);
            }
            if mask & 4 != 0 {
                camera.insert(DistanceFog::default());
            }
            frame(&mut app);
            let world = app.sub_app(RenderApp).world();
            let key = world.resource::<ViewKeyCache>()[&retained];
            assert_eq!(key.contains(MeshPipelineKey::ATMOSPHERE), mask & 1 != 0);
            assert_eq!(key.contains(MeshPipelineKey::DEPTH_PREPASS), mask & 2 != 0);
            assert_eq!(key.contains(MeshPipelineKey::DISTANCE_FOG), mask & 4 != 0);
            assert!(
                world
                    .resource::<DirtyWireframeSpecializations>()
                    .views
                    .contains(&retained)
            );
            frame(&mut app);
            assert!(
                app.sub_app(RenderApp)
                    .world()
                    .resource::<DirtyWireframeSpecializations>()
                    .views
                    .is_empty()
            );
        }
        app.sub_app_mut(RenderApp)
            .world_mut()
            .entity_mut(entity)
            .insert(Msaa::Off);
        frame(&mut app);
        assert!(
            app.sub_app(RenderApp)
                .world()
                .resource::<DirtyWireframeSpecializations>()
                .views
                .contains(&retained)
        );
    }

    #[test]
    fn only_the_changed_retained_view_requeues_already_visible_wireframes() {
        let mut app = fixture();
        let main = app.world_mut().spawn_empty().id();
        let (camera, changed) = view(&mut app, main, 0);
        let (_, unchanged) = view(&mut app, main, 1);
        frame(&mut app);
        frame(&mut app);
        app.sub_app_mut(RenderApp)
            .world_mut()
            .entity_mut(camera)
            .insert(DepthPrepass);
        frame(&mut app);
        let world = app.sub_app(RenderApp).world();
        let dirty = world.resource::<DirtyWireframeSpecializations>();
        assert_eq!(dirty.views.len(), 1);
        assert!(dirty.views.contains(&changed));
        assert!(!dirty.views.contains(&unchanged));

        // 没有新增网格也必须重选管线并替换旧分箱，否则仍会提交旧布局的间接绘制。
        let mesh: MainEntity = main.into();
        let visible = RenderVisibleEntitiesClass {
            entities_cpu_culling: vec![(camera, mesh)],
            ..default()
        };
        let pending = default();
        assert_eq!(dirty.iter_to_despecialize().count(), 0);
        assert_eq!(
            dirty
                .iter_to_specialize(changed, &visible, &pending)
                .count(),
            1
        );
        assert_eq!(dirty.iter_to_dequeue(changed, &visible).count(), 1);
        assert_eq!(dirty.iter_to_queue(changed, &visible, &pending).count(), 1);
        assert_eq!(
            dirty.iter_to_queue(unchanged, &visible, &pending).count(),
            0
        );
    }

    #[test]
    fn view_notifications_preserve_independent_mesh_and_wireframe_entity_changes() {
        let mut app = fixture();
        let mesh = app.world_mut().spawn_empty().id().into();
        let wireframe = app.world_mut().spawn_empty().id().into();
        let main = app.world_mut().spawn_empty().id();
        let (_, retained) = view(&mut app, main, 0);
        let world = app.sub_app_mut(RenderApp).world_mut();
        world
            .resource_mut::<DirtySpecializations>()
            .changed_renderables
            .insert(mesh);
        world
            .resource_mut::<DirtySpecializations>()
            .removed_renderables
            .insert(mesh);
        world
            .resource_mut::<DirtyWireframeSpecializations>()
            .changed_renderables
            .insert(wireframe);
        world
            .resource_mut::<DirtyWireframeSpecializations>()
            .removed_renderables
            .insert(wireframe);
        world.run_schedule(Render);
        let dirty = world.resource::<DirtyWireframeSpecializations>();
        assert!(dirty.views.contains(&retained));
        assert!(dirty.changed_renderables.contains(&wireframe));
        assert!(dirty.removed_renderables.contains(&wireframe));
        assert!(!dirty.changed_renderables.contains(&mesh));
        assert!(!dirty.removed_renderables.contains(&mesh));
    }
}

#[cfg(test)]
mod gpu_tests;
