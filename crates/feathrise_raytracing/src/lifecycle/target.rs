//! 动态地图唯一地形的目标登记及存活验证。

use bevy::{camera::RenderTarget, prelude::*}; // State、窗口目标、Query、Mesh3d与资源系统参数。
use feathrise_scene::{
    ClipmapRaytracingAnchor, DynamicMapRuntimePhase, DynamicMapRuntimeStatus,
    DynamicMapTerrainPrimitive, DynamicSparseClipmapPrimitive, SubMapRuntimeStatus,
}; // 动态地图必须复用场景运行时的完整Ready证据和正式地形标记。
use feathrise_scene_core::state::ScenePhase; // 两类获批地图目标使用明确场景边界。

use crate::{resources::RaytracingSceneTracker, types::RaytracingPrototypeTarget};

/// 动态地图完整Ready后登记唯一正式地形；Mesh句柄变化覆盖未保存Live Apply与GLB热重载。
pub(crate) fn observe_dynamic_map_target(
    scene: Res<State<ScenePhase>>, // 非动态场景即使残留状态资源也不得取得光追所有权。
    status: Res<DynamicMapRuntimeStatus>, // 白模、碰撞与可选地表全部完成后才允许登记。
    sub_maps: Option<Res<SubMapRuntimeStatus>>, // 容器Ready不能替代独立网格及子地图的完整采用证据。
    terrains: Query<
        (Entity, &Mesh3d),
        Or<(
            (
                With<DynamicMapTerrainPrimitive>,
                Without<DynamicSparseClipmapPrimitive>,
            ),
            With<ClipmapRaytracingAnchor>,
        )>,
    >, // GPU平面环带不能冒充BLAS；仅接受经过完整高度准备的世界固定锚点。
    mut tracker: ResMut<RaytracingSceneTracker>, // 相同实体、Mesh和地图修订的稳定帧不推进代次。
    terrain_sources: Option<Res<feathrise_scene::StaticTerrainRaytracingSources>>,
) {
    if *scene.get() != ScenePhase::Active
        || status.phase != DynamicMapRuntimePhase::Ready
        || sub_maps.as_deref().is_some_and(|sub_maps| !sub_maps.ready)
    {
        // 空容器可能先于子地表标记Ready；此时唯一primitive仍可能属于隐藏候选，不能提前切Deferred。
        // 坏热更保留ready=true的旧完整批次时仍可继续沿用，不以last_error撤销原有效显示。
        tracker.clear_dynamic();
        return;
    }
    let mut terrains = terrains.iter();
    let Some((entity, mesh)) = terrains.next() else {
        tracker.clear_dynamic(); // 标记尚未落地或热重建间隙继续保持完整光栅。
        return;
    };
    if terrains.next().is_some() {
        tracker.clear_dynamic(); // 多正式地形会破坏“一图一个BLAS”契约，绝不任选其一。
        return;
    }
    tracker.mark_dynamic_ready(entity, mesh.id(), status.revision);
    let revision = terrain_sources
        .as_deref()
        .map_or(0, |sources| sources.revision());
    if tracker.terrain_revision != revision {
        tracker.terrain_revision = revision;
        tracker.generation = tracker.generation.wrapping_add(1).max(1);
    }
}

/// 只返回当前仍存活且身份未漂移的明确目标根；失败时由生命周期统一回退。
pub(super) fn validated_target_root(
    tracker: &RaytracingSceneTracker,
    current_scene: ScenePhase,
    dynamic_terrains: &Query<
        &Mesh3d,
        Or<(
            With<DynamicMapTerrainPrimitive>,
            With<ClipmapRaytracingAnchor>,
        )>,
    >,
    dynamic_status: &DynamicMapRuntimeStatus,
) -> Option<Entity> {
    let root = tracker.root?;
    match tracker.target {
        RaytracingPrototypeTarget::DynamicMapTerrain => {
            let mesh_matches = dynamic_terrains
                .get(root)
                .is_ok_and(|mesh| Some(mesh.id()) == tracker.target_mesh);
            (current_scene == ScenePhase::Active
                && tracker.scene == Some(ScenePhase::Active)
                && dynamic_status.phase == DynamicMapRuntimePhase::Ready
                && dynamic_status.revision == tracker.dynamic_status_revision
                && mesh_matches)
                .then_some(root)
        }
        RaytracingPrototypeTarget::None => None,
    }
}

/// 只接受唯一窗口玩法相机；误带玩法标记的F12纹理视图不会取得Solari所有权。
pub(super) fn unique_window_gameplay_camera<'a>(
    cameras: impl Iterator<Item = (Entity, &'a RenderTarget)>,
) -> Option<Entity> {
    let mut window_cameras = cameras
        .filter_map(|(entity, target)| matches!(target, RenderTarget::Window(_)).then_some(entity));
    let camera = window_cameras.next()?;
    window_cameras.next().is_none().then_some(camera)
}

#[cfg(test)]
mod tests {
    use super::*; // 直接锁定动态目标的幂等换代与完整清除语义。

    #[test]
    fn stable_dynamic_target_does_not_rebuild_but_revision_change_does() {
        let entity = Entity::from_raw_u32(7).expect("测试动态地形实体合法");
        let mesh = Handle::<Mesh>::default().id();
        let mut tracker = RaytracingSceneTracker::default();
        tracker.mark_dynamic_ready(entity, mesh, 3);
        let first_generation = tracker.generation;
        tracker.mark_dynamic_ready(entity, mesh, 3);
        assert_eq!(tracker.generation, first_generation); // 稳定帧不得反复重建BLAS。
        tracker.mark_dynamic_ready(entity, mesh, 4);
        assert!(tracker.generation > first_generation); // 地图Ready修订变化必须使旧结果过期。
        tracker.clear_dynamic();
        assert_eq!(tracker.target, RaytracingPrototypeTarget::None);
        assert!(tracker.root.is_none()); // 离图或Ready失效后不保留可误接管的源实体。
    }

    fn target_observer_app() -> (App, Entity) {
        let mut app = App::new();
        app.insert_resource(State::new(ScenePhase::Active))
            .insert_resource(DynamicMapRuntimeStatus {
                phase: DynamicMapRuntimePhase::Ready,
                revision: 3,
                ..default()
            })
            .init_resource::<RaytracingSceneTracker>()
            .add_systems(Update, observe_dynamic_map_target);
        let terrain = app.world_mut().spawn((
            DynamicMapTerrainPrimitive,
            Mesh3d(Handle::default()),
            Visibility::Hidden,
        )).id(); // 复现容器Ready且仅有一个候选primitive；不靠多目标拒绝或缺少网格掩盖就绪门。
        (app, terrain)
    }

    #[test]
    fn target_observer_waits_for_complete_submaps_and_clears_on_reload_wait() {
        let (mut app, terrain) = target_observer_app();
        app.insert_resource(SubMapRuntimeStatus { total: 1, ..default() });
        app.update();
        let tracker = app.world().resource::<RaytracingSceneTracker>();
        assert_eq!(tracker.target, RaytracingPrototypeTarget::None);
        assert!(tracker.root.is_none(), "根容器Ready不能提前接管尚未发布的独立网格");

        app.world_mut().resource_mut::<SubMapRuntimeStatus>().ready = true;
        app.update();
        let tracker = app.world().resource::<RaytracingSceneTracker>();
        assert_eq!(tracker.target, RaytracingPrototypeTarget::DynamicMapTerrain);
        assert_eq!(tracker.root, Some(terrain));
        let ready_generation = tracker.generation;
        app.update();
        assert_eq!(app.world().resource::<RaytracingSceneTracker>().generation, ready_generation);

        app.world_mut().resource_mut::<SubMapRuntimeStatus>().ready = false;
        app.update();
        let tracker = app.world().resource::<RaytracingSceneTracker>();
        assert_eq!(tracker.target, RaytracingPrototypeTarget::None);
        assert!(tracker.root.is_none(), "重载重新等待时必须清除旧目标，生命周期才能归还Deferred租约");
        assert!(tracker.generation > ready_generation);
    }

    #[test]
    fn target_observer_rejects_incomplete_failure_but_keeps_complete_hot_reload_fallback() {
        let (mut app, terrain) = target_observer_app();
        app.insert_resource(SubMapRuntimeStatus {
            total: 1,
            last_error: Some("子地图地表失败".into()),
            ..default()
        });
        app.update();
        assert!(app.world().resource::<RaytracingSceneTracker>().root.is_none());

        app.world_mut().resource_mut::<SubMapRuntimeStatus>().ready = true;
        app.update();
        assert_eq!(app.world().resource::<RaytracingSceneTracker>().root, Some(terrain),
            "坏热更保留上一完整批次时，失败详情不能撤销有效光追目标");
    }

    #[test]
    fn target_observer_keeps_standalone_fixture_without_submap_host_compatible() {
        let (mut app, terrain) = target_observer_app();
        app.update();
        assert_eq!(app.world().resource::<RaytracingSceneTracker>().root, Some(terrain));
    }
}
