//! 两端共用原作者相机附件租约；进入试玩传None，恢复真实工程相机原有附件。
use super::components::{AmbientLease, PrepassLease, ShadingView};
use bevy::{
    core_pipeline::prepass::{DeferredPrepass, DepthPrepass, NormalPrepass},
    prelude::*,
};
use feathrise_settings::{GraphicsRuntimeLeaseRegistry, GraphicsRuntimeRequirements};

/// 由调用端在GraphicsRuntimeLeaseSystems::RestoreReleased之前发布本帧要求。
pub fn apply_shading(world: &mut World, entity: Entity, desired: Option<ShadingView>) {
    let Ok(camera) = world.get_entity(entity) else {
        return;
    };
    let needs_prepass = desired.is_some_and(|view| view.options.x <= 1.0 || view.options.w < 0.5);
    let needs_ambient = desired.is_some_and(|view| view.options.x == 2.0 && view.wire.y > 0.5);
    if camera.get::<ShadingView>().copied() == desired
        && needs_prepass == camera.contains::<PrepassLease>()
        && needs_ambient == camera.contains::<AmbientLease>()
        && (!needs_prepass
            || camera
                .get::<PrepassLease>()
                .is_some_and(|lease| lease.graphics.is_some())
            || (camera.get::<Msaa>() == Some(&Msaa::Off)
                && camera.contains::<DepthPrepass>()
                && camera.contains::<NormalPrepass>()))
    {
        return;
    }
    let acquire = (needs_prepass && !camera.contains::<PrepassLease>()).then(|| PrepassLease {
        msaa: camera.get::<Msaa>().copied().unwrap_or_default(),
        depth: camera.contains::<DepthPrepass>(),
        normal: camera.contains::<NormalPrepass>(),
        graphics: None,
    });
    if let Some(mut original) = acquire {
        if let Some(mut registry) = world.get_resource_mut::<GraphicsRuntimeLeaseRegistry>() {
            let mut requirements = GraphicsRuntimeRequirements::new(entity);
            requirements.force_single_sample = true;
            requirements.require_depth_prepass = true;
            requirements.require_normal_prepass = true;
            original.graphics = Some(registry.acquire(requirements));
        }
        world.entity_mut(entity).insert(original);
    }
    let mut camera = world.entity_mut(entity);
    if let Some(view) = desired {
        camera.insert(view);
    } else {
        camera.remove::<ShadingView>();
    }
    if needs_prepass {
        // 工程Window相机受玩家画质持续管理；统一租约在画质之后采用，避免Off被Sample4覆盖。
        // 无画质插件的独立作者/测试相机仍沿用原直接附件租约。
        if camera
            .get::<PrepassLease>()
            .is_some_and(|lease| lease.graphics.is_none())
        {
            camera.insert((Msaa::Off, DepthPrepass, NormalPrepass));
        }
    } else if let Some(original) = camera.get::<PrepassLease>().copied() {
        // 原Deferred桥持有自己的附件，不能归还本模块租约时拆除它的预通道。
        if original.graphics.is_none() && !camera.contains::<DeferredPrepass>() {
            camera.insert(original.msaa);
            if !original.depth {
                camera.remove::<DepthPrepass>();
            }
            if !original.normal {
                camera.remove::<NormalPrepass>();
            }
        }
        camera.remove::<PrepassLease>();
    }
    if needs_ambient {
        if !camera.contains::<AmbientLease>() {
            let original = AmbientLease(camera.get::<AmbientLight>().cloned());
            camera.insert(original);
        }
        camera.insert(AmbientLight {
            color: Color::srgb(0.88, 0.92, 1.0),
            brightness: 800.,
            affects_lightmapped_meshes: true,
        });
    } else if let Some(original) = camera.get::<AmbientLease>().cloned() {
        if let Some(ambient) = original.0 {
            camera.insert(ambient);
        } else {
            camera.remove::<AmbientLight>();
        }
        camera.remove::<AmbientLease>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_author_view_despawn_releases_only_its_graphics_ticket() {
        let mut world = World::new();
        world.init_resource::<GraphicsRuntimeLeaseRegistry>();
        let camera = world.spawn((Camera3d::default(), Msaa::Sample4)).id();
        let unrelated = world
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .acquire(GraphicsRuntimeRequirements::new(camera));
        let view = ShadingView {
            options: Vec4::new(0., 1., 0., 1.),
            wire: Vec4::ZERO,
        };
        apply_shading(&mut world, camera, Some(view));
        apply_shading(&mut world, camera, Some(view));
        assert_eq!(
            world
                .resource::<GraphicsRuntimeLeaseRegistry>()
                .active_lease_count(),
            2
        );
        let requirements = world
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .requirements_for_camera(camera)
            .unwrap();
        assert!(
            requirements.force_single_sample
                && requirements.require_depth_prepass
                && requirements.require_normal_prepass
        );
        // 由正式画质系统采用声明，不能提前污染它稍后捕获的附件基线。
        assert_eq!(world.get::<Msaa>(camera), Some(&Msaa::Sample4));
        assert!(world.get::<DepthPrepass>(camera).is_none());
        world.despawn(camera);
        assert_eq!(
            world
                .resource::<GraphicsRuntimeLeaseRegistry>()
                .active_lease_count(),
            1
        );
        assert!(
            world
                .resource_mut::<GraphicsRuntimeLeaseRegistry>()
                .release(unrelated)
        );
    }

    #[test]
    fn standalone_author_view_repairs_replaced_prepass_attachments() {
        let mut world = World::new();
        let camera = world.spawn(Msaa::Sample4).id();
        let view = ShadingView {
            options: Vec4::new(1., 1., 0., 1.),
            wire: Vec4::ZERO,
        };
        apply_shading(&mut world, camera, Some(view));
        world
            .entity_mut(camera)
            .insert(Msaa::Sample4)
            .remove::<NormalPrepass>();
        apply_shading(&mut world, camera, Some(view));
        assert_eq!(world.get::<Msaa>(camera), Some(&Msaa::Off));
        assert!(world.get::<NormalPrepass>(camera).is_some());
        apply_shading(&mut world, camera, None);
        assert_eq!(world.get::<Msaa>(camera), Some(&Msaa::Sample4));
        assert!(world.get::<DepthPrepass>(camera).is_none());
    }
    #[test]
    fn embedded_author_view_returns_original_camera_attachments_on_play() {
        let mut world = World::new();
        let camera = world
            .spawn((
                Msaa::Sample4,
                DepthPrepass,
                AmbientLight {
                    brightness: 17.,
                    ..default()
                },
            ))
            .id();
        let other = world.spawn(Msaa::Sample2).id();
        apply_shading(
            &mut world,
            camera,
            Some(ShadingView {
                options: Vec4::new(1., 1., 0., 1.),
                wire: Vec4::ZERO,
            }),
        );
        assert_eq!(world.get::<Msaa>(camera), Some(&Msaa::Off));
        assert!(world.get::<NormalPrepass>(camera).is_some());
        apply_shading(
            &mut world,
            camera,
            Some(ShadingView {
                options: Vec4::new(2., 1., 0., 1.),
                wire: Vec4::new(0., 1., 0., 0.),
            }),
        );
        assert_eq!(world.get::<AmbientLight>(camera).unwrap().brightness, 800.);
        apply_shading(&mut world, camera, None);
        assert_eq!(world.get::<Msaa>(camera), Some(&Msaa::Sample4));
        assert!(world.get::<DepthPrepass>(camera).is_some());
        assert!(world.get::<NormalPrepass>(camera).is_none());
        assert!(world.get::<ShadingView>(camera).is_none());
        assert_eq!(world.get::<AmbientLight>(camera).unwrap().brightness, 17.);
        assert_eq!(world.get::<Msaa>(other), Some(&Msaa::Sample2));
    }
}
