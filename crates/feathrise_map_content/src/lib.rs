//! 数据驱动地图内容公共入口。
//!
//! 本crate只定义可热加载的数据资产、R8水域作者源和目录注册表，不依赖具体GameScene或编辑器UI，
//! 从而让正式场景运行时与F12编辑器共享完全相同的文件格式。

mod assets; // 地图目录、单图清单、传送图和摆放实例资产。
#[cfg(test)]
mod runtime_kind_tests; // 公共场景类型与旧文件原文往返，不读取游戏素材。
#[cfg(test)]
mod legacy_extension_tests; // 旧游戏扩展只保留路径，不在公共地图内注册或读取角色类型。
#[cfg(test)]
mod weather_compatibility_tests; // 天气旧字段只读兼容与中性保存回归，不依赖游戏素材。
mod author_preview; // 活动地图标签的内存草稿通过临时强句柄参与正式就绪门。
pub mod embedded_snapshot; // 跨进程只交换正式作者源，编辑器与工程运行端共享同一版本合同。
pub mod embedded_preview; // 连续植被只投影生产完成的正式层与原始R8，不传整地图或GPU句柄。
pub mod embedded_surface_trail; // 原地表即时辅助图按固定行块投影，正式PBR仍沿原松笔路径。
pub mod embedded_placement_preview; // 原手柄和数值事务使用稳定placement身份投影到驻留工程。
pub mod embedded_author_preview; // 原已经发布的高度/建模/水域局部结果跨进程共享纯值合同。
pub mod authoring_io; // F12与无窗口工具共享作者锁、路径和流式摘要。
pub mod authoring_transaction; // 原地图跨文件事务下沉，不引入编辑器依赖。
mod boundary_assets; // 地图私有区域边界、正式校验与碰撞线段派生。
mod building_presets; // 全局建筑预设、占地、正门、地基与植被净空语义。
mod celestial_lighting; // 全局太阳/月光基础值、地图逐字段覆盖与热加载注册表。
mod collision_lods;
mod constants; // 稳定目录、文件名和格式版本。 // 新模式默认配置及编辑器和运行时共用校验。
pub use collision_lods::*; // 避免编辑器和物理接入各自解释倍率。
mod display_lods; // Clipmap显示LOD的兼容自动模式、显式作者默认值与纯结构校验。
pub use display_lods::*; // 编辑器和场景运行时必须共享同一显示范围语义。
mod mesh_lod; // 普通静态网格的通用策略与各后端专用LOD参数分开持久化。
pub use mesh_lod::{MeshCollisionLodSettings, MeshDisplayLodSettings, MeshLodMode, MeshLodPolicy};
mod courtyard_presets; // 全局院落组合预设及显式成员原型。
mod distribution; // 编辑器和运行时共享的确定性植被散布纯函数。
mod loaders; // 接管地图复合JSON和水域原始R8加载器。
mod map_instances; // 完整子地图引用、受检刚体变换和后台实例树展开。
mod placement_transform; // 显式资产变换范围由编辑器、保存和运行时共同采用。
mod editable_cuboid; // 点边面共用固定拓扑、半毫米坐标和保存前几何校验。
mod editable_mesh; // 有界共享四边面拓扑、同源三角化与完整闭壳验证。
mod mesh_extrusion; // 单面挤出的世界法线、局部逆算和无副作用完整记录候选。
mod placement_editor; // 每份摆放的名称、逻辑组及仅供编辑器使用的保护标记。
mod placement_water; // Blender整网格水体覆盖类型与纯校验保持独立，避免摆放资产定义继续膨胀。
mod puddles;
mod terrain_topology; // 静态三维地形和高度面让位共享作者协议与裁切算法。 // 积水潜力描述和受检R8规格独立维护。
mod terrain_objects; // 空白地图、独立地形身份、变换与各后端的统一作者合同。
mod clipmap_stitches; // 不同作者格距的完整边关系与共享边界曲线。
mod terrain_attachments; // 内容承托关联不归地形资产拥有，旧世界内容保持固定。
pub use terrain_attachments::{MapTerrainBinding, TerrainBindingIndex};
pub use puddles::MapPuddleDescriptor; // F12与正式场景共享唯一积水作者协议。
mod resources; // 当前地图目录和私有资产强句柄注册表。
mod runtime_paths; // 体素/体积后台读取使用显式工程磁盘根，不反向依赖编辑器。
pub use runtime_paths::RuntimeAssetRoot;
pub mod sparse_height; // 新地图稀疏高度作者源独立于 F12、场景和相机环带。
mod surface_crater; // 技能浅坑按地表四槽作者源采样玩法形变系数。
mod surface_presets; // 全局可编辑 PBR 地表预设、稳定引用和原子校验。
mod systems; // 首次加载、热更新和失败诊断。
mod types; // 地图ID、地形种类和资产用途。
mod vegetation_authoring; // 植被作者图层、图层组和离线编译规则；正式运行时层继续保持固定数量。
mod vegetation_paint_groups; // 跨网格绘制关联独立于网格内部的容量/竞争图层组。
pub use vegetation_paint_groups::MapVegetationPaintGroup;
mod vegetation_presets; // 开发者预设目录、稳定身份与 Blade/Mesh 数据规格。
mod water_assets; // R8覆盖、水位、流向与动态水材质作者参数。
mod water_presets; // 全局水色、四组风浪、两组微法线与稳定引用。

pub use assets::*; // 运行时和编辑器共享所有作者数据结构。
pub use map_instances::*; // 父图保存引用，运行时与编辑器共享独立实例身份。
pub use author_preview::{MapAuthorPreview, MapAuthorPreviewHandles};
pub use authoring_io::{ContentAuthorLock, content_hash}; // 共享作者入口。
pub use boundary_assets::*; // F12与动态地图运行时共享边界文件和固定预算。
pub use building_presets::*; // F12 与正式运行时共享同一建筑预设目录快照。
pub use celestial_lighting::*; // 场景、氛围与编辑器共享同一套天体配置解析规则。
pub use constants::*; // 编辑器创建目录时复用稳定文件名。
pub use courtyard_presets::*; // F12 印章和地图运行时共享稳定组合目录身份。
pub use distribution::*; // 编辑器预览与动态地图运行时不得产生不同布局。
pub use placement_transform::*; // 作者工具与正式候选共享实例数量和变换校验边界。
pub use editable_cuboid::{EDITABLE_CUBOID_EDGES, EDITABLE_CUBOID_FACES, EDITABLE_CUBOID_TRIANGLES};
pub use editable_mesh::{EditableMeshGeometry, MeshFace, MAX_EDITABLE_MESH_VERTICES, MAX_EDITABLE_MESH_FACES, MAX_EDITABLE_MESH_FACE_VERTICES, MAX_EDITABLE_MESH_EDGES};
pub use mesh_extrusion::{extruded_placement_record, extrusion_normal};
pub use placement_editor::*;
pub use placement_water::*; // F12作者工具与正式运行时共享同一网格水体类别、参数和校验边界。
pub use resources::*; // 消费者通过注册表查找私有资产句柄。
pub use sparse_height::*; // 创建器、运行时和未来独立编辑器共享同一高度清单与块格式。
pub use surface_crater::*; // 作者源异步载入和施法时不可变地表快照。
pub use surface_presets::*; // 动态地图运行时与 F12 共享同一预设快照。
pub use terrain_topology::*;
pub use terrain_objects::*;
pub use clipmap_stitches::*;
pub use types::*; // 跨crate共享稳定地图ID。
pub use vegetation_authoring::*; // F12作者工具共享图层组格式与严格预算校验。
pub use vegetation_presets::*; // 编辑器和运行时只通过同一目录解析稳定预设 ID。
pub use water_assets::*; // 编辑器与动态地图运行时共享水域文件格式和校验。
pub use water_presets::*; // F12与正式静水运行时共享同一水体外观快照。

use bevy::prelude::*; // App、Plugin与资产注册接口。

/// 注册地图目录、清单、传送图和显式实例热加载能力。
#[derive(Default)]
pub struct MapContentPlugin;

impl Plugin for MapContentPlugin {
    fn build(&self, app: &mut App) {
        map_instances::install(app);
        app.init_asset::<MapCatalogAsset>()
            .init_asset::<BuildingPresetCatalogAsset>() // 建筑数量和类别完全由全局 JSON 目录驱动。
            .init_asset::<CourtyardPresetCatalogAsset>() // 院落组合从独立全局目录热加载，不改变旧地图摆放格式。
            .init_asset::<VegetationPresetCatalogAsset>() // 开发者预设数量不绑定 Rust 枚举或固定数组。
            .init_asset::<SurfacePresetCatalogAsset>() // PBR 地表预设同样不绑定地图层数或硬编码按钮。
            .init_asset::<WaterPresetCatalogAsset>() // 水体预设数量与固定四波两法线GPU成本完全分离。
            .init_asset::<WorldEnvironmentAsset>() // 全局环境失败时注册表仍保留编译期安全默认值。
            .init_asset::<MapManifestAsset>()
            .init_asset::<SparseHeightManifestAsset>() // 新图稀疏清单经 AssetServer 受检加载，发布版无需工作区绝对路径。
            .init_asset::<MapVoxelTerrainSource>() // 独立体素通过同一资产源热加载，不依赖高度图。
            .init_asset_loader::<loaders::MapVoxelTerrainSourceLoader>()
            .init_asset::<SparseHeightTileAsset>() // 内容寻址块只按当前视图/碰撞需求申请句柄。
            .init_asset::<MapWaterMaskAsset>() // 原始R8作者源在Bevy IO阶段冻结，主线程不深拷贝整图。
            .init_asset::<MapPortalsAsset>()
            .init_asset::<MapPlacementsAsset>()
            .init_asset::<MapSurfaceAsset>()
            .init_asset::<MapSurfaceSourceAsset>() // 稀疏技能地形只读CPU作者四槽源。
            .init_asset::<MapVegetationAsset>()
            .init_asset::<MapWaterAsset>()
            .init_asset::<MapBoundariesAsset>()
            .init_asset_loader::<loaders::MapCatalogLoader>()
            .init_asset_loader::<loaders::BuildingPresetCatalogLoader>() // 固定入口支持首载、热更和失败保留旧快照。
            .init_asset_loader::<loaders::CourtyardPresetCatalogLoader>() // 组合目录在资产 IO 阶段有界读取。
            .init_asset_loader::<loaders::VegetationPresetCatalogLoader>() // 固定入口支持首次加载和热更新。
            .init_asset_loader::<loaders::SurfacePresetCatalogLoader>() // 目录和三张派生数组使用明确资产路径。
            .init_asset_loader::<loaders::WaterPresetCatalogLoader>() // 静水外观从固定全局JSON入口热加载。
            .init_asset_loader::<loaders::WorldEnvironmentLoader>()
            .init_asset_loader::<loaders::MapManifestLoader>()
            .init_asset_loader::<sparse_height::SparseHeightManifestLoader>() // 清单版本、路径地图 ID 和块引用一次校验。
            .init_asset_loader::<sparse_height::SparseHeightTileLoader>() // 二进制块执行有界读取、文件名哈希和头部校验。
            .init_asset_loader::<loaders::MapWaterMaskLoader>() // CPU建水读取作者源，KTX2继续保留为GPU派生纹理。
            .init_asset_loader::<loaders::MapPortalsLoader>()
            .init_asset_loader::<loaders::MapPlacementsLoader>()
            .init_asset_loader::<loaders::MapSurfaceLoader>()
            .init_asset_loader::<MapSurfaceSourceLoader>() // IO线程有界读取；不在施法帧访问文件。
            .init_asset_loader::<loaders::MapVegetationLoader>()
            .init_asset_loader::<loaders::MapWaterLoader>()
            .init_asset_loader::<loaders::MapBoundariesLoader>()
            .init_resource::<MapPuddleSaveHandoff>() // 保存后保持作者画面直到正式R8内容证明完成。
            .init_resource::<MapPuddleLiveOverride>() // 没有作者会话时默认为无覆盖，正式场景照常使用保存纹理。
            .init_resource::<SparseHeightLiveOverride>() // F12未保存块以MapId和修订覆盖正式Clipmap页。
            .init_resource::<ActiveDynamicMap>()
            .init_resource::<DynamicCelestialLightingPreview>() // F12 未保存预览通过共享小资源进入动态运行时。
            .add_systems(
                Startup,
                (
                    systems::load_map_catalog,
                    systems::load_world_environment,
                    systems::load_building_preset_catalog,
                    systems::load_courtyard_preset_catalog,
                    systems::load_vegetation_preset_catalog,
                    systems::load_surface_preset_catalog,
                    systems::load_water_preset_catalog,
                ),
            )
            .add_systems(
                Update,
                (
                    systems::apply_map_catalog,
                    systems::warn_map_catalog_load_failed,
                    systems::apply_world_environment,
                    systems::warn_world_environment_load_failed,
                    systems::apply_building_preset_catalog,
                    systems::warn_building_preset_catalog_load_failed,
                    systems::apply_courtyard_preset_catalog,
                    systems::warn_courtyard_preset_catalog_load_failed,
                    systems::apply_vegetation_preset_catalog,
                    systems::warn_vegetation_preset_catalog_load_failed,
                    systems::apply_surface_preset_catalog,
                    systems::warn_surface_preset_catalog_load_failed,
                    systems::apply_water_preset_catalog,
                    systems::warn_water_preset_catalog_load_failed,
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_id_rejects_paths_and_accepts_stable_ascii_ids() {
        assert!(MapId::parse("HN-01").is_ok());
        assert!(MapId::parse("draft_20260817").is_ok());
        assert!(MapId::parse("../HN-01").is_err());
        assert!(MapId::parse("开封").is_err());
    }

    #[test]
    fn catalog_entry_derives_private_asset_paths() {
        let entry = MapCatalogEntry {
            id: MapId::parse("HN-01").unwrap(),
            display_name: "开封城郊".to_owned(),
            folder: "maps/HN-01/".to_owned(),
            published: true,
            surface_path: None,
            vegetation_path: None,
            water_path: None,
            boundary_path: None,
            characters_path: None, // 旧地图没有追加角色作者文件。
        };
        assert_eq!(entry.manifest_path(), "maps/HN-01/map.map_manifest.json");
        assert_eq!(entry.portals_path(), "maps/HN-01/portals.map_portals.json");
        assert_eq!(
            entry.placements_path(),
            "maps/HN-01/placements.map_placements.json"
        );
    }

    #[test]
    fn legacy_manifest_without_environment_keeps_clear_sky_default() {
        let manifest: MapManifestAsset = serde_json::from_str(
            r#"{
                "schema_version": 1,
                "id": "MAP-LEGACY",
                "display_name": "旧动态地图",
                "runtime_scene": "DynamicMapScene",
                "world_min": [-32.0, -32.0],
                "world_max": [32.0, 32.0],
                "default_spawn": [0.0, 2.0, 0.0],
                "terrain": {
                    "kind": "heightfield",
                    "glb_path": "maps/MAP-LEGACY/terrain/terrain.glb",
                    "height_path": "maps/MAP-LEGACY/terrain/height.r16",
                    "resolution": [9, 9],
                    "base_height": -80.0,
                    "height_scale": 160.0
                },
                "asset_roots": []
            }"#,
        )
        .expect("旧清单缺少environment时必须继续反序列化"); // 向后兼容不能要求批量改写已有用户地图。
        assert_eq!(manifest.environment, MapEnvironmentDescriptor::default()); // 默认无天空盒并保留可编辑亮度。
    }

    #[test]
    fn legacy_collision_modes_migrate_to_single_shared_lod_format() {
        let mut terrain: MapTerrainDescriptor = serde_json::from_value(serde_json::json!({
            "kind": "sparse_clipmap",
            "glb_path": "",
            "height_path": "maps/MAP-old/terrain/height_tiles.map_height_manifest.json",
            "resolution": [513, 513],
            "base_height": -80.0,
            "height_scale": 160.0
        }))
        .unwrap(); // 旧稀疏清单没有新字段时必须继续运行原分块方案。
        assert_eq!(
            terrain.collision_mode,
            MapClipmapCollisionMode::LayeredSharedChunks
        );
        assert!(
            serde_json::to_value(&terrain)
                .unwrap()
                .get("collision_mode")
                .is_none()
        ); // 旧地图重存不被动改写物理路线。
        terrain.collision_mode = MapClipmapCollisionMode::LayeredSharedChunks; // 新选择必须显式持久化。
        let encoded = serde_json::to_value(&terrain).unwrap();
        assert!(encoded.get("collision_mode").is_none()); // 唯一模式不再输出旧路线字段。
        assert_eq!(encoded["collision_lods"][1]["spacing_multiplier"], 2); // 小地图缺字段初始倍率适配最高父页。
        let decoded: MapTerrainDescriptor = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.collision_mode, terrain.collision_mode); // 重进地图保持作者选择。
    }

    #[test]
    fn sparse_clipmap_cannot_enter_the_glb_derived_water_pipeline() {
        assert!(MapTerrainKind::Heightfield.supports_glb_derived_water()); // 成熟路线继续建立正式水域交接。
        assert!(!MapTerrainKind::SparseClipmap.supports_glb_derived_water()); // 新路线不能进入完整GLB河床构建器。
        assert!(!MapTerrainKind::Gltf.supports_glb_derived_water()); // 专用旧场景同样不冒充动态GLB高度场。
        assert!(MapTerrainKind::Heightfield.supports_static_water());
        assert!(MapTerrainKind::SparseClipmap.supports_static_water()); // 分页高度路线正式允许湖泊与池塘。
        assert!(MapTerrainKind::Heightfield.supports_spline_water());
        assert!(MapTerrainKind::SparseClipmap.supports_spline_water()); // 世界XYZ Ribbon不依赖完整GLB河床。
        assert!(!MapTerrainKind::Gltf.supports_spline_water()); // 专用旧场景仍不进入数据驱动样条水。
    }

    #[test]
    fn map_environment_rejects_non_cube_asset_path_shapes_before_loading() {
        let valid = MapEnvironmentDescriptor {
            sky_mode: MapSkyMode::Skybox, // 主资源在天空盒模式下同时接受可见 Cube 与 IBL 校验。
            skybox_path: Some("textures/test_skybox.ktx2".to_owned()), // 测试只校验路径形状，不依赖已删除的示例天空资产。
            skybox_brightness: 1_200.0, // 有限非负亮度可进入世界时间计算。
            environment_diffuse_intensity: 300.0, // 有限非负补光独立进入PBR漫反射计算。
            environment_specular_intensity: 800.0, // 有限非负反射独立进入PBR镜面计算。
            skybox_yaw_degrees: -15.0,  // 负角由运行时归一化但仍是合法作者值。
            weather: MapWeatherDescriptor::default(), // 测试天空参数时仍提供完整合法天气配置。
            celestial_overrides: MapCelestialLightingOverrides::default(), // 测试天空路径时完整跟随全局天体配置。
        };
        assert!(valid.validate().is_ok()); // 图像内容的Cube验证留给加载完成后的统一运行时。
        assert_eq!(valid.normalized_skybox_yaw_degrees(), 345.0); // 等价角必须稳定折回零至三百六十度。
        let invalid = MapEnvironmentDescriptor {
            skybox_path: Some("textures/sky.png".to_owned()), // 普通二维PNG不能成为天空盒。
            ..valid
        };
        assert!(invalid.validate().is_err()); // UI和运行时必须在AssetServer请求前拒绝明显错误格式。
        let derived = MapEnvironmentDescriptor {
            skybox_path: Some("textures/test_skybox_specular.ktx2".to_owned()), // 派生图不能充当主天空入口。
            ..MapEnvironmentDescriptor::default()
        };
        assert!(derived.validate().is_err()); // 防止运行时继续派生重复后缀的不存在路径。
    }

    #[test]
    fn legacy_environment_defaults_to_neutral_weather_profile() {
        let environment: MapEnvironmentDescriptor =
            serde_json::from_str(r#"{"skybox_brightness":1000.0,"skybox_yaw_degrees":0.0}"#)
                .expect("旧环境字段必须自动补齐正式天气配置"); // 向后兼容不要求批量改写已有作者地图。
        assert_eq!(environment.weather.region, MapWeatherRegion::Global); // 缺字段不再恢复原游戏地域。
        assert_eq!(environment.weather.climate, MapWeatherClimate::Uniform); // 缺省使用中性统一气候。
        assert_eq!(
            environment.environment_diffuse_intensity,
            DEFAULT_MAP_ENVIRONMENT_DIFFUSE_INTENSITY
        ); // 未声明过旧单字段的环境获得克制天空补光默认值。
        assert_eq!(
            environment.environment_specular_intensity,
            DEFAULT_MAP_ENVIRONMENT_SPECULAR_INTENSITY
        ); // 未声明过旧单字段的环境保留完整天空反射默认值。
        let encoded = serde_json::to_value(MapEnvironmentDescriptor::default())
            .expect("新地图环境必须可序列化"); // 新建和保存事务都使用同一Serde格式。
        assert_eq!(encoded["sky_mode"], "dynamic"); // 新保存文件明确记录互斥天空来源。
        assert_eq!(encoded["environment_diffuse_intensity"], 300.0); // 新保存文件明确记录人物与地形补光。
        assert_eq!(encoded["environment_specular_intensity"], 1_000.0); // 新保存文件明确记录水面与材质反射。
        assert!(encoded.get("environment_light_intensity").is_none()); // 旧单字段只能读取，不能继续扩散到新作者文件。
        assert_eq!(encoded["weather"]["region"], "global"); // 新保存只写中性身份。
        assert_eq!(encoded["weather"]["climate"], "uniform"); // 旧地域只能读取，不能扩散到新文件。
    }

    #[test] // 旧地图不需要补写备注，新地图多行备注按原文往返。
    fn map_manifest_notes_are_optional_and_preserve_newlines() {
        let mut manifest: MapManifestAsset = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "id": "MAP-notes", "display_name": "备注测试",
            "runtime_scene": "DynamicMapScene", "world_min": [-32.0, -32.0],
            "world_max": [32.0, 32.0], "default_spawn": [0.0, 2.0, 0.0],
            "terrain": {"kind": "heightfield", "glb_path": "maps/MAP-notes/terrain.glb",
                "resolution": [9, 9], "base_height": -80.0, "height_scale": 160.0},
            "asset_roots": []
        })).expect("清单没有备注仍须直接加载"); // 测试不依赖任何可被作者删除的实际地图。
        assert!(manifest.notes.is_empty()); // 旧地图以空备注展示。
        let encoded = serde_json::to_value(&manifest).unwrap(); // 旧清单重存不增加空占位。
        assert!(encoded.get("notes").is_none()); // 可选字段只有明确填写才出现。
        manifest.notes = "河畔入口\n需补路标".to_owned(); // 模拟作者新增两行自由文本。
        let encoded = serde_json::to_value(&manifest).unwrap(); // 私有清单持久化不复制到目录。
        assert_eq!(encoded["notes"], "河畔入口\n需补路标"); // 多行内容不被压平。
        let decoded: MapManifestAsset = serde_json::from_value(encoded).unwrap(); // 热加载沿用同一协议。
        assert_eq!(decoded.notes, manifest.notes); // 重启后可继续编辑原备注。
    }

    #[test]
    fn legacy_environment_without_celestial_fields_follows_current_global_profile() {
        let environment: MapEnvironmentDescriptor =
            serde_json::from_str(r#"{"skybox_brightness":1000.0,"skybox_yaw_degrees":0.0}"#)
                .expect("旧环境缺少天体覆盖字段时必须继续反序列化"); // 旧地图不需要升级格式或补写空字段。
        assert!(environment.celestial_overrides.is_empty()); // 缺失字段等价于六组属性全部跟随全局。

        let mut global = CelestialLightingProfile::default();
        global.day_base_illuminance_lux = 12_345.0; // 模拟用户在 F12 修改并保存新的全局基准。
        global.azimuth_offset_degrees = -30.0; // 解析结果还必须执行统一的方位角归一化。
        let resolved = resolve(&global, &environment.celestial_overrides);
        assert_eq!(resolved.day_base_illuminance_lux, 12_345.0); // 旧地图会立即采用最新全局照度。
        assert_eq!(resolved.azimuth_offset_degrees, 330.0); // 旧地图与新地图使用相同的最终解析规则。
    }

    #[test]
    fn legacy_environment_light_intensity_is_copied_to_both_new_channels() {
        let environment: MapEnvironmentDescriptor = serde_json::from_str(
            r#"{"skybox_brightness":1000.0,"environment_light_intensity":650.0,"skybox_yaw_degrees":0.0}"#,
        )
        .expect("旧单一环境强度必须继续可读"); // 已有用户地图无需手工迁移即可保持原外观。
        assert_eq!(environment.environment_diffuse_intensity, 650.0); // 旧值继续控制原有天空补光。
        assert_eq!(environment.environment_specular_intensity, 650.0); // 旧值同时继续控制原有天空反射。
    }

    #[test]
    fn sky_mode_defaults_to_dynamic_and_skybox_requires_a_main_cube() {
        let legacy: MapEnvironmentDescriptor = serde_json::from_str(
            r#"{"skybox_path":"textures/test_skybox.ktx2","skybox_brightness":1000.0,"skybox_yaw_degrees":0.0}"#,
        )
        .expect("缺少天空模式的旧地图必须继续读取"); // 旧主路径仅继续提供 IBL，不会突然恢复烘焙背景。
        assert_eq!(legacy.sky_mode, MapSkyMode::Dynamic); // 兼容默认必须保持此前已经验收的动态天空。
        assert!(legacy.validate().is_ok()); // 动态天空允许保留或不设置 IBL 身份。

        let invalid = MapEnvironmentDescriptor {
            sky_mode: MapSkyMode::Skybox, // 显式请求可见 Cube。
            skybox_path: None,            // 但没有可供显示和派生 IBL 的主资源。
            ..default()
        };
        assert!(invalid.validate().is_err()); // 半套天空盒配置不得进入保存或运行时加载。
    }

    #[test]
    fn empty_project_catalog_round_trips_without_implicit_maps() {
        // 公共合同使用自包含空目录；不以私有游戏地图充当SDK的默认数据。
        let catalog: MapCatalogAsset = serde_json::from_str(
            r#"{"schema_version":1,"maps":[]}"#,
        )
        .expect("空工程地图目录必须可解析");
        assert_eq!(catalog.schema_version, MAP_SCHEMA_VERSION);
        assert!(catalog.maps.is_empty());
        let reopened: MapCatalogAsset = serde_json::from_slice(
            &serde_json::to_vec(&catalog).expect("空目录必须可保存"),
        )
        .expect("空目录必须可重新读取");
        assert!(reopened.maps.is_empty());
    }

    #[test]
    fn portal_target_without_portal_id_round_trips_as_default_spawn() {
        let target = MapPortalTarget::default_spawn(
            MapId::parse("MAP-default-spawn").expect("测试目标地图ID必须合法"), // 使用真实稳定MapId构造默认出生点目标。
        );
        let encoded = serde_json::to_value(&target).expect("默认出生点目标必须可序列化"); // 作者保存仍走统一Serde数据格式。
        assert_eq!(encoded["map_id"], "MAP-default-spawn"); // 目标地图身份必须始终显式写入JSON。
        assert!(encoded.get("portal_id").is_none()); // 默认出生点不使用空字符串或保留魔法ID伪装传送点。
        let decoded: MapPortalTarget =
            serde_json::from_value(encoded).expect("默认出生点目标必须可重新载入"); // 热更新读取必须恢复相同目标语义。
        assert_eq!(decoded, target); // 保存和热加载不得把默认出生点变成普通传送点。
        assert!(decoded.uses_default_spawn()); // 公共判定方法必须与序列化语义一致。
    }
}
pub mod author_visuals;
