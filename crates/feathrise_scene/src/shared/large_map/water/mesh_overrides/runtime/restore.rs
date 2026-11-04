//! placement整网格水体的原始实例恢复与独占动态材质生命周期。

use std::collections::{HashSet, VecDeque};

use super::super::*; // 复用功能域的内部恢复记录和现有动态水材质类型。

const MESH_WATER_MATERIAL_RETIRE_BUDGET_PER_FRAME: usize = 4;

/// restore与组件Remove observer共享的固定上限去重FIFO，禁止在提交帧同步批量改Assets。
#[derive(Resource, Default)]
pub(in crate::shared::large_map::water) struct RetiredMeshWaterMaterials {
    handles: VecDeque<Handle<DynamicWaterMaterial>>,
    queued: HashSet<AssetId<DynamicWaterMaterial>>,
}

impl RetiredMeshWaterMaterials {
    fn enqueue(&mut self, handle: Handle<DynamicWaterMaterial>) {
        let id = handle.id();
        if self.queued.contains(&id) {
            return; // restore随后触发Remove observer时不会为同一独占材质占两个席位。
        }
        self.queued.insert(id);
        self.handles.push_back(handle);
    }

    fn remove_budget(&mut self, materials: &mut Assets<DynamicWaterMaterial>) {
        for _ in 0..MESH_WATER_MATERIAL_RETIRE_BUDGET_PER_FRAME {
            let Some(handle) = self.handles.pop_front() else {
                return;
            };
            self.queued.remove(&handle.id());
            let _ = materials.remove(handle.id());
        }
    }
}

/// 按根上有界实体表恢复上一轮Blender原材质与位姿；Mesh3d从未替换。
pub(super) fn restore_applied_primitives(
    root: Entity, // 只恢复这一个覆盖根自己的primitive。
    applied_roots: &Query<&DynamicWaterMeshOverrideApplied>, // 直接定位受硬上限约束的上轮实体。
    overridden_primitives: &Query<&DynamicWaterMeshOverridePrimitive>, // 每项携带完整原句柄和原位姿。
    retired_materials: &mut RetiredMeshWaterMaterials, // 上轮独占扩展材质转交4份/帧退役队列。
    commands: &mut Commands, // 恢复命令在本系统尾部与新接管命令按顺序原子落地。
) {
    let Ok(applied) = applied_roots.get(root) else {
        return; // 首次应用或空覆盖没有需要恢复的primitive。
    }; // 完成上轮接管索引读取。
    for &entity in &applied.entities {
        let Ok(original) = overridden_primitives.get(entity) else {
            continue; // WorldAsset热重载已删除的旧primitive由Remove observer回收资产。
        }; // 读取当前仍存活的恢复记录。
        if original.owner_root != root {
            warn!("拒绝恢复不属于当前placement根{root:?}的水网格{entity:?}"); // 所有者异常时宁可保留现状也不跨根破坏。
            continue; // 跳过异常所有者项。
        } // 完成primitive所有权核对。
        retired_materials.enqueue(original.dynamic_material.clone()); // 组件Remove observer后续重复登记会被AssetId去重。
        let mut entity_commands = commands.entity(entity); // 恢复命令全部定位到同一primitive。
        entity_commands
            .remove::<MeshMaterial3d<DynamicWaterMaterial>>() // 先移除GPU水材质所有者，避免两种材质并存。
            .remove::<DynamicWaterSurface>() // 复杂导入水的Disabled互动与动画状态同步退出查询。
            .insert((
                MeshMaterial3d(original.original_material.clone()), // 恢复Blender原始StandardMaterial句柄。
                original.original_transform, // 取消高度偏移并恢复完整局部位姿。
            ))
            .remove::<DynamicWaterMeshOverridePrimitive>(); // Remove observer对已精确释放的句柄再次幂等清理。
        if !original.had_no_frustum_culling {
            entity_commands.remove::<NoFrustumCulling>(); // 只移除接管新增的裁剪标记。
        } // 完成原视锥裁剪状态恢复。
        if !original.had_camera_raycast_ignore {
            entity_commands.remove::<CameraRaycastIgnore>(); // 原网格未忽略相机射线时才撤销标记。
        } // 完成原相机射线状态恢复。
        if !original.had_not_shadow_caster {
            entity_commands.remove::<NotShadowCaster>(); // 作者原本允许投影时恢复该行为。
        } // 完成原投影状态恢复。
        if original.had_not_shadow_receiver {
            entity_commands.insert(NotShadowReceiver); // 作者原本禁用接影时在撤销赋水后还原该标记。
        } else {
            entity_commands.remove::<NotShadowReceiver>(); // 作者原本允许接影时防御性清除接管期状态。
        } // 完成原接影状态精确恢复。
    } // 完成上轮全部受限primitive恢复。
    commands
        .entity(root) // 上轮索引已经消费，新接管成功后再写入新表。
        .remove::<DynamicWaterMeshOverrideApplied>(); // 只在新快照已成功预检或显式清除时退出上轮接管。
} // 结束上轮接管恢复。

/// 公开覆盖根组件被显式移除时，完整恢复存活World实例。
pub(in crate::shared::large_map::water) fn restore_removed_dynamic_water_mesh_overrides(
    remove: On<Remove, DynamicWaterMeshOverrides>, // Remove在组件真正移除前触发，仍可查询根内部索引。
    mut queue: ResMut<DynamicWaterMeshOverrideQueue>, // 无论清除还是despawn都O(1)撤销未处理队列项。
    mut work_state: ResMut<DynamicWaterMeshOverrideWorkState>, // 活动根只撤销修订，临时材质由处理系统分帧回收。
    applied_roots: Query<&DynamicWaterMeshOverrideApplied>,    // 只按有界实体表恢复。
    overridden: Query<&DynamicWaterMeshOverridePrimitive>,     // 只恢复本模块写入的primitive。
    mut retired_materials: ResMut<RetiredMeshWaterMaterials>, // 只转交句柄，实际Assets删除由固定预算系统执行。
    mut commands: Commands, // 排入原句柄、原位姿恢复与根状态移除。
) {
    work_state.forget_root(&mut queue, remove.entity); // 等待根摘链并释放换代阻断；活动游标下一帧按身份失效进入有界清理。
    if remove.trigger().new_archetype.is_none() {
        return; // 根despawn时子primitive也将销毁，由primitive Remove observer只回收独占Assets。
    } // 区分撤销覆盖与整根销毁。
    restore_applied_primitives(
        remove.entity,
        &applied_roots,
        &overridden,
        &mut retired_materials,
        &mut commands,
    ); // 组件移除也履行与clear完全相同的恢复契约。
    commands
        .entity(remove.entity) // 根继续存活，但已不属于整网格赋水功能域。
        .remove::<(
            DynamicWaterMeshOverrideApplied,
            DynamicWaterMeshOverrideStatus,
            DynamicWaterMeshOverrideInstanceIdentity,
        )>(); // 清理所有内部根状态。
} // 结束覆盖根组件移除恢复。

/// primitive恢复记录移除或实体despawn时，精确回收本轮独占动态材质。
pub(in crate::shared::large_map::water) fn cleanup_removed_mesh_override_assets(
    remove: On<Remove, DynamicWaterMeshOverridePrimitive>, // Remove发生时旧记录仍可查询。
    overridden: Query<&DynamicWaterMeshOverridePrimitive>, // 读取待回收的独占材质句柄。
    mut retired_materials: ResMut<RetiredMeshWaterMaterials>, // restore与despawn共享去重退役队列。
) {
    let Ok(removed) = overridden.get(remove.entity) else {
        return; // 异常实体无旧记录时无可回收资产。
    }; // 完成移除前恢复记录读取。
    retired_materials.enqueue(removed.dynamic_material.clone()); // 不在observer调用栈同步批量改Assets。
} // 结束primitive独占Assets回收observer。

/// 每帧最多从Assets精确删除四份已经不再挂到primitive的独占动态材质。
pub(in crate::shared::large_map::water) fn retire_removed_mesh_override_materials(
    mut retired: ResMut<RetiredMeshWaterMaterials>,
    mut materials: ResMut<Assets<DynamicWaterMaterial>>,
) {
    retired.remove_budget(&mut materials);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retire_queue_deduplicates_and_removes_only_four_per_frame() {
        let mut queue = RetiredMeshWaterMaterials::default();
        let mut materials = Assets::<DynamicWaterMaterial>::default();
        let handles = (0..6)
            .map(|_| materials.add(DynamicWaterMaterial::default()))
            .collect::<Vec<_>>();
        for handle in &handles {
            queue.enqueue(handle.clone());
        }
        queue.enqueue(handles[0].clone());
        assert_eq!(queue.handles.len(), 6);
        queue.remove_budget(&mut materials);
        assert_eq!(queue.handles.len(), 2);
        assert_eq!(materials.len(), 2);
        queue.remove_budget(&mut materials);
        assert!(queue.handles.is_empty());
        assert!(materials.is_empty());
    }
}
