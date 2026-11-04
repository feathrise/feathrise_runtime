//! 院落目录的纯校验；热更失败不得发布半份组合。

use std::collections::BTreeSet; // 组合 ID 与各组合内成员 ID 都要检测重复。

use super::*; // 使用同域版本和容量常量。
use crate::{MAX_MAP_PLACEMENT_SCALE, MIN_MAP_PLACEMENT_SCALE, valid_building_preset_id}; // 与普通摆放、建筑身份共享约束。

const MAX_COURTYARD_TEXT_CHARS: usize = 128; // 限制作者名称和稳定身份的 UI 成本。
const MAX_COURTYARD_ASSET_PATH_BYTES: usize = 4_096; // 与现有建筑模型路径预算一致。
const MAX_COURTYARD_LOCAL_DISTANCE: f32 = 100_000.0; // 防止失控模板将预览或校验包围盒放大到异常范围。
const MAX_COURTYARD_PRIMITIVE_INDEX: u32 = 1_000_000; // 损坏目标编号不会制造没有实际 GLTF 对应物的超大标签。

/// 返回字符串能否成为组合或成员的稳定 ID。
pub fn valid_courtyard_preset_id(value: &str) -> bool {
    !value.is_empty() // 空身份无法跨地图或预设修订追踪。
        && value.len() <= MAX_COURTYARD_TEXT_CHARS // 限制记录和 UI 的身份分配。
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) // 路径、空白及本地化文字不能进入技术身份。
}

/// 校验整份目录的版本、总预算和所有成员。
pub(super) fn validate_catalog(asset: &CourtyardPresetCatalogAsset) -> Result<(), String> {
    if asset.schema_version != COURTYARD_PRESET_SCHEMA_VERSION {
        return Err(format!(
            "院落预设目录版本{}与运行时版本{}不一致",
            asset.schema_version, COURTYARD_PRESET_SCHEMA_VERSION
        )); // 旧格式不能被猜测性加载。
    }
    if asset.presets.len() > MAX_COURTYARD_PRESETS {
        return Err(format!("院落预设数量超过{}项上限", MAX_COURTYARD_PRESETS)); // 整份目录有固定校验成本。
    }
    let mut ids = BTreeSet::new(); // 规范组合身份不得依赖数组顺序解析。
    let mut total_members = 0_usize; // 限制一次热更的总成员扫描和复制量。
    for preset in &asset.presets {
        if !valid_courtyard_preset_id(&preset.id)
            || preset.label.trim().is_empty()
            || preset.label.chars().count() > MAX_COURTYARD_TEXT_CHARS
        {
            return Err(format!("院落预设 {} 的 ID 或名称无效", preset.id)); // 作者列表必须始终可辨认和追踪。
        }
        if !ids.insert(preset.id.as_str()) {
            return Err(format!("院落预设 ID 重复：{}", preset.id)); // 重复 ID 不得覆盖前一项。
        }
        if preset.members.is_empty() || preset.members.len() > MAX_COURTYARD_MEMBERS_PER_PRESET {
            return Err(format!(
                "院落预设 {} 的成员数须为1至{}",
                preset.id, MAX_COURTYARD_MEMBERS_PER_PRESET
            )); // 空组合没有可盖印内容，超额组合不能进入单次候选。
        }
        total_members += preset.members.len(); // 单项上限和目录上限共同约束主线程热更成本。
        if total_members > MAX_COURTYARD_TOTAL_MEMBERS {
            return Err(format!(
                "院落预设目录成员总数超过{}",
                MAX_COURTYARD_TOTAL_MEMBERS
            )); // 多项不能绕开总预算。
        }
        let mut member_ids = BTreeSet::new(); // 成员身份仅要求在所属组合内唯一。
        for member in &preset.members {
            validate_member(member, &preset.id)?; // 任何坏成员都拒绝整份新目录。
            if !member_ids.insert(member.id.as_str()) {
                return Err(format!(
                    "院落预设 {} 的成员 ID 重复：{}",
                    preset.id, member.id
                )); // 局部覆盖不能歧义地指向两个成员。
            }
        }
    }
    Ok(()) // 空全局目录合法；旧地图不会因尚未创建组合而失败。
}

/// 校验单个显式摆放原型的身份、模型和局部变换。
fn validate_member(member: &CourtyardMemberDescriptor, preset_id: &str) -> Result<(), String> {
    if let Some(policy) = member.mesh_lod { policy.validate()?; }
    if !valid_courtyard_preset_id(&member.id) {
        return Err(format!(
            "院落预设 {preset_id} 的成员 ID 无效：{}",
            member.id
        )); // 局部修改必须有稳定可持久化身份。
    }
    if !(if member.asset_target.is_builtin() { member.asset_path.is_empty() } else { safe_scene_asset_path(&member.asset_path) }) || !valid_asset_target(member.asset_target) {
        return Err(format!(
            "院落预设 {preset_id} 的成员 {} 模型路径或目标无效",
            member.id
        )); // 只接受可由现有摆放链请求的 GLB/GLTF Scene0 或静态 primitive。
    }
    if (member.asset_target.is_builtin() && member.building_preset_id.is_some()) || member
        .building_preset_id
        .as_deref()
        .is_some_and(|id| !valid_building_preset_id(id))
    {
        return Err(format!(
            "院落预设 {preset_id} 的成员 {} 建筑身份无效",
            member.id
        )); // 跨目录是否存在、模型是否相符由实际盖印时按当前建筑目录核对。
    }
    if !member
        .local_translation
        .iter()
        .all(|value| value.is_finite() && value.abs() <= MAX_COURTYARD_LOCAL_DISTANCE)
        || !member.local_yaw_degrees.is_finite()
        || !member.scale.iter().all(|value| {
            value.is_finite()
                && *value >= MIN_MAP_PLACEMENT_SCALE
                && *value <= MAX_MAP_PLACEMENT_SCALE
        })
        || !member.terrain_offset.is_finite()
        || member.terrain_offset.abs() > MAX_COURTYARD_LOCAL_DISTANCE
    {
        return Err(format!(
            "院落预设 {preset_id} 的成员 {} 变换或地形偏移无效",
            member.id
        )); // 非有限或超界数值不能进入 Gizmo、地形采样和实例化。
    }
    Ok(()) // 完整成员可安全进入有界快照。
}

/// 只允许位于 assets 根内的相对 GLB/GLTF 文件，不接受 Bevy 标签或路径穿越。
fn safe_scene_asset_path(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_COURTYARD_ASSET_PATH_BYTES
        || value.contains(&['\\', '#', '?', ':'][..])
        || value.chars().any(char::is_control)
    {
        return false; // 排除绝对盘符、子资产后缀、查询和控制字符。
    }
    if value
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return false; // 首段空串、双斜杠与任意穿越段全部拒绝。
    }
    let lowercase = value.to_ascii_lowercase(); // 有 4 KiB 上限，扩展名检查分配有界。
    lowercase.ends_with(".glb") || lowercase.ends_with(".gltf") // 与显式摆放资产来源一致。
}

/// 目录只能声明已知目标形状与有界 GLTF 编号；静态性在实际资产加载时检查。
fn valid_asset_target(target: MapPlacementAssetTarget) -> bool {
    match target {
        MapPlacementAssetTarget::Scene0 => true, // 完整场景目标无额外编号。
        MapPlacementAssetTarget::Cuboid { .. } | MapPlacementAssetTarget::EditableCuboid { .. } | MapPlacementAssetTarget::EditableMesh { .. } => target.validate().is_ok(), // 内置成员复用相同几何与材质边界。
        MapPlacementAssetTarget::Primitive {
            mesh,
            primitive,
            material,
        } => {
            mesh <= MAX_COURTYARD_PRIMITIVE_INDEX // 非现实编号通常来自损坏或手写错误。
                && primitive <= MAX_COURTYARD_PRIMITIVE_INDEX // 子网格编号也必须有界。
                && material.is_none_or(|value| value <= MAX_COURTYARD_PRIMITIVE_INDEX) // 原关联材质标签保持有界。
        }
    }
}
