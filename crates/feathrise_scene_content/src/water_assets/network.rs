//! 三维河流样条、急流段与瀑布连接的公共作者数据和完整合法性校验。
//!
//! 【功能设计】本模块只保存可序列化纯数据并执行与场景内容规模无关的有界校验；
//! R8湖泊/池塘继续由父模块维护，运行时与编辑器通过父模块统一的`SceneWaterAsset::validate`
//! 一次验证全局稳定ID、静水引用和样条合法性，避免出现跨模块半套合法状态。

use std::collections::BTreeSet; // 全局水体ID、河流ID和路径内控制点ID使用确定顺序校验。

use serde::{Deserialize, Serialize}; // 新字段继续进入现有JSON作者资产且兼容旧文件缺省。

use super::{SceneWaterLayer, default_water_color, default_water_roughness}; // 河流与瀑布引用正式静水层并复用唯一材质默认值。
use crate::SceneId; // 网络校验诊断携带当前水域文件的稳定场景身份。

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_SCENE_WATER_RIVERS: usize = 32;

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_SCENE_WATERFALLS: usize = 32;

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_SCENE_WATER_RIVER_POINTS: usize = 64;

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_SCENE_WATERFALL_POINTS: usize = 16;

/// 旧预算常量保留API兼容与回归阈值，不再限制作者内容数量。
pub const MAX_SCENE_WATER_SPLINE_POINTS: usize = 512;

/// 样条水体允许的最小宽度，避免退化为零面积带状网格。
pub const MIN_SCENE_WATER_SPLINE_WIDTH_METERS: f32 = 0.10;

/// 样条水体允许的最大宽度，覆盖当前八公里场景但拒绝无界几何。
pub const MAX_SCENE_WATER_SPLINE_WIDTH_METERS: f32 = 8_192.0;

/// 样条水体允许的最小深度，保留稳定的接触与河床语义。
pub const MIN_SCENE_WATER_SPLINE_DEPTH_METERS: f32 = 0.01;

/// 样条水体允许的最大深度，拒绝损坏文件制造异常碰撞与水下范围。
pub const MAX_SCENE_WATER_SPLINE_DEPTH_METERS: f32 = 1_024.0;

/// 水域作者流速倍率的统一上限；静水层、河流和瀑布共用同一材质域。
pub const MAX_SCENE_WATER_FLOW_SPEED: f32 = 32.0;

/// 相邻样条控制点允许的最短三维距离，避免零长切线和NaN法向。
pub const MIN_SCENE_WATER_SPLINE_SEGMENT_METERS: f32 = 0.10;

/// 河流下游点允许的浮点上抬误差；超过该值会形成违反流向的逆坡。
pub const SCENE_WATER_DOWNHILL_EPSILON_METERS: f32 = 0.0001;

/// 水平R8水体的正式作者类别；两类共享几何，只用于工具语义和后续玩法配置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneStillWaterKind {
    /// 湖泊、水库和大面积静水默认使用该类别；旧水层自动落到这里。
    #[default]
    Lake,
    /// 池塘和小型蓄水区使用该类别。
    Pond,
}

/// 一条具有连续高程的河流；控制点顺序同时定义从上游到下游的流向。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneWaterRiver {
    /// 场景内稳定河流ID，并与静水体和瀑布ID全局互斥。
    pub id: String,
    /// 编辑器显示名称。
    pub label: String,
    /// 可选上游静水体；存在时必须引用当前文件中的稳定层ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_layer_id: Option<String>,
    /// 可选下游静水体；存在时必须引用当前文件中的稳定层ID。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_layer_id: Option<String>,
    /// 从上游到下游排列的三维作者控制点。
    pub points: Vec<SceneWaterSplinePoint>,
    /// 河面标准材质sRGBA颜色，A通道控制整体透明度。
    #[serde(default = "default_water_color")]
    pub base_color_srgba: [f32; 4],
    /// 河面感知粗糙度。
    #[serde(default = "default_water_roughness")]
    pub perceptual_roughness: f32,
}

/// 河流与瀑布共用的三维样条控制点。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneWaterSplinePoint {
    /// 当前路径内稳定点ID，插入和删除控制点后仍可绑定撤销与拖动事务。
    pub id: String,
    /// 控制点世界坐标，Y直接表达河流落差或瀑布高度。
    pub position: [f32; 3],
    /// 当前截面的完整水面宽度，单位为米。
    pub width: f32,
    /// 当前截面的玩法水深，单位为米。
    pub depth: f32,
    /// 当前截面的作者流速倍率。
    pub flow_speed: f32,
    /// 从当前点通往下一点的水流段类别；末点该字段只保留稳定默认值。
    #[serde(default)]
    pub outgoing: SceneWaterSegmentKind,
}

/// 河流样条相邻控制点之间的表面类别。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneWaterSegmentKind {
    /// 普通连续河面。
    #[default]
    River,
    /// 坡度较大但仍连续贴近河床的急流段。
    Rapid,
}

/// 一个连接高低水体的独立瀑布水幕。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneWaterfall {
    /// 场景内稳定瀑布ID，并与静水体和河流ID全局互斥。
    pub id: String,
    /// 编辑器显示名称。
    pub label: String,
    /// 瀑布上游端点；正式数据必须完整填写。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SceneWaterEndpoint>,
    /// 瀑布下游端点；正式数据必须完整填写。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SceneWaterEndpoint>,
    /// 从瀑布口到落水区排列的三维带状水幕控制点。
    pub points: Vec<SceneWaterSplinePoint>,
    /// 水幕顶底白亮泡沫表现强度，零表示关闭而一表示完整效果。
    #[serde(default = "default_waterfall_foam_strength")]
    pub foam_strength: f32,
    /// 水幕标准材质sRGBA颜色，A通道控制整体透明度。
    #[serde(default = "default_water_color")]
    pub base_color_srgba: [f32; 4],
    /// 水幕感知粗糙度。
    #[serde(default = "default_water_roughness")]
    pub perceptual_roughness: f32,
}

/// 瀑布对静水体或河流端点的稳定引用。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SceneWaterEndpoint {
    /// 引用一个水平湖泊或池塘。
    StillWater {
        /// 被引用的稳定静水层ID。
        layer_id: String,
    },
    /// 引用一条河流的上游首端。
    RiverStart {
        /// 被引用的稳定河流ID。
        river_id: String,
    },
    /// 引用一条河流的下游末端。
    RiverEnd {
        /// 被引用的稳定河流ID。
        river_id: String,
    },
}

/// 新瀑布缺省时采用可见但克制的水幕亮沫强度，旧文件不受该新增字段影响。
fn default_waterfall_foam_strength() -> f32 {
    0.65
}

/// 校验河流与瀑布共用的PBR参数，避免非有限值进入材质Uniform。
fn validate_water_material(
    owner: &str,
    base_color_srgba: [f32; 4],
    perceptual_roughness: f32,
) -> Result<(), String> {
    if !base_color_srgba
        .iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
        || !perceptual_roughness.is_finite()
        || !(0.0..=1.0).contains(&perceptual_roughness)
    {
        return Err(format!("{owner}包含无效水色或粗糙度"));
    }
    Ok(())
}

/// 校验一条样条的最少点数、稳定点ID和不会退化的米制截面。
fn validate_spline_points(
    owner: &str,
    points: &[SceneWaterSplinePoint],
    _legacy_max_points: usize, // 兼容既有调用；作者路径不再使用固定点数门。
) -> Result<(), String> {
    if points.len() < 2 {
        return Err(format!("{owner}至少需要两个控制点"));
    }
    let mut point_ids = BTreeSet::new();
    for point in points {
        if point.id.is_empty() || !point_ids.insert(point.id.as_str()) {
            return Err(format!("{owner}包含空白或重复控制点ID"));
        }
        if !point.position.iter().all(|value| value.is_finite())
            || !point.width.is_finite()
            || !(MIN_SCENE_WATER_SPLINE_WIDTH_METERS..=MAX_SCENE_WATER_SPLINE_WIDTH_METERS)
                .contains(&point.width)
            || !point.depth.is_finite()
            || !(MIN_SCENE_WATER_SPLINE_DEPTH_METERS..=MAX_SCENE_WATER_SPLINE_DEPTH_METERS)
                .contains(&point.depth)
            || !point.flow_speed.is_finite()
            || !(0.0..=MAX_SCENE_WATER_FLOW_SPEED).contains(&point.flow_speed)
        {
            return Err(format!(
                "{owner}控制点{}包含无效位置、宽度、深度或流速",
                point.id
            ));
        }
    }
    let minimum_segment_squared =
        MIN_SCENE_WATER_SPLINE_SEGMENT_METERS * MIN_SCENE_WATER_SPLINE_SEGMENT_METERS;
    for pair in points.windows(2) {
        let delta_x = pair[1].position[0] - pair[0].position[0];
        let delta_y = pair[1].position[1] - pair[0].position[1];
        let delta_z = pair[1].position[2] - pair[0].position[2];
        let length_squared = delta_x * delta_x + delta_y * delta_y + delta_z * delta_z;
        if length_squared < minimum_segment_squared {
            return Err(format!(
                "{owner}控制点{}与{}距离小于{MIN_SCENE_WATER_SPLINE_SEGMENT_METERS:.2}米",
                pair[0].id, pair[1].id
            ));
        }
    }
    Ok(())
}

/// 校验一个瀑布端点只引用当前文件中已经登记的静水体或河流。
fn validate_water_endpoint(
    owner: &str,
    endpoint: &SceneWaterEndpoint,
    layer_ids: &BTreeSet<&str>,
    river_ids: &BTreeSet<&str>,
) -> Result<(), String> {
    let valid = match endpoint {
        SceneWaterEndpoint::StillWater { layer_id } => layer_ids.contains(layer_id.as_str()),
        SceneWaterEndpoint::RiverStart { river_id } | SceneWaterEndpoint::RiverEnd { river_id } => {
            river_ids.contains(river_id.as_str())
        }
    };
    if !valid {
        return Err(format!("{owner}引用了不存在的水体端点"));
    }
    Ok(())
}

/// 使用全水网络统一的一万分之一米容差比较连接坐标。
fn connection_value_matches(actual: f32, expected: f32) -> bool {
    (actual - expected).abs() <= SCENE_WATER_DOWNHILL_EPSILON_METERS // 与逐段不抬升校验共用同一明确容差。
}

/// 校验河流首尾Y与其可选静水端点水位连续。
fn validate_still_water_connection_y(
    owner: &str,
    endpoint_label: &str,
    actual_y: f32,
    layer_id: &str,
    layers: &[SceneWaterLayer],
) -> Result<(), String> {
    let expected_y = layers
        .iter()
        .find(|layer| layer.id == layer_id)
        .map(|layer| layer.water_level)
        .ok_or_else(|| format!("{owner}引用了不存在的{endpoint_label}静水体{layer_id}"))?;
    if !connection_value_matches(actual_y, expected_y) {
        return Err(format!(
            "{owner}{endpoint_label}控制点Y必须与静水体{layer_id}水位连续"
        )); // 正式资产不能依赖运行时生成连接斜坡或垂直补片。
    }
    Ok(())
}

/// 校验瀑布首尾控制点与StillWater的Y或River端点的完整XYZ连续。
fn validate_waterfall_endpoint_continuity(
    owner: &str,
    endpoint_label: &str,
    actual_position: [f32; 3],
    endpoint: &SceneWaterEndpoint,
    layers: &[SceneWaterLayer],
    rivers: &[SceneWaterRiver],
) -> Result<(), String> {
    let matches = match endpoint {
        SceneWaterEndpoint::StillWater { layer_id } => layers
            .iter()
            .find(|layer| layer.id == layer_id.as_str())
            .is_some_and(|layer| connection_value_matches(actual_position[1], layer.water_level)), // 静水覆盖没有唯一XZ连接点，只约束水位。
        SceneWaterEndpoint::RiverStart { river_id } => rivers
            .iter()
            .find(|river| river.id == river_id.as_str())
            .and_then(|river| river.points.first())
            .is_some_and(|point| {
                (0..3).all(|axis| {
                    connection_value_matches(actual_position[axis], point.position[axis])
                })
            }), // 河端具有唯一三维中心点，瀑布必须完整贴合。
        SceneWaterEndpoint::RiverEnd { river_id } => rivers
            .iter()
            .find(|river| river.id == river_id.as_str())
            .and_then(|river| river.points.last())
            .is_some_and(|point| {
                (0..3).all(|axis| {
                    connection_value_matches(actual_position[axis], point.position[axis])
                })
            }), // 河尾同样要求完整XYZ连续，不能只对齐水位。
    };
    if !matches {
        return Err(format!("{owner}{endpoint_label}控制点与引用水体端点不连续")); // 所有引用已先验证存在，因此这里只报告明确几何错位。
    }
    Ok(())
}

/// 从父资产统一校验入口验证全部河流、瀑布、跨水体引用和完整样条合法性。
pub(super) fn validate_water_network<'a>(
    scene_id: &SceneId,
    rivers: &'a [SceneWaterRiver],
    waterfalls: &'a [SceneWaterfall],
    ids: &mut BTreeSet<&'a str>,
    layer_ids: &BTreeSet<&'a str>,
    layers: &'a [SceneWaterLayer],
) -> Result<(), String> {
    let mut river_ids = BTreeSet::new();
    for river in rivers {
        if river.id.is_empty() || !ids.insert(river.id.as_str()) {
            return Err(format!("场景{}包含空白或重复水体ID", scene_id));
        }
        river_ids.insert(river.id.as_str());
    }
    for waterfall in waterfalls {
        if waterfall.id.is_empty() || !ids.insert(waterfall.id.as_str()) {
            return Err(format!("场景{}包含空白或重复水体ID", scene_id));
        }
    }
    let mut total_spline_points = 0usize;
    for river in rivers {
        let owner = format!("河流{}", river.id);
        validate_spline_points(&owner, &river.points, MAX_SCENE_WATER_RIVER_POINTS)?;
        if river.points.windows(2).any(|pair| {
            pair[1].position[1] > pair[0].position[1] + SCENE_WATER_DOWNHILL_EPSILON_METERS
        }) {
            return Err(format!("{owner}控制点必须按水面Y不升高的下游顺序排列"));
        }
        validate_water_material(&owner, river.base_color_srgba, river.perceptual_roughness)?;
        if let Some(source) = river.source_layer_id.as_deref()
            && !layer_ids.contains(source)
        {
            return Err(format!("{owner}引用了不存在的上游静水体{source}"));
        }
        if let Some(source) = river.source_layer_id.as_deref() {
            validate_still_water_connection_y(
                &owner,
                "上游",
                river.points.first().map_or(0.0, |point| point.position[1]),
                source,
                layers,
            )?; // 河流首点必须从引用静水的真实水位连续流出。
        }
        if let Some(target) = river.target_layer_id.as_deref()
            && !layer_ids.contains(target)
        {
            return Err(format!("{owner}引用了不存在的下游静水体{target}"));
        }
        if let Some(target) = river.target_layer_id.as_deref() {
            validate_still_water_connection_y(
                &owner,
                "下游",
                river.points.last().map_or(0.0, |point| point.position[1]),
                target,
                layers,
            )?; // 河流末点必须与引用静水的真实水位连续汇合。
        }
        if river.source_layer_id.is_some() && river.source_layer_id == river.target_layer_id {
            return Err(format!("{owner}的上游和下游端点不能相同"));
        }
        total_spline_points = total_spline_points
            .checked_add(river.points.len())
            .ok_or_else(|| "水域样条控制点总数溢出".to_owned())?;
    }
    for waterfall in waterfalls {
        let owner = format!("瀑布{}", waterfall.id);
        validate_spline_points(&owner, &waterfall.points, MAX_SCENE_WATERFALL_POINTS)?;
        if waterfall.points.windows(2).any(|pair| {
            pair[1].position[1] > pair[0].position[1] + SCENE_WATER_DOWNHILL_EPSILON_METERS
        }) {
            return Err(format!("{owner}控制点必须按水面Y不升高的落水顺序排列")); // 任一局部抬升都会让瀑布材质流向逆转，不能只检查首尾总落差。
        }
        let waterfall_drop = waterfall
            .points
            .first()
            .zip(waterfall.points.last())
            .map(|(start, end)| start.position[1] - end.position[1])
            .unwrap_or_default();
        if waterfall_drop < MIN_SCENE_WATER_SPLINE_SEGMENT_METERS {
            return Err(format!(
                "{owner}终点水面Y必须比起点至少低{MIN_SCENE_WATER_SPLINE_SEGMENT_METERS:.2}米"
            ));
        }
        validate_water_material(
            &owner,
            waterfall.base_color_srgba,
            waterfall.perceptual_roughness,
        )?;
        if !waterfall.foam_strength.is_finite() || !(0.0..=1.0).contains(&waterfall.foam_strength) {
            return Err(format!("{owner}包含无效泡沫强度"));
        }
        let source = waterfall
            .source
            .as_ref()
            .ok_or_else(|| format!("{owner}缺少上游端点"))?;
        let target = waterfall
            .target
            .as_ref()
            .ok_or_else(|| format!("{owner}缺少下游端点"))?;
        if source == target {
            return Err(format!("{owner}的上游和下游端点不能相同"));
        }
        validate_water_endpoint(&owner, source, layer_ids, &river_ids)?;
        validate_water_endpoint(&owner, target, layer_ids, &river_ids)?;
        validate_waterfall_endpoint_continuity(
            &owner,
            "上游",
            waterfall
                .points
                .first()
                .map_or([0.0; 3], |point| point.position),
            source,
            layers,
            rivers,
        )?; // 瀑布首点必须贴合其稳定上游引用。
        validate_waterfall_endpoint_continuity(
            &owner,
            "下游",
            waterfall
                .points
                .last()
                .map_or([0.0; 3], |point| point.position),
            target,
            layers,
            rivers,
        )?; // 瀑布末点必须贴合其稳定下游引用。
        total_spline_points = total_spline_points
            .checked_add(waterfall.points.len())
            .ok_or_else(|| "水域样条控制点总数溢出".to_owned())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
