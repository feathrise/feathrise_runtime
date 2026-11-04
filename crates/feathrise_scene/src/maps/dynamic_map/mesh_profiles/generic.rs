//! 开发者目录声明的通用 Mesh 预设资产请求、就绪检查和原型装配。
//!
//! 【功能设计】一个预设可以声明任意数量 LOD 与部件；加载事务只持有当前一个预设，
//! 完整原型成功后才原子登记。地图不保存加载顺序、材质句柄或运行时批次身份。

use bevy::asset::{DependencyLoadState, LoadState, RecursiveDependencyLoadState, UntypedHandle};
use bevy::camera::{primitives::Aabb, visibility::VisibilityRange};
use bevy::prelude::*;
use bevy::render::render_resource::Face; // 显式单面覆盖恢复标准背面剔除。
use feathrise_environment::mesh_wind::{
    MeshWindLodLevel, MeshWindPart, TreeWindMaterial, TreeWindNoiseTexture,
    create_tree_wind_material, expanded_tree_wind_culling_aabb,
};
use feathrise_environment::vegetation::{
    VegetationHeightWeightedWind, VegetationMeshBatchVertexMode, VegetationMeshBillboard,
    VegetationMeshBillboardFrontAxis, VegetationMeshBillboardMaterial, VegetationMeshLod,
    VegetationMeshLodLevel, VegetationMeshPart, VegetationMeshPrototype, VegetationPrototypeId,
    create_vegetation_mesh_billboard_material,
};
use feathrise_map_content::{
    MeshPresetAlphaMode, MeshPresetLod, MeshPresetMaterialOverrides, MeshPresetMeshDerivation,
    MeshPresetPart, MeshPresetWindPart, VegetationPresetBillboard, VegetationPresetDescriptor,
    VegetationPresetRuntime,
};

use super::mesh_proxy::{build_rock_ultra_proxy, build_shrub_ultra_proxy}; // 目录派生模式与既有正式景观共用同一确定性几何算法。

/// 当前通用预设中一个部件的开发者描述与强类型资产句柄。
pub(super) struct GenericMeshPartLoad {
    descriptor: MeshPresetPart,         // 完整纯值在目录热更期间保持冻结。
    mesh: Handle<Mesh>,                 // 强句柄保证装配前资产不会被回收。
    material: Handle<StandardMaterial>, // GLTF 转换后的标准 PBR 材质。
}

/// 当前通用预设的一档资产输入。
pub(super) struct GenericMeshLodLoad {
    descriptor: MeshPresetLod,       // 距离范围和稳定档位身份。
    parts: Vec<GenericMeshPartLoad>, // 数量由开发者内容决定，不设产品上限。
}

/// 串行加载槽当前拥有的一个通用 Mesh 预设。
pub(super) struct GenericMeshProfileLoad {
    pub(super) preset_id: String, // 规范稳定 ID；别名只在目录解析边界存在。
    pub(super) catalog_revision: u64, // 完成值只代表请求时的原子目录修订。
    pub(super) prototype_id: VegetationPrototypeId, // 地图层和注册表共享的稳定身份。
    pub(super) lods: Vec<GenericMeshLodLoad>, // 任意数量完整档位。
    handles: Vec<UntypedHandle>,  // 统一失败/依赖就绪检查，不猜具体资产类型。
}

/// 一份通用风材质及其目录倍率；每帧只遍历实际生成的共享材质。
pub(super) struct GenericMeshWindBinding {
    pub(super) handle: Handle<TreeWindMaterial>,
    pub(super) lod: MeshWindLodLevel,
    pub(super) part: MeshWindPart,
    pub(super) strength_scale: f32,
    pub(super) frequency_scale: f32,
}

/// 完整装配成功、等待原子登记的通用原型与会话材质所有权。
pub(super) struct BuiltGenericMeshProfile {
    pub(super) prototype: VegetationMeshPrototype,
    pub(super) wind: Vec<GenericMeshWindBinding>,
    // 这些句柄承担完整结果的保活所有权；最后租约退出后drop，禁止按ID强删仍被旧渲染根引用的资产。
    pub(super) _billboards: Vec<Handle<VegetationMeshBillboardMaterial>>,
    pub(super) _generated_meshes: Vec<Handle<Mesh>>,
    pub(super) _generated_materials: Vec<Handle<StandardMaterial>>,
}

/// 请求一个目录中的纯数据 Mesh 预设；只向 AssetServer 发布轻量异步句柄。
pub(super) fn request_generic_mesh_profile(
    asset_server: &AssetServer,
    preset: &VegetationPresetDescriptor,
    catalog_revision: u64,
) -> Result<GenericMeshProfileLoad, String> {
    let VegetationPresetRuntime::Mesh { lods, .. } = &preset.runtime else {
        return Err(format!("植被预设{}不是通用数据Mesh", preset.id));
    };
    let prototype_id = VegetationPrototypeId(
        preset
            .resolved_prototype_id()
            .ok_or_else(|| format!("Mesh预设{}缺少正式原型ID", preset.id))?,
    );
    let mut handles = Vec::new(); // 资产数量由真实部件决定，不用固定数组或最大计数。
    let lods = lods
        .iter()
        .map(|lod| {
            let parts = lod
                .parts
                .iter()
                .map(|part| {
                    let mesh: Handle<Mesh> = asset_server.load(part.mesh_path.clone());
                    let material: Handle<StandardMaterial> =
                        asset_server.load(part.material_path.clone());
                    handles.push(mesh.clone().untyped());
                    handles.push(material.clone().untyped());
                    GenericMeshPartLoad {
                        descriptor: part.clone(),
                        mesh,
                        material,
                    }
                })
                .collect();
            GenericMeshLodLoad {
                descriptor: lod.clone(),
                parts,
            }
        })
        .collect();
    Ok(GenericMeshProfileLoad {
        preset_id: preset.id.clone(),
        catalog_revision,
        prototype_id,
        lods,
        handles,
    })
}

/// 当前通用预设是否有任一根资产或递归依赖明确失败。
pub(super) fn generic_mesh_profile_failed(
    asset_server: &AssetServer,
    load: &GenericMeshProfileLoad,
) -> bool {
    load.handles.iter().any(|handle| {
        asset_server
            .get_load_states(handle)
            .is_some_and(|(direct, dependencies, recursive)| {
                matches!(direct, LoadState::Failed(_))
                    || matches!(dependencies, DependencyLoadState::Failed(_))
                    || matches!(recursive, RecursiveDependencyLoadState::Failed(_))
            })
    })
}

/// 句柄释放前保留首个真实资产错误；失败回执必须包含路径和依赖原因，不能只报“加载失败”。
pub(super) fn generic_mesh_profile_failure_detail(
    asset_server: &AssetServer,
    load: &GenericMeshProfileLoad,
) -> Option<String> {
    load.handles.iter().find_map(|handle| {
        let (direct, dependencies, recursive) = asset_server.get_load_states(handle)?;
        let error = match (direct, dependencies, recursive) {
            (LoadState::Failed(error), _, _)
            | (_, DependencyLoadState::Failed(error), _)
            | (_, _, RecursiveDependencyLoadState::Failed(error)) => error,
            _ => return None,
        };
        let path = handle.path().map_or_else(|| format!("{:?}", handle.id()), ToString::to_string);
        Some(format!("{path}：{error}"))
    })
}

/// 当前通用预设全部 Mesh、材质及纹理依赖是否已经进入 Assets。
pub(super) fn generic_mesh_profile_loaded(
    asset_server: &AssetServer,
    load: &GenericMeshProfileLoad,
) -> bool {
    load.handles
        .iter()
        .all(|handle| asset_server.is_loaded_with_dependencies(handle))
}

/// 一个部件失败不能冒充整份预设结束；其他GLTF根仍在加载时继续占有共享single-flight槽。
pub(super) fn generic_mesh_profile_finished(
    asset_server: &AssetServer,
    load: &GenericMeshProfileLoad,
) -> bool {
    if generic_mesh_profile_loaded(asset_server, load) { return true; }
    load.handles.iter().all(|handle| {
        asset_server.is_loaded_with_dependencies(handle)
            || asset_server.get_load_states(handle).is_some_and(|(direct, dependencies, recursive)| {
                matches!(direct, LoadState::Failed(_))
                    || matches!(dependencies, DependencyLoadState::Failed(_))
                    || matches!(recursive, RecursiveDependencyLoadState::Failed(_))
            })
    })
}

fn mesh_aabb(meshes: &Assets<Mesh>, handle: &Handle<Mesh>, label: &str) -> Result<Aabb, String> {
    use bevy::camera::primitives::MeshAabb; // `compute_aabb` 由 MeshAabb trait 提供。

    meshes
        .get(handle)
        .ok_or_else(|| format!("{label} Mesh尚未进入Assets"))?
        .compute_aabb()
        .ok_or_else(|| format!("{label} Mesh缺少可计算AABB的位置属性"))
}

fn billboard_mode(mode: VegetationPresetBillboard) -> Option<VegetationMeshBillboard> {
    match mode {
        VegetationPresetBillboard::Fixed => Some(VegetationMeshBillboard::Fixed),
        VegetationPresetBillboard::Cylindrical => Some(VegetationMeshBillboard::Cylindrical {
            front_axis: VegetationMeshBillboardFrontAxis::NegativeZ,
        }),
        VegetationPresetBillboard::Spherical => Some(VegetationMeshBillboard::Spherical {
            front_axis: VegetationMeshBillboardFrontAxis::NegativeZ,
        }),
    } // 目录平面统一按 Bevy 负 Z 正面；未来需要其他轴时扩展数据字段而非猜资产名。
}

fn billboard_culling_aabb(source: Aabb) -> Aabb {
    let center = Vec3::from(source.center);
    let half = Vec3::from(source.half_extents);
    let radius = (center.abs() + half).length(); // 任意三维转向都落在以局部原点为中心的包围球内。
    Aabb::from_min_max(Vec3::splat(-radius), Vec3::splat(radius))
}

fn wind_lod(level: u8) -> MeshWindLodLevel {
    match level {
        0 => MeshWindLodLevel::High,
        1 => MeshWindLodLevel::Medium,
        _ => MeshWindLodLevel::Low,
    } // 任意更多远档继续复用低成本风分支，不限制档位数量。
}

/// 把目录部件语义转换成共享 Mesh 风材质的稳定语义。
fn wind_part(part: MeshPresetWindPart) -> MeshWindPart {
    match part {
        MeshPresetWindPart::Stem => MeshWindPart::Stem, // 树干禁用叶簇局部细摆。
        MeshPresetWindPart::Foliage => MeshWindPart::Foliage, // 树冠、灌木和花叶保留细摆。
    }
}

/// 在克隆的源材质上应用目录覆盖，未声明字段保持作者资产原值。
fn apply_material_overrides(
    material: &mut StandardMaterial,
    overrides: &MeshPresetMaterialOverrides,
) {
    if let Some(alpha_mode) = overrides.alpha_mode {
        material.alpha_mode = match alpha_mode {
            MeshPresetAlphaMode::Opaque => AlphaMode::Opaque, // 强制关闭源透明模式。
            MeshPresetAlphaMode::Mask { cutoff } => AlphaMode::Mask(cutoff), // 薄片植被使用可编辑裁剪阈值。
            MeshPresetAlphaMode::Blend => AlphaMode::Blend, // 显式启用常规透明混合。
        };
    }
    if let Some(double_sided) = overrides.double_sided {
        material.double_sided = double_sided; // 同步标准材质正反面光照语义。
        material.cull_mode = (!double_sided).then_some(Face::Back); // `false`同样是强制覆盖，不能残留源材质的双面剔除状态。
    }
}

/// 根据目录声明直接复用或确定性生成一个当前会话 Mesh。
fn resolve_runtime_mesh(
    descriptor: &MeshPresetPart,
    source: &Handle<Mesh>,
    meshes: &mut Assets<Mesh>,
    generated: &mut Vec<Handle<Mesh>>,
    label: &str,
) -> Result<Handle<Mesh>, String> {
    let derived = match &descriptor.mesh_derivation {
        MeshPresetMeshDerivation::Direct => return Ok(source.clone()), // 常规档位保持零拷贝强句柄。
        MeshPresetMeshDerivation::DispersedTriangles { max_triangles } => {
            let source_mesh = meshes
                .get(source)
                .ok_or_else(|| format!("{label}源Mesh尚未进入Assets"))?;
            build_shrub_ultra_proxy(source_mesh, *max_triangles)? // 保留真实薄片属性和空间轮廓。
        }
        MeshPresetMeshDerivation::ConvexHull { max_sample_points } => {
            let source_mesh = meshes
                .get(source)
                .ok_or_else(|| format!("{label}源Mesh尚未进入Assets"))?;
            build_rock_ultra_proxy(source_mesh, *max_sample_points)? // 生成封闭超远岩石代理。
        }
    };
    let handle = meshes.add(derived); // 派生网格只属于当前动态地图会话。
    generated.push(handle.clone()); // 成功替换或失败回滚都由调用者精确释放。
    Ok(handle)
}

/// 把一个已完整加载的目录预设装配成通用原型；失败会回收本次生成的扩展材质。
pub(super) fn build_generic_mesh_profile(
    load: &GenericMeshProfileLoad,
    meshes: &mut Assets<Mesh>,
    standard_materials: &mut Assets<StandardMaterial>,
    wind_materials: &mut Assets<TreeWindMaterial>,
    billboard_materials: &mut Assets<VegetationMeshBillboardMaterial>,
    noise: &TreeWindNoiseTexture,
) -> Result<BuiltGenericMeshProfile, String> {
    let mut wind = Vec::new();
    let mut billboards = Vec::new();
    let mut generated_meshes = Vec::new();
    let mut generated_materials = Vec::new();
    let result = (|| {
        let lods = load
            .lods
            .iter()
            .map(|lod| {
                let parts = lod
                    .parts
                    .iter()
                    .map(|part| {
                        let label = format!(
                            "预设{} LOD{} 部件{}",
                            load.preset_id, lod.descriptor.level, part.descriptor.part_id
                        );
                        let runtime_mesh = resolve_runtime_mesh(
                            &part.descriptor,
                            &part.mesh,
                            meshes,
                            &mut generated_meshes,
                            &label,
                        )?;
                        let source_aabb = mesh_aabb(meshes, &runtime_mesh, &label)?;
                        let mut base = standard_materials
                            .get(&part.material)
                            .cloned()
                            .ok_or_else(|| format!("{label} StandardMaterial尚未进入Assets"))?;
                        apply_material_overrides(&mut base, &part.descriptor.material_overrides);
                        let billboard = billboard_mode(part.descriptor.billboard);
                        let dynamic_billboard = billboard.is_some_and(|mode| mode.requires_camera());
                        if dynamic_billboard && part.descriptor.wind.is_some() {
                            return Err(format!(
                                "{label}不能同时启用动态公告板与高度权重风；二者都需要唯一顶点扩展管线"
                            ));
                        }
                        let (material, culling_aabb, batch_vertex_mode, wind_metadata) =
                            if let Some(spec) = part.descriptor.wind.as_ref() {
                                let lod_level = wind_lod(lod.descriptor.level);
                                let part_role = wind_part(spec.part);
                                let handle = wind_materials.add(create_tree_wind_material(
                                    base,
                                    source_aabb,
                                    noise,
                                    lod_level,
                                    part_role,
                                ));
                                wind.push(GenericMeshWindBinding {
                                    handle: handle.clone(),
                                    lod: lod_level,
                                    part: part_role,
                                    strength_scale: spec.strength,
                                    frequency_scale: spec.frequency,
                                });
                                (
                                    handle.into(),
                                    expanded_tree_wind_culling_aabb(source_aabb),
                                    VegetationMeshBatchVertexMode::Wind,
                                    Some(VegetationHeightWeightedWind {
                                        root_height: spec.root_height,
                                        tip_height: spec.tip_height,
                                        strength: spec.strength,
                                        frequency: spec.frequency,
                                    }),
                                )
                            } else if let Some(mode) = billboard.filter(|mode| mode.requires_camera())
                            {
                                let handle = billboard_materials.add(
                                    create_vegetation_mesh_billboard_material(base, mode),
                                );
                                billboards.push(handle.clone());
                                (
                                    handle.into(),
                                    billboard_culling_aabb(source_aabb),
                                    VegetationMeshBatchVertexMode::Billboard,
                                    None,
                                )
                            } else {
                                let material = if part.descriptor.material_overrides
                                    == MeshPresetMaterialOverrides::default()
                                {
                                    part.material.clone() // 无覆盖时继续复用 GLTF 原材质句柄。
                                } else {
                                    let handle = standard_materials.add(base); // 静态覆盖材质由当前预设会话拥有。
                                    generated_materials.push(handle.clone()); // 热替换和离场精确回收。
                                    handle
                                };
                                (
                                    material.into(),
                                    source_aabb,
                                    VegetationMeshBatchVertexMode::Static,
                                    None,
                                )
                            };
                        Ok(VegetationMeshPart {
                            part_id: part.descriptor.part_id,
                            mesh: runtime_mesh,
                            material,
                            local_transform: Transform {
                                translation: Vec3::from_array(part.descriptor.transform.translation),
                                rotation: Quat::from_rotation_y(
                                    part.descriptor.transform.yaw_degrees.to_radians(),
                                ),
                                scale: Vec3::from_array(part.descriptor.transform.scale),
                            },
                            culling_aabb: Some(culling_aabb),
                            casts_shadow: part.descriptor.casts_shadow,
                            billboard,
                            wind: wind_metadata,
                            batch_vertex_mode,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(VegetationMeshLod {
                    level: VegetationMeshLodLevel(lod.descriptor.level),
                    visibility_range: VisibilityRange {
                        start_margin: lod.descriptor.start[0]..lod.descriptor.start[1],
                        end_margin: lod.descriptor.end[0]..lod.descriptor.end[1],
                        use_aabb: false, // 全部档位按稳定实例根测距，部件局部偏移不改变 LOD 身份。
                    },
                    parts,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(VegetationMeshPrototype {
            id: load.prototype_id,
            lods,
        })
    })();
    match result {
        Ok(prototype) => Ok(BuiltGenericMeshProfile {
            prototype,
            wind,
            _billboards: billboards,
            _generated_meshes: generated_meshes,
            _generated_materials: generated_materials,
        }),
        Err(error) => {
            for binding in wind {
                wind_materials.remove(binding.handle.id());
            }
            for handle in billboards {
                billboard_materials.remove(handle.id());
            }
            for handle in generated_meshes {
                meshes.remove(handle.id());
            }
            for handle in generated_materials {
                standard_materials.remove(handle.id());
            }
            Err(error)
        }
    }
}
