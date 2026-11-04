//! 唯一相机复用正式物理空间树取得竖直地形柱；薄静水不能把无河床区域推断成无限水体。
use avian3d::prelude::{ColliderDisabled, SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;
use feathrise_grounding::ActiveGroundColliders;

use super::resources::UnderwaterViewState;
use crate::{
    components::{DynamicMapTerrainPrimitive, DynamicSparseTerrainCollider},
    maps::dynamic_map::DynamicMapTerrainRoot,
    VolumeSurfaceFields,
};

type TerrainColliders = (
    Or<(
        With<DynamicSparseTerrainCollider>,
        With<DynamicMapTerrainPrimitive>,
        With<DynamicMapTerrainRoot>,
        // 独立体素把已采用碰撞装在字段根，显示 primitive 则位于其子节点。
        With<VolumeSurfaceFields>,
    )>,
    Without<ColliderDisabled>,
);
type VisibilityHierarchy = (
    Option<&'static Visibility>,
    Option<&'static InheritedVisibility>,
    Option<&'static ChildOf>,
);

/// 每帧只有当前唯一透视相机执行两条最近射线，不遍历、冻结或复制全图地形。
pub(in super::super) fn sample_column(
    spatial: SpatialQuery,
    terrain: Query<(), TerrainColliders>,
    active_sources: ActiveGroundColliders,
    hierarchy: Query<VisibilityHierarchy>,
    mut state: ResMut<UnderwaterViewState>,
) {
    state.floor_y = None;
    state.ceiling_y = None;
    let Some(position) = state.position.filter(|position| position.is_finite()) else { return; };
    let filter = SpatialQueryFilter::default();
    let accepts = |entity| terrain.contains(entity)
        && active_sources.is_active(entity)
        && visible_source(entity, &hierarchy);

    // 查询范围由现有 BVH 的实际形状决定；MAX 是查询界限，不是作者水深或猜测的水底。
    if let Some(hit) = spatial.cast_ray_predicate(
        position, Dir3::NEG_Y, f32::MAX, true, &filter, &accepts,
    ) {
        // 最近实际面不满足承托条件时保守拒绝，不能穿过它寻找另一层河床。
        if hit.distance.is_finite() && hit.distance > 0.0 && hit.normal.y > 0.05 {
            state.floor_y = Some(position.y - hit.distance).filter(|height| height.is_finite());
        }
    }
    if let Some(hit) = spatial.cast_ray_predicate(
        position, Dir3::Y, f32::MAX, true, &filter, &accepts,
    ) {
        // 洞顶、地形背面和实心内部零距离命中都阻断介质，不按朝上法线穿透地底。
        if hit.distance.is_finite() && hit.distance >= 0.0 {
            state.ceiling_y = Some(position.y + hit.distance).filter(|height| height.is_finite());
        }
    }
}

fn visible_source(mut entity: Entity, hierarchy: &Query<VisibilityHierarchy>) -> bool {
    // 稀疏 Collider 叶没有 Visibility，必须沿固定上限父链读取所属地形的显隐。
    for _ in 0..256 {
        let Ok((visibility, inherited, parent)) = hierarchy.get(entity) else { return false; };
        if visibility == Some(&Visibility::Hidden) || inherited.is_some_and(|value| !value.get()) {
            return false;
        }
        if visibility == Some(&Visibility::Visible) {
            return true; // 显式可见按 Bevy 规则覆盖父级显隐；刚体禁用仍由活动来源过滤。
        }
        let Some(parent) = parent else { return true; };
        entity = parent.parent();
    }
    false // 异常深层或循环层级不授予水下体积。
}

/// 无物理插件的回归仍登记真实 Collider 和 BVH，供本模块与完整水下链共同使用。
#[cfg(test)]
pub(super) fn test_insert_terrain_collider(
    world: &mut World, position: Vec3, collider: avian3d::prelude::Collider,
) -> Entity {
    use avian3d::{
        collider_tree::{ColliderTreeProxy, ColliderTrees},
        prelude::{CollisionLayers, Position, Rotation, SimpleCollider},
    };
    world.init_resource::<ColliderTrees>();
    let aabb = collider.aabb(position, Quat::IDENTITY);
    let entity = world.spawn((
        DynamicSparseTerrainCollider, Position(position), Rotation(Quat::IDENTITY), collider,
    )).id();
    world.resource_mut::<ColliderTrees>().standalone_tree.add_proxy(aabb.into(), ColliderTreeProxy {
        collider: entity, body: None, layers: CollisionLayers::DEFAULT, flags: default(),
    });
    entity
}

#[cfg(test)]
mod tests {
    use avian3d::{collider_tree::ColliderTrees, prelude::{Collider, RigidBodyDisabled}};
    use super::*;

    fn app(position: Vec3) -> App {
        let mut app = App::new();
        app.init_resource::<ColliderTrees>()
            .insert_resource(UnderwaterViewState { position: Some(position), ..default() })
            .add_systems(Update, sample_column);
        app
    }

    fn slab(app: &mut App, center_y: f32) -> Entity {
        test_insert_terrain_collider(app.world_mut(), Vec3::Y * center_y, Collider::cuboid(20.0, 2.0, 20.0))
    }

    #[test]
    fn real_bed_and_deep_underground_view_have_different_columns() {
        let mut app = app(Vec3::Y * 3.0);
        slab(&mut app, 0.0);
        app.update();
        let column = app.world().resource::<UnderwaterViewState>();
        assert_eq!(column.floor_y, Some(1.0));
        assert_eq!(column.ceiling_y, None);

        app.world_mut().resource_mut::<UnderwaterViewState>().position = Some(Vec3::Y * -500.0);
        app.update();
        let column = app.world().resource::<UnderwaterViewState>();
        assert_eq!(column.floor_y, None, "地底相机不能复用先前水中的河床");
        assert_eq!(column.ceiling_y, Some(-1.0), "从地底向上看须记录真实地形背面");

        slab(&mut app, -1000.0);
        app.update();
        let column = app.world().resource::<UnderwaterViewState>();
        assert_eq!(column.floor_y, Some(-999.0));
        assert_eq!(column.ceiling_y, Some(-1.0), "另一层河床不能让上方地形消失");
    }

    #[test]
    fn nearest_queries_exclude_nonterrain_hidden_and_disabled_sources() {
        let mut app = app(Vec3::Y * 10.0);
        slab(&mut app, 0.0);
        slab(&mut app, 20.0);
        let object = slab(&mut app, 2.0);
        app.world_mut().entity_mut(object).remove::<DynamicSparseTerrainCollider>();
        let hidden = slab(&mut app, 4.0);
        app.world_mut().entity_mut(hidden).insert(Visibility::Hidden);
        let hidden_parent = app.world_mut().spawn(Visibility::Hidden).id();
        let hidden_child = slab(&mut app, 6.0);
        app.world_mut().entity_mut(hidden_child).insert(ChildOf(hidden_parent));
        let disabled_parent = app.world_mut().spawn(RigidBodyDisabled).id();
        let disabled_child = slab(&mut app, 8.0);
        app.world_mut().entity_mut(disabled_child).insert(ChildOf(disabled_parent));
        let disabled_collider = slab(&mut app, 12.0);
        app.world_mut().entity_mut(disabled_collider).insert(ColliderDisabled);
        let inherited_hidden = slab(&mut app, 14.0);
        app.world_mut().entity_mut(inherited_hidden).insert(InheritedVisibility::HIDDEN);
        app.update();
        let column = app.world().resource::<UnderwaterViewState>();
        assert_eq!(column.floor_y, Some(1.0));
        assert_eq!(column.ceiling_y, Some(19.0));
    }

    #[test]
    fn nearest_unstable_surface_is_not_skipped_for_a_deeper_bed() {
        let mut app = app(Vec3::Y * 3.0);
        slab(&mut app, 0.0);
        // y=40x+2 的实际三角朝上分量不足承托阈值，仍须挡住本次向下查询。
        test_insert_terrain_collider(app.world_mut(), Vec3::ZERO, Collider::trimesh(
            vec![Vec3::new(-1.0, -38.0, -1.0), Vec3::new(0.0, 2.0, 1.0), Vec3::new(1.0, 42.0, -1.0)],
            vec![[0, 1, 2]],
        ));
        app.update();
        assert_eq!(app.world().resource::<UnderwaterViewState>().floor_y, None);
    }

    #[test]
    fn all_adopted_terrain_collider_layouts_supply_the_same_real_bed() {
        for kind in 0..4 {
            let mut app = app(Vec3::Y * 3.0);
            let bed = slab(&mut app, 0.0);
            if kind != 0 {
                let mut bed = app.world_mut().entity_mut(bed);
                bed.remove::<DynamicSparseTerrainCollider>();
                match kind {
                    1 => { bed.insert(DynamicMapTerrainPrimitive); }
                    2 => { bed.insert(DynamicMapTerrainRoot); }
                    _ => { bed.insert(VolumeSurfaceFields::default()); }
                }
            }
            app.update();
            assert_eq!(app.world().resource::<UnderwaterViewState>().floor_y, Some(1.0));
        }
    }

    #[test]
    fn solid_interior_and_missing_camera_cannot_reuse_a_water_column() {
        let mut app = app(Vec3::ZERO);
        slab(&mut app, 0.0);
        app.update();
        let column = app.world().resource::<UnderwaterViewState>();
        assert_eq!(column.floor_y, None);
        assert_eq!(column.ceiling_y, Some(0.0));

        app.world_mut().resource_mut::<UnderwaterViewState>().position = None;
        app.update();
        let column = app.world().resource::<UnderwaterViewState>();
        assert_eq!((column.floor_y, column.ceiling_y), (None, None));
    }
}
