//! 大地图公共能力入口。
//! 这里只存放可被多张地图复用的场景运行时、GLB地形、动态水和热重载能力。

pub(crate) mod authoring_version; // 保存交接共享O(1)作者版本票据，避免摆放和水域相互依赖。
pub(crate) mod building_vegetation_clearance; // 正式摆放与 F12 草稿共用不可变建筑植被净空。
pub(crate) mod hot_reload; // 所有地图共用的GLB修改监听与碰撞重建流程。
pub mod large_map; // 动态地图共享的地形、水域和材质能力。
pub(crate) mod mesh_lod; // 普通静态网格共享质量策略，专用地形与植被后端保持独立。
pub(crate) mod placement_data; // 每图显式GLB/GLTF实例JSON与热更新原子重建。
pub(crate) mod portal_data; // 地图私有JSON传送图、跨地图目标解析与热更新原子重建。
pub(crate) mod rain_shelter; // glTF显式屋顶代理与无碰撞动态遮雨生命周期。
pub(crate) mod scene_detail_quality; // 树木、灌木与岩石视觉 LOD 和最远范围的持久画质桥接。
pub(crate) mod scene_residency;
pub(crate) mod scene_runtime; // 场景环境、传送门、NPC和启动门公共运行时。
pub(crate) mod surface_weather; // 独立积水作者图、区域湿润和地表/河湖共享GPU桥。
pub(crate) mod vegetation_quality; // 全地图 GPU 叶片植被质量与持久画质设置桥接。
pub(crate) mod vegetation_weather; // 天气连续风力到草、树与网格植被的唯一风参数桥。
pub(crate) mod world_asset_lifecycle; // WorldAsset热重载复用InstanceId时仍以资产epoch定向撤销旧Ready。
