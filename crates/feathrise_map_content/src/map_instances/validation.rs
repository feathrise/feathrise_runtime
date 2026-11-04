//! 子地图引用局部校验：格式预算明确失败，不截断作者内容或递归展开结果。

use std::collections::BTreeSet;

use super::MapInstancePlacement;
use crate::MapId;

pub const MAX_MAP_INSTANCES: usize = 256; // 单份清单引用预算；不是逐帧展开额度。
pub const MAX_MAP_INSTANCE_DEPTH: usize = 32; // 阻止损坏或恶意长链造成栈溢出。
pub const MAX_EXPANDED_MAP_INSTANCES: usize = 4096; // 整个父世界的明确实例内存预算。

pub fn validate_map_instances(map_id: &MapId, instances: &[MapInstancePlacement]) -> Result<(), String> {
    MapId::parse(map_id.as_str())?;
    if instances.len() > MAX_MAP_INSTANCES {
        return Err(format!("地图{map_id}的子地图引用超过{MAX_MAP_INSTANCES}个预算"));
    }
    let mut ids = BTreeSet::new();
    for instance in instances {
        super::MapInstanceId::parse(instance.id.as_str())?;
        MapId::parse(instance.source_map_id.as_str())?;
        instance.transform.validate()?;
        if instance.label.len() > 256 {
            return Err(format!("子地图{}的实例名称超过256字节", instance.id));
        }
        if instance.source_map_id == *map_id {
            return Err(format!("子地图循环引用：{map_id} → {map_id}（实例{}）", instance.id));
        }
        if !ids.insert(&instance.id) {
            return Err(format!("地图{map_id}的子地图实例ID重复：{}", instance.id));
        }
    }
    Ok(())
}
