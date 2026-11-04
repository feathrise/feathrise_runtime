//! Blade修订根与Mesh空间块的隐藏分帧生成。

use bevy::camera::{primitives::Aabb, visibility::NoAutoAabb};
use bevy::math::Vec3A;
use bevy::prelude::*;
use feathrise_environment::vegetation::blade::{
    GPU_GRASS_PATCH_AABB_HORIZONTAL_MARGIN, GpuBladeDensityMap, GpuBladeHeightMap, GpuBladePatch,
    GpuBladeProfileBanks,
};
use feathrise_environment::vegetation::{
    VegetationBladeLayer, VegetationLayerId, VegetationMeshBatchPlacement,
    VegetationMeshPrototypeRegistry, VegetationPrototypeId, spawn_vegetation_mesh_batch,
};
use feathrise_map_content::{MapVegetationKind, VegetationPresetRegistry};
use feathrise_scene_core::state::ScenePhase;

use super::collision::resolve_mesh_preset_physics;
use super::components::{DynamicBladeWindow, DynamicVegetationRoot};
use super::resources::{
    DynamicMeshChunkBuild, DynamicMeshChunkStaging, DynamicMeshLayerBuild,
    DynamicVegetationPending, DynamicVegetationRootBuild,
}; // 修订根和单块采用保持独立生命周期。

fn blade_window_count(pending: &DynamicVegetationPending) -> usize {
    pending
        .layers
        .iter()
        .filter(|layer| layer.definition.kind == MapVegetationKind::Blade)
        .map(|layer| {
            layer
                .blade_profile
                .as_ref()
                .map(|profile| profile.lods.len())
                .unwrap_or(0)
        })
        .sum() // 每层按自身可变档数创建窗口，不再固定四窗。
}

/// 建立当前修订父根并立即创建每层N个正式GPU窗口；Mesh空间块随后按视野加入。
pub(super) fn create_vegetation_root(
    commands: &mut Commands,
    pending: &DynamicVegetationPending,
    banks: &mut GpuBladeProfileBanks,
) -> Result<DynamicVegetationRootBuild, String> {
    let height_cells = pending
        .height_cells
        .as_ref()
        .cloned()
        .filter(|_| !pending.layers.is_empty());
    if !pending.layers.is_empty() && height_cells.is_none() && pending.clipmap_height.is_none()
        && pending.manifest.terrain.kind != feathrise_map_content::MapTerrainKind::Voxel {
        // 两种高度择一。
        return Err("非空植被修订缺少高度单元句柄".to_owned());
    }
    let world_min = Vec2::from_array(pending.manifest.world_min);
    let world_max = Vec2::from_array(pending.manifest.world_max);
    let blade_layer_count = pending
        .layers
        .iter()
        .filter(|layer| layer.definition.kind == MapVegetationKind::Blade)
        .count();
    let blade_window_count = blade_window_count(pending);
    if pending.layers.iter().filter_map(|layer| layer.blade_profile_bank)
        .any(|id| id.index() == 0 || id.index() as usize > pending.blade_profile_banks.len()) {
        return Err("植被局部绘制批次不在当前完整目录中".into());
    }
    // 隐藏候选与旧根持有不同的稳定槽；新增空网格也不能替换其他网格的GPU目录。
    let allocation = banks.allocate(pending.blade_profile_banks.clone())
        .ok_or("植被绘制批次超过GPU身份可表示范围")?;
    let root = commands
        .spawn((
            Name::new(format!("动态地图植被_{}", pending.map_id)), // 当前修订根只拥有Blade窗口和按需Mesh块。
            DynamicVegetationRoot,
            Transform::IDENTITY,
            Visibility::Inherited, // Blade窗口同一命令批完整创建，无需等待全图Mesh常驻。
            DespawnOnExit(ScenePhase::Active),
        ))
        .id();
    if blade_layer_count > 0 {
        let origin = Vec3::new(
            pending.manifest.default_spawn[0],
            pending.manifest.terrain.base_height,
            pending.manifest.default_spawn[2],
        );
        // Clipmap页已经携带同版真实范围；旧v2量程仅服务GLB，不能裁掉新浮点山峰或深谷的草窗。
        let (height_min, height_max) = pending.clipmap_height.as_ref().map_or_else(
            || (pending.manifest.terrain.base_height,
                pending.manifest.terrain.base_height + pending.manifest.terrain.height_scale),
            |height| (height.height_min, height.height_max),
        );
        commands.entity(root).with_children(|parent| {
            for layer in &pending.layers {
                if layer.definition.kind != MapVegetationKind::Blade {
                    continue;
                }
                let (Some(profile), Some(meshes), Some(runtime_layer_id), Some(profile_bank)) = (
                    layer.blade_profile.as_ref(),
                    layer.blade_meshes.as_ref(),
                    layer.definition.runtime_layer_id,
                    layer.blade_profile_bank,
                ) else {
                    continue;
                };
                let profile_bank = allocation.bank(profile_bank.index() as usize - 1)
                    .expect("建根前已完整核对局部绘制批次");
                let layer_seed =
                    layer.definition.seed as u32 ^ (layer.definition.seed >> 32) as u32;
                for (lod, mesh) in profile.lods.iter().zip(meshes.iter()) {
                    let extent = lod.patch_extent();
                    // 最粗档才应用作物超远水平覆盖补偿。
                    let is_coarsest = lod.level as usize + 1 == profile.lods.len();
                    let vegetation_config =
                        if is_coarsest && profile.ultra_horizontal_sweep > 0.0 {
                            feathrise_environment::vegetation::blade::GpuBladeConfig {
                                max_horizontal_sweep: profile.ultra_horizontal_sweep,
                                ..profile.vegetation_config
                            }
                        } else {
                            profile.vegetation_config
                        };
                    let patch =
                        GpuBladePatch::square(origin, extent, lod.grid_side, lod.seed ^ layer_seed)
                            .with_lod_range(
                                lod.inner_radius,
                                lod.outer_radius,
                                lod.inner_fade_width,
                                lod.outer_fade_width,
                            )
                            .with_lod_level(lod.level)
                            .with_hierarchical_lod(true)
                            .with_ultra_lod_budget(profile.ultra_lod_budget)
                            .with_vegetation_config(vegetation_config)
                            .with_view_lod_config(profile.view_lod) // 每层全部窗口共享真实档数及各档局部规格。
                            .with_height_field(world_min, world_max, height_min, height_max)
                            .with_density_field(world_min, world_max)
                            .with_density_gain(profile.density_gain)
                            .with_activation_coverage(1.0);
                    let world_y_min = height_min - vegetation_config.root_penetration - 0.2;
                    let world_y_max = height_max + vegetation_config.max_blade_height + 0.2;
                    let horizontal_margin = GPU_GRASS_PATCH_AABB_HORIZONTAL_MARGIN
                        .max(vegetation_config.max_horizontal_sweep);
                    let mut window = parent.spawn((
                        // 按后端补绑定。
                        Name::new(format!("动态GPU叶片_{}_{}", layer.definition.id, lod.name)),
                        Mesh3d(mesh.clone()),
                        patch,
                        profile_bank, // 当前修订会话批次选择对应四槽外观和纹理页。
                        GpuBladeDensityMap::new(layer.density.clone()),
                        Transform::from_translation(origin),
                        Aabb {
                            center: Vec3A::new(
                                0.0,
                                (world_y_min + world_y_max) * 0.5 - origin.y,
                                0.0,
                            ),
                            half_extents: Vec3A::new(
                                extent * 0.5 + horizontal_margin,
                                (world_y_max - world_y_min) * 0.5,
                                extent * 0.5 + horizontal_margin,
                            ),
                        },
                        NoAutoAabb,
                        DynamicBladeWindow {
                            snap_cells: lod.snap_cells,
                        },
                        VegetationBladeLayer(VegetationLayerId(runtime_layer_id)),
                        super::volume::VolumeVegetationLayer(layer.definition.id.clone()),
                    ));
                    if let Some(height) = pending.clipmap_height.as_ref() {
                        // 分页贴地。
                        window.insert(height.clone()); // 同次提交。
                    } else if let Some(height) = height_cells.as_ref() {
                        // GLB采样。
                        window.insert(GpuBladeHeightMap::new(height.clone())); // 保留旧图。
                    } else if pending.manifest.terrain.kind == feathrise_map_content::MapTerrainKind::Voxel {
                        // 保留完整预设/LOD作为三维草窗模板；没有承托源时本模板不渲染。
                        window.insert(Visibility::Hidden);
                    } // 单一高度后端。
                }
            }
        });
    }
    commands.entity(root).insert(allocation); // 根退役自动归还自己的槽位，兄弟网格的ID不变。
    Ok(DynamicVegetationRootBuild {
        root,
        blade_layer_count,
        blade_window_count,
    })
}

/// 为一个后台完成的空间块建立隐藏父根；该父根完整后才一次显示。
pub(super) fn create_mesh_chunk_staging(
    commands: &mut Commands,
    active_root: Entity,
    build: DynamicMeshChunkBuild,
) -> DynamicMeshChunkStaging {
    let key = build.key; // 名称和父子关系都使用同一稳定块身份。
    let root = commands
        .spawn((
            Name::new(format!(
                "动态Mesh植被块_{}_{}_{}",
                key.layer_index, key.x, key.z
            )), // F3层级可直接定位当前层和世界块。
            Transform::IDENTITY, // 实例根继续使用世界变换，块父根不施加空间偏移。
            Visibility::Hidden,  // 当前块全部实例排入前不显示半块内容。
            ChildOf(active_root), // 修订根回收时自动带走全部常驻块。
        ))
        .id();
    DynamicMeshChunkStaging {
        root,  // 批次节点落地后该父根一次显示。
        build, // 完成值直接转成一个逐株实例缓冲，不再扩张逐株ECS。
    }
}

fn mesh_chunk_batch_id(key: super::resources::DynamicMeshChunkKey) -> u64 {
    ((key.x as u32 as u64) << 32) | key.z as u32 as u64 // 层身份已单独保存，二维有符号块坐标逐位保持唯一。
}

/// 把一个完整后台空间块转成单个共享实例批次；作者数量不受运行时硬上限影响。
pub(super) fn spawn_mesh_chunk_batch(
    commands: &mut Commands,
    layer: &DynamicMeshLayerBuild,
    staging: &DynamicMeshChunkStaging,
    registry: &VegetationMeshPrototypeRegistry,
    presets: &VegetationPresetRegistry,
) -> Result<bool, String> {
    if staging.build.transforms.is_empty() {
        return Ok(true); // 空块仍会登记常驻，避免稳定焦点重复提交同一后台任务。
    }
    let runtime_layer_id = layer
        .layer
        .runtime_layer_id
        .ok_or_else(|| format!("Mesh植被层{}缺少正式运行时层ID", layer.layer.id))?;
    let prototype_id = layer
        .layer
        .prototype_id
        .ok_or_else(|| format!("Mesh植被层{}缺少正式原型ID", layer.layer.id))?;
    let prototype = registry
        .get(VegetationPrototypeId(prototype_id))
        .ok_or_else(|| {
            format!(
                "Mesh植被层{}的正式四级原型{}尚未就绪",
                layer.layer.id, prototype_id
            )
        })?;
    let profile = layer
        .layer
        .runtime_profile
        .as_deref()
        .ok_or_else(|| format!("Mesh植被层{}缺少正式预设ID", layer.layer.id))?;
    let embed_depth = resolve_mesh_preset_physics(presets, profile)?.ground_embed_depth;
    let placements = staging
        .build
        .transforms
        .iter()
        .map(|transform| {
            let mut translation = Vec3::from_array(transform.translation);
            translation.y -= embed_depth;
            VegetationMeshBatchPlacement::new(
                transform.instance_id, // 候选稳定ID继续与保存、重载和未来近场代理一一对应。
                Transform::from_translation(translation)
                    .with_rotation(Quat::from_rotation_y(transform.yaw_radians))
                    .with_scale(Vec3::splat(transform.scale)),
            )
        })
        .collect();
    let batch = spawn_vegetation_mesh_batch(
        commands,
        VegetationLayerId(runtime_layer_id),
        prototype,
        mesh_chunk_batch_id(staging.build.key),
        placements,
        Transform::IDENTITY,
    );
    commands.entity(batch).insert(ChildOf(staging.root));
    Ok(true) // 一个块只增加一个批次根和有限LOD部件节点，不需要按逐株游标分帧。
}

// 窗口数随各层 Blade 规格档数变化；不再存在固定「每层四窗」单测。
