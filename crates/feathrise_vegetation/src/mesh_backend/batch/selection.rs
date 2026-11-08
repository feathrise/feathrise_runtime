//! 主世界按唯一玩法观察位置变化驱动地重建每个批次节点的逐实例 LOD 前缀。

use std::sync::Arc;

use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;

use super::components::{
    VegetationMeshBatch, VegetationMeshBatchInstances, VegetationMeshBatchNode,
    VegetationMeshBatchPlacements, VegetationMeshBatchView, VegetationMeshSelectedInstance,
};
use super::resources::{VegetationMeshBatchQuality, VegetationMeshBatchSelectionState};

/// 四米触发下一次后台选择；八米保守带覆盖量化与在途期间的小幅运动，真实LOD仍由当前GPU视图决定。
const BATCH_LOD_REBUILD_DISTANCE_SQUARED: f32 = 16.0;
const BATCH_LOD_OBSERVER_MARGIN_METERS: f32 = 8.0;

/// 从不可变作者范围派生当前画质范围，禁止对上一帧结果累计缩放。
fn scaled_range(
    authored: &VisibilityRange,
    farthest_lod: bool,
    quality: VegetationMeshBatchQuality,
) -> VisibilityRange {
    let (lod_multiplier, view_distance_multiplier) = quality.multipliers();
    VisibilityRange {
        start_margin: authored.start_margin.start * lod_multiplier
            ..authored.start_margin.end * lod_multiplier,
        end_margin: authored.end_margin.start
            * if farthest_lod {
                view_distance_multiplier
            } else {
                lod_multiplier
            }
            ..authored.end_margin.end
                * if farthest_lod {
                    view_distance_multiplier
                } else {
                    lod_multiplier
                },
        use_aabb: authored.use_aabb,
    }
}

/// 只保留可能贡献当前 LOD 或交叉抖动的实例。
#[inline]
fn distance_in_range(distance: f32, range: &VisibilityRange) -> bool {
    distance >= (range.start_margin.start - BATCH_LOD_OBSERVER_MARGIN_METERS).max(0.0)
        && distance <= range.end_margin.end + BATCH_LOD_OBSERVER_MARGIN_METERS
}

/// 两个全局工作席位承接完整逐株扫描；地图实例、根旋转和相机移动只冻结小快照。
#[derive(Clone, PartialEq)]
pub(super) struct Stamp {
    root: Entity, source: usize, frame: Mat4, view: Option<Vec3>, range: Vec4,
    local: Mat4, offset: Vec3, hidden: bool,
    mode: super::super::types::VegetationMeshBatchVertexMode,
}

impl Stamp {
    fn same_content(&self, current: &Self) -> bool {
        let mut content = current.clone(); content.view = self.view;
        self == &content // 根、作者数组、整体框架、部件、显隐和画质仍须严格同代。
    }
    fn observer_distance_squared(&self, current: &Self) -> f32 {
        match (self.view, current.view) {
            (Some(frozen), Some(now)) => frozen.distance_squared(now),
            (None, None) => 0.0,
            _ => f32::INFINITY,
        }
    }
    fn covers(&self, current: &Self) -> bool {
        self.same_content(current) && self.observer_distance_squared(current)
            <= BATCH_LOD_OBSERVER_MARGIN_METERS * BATCH_LOD_OBSERVER_MARGIN_METERS
    } // 小幅镜头运动仍在结果的完整覆盖域内，不因观察点逐帧变化而永远丢弃任务。
    fn needs_refresh(&self, desired: &Self) -> bool {
        !self.same_content(desired) || self.observer_distance_squared(desired) >= BATCH_LOD_REBUILD_DISTANCE_SQUARED
    }
}
enum Worker {
    Build { entity: Entity, stamp: Stamp, task: bevy::tasks::Task<Arc<[VegetationMeshSelectedInstance]>> },
    Release(bevy::tasks::Task<()>),
}
#[derive(Resource, Default)]
pub(super) struct Workers { slots: [Option<Worker>; 2] }

pub(super) fn inserted(add: On<Add<VegetationMeshBatchInstances>>, mut state: ResMut<VegetationMeshBatchSelectionState>) {
    let entity = add.entity;
    if state.lookup.contains_key(&entity) { return; }
    let slot = state.free.pop().unwrap_or(state.entities.len());
    if slot == state.entities.len() { state.entities.push(Some(entity)); } else { state.entities[slot] = Some(entity); }
    state.lookup.insert(entity, slot);
}
pub(super) fn removed(remove: On<Remove<VegetationMeshBatchInstances>>, mut state: ResMut<VegetationMeshBatchSelectionState>) {
    if let Some(slot) = state.lookup.remove(&remove.entity) { state.entities[slot] = None; state.free.push(slot); }
    state.stamps.remove(&remove.entity);
}
fn stamp(child: &ChildOf, node: &VegetationMeshBatchNode, visibility: &Visibility,
    source: &VegetationMeshBatchPlacements, frame: &GlobalTransform,
    view: Option<Vec3>, quality: VegetationMeshBatchQuality) -> Stamp {
    let range = scaled_range(&node.authored_range, node.farthest_lod, quality);
    Stamp { root: child.parent(), source: source.0.as_ptr() as usize, frame: frame.to_matrix(), view,
        range: Vec4::new(range.start_margin.start, range.start_margin.end, range.end_margin.start, range.end_margin.end),
        local: node.local_transform.to_matrix(), offset: node.distance_offset,
        hidden: *visibility == Visibility::Hidden, mode: node.vertex_mode }
}
fn build(stamp: Stamp, placements: Arc<[super::components::VegetationMeshBatchPlacement]>) -> Arc<[VegetationMeshSelectedInstance]> {
    let Some(view) = stamp.view.filter(|_| !stamp.hidden) else { return Arc::from([]); };
    let range = VisibilityRange { start_margin: stamp.range.x..stamp.range.y,
        end_margin: stamp.range.z..stamp.range.w, use_aabb: false };
    let mut selected = Vec::with_capacity(placements.len());
    for placement in placements.iter() {
        let matrix = placement.transform.to_matrix();
        let anchor = (stamp.frame * matrix).transform_point3(stamp.offset);
        // GPU只乘一次当前批次层级变换，缓冲继续保留源地图坐标。
        let world_from_local = matrix * stamp.local;
        let shader_anchor = (stamp.frame * world_from_local).w_axis.truncate();
        // AABB测距点可能离部件原点很远；保守集合还须包含GPU实际渐变锚点，不能先剔除Shader仍该显示的株。
        if !distance_in_range(anchor.distance(view), &range)
            && !distance_in_range(shader_anchor.distance(view), &range) { continue; }
        selected.push(VegetationMeshSelectedInstance { world_from_local, lod_range: stamp.range,
            determinant_positive: world_from_local.determinant().is_sign_positive() });
    }
    selected.into()
}
#[allow(clippy::type_complexity)]
pub(super) fn select_vegetation_mesh_batch_lods(
    views: Query<&GlobalTransform, With<VegetationMeshBatchView>>,
    quality: Res<VegetationMeshBatchQuality>, mut state: ResMut<VegetationMeshBatchSelectionState>,
    mut workers: ResMut<Workers>,
    roots: Query<(&VegetationMeshBatchPlacements, &GlobalTransform), With<VegetationMeshBatch>>,
    mut nodes: Query<(&ChildOf, &VegetationMeshBatchNode, &Visibility, &mut VegetationMeshBatchInstances)>,
) {
    use bevy::tasks::{AsyncComputeTaskPool, futures::check_ready};
    let actual_view = views.single().ok().map(GlobalTransform::translation);
    if match (state.view_position, actual_view) {
        (Some(previous), Some(current)) => previous.distance_squared(current) >= BATCH_LOD_REBUILD_DISTANCE_SQUARED,
        (None, None) => false, _ => true,
    } { state.view_position = actual_view; }
    let view = state.view_position;
    for index in 0..workers.slots.len() {
        let ready = match workers.slots[index].as_mut() {
            Some(Worker::Build { task, .. }) => check_ready(task).map(Some),
            Some(Worker::Release(task)) => check_ready(task).map(|()| None),
            None => None,
        };
        let Some(result) = ready else { continue; };
        let previous = workers.slots[index].take().unwrap();
        if let (Worker::Build { entity, stamp: frozen, .. }, Some(instances)) = (previous, result) {
            let valid = state.lookup.contains_key(&entity);
            let mut retired = instances;
            if valid {
                if let Ok((child, node, visibility, mut selected)) = nodes.get_mut(entity) {
                    if let Ok((placements, frame)) = roots.get(child.parent()) {
                        if selected.volume.is_none() && frozen.covers(&stamp(child, node, visibility, placements, frame, actual_view, *quality)) {
                            state.revision = state.revision.wrapping_add(1).max(1);
                            retired = std::mem::replace(&mut selected.instances, retired);
                            selected.revision = state.revision; selected.vertex_mode = node.vertex_mode;
                            state.stamps.insert(entity, frozen);
                        }
                    }
                }
            }
            workers.slots[index] = Some(Worker::Release(AsyncComputeTaskPool::get().spawn(async move { drop(retired); })));
        }
    }
    for _ in 0..32.min(state.entities.len()) {
        let slot = state.cursor % state.entities.len(); state.cursor = state.cursor.wrapping_add(1);
        let Some(entity) = state.entities[slot] else { continue; };
        if workers.slots.iter().flatten().any(|worker| matches!(worker, Worker::Build { entity: current, .. } if *current == entity)) { continue; }
        let Some(worker_slot) = workers.slots.iter().position(Option::is_none) else { break; };
        let Ok((child, node, visibility, selected)) = nodes.get_mut(entity) else { continue; };
        if selected.volume.is_some() { continue; } // 三维作者格由GPU选择，保留独立后台上传。
        let Ok((placements, frame)) = roots.get(child.parent()) else { continue; };
        let desired = stamp(child, node, visibility, placements, frame, view, *quality);
        if selected.revision != 0 && state.stamps.get(&entity).is_some_and(|cached| !cached.needs_refresh(&desired)) { continue; }
        let source = placements.0.clone(); let frozen = desired.clone();
        workers.slots[worker_slot] = Some(Worker::Build { entity, stamp: desired,
            task: AsyncComputeTaskPool::get().spawn(async move { build(frozen, source) }) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_frame_controls_world_lod_without_baking_twice_into_gpu_instances() {
        let placement = super::super::components::VegetationMeshBatchPlacement::new(7,
            Transform::from_xyz(2.0, 0.0, 3.0));
        let part = Transform::from_xyz(0.0, 4.0, 0.0).to_matrix();
        let frame = Transform::from_xyz(100.0, 8.0, 20.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)).to_matrix();
        let frozen = Stamp { root: Entity::from_bits(1), source: 1, frame,
            view: Some(Vec3::new(103.0, 8.0, 18.0)), range: Vec4::new(0.0, 0.0, 5.0, 6.0),
            local: part, offset: Vec3::ZERO, hidden: false, mode: default() };
        let placements: Arc<[super::super::components::VegetationMeshBatchPlacement]> = Arc::from([placement]);
        let selected = build(frozen.clone(), placements.clone());
        assert_eq!(selected.len(), 1); // 世界相机在变换后的树旁，源坐标距离不能把它裁掉。
        assert!(selected[0].world_from_local.abs_diff_eq(placement.transform.to_matrix() * part, 0.0001));
        assert!((frame * selected[0].world_from_local).transform_point3(Vec3::ZERO)
            .abs_diff_eq(Vec3::new(103.0, 12.0, 18.0), 0.0001)); // GPU仅应用一次当前实例框架。
        let mut moved = frozen.clone();
        moved.frame = Transform::from_xyz(500.0, 0.0, 0.0).to_matrix();
        assert!(moved != frozen); // 迟到后台结果不能跨实例整体移动采用。
        assert!(build(moved, placements.clone()).is_empty());
        let mut wrong_view = frozen;
        wrong_view.view = Some(placement.transform.translation);
        assert!(build(wrong_view, placements).is_empty());
    }

    #[test]
    fn quality_scales_only_the_farthest_exit_with_view_distance() {
        let authored = VisibilityRange {
            start_margin: 100.0..120.0,
            end_margin: 300.0..360.0,
            use_aabb: false,
        };
        let mut quality = VegetationMeshBatchQuality::default();
        quality.set_multipliers(0.8, 0.6);
        let middle = scaled_range(&authored, false, quality);
        assert_eq!(middle.start_margin, 80.0..96.0);
        assert_eq!(middle.end_margin, 240.0..288.0);
        let farthest = scaled_range(&authored, true, quality);
        assert!((farthest.end_margin.start - 180.0).abs() < 0.0001);
        assert!((farthest.end_margin.end - 216.0).abs() < 0.0001);
    }

    #[test]
    fn observer_margin_preserves_both_sides_of_a_lod_handoff_without_changing_gpu_ranges() {
        let placements = Arc::from([super::super::components::VegetationMeshBatchPlacement::new(1,
            Transform::from_xyz(35.0, 0.0, 0.0))]);
        let frozen = Stamp { root: Entity::from_bits(1), source: 1, frame: Mat4::IDENTITY,
            view: Some(Vec3::ZERO), range: Vec4::new(0.0, 0.0, 25.0, 30.0), local: Mat4::IDENTITY,
            offset: Vec3::ZERO, hidden: false, mode: default() };
        let selected = build(frozen.clone(), placements);
        assert_eq!(selected.len(), 1); // 当镜头向前走6米，这株进入近档；后台旧快照也必须包含它。
        assert_eq!(selected[0].lod_range, frozen.range); // 仅扩张CPU候选，GPU当前镜头的真实淡出范围保持不变。
        let mut current = frozen.clone(); current.view = Some(Vec3::new(6.0, 0.0, 0.0));
        assert!(frozen.covers(&current));
        assert!(frozen.needs_refresh(&current)); // 保守结果可采用，同时继续申请新观察快照。
        current.view = Some(Vec3::new(9.0, 0.0, 0.0));
        assert!(!frozen.covers(&current)); // 越出完整覆盖域不能借旧结果冒充当前。
        current.view = frozen.view; current.frame = Transform::from_xyz(10.0, 0.0, 0.0).to_matrix();
        assert!(!frozen.covers(&current)); // 相机容差不能放宽整体移动、源修订或画质身份。
        let far = Stamp { range: Vec4::new(30.0, 35.0, 90.0, 100.0), ..frozen };
        let selected = build(far.clone(), Arc::from([super::super::components::VegetationMeshBatchPlacement::new(2,
            Transform::from_xyz(25.0, 0.0, 0.0))]));
        assert_eq!(selected.len(), 1); // 反方向移动6米会进入远档，交接两侧均须有完整候选。
        assert_eq!(selected[0].lod_range, far.range);
    }

    #[test]
    fn continuously_moving_camera_can_adopt_background_lod_results() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin))
            .init_resource::<VegetationMeshBatchQuality>()
            .init_resource::<VegetationMeshBatchSelectionState>().init_resource::<Workers>()
            .add_observer(inserted).add_observer(removed)
            .add_systems(PostUpdate, select_vegetation_mesh_batch_lods.after(bevy::transform::TransformSystems::Propagate));
        let root = app.world_mut().spawn((VegetationMeshBatch { layer: crate::VegetationLayerId(1),
            prototype: crate::VegetationPrototypeId(1), batch_id: 1, instance_count: 1 },
            VegetationMeshBatchPlacements(Arc::from([super::super::components::VegetationMeshBatchPlacement::new(1,
                Transform::from_xyz(35.0, 0.0, 0.0))])), Transform::from_xyz(-54.725, 28.96, 0.0))).id();
        let node = app.world_mut().spawn((ChildOf(root), Transform::IDENTITY, Visibility::Inherited,
            VegetationMeshBatchNode { authored_range: VisibilityRange::abrupt(0.0, 100.0), farthest_lod: true,
                local_transform: Transform::IDENTITY, distance_offset: Vec3::ZERO, vertex_mode: default() },
            VegetationMeshBatchInstances::empty(default()))).id();
        let view = app.world_mut().spawn((VegetationMeshBatchView, Transform::from_xyz(-54.725, 28.96, 0.0))).id();
        for step in 0..24 {
            app.world_mut().get_mut::<Transform>(view).unwrap().translation.x = -54.725 + step as f32 * 1.5;
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(2)); // 仅测试夹具让真实worker完成；生产系统每帧非阻塞轮询。
        }
        let selected = app.world().get::<VegetationMeshBatchInstances>(node).unwrap();
        assert!(selected.revision > 0, "连续移动不能让所有后台结果因相机票据改变而永久拒绝");
        assert_eq!(selected.instances.len(), 1);
    }

    #[test]
    fn conservative_selection_also_covers_the_shader_origin_when_aabb_anchor_is_distant() {
        let frozen = Stamp { root: Entity::from_bits(1), source: 1, frame: Mat4::IDENTITY,
            view: Some(Vec3::ZERO), range: Vec4::new(0.0, 0.0, 8.0, 10.0), local: Mat4::IDENTITY,
            offset: Vec3::new(0.0, 40.0, 0.0), hidden: false, mode: default() };
        let selected = build(frozen, Arc::from([super::super::components::VegetationMeshBatchPlacement::new(1,
            Transform::IDENTITY)]));
        assert_eq!(selected.len(), 1); // 相机仍在Shader近档范围内，不能仅凭远处AABB中心清空这株。
    }
}
