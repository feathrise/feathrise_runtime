//! 新格式统一校验；空列表合法，显式新对象不得回退到旧主地形或吞掉坏LOD。

use std::collections::BTreeSet;
use crate::{MapManifestAsset, MapTerrainKind, SparseHeightSpec, TerrainId};

pub fn validate_terrain_objects(manifest: &MapManifestAsset) -> Result<(), String> {
    crate::vegetation_paint_groups::validate(&manifest.vegetation_paint_groups)?;
    let Some(objects) = &manifest.terrains else { return Ok(()); };
    if !manifest.terrain.is_empty() {
        return Err("对象地图不能同时保存旧主地形和新地形列表".into());
    }
    let mut ids = BTreeSet::new();
    for (index, object) in objects.iter().enumerate() {
        crate::validate_clipmap_stitches(object)?;
        TerrainId::parse(object.id.as_str())?;
        if !ids.insert(&object.id) { return Err(format!("地形ID重复：{}", object.id.as_str())); }
        if object.id == TerrainId::legacy() && index != 0 { return Err("兼容主地形必须位于对象列表首项".into()); }
        if object.display_name.trim().is_empty() { return Err("地形名称不能为空".into()); }
        object.transform.validate()?;
        if let Some(policy) = object.mesh_lod { policy.validate()?; }
        if (0..2).any(|i| !object.world_min[i].is_finite() || !object.world_max[i].is_finite()
            || object.world_min[i] >= object.world_max[i]) { return Err("地形源范围必须有限且具有正面积".into()); }
        let descriptor = &object.terrain;
        for path in [object.surface_path.as_deref(), object.vegetation_path.as_deref(), object.water_path.as_deref(),
            (!descriptor.glb_path.is_empty()).then_some(descriptor.glb_path.as_str()), descriptor.height_path.as_deref(),
            object.voxel.as_ref().map(|voxel| voxel.source_path.as_str())].into_iter().flatten() { validate_path(path)?; }
        match descriptor.kind {
            MapTerrainKind::Empty => return Err("地形对象不能使用空地图类型".into()),
            MapTerrainKind::Gltf | MapTerrainKind::Heightfield => {
                if !descriptor.base_height.is_finite() || !descriptor.height_scale.is_finite() || descriptor.height_scale <= 0.0 {
                    return Err("GLB源的高度范围必须有限且为正".into());
                }
                if descriptor.glb_path.trim().is_empty() { return Err("GLB地形缺少模型来源".into()); }
                if object.voxel.is_some() { return Err("GLB地形不能携带体素作者配置".into()); }
            }
            MapTerrainKind::SparseClipmap => {
                let spec = SparseHeightSpec::from_resolution(object.world_min, object.world_max,
                    descriptor.resolution, descriptor.base_height, descriptor.height_scale)?;
                let path = descriptor.height_path.as_deref().filter(|path| !path.is_empty()).ok_or("Clipmap地形缺少高度作者源")?;
                let namespace = crate::sparse_height_manifest_terrain_id(&manifest.id, path)?;
                if namespace.as_ref() != Some(&object.id) && !(namespace.is_none() && object.id == TerrainId::legacy()) {
                    return Err("Clipmap高度源必须属于当前地图和当前地形对象".into());
                }
                if spec.cells.map(|cells| cells as u32 + 1) != descriptor.resolution { return Err("Clipmap两轴分辨率与源范围、格距不一致".into()); }
                crate::validate_collision_lods(&descriptor.collision_lods, &spec)?;
                if let Some(lods) = &descriptor.display_lods { crate::validate_display_lod_shape(lods)?; }
                if object.voxel.is_some() { return Err("Clipmap地形不能携带独立体素作者配置".into()); }
            }
            MapTerrainKind::Voxel => {
                let volume = object.voxel.as_ref().ok_or("体素地形缺少独立作者配置")?;
                if !volume.cell_size_meters.is_finite() || !(0.01..=0.5).contains(&volume.cell_size_meters)
                    || (0..3).any(|i| !volume.min[i].is_finite() || !volume.max[i].is_finite() || volume.min[i] >= volume.max[i])
                    || volume.source_path.trim().is_empty() { return Err("体素精度、初始范围或作者路径无效".into()); }
                if volume.min.into_iter().chain(volume.max).any(|value| (value / volume.cell_size_meters).abs() > 32_000_000.0) {
                    return Err("体素源坐标超过安全整数格点范围".into());
                }
                if !volume.source_path.starts_with(&format!("{}/", object.private_folder(&manifest.id)))
                    || !volume.source_path.ends_with(".map_voxel_source.json") {
                    return Err("体素作者源必须位于当前地图的地形对象私有目录".into());
                }
                if object.world_min != [volume.min[0], volume.min[2]] || object.world_max != [volume.max[0], volume.max[2]] {
                    return Err("体素对象的XZ范围与三维作者范围不一致".into());
                }
                for lods in [&volume.display_lods, &volume.collision_lods] {
                    if lods.is_empty() { return Err("体素显示和碰撞分别至少需要一层LOD".into()); }
                    let mut distance = 0.0;
                    let mut multiplier = 1;
                    for lod in lods {
                        if !lod.max_distance_meters.is_finite() || lod.max_distance_meters <= distance
                            || !lod.spacing_multiplier.is_power_of_two() || lod.spacing_multiplier > 32768 || lod.spacing_multiplier < multiplier {
                            return Err("体素LOD距离必须递增，倍率必须为向外不减的二次幂".into());
                        }
                        distance = lod.max_distance_meters;
                        multiplier = lod.spacing_multiplier;
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':')
        || path.split('/').any(|part| part.is_empty() || matches!(part, "." | "..")) {
        return Err("地形作者路径必须为assets内的安全相对路径".into());
    }
    Ok(())
}
