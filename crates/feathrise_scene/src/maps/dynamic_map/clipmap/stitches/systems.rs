//! 归属观察器与提取前原子发布；后台结果由既有实例事务采用。
use super::*;
pub(super) fn track(
    insert: On<Insert<GpuBladeClipmapHeight>>,
    owners: Query<&MapInstanceOwner>,
    mut index: ResMut<Followers>,
) {
    if let Ok(owner) = owners.get(insert.entity) {
        index.assign(insert.entity, owner.0);
    }
}
pub(super) fn track_owner(
    insert: On<Insert<MapInstanceOwner>>,
    heights: Query<(), With<GpuBladeClipmapHeight>>,
    owners: Query<&MapInstanceOwner>,
    mut index: ResMut<Followers>,
) {
    if heights.contains(insert.entity)
        && let Ok(owner) = owners.get(insert.entity)
    {
        index.assign(insert.entity, owner.0);
    }
}
pub(super) fn untrack(remove: On<Remove<GpuBladeClipmapHeight>>, mut index: ResMut<Followers>) {
    index.remove(remove.entity);
}
pub(super) fn untrack_owner(remove: On<Remove<MapInstanceOwner>>, mut index: ResMut<Followers>) {
    index.remove(remove.entity);
}

pub(super) fn publish(world: &mut World) {
    let mut query = world.query_filtered::<Entity, (
        With<TerrainObjectInstance>,
        Or<(
            Changed<StitchLodStamp>,
            Changed<MapSourceFrame>,
            Changed<TerrainObjectInstance>,
        )>,
    )>();
    let changed: Vec<_> = query.iter(world).collect();
    let mut roots = BTreeSet::new();
    for root in changed {
        let Some(object) = world.get::<TerrainObjectInstance>(root) else {
            continue;
        };
        if object.definition.clipmap_stitches.is_empty() {
            continue;
        }
        roots.insert(root);
        for link in object.definition.clipmap_stitches.iter() {
            if let Some(peer) = paired(world, root, link) {
                roots.insert(peer);
            }
        }
    }
    if roots.is_empty() {
        return;
    }
    for root in &roots {
        let Some(object) = world.get::<TerrainObjectInstance>(*root) else {
            continue;
        };
        let status = StitchStatus {
            assigned: object.definition.clipmap_stitches.len() as u8,
            active: object
                .definition
                .clipmap_stitches
                .iter()
                .filter(|link| {
                    paired(world, *root, link).is_some_and(|peer| aligned(world, *root, peer, link))
                })
                .count() as u8,
        };
        if world.get::<StitchStatus>(*root) != Some(&status) {
            world.entity_mut(*root).insert(status);
        }
    }
    let updates: Vec<_> = roots
        .into_iter()
        .map(|root| {
            let data = encode(world, root);
            let mut author = world
                .get::<TerrainObjectInstance>(root)
                .map_or_else(GpuBladeClipmapStitches::default, |o| {
                    collision_constraints(&o.definition.terrain.resolved_stitches)
                });
            for e in 0..4 {
                if data.edges[e].x < 2.0 {
                    author.edges[e] = Vec4::ZERO;
                }
            }
            (root, Arc::new(data), Arc::new(author))
        })
        .collect();
    world.resource_scope(
        |world, mut table: Mut<InstanceResourceTable<SparseHeightPageRuntime>>| {
            world.resource_scope(|world, mut white: Mut<Assets<ClipmapHeightMaterial>>| {
                world.resource_scope(
                    |_, mut surfaces: Mut<Assets<SparseClipmapSurfaceMaterial>>| {
                        for (root, data, author) in &updates {
                            if let Some(pages) = table.values.get_mut(root) {
                                pages.synchronize_stitches(
                                    data.clone(),
                                    author.clone(),
                                    &mut white,
                                    &mut surfaces,
                                );
                            }
                        }
                    },
                );
            });
        },
    );
    for (root, data, author) in updates {
        #[cfg(any(feature = "test_panel", feature = "author_view"))]
        super::super::grid::synchronize_stitches(world, root, &data);
        // 对象拾取始终读取实例表；编辑器当前选择只切作者输入，不能把所选侧误写到地图容器占位资源。
        if let Some(mut table) =
            world.get_resource_mut::<InstanceResourceTable<DynamicClipmapGroundSurface>>()
            && let Some(ground) = table.values.get_mut(&root)
            && let Some(snapshot) = ground.snapshot.as_mut()
        {
            snapshot.synchronize_stitches(data.clone(), author.clone());
        }
        let followers: Vec<_> = world
            .resource::<Followers>()
            .by_root
            .get(&root)
            .map_or_else(Vec::new, |v| v.iter().copied().collect());
        for follower in followers {
            if let Some(mut height) = world.get_mut::<GpuBladeClipmapHeight>(follower)
                && *height.stitches != *data
            {
                height.stitches = data.clone();
                height.revision[2] = ((u64::from(height.fine_center.x.to_bits()) << 32)
                    | u64::from(height.fine_center.y.to_bits()))
                    ^ data.revision();
            }
        }
    }
}
