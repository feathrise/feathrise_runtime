//! 子场景作者引用与实例路径：同源多个实例不能借用源 SceneId 作为运行身份。

use bevy::prelude::{EulerRot, Quat, Transform, Vec3};
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::SceneId;

/// 只在所属源场景中唯一；复制引用必须分配新身份，移动与撤销保持原身份。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SceneInstanceId(pub String);

impl SceneInstanceId {
    pub fn parse(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        SceneId::parse(value.clone()).map_err(|_| "子场景实例ID必须为1至64个ASCII字母、数字、短横线或下划线".to_owned())?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SceneInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 引用和网格展开共用完整刚体旋转；网格各轴缩放仍由独立对象保存。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneInstanceTransform {
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default)]
    pub yaw_degrees: f32,
    #[serde(default, skip_serializing_if = "zero_angle")]
    pub pitch_degrees: f32,
    #[serde(default, skip_serializing_if = "zero_angle")]
    pub roll_degrees: f32,
}
fn zero_angle(value: &f32) -> bool { *value == 0.0 }

impl SceneInstanceTransform {
    pub fn validate(&self) -> Result<(), String> {
        if !Vec3::from_array(self.translation).is_finite() || !self.yaw_degrees.is_finite()
            || !self.pitch_degrees.is_finite() || !self.roll_degrees.is_finite() {
            return Err("子场景的位置和朝向必须是有限数值".to_owned());
        }
        Ok(())
    }

    pub fn as_transform(&self) -> Transform {
        Transform::from_translation(Vec3::from_array(self.translation))
            .with_rotation(Quat::from_euler(EulerRot::YXZ,
                self.yaw_degrees.rem_euclid(360.0).to_radians(),
                self.pitch_degrees.rem_euclid(360.0).to_radians(),
                self.roll_degrees.rem_euclid(360.0).to_radians()))
    }

    /// 每一级先按父方向旋转局部位移；不能只相加平移而把孙场景留在旧方向。
    pub fn compose(&self, child: &Self) -> Self {
        let parent = self.as_transform();
        let mut result = Self {
            translation: parent.transform_point(Vec3::from_array(child.translation)).to_array(),
            yaw_degrees: (self.yaw_degrees + child.yaw_degrees).rem_euclid(360.0),
            pitch_degrees: child.pitch_degrees,
            roll_degrees: child.roll_degrees,
        };
        // 旧Yaw链保持原来的精确相加；父姿态倾斜时必须组合旋转，不能逐轴相加。
        if self.pitch_degrees == 0.0 && self.roll_degrees == 0.0 {
            return result;
        }
        let (yaw, pitch, roll) = (parent.rotation * child.as_transform().rotation).to_euler(EulerRot::YXZ);
        result.yaw_degrees = yaw.to_degrees().rem_euclid(360.0);
        result.pitch_degrees = pitch.to_degrees().rem_euclid(360.0);
        result.roll_degrees = roll.to_degrees().rem_euclid(360.0);
        result
    }

    pub fn to_world(&self, local: Vec3) -> Vec3 {
        self.as_transform().transform_point(local)
    }

    pub fn to_local(&self, world: Vec3) -> Vec3 {
        let transform = self.as_transform();
        transform.rotation.inverse() * (world - transform.translation)
    }
}

/// 父图只保存来源、身份和整体变换；内部成员及其作者文件不复制到父图。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneInstancePlacement {
    pub id: SceneInstanceId,
    pub source_scene_id: SceneId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default)]
    pub transform: SceneInstanceTransform,
}

/// 根场景与逐级引用身份组成真正的运行地址；源场景名称变化不会改变它。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SceneInstancePath {
    pub root_scene_id: SceneId,
    pub instances: Vec<SceneInstanceId>,
    /// 地形地址独立于子场景引用ID，二者同名也不会共享运行状态。
    pub terrain_id: Option<crate::TerrainId>,
}

impl SceneInstancePath {
    pub fn root(scene_id: SceneId) -> Self {
        Self { root_scene_id: scene_id, instances: Vec::new(), terrain_id: None }
    }

    pub fn child(&self, id: SceneInstanceId) -> Self {
        let mut child = self.clone();
        child.instances.push(id);
        child
    }

    pub fn is_root(&self) -> bool {
        self.instances.is_empty() && self.terrain_id.is_none()
    }

    pub fn terrain(&self, id: crate::TerrainId) -> Self {
        let mut path = self.clone();
        path.terrain_id = Some(id);
        path
    }

    pub fn is_descendant_of(&self, parent: &Self) -> bool {
        self.root_scene_id == parent.root_scene_id && self.instances.starts_with(&parent.instances)
    }
}

impl fmt::Display for SceneInstancePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.root_scene_id)?;
        for id in &self.instances {
            write!(f, "/{id}")?;
        }
        if let Some(id) = &self.terrain_id { write!(f, "/terrain:{}", id.as_str())?; }
        Ok(())
    }
}
