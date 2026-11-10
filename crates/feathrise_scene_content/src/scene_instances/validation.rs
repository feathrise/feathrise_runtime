//! 子场景引用局部校验：格式预算明确失败，不截断作者内容或递归展开结果。

use std::collections::BTreeSet;

use super::SceneInstancePlacement;
use crate::SceneId;

pub const MAX_SCENE_INSTANCES: usize = 256; // 单份清单引用预算；不是逐帧展开额度。
pub const MAX_SCENE_INSTANCE_DEPTH: usize = 32; // 阻止损坏或恶意长链造成栈溢出。
pub const MAX_EXPANDED_SCENE_INSTANCES: usize = 4096; // 整个父世界的明确实例内存预算。

pub fn validate_scene_instances(scene_id: &SceneId, instances: &[SceneInstancePlacement]) -> Result<(), String> {
    SceneId::parse(scene_id.as_str())?;
    if instances.len() > MAX_SCENE_INSTANCES {
        return Err(format!("场景{scene_id}的子场景引用超过{MAX_SCENE_INSTANCES}个预算"));
    }
    let mut ids = BTreeSet::new();
    for instance in instances {
        super::SceneInstanceId::parse(instance.id.as_str())?;
        SceneId::parse(instance.source_scene_id.as_str())?;
        instance.transform.validate()?;
        if instance.label.len() > 256 {
            return Err(format!("子场景{}的实例名称超过256字节", instance.id));
        }
        if instance.source_scene_id == *scene_id {
            return Err(format!("子场景循环引用：{scene_id} → {scene_id}（实例{}）", instance.id));
        }
        if !ids.insert(&instance.id) {
            return Err(format!("场景{scene_id}的子场景实例ID重复：{}", instance.id));
        }
    }
    Ok(())
}
