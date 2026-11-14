//! 场景内容资产的稳定目录与文件名契约。

/// 游戏启动时始终请求的场景目录资产。
pub const SCENE_CATALOG_ASSET_PATH: &str = "scenes/catalog.scene_catalog.json";

/// 编辑器创建的场景身份前缀。删除保护据此确认目录属于编辑器管理。
pub const GENERATED_SCENE_ID_PREFIX: &str = "SCENE-";

/// 动态场景共享的全局世界环境作者资产。
pub const WORLD_ENVIRONMENT_ASSET_PATH: &str = "config/world_environment.world_environment.json";

/// 开发者维护的全局建筑预设目录资产。
pub const BUILDING_PRESET_CATALOG_ASSET_PATH: &str =
    "config/building_presets.building_presets.json"; // 场景实例只附带稳定预设ID，模型路径仍保留独立加载能力。

/// 开发者维护的全局院落组合预设目录资产。
pub const COURTYARD_PRESET_CATALOG_ASSET_PATH: &str =
    "config/courtyard_presets.courtyard_presets.json"; // 场景旧摆放无需引用此目录，新组合成员只保存稳定身份与明确模型来源。

/// 开发者维护的全局植被预设目录资产。
pub const VEGETATION_PRESET_CATALOG_ASSET_PATH: &str =
    "config/vegetation_presets.vegetation_presets.json"; // 发布包只读取这一显式清单，不依赖运行时目录枚举。

/// 开发者维护的全局 PBR 地表预设目录资产。
pub const SURFACE_PRESET_CATALOG_ASSET_PATH: &str = "config/surface_presets.surface_presets.json"; // 场景只持久化稳定 ID，贴图数组切片由该目录当前顺序派生。

/// 开发者维护的全局水体外观预设目录资产。
pub const WATER_PRESET_CATALOG_ASSET_PATH: &str = "config/water_presets.water_presets.json"; // 静水层只引用稳定ID，目录项数量不进入场景格式或Shader布局。

/// 单个场景目录内的作者清单文件名。
pub const SCENE_MANIFEST_FILE_NAME: &str = "scene.scene_manifest.json";

/// 单个场景目录内的传送图文件名。
pub const SCENE_PORTALS_FILE_NAME: &str = "portals.scene_portals.json";

/// 单个场景目录内的显式实例文件名。
pub const SCENE_PLACEMENTS_FILE_NAME: &str = "placements.scene_placements.json";
/// 场景私有 GLB，同时承载静态布景的网格、材质和摆放节点。
pub const SCENE_PLACEMENTS_GLB_FILE_NAME: &str = "scene.glb";

/// 单个场景目录内的地表层与RGBA权重作者文件名。
pub const SCENE_SURFACE_FILE_NAME: &str = "surface.scene_surface.json";

/// 单个场景目录内的植被层与密度作者文件名。
pub const SCENE_VEGETATION_FILE_NAME: &str = "vegetation.scene_vegetation.json";

/// 单个场景目录内的水域层与R8覆盖作者文件名。
pub const SCENE_WATER_FILE_NAME: &str = "water.scene_water.json";

/// 单个场景目录内的区域边界与空气墙作者文件名。
pub const SCENE_BOUNDARIES_FILE_NAME: &str = "boundaries.scene_boundaries.json";

/// 场景目录、清单和私有作者文件共用这一代格式；地图格式的版本 1 不进入场景加载链。
pub const SCENE_SCHEMA_VERSION: u32 = 2;

/// 场景地表以 scene_id 绑定作者文件，继续使用 RGBA32 索引和 RGBA8 权重。
pub const SCENE_SURFACE_SCHEMA_VERSION: u32 = 3;
