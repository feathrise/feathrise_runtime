//! 多植被根的稳定绘制槽、自动回收及整目录重置后的迟到退役隔离。
use super::*;
use bevy::prelude::*;

fn allocate(
    world: &mut World,
    bank: GpuBladeProfileBank,
) -> (
    Entity,
    super::super::super::components::GpuBladeProfileBankId,
) {
    let allocation = world
        .resource_mut::<GpuBladeProfileBanks>()
        .allocate(vec![bank])
        .unwrap();
    let id = allocation.bank(0).unwrap();
    (world.spawn(allocation).id(), id)
}

#[test]
fn profile_bank_scope_roots_keep_stable_slots_and_reuse_only_retired_slots() {
    let mut world = World::new();
    world.init_resource::<GpuBladeProfileBanks>();
    world.init_resource::<Assets<Image>>();
    let first = GpuBladeProfileBank::new(
        GpuBladeProfiles::default(),
        Some(world.resource_mut::<Assets<Image>>().add(Image::default())),
    );
    let second = GpuBladeProfileBank::new(
        GpuBladeProfiles::default(),
        Some(world.resource_mut::<Assets<Image>>().add(Image::default())),
    );
    let (root_a, a) = allocate(&mut world, first.clone());
    let (root_b, b) = allocate(&mut world, second.clone());
    assert_ne!(a, b, "不同根的相同局部批次编号不能互相覆盖");
    let empty = world
        .resource_mut::<GpuBladeProfileBanks>()
        .allocate(vec![])
        .unwrap();
    let empty_root = world.spawn(empty).id();
    world.despawn(empty_root);
    assert_eq!(
        world.resource::<GpuBladeProfileBanks>().banks()[a.index() as usize - 1],
        first
    );
    assert_eq!(
        world.resource::<GpuBladeProfileBanks>().banks()[b.index() as usize - 1],
        second
    );
    world.despawn(root_a);
    let (root_c, c) = allocate(&mut world, first.clone());
    assert_eq!(a, c, "空洞按槽复用，不能挪动仍可见的兄弟根");
    assert_eq!(
        world.resource::<GpuBladeProfileBanks>().banks()[b.index() as usize - 1],
        second
    );
    world.despawn(root_b);
    assert_eq!(
        world.resource::<GpuBladeProfileBanks>().banks()[c.index() as usize - 1],
        first
    );
    world.despawn(root_c);
    assert!(world.resource::<GpuBladeProfileBanks>().is_empty());
}

#[test]
fn profile_bank_scope_old_root_cannot_release_reused_ids_after_catalog_reset() {
    let mut world = World::new();
    world.init_resource::<GpuBladeProfileBanks>();
    let bank = GpuBladeProfileBank::new(GpuBladeProfiles::default(), None);
    let (old, old_id) = allocate(&mut world, bank.clone());
    world.resource_mut::<GpuBladeProfileBanks>().clear();
    let (current, id) = allocate(&mut world, bank.clone());
    assert_eq!(id, old_id);
    world.despawn(old);
    assert_eq!(
        world.resource::<GpuBladeProfileBanks>().banks()[id.index() as usize - 1],
        bank
    );
    world.despawn(current);
    assert!(world.resource::<GpuBladeProfileBanks>().is_empty());
    for _ in 0..128 {
        let (root, _) = allocate(&mut world, bank.clone());
        world.despawn(root);
        assert!(
            world.resource::<GpuBladeProfileBanks>().is_empty(),
            "长期换根不能积累历史槽位"
        );
    }
}

#[test]
fn profile_bank_scope_retiring_another_resident_scene_cannot_release_current_catalog() {
    let mut world = World::new();
    world.init_resource::<GpuBladeProfileBanks>();
    let bank = GpuBladeProfileBank::new(GpuBladeProfiles::default(), None);
    let (old, old_id) = allocate(&mut world, bank.clone());
    let parked = world.remove_resource::<GpuBladeProfileBanks>().unwrap();
    world.init_resource::<GpuBladeProfileBanks>();
    let (current, current_id) = allocate(&mut world, bank.clone());
    assert_eq!(old_id, current_id, "不同驻留目录可以拥有相同局部下标");
    world.despawn(old);
    assert_eq!(
        world.resource::<GpuBladeProfileBanks>().banks()[current_id.index() as usize - 1],
        bank
    );
    world.despawn(current);
    assert!(world.resource::<GpuBladeProfileBanks>().is_empty());
    assert_eq!(parked.banks()[old_id.index() as usize - 1], bank);
}

#[test]
fn profile_bank_scope_inherited_volume_retains_template_until_its_last_root_retires() {
    let mut world = World::new();
    world.init_resource::<GpuBladeProfileBanks>();
    let bank = GpuBladeProfileBank::new(GpuBladeProfiles::default(), None);
    let (outdoor, id) = allocate(&mut world, bank.clone());
    let inherited = world
        .resource_mut::<GpuBladeProfileBanks>()
        .retain(vec![id])
        .unwrap();
    let volume = world.spawn(inherited).id();
    world.despawn(outdoor);
    assert_eq!(
        world.resource::<GpuBladeProfileBanks>().banks()[id.index() as usize - 1],
        bank
    );
    let replacement = world
        .resource_mut::<GpuBladeProfileBanks>()
        .retain(vec![id])
        .unwrap();
    world.entity_mut(volume).insert(replacement); // 原地刷新三维根也必须先续持再归还旧信用。
    assert_eq!(
        world.resource::<GpuBladeProfileBanks>().banks()[id.index() as usize - 1],
        bank
    );
    world.despawn(volume);
    assert!(world.resource::<GpuBladeProfileBanks>().is_empty());
}

#[test]
fn profile_bank_scope_invalid_reference_does_not_partially_pin_valid_slots() {
    let mut world = World::new();
    world.init_resource::<GpuBladeProfileBanks>();
    let (root, id) = allocate(
        &mut world,
        GpuBladeProfileBank::new(GpuBladeProfiles::default(), None),
    );
    let invalid =
        super::super::super::components::GpuBladeProfileBankId::for_dynamic_index(4).unwrap();
    assert!(
        world
            .resource_mut::<GpuBladeProfileBanks>()
            .retain(vec![id, invalid])
            .is_none()
    );
    world.despawn(root);
    assert!(
        world.resource::<GpuBladeProfileBanks>().is_empty(),
        "失败续持不能泄漏先前有效槽的信用"
    );
}
