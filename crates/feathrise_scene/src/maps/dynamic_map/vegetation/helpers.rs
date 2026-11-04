//! 动态植被作者数据校验、资产状态与数值纹理解码。

use std::collections::{BTreeMap, BTreeSet, HashSet}; // 校验身份，并按纹理页打包预设、去重当前修订的 Blade 网格。

use bevy::asset::{
    Asset, AssetEvent, AssetId, DependencyLoadState, LoadState, RecursiveDependencyLoadState,
};
use bevy::image::{ImageLoaderSettings, ImageSampler};
use bevy::mesh::VertexAttributeValues; // 在发布窗口前核对顶点阶段硬性要求的 position、normal 与 UV0 格式。
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use feathrise_environment::vegetation::blade::{
    GpuBladeProfileBank, GpuBladeProfileBankId, GpuBladeProfileSlot, GpuBladeProfiles,
    ensure_vertical_blade_uv,
};
use feathrise_map_content::{
    MAP_SCHEMA_VERSION, MapManifestAsset, MapVegetationAsset, MapVegetationKind,
    VegetationPresetDescriptor, VegetationPresetRegistry,
}; // GPU 四槽是批次内 ABI，目录数量通过任意批次 Vec 承接。

use super::resources::{DynamicVegetationLayerAssets, DynamicVegetationPending};
use crate::gpu_grass_content::{RuntimeBladeProfileAssets, load_blade_preset}; // 完整数据 Blade 从目录解析成同一四级运行时规格。

const MAX_DYNAMIC_TERRAIN_DIMENSION: u32 = 2_049;

pub(super) fn event_asset_id<A: Asset>(event: &AssetEvent<A>) -> AssetId<A> {
    match event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::LoadedWithDependencies { id }
        | AssetEvent::Unused { id } => *id,
    }
}

pub(super) fn event_requests_rebuild<A: Asset>(event: &AssetEvent<A>, failed: bool) -> bool {
    matches!(
        event,
        AssetEvent::Modified { .. } | AssetEvent::Removed { .. }
    ) || (failed
        && matches!(
            event,
            AssetEvent::Added { .. } | AssetEvent::LoadedWithDependencies { .. }
        ))
}

/// Blade 网格顶点布局检查后的 UV 准备状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DynamicBladeUvState {
    Ready,   // UV0 已是与顶点数一致的 Float32x2，可直接专用化 GPU 管线。
    Missing, // 其他必需属性合法，只需按纵向范围补齐缺失的 UV0。
}

/// 在 RenderApp 看到窗口之前验证 GPU Blade 顶点 ABI，防止错误布局下沉为 wgpu 致命校验错误。
fn inspect_dynamic_blade_mesh(mesh: &Mesh) -> Result<DynamicBladeUvState, String> {
    let vertex_count = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(values)) => values.len(), // WGSL location 0 固定读取三维局部位置。
        Some(_) => return Err("POSITION不是Float32x3".to_owned()), // 格式不同时顶点管线无法匹配。
        None => return Err("缺少POSITION".to_owned()), // 没有位置无法生成叶片形变或根尖 UV。
    };
    match mesh.attribute(Mesh::ATTRIBUTE_NORMAL) {
        Some(VertexAttributeValues::Float32x3(values)) if values.len() == vertex_count => {} // WGSL location 1 与位置顶点一一对应。
        Some(VertexAttributeValues::Float32x3(_)) => {
            return Err("NORMAL数量与POSITION不一致".to_owned()); // 不完整顶点流不得进入 GPU。
        }
        Some(_) => return Err("NORMAL不是Float32x3".to_owned()), // 管线 location 1 要求三维浮点法线。
        None => return Err("缺少NORMAL".to_owned()), // 不猜测任意开发者网格的叶片宽轴与光照法线。
    }
    match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
        Some(VertexAttributeValues::Float32x2(values)) if values.len() == vertex_count => {
            Ok(DynamicBladeUvState::Ready) // 正式图集 UV 与根尖比例都由资产自身提供。
        }
        Some(VertexAttributeValues::Float32x2(_)) => {
            Err("UV0数量与POSITION不一致".to_owned()) // 避免顶点索引读取越界属性。
        }
        Some(_) => Err("UV0不是Float32x2".to_owned()), // WGSL location 2 与图形管线都要求二维浮点。
        None => Ok(DynamicBladeUvState::Missing),      // 旧 HN-02 叶片允许在下方准备门自动补齐。
    }
}

/// Mesh 热更保留同一强句柄；合法 Modified 由 RenderAsset 原位上传，不拆掉整棵植被根。
pub(super) fn blade_mesh_event_requests_rebuild(
    event: &AssetEvent<Mesh>,    // 已确认属于当前修订某一 Blade LOD 的事件。
    current_mesh: Option<&Mesh>, // Modified 后 Assets<Mesh> 中的当前主世界内容。
    failed: bool,                // 失败修订允许 Added/LoadedWithDependencies 驱动恢复。
) -> bool {
    match event {
        AssetEvent::Modified { .. } => current_mesh
            .is_none_or(|mesh| inspect_dynamic_blade_mesh(mesh) != Ok(DynamicBladeUvState::Ready)), // 内部补 UV 产生的合法 Modified 不反复重建。
        AssetEvent::Removed { .. } => true, // 强句柄内容消失后必须进入事务失败或恢复。
        AssetEvent::Added { .. } | AssetEvent::LoadedWithDependencies { .. } => failed, // 只在上一次失败时重试新资产。
        AssetEvent::Unused { .. } => false, // 引用计数诊断不代表网格内容改变。
    }
}

/// 判断Image事件是否真的需要重建动态植被根；Blade基础密度修改由RenderAsset原位上传即可。
pub(super) fn image_event_requests_rebuild(
    event: &AssetEvent<Image>, // 当前已经确认属于高度或某一正式密度句柄的事件。
    cpu_dependent: bool,       // 高度与Mesh密度需要重新生成CPU实例；纯Blade密度不需要。
    failed: bool,              // 失败修订允许新增或依赖完成事件触发恢复。
) -> bool {
    match event {
        AssetEvent::Modified { .. } => cpu_dependent, // Blade Shader直接从同一Image句柄读取新基础Mip，无需销毁十二个窗口。
        AssetEvent::Removed { .. } => true,           // 句柄内容消失后必须进入正式失败/恢复事务。
        AssetEvent::Added { .. } | AssetEvent::LoadedWithDependencies { .. } => failed, // 只为失败后的修复重试。
        AssetEvent::Unused { .. } => false, // 引用计数诊断不代表像素内容发生变化。
    }
}

fn safe_asset_path(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.contains("..") && !path.contains('\\')
}

/// 损坏定义整份拒绝，禁止只生成部分层后把作者错误伪装为成功。
pub(super) fn validate_vegetation_definition(
    vegetation: &MapVegetationAsset,
    manifest: &MapManifestAsset,
    presets: &VegetationPresetRegistry, // 全部运行时身份和后端类型由当前原子目录快照解析。
) -> Result<(), String> {
    if vegetation.schema_version != MAP_SCHEMA_VERSION {
        return Err(format!("植被格式版本{}不受支持", vegetation.schema_version));
    }
    if vegetation.map_id != manifest.id {
        return Err("植被文件与地图清单所属ID不一致".to_owned());
    }
    // 真正空容器的占位地形分辨率是[0,0]；零层定义没有任何高度采样依赖。
    // 显式对象集合仍须存在，非空层和所有真实地形继续经过原尺寸/量程校验。
    let empty_container = manifest.terrain.is_empty()
        && manifest.terrains.is_some()
        && vegetation.layers.is_empty();
    if manifest.world_bounds().is_none()
        || !manifest.terrain.base_height.is_finite()
        || !manifest.terrain.height_scale.is_finite()
        || manifest.terrain.height_scale <= 0.0
        || (!empty_container && (manifest.terrain.resolution[0] < 2
            || manifest.terrain.resolution[1] < 2))
        || (!matches!(manifest.terrain.kind, feathrise_map_content::MapTerrainKind::SparseClipmap | feathrise_map_content::MapTerrainKind::Voxel) // 两种窗口后端不分配整图。
            && (manifest.terrain.resolution[0] > MAX_DYNAMIC_TERRAIN_DIMENSION // 保留GLB预算。
                || manifest.terrain.resolution[1] > MAX_DYNAMIC_TERRAIN_DIMENSION))
    // Clipmap规格另行校验。
    {
        return Err("植被依赖的地图范围、高度解码或地形分辨率无效".to_owned());
    }
    let mut ids = BTreeSet::new();
    let mut runtime_layer_ids = BTreeSet::new();
    for layer in &vegetation.layers {
        if layer.id.trim().is_empty() || !ids.insert(layer.id.as_str()) {
            return Err(format!("植被层包含空ID或重复ID：{}", layer.id));
        }
        if !safe_asset_path(&layer.density_source_path)
            || !safe_asset_path(&layer.density_texture_path)
            || !layer
                .density_texture_path
                .to_ascii_lowercase()
                .ends_with(".ktx2")
        {
            return Err(format!("植被层{}的模型或密度路径无效", layer.id));
        }
        let preset = resolve_layer_preset(layer, presets).ok_or_else(|| {
            format!(
                "植被层{}没有可解析的开发者预设{}",
                layer.id,
                layer.runtime_profile.as_deref().unwrap_or("<缺失预设ID>")
            )
        })?; // 未知稳定 ID 不再退回普通 GLB 或硬编码按钮。
        if preset.kind() != layer.kind {
            return Err(format!(
                "植被层{}的后端{:?}与预设{}不一致",
                layer.id, layer.kind, preset.id
            )); // Blade 与 Mesh 运行时链路不可隐式互换。
        }
        let runtime_layer_id = preset.resolved_runtime_layer_id(); // 省略字段的旧地图也从稳定预设身份恢复。
        if layer
            .runtime_layer_id
            .is_some_and(|stored| stored != runtime_layer_id)
        {
            return Err(format!(
                "植被层{}保存的运行时层ID与预设{}不一致",
                layer.id, preset.id
            )); // 同一稳定预设不能被地图文件改写成另一竞争层身份。
        }
        if !runtime_layer_ids.insert(runtime_layer_id) {
            return Err(format!("植被层{}的正式运行时层ID重复", layer.id));
        }
        match layer.kind {
            MapVegetationKind::Blade => {
                // 完整数据 Blade 已由目录类型校验，无额外固定名单。
            }
            MapVegetationKind::Mesh => {
                let prototype_id = preset
                    .resolved_prototype_id()
                    .expect("Mesh 预设目录校验后必须派生原型 ID"); // 类型已在上方匹配。
                if layer
                    .prototype_id
                    .is_some_and(|stored| stored != prototype_id)
                {
                    return Err(format!(
                        "植被层{}保存的原型ID与预设{}不一致",
                        layer.id, preset.id
                    ));
                }
            }
        }
        if layer.resolution[0] == 0
            || layer.resolution[1] == 0
            || !layer.resolution[0].is_power_of_two()
            || !layer.resolution[1].is_power_of_two()
        {
            return Err(format!("植被层{}密度尺寸必须是非零二次幂", layer.id));
        }
        let texels = u64::from(layer.resolution[0])
            .checked_mul(u64::from(layer.resolution[1]))
            .ok_or_else(|| format!("植被层{}密度尺寸乘法溢出", layer.id))?; // 只拒绝平台无法表达的真实溢出。
        usize::try_from(texels)
            .map_err(|_| format!("植被层{}密度尺寸超出当前平台寻址能力", layer.id))?; // 不设置人为 MiB 预算。
        if !layer.spacing_meters.is_finite()
            || layer.spacing_meters <= 0.0
            || !layer.scale_range.iter().all(|value| value.is_finite())
            || layer.scale_range[0] <= 0.0
            || layer.scale_range[1] < layer.scale_range[0]
        {
            return Err(format!("植被层{}的间距或缩放无效", layer.id));
        }
    }
    Ok(())
}

/// 只通过地图保存的稳定 ID 或历史别名解析当前规范预设。
fn resolve_layer_preset<'a>(
    layer: &feathrise_map_content::MapVegetationLayer, // 当前地图保存的层描述。
    presets: &'a VegetationPresetRegistry,               // 最近一次完整校验通过的开发者目录。
) -> Option<&'a VegetationPresetDescriptor> {
    let id = layer.runtime_profile.as_deref()?; // 第二版目录不再从作者资产路径猜测后端身份。
    presets.resolve(id) // 当前 ID 与历史别名统一返回规范项。
}

/// 数值纹理统一禁止sRGB并使用最近采样，保证密度和四角高度不被颜色处理污染。
pub(super) fn load_numeric_image(asset_server: &AssetServer, path: String) -> Handle<Image> {
    asset_server
        .load_builder()
        .with_settings(|settings: &mut ImageLoaderSettings| {
            settings.is_srgb = false;
            settings.sampler = ImageSampler::nearest();
        })
        .load(path)
}

/// 从完整作者定义建立强句柄集合；同一修订内不再重复请求AssetServer。
pub(super) fn load_pending_assets(
    asset_server: &AssetServer,
    vegetation: MapVegetationAsset,
    manifest: MapManifestAsset,
    presets: &VegetationPresetRegistry, // 使用与定义校验完全相同的原子目录快照。
    height_cells: Option<Handle<Image>>, // 直接接收GLB派生内存RGBA16 Image强句柄。
    heights: std::sync::Arc<[u16]>,     // 直接接收同代次CPU高度Arc。
    terrain_generation: u64,            // 保持CPU/GPU派生身份一致。
    generation: u32,
    revision: u32,
    draft_snapshot: Option<&super::DynamicBladeDraftSnapshot>, // Blade与Mesh草稿图片及CPU源来自同一完成值。
) -> Result<DynamicVegetationPending, String> {
    let mut canonical_layers = Vec::with_capacity(vegetation.layers.len()); // 层数量由作者文件决定，不做固定上限预分配。
    let mut blade_presets = BTreeMap::<String, RuntimeBladeProfileAssets>::new(); // 同一预设被多层引用时只请求一组四级网格和纹理。
    for mut definition in vegetation.layers.iter().cloned() {
        let preset = resolve_layer_preset(&definition, presets)
            .ok_or_else(|| format!("植被层{}无法从当前开发者目录解析预设", definition.id))?; // 校验后目录通常不会变化，仍为热更竞态保留明确错误。
        definition.runtime_profile = Some(preset.id.clone()); // 历史别名在当前修订内规范化。
        definition.runtime_layer_id = Some(preset.resolved_runtime_layer_id()); // 缺失数字身份由稳定预设 ID 恢复。
        definition.prototype_id = preset.resolved_prototype_id(); // Blade 清空无意义原型，Mesh 写入规范原型身份。
        definition.asset_path = preset.author_asset_path().to_owned(); // 作者追踪路径随当前规范预设更新。
        if definition.kind == MapVegetationKind::Blade && !blade_presets.contains_key(&preset.id) {
            let assets = load_blade_preset(asset_server, preset)?; // 完整数据 Blade 统一请求四档资产。
            blade_presets.insert(preset.id.clone(), assets); // 后续同预设层复用同一强句柄集合。
        }
        let density = if let Some(snapshot) = draft_snapshot {
            // 新Blade与Mesh层都不得请求尚未保存的KTX2路径。
            snapshot
                .densities
                .get(&definition.id)
                .cloned()
                .ok_or_else(|| format!("植被层{}缺少完整草稿密度", definition.id))? // 缺层拒绝整批。
        } else {
            load_numeric_image(asset_server, definition.density_texture_path.clone()) // 正式磁盘层继续异步加载KTX2。
        };
        canonical_layers.push((definition, density)); // 批次打包完成后再补齐会话 bank/slot。
    }
    let (blade_profile_banks, blade_bindings) = pack_blade_profile_banks(&blade_presets)?; // 同纹理页每四项一批，批次数不限。
    let mut layers = Vec::with_capacity(canonical_layers.len()); // 最终层顺序严格保持地图作者顺序。
    for (definition, density) in canonical_layers {
        if definition.kind == MapVegetationKind::Blade {
            let preset_id = definition
                .runtime_profile
                .as_deref()
                .expect("Blade 层已经规范化稳定预设 ID");
            let assets = blade_presets
                .get(preset_id)
                .expect("批次打包必须保留每个 Blade 预设资产");
            let (bank_id, slot) = *blade_bindings
                .get(preset_id)
                .expect("每个 Blade 预设必须获得会话 bank/slot");
            let mut profile = assets.spec.clone(); // 每层保存拥有型窗口规格，后续目录热更不悬空借用。
            profile.vegetation_config.profile_slot = slot; // 紧凑实例只写当前批次两位局部槽。
            layers.push(DynamicVegetationLayerAssets {
                source_density: draft_snapshot
                    .map(|snapshot| {
                        snapshot
                            .sources
                            .get(&definition.id)
                            .cloned()
                            .ok_or_else(|| format!("植被层{}缺少草稿CPU源", definition.id))
                    })
                    .transpose()?, // 缺少同代源拒绝整批。
                in_memory_density: draft_snapshot.is_some(), // 草稿句柄通过Assets就绪门验收。
                definition,                                  // 地图稳定 ID 与运行时身份已经规范化。
                density,                                     // 当前层正式概率图强句柄。
                blade_profile: Some(profile),                // 四级窗口纯值完整保留。
                blade_meshes: Some(assets.meshes.clone()), // 四档网格强句柄复用 AssetServer 缓存。
                blade_profile_bank: Some(bank_id),         // 只用于当前修订渲染绑定选择。
            });
        } else {
            layers.push(DynamicVegetationLayerAssets {
                source_density: draft_snapshot
                    .map(|snapshot| {
                        snapshot
                            .sources
                            .get(&definition.id)
                            .cloned()
                            .ok_or_else(|| format!("植被层{}缺少草稿CPU源", definition.id))
                    })
                    .transpose()?, // Mesh分块直接共享worker冻结的R8，不在帧线程复制。
                in_memory_density: draft_snapshot.is_some(), // 未保存Mesh层不等待磁盘KTX2。
                definition,                                  // Mesh 层也已规范化目录身份和原型 ID。
                density,                                     // CPU 分块散布随后按需冻结基础级。
                blade_profile: None,                         // Mesh 不创建 GPU 叶片窗口。
                blade_meshes: None,                          // Mesh 原型走独立通用注册链。
                blade_profile_bank: None,                    // Mesh 不参与 Blade group3 绑定。
            });
        }
    }
    let required_mesh_profiles = draft_snapshot.map_or_else(BTreeSet::new, |snapshot| {
        layers
            .iter()
            .filter(|layer| {
                layer.definition.kind == MapVegetationKind::Mesh
                    && snapshot.nonzero_layers.contains(&layer.definition.id)
            })
            .filter_map(|layer| layer.definition.runtime_profile.clone())
            .collect()
    }); // 草稿非零身份已在worker扫描完成；磁盘初载仍沿用一次基础级检查。
    Ok(DynamicVegetationPending {
        map_id: vegetation.map_id.clone(),
        manifest,
        clipmap_height: None, // 调用方补分页快照。
        height_cells,
        heights,
        terrain_generation,
        layers,
        blade_profile_banks, // 与上述会话 bank ID 使用同一确定性数组。
        generation,
        revision,
        mesh_profiles_scanned: draft_snapshot.is_some(),
        required_mesh_profiles,
        mesh_profiles_requested: false,
    })
}

/// 把当前地图实际引用的 Blade 预设按纹理页、每批最多四项确定性打包。
fn pack_blade_profile_banks(
    presets: &BTreeMap<String, RuntimeBladeProfileAssets>, // 规范 ID 已排序且每项唯一。
) -> Result<
    (
        Vec<GpuBladeProfileBank>, // 主世界提交时原子替换的任意批次数组。
        BTreeMap<String, (GpuBladeProfileBankId, GpuBladeProfileSlot)>, // 每个稳定预设的会话映射。
    ),
    String,
> {
    let mut texture_groups = BTreeMap::<Option<String>, Vec<&str>>::new(); // None 白纹理与每个显式纹理页分别成组。
    for (preset_id, assets) in presets {
        texture_groups
            .entry(assets.base_color_texture_path.clone())
            .or_default()
            .push(preset_id.as_str()); // BTreeMap 输入保证组内规范 ID 顺序稳定。
    }
    let mut banks = Vec::new(); // 实际地图没引用 Blade 时保持空目录。
    let mut bindings = BTreeMap::new(); // 地图层随后只做对数查询，不扫描任意批次数组。
    for preset_ids in texture_groups.into_values() {
        for chunk in preset_ids
            .chunks(feathrise_environment::vegetation::blade::GPU_GRASS_PROFILE_CAPACITY)
        {
            let bank_id = GpuBladeProfileBankId::for_dynamic_index(banks.len())
                .ok_or_else(|| "Blade 批次数超过当前会话 u32 身份可表达范围".to_owned())?; // 只拒绝真实整数表示溢出，不设产品数量上限。
            let mut profiles = GpuBladeProfiles::default(); // 不足四项的尾批使用中性安全外观填充未引用槽。
            let texture = chunk
                .first()
                .and_then(|preset_id| presets.get(*preset_id))
                .and_then(|assets| assets.base_color_texture.clone()); // 同组全部项共享相同规范纹理路径。
            for (slot_index, preset_id) in chunk.iter().enumerate() {
                let slot = GpuBladeProfileSlot::new(slot_index as u32)
                    .expect("chunks 固定容量保证局部槽位于零至三");
                let assets = presets
                    .get(*preset_id)
                    .expect("分组规范 ID 必须仍存在于预设资产表");
                profiles.set(slot, assets.appearance); // 每批仅上传四个内容常量，不复制到逐实例记录。
                bindings.insert((*preset_id).to_owned(), (bank_id, slot)); // 地图 JSON 永远不持久化该会话映射。
            }
            banks.push(GpuBladeProfileBank::new(profiles, texture)); // 纹理页相同才共享一个 group3 绑定组。
        }
    }
    Ok((banks, bindings)) // 数量只受当前真实内存与设备资源创建结果约束。
}

pub(super) fn handle_failed<A: Asset>(asset_server: &AssetServer, handle: &Handle<A>) -> bool {
    asset_server
        .get_load_states(handle.id())
        .is_some_and(|(direct, dependencies, recursive)| {
            matches!(direct, LoadState::Failed(_))
                || matches!(dependencies, DependencyLoadState::Failed(_))
                || matches!(recursive, RecursiveDependencyLoadState::Failed(_))
        })
}

pub(super) fn pending_asset_failure(
    asset_server: &AssetServer,
    pending: &DynamicVegetationPending,
) -> Option<String> {
    for layer in &pending.layers {
        if !layer.in_memory_density && handle_failed(asset_server, &layer.density) {
            return Some(format!("植被层{}密度KTX2加载失败", layer.definition.id));
        }
        if layer.blade_meshes.as_ref().is_some_and(|handles| {
            handles
                .iter()
                .any(|handle| handle_failed(asset_server, handle))
        }) {
            return Some(format!("植被层{}模型加载失败", layer.definition.id));
        }
    }
    for (bank_index, bank) in pending.blade_profile_banks.iter().enumerate() {
        if bank
            .blade_base_color
            .as_ref()
            .is_some_and(|handle| handle_failed(asset_server, handle))
        {
            return Some(format!("Blade批次{bank_index}综合色纹理加载失败")); // 显式纹理失败时不以白色回退伪装成功。
        }
    }
    None
}

pub(super) fn pending_assets_loaded(
    asset_server: &AssetServer,
    pending: &DynamicVegetationPending,
    images: &Assets<Image>, // 内存图片不具有AssetServer路径，须查询真实资产存在性。
) -> bool {
    pending.layers.iter().all(|layer| {
        (if layer.in_memory_density {
            images.contains(layer.density.id()) // 直接登记的图片不具有文件加载状态。
        } else {
            asset_server.is_loaded_with_dependencies(layer.density.id()) // 保留磁盘依赖完成门。
        }) && layer.blade_meshes.as_ref().is_none_or(|handles| {
            handles
                .iter()
                .all(|handle| asset_server.is_loaded_with_dependencies(handle.id()))
        })
    }) && pending.blade_profile_banks.iter().all(|bank| {
        bank.blade_base_color
            .as_ref()
            .is_none_or(|handle| asset_server.is_loaded_with_dependencies(handle.id()))
    }) // 当前修订显式图集也必须就绪，避免可见窗口先闪白再恢复。
}

/// 为当前修订的唯一 Blade 网格执行一次发布前准备；缺 UV 允许补齐，其他 ABI 错误整份拒绝。
pub(super) fn prepare_pending_blade_meshes(
    meshes: &mut Assets<Mesh>, // 只在强资产已加载后读取，且仅对真正缺 UV0 的网格取可写引用。
    pending: &DynamicVegetationPending, // 资产范围仅限当前地图实际引用的预设与 LOD。
) -> Result<Option<usize>, String> {
    let mut seen = HashSet::new(); // 第四档复用低模、多层复用同预设时只检查一次真实资产。
    let mut unique_meshes = Vec::new(); // 一次加载路径按实际使用量保留句柄与可读诊断标签。
    for layer in &pending.layers {
        let Some(handles) = layer.blade_meshes.as_ref() else {
            continue; // Mesh 后端没有 GPU Blade 顶点 ABI。
        };
        for (lod_index, handle) in handles.iter().enumerate() {
            if seen.insert(handle.id()) {
                unique_meshes.push((
                    handle.clone(),
                    format!("植被层{} LOD{lod_index}", layer.definition.id),
                )); // 首个引用者作为稳定资产错误上下文。
            }
        }
    }
    if unique_meshes
        .iter()
        .any(|(handle, _)| meshes.get(handle).is_none())
    {
        return Ok(None); // AssetServer 已报依赖完成与 Assets<Mesh> 可见之间的短暂交接不记为失败。
    }
    let mut missing_uv = Vec::new(); // 先完整校验全部网格，再进行幂等的兼容修改。
    for (handle, label) in &unique_meshes {
        let mesh = meshes
            .get(handle)
            .expect("上方原子就绪检查后网格必须仍存在");
        match inspect_dynamic_blade_mesh(mesh)
            .map_err(|error| format!("{label}顶点布局无效：{error}"))?
        {
            DynamicBladeUvState::Ready => {} // 开发者资产已满足正式图集与根尖坐标契约。
            DynamicBladeUvState::Missing => missing_uv.push((handle.clone(), label.clone())), // 旧叶片稍后按真实 Y 范围补齐。
        }
    }
    for (handle, label) in &missing_uv {
        let mut mesh = meshes
            .get_mut(handle)
            .expect("同一系统内已就绪的网格不得在准备途中消失");
        ensure_vertical_blade_uv(&mut mesh)
            .map_err(|error| format!("{label}无法自动补齐根尖UV：{error}"))?; // 只补缺失 UV，不改模型位置、法线、索引或图集数据。
        if inspect_dynamic_blade_mesh(&mesh) != Ok(DynamicBladeUvState::Ready) {
            return Err(format!("{label}补齐UV后仍不满足GPU Blade顶点布局")); // 进入 ECS 窗口前执行最后防御核对。
        }
    }
    Ok(Some(missing_uv.len())) // Some 表示全部唯一网格均已完整准备。
}

/// 取得R8基础Mip副本；后台作业不再持有或访问主世界Image资产。
pub(super) fn copy_r8_base(
    image: &Image,
    resolution: [u32; 2],
    label: &str,
) -> Result<Vec<u8>, String> {
    let expected = validate_r8_base(image, resolution, label)?;
    let data = image
        .data
        .as_ref()
        .expect("数值纹理CPU像素已由校验确认存在");
    Ok(data[..expected].to_vec())
}

/// 只核对GPU Blade层的基础级格式，不为无需CPU散布的层复制整张密度图。
pub(super) fn validate_r8_base(
    image: &Image,
    resolution: [u32; 2],
    label: &str,
) -> Result<usize, String> {
    let descriptor = &image.texture_descriptor;
    if descriptor.format != TextureFormat::R8Unorm
        || descriptor.size.width != resolution[0]
        || descriptor.size.height != resolution[1]
        || descriptor.size.depth_or_array_layers != 1
    {
        return Err(format!("{label}不是声明尺寸的二维R8_UNORM纹理"));
    }
    let expected = resolution[0] as usize * resolution[1] as usize;
    let data = image
        .data
        .as_ref()
        .ok_or_else(|| format!("{label}没有可供后台散布读取的CPU像素"))?;
    if data.len() < expected {
        return Err(format!("{label}基础Mip字节不足"));
    }
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use bevy::asset::RenderAssetUsages; // 纯 CPU 顶点契约测试使用最小可渲染网格。
    use bevy::mesh::PrimitiveTopology; // 单三角形足以覆盖 position、normal 与 UV0 布局。

    use super::*;

    #[test] // 分页地图不应继承GLB整图内存上限。
    fn maximum_clipmap_empty_vegetation_passes_without_dense_height_memory() {
        // 最大规格验证无像素分配。
        let mut manifest: MapManifestAsset =
            serde_json::from_value(serde_json::json!({ // 使用正常资产协议。
                "schema_version": 1, "id": "MAP-large-clipmap-grass", "display_name": "最大分页草地", // 独立测试图。
                "runtime_scene": "DynamicMapScene", "world_min": [0,0], "world_max": [8192,8192], // 最大世界范围。
                "default_spawn": [1,0,1], // 有效出生点。
                "terrain": {"kind":"heightfield", "glb_path":"maps/test/terrain.glb", "resolution":[65537,65537], "base_height":-1024, "height_scale":2048} // 解析后切换后端。
            }))
            .unwrap(); // 只反序列化，不分配高度。
        let vegetation = MapVegetationAsset {
            schema_version: MAP_SCHEMA_VERSION,
            map_id: manifest.id.clone(),
            layers: Vec::new(),
            authoring: None,
        }; // 空作者定义。
        let presets = VegetationPresetRegistry {
            // 沿用正式加载门的合法空目录夹具。
            catalog: Handle::default(), // 空层验证无需实际目录资产。
            revision: 1,                // 明确表示目录已经完整采用。
            resolved: default(),        // 空输出不依赖任何预设。
            last_error: None,           // 不是目录加载失败。
        }; // Registry本身没有Default实现。

        manifest.terrain.kind = feathrise_map_content::MapTerrainKind::SparseClipmap; // 正式分页路线。
        assert!(validate_vegetation_definition(&vegetation, &manifest, &presets).is_ok()); // 最大图仍能创建草地事务。
        manifest.terrain.kind = feathrise_map_content::MapTerrainKind::Heightfield; // 切回旧后端。
        assert!(validate_vegetation_definition(&vegetation, &manifest, &presets).is_err()); // 旧全图内存保护不放宽。
    } // 地形后端决定高度预算，而非植被地图尺寸。

    #[test]
    fn in_memory_blade_density_is_ready_without_asset_server_file_load() {
        let mut app = App::new(); // 无窗口验证正式运行时加载门。
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>(); // 真实AssetServer与Assets。
        let manifest: MapManifestAsset = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "id": "MAP-blade-test", "display_name": "草稿测试", // 独立夹具不依赖用户地图。
            "runtime_scene": "DynamicMapScene", "world_min": [0,0], "world_max": [10,10], // 有效小地图。
            "default_spawn": [5,0,5], // 安全出生点。
            "terrain": {"kind":"heightfield", "glb_path":"maps/test/terrain.glb", "resolution":[2,2], "base_height":0, "height_scale":10} // 只验证加载门，不请求模型。
        }))
        .expect("测试清单合法");
        let layer = serde_json::from_value(serde_json::json!({
            "id":"new-grass", "label":"新草", "kind":"blade", // 尚无任何磁盘文件的新层。
            "runtime_profile":"hn01_ordinary_grass", "runtime_layer_id":1, "asset_path":"grass.glb", // 有效预设引用形状。
            "density_source_path":"missing.r8", "density_texture_path":"missing.ktx2", // 明确不存在。
            "resolution":[2,2], "spacing_meters":1, "scale_range":[1,1], "max_instances":4, "seed":1 // 固定层参数。
        }))
        .expect("测试层合法");
        let density = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default()); // 由作者系统直接登记，无路径加载状态。
        let pending = DynamicVegetationPending {
            map_id: manifest.id.clone(),       // 故障地图身份。
            manifest,                          // 该测试只验证资产就绪门。
            clipmap_height: None,              // 此测试无分页。
            height_cells: None,                // 不进行地形构建。
            heights: std::sync::Arc::from([]), // 无需CPU高度。
            terrain_generation: 1,             // 固定代次。
            layers: vec![DynamicVegetationLayerAssets {
                source_density: None,     // 此测试只核对图片加载门。
                in_memory_density: true,  // 明确声明内存完成值。
                definition: layer,        // 正式层描述。
                density: density.clone(), // 强句柄保持资产存活。
                blade_profile: None,      // 无需模型构建。
                blade_meshes: None,       // 不请求模型文件。
                blade_profile_bank: None, // 不请求图集文件。
            }],
            blade_profile_banks: vec![],       // 仅验证密度加载门。
            generation: 1,                     // 地图代次。
            revision: 1,                       // 植被修订。
            mesh_profiles_scanned: true,       // 无Mesh依赖。
            required_mesh_profiles: default(), // 空原型集合。
            mesh_profiles_requested: false,    // 本测试不驱动请求系统。
        };
        let server = app.world().resource::<AssetServer>(); // 内存图片不经过该服务加载。
        assert!(!server.is_loaded_with_dependencies(density.id())); // 旧门会永久等待。
        assert!(pending_assets_loaded(
            server,
            &pending,
            app.world().resource::<Assets<Image>>()
        )); // 新门识别真实内存资产。
        assert!(pending_asset_failure(server, &pending).is_none()); // 无路径状态不能解释为失败。
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .remove(density.id()); // 模拟完成值实际丢失。
        assert!(!pending_assets_loaded(
            app.world().resource::<AssetServer>(),
            &pending,
            app.world().resource::<Assets<Image>>()
        )); // 仍须真实存在，不能无条件放行。
    }

    #[test]
    fn blade_density_modified_upload_does_not_rebuild_the_dynamic_root() {
        let id = Handle::<Image>::default().id(); // 事件判断只依赖变体，不依赖真实AssetServer。
        let modified = AssetEvent::Modified { id }; // 模拟F12未保存密度写入产生的标准修改事件。
        assert!(!image_event_requests_rebuild(&modified, false, false)); // 纯Blade只需Bevy自动重新提取Image。
        assert!(image_event_requests_rebuild(&modified, true, false)); // Mesh密度或高度仍必须重新构建实例。
        let removed = AssetEvent::Removed { id }; // 资产消失不能继续沿用原GPU句柄。
        assert!(image_event_requests_rebuild(&removed, false, false)); // 即使纯Blade也进入正式恢复事务。
    }

    /// 构造只缺 UV0 的合法叶片顶点流。
    fn blade_mesh_without_uv() -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList, // GPU Blade 正式原型使用三角形列表。
            RenderAssetUsages::default(),    // 主世界数据保留供准备门读写。
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0.0_f32, 0.0, 0.0], [0.1, 1.0, 0.0], [-0.1, 0.0, 0.0]], // 有限 Y 范围支持根尖 UV 派生。
        );
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            vec![[0.0_f32, 0.0, 1.0]; 3], // 三个顶点均提供 WGSL location 1 法线。
        );
        mesh // 故意不插入 UV0，模拟 HN-02 历史 GLB primitive。
    }

    #[test]
    fn legacy_blade_mesh_becomes_pipeline_ready_after_vertical_uv_patch() {
        let mut mesh = blade_mesh_without_uv(); // 准备前只缺少可兼容派生的 UV0。
        assert_eq!(
            inspect_dynamic_blade_mesh(&mesh),
            Ok(DynamicBladeUvState::Missing)
        ); // 必须区分“可补 UV”与其他致命顶点布局错误。
        ensure_vertical_blade_uv(&mut mesh).expect("有效纵向叶片应能补齐UV"); // 执行与生产动态链路相同的兼容修复。
        assert_eq!(
            inspect_dynamic_blade_mesh(&mesh),
            Ok(DynamicBladeUvState::Ready)
        ); // 补齐后三个硬性 location 必须同时可用。
    }

    #[test]
    fn missing_normal_is_rejected_before_render_pipeline_creation() {
        let mut mesh = blade_mesh_without_uv(); // 从其他属性合法的叶片开始。
        mesh.remove_attribute(Mesh::ATTRIBUTE_NORMAL); // 模拟开发者放入缺法线的数据预设网格。
        assert_eq!(
            inspect_dynamic_blade_mesh(&mesh),
            Err("缺少NORMAL".to_owned())
        ); // 运行时应进入可读失败，不能留给 wgpu 终止进程。
    }

    #[test]
    fn valid_internal_uv_modification_does_not_trigger_rebuild_loop() {
        let mut mesh = blade_mesh_without_uv(); // 先模拟旧网格首次被准备门修改。
        ensure_vertical_blade_uv(&mut mesh).expect("测试叶片应能补齐UV"); // Assets<Mesh>::get_mut 随后会产生 Modified 事件。
        let modified = AssetEvent::Modified {
            id: Handle::<Mesh>::default().id(),
        }; // 事件身份在纯函数测试中不影响布局判定。
        assert!(!blade_mesh_event_requests_rebuild(
            &modified,
            Some(&mesh),
            false
        )); // 已合法的原位上传不得反复销毁并重建动态植被根。
    }
}
