//! 共同边界曲线编码和实时姿态校验；只读当前批次身份。
use super::*;
pub(in crate::scenes::dynamic_scene) fn collision_constraints(
    profiles: &[feathrise_scene_content::ClipmapEdgeProfile],
) -> GpuBladeClipmapStitches {
    let mut data = GpuBladeClipmapStitches::default();
    for profile in profiles {
        let e = profile.edge.index();
        data.edges[e] = Vec4::new(
            profile.samples.len() as f32,
            profile.step,
            profile.blend_width,
            profile.collision_stride as f32,
        );
        for (i, h) in profile.samples.iter().enumerate() {
            data.heights[e][i / 4][i % 4] = *h;
        }
    }
    data
}
pub(super) fn paired(
    world: &World,
    root: Entity,
    link: &feathrise_scene_content::ClipmapStitch,
) -> Option<Entity> {
    let node = world.get::<SubsceneInstance>(root)?;
    let mut path = node.path.clone();
    path.terrain_id = Some(link.peer.clone());
    world
        .get_resource::<TerrainInstanceLookup>()?
        .instance(node.batch, &path)
}

/// 使用实时源变换验证，拖动预览不会把旧接缝跨空隙拉长；取消拖动同样触发恢复。
pub(super) fn aligned(
    world: &World,
    a: Entity,
    b: Entity,
    link: &feathrise_scene_content::ClipmapStitch,
) -> bool {
    let Some(ao) = world.get::<TerrainObjectInstance>(a) else {
        return false;
    };
    let Some(bo) = world.get::<TerrainObjectInstance>(b) else {
        return false;
    };
    let Some(af) = world.get::<SceneSourceFrame>(a) else {
        return false;
    };
    let Some(bf) = world.get::<SceneSourceFrame>(b) else {
        return false;
    };
    let Some(mate) = bo.definition.clipmap_stitches.iter().find(|s| {
        s.id == link.id
            && s.peer == ao.definition.id
            && s.edge == link.peer_edge
            && s.peer_edge == link.edge
    }) else {
        return false;
    };
    if !ao
        .definition
        .terrain
        .resolved_stitches
        .iter()
        .any(|p| p.edge == link.edge)
        || !bo
            .definition
            .terrain
            .resolved_stitches
            .iter()
            .any(|p| p.edge == mate.edge)
    {
        return false;
    }
    let ends = |o: &feathrise_scene_content::SceneTerrainObject,
                e: feathrise_scene_content::ClipmapEdge,
                p: &feathrise_scene_content::ClipmapEdgeProfile,
                f: &SceneSourceFrame| {
        let axis = e.axis();
        let normal = 1 - axis;
        let mut start = o.world_min;
        let mut end = o.world_max;
        let bound = if e.index() % 2 == 0 {
            o.world_min[normal]
        } else {
            o.world_max[normal]
        };
        start[normal] = bound;
        end[normal] = bound;
        [
            f.to_world(Vec3::new(start[0], p.samples[0], start[1])),
            f.to_world(Vec3::new(end[0], *p.samples.last().unwrap(), end[1])),
        ]
    };
    let aa = ends(&ao.definition, link.edge, &link.profile, af);
    let bb = ends(&bo.definition, mate.edge, &mate.profile, bf);
    let tolerance = (aa[0].distance(aa[1]) * 0.000001).max(0.0005);
    let reverse =
        aa[0].distance(bb[1]) < tolerance && aa[1].distance(bb[0]) < tolerance;
    let forward =
        aa[0].distance(bb[0]) < tolerance && aa[1].distance(bb[1]) < tolerance;
    let normal = |e: feathrise_scene_content::ClipmapEdge, f: &SceneSourceFrame| {
        f.source_to_world
            .transform_vector3(match e {
                feathrise_scene_content::ClipmapEdge::West => Vec3::NEG_X,
                feathrise_scene_content::ClipmapEdge::East => Vec3::X,
                feathrise_scene_content::ClipmapEdge::South => Vec3::NEG_Z,
                feathrise_scene_content::ClipmapEdge::North => Vec3::Z,
            })
            .normalize()
    };
    if (!reverse && !forward)
        || normal(link.edge, af).dot(normal(mate.edge, bf)) > -0.99999
        || link.profile.samples.len() != mate.profile.samples.len()
    {
        return false;
    }
    // 不能只核对XZ端点或脱离边坐标的Y：倾斜预览会让边上各点的高度和平面位置一同变化。
    let sample_point = |o: &feathrise_scene_content::SceneTerrainObject, e: feathrise_scene_content::ClipmapEdge,
                        i: usize, count: usize, height: f32, frame: &SceneSourceFrame| {
        let axis = e.axis(); let normal = 1 - axis;
        let mut p = o.world_min;
        p[axis] += (o.world_max[axis] - o.world_min[axis]) * i as f32 / (count - 1) as f32;
        p[normal] = if e.index() % 2 == 0 { o.world_min[normal] } else { o.world_max[normal] };
        frame.to_world(Vec3::new(p[0], height, p[1]))
    };
    link.profile.samples.iter().enumerate().all(|(i, height)| {
        let j = if reverse {
            mate.profile.samples.len() - 1 - i
        } else {
            i
        };
        let a = sample_point(&ao.definition, link.edge, i, link.profile.samples.len(), *height, af);
        let b = sample_point(&bo.definition, mate.edge, j, mate.profile.samples.len(), mate.profile.samples[j], bf);
        a.distance(b) < tolerance
    })
}

pub(super) fn encode(world: &World, root: Entity) -> GpuBladeClipmapStitches {
    let mut result = GpuBladeClipmapStitches::default();
    let Some(object) = world.get::<TerrainObjectInstance>(root) else {
        return result;
    };
    let local_step = world
        .get::<StitchLodStamp>(root)
        .map_or(Vec4::ZERO, |s| s.0);
    for link in object.definition.clipmap_stitches.iter() {
        let Some(peer) = paired(world, root, link) else {
            if let Some(profile) = object
                .definition
                .terrain
                .resolved_stitches
                .iter()
                .find(|p| p.edge == link.edge)
            {
                let edge = profile.edge.index();
                result.edges[edge] = Vec4::new(
                    profile.samples.len() as f32,
                    profile.step,
                    profile.blend_width,
                    (profile.samples.len() - 1) as f32,
                );
                for (i, value) in profile.samples.iter().enumerate() {
                    result.heights[edge][i / 4][i % 4] = *value;
                }
            }
            continue;
        };
        if !aligned(world, root, peer, link) {
            // 预览只移动已有完整局部形状；源定义尚未换代时，不在PostUpdate单独改掉物理对应的高度。
            if object
                .definition
                .terrain
                .resolved_stitches
                .iter()
                .any(|p| p.edge == link.edge)
                && let Some(old) = world
                    .get_resource::<InstanceResourceTable<SparseHeightPageRuntime>>()
                    .and_then(|t| t.values.get(&root))
            {
                let e = link.edge.index();
                result.edges[e] = old.stitches.edges[e];
                result.heights[e] = old.stitches.heights[e];
            }
            continue;
        }
        let profile = &link.profile;
        let edge = profile.edge.index();
        let axis = profile.edge.axis();
        let tangent_scale = |entity: Entity, axis: usize| {
            world.get::<SceneSourceFrame>(entity).map_or(1.0, |f| {
                f.source_to_world
                    .transform_vector3(if axis == 0 { Vec3::X } else { Vec3::Z })
                    .length()
            })
        };
        let scale = tangent_scale(root, axis);
        let other_axis = link.peer_edge.axis();
        let other_scale = tangent_scale(peer, other_axis);
        let peer_step = world
            .get::<StitchLodStamp>(peer)
            .map_or(0.0, |s| s.0[link.peer_edge.index()] * other_scale);
        // 内环边使用前一级吸附中心，会在过渡格产生最多四分之一粗格的相位偏移。
        // 共同格至少覆盖两倍最大几何格，保证相邻边点不会跨过共同曲线的中间折点。
        let world_step = if local_step[edge] > 0.0 && peer_step > 0.0 {
            (local_step[edge] * scale).max(peer_step) * 2.0
        } else {
            // 首载另一侧尚无已采用几何身份时，共同使用端点直边；不能先细化一侧再等粗网格出现。
            profile.step * scale * (profile.samples.len() - 1) as f32
        };
        let ratio = (world_step / (profile.step * scale)).max(1.0);
        let stride =
            (1_u32 << (ratio.log2().ceil() as u32).min(15)).min((profile.samples.len() - 1) as u32);
        result.edges[edge] = Vec4::new(
            profile.samples.len() as f32,
            profile.step,
            profile.blend_width,
            stride as f32,
        );
        for (i, value) in profile.samples.iter().enumerate() {
            result.heights[edge][i / 4][i % 4] = *value;
        }
    }
    result
}
