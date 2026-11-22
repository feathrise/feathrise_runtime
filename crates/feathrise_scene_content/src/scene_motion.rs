//! 主场景 GLB 动画和旧版本场景运动的只读兼容加载。

use bevy::asset::{AssetPath, LoadContext, ReadAssetBytesError, io::AssetReaderError};
use bevy::gltf::{Gltf, GltfLoaderSettings, extensions::{
    ErasedGltfExtensionHandler, GltfExtensionHandler, GltfExtensionHandlers,
}, gltf};
use bevy::prelude::*;
use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};

mod gltf_tracks;

const FILE_NAME: &str = "motion.scene_motion.json";
const ASSET_LABEL: &str = "SceneMotion";

type TrackTable = Arc<HashMap<String, Arc<SceneMotionTrack>>>;

/// 已冻结的只读对象动画；同一轨道供候选、实例和物理采样共享。
#[derive(Debug)]
pub struct SceneMotionTrack {
    curves: Curves,
    collider: Option<SceneMotionCollider>,
}

#[derive(Debug)]
enum Curves {
    Gltf(gltf_tracks::ObjectCurves),
    Legacy(LegacyMotion),
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct LegacyMotion {
    #[serde(deserialize_with = "vec3")]
    end_offset: Vec3,
    travel_seconds: f32,
    hold_seconds: f32,
    collider: SceneMotionCollider,
}

/// 碰撞盒尺寸使用摆放变换已缩放的局部米。
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub struct SceneMotionCollider {
    #[serde(deserialize_with = "vec3")]
    pub center: Vec3,
    #[serde(deserialize_with = "vec3")]
    pub half_extents: Vec3,
}

fn vec3<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec3, D::Error> {
    <[f32; 3]>::deserialize(deserializer).map(Vec3::from_array)
}

// 保留 SDK 之前对旧运动记录的只读反序列化。场景 GLB 的轨道仅从标准通道生成。
impl<'de> Deserialize<'de> for SceneMotionTrack {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let track = LegacyMotion::deserialize(deserializer)?;
        Ok(Self { curves: Curves::Legacy(track), collider: Some(track.collider) })
    }
}

impl SceneMotionTrack {
    pub fn collider_for(&self, record: &crate::ScenePlacementRecord) -> Option<SceneMotionCollider> {
        use crate::ScenePlacementAssetTarget::*;
        if matches!(self.curves, Curves::Legacy(_)) { return self.collider; }
        match &record.asset_target {
            Cuboid { size_mm, collision: true, .. } => self.collider.or_else(|| Some(SceneMotionCollider {
                center: Vec3::ZERO,
                half_extents: Vec3::from_array(size_mm.map(|size| size as f32 * 0.0005))
                    * Vec3::from_array(record.scale).abs(),
            })),
            EditableCuboid { collision: true, .. } | EditableMesh { collision: true, .. }
            | Primitive { collision: true, .. } | Scene { collision: true } => self.collider,
            _ => None,
        }
    }

    pub fn compatible_origin(&self, origin: Vec3) -> bool {
        match &self.curves {
            Curves::Gltf(_) => true,
            Curves::Legacy(track) => (origin + track.end_offset).is_finite(),
        }
    }

    /// 各通道独立覆盖摆放的局部变换。物理在同一个采样时刻采用位置和朝向。
    pub fn sample(&self, seconds: f64, origin: Transform) -> (Vec3, Quat) {
        let sample = self.sample_transform(seconds, origin);
        (sample.translation, sample.rotation)
    }

    /// 在同一时刻采样三个标准变换属性。没有关键帧的属性继续使用摆放初始值。
    pub fn sample_transform(&self, seconds: f64, origin: Transform) -> Transform {
        match &self.curves {
            Curves::Gltf(track) => track.sample(seconds, origin),
            Curves::Legacy(track) => Transform {
                translation: origin.translation + track.offset_at(seconds), ..origin
            },
        }
    }

    /// 将推进后的非折返时间限制在一个播放周期内，返回是否跨过循环接缝。
    pub fn advance(&self, elapsed: f64, delta: f64) -> (f64, bool) {
        match &self.curves {
            Curves::Gltf(track) => track.advance(elapsed, delta),
            Curves::Legacy(track) => ((elapsed + delta).rem_euclid(track.cycle_seconds()), false),
        }
    }
}

impl LegacyMotion {
    fn cycle_seconds(self) -> f64 {
        2.0 * (self.travel_seconds as f64 + self.hold_seconds as f64)
    }

    fn offset_at(self, elapsed: f64) -> Vec3 {
        let hold = self.hold_seconds as f64;
        let travel = self.travel_seconds as f64;
        let cycle = self.cycle_seconds();
        let phase = elapsed.rem_euclid(cycle);
        let progress = ((phase - hold) / travel).min((cycle - phase) / travel);
        self.end_offset * progress.clamp(0.0, 1.0) as f32
    }
}

#[derive(Deserialize)]
struct Document {
    schema_version: u32,
    scene_id: crate::SceneId,
    tracks: Vec<TrackEntry>,
}

#[derive(Deserialize)]
struct TrackEntry {
    placement_id: String,
    #[serde(flatten)]
    track: LegacyMotion,
}

/// 不在帧内解析文件或重建索引。失败结果独立于地形等原清单数据。
#[derive(Asset, TypePath, Debug)]
pub struct SceneMotionAsset {
    pub tracks: Result<TrackTable, String>,
    /// 主 GLB 已提供场景动画。手动或删光关键帧后的空表仍覆盖旧运动旁档。
    pub authoritative: bool,
    // 同一文件另有摆放元数据读取器。保留标准 GLB 根句柄，文件热更新才会同时重走两种类型。
    #[dependency]
    _standard_source: Option<Handle<Gltf>>,
}

/// 常驻工程的摆放可由内存 JSON 覆盖，动画仍使用这个场景原来登记的主 GLB。
#[derive(Resource, PartialEq, Eq)]
pub struct SceneMotionPreviewSource {
    pub scene_id: crate::SceneId,
    pub path: AssetPath<'static>,
}

impl SceneMotionPreviewSource {
    pub fn from_glb(scene_id: crate::SceneId, path: &str) -> Option<Self> {
        let path = SceneMotionAsset::glb_path(&AssetPath::try_parse(path).ok()?)?;
        Some(Self { scene_id, path })
    }
}

impl SceneMotionAsset {
    pub fn path(manifest: &Handle<crate::SceneManifestAsset>) -> Option<AssetPath<'static>> {
        Some(manifest.path()?.clone_owned().with_label(ASSET_LABEL))
    }

    pub fn placement_path(placements: &Handle<crate::ScenePlacementsAsset>) -> Option<AssetPath<'static>> {
        Self::glb_path(placements.path()?)
    }

    fn glb_path(path: &AssetPath<'_>) -> Option<AssetPath<'static>> {
        (path.path().extension().and_then(|ext| ext.to_str()) == Some("glb"))
            .then(|| path.clone_owned().with_label(ASSET_LABEL))
    }
}

fn parse(bytes: &[u8], expected_scene: &crate::SceneId) -> Result<HashMap<String, Arc<SceneMotionTrack>>, String> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let document: Document = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if document.schema_version != 1 || document.scene_id != *expected_scene {
        return Err(format!("运动轨道版本或场景 ID 不匹配（场景 {expected_scene}）"));
    }
    let mut tracks = HashMap::with_capacity(document.tracks.len());
    for TrackEntry { placement_id, track } in document.tracks {
        if placement_id.trim().is_empty()
            || !track.end_offset.is_finite()
            || !track.travel_seconds.is_finite() || track.travel_seconds <= 0.0
            || !track.hold_seconds.is_finite() || track.hold_seconds < 0.0
            || !track.collider.center.is_finite()
            || !(track.collider.half_extents * 2.0).is_finite()
            || track.collider.half_extents.min_element() <= 0.0
        {
            return Err(format!("对象 {placement_id} 的运动或碰撞盒参数无效"));
        }
        let track = Arc::new(SceneMotionTrack { curves: Curves::Legacy(track), collider: Some(track.collider) });
        if tracks.insert(placement_id.clone(), track).is_some() {
            return Err(format!("对象 {placement_id} 有多条运动轨道"));
        }
    }
    Ok(tracks)
}

/// 动画使用标准 GLB 载入器已解码的文档和缓冲，不为动画另读整份场景几何。
pub(crate) fn install_gltf_motion(app: &mut App) {
    app.init_resource::<GltfExtensionHandlers>();
    app.world().resource::<GltfExtensionHandlers>().0.try_write()
        .expect("GLB 载入器启动前登记场景动画")
        .push(Box::new(SceneGltfAnimation));
}

struct SceneGltfAnimation;
impl GltfExtensionHandler for SceneGltfAnimation {
    fn dyn_clone(&self) -> Box<dyn ErasedGltfExtensionHandler> {
        Box::new(Self)
    }

    fn on_root(&mut self, context: &mut LoadContext<'_>, file: &gltf::Gltf, _: &GltfLoaderSettings) {
        if context.path().path().file_name().and_then(|name| name.to_str())
            != Some(crate::SCENE_PLACEMENTS_GLB_FILE_NAME)
        {
            return;
        }
        let (tracks, authoritative) = match gltf_tracks::read(file) {
            Ok((tracks, authoritative)) => (Ok(Arc::new(tracks)), authoritative),
            Err(error) => (Err(error), true),
        };
        let source = context.path().clone_owned();
        let result: Result<_, std::convert::Infallible> = context.labeled_asset_scope(ASSET_LABEL, |child| {
            // 父 GLB 已由同一次载入在途。子资产持有其句柄，父资产不回持有本标签。
            let source = child.load::<Gltf>(source);
            Ok(SceneMotionAsset { tracks, authoritative, _standard_source: Some(source) })
        });
        let _ = result.unwrap();
    }
}

/// 通过原资产源定位兄弟文件，兼容项目自定义资产根和资产包。
pub(crate) async fn load_with_manifest(context: &mut LoadContext<'_>, scene: &crate::SceneId) {
    let tracks = match context.path().resolve_embed_str(FILE_NAME) {
        Ok(path) => match context.read_asset_bytes(path).await {
            Ok(bytes) => parse(&bytes, scene),
            Err(ReadAssetBytesError::AssetReaderError(AssetReaderError::NotFound(_))) => Ok(HashMap::new()),
            Err(error) => Err(error.to_string()),
        },
        Err(error) => Err(error.to_string()),
    };
    context.add_labeled_asset(ASSET_LABEL, SceneMotionAsset {
        tracks: tracks.map(Arc::new), authoritative: false, _standard_source: None,
    });
}
