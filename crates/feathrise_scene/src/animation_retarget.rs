//! 中性跨GLB动画重定向：源/目标骨架名称路径映射与外部Mixamo骨长保护。
//! 只处理调用方冻结的剪辑和路径；任务调度、ECS与资产登记仍由各自消费者拥有。

use bevy::{
    animation::{AnimationTargetId, VariableCurve, animation_curves::EvaluatorId},
    prelude::*,
    reflect::{TypeInfo, Typed},
};
use std::{any::TypeId, collections::HashMap};

/// 沿用原角色动画入口的根节点检测语义；多个候选仍取资产目录中的首项。
pub fn detect_anim_root_name(anim_gltf: &Gltf) -> Option<String> {
    let candidates: Vec<&str> = anim_gltf
        .named_nodes
        .keys()
        .filter(|name| !name.starts_with("mixamorig:"))
        .map(|name| name.as_ref())
        .collect();
    match candidates.len() {
        0 => None,
        1 => Some(candidates[0].to_string()),
        _ => {
            debug!(
                "GLTF 中有多个非骨骼节点：{:?}，取第一个作为根节点",
                candidates
            );
            Some(candidates[0].to_string())
        }
    }
}

/// 后台纯计算入口；目标路径必须从真实AnimationPlayer开始，不能包含Scene0包装。
pub fn retarget_clip(
    mut clip: AnimationClip,
    model_root_name: &str,
    bone_paths: &[Vec<String>],
    anim_root_name: &str,
) -> AnimationClip {
    let targets = build_target_map(bone_paths, anim_root_name);
    let fields = TransformCurveFields::new();
    let filters_external_pose = anim_root_name != model_root_name;
    let old_curves: Vec<_> = clip.curves_mut().drain().collect();
    for (old_target, curves) in old_curves {
        let target = targets.get(&old_target);
        let new_target = target.map(|t| t.model_target).unwrap_or(old_target);
        let curves = if filters_external_pose && target.is_some_and(|t| t.is_mixamo_bone) {
            // 外部动画的逐骨骼本地位移/缩放会覆盖目标骨长；只保留其旋转和其他非Transform曲线。
            curves
                .into_iter()
                .filter(|curve| !fields.changes_bone_length(curve))
                .collect()
        } else {
            curves
        };
        if !curves.is_empty() {
            clip.curves_mut().insert(new_target, curves);
        }
    }
    clip
}

#[derive(Clone, Copy)]
struct RetargetTarget {
    model_target: AnimationTargetId,
    is_mixamo_bone: bool,
}

struct TransformCurveFields {
    translation: (TypeId, usize),
    scale: (TypeId, usize),
}
impl TransformCurveFields {
    fn new() -> Self {
        Self {
            translation: transform_field_id("translation"),
            scale: transform_field_id("scale"),
        }
    }
    fn changes_bone_length(&self, curve: &VariableCurve) -> bool {
        let EvaluatorId::ComponentField(field) = curve.0.evaluator_id() else {
            return false;
        };
        **field == self.translation || **field == self.scale
    }
}
fn transform_field_id(name: &str) -> (TypeId, usize) {
    let TypeInfo::Struct(info) = Transform::type_info() else {
        unreachable!("Transform is expected to be a reflected struct");
    };
    (
        TypeId::of::<Transform>(),
        info.index_of(name)
            .expect("Transform animation field should exist"),
    )
}
fn build_target_map(
    bone_paths: &[Vec<String>],
    anim_root_name: &str,
) -> HashMap<AnimationTargetId, RetargetTarget> {
    let mut targets = HashMap::new();
    for path in bone_paths {
        if path.is_empty() {
            continue;
        }
        let model_target = path.iter().map(|s| s.as_str()).collect();
        let is_mixamo_bone = path.iter().any(|name| name.starts_with("mixamorig:"));
        let mut source_path = path.clone();
        source_path[0] = anim_root_name.to_string();
        targets
            .entry(source_path.iter().map(|s| s.as_str()).collect())
            .or_insert(RetargetTarget {
                model_target,
                is_mixamo_bone,
            });
        if let Some(first_bone) = path.iter().position(|name| name.starts_with("mixamorig:")) {
            // 模型可能在根与Mixamo首骨之间多出Armature；动画文件仍使用源根/首骨路径。
            let mut source_path = Vec::with_capacity(path.len() - first_bone + 1);
            source_path.push(anim_root_name.to_string());
            source_path.extend(path[first_bone..].iter().cloned());
            targets
                .entry(source_path.iter().map(|s| s.as_str()).collect())
                .or_insert(RetargetTarget {
                    model_target,
                    is_mixamo_bone: true,
                });
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::animation::animated_field;

    #[test]
    fn external_idle_retargets_player_and_preserves_target_bone_length() {
        let source: AnimationTargetId = ["巨剑_待机_05", "mixamorig:Hips"].into_iter().collect();
        let target: AnimationTargetId = ["Player", "mixamorig:Hips"].into_iter().collect();
        let unrelated: AnimationTargetId = ["morph"].into_iter().collect();
        let mut clip = AnimationClip::default();
        clip.add_curve_to_target(
            source,
            AnimatableCurve::new(
                animated_field!(Transform::translation),
                ConstantCurve::new(Interval::EVERYWHERE, Vec3::ONE),
            ),
        );
        clip.add_curve_to_target(
            source,
            AnimatableCurve::new(
                animated_field!(Transform::rotation),
                ConstantCurve::new(Interval::EVERYWHERE, Quat::IDENTITY),
            ),
        );
        clip.add_curve_to_target(
            source,
            AnimatableCurve::new(
                animated_field!(Transform::scale),
                ConstantCurve::new(Interval::EVERYWHERE, Vec3::ONE),
            ),
        );
        clip.add_curve_to_target(
            unrelated,
            AnimatableCurve::new(
                animated_field!(Transform::translation),
                ConstantCurve::new(Interval::EVERYWHERE, Vec3::ONE),
            ),
        );
        let paths = vec![vec!["Player".into(), "mixamorig:Hips".into()]];
        let same_paths = vec![vec!["巨剑_待机_05".into(), "mixamorig:Hips".into()]];
        let same = retarget_clip(clip.clone(), "巨剑_待机_05", &same_paths, "巨剑_待机_05");
        assert_eq!(
            same.curves().get(&source).unwrap().len(),
            3,
            "同根来源保留原位移缩放语义"
        );
        let mapped = retarget_clip(clip, "Player", &paths, "巨剑_待机_05");
        assert!(!mapped.curves().contains_key(&source));
        assert_eq!(mapped.curves().get(&target).unwrap().len(), 1);
        assert!(
            matches!(mapped.curves()[&target][0].0.evaluator_id(), EvaluatorId::ComponentField(field) if **field == transform_field_id("rotation"))
        );
        assert_eq!(mapped.curves().get(&unrelated).unwrap().len(), 1);
    }

    #[test]
    fn external_idle_maps_armature_and_ignores_scene_wrapper() {
        let paths = vec![vec![
            "Player".into(),
            "Armature".into(),
            "mixamorig:Hips".into(),
        ]];
        let targets = build_target_map(&paths, "巨剑_待机_05");
        let source = ["巨剑_待机_05", "mixamorig:Hips"].into_iter().collect();
        let expected = ["Player", "Armature", "mixamorig:Hips"]
            .into_iter()
            .collect();
        assert_eq!(targets[&source].model_target, expected);
    }
}
