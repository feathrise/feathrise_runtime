//! 稳定地形身份与作者参数；形状由后端生成器定义，不与数据后端混为一种枚举。

use bevy::prelude::{Asset, EulerRot, TypePath, Quat, Transform, Vec3};
use serde::{Deserialize, Serialize};
use crate::{MapId, MapPuddleDescriptor, MapTerrainDescriptor};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerrainId(String);

impl TerrainId {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        MapId::parse(value.clone()).map_err(|_| "地形ID必须为1至64个ASCII字母、数字、短横线或下划线".to_owned())?;
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str { &self.0 }
    pub fn legacy() -> Self { Self("primary".into()) }
}

/// 缩放作用于完整源坐标系；作者格距保持不变，世界LOD距离与碰撞需换算。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerrainObjectTransform {
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default)]
    pub yaw_degrees: f32,
    /// 兼容旧清单的Y朝向；新增X/Z角默认为零，旋转统一按YXZ欧拉次序组合。
    #[serde(default, skip_serializing_if = "zero_angle")]
    pub pitch_degrees: f32,
    #[serde(default, skip_serializing_if = "zero_angle")]
    pub roll_degrees: f32,
    #[serde(default = "unit_scale")]
    pub scale: [f32; 3],
}
fn unit_scale() -> [f32; 3] { [1.0; 3] }
fn zero_angle(value: &f32) -> bool { *value == 0.0 }
impl Default for TerrainObjectTransform {
    fn default() -> Self { Self { translation: [0.0; 3], yaw_degrees: 0.0, pitch_degrees: 0.0, roll_degrees: 0.0, scale: unit_scale() } }
}
impl TerrainObjectTransform {
    /// 与实例展开和属性栏共用Y朝向、X俯仰、Z翻滚的唯一组合次序。
    pub fn rotation(self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw_degrees.rem_euclid(360.0).to_radians(),
            self.pitch_degrees.rem_euclid(360.0).to_radians(), self.roll_degrees.rem_euclid(360.0).to_radians())
    }
    /// 关联姿态的旋转用四元数组合后再保存，不能把三个欧拉角分别相加。
    pub fn with_rotation(mut self, rotation: Quat) -> Self {
        let (yaw, pitch, roll) = rotation.to_euler(EulerRot::YXZ);
        self.yaw_degrees = yaw.to_degrees().rem_euclid(360.0);
        self.pitch_degrees = pitch.to_degrees().rem_euclid(360.0);
        self.roll_degrees = roll.to_degrees().rem_euclid(360.0);
        self
    }
    pub fn as_transform(self) -> Transform {
        Transform::from_translation(Vec3::from_array(self.translation))
            .with_rotation(self.rotation())
            .with_scale(Vec3::from_array(self.scale))
    }
    pub fn validate(self) -> Result<(), String> {
        if !Vec3::from_array(self.translation).is_finite() || !self.yaw_degrees.is_finite()
            || !self.pitch_degrees.is_finite() || !self.roll_degrees.is_finite()
            || self.scale.iter().any(|v| !v.is_finite() || *v <= 0.0) {
            return Err("地形位置、朝向必须有限，三个轴的缩放必须有限且大于零".into());
        }
        let matrix = self.as_transform().to_matrix();
        if !matrix.is_finite() || !matrix.inverse().is_finite() {
            return Err("地形变换无法安全求逆".into());
        }
        Ok(())
    }
}

/// 体素自己的三维作者精度和初始实心范围；不要求先创建高度地形。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapVoxelTerrainDescriptor {
    pub cell_size_meters: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// 持久作者版本路径；与显示网格独立，LOD改变不写回密度源。
    pub source_path: String,
    pub display_lods: Vec<crate::MapCollisionLod>,
    pub collision_lods: Vec<crate::MapCollisionLod>,
}

/// 地形独占作者源与画布；全局材质/植被预设继续共享，不复制整个地图内容。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MapTerrainObject {
    /// 显式互指边；位置改变只暂停关系，不删除记录或重采样草地。
    #[serde(default, skip_serializing_if = "crate::clipmap_stitches::is_empty")]
    pub clipmap_stitches: std::sync::Arc<[crate::ClipmapStitch]>,
    /// 通用网格LOD覆盖不改写作者后端；缺项继承全局且旧JSON继续省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh_lod: Option<crate::MeshLodPolicy>,
    pub id: TerrainId,
    pub display_name: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub transform: TerrainObjectTransform,
    /// 源坐标范围；整体移动和缩放不重采样作者数据。
    pub world_min: [f32; 2],
    pub world_max: [f32; 2],
    pub terrain: MapTerrainDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voxel: Option<MapVoxelTerrainDescriptor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vegetation_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub puddles: Option<MapPuddleDescriptor>,
}

/// 对象结构历史只交换Arc；首次迁移与撤销同时恢复旧主地形语义。
#[derive(Clone)]
pub struct MapTerrainCollection {
    pub legacy: MapTerrainDescriptor,
    pub objects: Option<std::sync::Arc<[MapTerrainObject]>>,
}
impl MapTerrainCollection {
    pub fn capture(map: &crate::MapManifestAsset) -> Self {
        Self { legacy: map.terrain.clone(), objects: map.terrains.clone() }
    }
    pub fn apply(&self, map: &mut crate::MapManifestAsset) {
        map.terrain.clone_from(&self.legacy);
        map.terrains.clone_from(&self.objects);
    }
}
fn enabled() -> bool { true }

impl MapTerrainObject {
    pub fn private_folder(&self, map: &MapId) -> String {
        format!("maps/{map}/terrains/{}", self.id.as_str())
    }
    /// 仅投影地形后端需要的视图，所属MapId不变；地图容器的相机、角色和引用不复制。
    pub fn source_manifest(&self, owner: &crate::MapManifestAsset) -> crate::MapManifestAsset {
        let mut source = owner.clone();
        source.display_name.clone_from(&self.display_name);
        source.world_min = self.world_min;
        source.world_max = self.world_max;
        source.terrain = self.terrain.clone();
        // 拼接可用曲线由完整来源图在后台解析；投影只继承快照，不在主帧重扫所有同级网格。
        // 世界配置按最小XZ缩放换算成保守源范围；非等比缩放也不会欠加载角色需要的地面。
        if source.terrain.kind == crate::MapTerrainKind::SparseClipmap {
            let meters = self.transform.scale[0].min(self.transform.scale[2]);
            for lod in &mut source.terrain.collision_lods { lod.max_distance_meters /= meters; }
            if let Some(lods) = &mut source.terrain.display_lods {
                let boundary = (self.world_max[0] - self.world_min[0]).max(self.world_max[1] - self.world_min[1]) * 0.5;
                let mut terminal = None;
                for (index, lod) in lods.iter_mut().enumerate() {
                    lod.outer_half_extent_meters = lod.outer_half_extent_meters.map(|v| v / meters);
                    if lod.outer_half_extent_meters.is_none_or(|extent| extent >= boundary) {
                        lod.outer_half_extent_meters = None; terminal = Some(index + 1); break;
                    }
                }
                // 缩小对象时较近LOD可能已覆盖全域，只裁剪运行计划，不改作者配置或拒绝合法缩放。
                if let Some(end) = terminal { lods.truncate(end); }
            }
            source.terrain.player_collision_preload_radius_meters /= meters;
            for value in [&mut source.terrain.role_collision_preload_radius_meters,
                &mut source.terrain.player_collision_span_meters, &mut source.terrain.role_collision_span_meters] {
                *value = value.map(|v| v / meters);
            }
        }
        source.terrains = None;
        source.vegetation_paint_groups = Default::default(); // 绘制组属于容器，不能复制到单网格投影。
        source.puddles.clone_from(&self.puddles);
        source.sub_maps = Default::default();
        source
    }
}

/// 独立体素的作者合同；缺席密度块从有限实心盒体求值，空地图创建不依赖高度源。
#[derive(Asset, TypePath, Clone, Debug, Serialize, Deserialize)]
pub struct MapVoxelTerrainSource {
    pub schema_version: u32,
    pub map_id: MapId,
    pub terrain_id: TerrainId,
    pub cell_size_meters: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
    #[serde(default)]
    pub blocks: std::sync::Arc<[MapVoxelDensityBlockRef]>,
    /// 对象私有的不可变v3字段载体；几何LOD换窗不改写表面、积水或种植根。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_fields_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MapVoxelDensityBlockRef {
    pub key: [i32; 3],
    pub sha256: String,
}

impl MapVoxelTerrainSource {
    pub fn validate(&self) -> Result<(), String> {
        MapId::parse(self.map_id.as_str())?;
        TerrainId::parse(self.terrain_id.as_str())?;
        if self.schema_version != 1 || !self.cell_size_meters.is_finite()
            || !(0.01..=0.5).contains(&self.cell_size_meters)
            || (0..3).any(|i| !self.min[i].is_finite() || !self.max[i].is_finite() || self.min[i] >= self.max[i]) {
            return Err("独立体素源的版本、格距或有限初始范围无效".into());
        }
        if self.min.into_iter().chain(self.max).any(|value| (value / self.cell_size_meters).abs() > 32_000_000.0) {
            return Err("体素源坐标超出32格整数块的安全范围".into());
        }
        if let Some(path) = &self.surface_fields_path {
            let folder = format!("maps/{}/terrains/{}/", self.map_id, self.terrain_id.as_str());
            if !path.starts_with(&folder) || path.contains('\\') || path.contains(':')
                || path.split('/').any(|part| part.is_empty() || matches!(part, "." | "..")) {
                return Err("体素三维字段必须属于当前地图的地形对象私有目录".into());
            }
        }
        let mut previous = None;
        for block in self.blocks.iter() {
            if block.key.iter().any(|v| v.abs_diff(0) > 1_000_000)
                || previous.is_some_and(|key| key >= block.key)
                || block.sha256.len() != 64 || !block.sha256.bytes().all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v)) {
                return Err("独立体素密度块身份、顺序或摘要无效".into());
            }
            previous = Some(block.key);
        }
        Ok(())
    }
    pub fn block_path(&self, hash: &str) -> String {
        format!("maps/{}/terrains/{}/density/{hash}.v16block", self.map_id, self.terrain_id.as_str())
    }
}
