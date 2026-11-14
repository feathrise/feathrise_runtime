//! 已完成模型密度复用正式预设、GPU草与树灌木批次；所有节点在完整提交前保持隐藏。
use super::super::super::mesh_profiles::{
    DynamicMeshProfileCache, DynamicMeshProfileRequests, DynamicMeshProfileState,
};
use super::resources::*;
use crate::gpu_grass_content::load_blade_preset;
use bevy::{
    camera::{primitives::Aabb, visibility::NoAutoAabb},
    ecs::system::SystemParam,
    math::Vec3A,
    prelude::*,
};
use feathrise_environment::vegetation::{
    VegetationBladeLayer, VegetationLayerId, VegetationMeshPrototypeRegistry,
    VegetationPrototypeId, VolumeMeshCells,
    blade::{GpuBladePatch, GpuBladeProfileBanks, GpuBladeVolumeSource},
    spawn_volume_mesh_batch,
};
use feathrise_scene_content::{
    SceneVegetationKind, VegetationPresetRegistry,
};
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const PROFILE_STEPS_PER_FRAME: usize = 8;

#[cfg(test)]
#[path = "spawn_tests.rs"]
mod tests;

#[derive(SystemParam)]
pub(super) struct Assets<'w> {
    pub server: Res<'w, AssetServer>,
    images: ResMut<'w, bevy::asset::Assets<Image>>,
    meshes: ResMut<'w, bevy::asset::Assets<Mesh>>,
    banks: ResMut<'w, GpuBladeProfileBanks>,
    prototypes: Res<'w, VegetationMeshPrototypeRegistry>,
    profiles: InstanceRes<'w, DynamicMeshProfileState>,
    requests: InstanceResMut<'w, DynamicMeshProfileRequests>,
    cache: Res<'w, DynamicMeshProfileCache>,
}

pub(super) fn prepare(
    identity: Identity,
    mut build: Build,
) -> Waiting {
    // 后台已给出完整依赖和四槽映射，此处只转移所有权，不重新扫描全部对象。
    let banks = std::mem::take(&mut build.banks);
    Waiting {
        identity,
        build,
        blades: BTreeMap::new(),
        banks,
        bindings: BTreeMap::new(),
        staging: None,
        retained: BTreeMap::new(),
        blade_count: 0,
        mesh_count: 0,
        prepared: false,
        checks: VecDeque::new(),
        mesh_checks: VecDeque::new(),
        missing_uv: VecDeque::new(),
        seen_meshes: BTreeSet::new(),
    }
}

fn ready(
    assets: &mut Assets,
    waiting: &mut Waiting,
    presets: &VegetationPresetRegistry,
) -> Result<bool, String> {
    // 一帧最多八项，每个Blade最多八档；已通过项从队列移除，不在等待帧重扫全场。
    for _ in 0..PROFILE_STEPS_PER_FRAME {
        let Some(dependency) = waiting.build.profiles.pop_front() else { break; };
        let preset = dependency.preset;
        match preset.kind() {
            SceneVegetationKind::Blade => {
                let blade = load_blade_preset(&assets.server, &preset)?;
                let (bank, slot) = dependency.binding.ok_or("模型草依赖缺少后台批次映射")?;
                let destination = waiting.banks.get_mut(bank.index().saturating_sub(1) as usize)
                    .ok_or("模型草后台批次映射越界")?;
                destination.profiles.set(slot, blade.appearance);
                destination.blade_base_color = blade.base_color_texture.clone();
                for (lod, handle) in blade.meshes.iter().enumerate() {
                    if waiting.seen_meshes.insert(handle.id()) {
                        waiting.mesh_checks.push_back((handle.clone(), format!("{} LOD{lod}", blade.spec.label)));
                    }
                }
                waiting.bindings.insert(preset.id.clone(), (bank, slot));
                waiting.checks.push_back(ProfileCheck::Blade(preset.id.clone()));
                waiting.blades.insert(preset.id, blade);
            }
            SceneVegetationKind::Mesh => {
                if !assets.requests.contains(&preset.id) { assets.requests.request(&preset.id)?; }
                waiting.checks.push_back(ProfileCheck::Mesh(preset.id));
            }
        }
    }
    if !waiting.build.profiles.is_empty() { return Ok(false); }
    let count = waiting.checks.len().min(PROFILE_STEPS_PER_FRAME);
    for _ in 0..count {
        let check = waiting.checks.pop_front().unwrap();
        let loaded = match &check {
            ProfileCheck::Blade(id) => {
                let blade = waiting.blades.get(id).ok_or("模型草准备地址已失效")?;
                let mut loaded = true;
                for mesh in &blade.meshes {
                    if super::super::helpers::handle_failed(&assets.server, mesh) {
                        return Err(format!("模型植被网格{}加载失败", blade.spec.label));
                    }
                    loaded &= assets.meshes.get(mesh).is_some();
                }
                if let Some(texture) = &blade.base_color_texture {
                    if super::super::helpers::handle_failed(&assets.server, texture) {
                        return Err(format!("模型植被纹理{}加载失败", blade.spec.label));
                    }
                    loaded &= assets.server.is_loaded_with_dependencies(texture.id());
                }
                loaded
            }
            ProfileCheck::Mesh(id) => {
                if let Some(error) = assets.profiles.failure_for(id, presets, &assets.cache) {
                    return Err(error.into());
                }
                assets.profiles.ready_for(id, presets, &assets.prototypes)
            }
        };
        if !loaded { waiting.checks.push_back(check); }
    }
    if !waiting.checks.is_empty() { return Ok(false); }
    // 全体ABI先只读校验，再开始修UV；后面的坏LOD不能使前面的网格被提前修改。
    for _ in 0..PROFILE_STEPS_PER_FRAME {
        let Some((handle, label)) = waiting.mesh_checks.front() else { break; };
        let Some(mesh) = assets.meshes.get(handle) else { return Ok(false); };
        let state = super::super::helpers::inspect_dynamic_blade_mesh(mesh)
            .map_err(|error| format!("{label}顶点布局无效：{error}"))?;
        let item = waiting.mesh_checks.pop_front().unwrap();
        if state == super::super::helpers::DynamicBladeUvState::Missing {
            waiting.missing_uv.push_back(item);
        }
    }
    if !waiting.mesh_checks.is_empty() { return Ok(false); }
    for _ in 0..PROFILE_STEPS_PER_FRAME {
        let Some(item) = waiting.missing_uv.front().cloned() else { break; };
        if super::super::helpers::prepare_blade_mesh_handles(&mut assets.meshes, [item])?.is_none() {
            return Ok(false);
        }
        waiting.missing_uv.pop_front();
    }
    Ok(waiting.missing_uv.is_empty())
}

/// 每帧至多安装一个作者层；每层最多八档GPU窗或一份固定节点的Mesh批次。
pub(super) fn advance(
    commands: &mut Commands,
    assets: &mut Assets,
    waiting: &mut Waiting,
    presets: &VegetationPresetRegistry,
) -> Result<bool, String> {
    if !waiting.prepared {
        if !ready(assets, waiting, presets)? {
            return Ok(false);
        }
        waiting.prepared = true;
    }
    if waiting.staging.is_none() {
        let Some(object) = waiting.build.objects.pop_front() else {
            return Ok(true);
        };
        if object.layers.is_empty() {
            return Ok(false);
        }
        let root = commands
            .spawn((
                Name::new(format!("模型植被_{}", object.id)),
                Transform::IDENTITY,
                Visibility::Hidden,
                ChildOf(object.entity),
                DespawnOnExit(feathrise_scene_core::state::ScenePhase::Active),
            ))
            .id();
        waiting.staging = Some(Staging {
            root,
            source: object.entity,
            id: object.id,
            layers: object.layers,
            banks: BTreeMap::new(),
        });
    }
    let staging = waiting.staging.as_mut().unwrap();
    let Some(mut layer) = staging.layers.pop_front() else {
        return Err("模型植被暂存游标为空".into());
    };
    let definition = &layer.definition;
    let profile_id = definition
        .runtime_profile
        .as_deref()
        .ok_or("模型植被输出缺少预设")?;
    match definition.kind {
        SceneVegetationKind::Blade => {
            let blade = waiting
                .blades
                .get(profile_id)
                .ok_or("模型草预设未完整请求")?;
            let (bank, slot) = waiting
                .bindings
                .get(profile_id)
                .copied()
                .ok_or("模型草绘制批次映射缺失")?;
            let bank = if let Some(entry) = staging.banks.get(&bank.index()) {
                *entry
            } else {
                let profile = waiting.banks.get(bank.index().saturating_sub(1) as usize)
                    .ok_or("模型草局部绘制批次越界")?;
                let allocation = assets.banks.allocate(vec![profile.clone()])
                    .ok_or("模型植被绘制批次身份耗尽")?;
                let allocated = allocation.bank(0).ok_or("模型绘制批次映射失效")?;
                commands.spawn((Name::new("模型植被外观批次"), Transform::IDENTITY,
                    Visibility::Inherited, ChildOf(staging.root), allocation));
                staging.banks.insert(bank.index(), allocated);
                allocated
            };
            let source = assets
                .images
                .add(layer.image.take().ok_or("模型草表面图片缺失")?);
            let center = (layer.min + layer.max) * 0.5;
            let seed = definition.seed as u32 ^ (definition.seed >> 32) as u32;
            for (lod, mesh) in blade.spec.lods.iter().zip(&blade.meshes) {
                let mut config = blade.spec.vegetation_config;
                config.profile_slot = slot;
                if lod.level as usize + 1 == blade.spec.lods.len() {
                    config.max_horizontal_sweep = config
                        .max_horizontal_sweep
                        .max(blade.spec.ultra_horizontal_sweep);
                }
                let patch = GpuBladePatch::square(
                    center,
                    lod.patch_extent(),
                    lod.grid_side,
                    lod.seed ^ seed,
                )
                .with_lod_range(
                    lod.inner_radius,
                    lod.outer_radius,
                    lod.inner_fade_width,
                    lod.outer_fade_width,
                )
                .with_lod_level(lod.level)
                .with_hierarchical_lod(true)
                .with_ultra_lod_budget(blade.spec.ultra_lod_budget)
                .with_vegetation_config(config)
                .with_view_lod_config(blade.spec.view_lod)
                .with_density_gain(blade.spec.density_gain)
                .with_activation_coverage(1.)
                .with_surface_sample_source(layer.cells.len() as u32, layer.finest_spacing)
                .with_volume_seed(seed);
                commands.spawn((
                    Name::new(format!("模型GPU叶片_{}_{}", definition.id, lod.name)),
                    ModelWindow {
                        source: staging.source,
                    },
                    Mesh3d(mesh.clone()),
                    patch,
                    bank,
                    VegetationBladeLayer(VegetationLayerId(
                        definition.runtime_layer_id.ok_or("模型草缺少正式层ID")?,
                    )),
                    GpuBladeVolumeSource(source.clone()),
                    Transform::from_translation(center),
                    Visibility::Inherited,
                    Aabb {
                        center: Vec3A::ZERO,
                        half_extents: Vec3A::from(
                            (layer.max - layer.min) * 0.5
                                + Vec3::splat(
                                    config.max_blade_height + config.max_horizontal_sweep + 0.3,
                                ),
                        ),
                    },
                    NoAutoAabb,
                    ChildOf(staging.root),
                ));
                waiting.blade_count += 1;
            }
        }
        SceneVegetationKind::Mesh => {
            let prototype = definition
                .prototype_id
                .and_then(|id| assets.prototypes.get(VegetationPrototypeId(id)))
                .ok_or("模型树灌木原型尚未就绪")?;
            let physics =
                super::super::collision::resolve_mesh_preset_physics(presets, profile_id)?;
            let batch = spawn_volume_mesh_batch(
                commands,
                VegetationLayerId(definition.runtime_layer_id.ok_or("模型植被缺少正式层ID")?),
                prototype,
                definition.seed,
                VolumeMeshCells {
                    cells: layer.cells.clone(),
                    seed: definition.seed as u32,
                    spacing: definition.spacing_meters,
                    scale_range: definition.scale_range,
                    ground_embed_depth: physics.ground_embed_depth,
                    min: layer.min,
                    max: layer.max,
                },
            );
            commands.entity(batch).insert(ChildOf(staging.root));
            waiting.mesh_count += 1;
        }
    }
    if staging.layers.is_empty() {
        let staging = waiting.staging.take().unwrap();
        waiting.retained.insert(staging.id, staging.root);
    }
    Ok(waiting.staging.is_none() && waiting.build.objects.is_empty())
}
