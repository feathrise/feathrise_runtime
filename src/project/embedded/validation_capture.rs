//! 两种验收共用一个真实窗口截图槽；Bevy会直接丢弃同帧同目标的第二个请求，不能各自无限等待回调。
use bevy::{prelude::*, render::view::screenshot::Screenshot};

#[path = "validation_capture/readiness.rs"]
mod readiness;
pub(super) use readiness::{ReadinessPlugin, poll_readiness};
#[path = "validation_capture/wire_geometry.rs"]
pub(super) mod wire_geometry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Owner { World, Preview }

#[derive(Resource, Default)]
pub(super) struct Slot { request: Option<(Owner, Entity)> }
impl Slot {
    pub(super) fn release(&mut self, owner: Owner, entity: Entity) -> bool {
        if self.request != Some((owner,entity)) { return false; }
        self.request=None;true
    }
}

pub(super) fn request(world: &mut World, owner: Owner) -> Option<Entity> {
    if world.resource::<Slot>().request.is_some() { return None; }
    let entity=world.spawn(Screenshot::primary_window()).id();
    world.resource_mut::<Slot>().request=Some((owner,entity));
    Some(entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn one_window_capture_slot_serializes_both_producers_and_rejects_old_callbacks() {
        let mut world=World::new();world.init_resource::<Slot>();
        let first=request(&mut world,Owner::World).unwrap();
        assert!(request(&mut world,Owner::Preview).is_none());
        assert_eq!(world.query::<&Screenshot>().iter(&world).count(),1);
        assert!(!world.resource_mut::<Slot>().release(Owner::Preview,first));
        assert!(world.resource_mut::<Slot>().release(Owner::World,first));
        let second=request(&mut world,Owner::Preview).unwrap();
        assert!(!world.resource_mut::<Slot>().release(Owner::World,first));
        assert!(request(&mut world,Owner::World).is_none());
        assert!(world.resource_mut::<Slot>().release(Owner::Preview,second));
        assert!(request(&mut world,Owner::World).is_some());
    }
}
