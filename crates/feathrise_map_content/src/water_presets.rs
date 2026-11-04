//! 全局可编辑水体外观预设、稳定身份和原子解析快照。
//!
//! 【功能设计】地图静水层只保存稳定 `preset_id`；四组宏观波和两组微法线保持固定 GPU 成本，
//! 预设目录本身不设置数量上限。水流速度与风浪速度分别保存，因此零流速湖面仍可受风起伏。

use std::collections::{BTreeMap, BTreeSet}; // 稳定 ID 与历史别名使用确定性索引。

use bevy::image::{
    // 统一静水微法线的线性数据与世界重复采样设置。
    ImageAddressMode,
    ImageFilterMode,
    ImageLoaderSettings,
    ImageSampler,
    ImageSamplerDescriptor, // 正式目录和未保存实时预览必须共享同一加载身份。
}; // 完成微法线加载设置导入。
use bevy::prelude::*; // Asset、Image、Handle 与 Resource 类型。
use bevy::reflect::TypePath; // JSON 目录注册为 Bevy 资产。
use serde::{Deserialize, Serialize}; // F12 与运行时共享同一作者格式。

/// 当前全局水体预设目录格式版本。
pub const WATER_PRESET_SCHEMA_VERSION: u32 = 1;

/// 每份预设固定计算的宏观解析波数量。
pub const WATER_MACRO_WAVE_COUNT: usize = 4;

/// 每份预设固定保留的滚动微法线层数量。
pub const WATER_MICRO_NORMAL_COUNT: usize = 2;

/// 使用正式静水采样设置加载一张微法线；目录热采用和F12未保存预览共用此唯一入口。
pub fn load_water_micro_normal(
    // 公开给编辑器草稿预览，避免同一路径先被错误sRGB设置占用。
    asset_server: &AssetServer, // AssetServer负责缓存、异步IO和热重载。
    path: String,               // 作者路径已经由目录或F12图片选择入口完成assets内校验。
) -> Handle<Image> {
    // 返回强句柄，由注册表或实时预览资源持有到候选完成。
    asset_server // 从全局资产服务建立带专用设置的加载请求。
        .load_builder() // 使用Bevy 0.19官方LoadBuilder覆盖图片默认设置。
        .with_settings(|settings: &mut ImageLoaderSettings| {
            // 法线方向数据不得按颜色空间解释。
            settings.is_srgb = false; // 切线空间法线保持线性采样。
            settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                // 世界平铺需要显式重复寻址与线性过滤。
                label: Some("water_micro_normal".into()), // GPU采样器诊断使用稳定名称。
                address_mode_u: ImageAddressMode::Repeat, // 水面U方向无限平铺。
                address_mode_v: ImageAddressMode::Repeat, // 水面V方向无限平铺。
                address_mode_w: ImageAddressMode::ClampToEdge, // 二维图片第三轴保持安全默认边界。
                mag_filter: ImageFilterMode::Linear,      // 放大时平滑法线方向。
                min_filter: ImageFilterMode::Linear,      // 缩小时平滑法线方向。
                mipmap_filter: ImageFilterMode::Linear,   // 跨Mip连续过渡，避免远景闪烁。
                ..Default::default()                      // 保留Bevy其余兼容采样器字段。
            }); // 完成专用微法线采样器设置。
        }) // 完成ImageLoaderSettings覆盖。
        .load(path) // 以唯一设置请求或复用对应图片资产。
} // 完成正式与草稿共用的微法线加载入口。

/// 开发者维护的全局水体预设目录。
#[derive(Asset, TypePath, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WaterPresetCatalogAsset {
    /// 目录格式版本。
    pub schema_version: u32,
    /// 任意数量的水体预设；地图从不持久化数组下标。
    pub presets: Vec<WaterPresetDescriptor>,
}

/// 一组固定成本的宏观方向波。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaterMacroWave {
    /// 世界 X/Z 平面的传播方向；加载后由运行时归一化。
    pub direction: [f32; 2],
    /// 波峰相对静止水位的振幅，单位米。
    pub amplitude_meters: f32,
    /// 相邻波峰的世界距离，单位米。
    pub wavelength_meters: f32,
    /// 波峰沿传播方向移动的速度，单位米每秒。
    pub speed_meters_per_second: f32,
    /// 水平 Gerstner 位移比例；零表示只保留垂直解析波。
    pub steepness: f32,
}

/// 一层沿世界 X/Z 滚动的切线空间法线。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaterMicroNormalLayer {
    /// PNG/JPG/JPEG 作者图或可直接发布的 KTX2；空值会绑定中性回退并关闭该层。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture_path: Option<String>,
    /// 纹理重复一次覆盖的世界米数。
    pub tile_meters: f32,
    /// 世界 X/Z 平面的独立滚动速度，单位米每秒。
    pub velocity_meters_per_second: [f32; 2],
    /// 对最终逐像素法线的贡献强度。
    pub strength: f32,
}

/// 一个可由 F12 新建、编辑和静水层引用的完整水体外观。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaterPresetDescriptor {
    /// 不随显示名或目录顺序改变的规范 ID。
    pub id: String,
    /// 重命名后仍可解析旧地图引用的历史 ID。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// F12 显示名称。
    pub label: String,
    /// StandardMaterial 使用的 sRGBA 水色；A 通道控制整体透明度。
    pub base_color_srgba: [f32; 4],
    /// StandardMaterial 感知粗糙度。
    pub perceptual_roughness: f32,
    /// StandardMaterial 非金属表面反射率。
    pub reflectance: f32,
    /// 玩家波谷对原始水色的最大压暗比例。
    pub trough_darkening: f32,
    /// 岸边顶点透明度作为波幅权重时使用的指数；一表示线性跟随现有岸带。
    pub shore_wave_attenuation: f32,
    /// 固定四组宏观解析波；预设数量不会改变每个顶点的循环次数。
    pub macro_waves: [WaterMacroWave; WATER_MACRO_WAVE_COUNT],
    /// 固定两层滚动微法线；没有贴图的层不贡献最终法线。
    pub micro_normals: [WaterMicroNormalLayer; WATER_MICRO_NORMAL_COUNT],
}

impl WaterPresetDescriptor {
    /// 返回宏观波可能产生的最大竖直负位移，供静水安全深度派生使用。
    pub fn maximum_vertical_displacement(&self) -> f32 {
        self.macro_waves
            .iter()
            .map(|wave| wave.amplitude_meters)
            .sum()
    }
}

/// 已完整校验、可按规范 ID 或历史别名查询的水体目录快照。
#[derive(Clone, Debug, Default)]
pub struct ResolvedWaterPresetCatalog {
    presets: Vec<WaterPresetDescriptor>,
    lookup: BTreeMap<String, usize>,
}

impl ResolvedWaterPresetCatalog {
    /// 校验整份目录并一次建立不可变查询快照。
    pub fn build(asset: &WaterPresetCatalogAsset) -> Result<Self, String> {
        validate_water_preset_catalog(asset)?;
        let mut lookup = BTreeMap::new();
        for (index, preset) in asset.presets.iter().enumerate() {
            lookup.insert(preset.id.clone(), index);
            for alias in &preset.aliases {
                lookup.insert(alias.clone(), index);
            }
        }
        Ok(Self {
            presets: asset.presets.clone(),
            lookup,
        })
    }

    /// 按开发者目录顺序返回全部预设。
    pub fn presets(&self) -> &[WaterPresetDescriptor] {
        &self.presets
    }

    /// 同时接受规范 ID 与历史别名。
    pub fn resolve(&self, id: &str) -> Option<&WaterPresetDescriptor> {
        self.lookup.get(id).map(|index| &self.presets[*index])
    }
}

/// 全局水体目录的强句柄、原子快照、微法线强句柄和热更诊断。
#[derive(Resource, Debug)]
pub struct WaterPresetRegistry {
    /// 启动时固定持有的目录资产句柄。
    pub catalog: Handle<WaterPresetCatalogAsset>,
    /// 整份目录成功采用后递增，水面任务用它拒收旧外观结果。
    pub revision: u64,
    /// 最近一次完整校验通过的目录。
    pub resolved: ResolvedWaterPresetCatalog,
    /// 每项预设的两张可选 Image 强句柄，避免等待期间被资产系统回收。
    image_handles: BTreeMap<String, [Option<Handle<Image>>; WATER_MICRO_NORMAL_COUNT]>,
    /// 最近一次目录加载或校验失败原因。
    pub last_error: Option<String>,
}

impl WaterPresetRegistry {
    /// 按规范 ID 或历史别名读取当前预设。
    pub fn resolve(&self, id: &str) -> Option<&WaterPresetDescriptor> {
        self.resolved.resolve(id)
    }

    /// 返回全部当前预设，不施加 UI 或产品数量上限。
    pub fn presets(&self) -> &[WaterPresetDescriptor] {
        self.resolved.presets()
    }

    /// 返回规范预设对应的两张可选微法线强句柄。
    pub fn image_handles(
        &self,
        id: &str,
    ) -> Option<&[Option<Handle<Image>>; WATER_MICRO_NORMAL_COUNT]> {
        let preset = self.resolve(id)?;
        self.image_handles.get(&preset.id)
    }

    /// 供目录采用系统一次替换全部派生强句柄。
    pub(crate) fn replace_image_handles(
        &mut self,
        handles: BTreeMap<String, [Option<Handle<Image>>; WATER_MICRO_NORMAL_COUNT]>,
    ) {
        self.image_handles = handles;
    }

    /// 创建尚未应用任何目录的启动占位注册表。
    pub(crate) fn loading(catalog: Handle<WaterPresetCatalogAsset>) -> Self {
        Self {
            catalog,
            revision: 0,
            resolved: Default::default(),
            image_handles: BTreeMap::new(),
            last_error: None,
        }
    }
}

/// 校验整份目录后才允许注册表替换上一完整快照。
fn validate_water_preset_catalog(asset: &WaterPresetCatalogAsset) -> Result<(), String> {
    if asset.schema_version != WATER_PRESET_SCHEMA_VERSION {
        return Err(format!(
            "水体预设目录版本{}与运行时版本{}不一致",
            asset.schema_version, WATER_PRESET_SCHEMA_VERSION
        ));
    }
    // 空工程可以没有水体预设；已有水面继续按自己的稳定ID解析，不能借用原游戏默认项。
    let mut names = BTreeSet::new();
    for preset in &asset.presets {
        validate_water_preset(preset)?;
        if !names.insert(preset.id.as_str()) {
            return Err(format!("水体预设 ID 重复：{}", preset.id));
        }
        for alias in &preset.aliases {
            if !names.insert(alias.as_str()) {
                return Err(format!("水体预设历史别名重复或占用现有 ID：{alias}"));
            }
        }
    }
    Ok(()) // 预设数量故意不设置产品上限。
}

/// 校验一个预设的 GPU 数值、身份和可选图片路径。
fn validate_water_preset(preset: &WaterPresetDescriptor) -> Result<(), String> {
    let scalar_values = [
        preset.perceptual_roughness,
        preset.reflectance,
        preset.trough_darkening,
        preset.shore_wave_attenuation,
    ];
    if !safe_stable_name(&preset.id)
        || preset.aliases.iter().any(|alias| !safe_stable_name(alias))
        || preset.label.trim().is_empty()
        || preset
            .base_color_srgba
            .iter()
            .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        || scalar_values.iter().any(|value| !value.is_finite())
        || !(0.0..=1.0).contains(&preset.perceptual_roughness)
        || !(0.0..=1.0).contains(&preset.reflectance)
        || !(0.0..=1.0).contains(&preset.trough_darkening)
        || !(0.1..=8.0).contains(&preset.shore_wave_attenuation)
    {
        return Err(format!(
            "水体预设 {} 的身份、颜色或 PBR 参数无效",
            preset.id
        ));
    }
    for (index, wave) in preset.macro_waves.iter().enumerate() {
        if wave.direction.iter().any(|value| !value.is_finite())
            || wave.direction[0] * wave.direction[0] + wave.direction[1] * wave.direction[1]
                <= f32::EPSILON
            || ![
                wave.amplitude_meters,
                wave.wavelength_meters,
                wave.speed_meters_per_second,
                wave.steepness,
            ]
            .iter()
            .all(|value| value.is_finite())
            || !(0.0..=10.0).contains(&wave.amplitude_meters)
            || !(0.1..=100_000.0).contains(&wave.wavelength_meters)
            || !(0.0..=1_000.0).contains(&wave.speed_meters_per_second)
            || !(0.0..=1.0).contains(&wave.steepness)
        {
            return Err(format!(
                "水体预设 {} 的宏观波{}参数无效",
                preset.id,
                index + 1
            ));
        }
    }
    if !preset.maximum_vertical_displacement().is_finite() {
        return Err(format!("水体预设 {} 的总波幅溢出", preset.id));
    }
    for (index, layer) in preset.micro_normals.iter().enumerate() {
        if layer
            .texture_path
            .as_deref()
            .is_some_and(|path| !safe_asset_path(path, &["png", "jpg", "jpeg", "ktx2"]))
            || !layer.tile_meters.is_finite()
            || !(0.01..=100_000.0).contains(&layer.tile_meters)
            || layer
                .velocity_meters_per_second
                .iter()
                .any(|value| !value.is_finite())
            || !layer.strength.is_finite()
            || !(0.0..=4.0).contains(&layer.strength)
        {
            return Err(format!(
                "水体预设 {} 的微法线{}参数无效",
                preset.id,
                index + 1
            ));
        }
    }
    Ok(())
}

fn safe_stable_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn safe_asset_path(value: &str, extensions: &[&str]) -> bool {
    let extension = value.rsplit_once('.').map(|(_, extension)| extension);
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains("..")
        && !value.contains('\\')
        && extension.is_some_and(|candidate| {
            extensions
                .iter()
                .any(|allowed| candidate.eq_ignore_ascii_case(allowed))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(id: &str) -> WaterPresetDescriptor {
        WaterPresetDescriptor {
            id: id.to_owned(),
            aliases: Vec::new(),
            label: id.to_owned(),
            base_color_srgba: [0.04, 0.18, 0.23, 0.72],
            perceptual_roughness: 0.08,
            reflectance: 0.55,
            trough_darkening: 0.18,
            shore_wave_attenuation: 1.0,
            macro_waves: [WaterMacroWave {
                direction: [1.0, 0.0],
                amplitude_meters: 0.02,
                wavelength_meters: 12.0,
                speed_meters_per_second: 0.8,
                steepness: 0.2,
            }; WATER_MACRO_WAVE_COUNT],
            micro_normals: std::array::from_fn(|_| WaterMicroNormalLayer {
                texture_path: None,
                tile_meters: 2.0,
                velocity_meters_per_second: [0.1, 0.0],
                strength: 0.0,
            }),
        }
    }

    #[test]
    fn aliases_resolve_and_catalog_size_has_no_product_limit() {
        let mut first = preset("calm_lake");
        first.aliases.push("old_lake".to_owned());
        let mut catalog = WaterPresetCatalogAsset {
            schema_version: WATER_PRESET_SCHEMA_VERSION,
            presets: vec![first],
        };
        catalog
            .presets
            .extend((0..1_000).map(|index| preset(&format!("water_{index}"))));
        let resolved = ResolvedWaterPresetCatalog::build(&catalog)
            .expect("一千项合法水体预设不应被产品数量上限拒绝");
        assert_eq!(resolved.presets().len(), 1_001);
        assert_eq!(resolved.resolve("old_lake").unwrap().id, "calm_lake");
    }

    #[test]
    fn invalid_wave_and_duplicate_alias_reject_the_whole_catalog() {
        let mut first = preset("lake");
        first.aliases.push("shared".to_owned());
        let mut second = preset("pond");
        second.aliases.push("shared".to_owned());
        let duplicate = WaterPresetCatalogAsset {
            schema_version: WATER_PRESET_SCHEMA_VERSION,
            presets: vec![first, second],
        };
        assert!(ResolvedWaterPresetCatalog::build(&duplicate).is_err());

        let mut invalid = preset("bad_wave");
        invalid.macro_waves[0].wavelength_meters = 0.0;
        assert!(
            ResolvedWaterPresetCatalog::build(&WaterPresetCatalogAsset {
                schema_version: WATER_PRESET_SCHEMA_VERSION,
                presets: vec![invalid],
            })
            .is_err()
        );
    }

}
