//! 目录驱动地形的 Solari 资源预算、停止线和诊断路径。

use bevy::diagnostic::DiagnosticPath; // 诊断路径使用const哈希，稳定帧查询不重复构造或分配字符串。

pub(crate) const DYNAMIC_EXPECTED_INSTANCE_COUNT: u32 = 1;
/// 动态目标只允许唯一地形材质资产，避免同一地形被意外拆成多个BLAS实例。
pub(crate) const DYNAMIC_EXPECTED_UNIQUE_MATERIAL_COUNT: u32 = 1;
/// 动态场景单个规则高度场的顶点上限，覆盖513×513采样；小场景可低于此值。
pub(crate) const DYNAMIC_MAX_TERRAIN_VERTEX_COUNT: u64 = 513 * 513;
/// 相同网格的三角形上限；这是预算，不要求场景具有固定几何数量。
pub(crate) const DYNAMIC_MAX_TERRAIN_TRIANGLE_COUNT: u64 = 512 * 512 * 2;
/// 最大地形派生为位置、法线、切线、UV和索引后的载荷。
pub(crate) const DYNAMIC_MAX_TERRAIN_DERIVED_BYTES: u64 =
    DYNAMIC_MAX_TERRAIN_VERTEX_COUNT * 48 + DYNAMIC_MAX_TERRAIN_TRIANGLE_COUNT * 3 * 4;

pub(crate) const MAX_INSTANCE_COUNT: u32 = 16; // P1 TLAS实例与BLAS硬上限。
pub(crate) const MAX_VERTEX_COUNT: u64 = 320_000; // 派生前先以源顶点数整体拒绝预算外资产。
pub(crate) const MAX_TRIANGLE_COUNT: u64 = 600_000; // 派生前先以源三角形数整体拒绝预算外资产。
pub(crate) const MAX_UNIQUE_MATERIAL_COUNT: u32 = 16; // 防止GLB材质意外拆分扩大绑定数组。
/// Solari运行时最多接管的已加载光栅材质资产数；初始捕获访问到第257项即整体停止。
pub(crate) const MAX_DEFERRED_RASTER_MATERIAL_COUNT: usize = 256;
/// 全部兼容材质在RenderApp确认为Deferred后，再保留两个完整光栅帧供管线专用化稳定。
pub(crate) const REQUIRED_DEFERRED_RASTER_READY_FRAMES: u8 = 2;
pub(crate) const MAX_DERIVED_MESH_BYTES: u64 = 32 * 1024 * 1024; // 全部派生几何CPU载荷硬上限。
pub(crate) const MAX_CPU_WORKING_SET_BYTES: u64 = 64 * 1024 * 1024; // 捕获、在途任务和完成结果合计硬上限。
pub(crate) const MAX_CAPTURE_BYTES_PER_FRAME: usize = 2 * 1024 * 1024; // 主线程每帧最多复制2MiB源数据。
/// 单项采用上限覆盖18.05MiB的完整白模地形，同时仍保持每帧最多登记一个Mesh。
pub(crate) const MAX_UPLOAD_BYTES_PER_FRAME: u64 = 20 * 1024 * 1024;
/// 编译期同时锁住资产载荷与采用门，避免只运行非测试构建时再次生成必然无法激活的配置。
const _: () = assert!(DYNAMIC_MAX_TERRAIN_DERIVED_BYTES <= MAX_UPLOAD_BYTES_PER_FRAME);
pub(crate) const MAX_ASYNC_TASKS: usize = 2; // 给管线编译与其他项目后台任务保留异步池容量。
pub(crate) const STAGE_TIMEOUT_SECONDS: f32 = 30.0; // 任一冷启动阶段超时都完整回退。
pub(crate) const REQUIRED_WARMUP_RENDER_FRAMES: u8 = 2; // 必须观察两个不同的完整RenderApp帧。

pub(crate) const CAMERA_TELEPORT_DISTANCE_METERS: f32 = 12.0; // 与项目TAA历史失效口径一致。
pub(crate) const CAMERA_CUT_ANGLE_RADIANS: f32 = std::f32::consts::FRAC_PI_4; // 四十五度镜头切换。
pub(crate) const CELESTIAL_LIGHT_RELATIVE_JUMP: f32 = 0.20; // 单帧照度变化超过20%视为天体光突变。
pub(crate) const CELESTIAL_LIGHT_REFERENCE_FLOOR_LUX: f32 = 1_000.0; // 晨昏近零照度以固定基准避免连续变化被误判为百分比突变。
pub(crate) const CELESTIAL_LIGHT_COLOR_JUMP: f32 = 0.15; // 线性色差超过该值清空时间历史。
pub(crate) const CELESTIAL_LIGHT_DIRECTION_JUMP_RADIANS: f32 = std::f32::consts::PI / 12.0; // 单帧方向跳变超过十五度时丢弃旧光照历史。

pub(crate) const MAX_INCREMENTAL_VRAM_MIB: u32 = 1024; // P1实测增量显存停止线。

pub(crate) static PRESAMPLE_LIGHT_TILES_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/solari_lighting/presample_light_tiles/elapsed_gpu"); // 光源瓦片预采样GPU毫秒。
pub(crate) static SOLARI_LIGHTING_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/solari_lighting/lighting/elapsed_gpu"); // Bevy 0.20合并后的直接光、间接光与反射。
pub(crate) static WORLD_CACHE_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/solari_lighting/world_cache/elapsed_gpu"); // 世界缓存维护GPU毫秒。
pub(crate) static WORLD_CACHE_ACTIVE_CELLS_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/solari_lighting/world_cache_active_cells_count"); // Bevy已有的活跃世界缓存cell计数，不新增GPU回读。
pub(crate) static DLSS_RAY_RECONSTRUCTION_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/dlss_ray_reconstruction/elapsed_gpu"); // NVIDIA RR重建与降噪GPU毫秒。
pub(crate) static DLSS_RAY_RECONSTRUCTION_CPU_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/dlss_ray_reconstruction/elapsed_cpu"); // 覆盖同步NGX Evaluate调用与命令录制的Render线程CPU毫秒。
pub(crate) static EARLY_PREPASS_GPU_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/early prepass/elapsed_gpu"); // 汇总深度、运动矢量、Deferred GBuffer及GPU草地预通道耗时。
pub(crate) static MAIN_OPAQUE_PASS_GPU_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/main_opaque_pass_3d/elapsed_gpu"); // 汇总主不透明与AlphaMask颜色通道耗时。
pub(crate) static MAIN_TRANSPARENT_PASS_GPU_DIAGNOSTIC: DiagnosticPath =
    DiagnosticPath::const_new("render/main_transparent_pass_3d/elapsed_gpu"); // 汇总主透明颜色通道耗时，无透明项时允许缺失。
