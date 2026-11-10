//! 主世界禁用的增量同步；保留RenderEntity及其GPU输出缓冲，暂停后台场景绘制与Compute。

use crate::residency::ResidentSceneEntity;
#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
use bevy::{
    ecs::{entity_disabling::Disabled, query::Allow},
    prelude::*,
    render::{ExtractSchedule, MainWorld, RenderApp, sync_world::RenderEntity},
};

#[derive(Resource, Default)]
struct RenderPauseChanges(Vec<(Entity, Entity)>); // 主实体与渲染实体成对登记，采用时核对最终状态。

pub(super) fn install(app: &mut App) {
    app.init_resource::<RenderPauseChanges>()
        .add_observer(pause_render_entity)
        .add_observer(resume_render_entity)
        .add_observer(pause_new_render_entity);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.add_systems(ExtractSchedule, apply_render_pause);
    } else {
        app.add_systems(Last, clear_headless_changes); // 无RenderApp的工具/回归不积累无法消费的渲染通知。
    }
}

fn clear_headless_changes(mut changes: ResMut<RenderPauseChanges>) {
    changes.0.clear();
}

fn pause_render_entity(
    event: On<Insert<Disabled>>,
    entities: Query<&RenderEntity, (With<ResidentSceneEntity>, Allow<Disabled>)>,
    mut changes: ResMut<RenderPauseChanges>,
) {
    if let Ok(entity) = entities.get(event.entity) {
        changes.0.push((event.entity, entity.id()));
    }
}

fn resume_render_entity(
    event: On<Remove<Disabled>>,
    entities: Query<&RenderEntity, (With<ResidentSceneEntity>, Allow<Disabled>)>,
    mut changes: ResMut<RenderPauseChanges>,
) {
    if let Ok(entity) = entities.get(event.entity) {
        // despawn 同样触发 Remove<Disabled>；此刻还不能认定为恢复。
        changes.0.push((event.entity, entity.id()));
    }
}

fn pause_new_render_entity(
    event: On<Insert<RenderEntity>>,
    entities: Query<&RenderEntity, (With<ResidentSceneEntity>, With<Disabled>)>,
    mut changes: ResMut<RenderPauseChanges>,
) {
    if let Ok(entity) = entities.get(event.entity) {
        changes.0.push((event.entity, entity.id()));
    }
}

fn apply_render_pause(mut main: ResMut<MainWorld>, mut commands: Commands) {
    // 只消费本帧有界暂停/恢复产生的变化，不逐帧扫描后台实体，也不移除持久GPU组件。
    main.resource_scope(|world, mut changes: Mut<RenderPauseChanges>| {
        for (source, entity) in changes.0.drain(..) {
            if world
                .get::<RenderEntity>(source)
                .is_none_or(|peer| peer.id() != entity)
            {
                continue; // 已销毁或映射换代的通知不得复活旧视图；实体回收交给原同步链。
            }
            if world.get::<Disabled>(source).is_some() {
                commands.entity(entity).try_insert(Disabled);
            } else {
                commands.entity(entity).try_remove::<Disabled>();
            }
        }
    });
}
