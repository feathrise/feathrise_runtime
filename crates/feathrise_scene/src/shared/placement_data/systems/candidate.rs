//! 显式实例隐藏候选的分帧生成、完整复验、原子采用与采用后标记清理。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use bevy::asset::{DependencyLoadState, LoadState, RecursiveDependencyLoadState}; // 子网格与原材质三层加载状态共同决定候选资格。
use bevy::prelude::*; // 候选实体生成、场景根强持有、容器显隐与固定预算标记清理。
use feathrise_map_content::MapId; // 候选容器名称继续使用重建系统已解析的当前地图身份。
use avian3d::prelude::{RigidBody, RigidBodyDisabled}; // 所有内置盒共享禁用候选刚体，采用/退役只切换一次物理门。

use crate::BuildingVegetationClearanceState; // 建筑候选与显式实例在同一原子采用点发布正式净空。
use crate::shared::large_map::DynamicWaterMeshOverrides; // 候选根携带当前JSON快照，World ready后空覆盖直通、非空覆盖进入单根队列。

use super::super::building_placement_collision; // 建筑正式根与F12预览共用真实网格静态碰撞。
use super::super::components::{
    MapPlacementInstance, MapPlacementPrimitiveVisual, MapPlacementWaterEvidence,
    PlacementBuildingCollisionGeneration, PlacementInstanceGate, PlacementPendingInstance,
    PlacementPendingRevision, PlacementStagingRoot,
}; // 候选批次通过共享门保持作者身份原子切换。
use super::super::direct_primitive_requires_skinning; // 编辑器与正式候选共用完全相同的蒙皮安全判定。
use super::super::resources::{
    MapPlacementRuntimePhase, MapPlacementRuntimeState, MapPlacementRuntimeStatus,
    PendingPlacementPlan, PlacementActiveBatch, PlacementStaging,
}; // 候选构建游标、联合Ready信用与公开运行时状态。
use super::fail_revision; // 候选身份或容器失效时沿用整份修订失败语义。
use super::retirement::{queue_retired_tree, retire_plan_data}; // 原子采用后旧活动树与大计划分别O(1)转移到固定预算回收器。

mod reuse; // 未变对象保留真实实体及玩法子树，只增量转移容器归属。
pub(super) use reuse::PlacementReuseInputs;

const PLACEMENT_SPAWN_BUDGET_PER_FRAME: usize = 128; // 复杂Scene生成分帧排入隐藏根。
pub(super) const PLACEMENT_READY_VERIFY_BUDGET_PER_FRAME: usize = 128; // 最终Ready身份核对不得在一帧扫描全图根表。
const PLACEMENT_MARKER_CLEANUP_BUDGET_PER_FRAME: usize = 128; // 采用后物理隔离标记只做有界清理。

/// 依赖全部Ready后创建隐藏容器，并在同一候选事务内推进分帧生成和最终采用。
pub(super) fn advance_placement_candidate(
    commands: &mut Commands,
    asset_server: &AssetServer, // 单primitive在生成前非阻塞确认网格及原材质完整Ready。
    primitive_meshes: &Assets<Mesh>, // 已采用Mesh只做常数次属性查询，禁止裸生成蒙皮primitive。
    map_id: &MapId,
    state: &mut MapPlacementRuntimeState,
    status: &mut MapPlacementRuntimeStatus,
    building_clearances: &mut BuildingVegetationClearanceState,
    container_visibilities: &mut InstanceQuery<&mut Visibility, With<PlacementStagingRoot>>,
    reusable_roots: &PlacementReuseInputs,
) {
    if state.staging.is_none() {
        let pending = state.pending.as_ref().expect("上方已确认候选计划存在");
        let gate = PlacementInstanceGate::new(false); // 候选根从创建到整批采用前共享同一个关闭作者门。
        let root = commands
            .spawn((
                Name::new(format!("地图显式实例候选_{map_id}")),
                PlacementStagingRoot,
                Transform::IDENTITY,
                Visibility::Hidden,
                RigidBody::Static,
                RigidBodyDisabled, // 候选期所有内置盒均无玩法碰撞；原子采用时才解除共享刚体门。
                bevy_tnua::TnuaNotPlatform, // Tnua按ColliderOf读取共享刚体门，隐藏候选不参与角色悬浮地面探测。
            ))
            .id();
        state.staging = Some(PlacementStaging {
            reuse: Default::default(),
            terrain_roots: Vec::new(),
            root,
            roots: Vec::with_capacity(pending.plans.len()), // 已受全图8192项硬门限制，采用后直接移动给活动批次。
            gate,
            generation: pending.generation,
            revision: pending.revision,
            building_preset_revision: pending.building_preset_revision, // 候选采用前持续绑定原目录快照。
            building_clearances: pending.building_clearances.clone(), // 完整数组只增加一个Arc引用。
            building_clearance_floor_heights: pending.building_clearance_floor_heights.clone(),
            next_plan: 0,
            ready_roots: Default::default(), // Entity去重集合最多增长到已校验的全图placement总数。
            blocked_scene_assets: Default::default(), // 首次候选没有尚待根级扫尾的Scene epoch。
            total_instances: pending.plans.len(),
            container_observed: false, // Commands预留Entity不算已生成，至少等下一帧Query确认容器存在。
            eligibility_epoch: 0,
            verification_epoch: 0,
            verification_cursor: 0,
        });
        status.advance_progress(); // 全部Scene依赖已Ready并建立了真实隐藏候选容器。
    }
    if !reuse::index_active_roots(state, status, reusable_roots) { return; }
    if let Err(detail) = spawn_placement_batch(
        // 任一蒙皮或加载错误都沿用整批失败托底。
        commands,         // 所有实体修改仍留在主世界。
        asset_server,     // 非阻塞检查两个精确子资产句柄。
        primitive_meshes, // 最终生成前读取当前Mesh顶点布局。
        state,            // 当前唯一候选事务。
        status,           // 只在真实进展或失败时更新公开诊断。
        reusable_roots,
    ) {
        fail_revision(state, status, detail); // 任一子资产失败保留上一完整活动批次。
        return;
    }
    finish_placement_candidate(
        commands,
        state,
        status,
        building_clearances,
        container_visibilities,
    );
}

/// 直接primitive的Mesh和StandardMaterial必须同时完整就绪，不能先显示白模或空网格。
fn primitive_assets_ready(
    asset_server: &AssetServer,
    primitive_meshes: &Assets<Mesh>, // Loaded状态与主世界Assets采用可能相差一帧。
    primitive: &super::super::resources::PendingPlacementPrimitive,
) -> Result<bool, String> {
    for (id, label) in [
        (primitive.mesh.id().untyped(), "primitive网格"),
        (primitive.material.id().untyped(), "primitive原关联材质"),
    ] {
        let Some((direct, dependencies, recursive)) = asset_server.get_load_states(id) else {
            return Ok(false); // 刚登记的子资产尚未进入状态表，下帧继续固定预算轮询。
        };
        if matches!(direct, LoadState::Failed(_))
            || matches!(dependencies, DependencyLoadState::Failed(_))
            || matches!(recursive, RecursiveDependencyLoadState::Failed(_))
        {
            return Err(format!("显式摆放的{label}加载失败")); // 不用默认材质掩盖坏编号或损坏依赖。
        }
        if !asset_server.is_loaded_with_dependencies(id) {
            return Ok(false); // 一个计划最多检查两个固定句柄，不扫描GLTF目录。
        }
    }
    if !primitive_meshes.contains(primitive.mesh.id()) {
        // 句柄完整Loaded后仍等待主世界Mesh实际可读。
        return Ok(false); // 不把短暂采用窗口误报成损坏或蒙皮失败。
    } // 下一帧继续检查当前精确计划，不跳过生成顺序。
    Ok(true)
}

/// 生成一条依赖已经完整Ready的候选根；返回值说明是否可立即获得Ready信用。
fn spawn_ready_placement(
    commands: &mut Commands,
    primitive_meshes: &Assets<Mesh>, // 直接primitive在任何调用路径中都必须先通过顶点布局复验。
    map_id: &MapId,
    plan: &PendingPlacementPlan,
    staging_root: Entity,
    gate: &PlacementInstanceGate,
    generation: u32,
    revision: u32,
) -> Result<(Entity, bool), String> {
    if plan.record.asset_target.is_builtin() {
        let root = commands.spawn((
            Name::new(format!("地图立方体_{}", plan.record.id)),
            plan.transform,
            Visibility::Inherited,
            MapPlacementInstance::new_pending(map_id, &plan.record, gate.clone()),
            PlacementPendingInstance,
            PlacementPendingRevision { generation, revision },
            ChildOf(staging_root),
        )).id();
        super::super::builtin::spawn_builtin_placement_visual(commands, root, plan.record.asset_target);
        return Ok((root, true)); // 固定拓扑与同源碰撞在命令落地时共同安装，无异步World或Trimesh依赖。
    }
    if let Some(primitive_asset) = plan.primitive.as_ref() {
        let (mesh, primitive) = match plan.record.asset_target {
            feathrise_map_content::MapPlacementAssetTarget::Primitive {
                mesh, primitive, ..
            } => (mesh, primitive),
            feathrise_map_content::MapPlacementAssetTarget::Scene0 | feathrise_map_content::MapPlacementAssetTarget::Cuboid { .. } | feathrise_map_content::MapPlacementAssetTarget::EditableCuboid { .. } | feathrise_map_content::MapPlacementAssetTarget::EditableMesh { .. } => {
                unreachable!("直接primitive句柄必须对应primitive作者目标")
            }
        };
        let primitive_mesh = primitive_meshes // 完整Ready后句柄必须已进入主世界Assets。
            .get(&primitive_asset.mesh) // 只读取当前一个候选Mesh，不扫描目录或顶点。
            .ok_or_else(|| "显式摆放的primitive网格尚未进入主世界Assets".to_owned())?;
        if direct_primitive_requires_skinning(primitive_mesh) {
            // JOINTS与WEIGHTS会令PBR管线要求Skin绑定组。
            return Err(format!(
                // 保留上一完整活动批次并公开精确坏记录身份。
                "显式摆放记录{}的Mesh{mesh}/Primitive{primitive}是蒙皮网格；请改用完整Scene0", // 直接primitive格式不携带Skin、关节或逆绑定矩阵。
                plan.record.id, // 作者可回F12场景树定位并删除或改为完整场景。
            )); // 任何Mesh3d实体生成前终止，避免GPU校验错误退出应用。
        } // 静态primitive继续走唯一可视子实体路径。
        let mut root_commands = commands.spawn((
            Name::new(format!("地图子资产实例_{}", plan.record.id)),
            plan.transform,
            Visibility::Inherited, // 候选期继承隐藏容器，整批采用后自动同时显示。
            MapPlacementInstance::new_pending(map_id, &plan.record, gate.clone()),
            PlacementPendingInstance,
            PlacementPendingRevision {
                generation,
                revision,
            },
            ChildOf(staging_root),
        ));
        let is_building =
            plan.record.building_preset_id.is_some() || plan.record.terrain_patch.is_some();
        if let Some(collision) = building_placement_collision(&plan.record) {
            root_commands
                .insert(collision)
                .insert(PlacementBuildingCollisionGeneration::default()) // World与碰撞边沿从零代开始配对。
                .observe(super::super::settlement::on_placement_building_collision_ready);
        }
        if let Some(collision) = super::super::terrain_placement_collision(&plan.record) {
            root_commands
                .insert(collision)
                .insert(PlacementBuildingCollisionGeneration::default())
                .observe(super::super::settlement::on_placement_building_collision_ready);
        }
        root_commands.with_child((
            Name::new(format!("地图子资产_Mesh{mesh}_Primitive{primitive}")),
            MapPlacementPrimitiveVisual, // 直接网格没有Scene0层级，视口据此只选择所属完整placement根。
            Mesh3d(primitive_asset.mesh.clone()), // 直接引用完整验证过的精确子网格。
            MeshMaterial3d(primitive_asset.material.clone()), // 保留预览选择时冻结的原/std材质。
            Transform::IDENTITY,         // 与单体预览一致，不继承源Scene0节点变换。
            Visibility::Inherited,       // 只跟随当前placement根，不越过隐藏候选原子门。
        ));
        Ok((root_commands.id(), !is_building)) // 建筑primitive还必须等待Avian层级碰撞完成。
    } else {
        let has_water_surfaces = !plan.record.water_surfaces.is_empty();
        let mut root_commands = commands.spawn((
            Name::new(format!("地图实例_{}", plan.record.id)),
            WorldAssetRoot(plan.scene.clone()),
            plan.transform,
            MapPlacementInstance::new_pending(map_id, &plan.record, gate.clone()),
            DynamicWaterMeshOverrides::new(plan.record.water_surfaces.clone()), // 空数组也保留热改入口，新值不复制World实例。
            PlacementPendingInstance,
            PlacementPendingRevision {
                generation,
                revision,
            },
            ChildOf(staging_root),
        ));
        if let Some(collision) = building_placement_collision(&plan.record) {
            root_commands
                .insert(collision)
                .insert(PlacementBuildingCollisionGeneration::default()) // Scene热更后必须等待同代新碰撞。
                .observe(super::super::settlement::on_placement_building_collision_ready);
        }
        if let Some(collision) = super::super::terrain_placement_collision(&plan.record) {
            root_commands
                .insert(collision)
                .insert(PlacementBuildingCollisionGeneration::default())
                .observe(super::super::settlement::on_placement_building_collision_ready);
        }
        if has_water_surfaces {
            root_commands.insert(MapPlacementWaterEvidence); // 保存等待只查询全图最多256项非空水体根。
        }
        let entity = root_commands
            .observe(super::super::settlement::on_placement_instance_ready) // 实例完成只写门标记，下一Update再与赋水结果合并结算。
            .observe(super::super::settlement::on_placement_progress) // 水体增量任务只用定向真实推进脉冲续接同一候选门。
            .id();
        Ok((entity, false)) // Scene0必须等待World与可选水体联合门发送真实Ready信用。
    }
}

/// 每帧只向隐藏根排入固定数量场景实例。
fn spawn_placement_batch(
    commands: &mut Commands,
    asset_server: &AssetServer,
    primitive_meshes: &Assets<Mesh>, // 每个计划最多查询一个已经Ready的Mesh资产。
    state: &mut MapPlacementRuntimeState,
    status: &mut MapPlacementRuntimeStatus,
    reusable_roots: &PlacementReuseInputs,
) -> Result<(), String> {
    let active = state.active.as_ref();
    let (Some(pending), Some(staging)) = (state.pending.as_ref(), state.staging.as_mut()) else {
        return Ok(());
    };
    let end = (staging.next_plan + PLACEMENT_SPAWN_BUDGET_PER_FRAME).min(pending.plans.len());
    let mut next_plan = staging.next_plan; // 子资产等待时只停在当前精确计划，不跳过生成顺序。
    while next_plan < end {
        let plan = &pending.plans[next_plan];
        if let Some((entity, ready)) = reuse::reuse_placement(commands, &pending.map_id, plan, staging, active, reusable_roots) {
            if ready { staging.ready_roots.insert(entity); }
            staging.roots.push(entity);
            next_plan += 1;
            continue;
        }
        if let Some(primitive) = plan.primitive.as_ref()
            && !primitive_assets_ready(asset_server, primitive_meshes, primitive)?
        {
            break; // 异步句柄尚未完整Ready，隐藏候选和旧活动批次都保持原状。
        }
        let (entity, immediately_ready) = spawn_ready_placement(
            commands,
            primitive_meshes, // 生成函数自身仍执行最终蒙皮安全门，不能依赖调用方约定。
            &pending.map_id,
            plan,
            staging.root,
            &staging.gate,
            staging.generation,
            staging.revision,
        )?; // Scene0与静态primitive共享稳定根身份，蒙皮primitive在生成前使整批失败。
        if immediately_ready {
            staging.ready_roots.insert(entity); // 两个依赖已Ready且无Scene水体事务，可直接取得候选信用。
        }
        staging.roots.push(entity); // 采用时不扫描世界，只解除本候选已知根的隔离标记。
        if plan.record.terrain_patch.is_some() {
            staging.terrain_roots.push(entity);
        }
        next_plan += 1; // 根及其全部句柄/observer命令完整建立后才推进游标。
    }
    if next_plan > staging.next_plan {
        staging.next_plan = next_plan;
        status.advance_progress(); // 一帧排入一个或多个根只记一次真实生成游标推进。
    }
    Ok(())
}

/// 所有场景都已排入且实例化完成后，分帧复验根表并以O(1)批次门原子采用。
pub(super) fn finish_placement_candidate(
    commands: &mut Commands,
    state: &mut MapPlacementRuntimeState,
    status: &mut MapPlacementRuntimeStatus,
    building_clearances: &mut BuildingVegetationClearanceState,
    container_visibilities: &mut InstanceQuery<&mut Visibility, With<PlacementStagingRoot>>,
) {
    let world_asset_events_caught_up = state.world_asset_events_caught_up; // 先冻结全局消息门，避免与候选可变借用重叠。
    let Some(staging) = state.staging.as_mut() else {
        return;
    };
    status.phase = MapPlacementRuntimePhase::Building;
    status.detail = format!(
        "正在隐藏生成显式实例：已排入{}/{}，就绪{}/{}，复验{}/{}",
        staging.next_plan,
        staging.total_instances,
        staging.ready_roots.len(),
        staging.total_instances,
        staging.verification_cursor,
        staging.total_instances,
    );
    if !staging.container_observed {
        if container_visibilities.get_mut(staging.root).is_err() {
            return; // 尤其零placement首帧，Commands::spawn尚未apply，不能误报容器失效。
        }
        staging.container_observed = true; // 后续同一Entity若消失才属于真实生命周期错误。
        status.advance_progress(); // Commands预留Entity首次变成可查询容器是独立真实进展。
    }
    if staging.next_plan < staging.total_instances
        || staging.roots.len() != staging.total_instances
        || staging.ready_roots.len() != staging.total_instances
        || !staging.blocked_scene_assets.is_empty()
        || !world_asset_events_caught_up
    {
        staging.verification_cursor = 0; // 尚未全Ready时不保留旧轮次的前缀信用。
        staging.verification_epoch = staging.eligibility_epoch;
        return;
    }
    if staging.verification_epoch != staging.eligibility_epoch {
        staging.verification_epoch = staging.eligibility_epoch;
        staging.verification_cursor = 0; // 任一热更事件都让已经检查的前缀失效，必须整批重新读取。
    }
    let end = (staging.verification_cursor + PLACEMENT_READY_VERIFY_BUDGET_PER_FRAME)
        .min(staging.total_instances);
    if staging.roots[staging.verification_cursor..end]
        .iter()
        .any(|root| !staging.ready_roots.contains(root))
    {
        staging.verification_cursor = 0;
        staging.verification_epoch = staging.eligibility_epoch;
        return; // 每帧只核对固定前缀；异常Entity不能靠相同集合长度冒充当前根。
    }
    if end > staging.verification_cursor {
        staging.verification_cursor = end;
        status.advance_progress(); // 最终根表复验按固定预算推进，长事务持续提供真实heartbeat。
    }
    if end < staging.total_instances {
        return; // 大地图跨多帧完成稳定Ready证明，候选和活动门仍保持关闭。
    }
    let (generation, revision, staging_root) = (staging.generation, staging.revision, staging.root); // 释放候选借用后核对当前输入事务。
    let pending_matches = state.pending.as_ref().is_some_and(|pending| {
        pending.generation == generation
            && pending.revision == revision
            && state.active_map_id.as_ref() == Some(&pending.map_id)
    });
    if !pending_matches {
        fail_revision(
            state,
            status,
            "显式实例候选完成时地图或修订身份已经换代".to_owned(),
        );
        return; // 换图或新JSON修订绝不能采用刚完成复验的旧批次。
    }
    if state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.terrain_holes != state.terrain_holes_applied)
    {
        // 候选 Mesh/碰撞已完整 Ready，下一轮地形后台任务才有权打开或关闭洞口。
        // 保留旧实例容器，地形发布系统与本系统按顺序在同一 Update 中完成切换。
        return;
    }
    if state.pending.as_ref().is_some_and(|pending| {
        (pending.terrain_patch_count > 0 || state.terrain_patch_active)
            && state.terrain_mesh_generation != Some([generation, revision])
    }) {
        return;
    }
    if container_visibilities.get_mut(staging_root).is_err() {
        fail_revision(
            state,
            status,
            "显式实例隐藏候选容器在原子采用前已经失效".to_owned(),
        );
        return; // 容器被外部删除时保留旧完整活动批次并发布可见失败。
    }
    let adopted_map_id = state
        .pending
        .as_ref()
        .expect("候选根必须对应完整计划")
        .map_id
        .clone(); // 先保留有界地图身份，计划与Scene集合随后整体移入退役槽。
    let staging = state.staging.take().expect("候选已经满足完整采用门");
    state.terrain_patch_active = state
        .pending
        .as_ref()
        .is_some_and(|pending| pending.terrain_patch_count > 0);
    // 形状已在后台完成。洞口、查询和新容器在本帧共同发布，候选期间没有隐形地形碰撞。
    for &root in &staging.terrain_roots {
        commands
            .entity(root)
            .insert(super::super::terrain_collision::InstallTerrainPatchCollider);
    }
    retire_plan_data(state); // 采用只撤销计划身份并O(1)转移所有权，不在本帧析构最多八千一百九十二项。
    if let Some((carry, _)) = staging.reuse.carry {
        // 完整复验后只转挂一个Identity父节点，所有未变实体/子树在同一采用点保持原状态。
        commands.entity(carry).insert(ChildOf(staging.root));
    }
    if let Some(old) = state.active.take() {
        old.gate.retire(); // 先O(1)永久撤销旧批次作者与保存证据身份。
        super::super::terrain_collision::retire_formal_terrain(commands, &old.terrain_roots);
        if let Ok(mut visibility) = container_visibilities.get_mut(old.root) {
            *visibility = Visibility::Hidden; // 同一系统内直接隐藏旧容器，不让公开Ready领先延迟命令。
        }
        queue_retired_tree(commands, state, old.root, old.roots); // 旧树只转移所有权，后续exclusive系统逐边清理。
    }
    *container_visibilities
        .get_mut(staging.root)
        .expect("上方已确认候选容器存在") = Visibility::Inherited;
    staging.gate.activate(); // 新容器可见后一次Release让全部根同时成为F12正式目标。
    commands.entity(staging.root).remove::<(RigidBodyDisabled, bevy_tnua::TnuaNotPlatform)>(); // 只解除共享刚体，内置盒整批同时进入玩法物理和角色地面探测。
    state.active = Some(PlacementActiveBatch {
        building_preset_revision: staging.building_preset_revision,
        terrain_roots: staging.terrain_roots,
        root: staging.root,
        roots: staging.roots, // Vec O(1)移动；物理标记由独立固定预算系统清理。
        gate: staging.gate,
        marker_cleanup_cursor: 0,
    });
    state.failure_logged_revision = None;
    status.phase = MapPlacementRuntimePhase::Ready;
    status.revision = status.revision.wrapping_add(1);
    building_clearances.set_formal_layered(
        adopted_map_id.clone(),           // 净空只服务本次原子采用的地图。
        status.revision,                  // 与公开placement采用修订保持同一提交点。
        staging.building_preset_revision, // 后台消费者可拒绝旧目录派生结果。
        staging.building_clearances,      // Arc完整移交，不复制全部四边形。
        staging.building_clearance_floor_heights,
    );
    status.advance_progress(); // 原子采用是本候选最后一次真实进展。
    status.placement_count = staging.total_instances;
    status.detail = format!("地图{adopted_map_id}显式实例已原子应用");
    info!(
        "{}：{}项，修订{}",
        status.detail, status.placement_count, status.revision
    );
}

/// 已采用批次的物理Pending标记只做固定预算清理；公开作者身份已经由共享门同时切换。
pub(crate) fn cleanup_active_placement_markers(
    mut commands: Commands,
    mut state: InstanceResMut<MapPlacementRuntimeState>,
) {
    let Some(active) = state.active.as_mut() else {
        return;
    };
    let end = (active.marker_cleanup_cursor + PLACEMENT_MARKER_CLEANUP_BUDGET_PER_FRAME)
        .min(active.roots.len());
    for &root in &active.roots[active.marker_cleanup_cursor..end] {
        commands
            .entity(root)
            .try_remove::<(PlacementPendingInstance, PlacementPendingRevision)>(); // 根被外部提前移除时保持幂等，不影响其余批次。
    }
    active.marker_cleanup_cursor = end;
}

#[cfg(test)]
#[path = "candidate/tests.rs"]
mod tests; // Scene0与直接primitive实体形态用独立ECS夹具锁定。

#[cfg(test)]
#[path = "candidate/reuse_tests.rs"]
mod reuse_tests;
