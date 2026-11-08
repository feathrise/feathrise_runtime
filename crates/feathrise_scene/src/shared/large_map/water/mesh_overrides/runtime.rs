//! placement整网格水体的有界ECS恢复、预检与应用。

use super::*; // 子模块复用父模块的私有预算、快照、编码与公开根类型。
use crate::shared::placement_data::MapPlacementProgressPulse; // 候选保存门只接收真实游标或阶段推进脉冲。
use crate::shared::world_asset_lifecycle::{
    WorldAssetLifecycleIndex, WorldAssetRootEpochInvalidated, WorldAssetRootRegistration,
}; // 同InstanceId原地热重载必须用资产epoch撤销并重新授信。

mod height_offset; // 世界Y米制高度沿局部父链有界反算，不读取可能尚未传播的GlobalTransform。
mod prepare; // single-flight候选按作者、层级、几何和材质阶段固定预算推进。
mod process; // 最终身份复验、single-flight推进与原子提交独立保持低于500行。
mod restore; // 原材质/位姿恢复和独占动态材质回收保持在独立生命周期模块。
mod uv_validation; // POSITION/UV0有限值与均匀三角证据使用跨帧游标。

pub(in crate::shared::large_map::water) use prepare::DynamicWaterMeshOverrideWorkState; // 上层插件初始化唯一single-flight工作资源。
use prepare::{
    MeshWaterRootWork, MeshWaterWorkStage, advance_preparation, discard_temporary_materials,
}; // 处理系统只推进一个活动根，并在全量成功后进入Commit。
pub(in crate::shared::large_map::water) use process::process_pending_dynamic_water_mesh_override;
use restore::restore_applied_primitives; // 新快照预检成功后才恢复上轮成功接管。

/// 把任意Blender基础材质规范化为项目水面透明PBR边界，同时保留其RGB、纹理与其余PBR输入。
pub(super) fn configure_whole_mesh_water_material(base: &mut StandardMaterial) {
    const DEFAULT_WATER_ALPHA: f32 = 0.72; // 与动态地图默认水色Alpha一致，普通Opaque材质赋水后也能直接成为透明水。
    let authored_alpha = base.base_color.alpha(); // 先结束不可变读取，再独占修改同一颜色值。
    base.base_color
        .set_alpha(authored_alpha.min(DEFAULT_WATER_ALPHA)); // 作者已经更透明时原样保留，默认不透明材质则降到统一水面上限。
    base.alpha_mode = AlphaMode::Blend; // “赋水”必须实际进入透明前向PBR，不能保留Opaque实体板外观。
    base.unlit = false; // 水面必须接收环境与方向光PBR反射，原对象的Unlit开关不延续到赋水结果。
    base.metallic = 0.0; // 水是介电材质；Blender对象原本的金属值不应污染赋水结果。
    base.perceptual_roughness = base
        .perceptual_roughness
        .min(DEFAULT_MAP_WATER_PERCEPTUAL_ROUGHNESS); // 保留作者更光滑设定，并让普通Blender材质主要读取清晰天空Mip0/1。
    base.double_sided = true; // 瀑布和倾斜水面可能从两侧观察。
    base.cull_mode = None; // 与既有绘制水面一致，避免水幕背面整片消失。
    base.depth_bias = WATER_TRANSPARENT_SORT_DEPTH_BIAS; // 复用正式水面透明排序边界。
}
pub(in crate::shared::large_map::water) use restore::{
    RetiredMeshWaterMaterials, cleanup_removed_mesh_override_assets,
    restore_removed_dynamic_water_mesh_overrides, retire_removed_mesh_override_materials,
}; // 水插件注册移除恢复与独占资产清理observer。

/// 把已完整预检的primitive一起切换到现有GPU水材质，原Mesh3d保持不变。
fn apply_prepared_primitives(
    root: Entity,                     // 新恢复记录的唯一所有者。
    prepared: Vec<PreparedPrimitive>, // 动态材质已分帧登记且本函数开始后无可失败步骤。
    commands: &mut Commands,          // 所有切换命令在系统尾部一起应用。
) {
    let mut applied_entities = Vec::with_capacity(prepared.len()); // 成功数量已受根primitive上限约束。
    for prepared in prepared {
        let candidate = prepared.candidate; // 拥有原恢复快照，不再借用预检数组。
        let material_handle = prepared.dynamic_material; // 取得此前按预算创建且从未挂到实体的独占材质。
        let mut transform = candidate
            .original_transform
            .expect("全量预检已确认primitive存在Transform"); // 应用期只消费已验证完成值。
        transform.translation += prepared.local_translation_offset; // 世界Y米制高度已反算到父坐标，旋转或缩放层级下也不发生斜移或倍率变化。
        let original_material = candidate
            .original_material
            .expect("全量预检已确认primitive存在StandardMaterial"); // 原PBR句柄只用于恢复。
        commands
            .entity(candidate.entity) // 修改实际glTF primitive，不在根旁创建另一张水面。
            .remove::<MeshMaterial3d<StandardMaterial>>() // 同一实体仅保留一个具体材质所有者。
            .remove::<NotShadowReceiver>() // 接管期间强制接收方向光阴影，让山体遮掉太阳/月亮直射高光。
            .insert((
                MeshMaterial3d(material_handle.clone()), // 复用项目唯一DynamicWaterMaterial管线。
                transform, // 只应用当前作者高度偏移后的primitive局部位姿。
                DynamicWaterSurface {
                    interaction_footprint: WaterInteractionFootprint::Disabled, // 复杂Blender网格本阶段禁用Legacy平面足迹，避免错误入水。
                    rest_water_level: 0.0, // Disabled足迹不读水位，保留有限占位值。
                    flow_direction: prepared.flow_style_encoding, // 每帧更新继续保留强制整网格uniform哨兵。
                    flow_speed: prepared.flow_speed,              // 逐帧材质同步使用当前作者流速。
                    player_ripples: Vec::new(), // Disabled纯视觉水面不分配八槽CPU池，也不进入逐帧互动查询。
                    last_player_takeoff_sequence: None, // 禁用互动时不消费玩家起跳身份。
                    previous_player_local_xz: None, // 禁用互动时不维护脚底水平轨迹。
                    previous_player_local_y: None, // 禁用互动时不维护脚底竖直轨迹。
                    distance_since_ripple: 0.0, // 禁用互动时不累积行走发波距离。
                    player_was_in_water: false, // 禁用互动时永不进入Legacy入水边沿。
                    jump_cycle: WaterJumpCycle::Idle, // 禁用互动时不武装起落水周期。
                }, // 完成既有GPU水面更新组件构造。
                NoFrustumCulling, // GPU顶点位移后仍保持渲染。
                CameraRaycastIgnore, // 复杂水面不阻挡相机。
                NotShadowCaster, // 透明水不给地表投射整块阴影。
                DynamicWaterMeshOverridePrimitive {
                    owner_root: root,  // 锁定当前placement根所有权。
                    original_material, // 保留Blender原StandardMaterial强句柄。
                    original_transform: candidate.original_transform.expect("预检后位姿必须存在"), // 保留未加高度偏移的原局部位姿。
                    dynamic_material: material_handle, // 原Mesh3d不变，只记录独占扩展材质供精确释放。
                    had_no_frustum_culling: candidate.had_no_frustum_culling, // 保留原裁剪标记状态。
                    had_camera_raycast_ignore: candidate.had_camera_raycast_ignore, // 保留原相机射线标记状态。
                    had_not_shadow_caster: candidate.had_not_shadow_caster, // 保留原投影标记状态。
                    had_not_shadow_receiver: candidate.had_not_shadow_receiver, // 保留原接影标记状态。
                }, // 完成内部恢复记录。
            )); // 完成当前primitive材质、位姿与恢复身份的同批切换，原Mesh3d组件完全不动。
        applied_entities.push(candidate.entity); // 根只索引本轮真实接管的受限primitive。
    } // 完成全部预检完成值应用。
    let mut root_commands = commands.entity(root); // 根级结果与全部子实体切换处于同一命令队列。
    if applied_entities.is_empty() {
        root_commands
            .remove::<DynamicWaterMeshOverrideApplied>() // 空覆盖不保留无意义实体表。
            .insert(DynamicWaterMeshOverrideStatus::Ready); // 明确放行普通placement候选。
    } else {
        root_commands.insert((
            DynamicWaterMeshOverrideApplied {
                entities: applied_entities, // 热改和撤销直接按有界表恢复，不扫描世界。
            }, // 完成根已接管索引。
            DynamicWaterMeshOverrideStatus::Ready, // 全部primitive已经通过预检并排入切换。
        )); // 完成根级成功状态发布。
    } // 完成空/非空应用结果分支。
} // 结束预检完成值应用。

/// 把已ready根标为立即成功或加入固定容量队列；队列永远不复制作者快照。
fn queue_root_or_mark_ready(
    root: Entity,                                       // 当前WorldAssetRoot稳定实体身份。
    instance_id: InstanceId,                            // 当前ready World实例绑定请求身份。
    ready_epoch: u64,                                   // 当前WorldReady对应的资产换代身份。
    overrides: &DynamicWaterMeshOverrides,              // 只判断空覆盖，处理期再读取最新完整值。
    has_applied_override: bool,                         // 空覆盖若仍有上轮水面就必须排队恢复。
    queue: &mut DynamicWaterMeshOverrideQueue,          // 所有ready与Changed请求共用唯一去重FIFO。
    work_state: &mut DynamicWaterMeshOverrideWorkState, // 活动同根Changed推进修订并取消旧游标。
    commands: &mut Commands, // 只发布O(1)根状态，不接触场景后代或Assets。
) {
    if !requires_pending_processing(overrides, has_applied_override) {
        if work_state.is_active_root(root) {
            let result = work_state.request(queue, root, instance_id, ready_epoch); // 活动根可能持有不可见临时材质，必须先按预算取消。
            debug_assert_eq!(result, MeshWaterEnqueueResult::Coalesced);
            commands
                .entity(root)
                .insert(DynamicWaterMeshOverrideStatus::Pending);
            return;
        }
        work_state.cancel_root(queue, root); // 等待中的旧请求和修订一起O(1)撤销。
        commands
            .entity(root) // 普通placement空覆盖不进入可能积压的水体队列。
            .insert(DynamicWaterMeshOverrideStatus::Ready); // World实例ready后立即放行联合候选门。
        return; // 没有旧接管可恢复，也没有新覆盖可预检。
    } // 完成空覆盖即时Ready快路径。
    match work_state.request(queue, root, instance_id, ready_epoch) {
        MeshWaterEnqueueResult::Queued | MeshWaterEnqueueResult::Coalesced => {
            commands
                .entity(root) // 同根多次Changed只刷新状态，FIFO仍保留原来的一项。
                .insert(DynamicWaterMeshOverrideStatus::Pending); // 旧成功水面或Blender原材质在等待期间保持不变。
        }
        MeshWaterEnqueueResult::Full => {
            let detail = "整网格水体待处理身份的内存分配失败".to_owned(); // 没有根数总门，只有真实分配失败。
            warn!("placement整网格水体入队失败：root={root:?} detail={detail}"); // 发布可定位的根级诊断。
            commands
                .entity(root) // 新根保留Blender材质，已有根继续显示上一成功水材质。
                .insert(DynamicWaterMeshOverrideStatus::Failed(detail)); // 候选placement据此拒绝整份修订。
        }
    } // 完成入队或失败关闭结果发布。
} // 结束根请求收集。

/// World实例完成后只做O(1)入队；空覆盖普通placement直接Ready。
pub(in crate::shared::large_map::water) fn initialize_dynamic_water_mesh_overrides(
    ready: On<WorldInstanceReady>, // 子层级与依赖资产已完整生成的根事件。
    roots: Query<
        (
            &DynamicWaterMeshOverrides,           // 当前最新整批作者快照。
            &WorldInstance,                       // 拒绝同根旧实例迟到ready事件。
            &WorldAssetRoot,                      // 资产ID定位同InstanceId热重载epoch。
            Has<DynamicWaterMeshOverrideApplied>, // 热重载或空覆盖清除是否仍需恢复上轮。
        ),
        With<WorldAssetRoot>,
    >, // 只命中显式赋水根，旧GLB白名单根不在此查询。
    mut queue: ResMut<DynamicWaterMeshOverrideQueue>, // 只登记轻量根实体，不遍历World层级。
    mut work_state: ResMut<DynamicWaterMeshOverrideWorkState>, // 同根新World实例立即使旧游标过期。
    mut asset_index: ResMut<WorldAssetLifecycleIndex>, // WorldReady把精确根授信到当前资产epoch。
    mut commands: Commands,        // 即时发布Pending、Ready或队列满Failed状态。
) {
    let Ok((overrides, instance, world_asset, has_applied_override)) = roots.get(ready.entity)
    else {
        return; // 未安装公开覆盖组件的旧正式GLB根保持原observer行为。
    }; // 完成显式赋水根读取。
    if **instance != ready.instance_id {
        return; // 热重载迟到事件不得重施到新World实例。
    } // 完成当前World实例ID核对。
    if !work_state.accept_world_instance(ready.entity, ready.instance_id) {
        return; // WorldAssetRoot刚替换但WorldInstance尚未换票时，旧Ready不得回流成新请求。
    }
    let ready_epoch = match asset_index.mark_world_ready(ready.entity, world_asset.0.id()) {
        Ok(epoch) => epoch,
        Err(WorldAssetRootRegistration::Full) => {
            commands
                .entity(ready.entity)
                .insert(DynamicWaterMeshOverrideStatus::Failed(
                    "WorldAsset整网格水根索引超过固定上限".to_owned(),
                ));
            return;
        }
        Err(_) => unreachable!("mark_world_ready只会返回容量失败"),
    };
    commands
        .entity(ready.entity)
        .insert(DynamicWaterMeshOverrideInstanceIdentity {
            instance_id: **instance,
            ready_epoch,
        }); // 后续Ready、Failed与进展都明确绑定本次实例和资产epoch。
    queue_root_or_mark_ready(
        ready.entity,
        **instance,
        ready_epoch,
        overrides,
        has_applied_override,
        &mut queue,
        &mut work_state,
        &mut commands,
    ); // 有覆盖根等待逐帧处理，普通空覆盖根立即Ready。
} // 结束World ready赋水入队observer。

/// `WorldAssetRoot`替换或移除当下撤销旧水Ready，不再使用稳定帧全根变化过滤Query。
pub(in crate::shared::large_map::water) fn invalidate_discarded_dynamic_water_mesh_world_asset(
    discard: On<Discard<WorldAssetRoot>>, // World spawner改写InstanceId之前先收到权威资产根生命周期事件。
    roots: Query<&WorldInstance, (With<DynamicWaterMeshOverrides>, With<WorldAssetRoot>)>, // 只读取发生Discard的精确整网格水根。
    mut queue: ResMut<DynamicWaterMeshOverrideQueue>, // 旧实例等待项立即O(1)摘链。
    mut work_state: ResMut<DynamicWaterMeshOverrideWorkState>, // 活动旧游标失效并冻结旧InstanceId。
    mut commands: Commands,                           // 根若继续存活则马上撤销公开Ready身份。
) {
    let Ok(instance) = roots.get(discard.entity) else {
        return; // 尚未产生WorldInstance或不属于整网格水功能域时无旧Ready可撤销。
    };
    work_state.block_world_instance(&mut queue, discard.entity, **instance); // 同帧旧Ready事件也会被明确旧票据拒绝。
    if discard.trigger().new_archetype.is_some() {
        commands
            .entity(discard.entity)
            .try_insert(DynamicWaterMeshOverrideStatus::Pending)
            .try_remove::<DynamicWaterMeshOverrideInstanceIdentity>(); // 新实例Ready前保存与候选门都不能复用旧信用。
    } // despawn路径不向即将消失的实体追加延迟命令。
} // 结束资产根Discard失效observer。

/// `WorldAssetRoot`真实移除时清理阻断票据与待处理身份；Replace不会触发本observer。
pub(in crate::shared::large_map::water) fn cancel_removed_dynamic_water_mesh_world_asset(
    remove: On<Remove<WorldAssetRoot>>, // Remove在旧覆盖组件仍可查询时提供精确根实体。
    roots: Query<(), With<DynamicWaterMeshOverrides>>, // 普通World根不接触整网格水队列。
    mut queue: ResMut<DynamicWaterMeshOverrideQueue>, // 待处理根O(1)取消。
    mut work_state: ResMut<DynamicWaterMeshOverrideWorkState>, // 释放换代阻断并让活动游标进入有界Discard。
) {
    if roots.get(remove.entity).is_ok() {
        work_state.forget_root(&mut queue, remove.entity); // 根不再等待新Ready，全部轻量身份立即释放。
    }
} // 结束资产根Remove清理observer。

/// 覆盖整组件Add/替换时只合并该根最新请求，稳定帧不扫描全部World根原型。
pub(in crate::shared::large_map::water) fn queue_inserted_dynamic_water_mesh_overrides(
    insert: On<Insert<DynamicWaterMeshOverrides>>, // Bevy 0.19 Insert同时覆盖首次Add与整组件替换。
    roots: Query<
        (
            &DynamicWaterMeshOverrides,           // 处理时仍会重读该组件的最新值。
            &WorldInstance,                       // 只接纳当前真正ready的实例。
            &WorldAssetRoot,                      // 结构Insert不得绕过当前资产epoch门。
            Has<DynamicWaterMeshOverrideApplied>, // 空覆盖是否必须排队恢复上轮成功水面。
        ),
        With<WorldAssetRoot>,
    >, // 只用`get(insert.entity)`读取一个发生生命周期事件的根。
    instance_spawner: Res<WorldInstanceSpawner>, // 后插入或热改覆盖只在当前World实例真正ready时收集。
    asset_index: Res<WorldAssetLifecycleIndex>,  // O(1)确认本根已取得当前WorldReady而非旧Ready。
    mut queue: ResMut<DynamicWaterMeshOverrideQueue>, // 同根重复Changed只占FIFO一项。
    mut work_state: ResMut<DynamicWaterMeshOverrideWorkState>, // Changed只推进轻量请求修订，不复制作者快照。
    mut commands: Commands, // O(1)发布Pending、Ready或队列满诊断。
) {
    let Ok((overrides, instance, world_asset, has_applied_override)) = roots.get(insert.entity)
    else {
        return; // 根组件早于WorldInstance存在时，由后续Ready observer读取最新覆盖。
    };
    if !instance_spawner.instance_is_ready(**instance) {
        return; // 尚未生成完整子层级时不启动任何水体预检。
    }
    if !work_state.accept_world_instance(insert.entity, **instance) {
        return; // 资产根换代间隙的旧实例覆盖写入不得绕过Discard阻断。
    }
    let Some(ready_epoch) = asset_index.current_ready_epoch(insert.entity, world_asset.0.id())
    else {
        work_state.cancel_root(&mut queue, insert.entity);
        commands
            .entity(insert.entity)
            .insert(DynamicWaterMeshOverrideStatus::Pending)
            .remove::<DynamicWaterMeshOverrideInstanceIdentity>();
        return; // 组件可先写入；只有后续当前epoch WorldReady能开始读取实例层级。
    };
    commands
        .entity(insert.entity)
        .insert(DynamicWaterMeshOverrideInstanceIdentity {
            instance_id: **instance,
            ready_epoch,
        });
    queue_root_or_mark_ready(
        insert.entity,
        **instance,
        ready_epoch,
        overrides,
        has_applied_override,
        &mut queue,
        &mut work_state,
        &mut commands,
    ); // 多次Insert只推进轻量修订，FIFO仍只保存一项Entity。
} // 结束整网格赋水Insert收集observer。

/// 资产epoch定向扫到根时立即撤销旧信用；积压期已经生成的新World则以当前epoch重新排队。
pub(in crate::shared::large_map::water) fn invalidate_dynamic_water_mesh_asset_epoch(
    invalidated: On<WorldAssetRootEpochInvalidated>,
    roots: Query<(
        &DynamicWaterMeshOverrides,
        &WorldInstance,
        &WorldAssetRoot,
        Has<DynamicWaterMeshOverrideApplied>,
    )>,
    instance_spawner: Res<WorldInstanceSpawner>,
    asset_index: Res<WorldAssetLifecycleIndex>,
    mut queue: ResMut<DynamicWaterMeshOverrideQueue>,
    mut work_state: ResMut<DynamicWaterMeshOverrideWorkState>,
    mut commands: Commands,
) {
    let Ok((overrides, instance, world_asset, has_applied_override)) =
        roots.get(invalidated.entity)
    else {
        return;
    };
    if world_asset.0.id() != invalidated.asset_id {
        return; // 根在有界扫到前已经迁移到另一份GLB，旧资产事件无权修改它。
    }
    work_state.cancel_root(&mut queue, invalidated.entity);
    let ready_epoch = asset_index.current_ready_epoch(invalidated.entity, invalidated.asset_id);
    if !invalidated.ready_after_event
        || ready_epoch != Some(invalidated.epoch)
        || !instance_spawner.instance_is_ready(**instance)
    {
        commands
            .entity(invalidated.entity)
            .try_insert(DynamicWaterMeshOverrideStatus::Pending)
            .try_remove::<DynamicWaterMeshOverrideInstanceIdentity>();
        return;
    }
    commands
        .entity(invalidated.entity)
        .try_insert(DynamicWaterMeshOverrideInstanceIdentity {
            instance_id: **instance,
            ready_epoch: invalidated.epoch,
        });
    queue_root_or_mark_ready(
        invalidated.entity,
        **instance,
        invalidated.epoch,
        overrides,
        has_applied_override,
        &mut queue,
        &mut work_state,
        &mut commands,
    ); // 被Bevy debounce抑制时重建当前旧Scene；积压期新Ready则重建实际新Scene。
}

/// 一次性O(1)数值补丁只改冻结索引的一项，再沿与整组件Insert相同的Pending/重建链推进。
pub(in crate::shared::large_map::water) fn apply_dynamic_water_mesh_numeric_patch(
    insert: On<Insert<DynamicWaterMeshOverrideNumericPatch>>,
    mut roots: Query<(
        &DynamicWaterMeshOverrideNumericPatch,
        &mut DynamicWaterMeshOverrides,
        &WorldInstance,
        &WorldAssetRoot,
        Has<DynamicWaterMeshOverrideApplied>,
    )>,
    instance_spawner: Res<WorldInstanceSpawner>,
    asset_index: Res<WorldAssetLifecycleIndex>,
    mut queue: ResMut<DynamicWaterMeshOverrideQueue>,
    mut work_state: ResMut<DynamicWaterMeshOverrideWorkState>,
    mut commands: Commands,
) {
    let Ok((patch, mut overrides, instance, world_asset, has_applied_override)) =
        roots.get_mut(insert.entity)
    else {
        commands
            .entity(insert.entity)
            .try_remove::<DynamicWaterMeshOverrideNumericPatch>();
        return;
    };
    if let Err(detail) = overrides.apply_numeric_patch(*patch) {
        work_state.cancel_root(&mut queue, insert.entity);
        commands
            .entity(insert.entity)
            .insert(DynamicWaterMeshOverrideStatus::Failed(detail))
            .remove::<DynamicWaterMeshOverrideNumericPatch>();
        return; // 无效补丁已经在组件内回滚旧数值，不污染作者快照。
    }
    commands
        .entity(insert.entity)
        .remove::<DynamicWaterMeshOverrideNumericPatch>(); // observer完成后不保留逐帧扫描标记。
    let Some(ready_epoch) = asset_index.current_ready_epoch(insert.entity, world_asset.0.id())
    else {
        work_state.cancel_root(&mut queue, insert.entity);
        commands
            .entity(insert.entity)
            .insert(DynamicWaterMeshOverrideStatus::Pending)
            .remove::<DynamicWaterMeshOverrideInstanceIdentity>();
        return; // 数值已写入，后续WorldReady会读取最新值并开始重建。
    };
    if !instance_spawner.instance_is_ready(**instance)
        || !work_state.accept_world_instance(insert.entity, **instance)
    {
        return;
    }
    commands
        .entity(insert.entity)
        .insert(DynamicWaterMeshOverrideInstanceIdentity {
            instance_id: **instance,
            ready_epoch,
        });
    queue_root_or_mark_ready(
        insert.entity,
        **instance,
        ready_epoch,
        &overrides,
        has_applied_override,
        &mut queue,
        &mut work_state,
        &mut commands,
    );
}

#[cfg(test)]
mod tests {
    use bevy::world_serialization::{
        WorldAsset, WorldAssetRoot, WorldInstance, WorldInstanceSpawner, world_instance_spawner,
    };

    use super::*;

    #[test]
    fn replacing_ready_world_instance_immediately_invalidates_old_water_ready() {
        let mut app = App::new();
        app.init_resource::<WorldInstanceSpawner>()
            .init_resource::<DynamicWaterMeshOverrideQueue>()
            .init_resource::<DynamicWaterMeshOverrideWorkState>()
            .add_observer(invalidate_discarded_dynamic_water_mesh_world_asset)
            .add_observer(cancel_removed_dynamic_water_mesh_world_asset)
            .add_systems(Update, world_instance_spawner); // 生命周期observer在spawner改写InstanceId之前已经同步撤销旧Ready。
        let root = app
            .world_mut()
            .spawn((
                WorldAssetRoot(Handle::<WorldAsset>::default()),
                DynamicWaterMeshOverrides::default(),
            ))
            .id();
        app.update(); // 首次生成WorldInstance并建立内部实例身份。
        let first_instance = **app
            .world()
            .get::<WorldInstance>(root)
            .expect("首次world spawner必须写入实例票据");
        app.world_mut().entity_mut(root).insert((
            DynamicWaterMeshOverrideInstanceIdentity {
                instance_id: first_instance,
                ready_epoch: 0,
            },
            DynamicWaterMeshOverrideStatus::Ready,
        )); // 模拟旧GLB已经完成整网格水接管。

        app.world_mut()
            .entity_mut(root)
            .insert(WorldAssetRoot(Handle::<WorldAsset>::default())); // 即使句柄相同，Changed也会生成唯一新InstanceId。
        app.update(); // 新实例尚未进入spawner ready表，旧Ready必须在这一帧失效。

        let world = app.world();
        let second_instance = **world
            .get::<WorldInstance>(root)
            .expect("换代后根仍必须携带新实例票据");
        assert_ne!(first_instance, second_instance);
        assert_eq!(
            world.get::<DynamicWaterMeshOverrideStatus>(root),
            Some(&DynamicWaterMeshOverrideStatus::Pending)
        ); // 保存/候选不能在新WorldInstanceReady前复用旧实例的Ready。
        assert_eq!(
            world
                .get::<DynamicWaterMeshOverrideInstanceIdentity>(root)
                .map(|identity| identity.instance_id),
            None
        ); // 新WorldInstanceReady尚未到达，旧身份必须保持撤销而不能提前绑定新票据。
    }

    #[test]
    fn stable_collection_has_no_changed_query_over_placement_roots() {
        let source = include_str!("runtime.rs"); // 把稳定帧复杂度契约锁定在实际生产收集模块源码。
        assert!(!source.contains(concat!("Changed<", "WorldInstance>"))); // World换代必须由WorldAssetRoot生命周期observer定向触发。
        assert!(!source.contains(concat!("Changed<", "DynamicWaterMeshOverrides>"))); // 覆盖热改必须由Insert observer合并到现有4096项FIFO。
    }
}
