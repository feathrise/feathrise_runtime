//! 全局可编辑 PBR 地表预设的作者格式、稳定身份与原子快照。
//!
//! 【功能设计】地图只引用稳定预设 ID，不保存目录数组下标。基础色、法线和 ARM+高度数组
//! 是 F12 从源贴图派生的可重建资源，三张数组切片与 `presets` 顺序一一对应。

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use bevy::reflect::TypePath;
use serde::{Deserialize, Serialize};

use crate::MapSurfaceLayer;

/// 当前全局地表预设目录格式版本。
pub const SURFACE_PRESET_SCHEMA_VERSION: u32 = 1;

/// 全局 PBR 地表预设目录及其可重建数组路径。
#[derive(Asset, TypePath, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SurfacePresetCatalogAsset {
    pub schema_version: u32,
    /// 三张数组的每层统一尺寸；保存时会把各源图重采样到该尺寸。
    pub texture_resolution: [u32; 2],
    pub base_color_array_path: String,
    pub normal_array_path: String,
    pub arm_height_array_path: String,
    /// 数量不设产品上限；实际切片数受当前 GPU 数组层数和资源预算约束。
    pub presets: Vec<SurfacePresetDescriptor>,
}

/// 一个可由 F12 新建、编辑和引用的 PBR 地表预设。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfacePresetDescriptor {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub label: String,
    /// 线性采样后与基础色纹理相乘的作者 sRGBA 颜色。
    pub base_color_srgba: [f32; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_color_texture_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normal_texture_path: Option<String>,
    /// R=AO、G=感知粗糙度、B=金属度、A=高度。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm_height_texture_path: Option<String>,
    pub tile_meters: f32,
    pub normal_strength: f32,
    /// DirectX 法线贴图需要在取样后反转绿通道。
    #[serde(default)]
    pub normal_y_flip: bool,
    pub height_blend_strength: f32,
    pub perceptual_roughness: f32,
    pub metallic: f32,
    /// 技能浅坑深度乘数，独立于PBR粗糙度；旧目录默认为1。
    #[serde(default = "default_crater_response")]
    pub crater_response: f32,
    #[serde(default)]
    pub projection: SurfaceProjection,
}

fn default_crater_response() -> f32 {
    1.0 // 不改变已有地表的浅坑深度。
}

/// 地表纹理的世界稳定投影策略。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceProjection {
    /// 高度地形默认沿世界 X/Z 平铺，两次导数直接选择完整 Mip。
    #[default]
    WorldXz,
    /// 陱坡沿三轴加权，用额外取样换取悬崖无拉伸。
    Triplanar,
}

impl SurfaceProjection {
    /// GPU 存储表使用的稳定整数编码。
    pub const fn code(self) -> u32 {
        match self {
            Self::WorldXz => 0,
            Self::Triplanar => 1,
        }
    }
}

/// 已完整校验的不可变目录快照。
#[derive(Clone, Debug, Default)]
pub struct ResolvedSurfacePresetCatalog {
    presets: Vec<SurfacePresetDescriptor>,
    lookup: BTreeMap<String, usize>,
    base_color_array_path: String,
    normal_array_path: String,
    arm_height_array_path: String,
    texture_resolution: [u32; 2],
}

impl ResolvedSurfacePresetCatalog {
    pub fn build(asset: &SurfacePresetCatalogAsset) -> Result<Self, String> {
        validate_surface_catalog(asset)?;
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
            base_color_array_path: asset.base_color_array_path.clone(),
            normal_array_path: asset.normal_array_path.clone(),
            arm_height_array_path: asset.arm_height_array_path.clone(),
            texture_resolution: asset.texture_resolution,
        })
    }

    pub fn presets(&self) -> &[SurfacePresetDescriptor] {
        &self.presets
    }

    pub fn resolve(&self, id: &str) -> Option<&SurfacePresetDescriptor> {
        self.lookup.get(id).map(|index| &self.presets[*index])
    }

    /// 返回当前数组切片；仅在当前快照内有意义，不得持久化进地图。
    pub fn texture_slice(&self, id: &str) -> Option<u32> {
        self.lookup
            .get(id)
            .and_then(|index| u32::try_from(*index).ok()) // KTX2切片是u32协议字段，不允许极端损坏目录静默截断。
    }

    pub fn array_paths(&self) -> (&str, &str, &str) {
        (
            &self.base_color_array_path,
            &self.normal_array_path,
            &self.arm_height_array_path,
        )
    }

    pub const fn texture_resolution(&self) -> [u32; 2] {
        self.texture_resolution
    }
}

impl SurfacePresetDescriptor {
    /// 为地图创建一个稳定局部层引用。
    pub fn create_map_layer(&self) -> MapSurfaceLayer {
        MapSurfaceLayer {
            id: self.id.clone(),
            label: self.label.clone(),
            preset_id: self.id.clone(),
        }
    }
}

fn validate_surface_catalog(asset: &SurfacePresetCatalogAsset) -> Result<(), String> {
    if asset.schema_version != SURFACE_PRESET_SCHEMA_VERSION {
        return Err(format!(
            "地表预设目录版本{}与运行时版本{}不一致",
            asset.schema_version, SURFACE_PRESET_SCHEMA_VERSION
        ));
    }
    let texture_bytes_are_addressable = usize::try_from(asset.texture_resolution[0])
        .ok()
        .and_then(|width| {
            usize::try_from(asset.texture_resolution[1])
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .is_some(); // 只拒绝当前平台无法表达的尺寸，不设置任意产品分辨率上限。
    let array_paths = [
        asset.base_color_array_path.as_str(),
        asset.normal_array_path.as_str(),
        asset.arm_height_array_path.as_str(),
    ];
    if asset.texture_resolution.iter().any(|value| *value == 0)
        || !texture_bytes_are_addressable
        || !safe_asset_path(&asset.base_color_array_path, &["ktx2"])
        || !safe_asset_path(&asset.normal_array_path, &["ktx2"])
        || !safe_asset_path(&asset.arm_height_array_path, &["ktx2"])
        || array_paths.into_iter().collect::<BTreeSet<_>>().len() != array_paths.len()
    {
        return Err("地表预设数组尺寸、内存表达或三个独立派生 KTX2 路径无效".to_owned());
    }
    // 空工程尚无作者地表；保留尺寸/路径校验，只有实际绘制或引用时才要求具体预设。
    if u32::try_from(asset.presets.len()).is_err() {
        return Err("地表预设数量超出 KTX2 数组层协议可表达范围".to_owned());
    }
    let mut names = BTreeSet::new();
    for preset in &asset.presets {
        validate_surface_preset(preset)?;
        if !names.insert(preset.id.as_str()) {
            return Err(format!("地表预设 ID 重复：{}", preset.id));
        }
        for alias in &preset.aliases {
            if !names.insert(alias.as_str()) {
                return Err(format!("地表预设历史别名重复或占用现有 ID：{alias}"));
            }
        }
    }
    Ok(()) // 这里故意不设预设数量上限。
}

fn validate_surface_preset(preset: &SurfacePresetDescriptor) -> Result<(), String> {
    // KTX2是派生运行时数组；作者源必须能由后台CPU解码器稳定读取。
    let source_extensions = ["png", "jpg", "jpeg"];
    if !safe_stable_name(&preset.id)
        || preset.aliases.iter().any(|alias| !safe_stable_name(alias))
        || preset.label.trim().is_empty()
        || preset
            .base_color_texture_path
            .as_deref()
            .is_some_and(|path| !safe_asset_path(path, &source_extensions))
        || preset
            .normal_texture_path
            .as_deref()
            .is_some_and(|path| !safe_asset_path(path, &source_extensions))
        || preset
            .arm_height_texture_path
            .as_deref()
            .is_some_and(|path| !safe_asset_path(path, &source_extensions))
        || preset
            .base_color_srgba
            .iter()
            .any(|value| !value.is_finite())
        || preset
            .base_color_srgba
            .iter()
            .any(|value| !(0.0..=1.0).contains(value))
        || ![
            preset.tile_meters,
            preset.normal_strength,
            preset.height_blend_strength,
            preset.perceptual_roughness,
            preset.metallic,
            preset.crater_response,
        ]
        .iter()
        .all(|value| value.is_finite())
        || preset.tile_meters <= 0.0
        || preset.normal_strength < 0.0
        || preset.height_blend_strength < 0.0
        || !(0.0..=1.0).contains(&preset.perceptual_roughness)
        || !(0.0..=1.0).contains(&preset.metallic)
        || !(0.0..=1.0).contains(&preset.crater_response)
    {
        return Err(format!(
            "地表预设 {} 的身份、路径或 PBR 参数无效",
            preset.id
        ));
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

    fn valid_catalog() -> SurfacePresetCatalogAsset {
        SurfacePresetCatalogAsset {
            schema_version: SURFACE_PRESET_SCHEMA_VERSION,
            texture_resolution: [1024, 1024],
            base_color_array_path: "textures/surface/base.ktx2".to_owned(),
            normal_array_path: "textures/surface/normal.ktx2".to_owned(),
            arm_height_array_path: "textures/surface/arm_height.ktx2".to_owned(),
            presets: vec![SurfacePresetDescriptor {
                id: "wet_soil".to_owned(),
                aliases: vec!["old_mud".to_owned()],
                label: "湿土".to_owned(),
                base_color_srgba: [1.0; 4],
                base_color_texture_path: Some("textures/mud.jpg".to_owned()),
                normal_texture_path: None,
                arm_height_texture_path: Some("textures/mud_arm.png".to_owned()),
                tile_meters: 3.0,
                normal_strength: 1.0,
                normal_y_flip: false,
                height_blend_strength: 0.4,
                perceptual_roughness: 1.0,
                metallic: 0.0,
                crater_response: 1.0,
                projection: SurfaceProjection::WorldXz,
            }],
        }
    }

    #[test]
    fn aliases_resolve_without_becoming_persisted_slices() {
        let resolved = ResolvedSurfacePresetCatalog::build(&valid_catalog()).unwrap();
        assert_eq!(resolved.resolve("old_mud").unwrap().id, "wet_soil");
        assert_eq!(resolved.texture_slice("wet_soil"), Some(0));
        assert_eq!(
            resolved
                .resolve("wet_soil")
                .unwrap()
                .create_map_layer()
                .preset_id,
            "wet_soil"
        );
    }

    #[test]
    fn catalog_rejects_duplicate_aliases_and_invalid_gpu_values() {
        let mut duplicate = valid_catalog();
        let mut second = duplicate.presets[0].clone();
        second.id = "grass_soil".to_owned();
        second.aliases = vec!["old_mud".to_owned()];
        duplicate.presets.push(second);
        assert!(ResolvedSurfacePresetCatalog::build(&duplicate).is_err());

        let mut invalid = valid_catalog();
        invalid.presets[0].normal_strength = f32::NAN;
        assert!(ResolvedSurfacePresetCatalog::build(&invalid).is_err());

        let mut invalid_response = valid_catalog();
        invalid_response.presets[0].crater_response = 1.2;
        assert!(ResolvedSurfacePresetCatalog::build(&invalid_response).is_err()); // 不允许放大超出技能深度的材质系数。

        let mut aliased_arrays = valid_catalog();
        aliased_arrays.normal_array_path = aliased_arrays.base_color_array_path.clone();
        assert!(ResolvedSurfacePresetCatalog::build(&aliased_arrays).is_err()); // 不同颜色空间与采样语义不能指向同一派生文件。
    }

    #[test]
    fn old_surface_presets_keep_full_crater_response() {
        let mut descriptor = serde_json::to_value(&valid_catalog().presets[0]).unwrap();
        descriptor
            .as_object_mut()
            .unwrap()
            .remove("crater_response");
        let restored: SurfacePresetDescriptor = serde_json::from_value(descriptor).unwrap();
        assert_eq!(restored.crater_response, 1.0); // 历史预设不需要重写JSON。
    }
}

#[cfg(test)]
#[path = "surface_presets/fixture_tests.rs"]
mod fixture_tests; // 公共目录用中性数据回归；私有素材与派生数组发布审计由原游戏维护。
