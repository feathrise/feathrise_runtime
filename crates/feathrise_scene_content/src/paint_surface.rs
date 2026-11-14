//! 模型与地形共用的种植目标和模型表面稀疏密度；不持久化 Entity、显示 LOD 或物理碰撞。

use serde::{Deserialize, Serialize};

use crate::TerrainId;

/// 目标身份属于源场景；运行时另附完整实例路径，不能用同一个 ID 借用兄弟实例的姿态。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SurfaceTarget {
    Terrain { terrain_id: TerrainId },
    Model { object_id: String },
}

impl SurfaceTarget {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Terrain { terrain_id } => TerrainId::parse(terrain_id.as_str()).map(|_| ()),
            Self::Model { object_id } if !object_id.is_empty()
                && object_id.len() <= 256
                && !object_id.chars().any(char::is_control) => Ok(()),
            Self::Model { .. } => Err("模型表面需要有效的稳定对象 ID".into()),
        }
    }
}

/// primitive 为对象内部的稳定遍历序号，不使用重新打包后可能改变的 GLB 全局 mesh 索引。
/// 两个权重分别对应三角面的第二、第三个顶点，首顶点权重为 1-u-v。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelSurfaceAnchor {
    pub primitive: u32,
    pub triangle: u32,
    pub barycentric: [f32; 2],
}

impl ModelSurfaceAnchor {
    pub fn validate(&self) -> Result<(), String> {
        let [u, v] = self.barycentric;
        if u.is_finite() && v.is_finite() && u >= -1e-5 && v >= -1e-5 && u + v <= 1.00001 {
            Ok(())
        } else {
            Err("模型表面锚点的重心坐标无效".into())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelSurfaceVegetationSample {
    pub anchor: ModelSurfaceAnchor,
    /// 稳定作者植被层 ID 与归一化密度；组合仍由原图层编译规则解析为正式输出。
    pub layers: Vec<(String, u8)>,
}

/// 密度与对象局部表面绑定。位置变化保持锚点；连接关系变化则明确等待重新绑定。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelSurfaceVegetation {
    pub target: SurfaceTarget,
    pub topology: [u8; 32],
    pub spacing_meters: f32,
    /// 正式保存将样本拆成有界私有块；编辑和预览在后台读回 samples，不在输入帧做 IO。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_blocks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<ModelSurfaceVegetationSample>,
}

impl ModelSurfaceVegetation {
    pub fn empty(object_id: impl Into<String>, topology: [u8; 32], spacing_meters: f32) -> Self {
        Self { target: SurfaceTarget::Model { object_id: object_id.into() }, topology,
            spacing_meters, sample_blocks: Vec::new(), samples: Vec::new() }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.target.validate()?;
        if !matches!(self.target, SurfaceTarget::Model { .. }) {
            return Err("模型稀疏密度必须绑定模型目标".into());
        }
        if !self.spacing_meters.is_finite() || self.spacing_meters <= 0.0 {
            return Err("模型表面采样格距必须有限且大于零".into());
        }
        if self.sample_blocks.iter().any(|path| path.is_empty() || path.contains(['\\', ':'])
            || path.starts_with('/') || path.split('/').any(|part| part.is_empty() || part == "." || part == "..")) {
            return Err("模型植被样本块必须使用资产根内的安全相对路径".into());
        }
        let mut anchors = std::collections::BTreeSet::new();
        for sample in &self.samples {
            sample.anchor.validate()?;
            let anchor = sample.anchor;
            if !anchors.insert((anchor.primitive, anchor.triangle,
                anchor.barycentric[0].to_bits(), anchor.barycentric[1].to_bits())) {
                return Err("模型表面不能重复保存同一锚点".into());
            }
            let mut layers = std::collections::BTreeSet::new();
            if sample.layers.is_empty() || sample.layers.iter().any(|(id, density)| {
                id.is_empty() || id.len() > 256 || id.chars().any(char::is_control)
                    || *density == 0 || !layers.insert(id)
            }) {
                return Err("模型表面的植被图层或密度无效".into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_density_round_trips_and_rejects_duplicate_anchors() {
        let mut data = ModelSurfaceVegetation::empty("bridge", [7; 32], 0.25);
        data.samples.push(ModelSurfaceVegetationSample {
            anchor: ModelSurfaceAnchor { primitive: 0, triangle: 4, barycentric: [0.2, 0.3] },
            layers: vec![("grass".into(), 144)],
        });
        data.validate().unwrap();
        let read: ModelSurfaceVegetation = serde_json::from_slice(&serde_json::to_vec(&data).unwrap()).unwrap();
        assert_eq!(read, data);
        data.samples.push(data.samples[0].clone());
        assert!(data.validate().is_err());
    }
}
