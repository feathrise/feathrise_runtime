//! 整网格水single-flight工作的最终身份复验、增量推进与原子提交。

use bevy::ecs::system::SystemParam;

use super::*;

#[derive(SystemParam)]
pub(in crate::shared::large_scene::water) struct MeshWaterRuntimeIdentity<'w, 's> {
    roots: Query<
        'w,
        's,
        (
            &'static DynamicWaterMeshOverrides,
            &'static WorldInstance,
            &'static WorldAssetRoot,
        ),
    >,
    asset_index: Res<'w, WorldAssetLifecycleIndex>,
    instance_spawner: Res<'w, WorldInstanceSpawner>,
}

/// 每帧只推进全局唯一活动根的一个固定预算阶段；全量成功前旧画面保持不变。
#[allow(clippy::too_many_arguments)]
pub(in crate::shared::large_scene::water) fn process_pending_dynamic_water_mesh_override(
    mut queue: ResMut<DynamicWaterMeshOverrideQueue>,
    mut work_state: ResMut<DynamicWaterMeshOverrideWorkState>,
    identity: MeshWaterRuntimeIdentity,
    applied_roots: Query<&DynamicWaterMeshOverrideApplied>,
    children: Query<&Children>,
    world_roots: Query<(), With<WorldAssetRoot>>,
    primitives: Query<ImportedPrimitiveData>,
    nodes: Query<(&Name, &ChildOf)>,
    local_hierarchy: Query<(&Transform, Option<&ChildOf>)>,
    overridden: Query<&DynamicWaterMeshOverridePrimitive>,
    time: Res<Time>,
    meshes: Res<Assets<Mesh>>,
    standard_materials: Res<Assets<StandardMaterial>>,
    mut dynamic_materials: ResMut<Assets<DynamicWaterMaterial>>,
    mut retired_materials: ResMut<RetiredMeshWaterMaterials>,
    mut commands: Commands,
) {
    if !identity.asset_index.events_caught_up() {
        return; // 第129条以后的Modified尚未归属时，旧/新浅工作都不得越过全局保守门。
    }
    let mut work = if let Some(work) = work_state.take_active() {
        work
    } else {
        let Some((root, revision, requested_instance, requested_epoch)) =
            work_state.take_next(&mut queue)
        else {
            return; // 稳定帧只执行常数次Option/HashMap检查。
        };
        let Ok((overrides, instance, world_asset)) = identity.roots.get(root) else {
            work_state.finish_revision(root, revision);
            return;
        };
        if !identity.instance_spawner.instance_is_ready(**instance)
            || !identity.asset_index.request_epoch_is_current(
                root,
                world_asset.0.id(),
                requested_epoch,
            )
        {
            work_state.finish_revision(root, revision);
            return; // 新World ready observer会为当前实例重新请求。
        }
        let had_applied_override = applied_roots.contains(root);
        if !requires_pending_processing(overrides, had_applied_override) {
            commands
                .entity(root)
                .insert(DynamicWaterMeshOverrideStatus::Ready);
            work_state.finish_revision(root, revision);
            return;
        }
        MeshWaterRootWork::new(
            root,
            Some(requested_instance),
            Some(requested_epoch),
            revision,
            overrides.surfaces().len(),
            had_applied_override,
        )
    };
    let root = work.root;
    let current_root = identity.roots.get(root).ok();
    let identity_matches = work_state.current_revision(root) == Some(work.revision)
        && current_root.is_some_and(|(_, instance, world_asset)| {
            work.instance_id == Some(**instance)
                && work.ready_epoch.is_some_and(|ready_epoch| {
                    identity.asset_index.request_epoch_is_current(
                        root,
                        world_asset.0.id(),
                        ready_epoch,
                    )
                })
                && identity.instance_spawner.instance_is_ready(**instance)
        });
    if !identity_matches {
        work.begin_cancellation();
    }
    if work.stage == MeshWaterWorkStage::Discard {
        commands.trigger(ScenePlacementProgressPulse { entity: root });
        if !discard_temporary_materials(&mut work, &mut dynamic_materials) {
            work_state.set_active(work);
            return; // 每帧最多释放四份临时材质。
        }
        let failure_detail = work.failure_detail.take();
        let revision_is_current = work_state.current_revision(root) == Some(work.revision);
        if identity_matches && revision_is_current {
            if let Some(detail) = failure_detail {
                warn!(
                    "placement整网格水体接管失败：root={root:?} retained_previous={} detail={detail}",
                    work.had_applied_override
                );
                commands
                    .entity(root)
                    .insert(DynamicWaterMeshOverrideStatus::Failed(detail));
            }
            work_state.finish_revision(root, work.revision);
        } else {
            work_state.requeue_if_newer(&mut queue, root, work.revision);
        }
        return;
    }
    let (overrides, _, _) = current_root.expect("identity_matches已确认活动根存在");
    let preparation = advance_preparation(
        &mut work,
        overrides,
        &children,
        &world_roots,
        &primitives,
        &nodes,
        &local_hierarchy,
        &overridden,
        &meshes,
        &standard_materials,
        &mut dynamic_materials,
        time.elapsed_secs(),
    );
    commands.trigger(ScenePlacementProgressPulse { entity: root });
    if let Err(detail) = preparation {
        work.begin_failure(detail);
        work_state.set_active(work);
        return;
    }
    if work.stage != MeshWaterWorkStage::Commit {
        work_state.set_active(work);
        return;
    }
    let primitive_count = work.prepared.len();
    let override_count = work.surfaces.len();
    restore_applied_primitives(
        root,
        &applied_roots,
        &overridden,
        &mut retired_materials,
        &mut commands,
    );
    apply_prepared_primitives(root, work.prepared, &mut commands);
    work_state.finish_revision(root, work.revision);
    info!(
        "placement整网格水体已应用：root={root:?} overrides={} primitives={primitive_count} interaction=Disabled",
        override_count,
    );
}
