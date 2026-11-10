//! 全量几何成功后的不可见动态材质分帧登记与取消回收。

use bevy::prelude::*; // Assets、StandardMaterial和Time完成固定数量材质准备。

use super::super::super::{DynamicWaterMaterial, PreparedPrimitive, create_dynamic_water_material}; // 继续使用项目唯一GPU水材质构造器、资产类型和提交值。
use super::super::configure_whole_mesh_water_material; // 复用唯一透明水PBR规范。
use super::work::{MeshWaterRootWork, MeshWaterWorkStage}; // 材质游标与临时句柄归单根工作所有。

pub(super) const MESH_WATER_MATERIAL_BUDGET_PER_FRAME: usize = 4; // 登记和取消都最多处理四份独占材质。

/// 每帧克隆并登记固定数量材质；句柄在Commit前不挂到任何实体，画面仍保持上一完整状态。
pub(super) fn advance_material_preparation(
    work: &mut MeshWaterRootWork,
    elapsed_seconds: f32,
    standard_materials: &Assets<StandardMaterial>,
    dynamic_materials: &mut Assets<DynamicWaterMaterial>,
) -> Result<(), String> {
    let material_time = *work.material_time.get_or_insert(elapsed_seconds); // 全根材质共享同一首帧当前/历史基线。
    let end = work
        .material_cursor
        .saturating_add(MESH_WATER_MATERIAL_BUDGET_PER_FRAME)
        .min(work.validated.len());
    for validated in &work.validated[work.material_cursor..end] {
        let mut base_material = standard_materials
            .get(&validated.base_material)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "网格primitive{:?}的Blender StandardMaterial资产不可用",
                    validated.candidate.entity
                )
            })?; // 单项失败会先回收此前临时材质，绝不触碰旧ECS画面。
        configure_whole_mesh_water_material(&mut base_material); // 保留作者RGB/纹理并统一透明水边界。
        let dynamic_material = dynamic_materials.add(create_dynamic_water_material(
            base_material,
            material_time,
            validated.flow_style_encoding,
            validated.flow_speed,
            None, // placement整网格沿用固定样条外观；静水预设只绑定R8静水层。
        )); // 新资产尚未挂到实体，仅由活动工作持有。
        work.prepared.push(PreparedPrimitive {
            candidate: validated.candidate.clone(),
            dynamic_material,
            flow_speed: validated.flow_speed,
            flow_style_encoding: validated.flow_style_encoding,
            local_translation_offset: validated.local_translation_offset,
        });
    }
    work.material_cursor = end;
    if end == work.validated.len() {
        work.stage = MeshWaterWorkStage::Commit; // 全部材质成功后才开放最终身份复验与原子提交。
    }
    Ok(())
}

/// 失败、Changed或World换代时按同一材质预算释放未发布资产。
pub(in crate::shared::large_scene::water::mesh_overrides::runtime) fn discard_temporary_materials(
    work: &mut MeshWaterRootWork,
    dynamic_materials: &mut Assets<DynamicWaterMaterial>,
) -> bool {
    let mut remaining = MESH_WATER_MATERIAL_BUDGET_PER_FRAME;
    while remaining > 0 {
        let Some(prepared) = work.prepared.pop() else {
            return true; // 所有不可见资产已清理，调用方可发布最新失败或重排新修订。
        };
        let _ = dynamic_materials.remove(prepared.dynamic_material.id()); // 每项独占句柄只释放一次。
        remaining -= 1;
    }
    work.prepared.is_empty() // 恰好消费最后一项时本帧即可完成清理。
}

#[cfg(test)]
mod tests {
    use super::super::work::ValidatedPrimitive;
    use super::*;
    use crate::shared::large_scene::water::mesh_overrides::PrimitiveCandidate;

    fn candidate(entity: Entity, material: Handle<StandardMaterial>) -> PrimitiveCandidate {
        PrimitiveCandidate {
            entity,
            object_node: entity,
            node_path: vec!["河道".to_owned()],
            mesh_name: "河面".to_owned(),
            mesh: None,
            original_material: Some(material),
            original_transform: Some(Transform::IDENTITY),
            had_no_frustum_culling: false,
            had_camera_raycast_ignore: false,
            had_not_shadow_caster: false,
            had_not_shadow_receiver: false,
        }
    }

    #[test]
    fn material_failure_cleans_unpublished_prefix_without_entering_commit() {
        let root = Entity::from_raw_u32(81).expect("测试根实体必须合法");
        let first = Entity::from_raw_u32(82).expect("首个primitive实体必须合法");
        let second = Entity::from_raw_u32(83).expect("第二个primitive实体必须合法");
        let mut standard_materials = Assets::<StandardMaterial>::default();
        let existing = standard_materials.add(StandardMaterial::default());
        let missing = Handle::<StandardMaterial>::default();
        let mut dynamic_materials = Assets::<DynamicWaterMaterial>::default();
        let mut work = MeshWaterRootWork::new(root, None, None, 1, 1, true);
        work.stage = MeshWaterWorkStage::Materials;
        work.validated = vec![
            ValidatedPrimitive {
                candidate: candidate(first, existing.clone()),
                base_material: existing,
                flow_speed: 1.0,
                flow_style_encoding: Vec2::ZERO,
                local_translation_offset: Vec3::ZERO,
            },
            ValidatedPrimitive {
                candidate: candidate(second, missing.clone()),
                base_material: missing,
                flow_speed: 1.0,
                flow_style_encoding: Vec2::ZERO,
                local_translation_offset: Vec3::ZERO,
            },
        ];

        let detail = advance_material_preparation(
            &mut work,
            2.0,
            &standard_materials,
            &mut dynamic_materials,
        )
        .expect_err("第二份缺失材质必须在Commit前失败");
        assert_eq!(work.stage, MeshWaterWorkStage::Materials); // 出错函数本身绝不开放提交阶段。
        assert_eq!(work.prepared.len(), 1); // 首份只存在于不可见工作集，尚未挂到任何primitive。
        assert_eq!(dynamic_materials.iter().count(), 1);

        work.begin_failure(detail);
        assert_eq!(work.stage, MeshWaterWorkStage::Discard);
        assert!(discard_temporary_materials(
            &mut work,
            &mut dynamic_materials
        ));
        assert!(work.prepared.is_empty());
        assert_eq!(dynamic_materials.iter().count(), 0); // 前缀临时资产完整回收，旧材质/位姿没有半应用入口。
    }
}
