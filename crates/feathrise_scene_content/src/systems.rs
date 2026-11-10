//! 场景目录首次加载、热更新和失败诊断系统。

use std::collections::{BTreeMap, BTreeSet}; // 校验重复ID并构建确定性句柄表。

use bevy::asset::{AssetEvent, AssetLoadFailedEvent}; // 资产首次加载、热更与失败消息。
use bevy::prelude::*; // ECS系统参数和日志。

use crate::{
    BUILDING_PRESET_CATALOG_ASSET_PATH,
    BuildingPresetCatalogAsset,
    BuildingPresetRegistry,
    COURTYARD_PRESET_CATALOG_ASSET_PATH,
    CourtyardPresetCatalogAsset,
    CourtyardPresetRegistry,
    SCENE_CATALOG_ASSET_PATH,
    SCENE_SCHEMA_VERSION,
    SceneBoundariesAsset,
    SceneCatalogAsset,
    SceneContentHandles,
    SceneContentRegistry,
    SceneManifestAsset,
    ScenePlacementsAsset,
    ScenePortalsAsset,
    SceneSurfaceAsset,
    SceneVegetationAsset,
    SceneWaterAsset,
    SURFACE_PRESET_CATALOG_ASSET_PATH,
    SurfacePresetCatalogAsset,
    SurfacePresetRegistry,
    VEGETATION_PRESET_CATALOG_ASSET_PATH,
    VegetationPresetCatalogAsset,
    VegetationPresetRegistry,
    WATER_PRESET_CATALOG_ASSET_PATH,
    WORLD_ENVIRONMENT_ASSET_PATH,
    WaterPresetCatalogAsset,
    WaterPresetRegistry,
    WorldEnvironmentAsset,
    WorldEnvironmentRegistry,
    load_water_micro_normal, // 正式目录与F12草稿共享线性重复采样设置。
};

/// 启动时请求唯一全局建筑预设目录，并以强句柄保持热更新订阅。
pub(crate) fn load_building_preset_catalog(
    mut commands: Commands,         // 插入全局只读建筑目录注册表。
    asset_server: Res<AssetServer>, // 通过 Bevy 异步资产 IO 请求固定入口。
) {
    let catalog =
        asset_server.load::<BuildingPresetCatalogAsset>(BUILDING_PRESET_CATALOG_ASSET_PATH); // 空目录也是真实可编辑初始状态。
    commands.insert_resource(BuildingPresetRegistry {
        catalog,                      // 强句柄保证首载与后续热更事件可达。
        revision: 0,                  // 首次完整采用后推进到一。
        resolved: Default::default(), // IO 等待期间公开空快照而不伪造房屋。
        last_error: None,             // 尚未收到失败事件时不显示错误。
    });
}

/// 建筑目录首载或热更后，只在整份版本、身份和几何校验通过时原子采用。
pub(crate) fn apply_building_preset_catalog(
    assets: Res<Assets<BuildingPresetCatalogAsset>>, // 只读取 AssetServer 已完成的完整 JSON 值。
    mut events: MessageReader<AssetEvent<BuildingPresetCatalogAsset>>, // 稳定帧没有事件时不扫描目录。
    mut registry: Option<ResMut<BuildingPresetRegistry>>, // 独立系统测试允许资源尚未初始化。
) {
    let Some(registry) = registry.as_deref_mut() else {
        return; // 没有注册表时不存在可采用目标。
    };
    let id = registry.catalog.id(); // 只响应固定全局目录句柄。
    if !events.read().any(|event| {
        event.is_added(id) || event.is_modified(id) || event.is_loaded_with_dependencies(id)
    }) {
        return; // 无关资产事件不会触发任意数量建筑重新校验。
    }
    let Some(asset) = assets.get(&registry.catalog) else {
        return; // 移除和 IO 失败由专用失败路径保留旧快照。
    };
    match crate::ResolvedBuildingPresetCatalog::build(asset) {
        Ok(resolved) => {
            registry.resolved = resolved; // 描述数组与别名索引在同一边界替换。
            registry.revision = registry.revision.wrapping_add(1).max(1); // 消费者据此拒绝旧派生结果。
            registry.last_error = None; // 成功热修清除旧诊断。
            info!(
                "建筑预设目录已应用：{}项，修订{}",
                registry.presets().len(),
                registry.revision,
            ); // 日志只报告真实目录状态。
        }
        Err(error) => {
            registry.last_error = Some(format!("建筑预设目录无效：{error}")); // 旧完整快照继续服务已打开场景。
            error!("{}", registry.last_error.as_deref().unwrap_or_default()); // F12 与日志共享明确原因。
        }
    }
}

/// 建筑目录文件缺失或 IO 失败时保留上一完整快照并发布诊断。
pub(crate) fn warn_building_preset_catalog_load_failed(
    mut failures: MessageReader<AssetLoadFailedEvent<BuildingPresetCatalogAsset>>, // 只消费建筑目录加载失败。
    mut registry: Option<ResMut<BuildingPresetRegistry>>, // 启动边界测试允许没有完整插件资源。
) {
    for failure in failures.read() {
        let message = format!("建筑预设目录加载失败：{} ({})", failure.path, failure.error,); // 保留真实路径和 IO 原因供作者修复。
        if let Some(registry) = registry.as_deref_mut() {
            registry.last_error = Some(message.clone()); // 不清空上一份已经采用的建筑快照。
        }
        error!("{message}"); // 无注册表时仍保留启动日志证据。
    }
}

/// 启动时请求唯一全局院落预设目录，并持有强句柄订阅热更新。
pub(crate) fn load_courtyard_preset_catalog(
    mut commands: Commands,         // 只建立院落目录的全局注册表。
    asset_server: Res<AssetServer>, // 由资产 IO 异步读取固定目录入口。
) {
    let catalog =
        asset_server.load::<CourtyardPresetCatalogAsset>(COURTYARD_PRESET_CATALOG_ASSET_PATH); // 空目录是合法的初始作者状态。
    commands.insert_resource(CourtyardPresetRegistry {
        catalog,                      // 强句柄保持首载与后续热更可达。
        revision: 0,                  // 首次完整成功采用才推进到一。
        resolved: Default::default(), // 等待 IO 时仅公开空快照。
        last_error: None,             // 尚无失败事件时不伪造诊断。
    });
}

/// 院落目录收到完整资产事件后，只在全量校验通过时替换旧快照。
pub(crate) fn apply_courtyard_preset_catalog(
    assets: Res<Assets<CourtyardPresetCatalogAsset>>, // 只借用资产服务器已装入的完整 JSON。
    mut events: MessageReader<AssetEvent<CourtyardPresetCatalogAsset>>, // 稳定帧不扫描目录。
    mut registry: Option<ResMut<CourtyardPresetRegistry>>, // 独立测试允许启动注册表尚不存在。
) {
    let Some(registry) = registry.as_deref_mut() else {
        return; // 没有全局目录句柄便无可采用目标。
    };
    let id = registry.catalog.id(); // 无关院落资产的热更不影响固定目录。
    if !events.read().any(|event| {
        event.is_added(id) || event.is_modified(id) || event.is_loaded_with_dependencies(id)
    }) {
        return; // 无匹配事件时沿用上次已经校验的完整快照。
    }
    let Some(asset) = assets.get(&registry.catalog) else {
        return; // 删除、尚未完成 IO 或读取失败均保留旧快照。
    };
    match crate::ResolvedCourtyardPresetCatalog::build(asset) {
        Ok(resolved) => {
            registry.resolved = resolved; // 成员数组和稳定 ID 索引一次替换。
            registry.revision = registry.revision.wrapping_add(1).max(1); // 消费者据此拒收旧候选。
            registry.last_error = None; // 成功修复清除先前的完整校验错误。
            info!(
                "院落预设目录已应用：{}项，修订{}",
                registry.presets().len(),
                registry.revision
            ); // 日志反映真实采用状态。
        }
        Err(error) => {
            registry.last_error = Some(format!("院落预设目录无效：{error}")); // 不清空旧成员与索引，不提升修订。
            error!("{}", registry.last_error.as_deref().unwrap_or_default()); // 作者可依据具体项修复源文件。
        }
    }
}

/// 院落目录文件缺失或读取失败时报告错误并保留上一完整快照。
pub(crate) fn warn_courtyard_preset_catalog_load_failed(
    mut failures: MessageReader<AssetLoadFailedEvent<CourtyardPresetCatalogAsset>>, // 只消费院落资产类型的 IO 失败。
    mut registry: Option<ResMut<CourtyardPresetRegistry>>, // 无注册表时仍保留日志诊断。
) {
    for failure in failures.read() {
        let message = format!("院落预设目录加载失败：{} ({})", failure.path, failure.error); // 保留真实路径和错误。
        if let Some(registry) = registry.as_deref_mut() {
            registry.last_error = Some(message.clone()); // 保留已经采用的旧院落目录。
        }
        error!("{message}"); // 不静默吞掉作者 JSON 解析或 IO 错误。
    }
}

/// 启动时请求唯一场景目录，并以强句柄保持热更新订阅。
pub(crate) fn load_scene_catalog(mut commands: Commands, asset_server: Res<AssetServer>) {
    let catalog = asset_server.load::<SceneCatalogAsset>(SCENE_CATALOG_ASSET_PATH);
    commands.insert_resource(SceneContentRegistry {
        catalog,
        revision: 0,
        scenes: BTreeMap::new(),
        last_error: None,
    });
}

/// 启动时请求唯一开发者植被预设目录，并以强句柄保持热更新订阅。
pub(crate) fn load_vegetation_preset_catalog(
    mut commands: Commands,         // 插入全局只读目录注册表。
    asset_server: Res<AssetServer>, // 通过 Bevy 异步资产 IO 加载固定入口。
) {
    let catalog =
        asset_server.load::<VegetationPresetCatalogAsset>(VEGETATION_PRESET_CATALOG_ASSET_PATH); // 固定显式文件可进入发布资产依赖图，不运行目录枚举。
    commands.insert_resource(VegetationPresetRegistry {
        catalog,                      // 强句柄让热更新事件持续可达。
        revision: 0,                  // 首次完整采用后推进到一。
        resolved: Default::default(), // IO 等待期间 UI 显示空目录而不使用硬编码按钮。
        last_error: None,             // 尚未收到失败事件时不伪造诊断。
    });
}

/// 开发者预设目录首次加载或热更后，仅在整份校验通过时原子采用。
pub(crate) fn apply_vegetation_preset_catalog(
    assets: Res<Assets<VegetationPresetCatalogAsset>>, // 只读取 AssetServer 已完成的 JSON 值。
    mut events: MessageReader<AssetEvent<VegetationPresetCatalogAsset>>, // 无事件稳定帧不重复扫描任意数量预设。
    mut registry: Option<ResMut<VegetationPresetRegistry>>, // 独立测试允许插件资源尚未建立。
) {
    let Some(registry) = registry.as_deref_mut() else {
        return; // 没有注册表时不存在可采用目标。
    };
    let id = registry.catalog.id(); // 只响应固定全局目录句柄。
    if !events.read().any(|event| {
        event.is_added(id) || event.is_modified(id) || event.is_loaded_with_dependencies(id)
    }) {
        return; // 稳定帧不遍历或重建目录索引。
    }
    let Some(asset) = assets.get(&registry.catalog) else {
        return; // Removed 由失败或后续修复事件处理，当前快照继续有效。
    };
    match crate::ResolvedVegetationPresetCatalog::build(asset) {
        Ok(resolved) => {
            registry.resolved = resolved; // 完整目录与别名索引同一边界替换。
            registry.revision = registry.revision.wrapping_add(1).max(1); // 消费者可丢弃旧派生结果。
            registry.last_error = None; // 成功热更清除旧诊断。
            info!(
                "开发者植被预设目录已应用：{}项，修订{}",
                registry.presets().len(),
                registry.revision
            ); // 日志只报告真实项数，不暗示产品上限。
        }
        Err(error) => {
            registry.last_error = Some(format!("开发者植被预设目录无效：{error}")); // 旧完整快照继续服务当前场景。
            error!("{}", registry.last_error.as_deref().unwrap_or_default()); // F12 与日志共享可操作原因。
        }
    }
}

/// 预设目录文件缺失或 IO 失败时保留上一完整快照并发布诊断。
pub(crate) fn warn_vegetation_preset_catalog_load_failed(
    mut failures: MessageReader<AssetLoadFailedEvent<VegetationPresetCatalogAsset>>, // 只消费对应资产类型的明确失败。
    mut registry: Option<ResMut<VegetationPresetRegistry>>, // 启动边界测试允许没有完整插件资源。
) {
    for failure in failures.read() {
        let message = format!(
            "开发者植被预设目录加载失败：{} ({})",
            failure.path, failure.error
        ); // 保留真实路径和 IO 原因。
        if let Some(registry) = registry.as_deref_mut() {
            registry.last_error = Some(message.clone()); // 不清空上一成功快照，热修复后可继续采用。
        }
        error!("{message}"); // 无注册表时仍给开发者明确日志。
    }
}

/// 启动时请求唯一全局地表预设目录。
pub(crate) fn load_surface_preset_catalog(mut commands: Commands, asset_server: Res<AssetServer>) {
    let catalog = asset_server.load::<SurfacePresetCatalogAsset>(SURFACE_PRESET_CATALOG_ASSET_PATH);
    commands.insert_resource(SurfacePresetRegistry {
        catalog,
        revision: 0,
        resolved: Default::default(),
        last_error: None,
    });
}

/// 只在整份目录通过版本、身份、路径和 GPU 数值校验后原子采用。
pub(crate) fn apply_surface_preset_catalog(
    assets: Res<Assets<SurfacePresetCatalogAsset>>,
    mut events: MessageReader<AssetEvent<SurfacePresetCatalogAsset>>,
    mut registry: Option<ResMut<SurfacePresetRegistry>>,
) {
    let Some(registry) = registry.as_deref_mut() else {
        return;
    };
    let id = registry.catalog.id();
    if !events.read().any(|event| {
        event.is_added(id) || event.is_modified(id) || event.is_loaded_with_dependencies(id)
    }) {
        return;
    }
    let Some(asset) = assets.get(&registry.catalog) else {
        return;
    };
    match crate::ResolvedSurfacePresetCatalog::build(asset) {
        Ok(resolved) => {
            registry.resolved = resolved;
            registry.revision = registry.revision.wrapping_add(1).max(1);
            registry.last_error = None;
            info!(
                "PBR地表预设目录已应用：{}项，修订{}",
                registry.presets().len(),
                registry.revision
            );
        }
        Err(error) => {
            registry.last_error = Some(format!("PBR地表预设目录无效：{error}"));
            error!("{}", registry.last_error.as_deref().unwrap_or_default());
        }
    }
}

/// 目录 IO 失败时保留上一份完整快照，热修文件后可继续采用。
pub(crate) fn warn_surface_preset_catalog_load_failed(
    mut failures: MessageReader<AssetLoadFailedEvent<SurfacePresetCatalogAsset>>,
    mut registry: Option<ResMut<SurfacePresetRegistry>>,
) {
    for failure in failures.read() {
        let message = format!(
            "PBR地表预设目录加载失败：{} ({})",
            failure.path, failure.error
        );
        if let Some(registry) = registry.as_deref_mut() {
            registry.last_error = Some(message.clone());
        }
        error!("{message}");
    }
}

/// 启动时请求唯一全局水体预设目录。
pub(crate) fn load_water_preset_catalog(mut commands: Commands, asset_server: Res<AssetServer>) {
    let catalog = asset_server.load::<WaterPresetCatalogAsset>(WATER_PRESET_CATALOG_ASSET_PATH);
    commands.insert_resource(WaterPresetRegistry::loading(catalog));
}

/// 整份水体目录校验通过后，原子替换描述快照和全部可选微法线强句柄。
pub(crate) fn apply_water_preset_catalog(
    asset_server: Res<AssetServer>,
    assets: Res<Assets<WaterPresetCatalogAsset>>,
    mut events: MessageReader<AssetEvent<WaterPresetCatalogAsset>>,
    mut registry: Option<ResMut<WaterPresetRegistry>>,
) {
    let Some(registry) = registry.as_deref_mut() else {
        return;
    };
    let id = registry.catalog.id();
    if !events.read().any(|event| {
        event.is_added(id) || event.is_modified(id) || event.is_loaded_with_dependencies(id)
    }) {
        return;
    }
    let Some(asset) = assets.get(&registry.catalog) else {
        return;
    };
    match crate::ResolvedWaterPresetCatalog::build(asset) {
        Ok(resolved) => {
            let image_handles = resolved
                .presets()
                .iter()
                .map(|preset| {
                    let handles = std::array::from_fn(|index| {
                        preset.micro_normals[index]
                            .texture_path
                            .as_ref()
                            .map(|path| load_water_micro_normal(&asset_server, path.clone()))
                    });
                    (preset.id.clone(), handles)
                })
                .collect();
            registry.resolved = resolved;
            registry.replace_image_handles(image_handles);
            registry.revision = registry.revision.wrapping_add(1).max(1);
            registry.last_error = None;
            info!(
                "水体外观预设目录已应用：{}项，修订{}",
                registry.presets().len(),
                registry.revision
            );
        }
        Err(error) => {
            registry.last_error = Some(format!("水体外观预设目录无效：{error}"));
            error!("{}", registry.last_error.as_deref().unwrap_or_default());
        }
    }
}

/// 水体目录 IO 失败时保留上一完整快照和强图片句柄。
pub(crate) fn warn_water_preset_catalog_load_failed(
    mut failures: MessageReader<AssetLoadFailedEvent<WaterPresetCatalogAsset>>,
    mut registry: Option<ResMut<WaterPresetRegistry>>,
) {
    for failure in failures.read() {
        let message = format!(
            "水体外观预设目录加载失败：{} ({})",
            failure.path, failure.error
        );
        if let Some(registry) = registry.as_deref_mut() {
            registry.last_error = Some(message.clone());
        }
        error!("{message}");
    }
}

/// 启动时请求唯一全局世界环境文件，并在 IO 完成前立即发布安全编译期默认值。
pub(crate) fn load_world_environment(mut commands: Commands, asset_server: Res<AssetServer>) {
    let handle = asset_server.load::<WorldEnvironmentAsset>(WORLD_ENVIRONMENT_ASSET_PATH); // 强句柄保持全局配置热更新订阅。
    commands.insert_resource(WorldEnvironmentRegistry::with_handle(handle)); // 文件缺失也不会让动态场景失去方向光。
}

/// 全局世界环境首次加载或热更后，仅在完整校验通过时原子采用六组属性。
pub(crate) fn apply_world_environment(
    assets: Res<Assets<WorldEnvironmentAsset>>, // 只读取 AssetServer 已完成的完整 JSON 资产。
    mut events: MessageReader<AssetEvent<WorldEnvironmentAsset>>, // 稳定帧不轮询文件或重复校验。
    mut registry: Option<ResMut<WorldEnvironmentRegistry>>, // 独立测试允许插件资源尚未建立。
) {
    let Some(registry) = registry.as_deref_mut() else {
        return; // 没有启动注册表时不存在可采用目标。
    };
    let id = registry.handle.id(); // 只响应固定全局句柄，拒绝无关同类型测试资产。
    if !events.read().any(|event| {
        event.is_added(id) || event.is_modified(id) || event.is_loaded_with_dependencies(id)
    }) {
        return; // 没有相关资产事件时不做任何稳定帧工作。
    }
    let Some(asset) = assets.get(&registry.handle) else {
        return; // 移除事件由失败路径或后续重载处理，当前有效默认值保持不变。
    };
    match asset.validate() {
        Ok(()) => {
            registry.celestial_lighting = asset.celestial_lighting.clone().normalized(); // 六组属性作为一个完整快照采用。
            registry.last_error = None; // 成功热更清除旧错误提示。
        }
        Err(error) => {
            registry.celestial_lighting = Default::default(); // 无效文件明确回退编译期默认值，不沿用可能误导的旧草稿。
            registry.last_error = Some(format!("全局世界环境配置无效：{error}")); // F12 保留可操作错误。
            error!("{}", registry.last_error.as_deref().unwrap_or_default()); // 日志与编辑器使用同一诊断。
        }
    }
    registry.revision = registry.revision.wrapping_add(1); // 成功与失败回退都要求动态场景重新解析最终配置。
}

/// 全局世界环境文件缺失或 IO 失败时使用编译期默认值并发布可见错误。
pub(crate) fn warn_world_environment_load_failed(
    mut failures: MessageReader<AssetLoadFailedEvent<WorldEnvironmentAsset>>, // 只处理 AssetServer 明确失败事件。
    mut registry: Option<ResMut<WorldEnvironmentRegistry>>, // 独立系统测试允许缺少注册表。
) {
    for failure in failures.read() {
        let message = format!(
            "全局世界环境配置加载失败：{} ({})",
            failure.path, failure.error
        ); // 保留真实路径与 IO 原因。
        if let Some(registry) = registry.as_deref_mut() {
            registry.celestial_lighting = Default::default(); // 文件缺失也保持当前动态场景既有画面。
            registry.last_error = Some(message.clone()); // F12 可提示作者修复或保存新文件。
            registry.revision = registry.revision.wrapping_add(1); // 消费者同帧后续或下一帧采用明确默认快照。
        }
        error!("{message}"); // 无注册表时仍记录启动诊断。
    }
}

/// 在场景目录首次加载或热更后原子替换全部私有资产句柄。
pub(crate) fn apply_scene_catalog(
    asset_server: Res<AssetServer>,
    catalogs: Res<Assets<SceneCatalogAsset>>,
    mut events: MessageReader<AssetEvent<SceneCatalogAsset>>,
    mut registry: Option<ResMut<SceneContentRegistry>>,
) {
    let Some(registry) = registry.as_deref_mut() else {
        return;
    };
    let catalog_id = registry.catalog.id();
    let changed = events.read().any(|event| {
        event.is_added(catalog_id)
            || event.is_modified(catalog_id)
            || event.is_loaded_with_dependencies(catalog_id)
    });
    if !changed {
        return;
    }
    let Some(catalog) = catalogs.get(&registry.catalog) else {
        return;
    };
    if catalog.schema_version != SCENE_SCHEMA_VERSION {
        registry.last_error = Some(format!(
            "场景目录版本{}与运行时版本{}不一致",
            catalog.schema_version, SCENE_SCHEMA_VERSION
        ));
        return;
    }
    let mut unique_ids = BTreeSet::new();
    let mut next_scenes = BTreeMap::new();
    for entry in &catalog.scenes {
        if entry.id.as_str().is_empty() || !unique_ids.insert(entry.id.clone()) {
            registry.last_error = Some(format!("场景目录包含空ID或重复ID：{}", entry.id));
            return;
        }
        if entry.folder.starts_with('/')
            || entry.folder.contains("..")
            || entry.folder.contains('\\')
        {
            registry.last_error = Some(format!("场景{}目录必须是assets内正斜杠相对路径", entry.id));
            return;
        }
        let optional_paths_are_safe = [
            entry.surface_path.as_deref(),
            entry.vegetation_path.as_deref(),
            entry.water_path.as_deref(),
            entry.boundary_path.as_deref(),
            entry.characters_path.as_deref(), // 新角色文件使用同一受检assets相对路径。
        ]
        .into_iter()
        .flatten()
        .all(|path| {
            !path.is_empty()
                && !path.starts_with('/')
                && !path.contains("..")
                && !path.contains('\\')
        });
        if !optional_paths_are_safe {
            registry.last_error = Some(format!(
                "场景{}的地表、植被、水域或区域边界作者路径必须位于assets内",
                entry.id
            ));
            return;
        }
        next_scenes.insert(
            entry.id.clone(),
            SceneContentHandles {
                characters_path: entry.characters_path.clone(), // 只保留受检兼容引用，typed资产由游戏显式请求。
                manifest: asset_server.load::<SceneManifestAsset>(entry.manifest_path()),
                portals: asset_server.load::<ScenePortalsAsset>(entry.portals_path()),
                placements: asset_server.load::<ScenePlacementsAsset>(entry.placements_path()),
                surface: entry
                    .surface_path
                    .as_ref()
                    .map(|path| asset_server.load::<SceneSurfaceAsset>(path.clone())),
                vegetation: entry
                    .vegetation_path
                    .as_ref()
                    .map(|path| asset_server.load::<SceneVegetationAsset>(path.clone())),
                water: entry
                    .water_path
                    .as_ref()
                    .map(|path| asset_server.load::<SceneWaterAsset>(path.clone())),
                boundaries: entry
                    .boundary_path
                    .as_ref()
                    .map(|path| asset_server.load::<SceneBoundariesAsset>(path.clone())),
            },
        );
    }
    registry.scenes = next_scenes;
    registry.revision = registry.revision.wrapping_add(1);
    registry.last_error = None;
    info!(
        "场景目录已应用：{}个场景，修订{}",
        registry.scenes.len(),
        registry.revision
    );
}

/// 对场景目录加载失败提供明确诊断而不静默回退为硬编码场景。
pub(crate) fn warn_scene_catalog_load_failed(
    mut failures: MessageReader<AssetLoadFailedEvent<SceneCatalogAsset>>,
    mut registry: Option<ResMut<SceneContentRegistry>>,
) {
    for failure in failures.read() {
        let message = format!("场景目录加载失败：{} ({})", failure.path, failure.error);
        if let Some(registry) = registry.as_deref_mut() {
            registry.last_error = Some(message.clone());
        }
        error!("{message}");
    }
}
