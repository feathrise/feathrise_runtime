//! R8覆盖驱动的动态水面纯网格构建与ECS安装入口。
//!
//! 【功能设计】完整遮罩降采样、岸边距离和索引生成都在调用方的有界后台任务执行；
//! 主线程只把已经完成的连续数组移动进Mesh资产并原子替换实体。编辑器预览和正式动态地图使用同一入口。

use bevy::camera::primitives::Aabb; // 后台完成值携带局部包围盒，主线程安装时无需再次扫描顶点。
use bevy::prelude::*; // 水面身份组件、Vec2与纯网格连续数组。
use feathrise_map_content::{
    MIN_STABLE_WATER_DEPTH_METERS, MapId, MapWaterLayer, WATER_MACRO_WAVE_COUNT,
    WATER_MICRO_NORMAL_COUNT, WaterMacroWave, WaterMicroNormalLayer, WaterPresetDescriptor,
}; // 作者材质参数、固定波形与稳定水面身份。

#[cfg(test)]
use super::footprint::WaterInteractionFootprint; // 子模块回归继续通过父作用域构造真实互动足迹。
use super::footprint::{PaintedWaterFootprint, RibbonWaterFootprint}; // 纯构建值携带水平网格或斜河互动足迹。


mod shore; // R8与R16联合稳定核心、局部裁剪和有限米制视觉岸带。
mod spawn; // 主线程候选资产登记、GPU预热承接与真实互动实体生成。
mod terrain; // 正式运行时与F12共用的不可变R16高度输入。
mod upload_chunks;
pub use upload_chunks::chunk_water_surfaces;

use shore::{
    build_shore_vertex_alpha, build_stable_water_core, downsample_stable_coverage,
    expanded_active_bounds,
}; // 网格入口只负责编排纯后台步骤，不重复维护岸线算法。
pub use spawn::{
    PaintedWaterSurfaceSpawn, StagedWaterSurface, advance_water_surface_material,
    create_water_surface_material, spawn_painted_water_surface, spawn_staged_water_surface,
    spawn_staged_water_warmup, spawn_water_surface, stage_water_surface_assets,
    stage_water_surface_chunk,
}; // 保持编辑器与运行时既有公开路径，同时把ECS安装职责隔离到独立子模块。
pub use terrain::PaintedWaterTerrainInput; // 动态地图和F12通过场景crate稳定入口共享同一高度快照契约。

/// 绘制水面实体用途；编辑器据此隐藏同层旧运行时水面，避免透明重叠。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintedWaterPurpose {
    /// 从已保存KTX2热加载生成的正式动态地图水面。
    Runtime,
    /// F12尚未保存或等待热更新期间显示的作者预览。
    EditorPreview,
}

/// 统一水面实体的正式几何类别；编辑器可用同一marker隐藏湖、河、急流与瀑布。
#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 纯身份枚举随实体marker复制，不持有作者资产。
pub enum WaterSurfaceKind {
    Still,     // R8湖泊与池塘继续使用水平规则网格。
    River,     // 三维样条河流可包含普通河段和急流段。
    Waterfall, // 近竖直幕布只参与视觉动画，不参与玩家波源。
} // 结束统一水面几何类别定义。

/// 一张统一水面的稳定身份、材质参数、变换和互动策略。
#[derive(Clone, Debug)] // 后台完成值携带小型描述，主线程无需重新读取作者资产。
pub struct WaterSurfaceDescriptor {
    pub id: String,                  // 地图内稳定水体ID，兼容原layer_id marker字段。
    pub kind: WaterSurfaceKind,      // 湖、河或瀑布的正式几何类别。
    pub base_color_srgba: [f32; 4],  // 当前水体标准PBR基础颜色和透明度。
    pub perceptual_roughness: f32,   // 当前水体标准PBR感知粗糙度。
    pub reflectance: f32,            // 非金属水面的StandardMaterial反射率。
    pub trough_darkening: f32,       // 玩家波谷对原水色的最大压暗比例。
    pub shore_wave_attenuation: f32, // 静水岸带顶点alpha转为波幅时使用的指数。
    pub macro_waves: [WaterMacroWave; WATER_MACRO_WAVE_COUNT], // 固定四组风浪与水流速度完全分离。
    pub micro_normals: [WaterMicroNormalLayer; WATER_MICRO_NORMAL_COUNT], // 固定两层逐像素滚动法线参数。
    pub micro_normal_textures: [Option<Handle<Image>>; WATER_MICRO_NORMAL_COUNT], // 主世界加载完成的可选强句柄。
    pub flow_direction: Vec2, // 湖面统一XZ流向；样条水从逐顶点切线与静止法线读取方向。
    pub flow_speed: f32,      // 湖面基础流速；样条网格主要读取UV1逐点流速。
    pub translation_y: f32,   // R8静水通过实体Y平移表达绝对水位，样条水保持零。
    pub rest_water_level: f32, // 水平水体的局部静止水位；斜河由带状足迹逐点返回。
    pub interaction_enabled: bool, // 瀑布明确为false，保证不会发射或淘汰玩家八槽波。
} // 结束统一水面描述定义。

impl WaterSurfaceDescriptor {
    /// 从现有静水层建立保持旧资产身份和绝对水位语义的统一描述。
    pub fn from_layer(layer: &MapWaterLayer) -> Self {
        let raw_direction = Vec2::from_array(layer.flow_direction); // 作者XZ数组转换为Shader与组件共用向量。
        let flow_direction = if raw_direction.length_squared() <= f32::EPSILON {
            Vec2::X // 零向量继续使用历史东西向默认值。
        } else {
            raw_direction.normalize() // 非零作者流向在CPU归一化，GPU不接收幅度污染方向。
        };
        Self {
            id: layer.id.clone(),                     // 静水稳定层ID直接成为统一水体ID。
            kind: WaterSurfaceKind::Still,            // 现有R8层保持湖泊/池塘水平水体类别。
            base_color_srgba: layer.base_color_srgba, // 复用当前正式水色与透明度。
            perceptual_roughness: layer.perceptual_roughness, // 复用当前正式水面粗糙度。
            reflectance: 0.5, // 旧文件没有独立反射字段，保留StandardMaterial常用非金属基准。
            trough_darkening: super::constants::PLAYER_RIPPLE_TROUGH_DARKENING,
            shore_wave_attenuation: 1.0,
            macro_waves: legacy_macro_waves(), // 旧水层继续得到固定成本背景缓波。
            micro_normals: legacy_micro_normals(), // 旧水层没有微法线资源，因此两层强度均为零。
            micro_normal_textures: [None, None],
            flow_direction,                   // 发布归一化静水流向。
            flow_speed: layer.flow_speed,     // 发布受检静水流速。
            translation_y: layer.water_level, // 实体平移继续表达绝对世界Y水位。
            rest_water_level: 0.0,            // 水面子实体局部静止高度保持零。
            interaction_enabled: true,        // 湖泊与池塘继续允许踏水和跳跃波纹。
        }
    }

    /// 从静水层和当前全局预设快照建立完整描述；目录别名在调用前已经规范化。
    pub fn from_layer_with_preset(
        layer: &MapWaterLayer,
        preset: &WaterPresetDescriptor,
        textures: [Option<Handle<Image>>; WATER_MICRO_NORMAL_COUNT],
    ) -> Self {
        let mut descriptor = Self::from_layer(layer);
        descriptor.base_color_srgba = preset.base_color_srgba;
        descriptor.perceptual_roughness = preset.perceptual_roughness;
        descriptor.reflectance = preset.reflectance;
        descriptor.trough_darkening = preset.trough_darkening;
        descriptor.shore_wave_attenuation = preset.shore_wave_attenuation;
        descriptor.macro_waves = preset.macro_waves;
        descriptor.micro_normals = preset.micro_normals.clone();
        descriptor.micro_normal_textures = textures;
        descriptor
    }

    /// 根据本外观最大风浪和固定玩家波源上限派生不会露底的安全水深。
    pub fn minimum_stable_depth(&self) -> f32 {
        let macro_depth = self
            .macro_waves
            .iter()
            .map(|wave| wave.amplitude_meters)
            .sum::<f32>();
        MIN_STABLE_WATER_DEPTH_METERS
            .max(macro_depth + super::constants::PLAYER_RIPPLE_MAX_HEIGHT + 0.02)
    }
}

/// 旧水文件、河流和Blender整网格使用的固定缓波；新静水由目录完全覆盖。
pub(crate) fn legacy_macro_waves() -> [WaterMacroWave; WATER_MACRO_WAVE_COUNT] {
    [
        WaterMacroWave {
            direction: [1.0, 0.33],
            amplitude_meters: super::constants::PRIMARY_WAVE_AMPLITUDE,
            wavelength_meters: 56.0,
            speed_meters_per_second: 1.0,
            steepness: 0.0,
        },
        WaterMacroWave {
            direction: [0.32, -1.0],
            amplitude_meters: super::constants::CROSSING_WAVE_AMPLITUDE,
            wavelength_meters: 42.0,
            speed_meters_per_second: 0.7,
            steepness: 0.0,
        },
        WaterMacroWave {
            direction: [-0.8, 1.0],
            amplitude_meters: 0.0,
            wavelength_meters: 18.0,
            speed_meters_per_second: 0.4,
            steepness: 0.0,
        },
        WaterMacroWave {
            direction: [-1.0, -0.2],
            amplitude_meters: 0.0,
            wavelength_meters: 8.0,
            speed_meters_per_second: 0.25,
            steepness: 0.0,
        },
    ]
}

/// 未配置滚动图片时的两层安全中性参数。
pub(crate) fn legacy_micro_normals() -> [WaterMicroNormalLayer; WATER_MICRO_NORMAL_COUNT] {
    std::array::from_fn(|_| WaterMicroNormalLayer {
        texture_path: None,
        tile_meters: 1.0,
        velocity_meters_per_second: [0.0, 0.0],
        strength: 0.0,
    })
}

/// 一张已安装绘制水面的稳定身份。
#[derive(Component, Clone, Debug)]
pub struct PaintedWaterSurface {
    /// 所属地图ID。
    pub map_id: MapId,
    /// 所属水域层ID。
    pub layer_id: String,
    /// 湖、河或瀑布类别；同一marker覆盖全部正式与作者预览水体。
    pub kind: WaterSurfaceKind,
    /// 正式运行时或编辑器预览用途。
    pub purpose: PaintedWaterPurpose,
}

/// 后台生成完成、尚未登记Bevy资产的连续水面数据。
#[derive(Clone, Debug)]
pub struct PaintedWaterMeshData {
    pub(super) local_aabb: Option<Aabb>, // 后台从静止顶点计算；空层没有可定位几何。
    pub(super) positions: Vec<[f32; 3]>, // 带状纯构建器在同一water模块内写入顶点位置。
    pub(super) normals: Vec<[f32; 3]>,   // 带状纯构建器在同一water模块内写入静止法线。
    pub(super) tangents: Option<Vec<[f32; 4]>>, // 静水为None，样条水携带局部流向切线。
    pub(super) uvs: Vec<[f32; 2]>,       // UV0统一作为水材质主采样坐标。
    pub(super) uvs_b: Option<Vec<[f32; 2]>>, // UV1仅由样条水编码逐点流速和带身份哨兵的样式。
    pub(super) colors: Vec<[f32; 4]>,    // 顶点alpha继续控制湖岸或瀑布侧边淡出。
    pub(super) indices: Vec<u32>,        // 后台任务完成的U32三角索引。
    pub(super) footprint: PaintedWaterFootprint, // 静水规则网格的精确可见足迹。
    pub(super) ribbon_footprint: Option<RibbonWaterFootprint>, // 斜河实际Y与作者深度足迹。
}

/// 统一水网络公开沿用现有连续Mesh数据类型，避免编辑器与运行时维护两份格式。
pub type WaterSurfaceMeshData = PaintedWaterMeshData; // 兼容原Painted命名，同时公开正式统一语义。

impl PaintedWaterMeshData {
    /// 独立水画布沿用同一R8几何、材质和互动足迹；没有地形时明确省略河床裁切。
    /// 调用方仍须在唯一后台任务中构建，不能为此创建假高度图或复制地形。
    pub fn from_independent_mask(
        mask: &[u8], resolution: [u32; 2], world_min: [f32; 2], world_max: [f32; 2], water_level: f32,
    ) -> Result<Self, String> {
        build_painted_water_mesh_inner(mask, resolution, world_min, world_max, None, water_level, MIN_STABLE_WATER_DEPTH_METERS)
    }

    /// 空遮罩不创建无意义的透明实体或GPU资产。
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// 返回用于诊断和测试的实际三角形数量。
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// 估算本次主世界到RenderApp的连续顶点和索引上传字节，供共享帧预算背压。
    pub fn estimated_gpu_bytes(&self) -> usize {
        self.positions.len() * std::mem::size_of::<[f32; 3]>()
            + self.normals.len() * std::mem::size_of::<[f32; 3]>()
            + self
                .tangents
                .as_ref()
                .map_or(0, |values| values.len() * std::mem::size_of::<[f32; 4]>())
            + self.uvs.len() * std::mem::size_of::<[f32; 2]>()
            + self
                .uvs_b
                .as_ref()
                .map_or(0, |values| values.len() * std::mem::size_of::<[f32; 2]>())
            + self.colors.len() * std::mem::size_of::<[f32; 4]>()
            + self.indices.len() * std::mem::size_of::<u32>()
    }

    /// 从固定预算样条带状构建器接收完整连续数组和同源互动足迹。
    pub(super) fn from_ribbon(
        positions: Vec<[f32; 3]>, // 三维河流或瀑布幕布静止顶点。
        normals: Vec<[f32; 3]>,   // 每个顶点的静止带状法线。
        tangents: Vec<[f32; 4]>,  // xyz编码局部流向，w保持PBR切线手性一。
        uvs: Vec<[f32; 2]>,       // UV0.x为累计米，UV0.y为横向零至一。
        uvs_b: Vec<[f32; 2]>,     // UV1.x为局部流速，UV1.y为高位样条身份加河流/急流/瀑布样式。
        colors: Vec<[f32; 4]>,    // 顶点颜色alpha允许瀑布两侧边缘淡出。
        indices: Vec<u32>,        // 固定三角列表索引。
        ribbon_footprint: Option<RibbonWaterFootprint>, // 河流携带实际斜面足迹，瀑布传None禁用互动。
    ) -> Self {
        let local_aabb = local_aabb_from_positions(&positions); // 样条后台任务顺手缓存静止网格范围。
        Self {
            local_aabb,               // 安装实体时直接插入Aabb，禁止主线程重扫带状顶点。
            positions,                // 移交完整三维顶点数组。
            normals,                  // 移交完整静止法线数组。
            tangents: Some(tangents), // 带状网格始终提供局部流向切线。
            uvs,                      // 移交累计米与横向UV。
            uvs_b: Some(uvs_b),       // 带状网格始终提供逐点流速与带身份哨兵的样式。
            colors,                   // 移交透明边和PBR乘色。
            indices,                  // 移交完整受检三角索引。
            footprint: PaintedWaterFootprint::empty([0.0, 0.0], [0.0, 0.0]), // 带状水不用R8规则网格足迹。
            ribbon_footprint, // 河流Some、瀑布None的互动所有权明确分离。
        }
    }
}

/// 在既有后台网格构建阶段计算静止顶点局部包围盒；空几何保持None。
fn local_aabb_from_positions(positions: &[[f32; 3]]) -> Option<Aabb> {
    let first = Vec3::from_array(*positions.first()?); // 第一顶点同时初始化最小值与最大值。
    let (minimum, maximum) = positions.iter().skip(1).fold(
        (first, first), // 单顶点网格也得到合法零半径包围盒。
        |(minimum, maximum), position| {
            let position = Vec3::from_array(*position); // 连续数组值转换为Bevy三维向量。
            (minimum.min(position), maximum.max(position)) // 单遍累计每轴有限范围。
        },
    );
    Some(Aabb::from_min_max(minimum, maximum)) // Aabb随后随完成值跨线程移交给主世界。
}

/// 后台统一水网络构建返回的小型描述和完整Mesh连续数据。
#[derive(Clone, Debug)] // single-flight任务可原子返回整张地图全部水体。
pub struct BuiltWaterSurface {
    pub descriptor: WaterSurfaceDescriptor, // 主线程生成实体所需全部小型身份与材质参数。
    pub mesh: PaintedWaterMeshData,         // 后台完成且尚未登记Assets的连续网格数据。
} // 结束统一水面后台完成值定义。

/// 从完整R8作者遮罩和同修订R16地形构建固定预算水面；调用方必须在有界后台任务中执行。
pub fn build_painted_water_mesh(
    mask: &[u8],
    resolution: [u32; 2],
    world_min: [f32; 2],
    world_max: [f32; 2],
    terrain: &PaintedWaterTerrainInput,
    water_level: f32,
) -> Result<PaintedWaterMeshData, String> {
    build_painted_water_mesh_with_depth(
        mask,
        resolution,
        world_min,
        world_max,
        terrain,
        water_level,
        MIN_STABLE_WATER_DEPTH_METERS,
    )
}

/// 使用当前水体预设派生的安全深度构建静水网格；仍由调用方放入有界后台任务。
#[allow(clippy::too_many_arguments)]
pub fn build_painted_water_mesh_with_depth(
    mask: &[u8],
    resolution: [u32; 2],
    world_min: [f32; 2],
    world_max: [f32; 2],
    terrain: &PaintedWaterTerrainInput,
    water_level: f32,
    minimum_depth: f32,
) -> Result<PaintedWaterMeshData, String> {
    build_painted_water_mesh_inner(mask, resolution, world_min, world_max, Some(terrain), water_level, minimum_depth)
}

#[allow(clippy::too_many_arguments)]
fn build_painted_water_mesh_inner(
    mask: &[u8], resolution: [u32; 2], world_min: [f32; 2], world_max: [f32; 2],
    terrain: Option<&PaintedWaterTerrainInput>, water_level: f32, minimum_depth: f32,
) -> Result<PaintedWaterMeshData, String> {
    let expected = (resolution[0] as usize).checked_mul(resolution[1] as usize);
    if resolution[0] < 2
        || resolution[1] < 2
        || expected != Some(mask.len())
        || !world_min
            .iter()
            .chain(&world_max)
            .all(|value| value.is_finite())
        || world_min[0] >= world_max[0]
        || world_min[1] >= world_max[1]
        || !(world_max[0] - world_min[0]).is_finite()
        || !(world_max[1] - world_min[1]).is_finite()
        || !water_level.is_finite()
        || !minimum_depth.is_finite()
        || minimum_depth < MIN_STABLE_WATER_DEPTH_METERS
        || terrain.is_some_and(|terrain| !terrain.contains_world_bounds(world_min, world_max))
    {
        return Err("河水遮罩长度、分辨率、世界范围、水位或地形覆盖无效".to_owned());
    }
    let stable_core = build_stable_water_core(
        mask,          // 使用完整源R8而不是先降采样，保留精确稳定几何核心。
        resolution,    // 源像素分辨率同时决定精确三角曲面检查矩形。
        world_min,     // 当前水层与地形共享世界X/Z范围。
        world_max,     // 当前水层与地形共享世界X/Z范围。
        terrain,       // 已有地形继续严格裁岸；独立画布None不虚构任何地面。
        water_level,   // 当前层独立静止水位。
        minimum_depth, // 当前预设最大风浪也不得在波谷露出地面。
    );
    let Some(core_bounds) = stable_core.bounds else {
        // 【水位编辑语义】R8是候选最大范围；水位降低或细笔刷落在粗R16格间时，当前
        // 交集可以合法干涸。返回严格空网格既不穿地，也不会把可逆的水位拖动升级成预览失败。
        let footprint = PaintedWaterFootprint::empty(world_min, world_max); // 合法空层保留一个永不命中的完整范围足迹。
        return Ok(PaintedWaterMeshData {
            local_aabb: None,       // 没有可见三角形的水层不得伪造定位范围。
            positions: Vec::new(),  // 合法全零水层不登记任何GPU顶点。
            normals: Vec::new(),    // 合法全零水层不登记任何GPU法线。
            tangents: None,         // 静水顶点着色器使用固定X切线回退。
            uvs: Vec::new(),        // 合法全零水层不登记任何GPU UV。
            uvs_b: None,            // 静水通过缺省负样式值进入统一流向uniform分支。
            colors: Vec::new(),     // 合法全零水层不登记任何顶点颜色。
            indices: Vec::new(),    // 空索引让调用方跳过实体和材质资产。
            footprint,              // 保留空足迹供纯逻辑测试与一致性检查。
            ribbon_footprint: None, // 静水不携带样条足迹。
        });
    };
    let shore_band_meters = terrain.map_or(2.0, PaintedWaterTerrainInput::visual_shore_band_meters); // 无地形沿用既有两米R8视觉边缘；有地形的岸带保持原值。
    let active_bounds = expanded_active_bounds(
        core_bounds,       // 仅围绕真实稳定水域核心建立局部网格。
        resolution,        // 岸带按源像素换算后夹紧到地图边界。
        world_min,         // 用完整地图范围计算真实米制像素尺寸。
        world_max,         // 用完整地图范围计算真实米制像素尺寸。
        shore_band_meters, // 裁剪范围和顶点alpha传播必须采用同一个米制岸带。
    );
    let active_source_size = active_bounds.size(); // 局部核心加岸带的源单元数量。
    let cells = active_source_size; // 保留作者分辨率，完整几何在后台拆成上传块。
    let vertex_width = cells[0].checked_add(1).ok_or_else(|| "水面顶点行宽溢出".to_owned())?;
    let vertex_height = cells[1].checked_add(1).ok_or_else(|| "水面顶点列高溢出".to_owned())?;
    let vertex_count = (vertex_width as usize).checked_mul(vertex_height as usize)
        .ok_or_else(|| "水面顶点数量溢出".to_owned())?;
    u32::try_from(vertex_count).map_err(|_| "水面顶点无法使用U32网格索引表示".to_owned())?;
    let coverage = downsample_stable_coverage(
        &stable_core.coverage, // 视觉网格也只从安全深水核心开始派生。
        resolution,            // 最大值分区读取完整源R8行宽。
        active_bounds,         // 远处平地完全不进入局部网格。
        cells,                 // 采用完整作者精度，后台再拆分上传单元。
    );
    let full_world_size = [world_max[0] - world_min[0], world_max[1] - world_min[1]];
    let active_world_min = [
        world_min[0] + active_bounds.min[0] as f32 / resolution[0] as f32 * full_world_size[0],
        world_min[1] + active_bounds.min[1] as f32 / resolution[1] as f32 * full_world_size[1],
    ]; // 局部网格最小端点严格落在源R8单元边界。
    let active_world_max = [
        world_min[0]
            + active_bounds.max_exclusive[0] as f32 / resolution[0] as f32 * full_world_size[0],
        world_min[1]
            + active_bounds.max_exclusive[1] as f32 / resolution[1] as f32 * full_world_size[1],
    ]; // 局部网格最大端点严格落在源R8单元边界。
    let active_world_size = [
        active_world_max[0] - active_world_min[0],
        active_world_max[1] - active_world_min[1],
    ]; // 顶点和米制岸带共用相同局部跨度。
    let vertex_alpha =
        build_shore_vertex_alpha(&coverage, cells, active_world_size, shore_band_meters); // 视觉岸带覆盖完整作者岸坡，在外边界平滑归零。
    let mut positions = Vec::with_capacity(vertex_count);
    let mut normals = Vec::with_capacity(vertex_count);
    let mut uvs = Vec::with_capacity(vertex_count);
    let mut colors = Vec::with_capacity(vertex_count);
    for z in 0..vertex_height {
        let z_ratio = z as f32 / cells[1] as f32;
        let world_z = active_world_min[1] + active_world_size[1] * z_ratio;
        for x in 0..vertex_width {
            let x_ratio = x as f32 / cells[0] as f32;
            let world_x = active_world_min[0] + active_world_size[0] * x_ratio;
            let vertex_index = (z * vertex_width + x) as usize;
            positions.push([world_x, 0.0, world_z]);
            normals.push([0.0, 1.0, 0.0]);
            uvs.push([x_ratio, 0.5]); // 绘制水不固定R8边缘，背景波和互动波都保持完整自由振幅。
            colors.push([1.0, 1.0, 1.0, vertex_alpha[vertex_index]]); // 仅视觉外带alpha平滑归零。
        }
    }
    let mut indices = Vec::new();
    for z in 0..cells[1] {
        for x in 0..cells[0] {
            let top_left = z * vertex_width + x;
            let top_right = top_left + 1;
            let bottom_left = top_left + vertex_width;
            let bottom_right = bottom_left + 1;
            if [top_left, top_right, bottom_left, bottom_right]
                .into_iter()
                .all(|vertex| vertex_alpha[vertex as usize] <= f32::EPSILON)
            {
                continue;
            }
            indices.extend_from_slice(&[
                top_left,
                bottom_left,
                bottom_right,
                top_left,
                bottom_right,
                top_right,
            ]); // X/Z平面使用朝上绕序，法线、阴影与水材质正面一致。
        }
    }
    let footprint = PaintedWaterFootprint::from_visible_mesh(
        cells,            // 互动查询与实际Mesh使用完全相同的规则网格单元数。
        active_world_min, // 只覆盖本次实际生成的局部水面最小端点。
        active_world_max, // 只覆盖本次实际生成的局部水面最大端点。
        vertex_alpha,     // 移交与顶点颜色相同的透明度，浅滩无需另建猜测位图。
    );
    let local_aabb = local_aabb_from_positions(&positions); // 水平网格仍在后台任务中完成一次范围缓存。
    Ok(PaintedWaterMeshData {
        local_aabb, // 编辑器双击定位和其他主线程消费者直接复用完成值。
        positions,
        normals,
        tangents: None, // 水平R8网格继续使用着色器固定局部X切线。
        uvs,
        uvs_b: None, // 水平R8网格通过材质uniform读取统一流向和流速。
        colors,
        indices,
        footprint, // 玩家读取实际可见水面足迹，再由脚底深度排除岸上与共面地形。
        ribbon_footprint: None, // 水平R8网格不需要三维带状足迹。
    })
}

#[cfg(test)]
mod tests; // 联合R8/R16、局部岸带、自由波UV与精确足迹回归保持独立文件。
