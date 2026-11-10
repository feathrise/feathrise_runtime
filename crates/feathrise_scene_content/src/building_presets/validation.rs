//! 建筑预设目录的版本、身份、路径与局部几何纯校验。

use std::collections::BTreeSet; // 规范 ID 与历史别名共享确定性唯一命名空间。

use super::*; // 校验同域全部公开数据结构。

const MAX_BUILDING_TEXT_CHARS: usize = 128; // 单项显示文本保持明确上限，避免异常目录制造无界 UI 文案。
const MAX_BUILDING_ASSET_PATH_BYTES: usize = 4_096; // 与普通摆放运行时资产路径硬门保持一致。
const MAX_BUILDING_LOCAL_DISTANCE: f32 = 100_000.0; // 局部语义足以覆盖超大型城门并拒绝异常数值。

/// 返回字符串是否可以作为建筑规范 ID 或历史别名。
pub fn valid_building_preset_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_BUILDING_TEXT_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')) // 稳定身份不含路径、本地化字符或空白。
}

/// 完整校验目录，任一错误都阻止新修订部分采用。
pub(super) fn validate_catalog(asset: &BuildingPresetCatalogAsset) -> Result<(), String> {
    if asset.schema_version != BUILDING_PRESET_SCHEMA_VERSION {
        return Err(format!(
            "建筑预设目录版本{}与运行时版本{}不一致",
            asset.schema_version, BUILDING_PRESET_SCHEMA_VERSION,
        )); // 不兼容目录必须显式迁移，不能猜测字段语义。
    }
    let mut names = BTreeSet::new(); // 当前 ID 与历史别名共同占用唯一命名空间。
    for preset in &asset.presets {
        validate_preset(preset)?; // 先检查当前项全部局部数据，再登记跨项身份。
        if !names.insert(preset.id.as_str()) {
            return Err(format!("建筑预设 ID 重复：{}", preset.id)); // 两个房屋不能共享规范身份。
        }
        for alias in &preset.aliases {
            if !names.insert(alias.as_str()) {
                return Err(format!("建筑预设历史别名重复或占用现有 ID：{alias}")); // 解析结果不得依赖数组顺序。
            }
        }
    }
    Ok(()) // 预设数量由资产文件和内存能力约束，不绑定 Rust 按钮或枚举长度。
}

/// 校验单个建筑预设的全部持久字段。
fn validate_preset(preset: &BuildingPresetDescriptor) -> Result<(), String> {
    if !valid_building_preset_id(&preset.id)
        || preset
            .aliases
            .iter()
            .any(|alias| !valid_building_preset_id(alias))
        || preset.label.trim().is_empty()
        || preset.label.chars().count() > MAX_BUILDING_TEXT_CHARS
    {
        return Err(format!("建筑预设 {} 的 ID、别名或名称无效", preset.id)); // UI 与场景引用都需要稳定有界身份。
    }
    if preset.asset_target.is_builtin() || !safe_scene_asset_path(&preset.asset_path) {
        return Err(format!("建筑预设 {} 的模型路径无效", preset.id)); // 只允许 assets 根内可摆放场景文件。
    }
    if !valid_rect(preset.footprint) || !valid_rect(preset.vegetation_clearance) {
        return Err(format!("建筑预设 {} 的占地或植被净空无效", preset.id)); // 重叠和净空索引不能接收零尺寸或非有限矩形。
    }
    if !finite_slice(&preset.entrance.position)
        || !preset.entrance.outward_yaw_degrees.is_finite()
        || !finite_slice(&preset.entrance.clearance_half_extents)
        || preset
            .entrance
            .clearance_half_extents
            .iter()
            .any(|value| *value <= 0.0 || *value > MAX_BUILDING_LOCAL_DISTANCE)
    {
        return Err(format!("建筑预设 {} 的正门或门前净空无效", preset.id)); // 门口预览与后续沿街布局必须使用可解释尺寸。
    }
    if !preset.foundation.model_base_y.is_finite()
        || preset.foundation.model_base_y.abs() > MAX_BUILDING_LOCAL_DISTANCE
        || !preset.foundation.max_ground_height_delta.is_finite()
        || preset.foundation.max_ground_height_delta < 0.0
        || preset.foundation.max_ground_height_delta > MAX_BUILDING_LOCAL_DISTANCE
    {
        return Err(format!("建筑预设 {} 的地基参数无效", preset.id)); // 坡地检查拒绝 NaN、负容差和异常局部距离。
    }
    if preset.connections.len() > MAX_BUILDING_CONNECTION_POINTS {
        return Err(format!(
            "建筑预设 {} 的连接点超过{}个上限",
            preset.id, MAX_BUILDING_CONNECTION_POINTS,
        )); // 所选模块逐点吸附必须保持固定小预算。
    }
    let mut connection_ids = BTreeSet::new(); // 稳定点身份只需在所属预设内唯一。
    for connection in &preset.connections {
        if !valid_building_preset_id(&connection.id)
            || !valid_building_preset_id(&connection.kind)
            || !connection_ids.insert(connection.id.as_str())
        {
            return Err(format!("建筑预设 {} 的连接点 ID 或兼容类型无效", preset.id));
        }
        if !finite_slice(&connection.position)
            || connection
                .position
                .iter()
                .any(|value| value.abs() > MAX_BUILDING_LOCAL_DISTANCE)
            || !connection.outward_yaw_degrees.is_finite()
        {
            return Err(format!("建筑预设 {} 的连接点位置或方向无效", preset.id));
        }
        let quarter_turn = (connection.outward_yaw_degrees / 90.0).round() * 90.0;
        if (connection.outward_yaw_degrees - quarter_turn).abs() > 0.001 {
            return Err(format!(
                "建筑预设 {} 的连接点方向必须使用90°档位",
                preset.id
            ));
        }
    }
    if preset
        .entrance
        .position
        .iter()
        .any(|value| value.abs() > MAX_BUILDING_LOCAL_DISTANCE)
    {
        return Err(format!("建筑预设 {} 的正门位置超出局部范围", preset.id)); // 防止预览 Gizmo 产生异常大包围。
    }
    Ok(()) // 当前项可以安全进入目录快照和建筑空间索引。
}

/// 校验一个局部矩形的有限中心与严格正半尺寸。
fn valid_rect(rect: BuildingFootprintRect) -> bool {
    finite_slice(&rect.center)
        && finite_slice(&rect.half_extents)
        && rect
            .center
            .iter()
            .all(|value| value.abs() <= MAX_BUILDING_LOCAL_DISTANCE)
        && rect
            .half_extents
            .iter()
            .all(|value| *value > 0.0 && *value <= MAX_BUILDING_LOCAL_DISTANCE) // OBB 计算不得接收退化轴。
}

/// 校验路径位于 assets 根且指向 GLB/GLTF。
fn safe_scene_asset_path(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_BUILDING_ASSET_PATH_BYTES
        || value.starts_with('/')
        || value.contains("..")
        || value.contains('\\')
    {
        return false; // 分配小写副本前先用常数时间长度门拒绝异常输入。
    }
    let lowercase = value.to_ascii_lowercase(); // 四 KiB 上限确保小写化分配有界。
    lowercase.ends_with(".glb") || lowercase.ends_with(".gltf") // 与普通摆放支持的场景格式保持一致。
}

/// 返回切片中全部浮点数是否有限。
fn finite_slice(values: &[f32]) -> bool {
    values.iter().all(|value| value.is_finite()) // 统一阻止 NaN 和无穷进入几何、排序或序列化比较。
}
