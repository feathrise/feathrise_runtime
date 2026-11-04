//! 完整Scene0证明后捕获全部真实几何；迟到、热更和捕获期间的移动都不安装混合版本碰撞。
use bevy::{prelude::*, tasks::{AsyncComputeTaskPool, futures::check_ready}, asset::AssetEvent, world_serialization::WorldAssetRoot};
use feathrise_scene_core::residency::{InstanceResMut, ResidentAssetEventReader};
use super::{resources::*, capture::Capture};
use crate::maps::dynamic_map::resources::{DynamicMapRuntimeState, DynamicMapRuntimeStatus, DynamicMapRuntimePhase};

pub(super) fn drive(mut commands: Commands, mut state: InstanceResMut<GlbObjectRuntime>,
    mut map: InstanceResMut<DynamicMapRuntimeState>, mut status: InstanceResMut<DynamicMapRuntimeStatus>,
    roots: Query<(&WorldAssetRoot, &GlobalTransform)>, lifecycle: Res<crate::shared::world_asset_lifecycle::WorldAssetLifecycleIndex>,
    meshes: Res<Assets<Mesh>>, children: Query<&Children>, visuals: Query<(&Mesh3d, &GlobalTransform, Option<&crate::StaticMeshLodOriginal>)>,
    names: Query<&Name>,
    mut events: ResidentAssetEventReader<Mesh>, time: Res<Time<Real>>, mut budget: ResMut<CaptureBudget>) {
    if map.glb_path.is_none() || map.derive_glb_height { return; }
    let Some(root) = map.root else { return; };
    let Ok((asset, pose)) = roots.get(root) else { return; };
    for event in events.read() {
        if matches!(event, AssetEvent::Modified { id } | AssetEvent::Removed { id } if state.sources.contains(id)) { state.revision = state.revision.wrapping_add(1); }
    }
    let Some(epoch) = lifecycle.current_ready_epoch(root, asset.0.id()) else { map.collision_ready = false; return; };
    let desired = Identity { root, epoch, revision: state.revision, scale: pose.compute_transform().scale.to_array().map(f32::to_bits) };
    state.desired = Some(desired);
    if state.release.as_mut().and_then(check_ready).is_some() { state.release = None; }
    if let Some((identity, result)) = state.task.as_mut().and_then(check_ready) {
        state.task = None;
        if identity == desired && lifecycle.request_epoch_is_current(root, asset.0.id(), epoch) {
            match result {
                Ok(collider) => { commands.entity(root).insert(collider); state.applied = Some(identity); map.collision_ready = true;
                    status.phase = DynamicMapRuntimePhase::Ready; status.detail = "原始GLB全部真实网格与三角碰撞已就绪".into(); }
                Err(error) => { state.failed = Some(identity); status.phase = DynamicMapRuntimePhase::Failed; status.detail = error; }
            }
        } else { state.release = Some(AsyncComputeTaskPool::get().spawn(async move { drop(result); })); }
    }
    if state.applied == Some(desired) || state.failed == Some(desired) || state.task.is_some() || state.release.is_some() { return; }
    let inverse = pose.to_matrix().inverse();
    if !inverse.is_finite() { status.phase = DynamicMapRuntimePhase::Failed; status.detail = "GLB地形变换无效".into(); return; }
    if state.capture.as_ref().is_some_and(|capture| capture.identity != desired) {
        let retired = state.capture.take(); state.release = Some(AsyncComputeTaskPool::get().spawn(async move { drop(retired); })); return;
    }
    if state.capture.is_none() { state.capture = Some(Capture::new(desired, inverse)); map.collision_ready = false; }
    state.capture.as_mut().unwrap().set_root_inverse(inverse);
    budget.begin(time.elapsed_secs_f64().to_bits());
    let result = state.capture.as_mut().unwrap().step(&meshes, &children, &visuals, &names, &mut budget);
    for entity in state.capture.as_mut().unwrap().markers.drain(..) {
        commands.entity(entity).insert(crate::DynamicMapTerrainPrimitive);
        #[cfg(any(feature = "test_panel", feature = "author_view"))] // 玩法承托始终登记；作者网格线标记只存在于F12构建。
        commands.entity(entity).insert(crate::TerrainGridTarget);
    }
    match result {
        Ok(true) => {
            let mut capture = state.capture.take().unwrap();
            let retired = std::mem::replace(&mut state.sources, std::mem::take(&mut capture.sources));
            state.task = Some(AsyncComputeTaskPool::get().spawn(async move { drop(retired); (desired, capture.build()) }));
        }
        Ok(false) => status.detail = "正在分帧冻结原始GLB的全部碰撞几何".into(),
        Err(error) => { state.failed = Some(desired); let retired = state.capture.take();
            state.release = Some(AsyncComputeTaskPool::get().spawn(async move { drop(retired); }));
            status.phase = DynamicMapRuntimePhase::Failed; status.detail = error; }
    }
}
