//! 地图内容资产的稳定目录与文件名契约。

/// 游戏启动时始终请求的地图目录资产。
pub const MAP_CATALOG_ASSET_PATH: &str = "maps/catalog.map_catalog.json";

/// 动态地图共享的全局世界环境作者资产。
pub const WORLD_ENVIRONMENT_ASSET_PATH: &str = "config/world_environment.world_environment.json";

/// 开发者维护的全局建筑预设目录资产。
pub const BUILDING_PRESET_CATALOG_ASSET_PATH: &str =
    "config/building_presets.building_presets.json"; // 地图实例只附带稳定预设ID，模型路径仍保留独立加载能力。

/// 开发者维护的全局院落组合预设目录资产。
pub const COURTYARD_PRESET_CATALOG_ASSET_PATH: &str =
    "config/courtyard_presets.courtyard_presets.json"; // 地图旧摆放无需引用此目录，新组合成员只保存稳定身份与明确模型来源。

/// 开发者维护的全局植被预设目录资产。
pub const VEGETATION_PRESET_CATALOG_ASSET_PATH: &str =
    "config/vegetation_presets.vegetation_presets.json"; // 发布包只读取这一显式清单，不依赖运行时目录枚举。

/// 开发者维护的全局 PBR 地表预设目录资产。
pub const SURFACE_PRESET_CATALOG_ASSET_PATH: &str = "config/surface_presets.surface_presets.json"; // 地图只持久化稳定 ID，贴图数组切片由该目录当前顺序派生。

/// 开发者维护的全局水体外观预设目录资产。
pub const WATER_PRESET_CATALOG_ASSET_PATH: &str = "config/water_presets.water_presets.json"; // 静水层只引用稳定ID，目录项数量不进入地图格式或Shader布局。

/// 单张地图目录内的作者清单文件名。
pub const MAP_MANIFEST_FILE_NAME: &str = "map.map_manifest.json";

/// 单张地图目录内的传送图文件名。
pub const MAP_PORTALS_FILE_NAME: &str = "portals.map_portals.json";

/// 单张地图目录内的显式实例文件名。
pub const MAP_PLACEMENTS_FILE_NAME: &str = "placements.map_placements.json";

/// 单张地图目录内的地表层与RGBA权重作者文件名。
pub const MAP_SURFACE_FILE_NAME: &str = "surface.map_surface.json";

/// 单张地图目录内的植被层与密度作者文件名。
pub const MAP_VEGETATION_FILE_NAME: &str = "vegetation.map_vegetation.json";

/// 单张地图目录内的水域层与R8覆盖作者文件名。
pub const MAP_WATER_FILE_NAME: &str = "water.map_water.json";

/// 单张地图目录内的区域边界与空气墙作者文件名。
pub const MAP_BOUNDARIES_FILE_NAME: &str = "boundaries.map_boundaries.json";

/// 当前数据格式版本；不兼容变更必须提升版本并提供迁移器。
pub const MAP_SCHEMA_VERSION: u32 = 1;

/// 当前动态地图地表文件格式版本。
pub const MAP_SURFACE_SCHEMA_VERSION: u32 = 2; // 第二版改为 RGBA32 索引 + RGBA8 权重，不再保留五层残余格式。
