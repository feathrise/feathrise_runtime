//! 使用 Bevy 正式双世界同步与提取调度验证暂停、暖恢复和关闭；无需 GPU。

use super::*;
use bevy::{
    ecs::schedule::{ScheduleLabel, SystemSet},
    render::{Render, RenderSystems, extract_plugin::ExtractPlugin, sync_world::SyncToRenderWorld},
};

fn fixture() -> App {
    let mut app = App::new();
    // 复用正式渲染提取调度，测试夹具不安装设备错误处理。
    app.add_plugins(ExtractPlugin::<RenderApp>::new(
        |_, _| {},
        Render::base_schedule,
        Render.intern(),
        RenderSystems::ExtractCommands.intern(),
        RenderSystems::PostCleanup.intern(),
    ));
    install(&mut app);
    app.sub_app_mut(RenderApp).update_schedule = Some(Render.intern());
    app
}

fn resident(app: &mut App, disabled: bool) -> Entity {
    let entity = app
        .world_mut()
        .spawn((
            ResidentSceneEntity(crate::residency::ResidentSceneToken(1)),
            SyncToRenderWorld::default(),
        ))
        .id();
    if disabled {
        app.world_mut().entity_mut(entity).insert(Disabled);
    }
    entity
}

#[test]
fn parked_view_resumes_with_original_render_identity_and_data() {
    let mut app = fixture();
    let source = resident(&mut app, true);
    app.update();
    let peer = app.world().get::<RenderEntity>(source).unwrap().id();
    app.sub_app_mut(RenderApp)
        .world_mut()
        .entity_mut(peer)
        .insert(Name::new("持久视图数据"));
    assert!(
        app.sub_app(RenderApp)
            .world()
            .get::<Disabled>(peer)
            .is_some()
    );
    for _ in 0..3 {
        app.world_mut().entity_mut(source).remove::<Disabled>();
        app.update();
        assert!(
            app.sub_app(RenderApp)
                .world()
                .get::<Disabled>(peer)
                .is_none()
        );
        assert_eq!(app.world().get::<RenderEntity>(source).unwrap().id(), peer);
        app.world_mut().entity_mut(source).insert(Disabled);
        app.update();
        assert!(
            app.sub_app(RenderApp)
                .world()
                .get::<Disabled>(peer)
                .is_some()
        );
    }
    assert_eq!(
        app.sub_app(RenderApp)
            .world()
            .get::<Name>(peer)
            .unwrap()
            .as_str(),
        "持久视图数据"
    );
}

#[test]
fn closing_parked_scene_retires_render_peer_in_same_extraction() {
    let mut app = fixture();
    let source = resident(&mut app, false);
    app.update();
    let peer = app.world().get::<RenderEntity>(source).unwrap().id();
    app.world_mut().entity_mut(source).insert(Disabled);
    app.update();
    app.world_mut().despawn(source); // 同时产生 Remove<Disabled>，不能被解释成暖恢复。
    app.update();
    assert!(app.sub_app(RenderApp).world().get_entity(peer).is_err());
}

#[test]
fn same_frame_pause_resume_and_despawn_use_final_lifecycle_state() {
    let mut app = fixture();
    let source = resident(&mut app, false);
    app.update();
    let peer = app.world().get::<RenderEntity>(source).unwrap().id();
    app.world_mut().entity_mut(source).insert(Disabled);
    app.world_mut().entity_mut(source).remove::<Disabled>();
    app.world_mut().entity_mut(source).insert(Disabled);
    app.update();
    assert!(
        app.sub_app(RenderApp)
            .world()
            .get::<Disabled>(peer)
            .is_some()
    );
    app.world_mut().entity_mut(source).remove::<Disabled>();
    app.world_mut().entity_mut(source).insert(Disabled);
    app.world_mut().despawn(source);
    app.update();
    assert!(app.sub_app(RenderApp).world().get_entity(peer).is_err());
}

#[test]
fn outdated_resume_cannot_enable_a_replaced_render_mapping() {
    let mut app = fixture();
    let source = resident(&mut app, true);
    app.update();
    let old_peer = app.world().get::<RenderEntity>(source).unwrap().id();
    app.world_mut().entity_mut(source).remove::<Disabled>();
    let new_peer = app.sub_app_mut(RenderApp).world_mut().spawn_empty().id();
    app.world_mut()
        .entity_mut(source)
        .insert((RenderEntity::from(new_peer), Disabled));
    app.update();
    assert!(
        app.sub_app(RenderApp)
            .world()
            .get::<Disabled>(old_peer)
            .is_some(),
        "旧身份的恢复消息必须拒收"
    );
    assert!(
        app.sub_app(RenderApp)
            .world()
            .get::<Disabled>(new_peer)
            .is_some()
    );
}
