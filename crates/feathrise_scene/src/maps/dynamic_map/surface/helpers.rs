//! 动态地图索引地表的定义校验、全局纹理请求和 PBR 材质构造。

use std::collections::BTreeSet;

use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;
use bevy::render::storage::ShaderBuffer;
use feathrise_environment::terrain_surface::{
    TerrainSurfaceExtension, TerrainSurfaceFailure, TerrainSurfaceFallbackImages,
    TerrainSurfaceIndexedDescriptor, TerrainSurfaceIndexedLayer, TerrainSurfaceMaterial,
    TerrainSurfaceMaterialOverride, load_terrain_surface_array, load_terrain_surface_image, terrain_surface_image_failed,
};
use feathrise_map_content::{MAP_SURFACE_SCHEMA_VERSION, MapSurfaceAsset, SurfacePresetRegistry};

use super::resources::DynamicSurfaceTextures;
use crate::maps::dynamic_map::{DYNAMIC_TERRAIN_MESH_NAME, DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME};

pub(super) fn source_identity(mesh: &str, material: &str) -> bool {
    mesh == DYNAMIC_TERRAIN_MESH_NAME && material == DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME
}

pub(super) fn retained_active_surface(
    active: Option<&Handle<TerrainSurfaceMaterial>>,
    installed: Option<&MeshMaterial3d<TerrainSurfaceMaterial>>,
    material_override: Option<&TerrainSurfaceMaterialOverride>,
) -> bool {
    let Some((active, installed)) = active.zip(installed) else {
        return false;
    };
    material_override.map_or_else(
        || active.id() == installed.0.id(),
        |material_override| material_override.matches(active.id(), installed.0.id()),
    )
}

fn safe_asset_path(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.contains("..") && !path.contains('\\')
}

/// 校验地图局部层及其全局预设引用；不设层数产品上限。
pub(super) fn validate_surface_definition(
    surface: &MapSurfaceAsset,
    presets: &SurfacePresetRegistry,
) -> Result<(), String> {
    if surface.schema_version != MAP_SURFACE_SCHEMA_VERSION {
        return Err(format!("地表格式版本{}不受支持", surface.schema_version));
    }
    if surface.resolution.contains(&0)
        || !safe_asset_path(&surface.weight_source_path)
        || !safe_asset_path(&surface.weight_texture_path)
        || !safe_asset_path(&surface.index_source_path)
        || !safe_asset_path(&surface.index_texture_path)
    {
        return Err("地表索引、权重路径或分辨率无效".to_owned());
    }
    if u32::try_from(surface.layers.len()).is_err() {
        // 空作者表代表裸地，不再要求隐含基础作者层。
        return Err("地表局部索引数量必须可由 u32 表达".to_owned()); // 只限制GPU索引表达能力。
    }
    let mut ids = BTreeSet::new();
    for layer in &surface.layers {
        if layer.id.trim().is_empty()
            || layer.label.trim().is_empty()
            || !ids.insert(layer.id.as_str())
        {
            return Err("地表层包含空白或重复的地图局部 ID".to_owned());
        }
        if presets.resolve(&layer.preset_id).is_none() {
            return Err(format!(
                "地表层{}引用了不存在的全局预设{}",
                layer.id, layer.preset_id
            ));
        }
    }
    Ok(())
}

pub(super) fn load_surface_textures(
    asset_server: &AssetServer,
    surface: &MapSurfaceAsset,
    presets: &SurfacePresetRegistry,
    fallback: &TerrainSurfaceFallbackImages, // 空作者层表使用内部中性绑定，不引用全局预设数组。
) -> DynamicSurfaceTextures {
    let (base_color_path, normal_path, arm_height_path) = presets.resolved.array_paths();
    DynamicSurfaceTextures {
        neutral_arrays: surface.layers.is_empty(), // 仅磁盘索引和权重继续参与AssetServer加载确认。
        weight: load_terrain_surface_image(
            asset_server,
            surface.weight_texture_path.clone(),
            false,
            false,
            "dynamic_terrain_surface_weights",
        ),
        index: load_terrain_surface_image(
            asset_server,
            surface.index_texture_path.clone(),
            false,
            false,
            "dynamic_terrain_surface_indices",
        ),
        base_color_array: if surface.layers.is_empty() {
            fallback.white_base_color_array.clone()
        } else {
            load_terrain_surface_array(
                asset_server,
                base_color_path.to_owned(),
                true,
                "dynamic_terrain_surface_base_color_array",
            )
        }, // 空表保留合法中性数组绑定。
        normal_array: if surface.layers.is_empty() {
            fallback.neutral_normal_array.clone()
        } else {
            load_terrain_surface_array(
                asset_server,
                normal_path.to_owned(),
                false,
                "dynamic_terrain_surface_normal_array",
            )
        }, // 空表不加载全局法线数组。
        arm_height_array: if surface.layers.is_empty() {
            fallback.neutral_arm_array.clone()
        } else {
            load_terrain_surface_array(
                asset_server,
                arm_height_path.to_owned(),
                false,
                "dynamic_terrain_surface_arm_height_array",
            )
        }, // 空表不加载全局ARM数组。
    }
}

pub(super) fn surface_texture_failure(
    asset_server: &AssetServer,
    textures: &DynamicSurfaceTextures,
) -> TerrainSurfaceFailure {
    if terrain_surface_image_failed(asset_server, &textures.weight) {
        TerrainSurfaceFailure::WeightMap
    } else if terrain_surface_image_failed(asset_server, &textures.index) {
        TerrainSurfaceFailure::IndexMap
    } else if terrain_surface_image_failed(asset_server, &textures.base_color_array) {
        TerrainSurfaceFailure::BaseColorArray
    } else if terrain_surface_image_failed(asset_server, &textures.normal_array) {
        TerrainSurfaceFailure::NormalArray
    } else if terrain_surface_image_failed(asset_server, &textures.arm_height_array) {
        TerrainSurfaceFailure::ArmArray
    } else {
        TerrainSurfaceFailure::None
    }
}

pub(super) fn surface_textures_loaded(
    asset_server: &AssetServer,
    textures: &DynamicSurfaceTextures,
) -> bool {
    textures
        .images()
        .iter()
        .take(if textures.neutral_arrays { 2 } else { 5 }) // 内部数组由RenderApp探针验证GPU就绪，AssetServer没有其磁盘状态。
        .all(|image| asset_server.is_loaded_with_dependencies(image.id()))
}

/// 构造本修订唯一材质及其局部层存储表；调用者负责与候选/活动材质同步回收表。
#[allow(clippy::too_many_arguments)]
pub(super) fn build_surface_material(
    mut white: StandardMaterial,
    surface: &MapSurfaceAsset,
    presets: &SurfacePresetRegistry,
    textures: &DynamicSurfaceTextures,
    fallback: &TerrainSurfaceFallbackImages,
    buffers: &mut Assets<ShaderBuffer>,
    world_min: Vec2,
    world_max: Vec2,
) -> Result<(TerrainSurfaceMaterial, Handle<ShaderBuffer>), String> {
    white.base_color = Color::WHITE;
    white.base_color_texture = None;
    white.metallic_roughness_texture = None;
    white.normal_map_texture = None;
    white.occlusion_texture = None;
    white.emissive_texture = None;
    white.perceptual_roughness = 1.0;
    white.metallic = 0.0;

    let mut table = Vec::with_capacity(surface.layers.len());
    let mut volume_entries = Vec::new();
    let mut preset_indices = std::collections::BTreeMap::new();
    let mut volume_soil = None;
    let mut volume_rock = None;
    for layer in &surface.layers {
        let preset = presets.resolve(&layer.preset_id).ok_or_else(|| {
            format!(
                "地表层{}的预设{}在构造材质前消失",
                layer.id, layer.preset_id
            )
        })?;
        let slice = presets
            .resolved
            .texture_slice(&layer.preset_id)
            .ok_or_else(|| format!("地表预设{}缺少数组切片", layer.preset_id))?;
        let linear = Color::srgba(
            preset.base_color_srgba[0],
            preset.base_color_srgba[1],
            preset.base_color_srgba[2],
            preset.base_color_srgba[3],
        )
        .to_linear();
        table.push(TerrainSurfaceIndexedLayer {
            base_color: Vec4::new(linear.red, linear.green, linear.blue, linear.alpha),
            pbr_and_texture: Vec4::new(
                preset.perceptual_roughness,
                preset.metallic,
                preset.tile_meters,
                slice as f32,
            ),
            normal_height_projection: Vec4::new(
                preset.normal_strength,
                preset.height_blend_strength,
                preset.projection.code() as f32,
                if preset.normal_y_flip { 1.0 } else { 0.0 },
            ),
        });
        let index = (table.len() - 1) as u32;
        volume_entries.push((preset.id.clone(), index));
        preset_indices.entry(preset.id.clone()).or_insert(index);
    }
    // 三维作者使用稳定预设ID；全局数组已经由此材质加载，不要求洞壁预设占用室外涂绘层。
    if !surface.layers.is_empty() {
        for preset in presets.presets() {
            let index = if let Some((_, index)) = preset_indices.get_key_value(&preset.id) {
                *index
            } else {
                let slice = presets
                    .resolved
                    .texture_slice(&preset.id)
                    .ok_or("体积地表预设缺少切片")?;
                let c = Color::srgba(
                    preset.base_color_srgba[0],
                    preset.base_color_srgba[1],
                    preset.base_color_srgba[2],
                    preset.base_color_srgba[3],
                )
                .to_linear();
                let index = table.len() as u32;
                table.push(TerrainSurfaceIndexedLayer {
                    base_color: Vec4::new(c.red, c.green, c.blue, c.alpha),
                    pbr_and_texture: Vec4::new(
                        preset.perceptual_roughness,
                        preset.metallic,
                        preset.tile_meters,
                        slice as f32,
                    ),
                    normal_height_projection: Vec4::new(
                        preset.normal_strength,
                        preset.height_blend_strength,
                        preset.projection.code() as f32,
                        if preset.normal_y_flip { 1.0 } else { 0.0 },
                    ),
                });
                volume_entries.push((preset.id.clone(), index));
                preset_indices.insert(preset.id.clone(), index);
                index
            };
            for alias in &preset.aliases {
                volume_entries.push((alias.clone(), index));
            }
            let name = format!("{} {}", preset.id, preset.label).to_lowercase();
            if volume_soil.is_none()
                && (name.contains("soil")
                    || name.contains("dirt")
                    || name.contains("泥")
                    || name.contains("土"))
            {
                volume_soil = Some(index);
            }
            if volume_rock.is_none()
                && (name.contains("rock")
                    || name.contains("stone")
                    || name.contains("岩")
                    || name.contains("石"))
            {
                volume_rock = Some(index);
            }
        }
    }
    if table.is_empty() {
        // GPU存储绑定不能是零字节，哨兵不是作者层且不会被Shader取样。
        table.push(TerrainSurfaceIndexedLayer {
            // 空表只创建固定大小中性绑定，避免借用任意全局预设。
            base_color: Vec4::ONE, // 有效层数为零时不读取此字段。
            pbr_and_texture: Vec4::new(1.0, 0.0, 1.0, 0.0), // 固定中性粗糙度与合法填充值。
            normal_height_projection: Vec4::ZERO, // 不添加法线、高度或投影变化。
        }); // 完成GPU哨兵。
    } // 作者定义仍保持真正空列表。
    let soil = volume_soil.unwrap_or_else(|| {
        let i = table.len() as u32;
        table.push(TerrainSurfaceIndexedLayer {
            base_color: Vec4::new(0.24, 0.15, 0.08, 1.0),
            pbr_and_texture: Vec4::new(0.94, 0.0, 1.0, -1.0),
            normal_height_projection: Vec4::ZERO,
        });
        i
    });
    let rock = volume_rock.unwrap_or_else(|| {
        let i = table.len() as u32;
        table.push(TerrainSurfaceIndexedLayer {
            base_color: Vec4::new(0.25, 0.26, 0.24, 1.0),
            pbr_and_texture: Vec4::new(0.88, 0.0, 1.0, -1.0),
            normal_height_projection: Vec4::ZERO,
        });
        i
    });
    let lookup = buffers.add(ShaderBuffer::from(
        feathrise_environment::terrain_surface::build_volume_lookup(&volume_entries, soil, rock)?,
    ));
    let layer_table = buffers.add(ShaderBuffer::from(table));
    let mut material = ExtendedMaterial {
        base: white,
        extension: TerrainSurfaceExtension::new_indexed(TerrainSurfaceIndexedDescriptor {
            world_min,
            world_max,
            weight_map: textures.weight.clone(),
            index_map: textures.index.clone(),
            base_color_array: textures.base_color_array.clone(),
            normal_array: textures.normal_array.clone(),
            arm_height_array: textures.arm_height_array.clone(),
            layer_table: layer_table.clone(),
            layer_count: u32::try_from(surface.layers.len())
                .map_err(|_| "地图地表层数量超出 u32 可表达范围".to_owned())?,
            zero_weight: fallback.zero_weight.clone(),
        }),
    };
    material.extension.set_volume_lookup(lookup);
    Ok((material, layer_table))
}

#[cfg(test)]
mod tests {
    #[test] // 清空作者层必须合法且无需解析任何全局预设，GPU绑定仍必须非空。
    fn empty_surface_definition_builds_without_global_presets() {
        let surface = feathrise_map_content::MapSurfaceAsset {
            // 保留完整作者文件身份与合法纹理路径。
            schema_version: feathrise_map_content::MAP_SURFACE_SCHEMA_VERSION, // 使用正式版本。
            map_id: feathrise_map_content::MapId::parse("MAP-empty-surface").unwrap(), // 独立地图身份。
            weight_source_path: "maps/empty/weights.png".into(),         // 空地表仍保存权重作者源。
            weight_texture_path: "maps/empty/weights.ktx2".into(),       // 零权重运行时图。
            index_source_path: "maps/empty/indices.bin".into(),          // 零索引作者源。
            index_texture_path: "maps/empty/indices.ktx2".into(),        // 零索引运行时图。
            resolution: [1, 1],                                          // 固定最小测试规模。
            layers: Vec::new(), // 最后一层删除后的真实定义。
        }; // 完成空地表夹具。
        let presets = feathrise_map_content::SurfacePresetRegistry {
            // 空目录证明没有隐含材质依赖。
            catalog: Handle::default(),   // 测试不加载任何全局目录资产。
            revision: 0,                  // 尚无有效全局预设也应支持裸地。
            resolved: Default::default(), // 使用生产空快照类型。
            last_error: None,             // 无加载错误干扰构造验证。
        }; // 完成与生产启动状态一致的空目录。
        assert!(super::validate_surface_definition(&surface, &presets).is_ok()); // 空表必须通过运行时校验。
        let mut world = World::new(); // 仅创建内部图片与GPU表资产，不启动渲染器。
        world.init_resource::<Assets<Image>>(); // 回退图片需要正式资产容器。
        world.init_resource::<Assets<bevy::render::storage::ShaderBuffer>>(); // 回退表需要正式资产容器。
        let fallback =
            feathrise_environment::terrain_surface::TerrainSurfaceFallbackImages::from_world(&mut world); // 创建与运行时相同的中性绑定。
        let textures = super::DynamicSurfaceTextures {
            // 空作者表的合法纹理句柄组。
            neutral_arrays: true,                 // 内部数组不经过磁盘加载状态。
            weight: fallback.zero_weight.clone(), // 全零权重。
            index: fallback.zero_index.clone(),   // 全零索引。
            base_color_array: fallback.white_base_color_array.clone(), // 无全局基础色引用。
            normal_array: fallback.neutral_normal_array.clone(), // 无全局法线引用。
            arm_height_array: fallback.neutral_arm_array.clone(), // 无全局ARM引用。
        }; // 完成中性纹理组。
        let mut buffers = world.resource_mut::<Assets<bevy::render::storage::ShaderBuffer>>(); // 观察材质构造真正登记GPU表。
        let (_, table) = super::build_surface_material(
            StandardMaterial::default(),
            &surface,
            &presets,
            &textures,
            &fallback,
            &mut buffers,
            Vec2::ZERO,
            Vec2::ONE,
        )
        .unwrap(); // 空预设目录也应成功构造。
        assert!(buffers.get(&table).is_some()); // 零作者层仍有合法非空哨兵绑定。
        assert!(surface.layers.is_empty()); // GPU哨兵不能回写成作者层。
    } // 完成清空地表运行时构造回归。
    use super::retained_active_surface;
    use bevy::asset::uuid_handle;
    use bevy::prelude::*;
    use feathrise_environment::terrain_surface::{
        TerrainSurfaceMaterial, TerrainSurfaceMaterialOverride,
    };

    #[test]
    fn same_active_material_is_recognized_but_stale_material_is_not() {
        let active = Handle::<TerrainSurfaceMaterial>::default();
        let installed = MeshMaterial3d(active.clone());
        let stale = MeshMaterial3d(uuid_handle!("5de6178f-3f9d-43e5-98a7-5056b19c8a91"));
        let deferred = uuid_handle!("35de6178-f3f9-43e5-98a7-5056b19c8a92");
        let deferred_installed = MeshMaterial3d(deferred.clone());
        let material_override = TerrainSurfaceMaterialOverride::new(&active, &deferred);

        assert!(retained_active_surface(
            Some(&active),
            Some(&installed),
            None
        ));
        assert!(retained_active_surface(
            Some(&active),
            Some(&deferred_installed),
            Some(&material_override),
        ));
        assert!(!retained_active_surface(
            Some(&active),
            Some(&installed),
            Some(&material_override),
        ));
        assert!(!retained_active_surface(Some(&active), Some(&stale), None));
        assert!(!retained_active_surface(Some(&active), None, None));
    }
}
