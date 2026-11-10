//! 地表作者四槽源的有界异步加载与技能浅坑的不可变 CPU 采样快照。

use std::io::{Error, ErrorKind};
use std::ops::Deref;
use std::sync::Arc;

use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use bevy::ecs::error::BevyError;
use bevy::prelude::*;
use sha2::{Digest, Sha256};

use crate::loaders::read_bounded;

const MAX_SURFACE_SOURCE_BYTES: usize = 64 * 1024 * 1024; // 坏文件不能无界增长；正式画布目前为512²。

/// 作者源由 AssetServer 的 IO 线程读取，战斗帧只克隆共享缓冲。
#[derive(Asset, TypePath, Clone, Debug)]
pub struct SceneSurfaceSourceAsset {
    bytes: Arc<[u8]>,
    content_sha256: [u8; 32], // IO加载时冻结真实内容身份；事件票据不能代替像素版本。
}

impl SceneSurfaceSourceAsset {
    /// 仅在既有IO/CPU任务或测试中调用；主帧只读取已冻结摘要，不扫描整图。
    pub fn from_bytes(bytes: Arc<[u8]>) -> Self {
        let content_sha256 = Sha256::digest(bytes.as_ref()).into();
        Self {
            bytes,
            content_sha256,
        }
    }

    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    } // 只能共享不可变缓冲，不能绕过摘要更新原位修改字节。
}

/// 本次原子保存的CPU交接期望值；定义与raw摘要均由已有保存worker冻结。
#[derive(Clone, Debug)]
pub struct SurfaceSourceSaveIdentity {
    definition: [u8; 32],
    sources: Option<[[u8; 32]; 2]>,
}

impl SurfaceSourceSaveIdentity {
    /// 定义单独改写时，raw仍沿用现有已验证文件；不能让旧定义事件解除新层结构的CPU门。
    pub fn for_definition(definition: &crate::SceneSurfaceAsset) -> Self {
        let mut hash = SurfaceSourceDefinitionHasher::new(definition);
        for layer in &definition.layers {
            hash.push_layer(layer);
        }
        Self {
            definition: hash.finish(),
            sources: None,
        }
    }

    /// 与本批真正写盘的四槽源计算相同摘要；输入借用作者Arc，不额外复制画布。
    pub fn for_author_buffers(
        definition: &crate::SceneSurfaceAsset,
        indices: &[u8],
        weights: &[u8],
    ) -> Self {
        let mut identity = Self::for_definition(definition);
        identity.sources = Some([
            Sha256::digest(indices).into(),
            Sha256::digest(weights).into(),
        ]);
        identity
    }

    /// 只在CPU候选首次建立/失效后比较；旧journal即使再次重放Modified也不具备新内容证据。
    pub fn matches(
        &self,
        definition: [u8; 32],
        indices: &SceneSurfaceSourceAsset,
        weights: &SceneSurfaceSourceAsset,
    ) -> bool {
        self.sources
            .is_none_or(|sources| sources == [indices.content_sha256, weights.content_sha256])
            && self.definition == definition
    }
}

/// 正式CPU已有的系数冻结循环顺带累积小型定义身份，不为校验另扫一次层表或任何像素。
pub struct SurfaceSourceDefinitionHasher(Sha256);

impl SurfaceSourceDefinitionHasher {
    pub fn new(definition: &crate::SceneSurfaceAsset) -> Self {
        let mut state = Self(Sha256::new());
        state.0.update(definition.schema_version.to_le_bytes());
        state.string(definition.scene_id.as_str());
        for dimension in definition.resolution {
            state.0.update(dimension.to_le_bytes());
        }
        for path in [
            &definition.index_source_path,
            &definition.weight_source_path,
            &definition.index_texture_path,
            &definition.weight_texture_path,
        ] {
            state.string(path);
        }
        state
            .0
            .update((definition.layers.len() as u64).to_le_bytes());
        state
    }

    fn string(&mut self, value: &str) {
        self.0.update((value.len() as u64).to_le_bytes());
        self.0.update(value.as_bytes()); // 长度分隔，字段串接不能产生歧义。
    }

    pub fn push_layer(&mut self, layer: &crate::SceneSurfaceLayer) {
        self.string(&layer.id);
        self.string(&layer.label);
        self.string(&layer.preset_id);
    }

    pub fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

#[derive(Default, TypePath)]
pub struct SceneSurfaceSourceLoader;

impl AssetLoader for SceneSurfaceSourceLoader {
    type Asset = SceneSurfaceSourceAsset;
    type Settings = ();
    type Error = BevyError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let bytes = read_bounded(reader, MAX_SURFACE_SOURCE_BYTES, "地表四槽作者源").await?;
        if bytes.is_empty() {
            return Err(Error::new(ErrorKind::InvalidData, "地表作者源为空").into());
        }
        Ok(SceneSurfaceSourceAsset::from_bytes(bytes.into()))
    }

    fn extensions(&self) -> &[&str] {
        &["rgba8", "rgba32"] // 原始索引/权重仅用于CPU形变，不替换正式KTX2渲染。
    }
}

/// 文件和作者缓冲都只共享引用，候选发布不得新增完整像素副本。
#[derive(Clone, Debug)]
enum SurfaceCraterBytes {
    File(Arc<[u8]>),
    Author(Arc<Vec<u8>>), // 保留作者写时复制缓冲；发布帧不得为CPU玩法重新复制整张画布。
}

impl Deref for SurfaceCraterBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::File(bytes) => bytes,
            Self::Author(bytes) => bytes,
        }
    }
}

/// 笔印持有施法时的地表及预设系数快照，后来重涂不会追溯改变旧坑。
#[derive(Clone, Debug)]
pub struct SurfaceCraterResponse {
    bounds_min: Vec2,
    bounds_max: Vec2,
    resolution: [u32; 2],
    indices: SurfaceCraterBytes,
    weights: SurfaceCraterBytes,
    coefficients: Vec<f32>,
}

impl SurfaceCraterResponse {
    pub fn new(
        bounds_min: Vec2,
        bounds_max: Vec2,
        resolution: [u32; 2],
        indices: Arc<[u8]>,
        weights: Arc<[u8]>,
        coefficients: Vec<f32>,
    ) -> Result<Self, String> {
        Self::from_buffers(
            bounds_min,
            bounds_max,
            resolution,
            SurfaceCraterBytes::File(indices),
            SurfaceCraterBytes::File(weights),
            coefficients,
        )
    }

    /// 编辑器在原有single-flight预览任务内验证源，CPU响应与完整GPU材质共用同修订Arc。
    pub fn from_author_buffers(
        bounds_min: Vec2,
        bounds_max: Vec2,
        resolution: [u32; 2],
        indices: Arc<Vec<u8>>,
        weights: Arc<Vec<u8>>,
        coefficients: Vec<f32>,
    ) -> Result<Self, String> {
        Self::from_buffers(
            bounds_min,
            bounds_max,
            resolution,
            SurfaceCraterBytes::Author(indices),
            SurfaceCraterBytes::Author(weights),
            coefficients,
        )
    }

    fn from_buffers(
        bounds_min: Vec2,
        bounds_max: Vec2,
        resolution: [u32; 2],
        indices: SurfaceCraterBytes,
        weights: SurfaceCraterBytes,
        coefficients: Vec<f32>,
    ) -> Result<Self, String> {
        let pixels = usize::try_from(resolution[0])
            .ok()
            .and_then(|width| usize::try_from(resolution[1]).ok()?.checked_mul(width))
            .filter(|pixels| *pixels > 0)
            .ok_or("地表形变画布尺寸无效")?;
        if indices.len() != pixels.checked_mul(16).ok_or("地表索引尺寸溢出")?
            || weights.len() != pixels.checked_mul(4).ok_or("地表权重尺寸溢出")?
            || !bounds_min.is_finite()
            || !bounds_max.is_finite()
            || bounds_max.cmple(bounds_min).any()
            || coefficients
                .iter()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err("地表形变源长度、边界或预设系数无效".into());
        }
        Ok(Self {
            bounds_min,
            bounds_max,
            resolution,
            indices,
            weights,
            coefficients,
        })
    }

    /// 与地表 Shader 同样读取四邻像素的四槽，按非零贡献混合玩法系数。
    pub fn sample(&self, position: Vec2) -> f32 {
        let uv = (position - self.bounds_min) / (self.bounds_max - self.bounds_min);
        if !uv.is_finite() || (uv.cmplt(Vec2::ZERO) | uv.cmpgt(Vec2::ONE)).any() {
            return 1.0; // 越界或裸地保持旧技能深度，不复制边缘材质。
        }
        let pixel =
            uv * Vec2::new(self.resolution[0] as f32, self.resolution[1] as f32) - Vec2::splat(0.5);
        let base = pixel.floor();
        let fraction = pixel - base;
        let mut candidates = [(u32::MAX, 0.0_f32); 16]; // 四邻像素最多产生十六个局部层身份。
        let mut candidate_count = 0;
        for offset_y in 0..2 {
            for offset_x in 0..2 {
                let x = (base.x as i32 + offset_x).clamp(0, self.resolution[0] as i32 - 1) as usize;
                let y = (base.y as i32 + offset_y).clamp(0, self.resolution[1] as i32 - 1) as usize;
                let bilinear = if offset_x == 0 {
                    1.0 - fraction.x
                } else {
                    fraction.x
                } * if offset_y == 0 {
                    1.0 - fraction.y
                } else {
                    fraction.y
                };
                let pixel_index = y * self.resolution[0] as usize + x;
                for slot in 0..4 {
                    let weight = self.weights[pixel_index * 4 + slot] as f32 * bilinear;
                    if weight == 0.0 {
                        continue;
                    }
                    let start = pixel_index * 16 + slot * 4;
                    let index = u32::from_le_bytes(
                        self.indices[start..start + 4]
                            .try_into()
                            .expect("四槽长度已校验"),
                    );
                    if let Some((_, contribution)) = candidates[..candidate_count]
                        .iter_mut()
                        .find(|(candidate, _)| *candidate == index)
                    {
                        *contribution += weight;
                    } else {
                        candidates[candidate_count] = (index, weight);
                        candidate_count += 1;
                    }
                }
            }
        }
        let mut weighted = 0.0;
        let mut total = 0.0;
        for _ in 0..4 {
            let Some(best) = (0..candidate_count)
                .filter(|index| candidates[*index].1 > 0.0)
                .max_by(|left, right| {
                    candidates[*left]
                        .1
                        .total_cmp(&candidates[*right].1)
                        .then_with(|| candidates[*right].0.cmp(&candidates[*left].0))
                })
            else {
                break;
            };
            let (index, contribution) = candidates[best];
            weighted += contribution
                * self
                    .coefficients
                    .get(index as usize)
                    .copied()
                    .unwrap_or(1.0);
            total += contribution;
            candidates[best].1 = 0.0;
        } // Shader同样只归一化贡献最大的四个局部层。
        if total > 0.0 { weighted / total } else { 1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_saved_identity_requires_exact_definition_and_both_actual_raw_contents() {
        let definition = crate::SceneSurfaceAsset {
            schema_version: crate::SCENE_SURFACE_SCHEMA_VERSION,
            scene_id: crate::SceneId::parse("MAP-surface-content-proof").unwrap(),
            resolution: [1; 2],
            layers: vec![crate::SceneSurfaceLayer {
                id: "hard".into(),
                label: "硬地".into(),
                preset_id: "rock".into(),
            }],
            index_source_path: "scenes/proof/indices.rgba32".into(),
            weight_source_path: "scenes/proof/weights.rgba8".into(),
            index_texture_path: "scenes/proof/indices.ktx2".into(),
            weight_texture_path: "scenes/proof/weights.ktx2".into(),
        };
        let expected =
            SurfaceSourceSaveIdentity::for_author_buffers(&definition, &[0; 16], &[255, 0, 0, 0]);
        let new_indices = SceneSurfaceSourceAsset::from_bytes(Arc::from([0_u8; 16]));
        let old_indices = SceneSurfaceSourceAsset::from_bytes(Arc::from([1_u8; 16]));
        let new_weights = SceneSurfaceSourceAsset::from_bytes(Arc::from([255_u8, 0, 0, 0]));
        let old_weights = SceneSurfaceSourceAsset::from_bytes(Arc::from([0_u8; 4]));
        let hash = SurfaceSourceSaveIdentity::for_definition(&definition).definition;
        assert!(!expected.matches(hash, &old_indices, &old_weights));
        assert!(!expected.matches(hash, &new_indices, &old_weights));
        assert!(!expected.matches(hash, &old_indices, &new_weights));
        assert!(expected.matches(hash, &new_indices, &new_weights));
        let mut other = definition.clone();
        other.layers[0].preset_id = "soil".into(); // 数量/像素相同但局部预设绑定已变化，同样不能提前释放旧CPU系数。
        let other_hash = SurfaceSourceSaveIdentity::for_definition(&other).definition;
        assert!(!expected.matches(other_hash, &new_indices, &new_weights));
        let definition_only = SurfaceSourceSaveIdentity::for_definition(&other);
        assert!(!definition_only.matches(hash, &new_indices, &new_weights));
        assert!(definition_only.matches(other_hash, &new_indices, &new_weights));
    }

    #[test]
    fn author_response_retains_shared_buffers_and_frozen_preset_without_pixel_copy() {
        let indices = Arc::new(vec![0_u8; 16]);
        let weights = Arc::new(vec![255_u8, 0, 0, 0]);
        let snapshot = SurfaceCraterResponse::from_author_buffers(
            Vec2::ZERO,
            Vec2::ONE,
            [1, 1],
            indices.clone(),
            weights.clone(),
            vec![0.1],
        )
        .unwrap();
        assert_eq!(snapshot.indices.as_ptr(), indices.as_ptr());
        assert_eq!(snapshot.weights.as_ptr(), weights.as_ptr());
        assert!((snapshot.sample(Vec2::splat(0.5)) - 0.1).abs() < 0.001);
        assert_eq!(Arc::strong_count(&indices), 2);
        assert_eq!(Arc::strong_count(&weights), 2);
    }

    #[test]
    fn mixed_surface_transitions_and_rejects_bad_sources() {
        let indices: Arc<[u8]> = [0_u32, 1, 0, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .cycle()
            .take(2 * 2 * 16)
            .collect::<Vec<_>>()
            .into();
        let weights: Arc<[u8]> = [255, 0, 0, 0, 0, 255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0]
            .to_vec()
            .into();
        let snapshot = SurfaceCraterResponse::new(
            Vec2::ZERO,
            Vec2::splat(2.0),
            [2, 2],
            indices.clone(),
            weights.clone(),
            vec![1.0, 0.1],
        )
        .unwrap();
        assert!((snapshot.sample(Vec2::new(0.5, 0.5)) - 1.0).abs() < 0.001);
        assert!((snapshot.sample(Vec2::new(1.5, 0.5)) - 0.1).abs() < 0.001);
        assert!((snapshot.sample(Vec2::new(1.0, 0.5)) - 0.55).abs() < 0.001);
        assert!(
            SurfaceCraterResponse::new(
                Vec2::ZERO,
                Vec2::ONE,
                [2, 2],
                indices,
                Arc::from([1_u8]),
                vec![1.0]
            )
            .is_err()
        );
    }

    #[test]
    fn response_uses_only_four_strongest_visual_layers() {
        let indices: Arc<[u8]> = (0_u32..16)
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>()
            .into();
        let weights: Arc<[u8]> = [255_u8; 16].to_vec().into();
        let response = SurfaceCraterResponse::new(
            Vec2::ZERO,
            Vec2::splat(2.0),
            [2, 2],
            indices,
            weights,
            (0..16)
                .map(|index| if index < 4 { 0.1 } else { 1.0 })
                .collect(),
        )
        .unwrap();
        assert!((response.sample(Vec2::ONE) - 0.1).abs() < 0.001); // 同权重时与Shader一样取低ID的四层。
    }
}
