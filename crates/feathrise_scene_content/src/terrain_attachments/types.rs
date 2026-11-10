//! 关联只保存稳定地形ID与源局部姿态，缓存世界坐标不参与关联身份。
use crate::{TerrainId, TerrainObjectTransform};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneTerrainBinding {
    pub terrain_id: TerrainId,
    pub local_pose: TerrainObjectTransform,
    /// 跟随高度的摆放物在源局部保存离地量，缩放承托对象时世界离地量一并缩放。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_height_offset: Option<f32>,
}
impl SceneTerrainBinding {
    pub fn validate(&self) -> Result<(), String> {
        TerrainId::parse(self.terrain_id.as_str())?;
        if self
            .local_height_offset
            .is_some_and(|offset| !offset.is_finite())
        {
            return Err("关联离地偏移必须有限".into());
        }
        self.local_pose.validate()
    }
    /// 世界位置完整逆变换，朝向和各轴尺寸按作者TRS语义保存；整体缩放不重采样内容。
    pub fn capture(
        id: TerrainId,
        terrain: TerrainObjectTransform,
        world: TerrainObjectTransform,
    ) -> Result<Self, String> {
        terrain.validate()?;
        world.validate()?;
        let mut local_pose = TerrainObjectTransform {
            translation: terrain
                .as_transform()
                .to_matrix()
                .inverse()
                .transform_point3(Vec3::from_array(world.translation))
                .to_array(),
            yaw_degrees: world.yaw_degrees - terrain.yaw_degrees,
            scale: (Vec3::from_array(world.scale) / Vec3::from_array(terrain.scale)).to_array(),
            pitch_degrees: world.pitch_degrees,
            roll_degrees: world.roll_degrees,
        };
        if terrain.pitch_degrees != 0.0 || terrain.roll_degrees != 0.0 {
            // 关联沿作者TRS语义分别保存位置、完整旋转和各轴尺寸，不对欧拉角逐轴相减。
            local_pose = local_pose.with_rotation(terrain.rotation().inverse() * world.rotation());
        }
        let result = Self {
            terrain_id: id,
            local_pose,
            local_height_offset: None,
        };
        result.validate()?;
        Ok(result)
    }
    pub fn resolve(
        &self,
        terrain: TerrainObjectTransform,
    ) -> Result<TerrainObjectTransform, String> {
        self.validate()?;
        terrain.validate()?;
        let mut pose = TerrainObjectTransform {
            translation: terrain
                .as_transform()
                .to_matrix()
                .transform_point3(Vec3::from_array(self.local_pose.translation))
                .to_array(),
            yaw_degrees: terrain.yaw_degrees + self.local_pose.yaw_degrees,
            scale: (Vec3::from_array(terrain.scale) * Vec3::from_array(self.local_pose.scale))
                .to_array(),
            pitch_degrees: self.local_pose.pitch_degrees,
            roll_degrees: self.local_pose.roll_degrees,
        };
        if terrain.pitch_degrees != 0.0 || terrain.roll_degrees != 0.0 {
            pose = pose.with_rotation(terrain.rotation() * self.local_pose.rotation());
        }
        pose.validate()?;
        Ok(pose)
    }
}
