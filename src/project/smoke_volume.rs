//! F12体积植被验收只消费正式采用回执，不能把隐藏二维模板计作三维显示。

#[path = "smoke_volume/types.rs"]
mod types;
#[cfg(test)]
#[path = "smoke_volume/tests.rs"]
mod tests;

pub(super) use types::Requirements;
use types::{SavedLayer, SavedTerrain};
use super::{context, ProjectContext};
use bevy::{ecs::system::SystemState, prelude::*};
use feathrise_vegetation::{VegetationBladeLayer, blade::{
    GpuBladeDensityMap, GpuBladePatch, GpuBladeSourceRevision, GpuBladeVolumeSource,
}};
use feathrise_scene::{DynamicClipmapGroundSurface, DynamicVegetationPhase,
    DynamicVegetationStatus, TerrainObjectResource, VolumeSurfaceFields};
use feathrise_scene_content::{SceneCatalogEntry, SceneId, SceneInstancePath,
    SceneManifestAsset, SceneVegetationAsset, SceneVegetationKind, TerrainId};
use serde_json::{Value, json};
use std::{path::Path, sync::Arc};

const MAX_TERRAINS: usize = 64;
const MAX_LAYERS: usize = 64;
const MAX_FIELDS: usize = 32;

/// 只在显式验收安装阶段读有限的保存目录；游戏帧不读盘、不解码三维字段。
pub(super) fn load(project: &ProjectContext, row: &SceneCatalogEntry) -> Result<Arc<Requirements>, String> {
    let mut expected = Requirements { scene: row.id.clone(), ..Default::default() };
    if std::env::var("FEATHRISE_GEOMETRY_UNIFICATION_VALIDATION").as_deref() != Ok("1") {
        return Ok(Arc::new(expected));
    }
    let manifest: SceneManifestAsset = read_json(project, &row.manifest_path())?;
    if manifest.id != row.id { return Err("三维验收清单不属于当前保存场景".into()); }
    let objects = manifest.terrains.as_deref().unwrap_or_default();
    if objects.len() > MAX_TERRAINS { return Err("三维验收地形目录超过64项".into()); }
    for object in objects.iter().filter(|object| object.enabled) {
        let (Some(voxel), Some(path)) = (&object.voxel, &object.vegetation_path) else { continue; };
        let document: SceneVegetationAsset = read_json(project, path)?;
        if document.scene_id != row.id || document.layers.len() > MAX_LAYERS {
            return Err("三维验收植被目录的场景身份或64层预算无效".into());
        }
        let layers = document.layers.iter().filter(|layer| layer.kind == SceneVegetationKind::Blade)
            .map(|layer| Ok(SavedLayer { id: layer.id.clone(), runtime_id: layer.runtime_layer_id
                .filter(|id| *id != 0).ok_or("三维验收保存层缺少正式运行层身份")? }))
            .collect::<Result<Vec<_>, String>>()?;
        let ids = layers.iter().map(|layer| &layer.id).collect::<std::collections::BTreeSet<_>>();
        let runtime_ids = layers.iter().map(|layer| layer.runtime_id).collect::<std::collections::BTreeSet<_>>();
        if ids.len() != layers.len() || runtime_ids.len() != layers.len() {
            return Err("三维验收保存层存在重复的作者或运行身份".into());
        }
        if !layers.is_empty() {
            expected.terrains.push(SavedTerrain { id: object.id.clone(), source_path: voxel.source_path.clone(), layers });
        }
    }
    Ok(Arc::new(expected))
}

fn read_json<T: serde::de::DeserializeOwned>(project: &ProjectContext, path: &str) -> Result<T, String> {
    use std::io::Read;
    let path = context::checked_existing(project.asset_root(), Path::new(path), false)?;
    let mut bytes = Vec::new();
    std::fs::File::open(&path).map_err(|error| error.to_string())?.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    if bytes.len() > 1024 * 1024 { return Err("三维验收单份目录超过1MiB".into()); }
    serde_json::from_slice(&bytes).map_err(|error| format!("三维验收保存目录读取失败：{error}"))
}

/// 每帧只检查至多64个目标、每目标32个共享根身份及缓存层回执，绝不遍历作者格。
pub(super) fn ready(world: &mut World, expected: &Requirements) -> bool {
    if expected.terrains.is_empty() { return true; }
    let mut access = SystemState::<(TerrainObjectResource<DynamicVegetationStatus>,
        TerrainObjectResource<DynamicClipmapGroundSurface>)>::new(world);
    let Ok((statuses, grounds)) = access.get(world) else { return false; };
    expected.terrains.iter().all(|target| {
        let Some((instance, status)) = statuses.for_saved_object(&expected.scene, &target.id) else { return false; };
        let Some((ground_instance, ground)) = grounds.for_saved_object(&expected.scene, &target.id) else { return false; };
        let ground = ground.snapshot.as_ref();
        instance == ground_instance && cached_ready(world, &expected.scene, instance, status,
            ground.filter(|ground| ground.ready).map(|ground| ground.content_revision_words()), target)
    })
}

fn cached_ready(world: &World, scene: &SceneId, instance: Entity, status: &DynamicVegetationStatus,
    ground: Option<[u64; 7]>, target: &SavedTerrain) -> bool {
    status.scene_id.as_ref() == Some(scene) && status.phase == DynamicVegetationPhase::Ready
        && status.volume_ready && status.volume_generation != 0
        && ground.is_some() && status.volume_ground_revision == ground
        && status.volume_root.is_some_and(|root| visible_descendant(world, root, instance))
        && !status.volume_fields.is_empty() && status.volume_fields.len() <= MAX_FIELDS
        && status.volume_fields.iter().all(|(entity, snapshot)| {
            visible_descendant(world, *entity, instance) && world.get::<VolumeSurfaceFields>(*entity)
                .is_some_and(|field| Arc::as_ptr(&field.0) as usize == *snapshot)
        })
        && target.layers.iter().all(|layer| {
            let total = status.volume_source_counts.get(&layer.id).copied().unwrap_or(0);
            let positive = status.volume_source_positive_counts.get(&layer.id).copied().unwrap_or(0);
            // 只有实际写入正密度的源才能证明当前侧仍有植被。
            status.volume_sources.contains_key(&layer.id) && positive > 0 && positive <= total
                && status.volume_window_counts.get(&layer.id).copied().unwrap_or(0) > 0
        })
}

/// 截图帧才在实际三维根内做一次有界窗口核对；来源、采用代、密度及对象姿态须同代。
pub(super) fn snapshot(world: &World, expected: &Requirements, terrain: &TerrainId,
    instance: Entity, status: &DynamicVegetationStatus, current_ground: Option<[u64; 7]>) -> Result<Value, String> {
    let Some(target) = expected.terrains.iter().find(|target| &target.id == terrain) else { return Ok(Value::Null); };
    let root = status.volume_root.ok_or("体积植被未发布正式三维根")?;
    let instance_path = SceneInstancePath::root(expected.scene.clone()).terrain(terrain.clone());
    let frame = world.get::<feathrise_scene_core::residency::SceneSourceFrame>(instance)
        .ok_or("体积植被活动实例缺少来源坐标")?;
    let fields_current = !status.volume_fields.is_empty() && status.volume_fields.len() <= MAX_FIELDS
        && status.volume_fields.iter().all(|(entity, snapshot)| visible_descendant(world, *entity, instance)
            && world.get::<VolumeSurfaceFields>(*entity).is_some_and(|field| Arc::as_ptr(&field.0) as usize == *snapshot));
    let mut stack = vec![root];
    let mut windows = Vec::new();
    let mut visited = 0;
    while let Some(entity) = stack.pop() {
        visited += 1;
        if visited > MAX_LAYERS * 8 + 1 { return Err("三维验收窗口子树超过有界预算".into()); }
        if let Some(children) = world.get::<Children>(entity) { stack.extend(children.iter()); }
        if let Some(source) = world.get::<GpuBladeVolumeSource>(entity) { windows.push((entity, source)); }
    }
    let layers = target.layers.iter().map(|layer| {
        let source = status.volume_sources.get(&layer.id);
        let expected_windows = status.volume_window_counts.get(&layer.id).copied().unwrap_or(0);
        let actual = windows.iter().filter(|(entity, _)| world.get::<VegetationBladeLayer>(*entity)
            .is_some_and(|binding| binding.0.0 == layer.runtime_id)).collect::<Vec<_>>();
        let mut lods = std::collections::BTreeSet::new();
        let bindings_current = actual.iter().all(|(entity, bound)| {
            let (Some(revision), Some(patch), Some(local), Some(global)) = (
                world.get::<GpuBladeSourceRevision>(*entity), world.get::<GpuBladePatch>(*entity),
                world.get::<Transform>(*entity), world.get::<GlobalTransform>(*entity)) else { return false; };
            lods.insert(patch.lod_level());
            source == Some(&bound.0) && revision.0 == status.volume_generation && patch.instance_count() > 0
                && world.get::<GpuBladeDensityMap>(*entity).is_some() && visible_descendant(world, *entity, root)
                && (global.to_matrix() * local.to_matrix().inverse()).abs_diff_eq(frame.source_to_world, 0.001)
        });
        json!({"id":layer.id,"runtime_layer_id":layer.runtime_id,"source_present":source.is_some(),
            "source_samples":status.volume_source_counts.get(&layer.id).copied().unwrap_or(0),
            "positive_source_samples":status.volume_source_positive_counts.get(&layer.id).copied().unwrap_or(0),
            "expected_windows":expected_windows,"actual_windows":actual.len(),"unique_lods":lods.len(),
            "bindings_current":bindings_current})
    }).collect::<Vec<_>>();
    let value = json!({"ready":status.volume_ready,"generation":status.volume_generation,
        "instance":instance.to_bits(),"instance_path":instance_path.to_string(),"saved_source_path":target.source_path,
        "root":root.to_bits(),"root_owned":visible_descendant(world, root, instance),
        "fields_current":fields_current,"fields":status.volume_fields.iter().map(|(entity, snapshot)|
            json!({"entity":entity.to_bits(),"snapshot":snapshot})).collect::<Vec<_>>(),
        "ground_revision":status.volume_ground_revision,
        "ground_current":current_ground.is_some() && current_ground == status.volume_ground_revision,
        "saved_layers":target.layers.iter().map(|layer| &layer.id).collect::<Vec<_>>(),"layers":layers});
    if !report_ready(&value) { return Err(format!("三维真实采用证据不完整：{value}")); }
    Ok(value)
}

fn report_ready(value: &Value) -> bool {
    value["ready"] == true && value["generation"].as_u64().unwrap_or(0) > 0
        && value["root_owned"] == true && value["fields_current"] == true && value["ground_current"] == true
        && value["ground_revision"].as_array().is_some_and(|words| words.len() == 7)
        && value["layers"].as_array().is_some_and(|layers| !layers.is_empty()
            && value["saved_layers"].as_array().is_some_and(|saved| saved.len() == layers.len()
                && saved.iter().all(|id| layers.iter().filter(|layer| &layer["id"] == id).count() == 1))
            && layers.iter().all(|layer| {
            let expected = layer["expected_windows"].as_u64().unwrap_or(0);
            let total = layer["source_samples"].as_u64().unwrap_or(0);
            let positive = layer["positive_source_samples"].as_u64().unwrap_or(0);
            layer["source_present"] == true && positive > 0 && positive <= total
                && expected > 0 && layer["actual_windows"].as_u64() == Some(expected)
                && layer["unique_lods"].as_u64() == Some(expected) && layer["bindings_current"] == true
        }))
}

fn visible_descendant(world: &World, mut entity: Entity, target: Entity) -> bool {
    for _ in 0..32 {
        if world.get::<Visibility>(entity) == Some(&Visibility::Hidden) { return false; }
        if entity == target { return true; }
        let Some(parent) = world.get::<ChildOf>(entity) else { return false; };
        entity = parent.parent();
    }
    false
}
