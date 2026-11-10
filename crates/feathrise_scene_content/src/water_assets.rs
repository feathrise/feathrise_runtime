//! 数据驱动场景的水平静水体与连续三维水网作者资产。
//!
//! 【功能设计】Lake/Pond 保留独立水位与R8覆盖；有地形的旧水层继续使用真实河床，
//! 空根水层则持有自身XZ范围，不要求地形画布。河流、急流和瀑布由显式三维控制点
//! 生成连续带状网格，全部水体沿用同一稳定身份、材质与后台构建和GPU采用链。

use std::collections::BTreeSet; // 校验稳定层ID不得重复。
use std::sync::Arc; // 无Mip R8作者源由资产管线冻结为可O(1)共享的不可变快照。

use bevy::prelude::*; // Asset和TypePath。
use bevy::reflect::TypePath; // JSON作者数据注册为Bevy资产。
use serde::{Deserialize, Serialize}; // 编辑器与运行时共享同一文件格式。

use crate::{SCENE_SCHEMA_VERSION, SceneId}; // 水域文件通过稳定场景ID归属场景私有目录。

mod network; // 三维河流、急流和瀑布独立维护类型、预算与纯校验逻辑。

pub use network::{
    SCENE_WATER_DOWNHILL_EPSILON_METERS, MAX_SCENE_WATER_FLOW_SPEED, MAX_SCENE_WATER_RIVER_POINTS,
    MAX_SCENE_WATER_RIVERS, MAX_SCENE_WATER_SPLINE_DEPTH_METERS, MAX_SCENE_WATER_SPLINE_POINTS,
    MAX_SCENE_WATER_SPLINE_WIDTH_METERS, MAX_SCENE_WATERFALL_POINTS, MAX_SCENE_WATERFALLS,
    MIN_SCENE_WATER_SPLINE_DEPTH_METERS, MIN_SCENE_WATER_SPLINE_SEGMENT_METERS,
    MIN_SCENE_WATER_SPLINE_WIDTH_METERS, SceneStillWaterKind, SceneWaterEndpoint, SceneWaterRiver,
    SceneWaterSegmentKind, SceneWaterSplinePoint, SceneWaterfall,
}; // 保持全部公共水域类型和固定预算继续从crate根路径稳定导出。

/// 旧八层预算常量仅兼容既有API；水层数量不再受此值限制。
pub const MAX_SCENE_WATER_LAYERS: usize = 8;

/// 正式水面默认感知粗糙度；在十级天空GGX链上主要采样清晰的Mip0/1。
pub const DEFAULT_SCENE_WATER_PERCEPTUAL_ROUGHNESS: f32 = 0.06; // 保留轻微水面柔化，同时恢复可辨认的天空倒影。

/// 已由Bevy异步资产管线读取的无Mip R8水域作者遮罩。
///
/// 【功能设计】这里保留编辑器保存的`.r8`权威作者源；`mask_texture_path`指向的KTX2
/// 继续作为GPU与文件派生物存在，但CPU建网格不得再从Image基础Mip深拷贝整图。
#[derive(Asset, TypePath, Clone, Debug)]
pub struct SceneWaterMaskAsset {
    pixels: Arc<[u8]>,  // 动态水后台任务可以O(1)取得与当前资产代次一致的只读遮罩。
    has_coverage: bool, // 资产IO任务一次扫描，帧线程据此跳过空层而不遍历4096²像素。
}

impl SceneWaterMaskAsset {
    /// 仅供本crate加载器用已完整读取的共享字节构造资产。
    pub(crate) fn from_shared(pixels: Arc<[u8]>) -> Self {
        let has_coverage = pixels.iter().any(|value| *value != 0); // 加载器运行在资产IO线程。
        Self {
            pixels,
            has_coverage,
        }
    }

    /// 返回不复制整张R8画布的稳定遮罩快照。
    pub fn shared_pixels(&self) -> Arc<[u8]> {
        Arc::clone(&self.pixels)
    }

    /// 返回像素数，供水JSON声明尺寸交叉校验。
    pub fn pixel_count(&self) -> usize {
        self.pixels.len()
    }

    /// 返回加载阶段冻结的非零覆盖证据；运行时空层不创建图片、材质或绘制实体。
    pub fn has_coverage(&self) -> bool {
        self.has_coverage
    }
}

/// 正式水域核心相对静止水位必须具备的最小深度。
///
/// 这是旧水面与低波幅预设的安全底线；数据驱动静水还会按四组振幅、玩家波上限和余量
/// 派生更大的实际核心深度，避免高风浪预设在波谷露出河床。
pub const MIN_STABLE_WATER_DEPTH_METERS: f32 = 0.20;

/// 单个场景全部可绘制水域层。
#[derive(Asset, TypePath, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneWaterAsset {
    /// 数据格式版本。
    #[serde(deserialize_with = "crate::types::deserialize_scene_schema_version")]
    pub schema_version: u32,
    /// 文件所属场景。
    pub scene_id: SceneId,
    /// 不同水位的水平湖泊或池塘；新场景默认为空，作者可显式创建或全部删除。
    pub layers: Vec<SceneWaterLayer>,
    /// 有连续高度、宽度、深度和流速的河流样条；旧文件缺省时保持为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rivers: Vec<SceneWaterRiver>,
    /// 连接高低水体的独立瀑布水幕；旧文件缺省时保持为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waterfalls: Vec<SceneWaterfall>,
}

impl Default for SceneWaterAsset {
    fn default() -> Self {
        Self {
            schema_version: SCENE_SCHEMA_VERSION,
            scene_id: SceneId::default(),
            layers: Vec::new(),
            rivers: Vec::new(),
            waterfalls: Vec::new(),
        }
    }
}

/// R8覆盖图生成的水平湖泊或池塘；保留旧层结构实现无损格式兼容。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneWaterLayer {
    /// 场景内稳定层ID。
    pub id: String,
    /// 编辑器显示名称。
    pub label: String,
    /// 作者可改写的无Mip R8覆盖源。
    pub mask_source_path: String,
    /// 运行时加载的完整Mip R8_UNORM KTX2。
    pub mask_texture_path: String,
    /// 覆盖源宽高。
    pub resolution: [u32; 2],
    /// 独立静水的源坐标XZ范围；旧文件缺省时沿用所属场景/地形画布范围。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_bounds: Option<[[f32; 2]; 2]>,
    /// 静止水面世界Y；波浪围绕此高度产生GPU位移。
    pub water_level: f32,
    /// 水平静水体的作者语义；旧水层统一迁移为湖泊而不改稳定ID和遮罩路径。
    #[serde(default)]
    pub still_kind: SceneStillWaterKind,
    /// 全局水体外观预设稳定 ID；空值仅用于读取尚未迁移的旧水域文件。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preset_id: String,
    /// X/Z平面归一化水流方向；静水风浪方向由所选预设独立定义。
    #[serde(default = "default_flow_direction")]
    pub flow_direction: [f32; 2],
    /// 作者水流速度；静水风浪与微法线使用预设内各自速度，不会被零流速停掉。
    #[serde(default = "default_flow_speed")]
    pub flow_speed: f32,
    /// 水面标准材质sRGBA颜色，A通道控制整体透明度。
    #[serde(default = "default_water_color")]
    pub base_color_srgba: [f32; 4],
    /// 水面感知粗糙度。
    #[serde(default = "default_water_roughness")]
    pub perceptual_roughness: f32,
}

impl SceneWaterLayer {
    /// 独立水画布不依赖地形占位范围；旧层继续使用原来源范围，不移动已有R8。
    pub fn effective_canvas_bounds(
        &self,
        fallback_min: [f32; 2],
        fallback_max: [f32; 2],
    ) -> ([f32; 2], [f32; 2]) {
        self.canvas_bounds.map_or((fallback_min, fallback_max), |bounds| (bounds[0], bounds[1]))
    }
}

fn default_flow_direction() -> [f32; 2] {
    [1.0, 0.0]
}

fn default_flow_speed() -> f32 {
    1.0
}

fn default_water_color() -> [f32; 4] {
    [0.055, 0.24, 0.29, 0.72]
}

fn default_water_roughness() -> f32 {
    DEFAULT_SCENE_WATER_PERCEPTUAL_ROUGHNESS // 旧文件缺字段与新作者水体共用已验收的清晰反射基准。
}

/// 返回可跨Windows/Linux稳定比较的资产路径键；拒绝所有可形成同一文件别名的词法写法。
fn portable_asset_path_key(path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\\') || path.contains('\0') {
        return None; // 只允许AssetServer统一使用的正斜杠UTF-8相对路径。
    }
    if path.split('/').any(|component| {
        component.is_empty()
            || component == "."
            || component == ".."
            || component.contains(':')
            || component.ends_with('.')
            || component.ends_with(' ')
    }) {
        return None; // 空段、当前/父目录、盘符及Win32尾随别名都不能进入作者资产。
    }
    Some(path.to_lowercase()) // 项目资产必须在默认大小写不敏感的Windows文件系统上也保持唯一。
}

fn has_asset_extension(path: &str, expected: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case(expected))
}

impl SceneWaterAsset {
    /// 校验作者数据的身份、预算和全部资产路径；失败数据不得进入后台建网格任务。
    pub fn validate(&self, expected_scene_id: &SceneId) -> Result<(), String> {
        if &self.scene_id != expected_scene_id {
            return Err(format!(
                "水域文件场景ID{}与当前场景{}不一致",
                self.scene_id, expected_scene_id
            ));
        }
        let mut ids = BTreeSet::new();
        let mut layer_ids = BTreeSet::new();
        let mut mask_paths = BTreeSet::new(); // raw R8与派生KTX2在全资产范围内都必须一对一拥有文件。
        for layer in &self.layers {
            if layer.id.is_empty() || !ids.insert(layer.id.as_str()) {
                return Err(format!("场景{}包含空白或重复水体ID", self.scene_id));
            }
            layer_ids.insert(layer.id.as_str());
            for (role, extension, path) in [
                ("R8作者源", "r8", &layer.mask_source_path),
                ("KTX2派生纹理", "ktx2", &layer.mask_texture_path),
            ] {
                let Some(path_key) = portable_asset_path_key(path) else {
                    return Err(format!(
                        "水域层{}的{}必须是无别名的assets相对路径",
                        layer.id, role
                    ));
                };
                if !has_asset_extension(path, extension) {
                    return Err(format!(
                        "水域层{}的{}必须使用.{}文件后缀",
                        layer.id, role, extension
                    )); // 角色后缀保证raw loader与KTX2派生物不会互相替代。
                }
                if !mask_paths.insert(path_key) {
                    return Err(format!(
                        "水域层{}的{}路径{}与另一水域输出冲突",
                        layer.id, role, path
                    )); // 同层source==texture和跨层复用都可能静默覆盖作者源。
                }
            }
            if layer.resolution[0] < 2
                || layer.resolution[1] < 2
                || (layer.resolution[0] as usize)
                    .checked_mul(layer.resolution[1] as usize)
                    .is_none()
            {
                return Err(format!(
                    "水域层{}分辨率{}×{}无效或尺寸乘积溢出",
                    layer.id, layer.resolution[0], layer.resolution[1]
                ));
            }
            if layer.canvas_bounds.is_some_and(|[min, max]| {
                !min.iter().chain(&max).all(|value| value.is_finite())
                    || min[0] >= max[0] || min[1] >= max[1]
                    || !(max[0] - min[0]).is_finite() || !(max[1] - min[1]).is_finite()
            }) {
                return Err(format!("水域层{}的独立画布范围无效", layer.id));
            }
            if !layer.water_level.is_finite()
                || !layer.flow_direction.iter().all(|value| value.is_finite())
                || !layer.flow_speed.is_finite()
                || !(0.0..=MAX_SCENE_WATER_FLOW_SPEED).contains(&layer.flow_speed)
                || !layer
                    .base_color_srgba
                    .iter()
                    .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
                || !layer.perceptual_roughness.is_finite()
                || !(0.0..=1.0).contains(&layer.perceptual_roughness)
            {
                return Err(format!("水域层{}包含无效水位、流速或材质参数", layer.id));
            }
            if !layer.preset_id.is_empty()
                && !layer
                    .preset_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            {
                return Err(format!("水域层{}的水体预设ID无效", layer.id));
            }
        }
        network::validate_water_network(
            &self.scene_id,     // 网络诊断继续使用父资产的稳定场景身份。
            &self.rivers,     // 河流样条由网络子模块完成点序、预算与引用校验。
            &self.waterfalls, // 瀑布与河流在同一全局ID和端点域中联合验证。
            &mut ids,         // 静水ID已经登记，网络ID不得与其碰撞。
            &layer_ids,       // 河流上下游和瀑布静水端点只能引用现有R8层。
            &self.layers,     // 正式连接连续性读取被引用静水体的真实作者水位。
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer() -> SceneWaterLayer {
        SceneWaterLayer {
            id: "river".to_owned(),
            label: "河水".to_owned(),
            mask_source_path: "scenes/MAP-test/water/river.r8".to_owned(),
            mask_texture_path: "scenes/MAP-test/water/river.ktx2".to_owned(),
            resolution: [512, 512],
            canvas_bounds: None,
            water_level: 0.0,
            still_kind: SceneStillWaterKind::Lake,
            preset_id: String::new(),
            flow_direction: [1.0, 0.0],
            flow_speed: 1.0,
            base_color_srgba: default_water_color(),
            perceptual_roughness: default_water_roughness(),
        }
    }

    #[test]
    fn water_asset_accepts_an_explicitly_empty_water_network() {
        let scene_id = SceneId::parse("MAP-empty-water").expect("测试场景ID合法");
        let asset = SceneWaterAsset {
            schema_version: crate::SCENE_SCHEMA_VERSION,
            scene_id: scene_id.clone(),
            layers: Vec::new(),
            rivers: Vec::new(),
            waterfalls: Vec::new(),
        };

        assert!(asset.validate(&scene_id).is_ok()); // 零静水、零河流、零瀑布是正式可保存状态。
    }

    #[test]
    fn water_asset_rejects_duplicate_layers_and_escaping_paths() {
        let scene_id = SceneId::parse("MAP-test").unwrap();
        let mut asset = SceneWaterAsset {
            schema_version: crate::SCENE_SCHEMA_VERSION,
            scene_id: scene_id.clone(),
            layers: vec![layer()],
            rivers: Vec::new(),     // 旧层校验测试不引入样条河流。
            waterfalls: Vec::new(), // 旧层校验测试不引入瀑布。
        };
        assert!(asset.validate(&scene_id).is_ok());
        asset.layers.push(layer());
        assert!(asset.validate(&scene_id).is_err());
        asset.layers.pop();
        asset.layers[0].mask_source_path = "../outside.r8".to_owned();
        assert!(asset.validate(&scene_id).is_err());
    }

    #[test]
    fn water_asset_rejects_mask_output_aliases_and_shared_files() {
        let scene_id = SceneId::parse("MAP-test").expect("测试场景ID合法");
        let mut asset = SceneWaterAsset {
            schema_version: crate::SCENE_SCHEMA_VERSION,
            scene_id: scene_id.clone(),
            layers: vec![layer()],
            rivers: Vec::new(),
            waterfalls: Vec::new(),
        };
        asset.layers[0].mask_texture_path = asset.layers[0].mask_source_path.clone();
        assert!(asset.validate(&scene_id).is_err()); // KTX2不得覆盖同层权威raw R8。

        asset.layers[0] = layer();
        asset.layers[0].mask_source_path = "scenes/MAP-test/water/./river.r8".to_owned();
        assert!(asset.validate(&scene_id).is_err()); // 当前目录段不得绕过字符串路径身份。

        asset.layers[0] = layer();
        asset.layers[0].mask_source_path = "scenes/MAP-test/scene.scene_manifest.json".to_owned();
        assert!(asset.validate(&scene_id).is_err()); // 水保存绝不能覆盖同场景清单或其他权威文件。

        asset.layers[0] = layer();
        let mut pond = layer();
        pond.id = "pond".to_owned();
        pond.mask_source_path = "scenes/MAP-test/water/RIVER.R8".to_owned();
        pond.mask_texture_path = "scenes/MAP-test/water/pond.ktx2".to_owned();
        asset.layers.push(pond);
        assert!(asset.validate(&scene_id).is_err()); // Windows大小写别名也不得让两层共享文件。
    }

    #[test]
    fn legacy_water_json_defaults_to_a_lake_without_flow_network() {
        let legacy = r#"{
            "schema_version": 2,
            "scene_id": "MAP-test",
            "layers": [{
                "id": "river",
                "label": "旧河水层",
                "mask_source_path": "scenes/MAP-test/water/river.r8",
                "mask_texture_path": "scenes/MAP-test/water/river.ktx2",
                "resolution": [512, 512],
                "water_level": -2.75
            }]
        }"#;
        let asset: SceneWaterAsset =
            serde_json::from_str(legacy).expect("旧水域JSON必须继续无损加载");
        let scene_id = SceneId::parse("MAP-test").expect("测试场景ID必须有效");

        assert_eq!(asset.layers.len(), 1); // 旧层数量和稳定身份不能被迁移吞掉。
        assert_eq!(asset.layers[0].id, "river"); // 旧稳定ID继续服务存档和遮罩路径。
        assert_eq!(asset.layers[0].water_level, -2.75); // 精确作者水位保持不变。
        assert_eq!(asset.layers[0].canvas_bounds, None); // 旧R8继续沿用原场景范围。
        assert_eq!(asset.layers[0].still_kind, SceneStillWaterKind::Lake); // 缺省类别迁移为湖泊。
        assert!(asset.layers[0].preset_id.is_empty()); // 旧文件明确走兼容外观，不伪造不存在的目录引用。
        assert_eq!(asset.layers[0].flow_direction, [1.0, 0.0]); // 旧流向默认仍保留。
        assert_eq!(asset.layers[0].flow_speed, 1.0); // 旧流速默认仍保留。
        assert_eq!(
            asset.layers[0].perceptual_roughness,
            DEFAULT_SCENE_WATER_PERCEPTUAL_ROUGHNESS,
        ); // 缺失粗糙度的旧水体采用清晰天空反射默认值。
        assert!(asset.rivers.is_empty()); // 兼容加载不能凭空生成样条河流。
        assert!(asset.waterfalls.is_empty()); // 兼容加载不能凭空生成瀑布。
        assert!(asset.validate(&scene_id).is_ok()); // 迁移后的完整资产必须通过正式校验。
    }

    #[test]
    fn independent_water_bounds_round_trip_and_reject_invalid_ranges() {
        let mut water = SceneWaterAsset {
            schema_version: crate::SCENE_SCHEMA_VERSION,
            scene_id: SceneId::parse("MAP-test").unwrap(),
            layers: vec![layer()],
            rivers: Vec::new(), waterfalls: Vec::new(),
        };
        let bounds = [[-128., -64.], [128., 64.]];
        water.layers[0].canvas_bounds = Some(bounds);
        assert!(water.validate(&water.scene_id).is_ok());
        let loaded: SceneWaterAsset = serde_json::from_slice(&serde_json::to_vec(&water).unwrap()).unwrap();
        assert_eq!(loaded, water);
        assert_eq!(loaded.layers[0].effective_canvas_bounds([0.; 2], [1.; 2]), (bounds[0], bounds[1]));
        for invalid in [[[0., 0.], [0., 1.]], [[1., 0.], [0., 1.]], [[f32::NAN, 0.], [1., 1.]],
            [[-f32::MAX, 0.], [f32::MAX, 1.]]] {
            water.layers[0].canvas_bounds = Some(invalid);
            assert!(water.validate(&water.scene_id).is_err());
        }
        water.layers[0].canvas_bounds = None;
        assert_eq!(water.layers[0].effective_canvas_bounds([-3.; 2], [4.; 2]), ([-3.; 2], [4.; 2]));
    }
}
