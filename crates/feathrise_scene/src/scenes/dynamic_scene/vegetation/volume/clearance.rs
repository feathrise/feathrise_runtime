//! 三维作者格的建筑净空：后台BVH只查询同一真实承托面，不跨洞顶清除植被。
use crate::{BuildingVegetationClearanceSnapshot, ClipmapGroundHit, ClipmapGroundSnapshot};
use bevy::{ecs::system::SystemParam, prelude::*};
use feathrise_scene_content::SceneId;
use feathrise_scene_core::residency::{InstanceFrame, InstanceRes, SceneSourceFrame, SceneTerrainOwner};

/// 净空归所属场景实例，不归地形自己的资源表；查询点仍保留地形源坐标。
#[derive(SystemParam)]
pub(super) struct Inputs<'w, 's> {
    frame: InstanceFrame<'w>,
    states: InstanceRes<'w, crate::BuildingVegetationClearanceState>,
    owners: Query<'w, 's, &'static SceneTerrainOwner>,
    frames: Query<'w, 's, &'static SceneSourceFrame>,
}
impl Inputs<'_, '_> {
    pub(super) fn freeze(&self, scene: &SceneId)
        -> Option<(Option<BuildingVegetationClearanceSnapshot>, SceneSourceFrame, Option<Entity>)> {
        let instance = self.frame.instance();
        let owner = instance.and_then(|entity| self.owners.get(entity).ok())
            .map_or(instance, |owner| owner.scene_instance);
        let terrain_frame = self.frame.frame();
        let scene_frame = match owner {
            Some(owner) if Some(owner) == instance => terrain_frame,
            Some(owner) => *self.frames.get(owner).ok()?,
            None => SceneSourceFrame::default(),
        };
        // 场景和地形共同整体移动时相对矩阵不变；不能借兄弟实例的净空或姿态。
        let source_to_world = scene_frame.world_to_source * terrain_frame.source_to_world;
        let frame = SceneSourceFrame { source_to_world, world_to_source: source_to_world.inverse() };
        Some((self.states.for_instance(owner)?.snapshot_for(scene), frame, owner))
    }
}

struct Node {
    bounds: Rect,
    start: usize,
    end: usize,
    children: Option<(usize, usize)>,
}
pub(super) struct Index {
    snapshot: BuildingVegetationClearanceSnapshot,
    frame: SceneSourceFrame, // 此处“world”表示所属场景源坐标，不是整个运行世界。
    order: Vec<usize>,
    nodes: Vec<Node>,
}
impl Index {
    /// 作者格数量和建筑数量都可增长，禁止逐格扫描整图；树只在源身份换代的计算任务里建立。
    pub(super) fn new(snapshot: BuildingVegetationClearanceSnapshot, frame: SceneSourceFrame) -> Result<Self, String> {
        if snapshot.regions.len() > 8192
            || (!snapshot.floor_heights.is_empty()
                && snapshot.floor_heights.len() != snapshot.regions.len())
            || snapshot.floor_heights.iter().any(|h| !h.is_finite())
            || snapshot
                .regions
                .iter()
                .any(|r| r.corners.iter().any(|p| !p.is_finite()))
        {
            return Err("三维建筑净空数量或地基层数据无效".into());
        }
        let mut index = Self {
            order: (0..snapshot.regions.len()).collect(),
            snapshot,
            frame,
            nodes: Vec::new(),
        };
        if !index.order.is_empty() {
            index.build_node(0, index.order.len());
        }
        Ok(index)
    }
    fn build_node(&mut self, start: usize, end: usize) -> usize {
        let mut bounds = self.snapshot.regions[self.order[start]].aabb();
        for &i in &self.order[start + 1..end] {
            let b = self.snapshot.regions[i].aabb();
            bounds.min = bounds.min.min(b.min);
            bounds.max = bounds.max.max(b.max);
        }
        let node = self.nodes.len();
        self.nodes.push(Node {
            bounds,
            start,
            end,
            children: None,
        });
        if end - start > 8 {
            let axis = usize::from((bounds.max - bounds.min).y > (bounds.max - bounds.min).x);
            let regions = &self.snapshot.regions;
            self.order[start..end].sort_unstable_by(|&a, &b| {
                let x = regions[a].aabb();
                let y = regions[b].aabb();
                (x.min + x.max)[axis]
                    .total_cmp(&(y.min + y.max)[axis])
                    .then_with(|| a.cmp(&b))
            });
            let middle = (start + end) / 2;
            let left = self.build_node(start, middle);
            let right = self.build_node(middle, end);
            self.nodes[node].children = Some((left, right));
        }
        node
    }
    pub(super) fn excludes(
        &self,
        point: Vec3,
        normal: Vec3,
        cell_size: f32,
        ground: &ClipmapGroundSnapshot,
    ) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let local_point = point;
        let point = self.frame.to_world(local_point);
        let Some(normal) = self.frame.world_to_source.transpose().transform_vector3(normal).try_normalize() else {
            return false;
        };
        let mut stack = [0usize; 64];
        let mut count = 1;
        while count > 0 {
            count -= 1;
            let node = &self.nodes[stack[count]];
            if !node.bounds.contains(point.xz()) {
                continue;
            }
            if let Some((left, right)) = node.children {
                stack[count] = left;
                stack[count + 1] = right;
                count += 2;
                continue;
            }
            for &i in &self.order[node.start..node.end] {
                if !self.snapshot.regions[i].contains(point.xz()) {
                    continue;
                }
                let floor_y = self.snapshot.floor_heights.get(i).copied().or_else(|| {
                    // 旧二维接口明确只属于外表面；不得把未知层解释为整根垂直柱。
                    match ground.sample_authoritative_surface(local_point.xz()) {
                        ClipmapGroundHit::Hit(p) => Some(self.frame.to_world(p).y),
                        _ => None,
                    }
                });
                let Some(floor_y) = floor_y else {
                    continue;
                };
                let from = Vec3::new(point.x, floor_y + 0.5, point.z);
                if let Ok((hit, support_normal)) = ground.support_surface_below_in_frame(from, 100000., self.frame)
                    && normal.dot(support_normal) > 0.25
                    && self.frame.to_local(hit).distance(local_point) <= cell_size * 2.0 + 0.05 {
                    // 先以同一次命中的场景法线隔离正反面，再按作者局部米比较位置。
                    // 即使0.05米薄板远小于格距，背面样本也不能借顶面承托被建筑清除。
                    return true; // 比较真实面而非根Y差，坡地仍生效，洞顶隔开的另一层仍保留。
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use feathrise_scene_content::BuildingWorldQuad;
    use std::sync::Arc;
    #[test]
    fn underground_clearance_does_not_cross_a_solid_roof() {
        let ground = ClipmapGroundSnapshot::stacked_for_test();
        let region = BuildingWorldQuad {
            corners: [
                Vec2::ZERO,
                Vec2::new(4., 0.),
                Vec2::splat(4.),
                Vec2::new(0., 4.),
            ],
        };
        let make = |floor_y| {
            Index::new(BuildingVegetationClearanceSnapshot {
                scene_id: ground.scene_id.clone(),
                revision: 1,
                regions: Arc::from(vec![region]),
                floor_heights: Arc::from(vec![floor_y]),
            }, SceneSourceFrame::default())
            .unwrap()
        };
        assert!(make(2.).excludes(Vec3::new(1., 2., 1.), Vec3::Y, 0.1, &ground));
        assert!(!make(8.).excludes(Vec3::new(1., 2., 1.), Vec3::Y, 0.1, &ground));
        assert!(!make(2.).excludes(Vec3::new(6., 2., 1.), Vec3::Y, 0.1, &ground));
    }

    #[test]
    fn clearance_uses_scene_vertical_support_and_author_units_after_full_terrain_transform() {
        let ground = ClipmapGroundSnapshot::stacked_for_test();
        let local = Vec3::new(1., 2., 1.);
        let frame = SceneSourceFrame::from_transform(Transform::from_xyz(48., 3., -48.)
            .with_rotation(Quat::from_euler(EulerRot::XYZ, 0.35, 0.6, -0.15))
            .with_scale(Vec3::new(2., 0.2, 0.5)));
        let point = frame.to_world(local);
        let quad = |center: Vec2| BuildingWorldQuad { corners: [
            center + Vec2::splat(-0.2), center + Vec2::new(0.2,-0.2),
            center + Vec2::splat(0.2), center + Vec2::new(-0.2,0.2)] };
        let make = |center: Vec2| Index::new(BuildingVegetationClearanceSnapshot {
            scene_id: ground.scene_id.clone(), revision: 1, regions: Arc::from([quad(center)]),
            floor_heights: Arc::from([point.y]),
        }, frame).unwrap();
        // 先从当前表面上方极近处取得场景竖直首面，证明整条射线和法线确实经过完整矩阵。
        let support = ground.support_below_in_frame(point + Vec3::Y * 0.02, 1., frame);
        let ClipmapGroundHit::Hit(hit) = support else { panic!("倾斜非均匀缩放的真实朝上表面必须承托：{support:?}"); };
        assert!(frame.to_local(hit).distance(local) < 0.001);
        assert!(make(point.xz()).excludes(local, Vec3::Y, 0.1, &ground), "实际覆盖仍须清除");
        assert!(!make(Vec2::ZERO).excludes(local, Vec3::Y, 0.1, &ground), "远处建筑不得误清地形局部原点");
        // Y缩放很小时世界近邻并不代表同一作者层；比较必须回到与cell_size一致的局部米。
        assert!(!make(point.xz()).excludes(Vec3::new(1., 8., 1.), Vec3::NEG_Y, 0.1, &ground),
            "缩放压近的另一层仍不能被局部0.25米容差借用");
    }

    #[test]
    fn clearance_input_uses_owning_scene_table_and_relative_frame_for_sibling_instances() {
        use bevy::ecs::system::RunSystemOnce;
        use feathrise_scene_core::residency::{InstanceResourceTable, SceneRuntimeScope};
        let mut world = World::new();
        world.init_resource::<crate::BuildingVegetationClearanceState>();
        world.init_resource::<InstanceResourceTable<crate::BuildingVegetationClearanceState>>();
        let scene = SceneId::parse("M1-SAMPLE").unwrap();
        let owner_frame = SceneSourceFrame::from_transform(Transform::from_xyz(100., 7., 20.)
            .with_rotation(Quat::from_rotation_y(0.7)).with_scale(Vec3::splat(2.)));
        let owner = world.spawn(owner_frame).id();
        let sibling = world.spawn(SceneSourceFrame::default()).id();
        for (entity, x) in [(owner, 48.), (sibling, -48.)] {
            let mut state = crate::BuildingVegetationClearanceState::default();
            state.set_formal_layered(scene.clone(), 1, 1,
                Arc::from([BuildingWorldQuad { corners: [Vec2::splat(x);4] }]), Arc::from([0.]));
            world.resource_mut::<InstanceResourceTable<crate::BuildingVegetationClearanceState>>().values.insert(entity, state);
        }
        let relative = Transform::from_xyz(48., 0., -48.).with_rotation(Quat::from_rotation_z(0.2))
            .with_scale(Vec3::new(2., 0.5, 1.));
        let source_to_world = owner_frame.source_to_world * relative.to_matrix();
        let terrain_frame = SceneSourceFrame { source_to_world, world_to_source: source_to_world.inverse() };
        let terrain = world.spawn(SceneTerrainOwner { scene_instance: Some(owner) }).id();
        world.insert_resource(SceneRuntimeScope { instance: Some(terrain), frame: terrain_frame });
        let wanted = scene.clone();
        let (snapshot, frame, found_owner) = world.run_system_once(move |inputs: Inputs| inputs.freeze(&wanted)).unwrap().unwrap();
        assert_eq!(found_owner, Some(owner));
        assert_eq!(snapshot.unwrap().regions[0].corners[0], Vec2::splat(48.), "同源兄弟实例的净空表不能串用");
        assert!(frame.source_to_world.abs_diff_eq(relative.to_matrix(), 0.0001));
        // 场景和地形共同变换后仍以其相对姿态查询，不将全局位姿误作局部净空坐标。
        assert!(frame.to_world(Vec3::ZERO).distance(relative.translation) < 0.0001);
    }
}
