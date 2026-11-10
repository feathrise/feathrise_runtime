//! 后台引用展开：完整检查来源链和容量后一次发布实例树，失败保留旧世界。

use bevy::prelude::{Rect, Vec2, Vec3};
use std::{collections::BTreeMap, sync::Arc};

use super::{SceneInstancePath, SceneInstancePlacement, SceneInstanceTransform, MAX_EXPANDED_SCENE_INSTANCES, MAX_SCENE_INSTANCE_DEPTH, validate_scene_instances};
use crate::SceneId;

/// 清单事件产生的小型不可变引用快照；无需为遍历深拷贝地形、备注或作者纹理。
#[derive(Clone, Debug)]
pub struct SceneInstanceSource {
    pub scene_id: SceneId,
    pub revision: u64, // 保存清单的来源代次，纯父图姿态预览沿用原值。
    pub world_bounds: Rect,
    pub instances: Arc<[SceneInstancePlacement]>,
    /// 浅共享作者对象；后台展开时复用同一场景/地形实例调度，不创建伪场景ID。
    pub terrains: Option<Arc<[crate::SceneTerrainObject]>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpandedSceneInstance {
    pub path: SceneInstancePath,
    pub source_scene_id: SceneId,
    pub source_revision: u64,
    pub label: String,
    pub transform: SceneInstanceTransform,
    pub local_transform: SceneInstanceTransform, // 整体移动父实例不修改源清单中的内部局部姿态。
    pub local_bounds: Rect,
    pub world_bounds: Rect,
    pub terrain: Option<Arc<crate::SceneTerrainObject>>,
    pub container: bool, // 容器本身没有几何范围；空间保护只读取独立网格子节点。
}

impl ExpandedSceneInstance {
    pub fn local_pose(&self) -> bevy::prelude::Transform {
        self.local_transform.as_transform().with_scale(self.terrain.as_ref()
            .map_or(Vec3::ONE, |object| Vec3::from_array(object.transform.scale)))
    }
    pub fn world_pose(&self) -> bevy::prelude::Transform {
        self.transform.as_transform().with_scale(self.terrain.as_ref()
            .map_or(Vec3::ONE, |object| Vec3::from_array(object.transform.scale)))
    }
}

/// 根节点也进入结果，消费者统一处理来源；同源兄弟分别拥有不同 path。
pub fn expand_scene_instances(root: &SceneId, sources: &BTreeMap<SceneId, Arc<SceneInstanceSource>>) -> Result<Vec<ExpandedSceneInstance>, String> {
    let mut result = Vec::new();
    let mut ancestry = Vec::new();
    let mut scene_count = 0;
    expand(root, SceneInstancePath::root(root.clone()), String::new(), SceneInstanceTransform::default(), SceneInstanceTransform::default(), sources, &mut ancestry, &mut result, &mut scene_count)?;
    Ok(result)
}

fn expand(source_id: &SceneId, path: SceneInstancePath, label: String, transform: SceneInstanceTransform, local_transform: SceneInstanceTransform, sources: &BTreeMap<SceneId, Arc<SceneInstanceSource>>, ancestry: &mut Vec<SceneId>, result: &mut Vec<ExpandedSceneInstance>, scene_count: &mut usize) -> Result<(), String> {
    if ancestry.contains(source_id) {
        let mut cycle: Vec<_> = ancestry.iter().map(ToString::to_string).collect();
        cycle.push(source_id.to_string());
        return Err(format!("子场景循环引用：{}", cycle.join(" → ")));
    }
    if path.instances.len() > MAX_SCENE_INSTANCE_DEPTH {
        return Err(format!("子场景嵌套超过{MAX_SCENE_INSTANCE_DEPTH}层：{path}"));
    }
    if *scene_count >= MAX_EXPANDED_SCENE_INSTANCES {
        return Err(format!("父场景的展开实例超过{MAX_EXPANDED_SCENE_INSTANCES}个预算"));
    }
    *scene_count += 1; // 既有引用预算只计算场景节点；地形对象共用准备预算，不占用场景引用名额。
    let source = sources.get(source_id).ok_or_else(|| format!("子场景来源不存在或尚未就绪：{source_id}（引用{path}）"))?;
    if source.scene_id != *source_id {
        return Err(format!("子场景来源身份不匹配：{source_id}"));
    }
    validate_scene_instances(source_id, &source.instances)?;
    let bounds = source.world_bounds;
    if !bounds.min.is_finite() || !bounds.max.is_finite() || !bounds.min.cmplt(bounds.max).all() {
        return Err(format!("子场景{source_id}的地形范围无效"));
    }
    transform.validate()?;
    // 旋转后以四个真实角点求世界包围盒，只用于空间粗筛，精确查询仍逆变换至源图。
    let corners = [bounds.min, Vec2::new(bounds.max.x, bounds.min.y), bounds.max, Vec2::new(bounds.min.x, bounds.max.y)];
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for corner in corners {
        let point = transform.to_world(Vec3::new(corner.x, 0.0, corner.y));
        min = min.min(Vec2::new(point.x, point.z));
        max = max.max(Vec2::new(point.x, point.z));
    }
    let world_bounds = projected_bounds(min, max).ok_or_else(|| format!("子场景{path}变换后的范围溢出或丢失精度"))?;
    result.push(ExpandedSceneInstance { path: path.clone(), source_scene_id: source_id.clone(), source_revision: source.revision, label, transform, local_transform, local_bounds: bounds, world_bounds, terrain: None, container: source.terrains.is_some() });
    if let Some(objects) = &source.terrains {
        for object in objects.iter().filter(|object| object.enabled) {
            object.transform.validate()?;
            let local_transform = SceneInstanceTransform { translation: object.transform.translation, yaw_degrees: object.transform.yaw_degrees,
                pitch_degrees: object.transform.pitch_degrees, roll_degrees: object.transform.roll_degrees };
            let world_transform = transform.compose(&local_transform);
            let pose = world_transform.as_transform().with_scale(Vec3::from_array(object.transform.scale));
            let local_bounds = Rect::from_corners(Vec2::from_array(object.world_min), Vec2::from_array(object.world_max));
            // 倾斜会把源高度投影到世界XZ；四个Y=0角点会漏掉高山或把竖直网格误判为空。
            let (bottom, top) = object.voxel.as_ref().map_or(
                (object.terrain.base_height, object.terrain.base_height + object.terrain.height_scale),
                |voxel| (voxel.min[1], voxel.max[1]));
            let mut min = Vec2::splat(f32::INFINITY);
            let mut max = Vec2::splat(f32::NEG_INFINITY);
            for bits in 0..8 {
                let point = pose.transform_point(Vec3::new(
                    if bits & 1 == 0 { local_bounds.min.x } else { local_bounds.max.x },
                    if bits & 2 == 0 { bottom } else { top },
                    if bits & 4 == 0 { local_bounds.min.y } else { local_bounds.max.y }));
                let world = Vec2::new(point.x, point.z);
                min = min.min(world); max = max.max(world);
            }
            let world_bounds = projected_bounds(min, max).ok_or("地形变换后的范围无效")?;
            let mut projected = object.clone();
            projected.terrain.resolved_stitches = crate::resolve_clipmap_stitches(object, objects);
            result.push(ExpandedSceneInstance { path: path.terrain(object.id.clone()), source_scene_id: source_id.clone(),
                source_revision: source.revision, label: object.display_name.clone(), transform: world_transform,
                local_transform, local_bounds, world_bounds, terrain: Some(Arc::new(projected)), container: false });
        }
    }
    ancestry.push(source_id.clone());
    for child in source.instances.iter() {
        expand(&child.source_scene_id, path.child(child.id.clone()), child.label.clone(), transform.compose(&child.transform), child.transform, sources, ancestry, result, scene_count)?;
    }
    ancestry.pop();
    Ok(())
}

/// 合法竖直面在XZ可退化成线；仅为粗筛扩一格浮点精度，不修改作者几何。
fn projected_bounds(mut min: Vec2, mut max: Vec2) -> Option<Rect> {
    if !min.is_finite() || !max.is_finite() || !min.cmple(max).all() { return None; }
    for axis in 0..2 {
        if min[axis] == max[axis] { min[axis] = min[axis].next_down(); max[axis] = max[axis].next_up(); }
    }
    (min.is_finite() && max.is_finite()).then(|| Rect::from_corners(min, max))
}
