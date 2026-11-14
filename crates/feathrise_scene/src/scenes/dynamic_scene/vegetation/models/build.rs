//! 单飞后台适配模型几何与作者图层；只返回完整三角面候选和通用树灌木格，不生成逐株Entity。
use super::resources::*;
#[cfg(test)]
use crate::paint_surface::load_model_surface_source;
use crate::paint_surface::{MeshSurfaceSnapshot, read_model_surface_source};
use bevy::prelude::*;
use feathrise_environment::vegetation::blade::{
    GpuBladeProfileBank, GpuBladeProfileBankId, GpuBladeProfileSlot, GpuBladeProfiles,
    GpuBladeVolumeSource, GPU_GRASS_PROFILE_CAPACITY,
};
use feathrise_scene_content::{
    SceneVegetationAsset, SceneVegetationKind, SurfaceVegetationCompiler,
    VegetationPresetDescriptor, VegetationPresetRuntime,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    sync::Arc,
};

const UPLOAD_BUDGET: usize = 8 * 1024 * 1024;

#[cfg(test)]
pub(super) fn build(
    root: &Path,
    definition: Arc<SceneVegetationAsset>,
    objects: Vec<ObjectInput>,
    presets: Vec<VegetationPresetDescriptor>,
    mut warnings: Vec<String>,
) -> Result<Build, String> {
    let compiler = SurfaceVegetationCompiler::new(&definition)?;
    if presets.len() != definition.layers.len() {
        return Err("模型植被预设与正式输出不同代".into());
    }
    let mut budget = UPLOAD_BUDGET;
    let mut output = VecDeque::new();
    for object in objects {
        let geometry = load_model_surface_source(root, &object.path, object.target);
        if let Some(object) = append_object(
            &definition,
            &compiler,
            object,
            geometry,
            &presets,
            &mut warnings,
            &mut budget,
        )? {
            output.push_back(object);
        }
    }
    finish(output, warnings, presets)
}

/// 单个来源逐项IO，不在内存堆积全场GLB字节；每次await都会释放计算worker给其他域。
pub(super) async fn build_async(
    root: &Path,
    definition: Arc<SceneVegetationAsset>,
    objects: Vec<ObjectInput>,
    presets: Vec<VegetationPresetDescriptor>,
    mut warnings: Vec<String>,
) -> Result<Build, String> {
    let compiler = SurfaceVegetationCompiler::new(&definition)?;
    if presets.len() != definition.layers.len() {
        return Err("模型植被预设与正式输出不同代".into());
    }
    let mut budget = UPLOAD_BUDGET;
    let mut output = VecDeque::new();
    for object in objects {
        let root = root.to_owned();
        let path = object.path.clone();
        let target = object.target;
        let source = bevy::tasks::IoTaskPool::get()
            .spawn(async move { read_model_surface_source(&root, &path, target) })
            .await;
        let geometry = source.and_then(|source| source.build());
        if let Some(object) = append_object(
            &definition,
            &compiler,
            object,
            geometry,
            &presets,
            &mut warnings,
            &mut budget,
        )? {
            output.push_back(object);
        }
    }
    finish(output, warnings, presets)
}

/// 去重、纹理分组及四槽映射随完整生成结果返回；规模相关扫描全部留在计算池。
fn finish(objects: VecDeque<ObjectBuild>, warnings: Vec<String>, presets: Vec<VegetationPresetDescriptor>)
    -> Result<Build, String> {
    let used: BTreeSet<_> = objects.iter().flat_map(|object| &object.layers)
        .filter_map(|layer| layer.definition.runtime_profile.clone()).collect();
    let profiles: BTreeMap<_, _> = presets.into_iter().filter(|preset| used.contains(preset.id.as_str()))
        .map(|preset| (preset.id.clone(), preset)).collect();
    if profiles.len() != used.len() { return Err("模型植被生成结果缺少完整预设依赖".into()); }
    let mut groups = BTreeMap::<Option<String>, Vec<&str>>::new();
    for (id, preset) in &profiles {
        if let VegetationPresetRuntime::Blade { base_color_texture_path, .. } = &preset.runtime {
            groups.entry(base_color_texture_path.clone()).or_default().push(id);
        }
    }
    let mut banks = Vec::new();
    let mut bindings = BTreeMap::new();
    for ids in groups.into_values() {
        for chunk in ids.chunks(GPU_GRASS_PROFILE_CAPACITY) {
            let bank = GpuBladeProfileBankId::for_dynamic_index(banks.len())
                .ok_or("模型植被外观批次数超出u32身份范围")?;
            for (index, id) in chunk.iter().enumerate() {
                bindings.insert((*id).to_owned(), (bank, GpuBladeProfileSlot::new(index as u32)
                    .expect("四槽分块保证局部槽有效")));
            }
            banks.push(GpuBladeProfileBank::new(GpuBladeProfiles::default(), None));
        }
    }
    let profiles = profiles.into_iter().map(|(id, preset)| ProfileDependency {
        preset, binding: bindings.remove(&id),
    }).collect();
    Ok(Build { objects, warnings, profiles, banks })
}

fn append_object(
    definition: &SceneVegetationAsset,
    compiler: &SurfaceVegetationCompiler,
    object: ObjectInput,
    geometry: Result<MeshSurfaceSnapshot, String>,
    presets: &[VegetationPresetDescriptor],
    warnings: &mut Vec<String>,
    budget: &mut usize,
) -> Result<Option<ObjectBuild>, String> {
    let geometry = match geometry {
        Ok(surface) => surface,
        Err(e) => {
            warnings.push(format!("{}：{e}；作者数据已保留", object.id));
            return Ok(None);
        }
    };
    if geometry.topology() != object.density.topology {
        warnings.push(format!(
            "{}：模型拓扑已改变，作者数据已保留，请重新绑定",
            object.id
        ));
        return Ok(None);
    }
    let resolved = geometry.resolved_vegetation(&object.density, Mat4::IDENTITY)?;
    let mut cells = BTreeMap::<usize, Vec<[[f32; 4]; 3]>>::new();
    for sample in &resolved {
        // 选择格属于源模型局部坐标，整体变换不重新洗牌作者组所有权。
        let densities = compiler.resolve(&sample.layers, [sample.position.x, sample.position.z]);
        for (index, density) in densities {
            let preset = &presets[index];
            let destination = cells.entry(index).or_default();
            let finest = match &preset.runtime {
                VegetationPresetRuntime::Blade { view_lod, .. } => view_lod.finest_grid_spacing,
                VegetationPresetRuntime::Mesh { .. } => object.density.spacing_meters,
            };
            let points = if preset.kind() == SceneVegetationKind::Blade {
                geometry.vegetation_candidates(
                    sample.anchor,
                    object.density.spacing_meters,
                    finest,
                    *budget / 48,
                )?
            } else {
                vec![(
                    geometry
                        .resolve(sample.anchor, Mat4::IDENTITY)
                        .ok_or("模型锚点已失效")?,
                    sample.cell_area,
                )]
            };
            let bytes = points.len().checked_mul(48).ok_or("模型植被候选大小溢出")?;
            *budget = budget
                .checked_sub(bytes)
                .ok_or("模型植被候选超过8MiB后台上传预算")?;
            for (point, area) in points {
                let p = point.position;
                let n = point.normal;
                let a = n.abs();
                let axis = if a.x > a.y && a.x > a.z {
                    0u32
                } else if a.y >= a.z {
                    1
                } else {
                    2
                };
                let mut hash = point.anchor.primitive.wrapping_mul(73856093)
                    ^ point.anchor.triangle.wrapping_mul(19349663)
                    ^ point.anchor.barycentric[0].to_bits().wrapping_mul(83492791)
                    ^ point.anchor.barycentric[1].to_bits();
                hash = (hash ^ (hash >> 16)).wrapping_mul(2246822519);
                hash ^= hash >> 13;
                destination.push([
                    [p.x, p.y, p.z, f32::from(density) / 255.],
                    [n.x, n.y, n.z, area.sqrt().max(0.00001)],
                    [p.x, p.y, p.z, (((hash & 0x003fffff) << 2) | axis) as f32],
                ]);
            }
        }
    }
    let mut layers = VecDeque::new();
    for (index, values) in cells {
        if values.is_empty() {
            continue;
        }
        let preset = &presets[index];
        let cells: Arc<[[[f32; 4]; 3]]> = values.into();
        let image = if preset.kind() == SceneVegetationKind::Blade {
            let image = GpuBladeVolumeSource::image_from_surface_samples(&cells, UPLOAD_BUDGET)?;
            // 前面已计算实际记录字节；再计入纹理行尾对齐的额外成本。
            let extra = image
                .data
                .as_ref()
                .map_or(0, Vec::len)
                .saturating_sub(cells.len() * 48);
            *budget = budget
                .checked_sub(extra)
                .ok_or("模型植被源超过8MiB上传预算")?;
            Some(image)
        } else {
            None
        };
        let mut layer = definition.layers[index].clone();
        if layer.kind != preset.kind() {
            return Err(format!("模型植被层{}后端与预设不一致", layer.id));
        }
        layer.runtime_layer_id = Some(preset.resolved_runtime_layer_id());
        layer.prototype_id = preset.resolved_prototype_id();
        layer.runtime_profile = Some(preset.id.clone());
        let first = Vec3::from_slice(&cells[0][0][..3]);
        let (min, max) = cells.iter().fold((first, first), |(min, max), c| {
            let p = Vec3::from_slice(&c[0][..3]);
            (min.min(p), max.max(p))
        });
        let finest_spacing = match &preset.runtime {
            VegetationPresetRuntime::Blade { view_lod, .. } => view_lod.finest_grid_spacing,
            _ => preset.spacing_meters,
        };
        layers.push_back(LayerBuild {
            definition: layer,
            cells,
            image,
            finest_spacing,
            min,
            max,
        });
    }
    Ok(Some(ObjectBuild {
        entity: object.entity,
        id: object.id,
        layers,
    }))
}
