//! 正式大场景水面的 GPU 波浪与逐像素沿程流动扩展材质。
//! 【功能设计】顶点阶段负责真实波高和运动历史；片元阶段只为样条水增加连续流纹、急流白沫和微法线，
//! Blender或作者提供的颜色、透明度、纹理与StandardMaterial PBR光照契约继续完整保留。

use bevy::asset::Asset; // GPU水面扩展作为ExtendedMaterial的一部分进入Assets和RenderAssets。
use bevy::pbr::{ExtendedMaterial, MaterialExtension, StandardMaterial}; // 组合原生PBR材质与自有顶点位移扩展。
use bevy::prelude::*; // 引入Handle、Shader、Vec4等材质与参数构造类型。
use bevy::reflect::TypePath; // 自有Asset使用稳定Rust类型路径标识。
use bevy::render::render_resource::AsBindGroup; // 把固定640字节Uniform映射到材质binding 100。
use bevy::render::storage::ShaderBuffer; // 全水域共用固定天气缓冲，禁止逐帧扫描更新所有水材质。
use bevy::shader::ShaderRef; // MaterialExtension通过固定内部句柄选择顶点Shader。
use feathrise_environment::terrain_surface::surface_weather_buffer_handle; // 与地面积水绑定同一个天气身份。

use super::constants::{
    // GPU参数全部从现有水面常量构造，避免Rust业务口径分叉。
    BANK_WAVE_FADE_UV_WIDTH,        // 两侧河岸的UV衰减宽度。
    PLAYER_RIPPLE_ATTACK_SECONDS,   // 新波源建立到完整振幅所需秒数。
    PLAYER_RIPPLE_BAND_HALF_WIDTH,  // 移动波包中心两侧的有效半宽。
    PLAYER_RIPPLE_JUMP_AMPLITUDE,   // 归一化波谷信号使用的最大事件振幅基准。
    PLAYER_RIPPLE_LAYER_WAVELENGTH, // 玩家同心波峰与波谷的完整波长。
    PLAYER_RIPPLE_LIFETIME,         // 单个玩家波源保留秒数。
    PLAYER_RIPPLE_MAX_HEIGHT,       // 多个波源叠加后的几何限幅。
    PLAYER_RIPPLE_SPEED,            // 玩家波前向外传播速度。
    PLAYER_RIPPLE_TROUGH_DARKENING, // 波谷对Blender原生材质的最大乘色压暗。
}; // 完成GPU参数常量导入。
use super::painted::{WaterSurfaceDescriptor, legacy_macro_waves, legacy_micro_normals};
use super::shaders::{
    // 主通道、预通道和片元阶段分别使用匹配Bevy IO结构的固定入口。
    DYNAMIC_WATER_FRAGMENT_SHADER_HANDLE, // 透明前向PBR使用的逐像素流动入口。
    DYNAMIC_WATER_PREPASS_SHADER_HANDLE,  // 深度、法线、运动向量与延迟顶点入口。
    DYNAMIC_WATER_VERTEX_SHADER_HANDLE,   // 主渲染与方向光阴影顶点入口。
}; // 完成固定Shader句柄导入。
use super::types::{
    DynamicWaterGpuFrame, // 单帧固定二十个vec4的GPU参数布局。
    DynamicWaterUniform,  // 当前帧与历史帧组合后的完整材质Uniform。
    PlayerWaterRipple,    // 固定八槽入水、脚步、起跳与落水统一波源。
}; // 复用固定GPU ABI与CPU波源值。 // 新静水读取预设，旧水体使用同一兼容外观。

/// GPU水面材质扩展，只绑定当前帧与上一帧的固定波形参数。
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)] // 非bindless材质适合正式场景少量独立水面。
pub struct DynamicWaterExtension {
    #[uniform(100)] // 两帧各二十个vec4，主通道与运动向量预通道共享该绑定。
    pub(crate) uniform: DynamicWaterUniform, // Clipmap雨水高度代理逐字复用同一当前/历史波形。
    #[texture(101)]
    #[sampler(102)]
    pub(super) micro_normal_a: Option<Handle<Image>>, // None由Bevy绑定回退图，uniform强度严格为零。
    #[texture(103)]
    #[sampler(104)]
    pub(super) micro_normal_b: Option<Handle<Image>>, // 第二层使用独立采样器，允许不同过滤和滚动贴图。
    #[storage(119, read_only)] // 全局64字节天气由环境插件每帧只上传一次。
    pub(super) surface_weather: Handle<ShaderBuffer>, // 所有湖泊、样条和整网格共享，无单材质天气更新。
    #[storage(118, read_only)] // 与地表使用同一固定遮雨体集合。
    pub(super) rain_shelter: Handle<ShaderBuffer>, // 无碰撞桥面也能阻止水面雨纹。
} // 结束GPU水面材质扩展定义。

impl Default for DynamicWaterExtension {
    // 默认材质也必须持有可准备的共享天气句柄，不能使用空存储资产。
    fn default() -> Self {
        // 保留原有零波形和可选微法线默认语义。
        Self {
            // 仅天气句柄具有全局稳定身份。
            uniform: default(),   // 默认没有宏观波或玩家事件。
            micro_normal_a: None, // 缺图沿用Bevy中性回退。
            micro_normal_b: None, // 第二层同样默认关闭。
            surface_weather: surface_weather_buffer_handle(), // 与正式创建入口绑定同一个64字节天气资源。
            rain_shelter: feathrise_environment::rain_shelter::rain_shelter_buffer_handle(), // 默认空集合安全回退。
        } // 完成默认扩展构造。
    } // 结束默认入口。
} // 结束默认共享缓冲身份保证。

/// Blender标准水材质与项目GPU波浪扩展组成的具体资产类型。
pub type DynamicWaterMaterial = ExtendedMaterial<StandardMaterial, DynamicWaterExtension>; // 编辑器绘制水面与Blender正式水面共享同一具体材质类型。

impl MaterialExtension for DynamicWaterExtension {
    // 主渲染与方向光阴影使用当前帧位移、动态法线和波谷顶点乘色。
    fn vertex_shader() -> ShaderRef {
        DYNAMIC_WATER_VERTEX_SHADER_HANDLE.clone().into() // 返回编译进scene crate的固定主顶点Shader。
    } // 完成主顶点Shader选择。

    // 深度、法线和运动向量预通道同时读取当前与上一帧波形。
    fn prepass_vertex_shader() -> ShaderRef {
        DYNAMIC_WATER_PREPASS_SHADER_HANDLE.clone().into() // 返回匹配prepass_io的固定顶点Shader。
    } // 完成预通道顶点Shader选择。

    // 延迟PBR预通道必须采用与主画面完全相同的水面位移和法线。
    fn deferred_vertex_shader() -> ShaderRef {
        DYNAMIC_WATER_PREPASS_SHADER_HANDLE.clone().into() // 复用预通道入口并由Bevy设置延迟定义。
    } // 完成延迟顶点Shader选择。

    // 前向透明PBR在光照前加入逐像素沿程流纹，不用环境反射也能直接看出下游方向。
    fn fragment_shader() -> ShaderRef {
        DYNAMIC_WATER_FRAGMENT_SHADER_HANDLE.clone().into() // 返回与StandardMaterial前向IO兼容的固定片元Shader。
    } // 完成前向片元Shader选择。
} // 完成水面MaterialExtension实现。

/// 把静水预设、流动和当前八槽统一玩家波源编码为固定二十vec4的单帧GPU参数。
pub(super) fn dynamic_water_gpu_frame(
    elapsed_seconds: f32, // 连续时间驱动四层独立风浪与两层微法线相位。
    player_ripples: &[PlayerWaterRipple], // 最多八个仍在寿命内的统一玩家事件波源。
    flow_direction: Vec2, // 普通水体是局部XZ流向；整网格可用x高位哨兵强制UV0流场，样条仍读UV1。
    flow_speed: f32,      // 当前水体的流动速度；静水四层风浪使用各自速度，不受零流速影响。
    appearance: Option<&WaterSurfaceDescriptor>, // Some为数据驱动静水，None为旧GLB或样条兼容外观。
) -> DynamicWaterGpuFrame {
    let mut packed_ripples = [Vec4::ZERO; super::constants::PLAYER_RIPPLE_CAPACITY]; // 固定数组避免逐帧分配和可变长度GPU上传。
    for (packed, ripple) in packed_ripples.iter_mut().zip(player_ripples) {
        // 每槽只上传局部中心、年龄和初始振幅，逐顶点波包由Shader重建。
        *packed = Vec4::new(
            ripple.center_local_xz.x, // x保存水面局部波源中心X。
            ripple.center_local_xz.y, // y保存水面局部波源中心Z。
            ripple.age,               // z保存波源年龄供GPU计算半径和寿命衰减。
            ripple.amplitude,         // w保存入水、脚步、起跳或落水对应的初始振幅。
        ); // 完成当前固定槽编码。
    } // 完成全部有效波源编码，剩余槽保持严格零值。
    let fallback_waves = legacy_macro_waves();
    let fallback_micro = legacy_micro_normals();
    let waves = appearance.map_or(&fallback_waves, |descriptor| &descriptor.macro_waves);
    let micro = appearance.map_or(&fallback_micro, |descriptor| &descriptor.micro_normals);
    let mut packed_waves = [Vec4::ZERO; 4];
    let mut wave_speeds = Vec4::ZERO;
    let mut wave_steepness = Vec4::ZERO;
    for (index, wave) in waves.iter().enumerate() {
        let direction = Vec2::from_array(wave.direction).normalize_or_zero();
        packed_waves[index] = Vec4::new(
            direction.x,
            direction.y,
            wave.amplitude_meters.max(0.0),
            std::f32::consts::TAU / wave.wavelength_meters.max(0.1),
        );
        wave_speeds[index] = wave.speed_meters_per_second.max(0.0);
        wave_steepness[index] = wave.steepness.clamp(0.0, 1.0);
    }
    let pack_micro = |index: usize| {
        let layer = &micro[index];
        let has_texture = appearance
            .and_then(|descriptor| descriptor.micro_normal_textures[index].as_ref())
            .is_some();
        Vec4::new(
            layer.tile_meters.max(0.01),
            layer.velocity_meters_per_second[0],
            layer.velocity_meters_per_second[1],
            if has_texture {
                layer.strength.max(0.0)
            } else {
                0.0
            },
        )
    };
    DynamicWaterGpuFrame {
        // 四个头部vec4保存背景波、玩家波形与水面静态参数。
        time_and_visual: Vec4::new(
            elapsed_seconds, // x为当前连续时间。
            appearance.map_or(PLAYER_RIPPLE_TROUGH_DARKENING, |value| {
                value.trough_darkening
            }),
            appearance.map_or(1.0, |value| value.shore_wave_attenuation),
            0.0,
        ), // 完成时间与视觉参数编码。
        ripple_shape: Vec4::new(
            PLAYER_RIPPLE_LIFETIME,         // x为单个波源寿命。
            PLAYER_RIPPLE_SPEED,            // y为波前传播速度。
            PLAYER_RIPPLE_BAND_HALF_WIDTH,  // z为有限环带半宽。
            PLAYER_RIPPLE_LAYER_WAVELENGTH, // w为同心波层波长。
        ), // 完成玩家波包形状参数编码。
        ripple_limits: Vec4::new(
            PLAYER_RIPPLE_ATTACK_SECONDS, // x为新波建立时间。
            PLAYER_RIPPLE_MAX_HEIGHT,     // y为叠加几何高度上限。
            player_ripples.len() as f32,  // z为有效固定槽数量。
            PLAYER_RIPPLE_JUMP_AMPLITUDE, // w为波谷信号归一化振幅基准。
        ), // 完成玩家波包限幅与计数参数编码。
        surface_settings: Vec4::new(
            BANK_WAVE_FADE_UV_WIDTH, // x为两侧岸带内恢复波幅的UV宽度。
            flow_direction.x,        // y为当前水体局部X方向分量。
            flow_direction.y,        // z为当前水体局部Z方向分量。
            flow_speed.max(0.0),     // w为基础流速；作者校验负责限制正式上限。
        ), // 完成岸带、流向与流速静态参数编码。
        wave_directions_amplitudes: packed_waves,
        wave_speeds,
        wave_steepness,
        micro_normal_a: pack_micro(0),
        micro_normal_b: pack_micro(1),
        ripples: packed_ripples, // 写入固定八槽统一玩家波源数组。
    } // 返回可直接上传材质Uniform的单帧参数。
} // 结束单帧GPU水面参数构造。

/// 从Blender导入材质创建首帧无运动脉冲的GPU水面材质。
pub(super) fn create_dynamic_water_material(
    base: StandardMaterial, // 已保留全部Blender颜色、透明度、纹理和PBR参数的独立副本。
    elapsed_seconds: f32,   // 初始化时的连续时间让首次可见帧与全局背景波相位一致。
    flow_direction: Vec2,   // 首帧同时冻结当前水体流向，避免材质出现默认方向脉冲。
    flow_speed: f32,        // 首帧同时冻结当前水体流速，供主通道与运动历史共用。
    appearance: Option<&WaterSurfaceDescriptor>, // 新静水提供完整预设，旧资产传None。
) -> DynamicWaterMaterial {
    let initial =
        dynamic_water_gpu_frame(elapsed_seconds, &[], flow_direction, flow_speed, appearance); // 首帧没有玩家事件波，并使用当前背景时间和正式流向。
    DynamicWaterMaterial {
        base, // 标准PBR片元流程继续完整复用导入材质。
        extension: DynamicWaterExtension {
            uniform: DynamicWaterUniform {
                current: initial,  // 当前帧从正确背景波相位开始。
                previous: initial, // 历史与当前完全相同，避免首次运动向量脉冲。
            }, // 完成固定GPU Uniform初始化。
            micro_normal_a: appearance.and_then(|value| value.micro_normal_textures[0].clone()),
            micro_normal_b: appearance.and_then(|value| value.micro_normal_textures[1].clone()),
            surface_weather: surface_weather_buffer_handle(), // 固定共享句柄让新水面立即采用当前雨量和暂停相位。
            rain_shelter: feathrise_environment::rain_shelter::rain_shelter_buffer_handle(), // 新水面同帧采用已发布遮雨代理。
        }, // 完成自有顶点扩展初始化。
    } // 返回可登记到Assets的具体水面材质。
} // 结束GPU水面材质创建。

/// 候选资产分帧登记完成后，把当前与历史时间一起重置到统一采用前基线。
pub(super) fn rebase_dynamic_water_material(
    material: &mut DynamicWaterMaterial,
    elapsed_seconds: f32,
    flow_direction: Vec2,
    flow_speed: f32,
) {
    let mut initial = material.extension.uniform.current;
    initial.time_and_visual.x = elapsed_seconds;
    initial.ripple_limits.z = 0.0;
    initial.surface_settings.y = flow_direction.x;
    initial.surface_settings.z = flow_direction.y;
    initial.surface_settings.w = flow_speed.max(0.0);
    initial.ripples = [Vec4::ZERO; super::constants::PLAYER_RIPPLE_CAPACITY];
    material.extension.uniform.current = initial;
    material.extension.uniform.previous = initial; // 等待GPU重新准备期间不积累陈旧运动历史。
}

/// 推进同一水面材质的当前帧与上一帧固定参数。
pub(super) fn update_dynamic_water_material(
    material: &mut DynamicWaterMaterial, // 当前水面独占的扩展材质资产。
    elapsed_seconds: f32,                // 本帧背景缓波连续时间。
    player_ripples: &[PlayerWaterRipple], // 本帧老化和触发完成后的最多八个统一玩家波源。
    flow_direction: Vec2,                // 本帧水体流向与玩家波一同进入当前历史帧。
    flow_speed: f32,                     // 本帧水体流速与时间共同决定背景波平移速度。
) {
    material.extension.uniform.previous = material.extension.uniform.current; // 先保存上一帧全部波形供运动向量重建。
    let mut current = material.extension.uniform.previous;
    current.time_and_visual.x = elapsed_seconds;
    current.ripple_limits.z = player_ripples.len() as f32;
    current.surface_settings.y = flow_direction.x;
    current.surface_settings.z = flow_direction.y;
    current.surface_settings.w = flow_speed.max(0.0);
    current.ripples = [Vec4::ZERO; super::constants::PLAYER_RIPPLE_CAPACITY];
    for (packed, ripple) in current.ripples.iter_mut().zip(player_ripples) {
        *packed = Vec4::new(
            ripple.center_local_xz.x,
            ripple.center_local_xz.y,
            ripple.age,
            ripple.amplitude,
        );
    }
    material.extension.uniform.current = current; // 静态预设参数保持不变，只推进时间、流动与八槽玩家波。
} // 结束水面GPU参数逐帧推进。

#[cfg(test)] // GPU ABI与历史推进测试不启动窗口或RenderApp。
mod tests {
    use std::mem::size_of; // 固定Uniform字节数直接锁定Rust物理布局。

    use bevy::prelude::*; // 构造默认PBR材质和局部波源坐标。
    use bevy::render::render_resource::ShaderType; // 同时锁定encase实际上传到GPU的最小绑定大小。
    use feathrise_scene_content::{SceneStillWaterKind, SceneWaterLayer}; // 构造带稳定预设外观的最小静水层。

    use super::{
        WaterSurfaceDescriptor, create_dynamic_water_material, dynamic_water_gpu_frame,
        update_dynamic_water_material,
    }; // 调用正式材质生命周期入口。
    use crate::shared::large_scene::water::types::{
        DynamicWaterGpuFrame, // 检查单帧二十vec4 ABI大小。
        DynamicWaterUniform,  // 检查当前与历史四十vec4组合大小。
        PlayerWaterRipple,    // 构造统一玩家波源固定槽编码样本。
    }; // 验证固定GPU ABI与槽位编码。

    #[test] // Rust侧布局必须与WGSL两个二十vec4帧严格一致。
    fn dynamic_water_uniform_layout_stays_at_forty_vec4s() {
        assert_eq!(size_of::<DynamicWaterGpuFrame>(), 320); // 单帧固定二十个vec4。
        assert_eq!(size_of::<DynamicWaterUniform>(), 640); // 当前与历史合计四十个vec4。
        assert_eq!(DynamicWaterGpuFrame::min_size().get(), 320); // ShaderType不得为单帧插入额外尾部填充。
        assert_eq!(DynamicWaterUniform::min_size().get(), 640); // GPU binding实际最小尺寸必须与WGSL四十个vec4完全一致。
    } // 完成GPU ABI字节布局回归。

    #[test] // 静水风浪必须由预设速度独立推进，不能被场景水流零值意外停掉。
    fn data_driven_still_water_keeps_waves_when_flow_speed_is_zero() {
        let layer = SceneWaterLayer {
            id: "still-water".to_owned(),
            label: "静水测试".to_owned(),
            mask_source_path: "scenes/MAP-test/water/still-water.r8".to_owned(),
            mask_texture_path: "scenes/MAP-test/water/still-water.ktx2".to_owned(),
            resolution: [2, 2],
            canvas_bounds: None,
            water_level: 0.0,
            still_kind: SceneStillWaterKind::Lake,
            preset_id: "windy_lake".to_owned(),
            flow_direction: [0.0, 0.0],
            flow_speed: 0.0,
            base_color_srgba: [0.04, 0.18, 0.24, 0.72],
            perceptual_roughness: 0.06,
        }; // 场景层明确关闭水流，但保留独立预设引用。
        let mut appearance = WaterSurfaceDescriptor::from_layer(&layer);
        for wave in &mut appearance.macro_waves {
            wave.amplitude_meters = 0.0; // 先清空兼容风浪，确保断言只观察指定预设值。
        }
        appearance.macro_waves[0].amplitude_meters = 0.41;
        appearance.macro_waves[0].wavelength_meters = 23.0;
        appearance.macro_waves[0].speed_meters_per_second = 3.25;
        appearance.macro_waves[0].steepness = 0.7;
        appearance.shore_wave_attenuation = 2.2;

        let frame = dynamic_water_gpu_frame(8.0, &[], Vec2::ZERO, 0.0, Some(&appearance));

        assert_eq!(frame.surface_settings.w, 0.0); // 场景水流保持作者给出的真正静止状态。
        assert_eq!(frame.wave_speeds.x, 3.25); // 第一组风浪仍带有自身非零传播速度。
        assert_eq!(frame.wave_directions_amplitudes[0].z, 0.41); // 波幅不依赖水流速度。
        assert_eq!(frame.wave_steepness.x, 0.7); // Gerstner水平位移继续来自同一预设。
        assert_eq!(frame.time_and_visual.z, 2.2); // 岸边衰减指数也进入固定GPU参数。
        assert!(
            (appearance.minimum_stable_depth() - 0.55).abs() < 0.000_01,
            "安全河床深度必须覆盖0.41m风浪、0.12m玩家波与0.02m余量",
        ); // 高波预设不能继续沿用可能露底的固定0.20m核心。
    } // 完成零水流独立风浪与安全深度回归。

    #[test] // 固定数组只写有效波源，其余槽必须保持零值。
    fn dynamic_water_frame_packs_active_ripples_without_allocation_shape_changes() {
        let ripple = PlayerWaterRipple {
            center_local_xz: Vec2::new(3.0, -2.0), // 使用非零局部中心验证XZ映射。
            age: 0.5,                              // 使用半秒年龄验证Z分量。
            amplitude: 0.07,                       // 使用标准入水量级验证W分量。
        }; // 完成单波源测试值构造。
        let frame = dynamic_water_gpu_frame(
            12.0,                // 使用非零背景时间验证首个头部分量。
            &[ripple],           // 编码一枚有效波源和七个严格零值空槽。
            Vec2::new(0.6, 0.8), // 使用明确单位XZ流向锁定湖面uniform接线。
            2.5,                 // 使用非默认流速锁定湖面uniform接线。
            None,                // 使用固定兼容风浪参数。
        );
        assert_eq!(frame.ripple_limits.z, 1.0); // 有效槽计数必须精确为一。
        assert_eq!(frame.ripples[0], Vec4::new(3.0, -2.0, 0.5, 0.07)); // 第一槽按XZ、年龄、振幅排列。
        assert!(frame.ripples[1..].iter().all(|slot| *slot == Vec4::ZERO)); // 未使用槽不得携带旧帧残留。
        assert_eq!(frame.surface_settings.y, 0.6); // 湖面uniform必须携带作者局部X流向。
        assert_eq!(frame.surface_settings.z, 0.8); // 湖面uniform必须携带作者局部Z流向。
        assert_eq!(frame.surface_settings.w, 2.5); // 湖面uniform必须携带作者流速倍率。
    } // 完成固定波源数组编码回归。

    #[test] // 每次更新前必须保存完整旧帧参数，新增统一大波也要获得连续运动历史。
    fn dynamic_water_material_keeps_previous_gpu_frame_for_new_ripple() {
        let mut material = create_dynamic_water_material(
            StandardMaterial::default(), // 创建默认PBR外观供历史测试。
            1.0,                         // 首帧背景时间。
            Vec2::X,                     // 首帧沿局部X流动。
            1.0,                         // 首帧使用单位流速。
            None,                        // 单元测试覆盖旧兼容外观。
        ); // 创建当前历史相同的首帧材质。
        let first_current = material.extension.uniform.current; // 保存初始化帧供更新后比较。
        let ripple_start = PlayerWaterRipple {
            center_local_xz: Vec2::new(2.0, 4.0), // 使用非零中心追踪统一大波历史。
            age: 0.0,                             // 新事件必须从零年龄开始并由统一attack平滑建立。
            amplitude: 0.09,                      // 使用正式起跳与落水共同采用的强波振幅。
        }; // 完成统一波源首帧测试值构造。
        update_dynamic_water_material(
            &mut material,   // 更新同一独占扩展材质。
            2.0,             // 推进到下一秒。
            &[ripple_start], // 启动一枚统一大波。
            Vec2::Y,         // 同帧把湖面流向改为局部+Z。
            2.0,             // 同帧把湖面流速改为二倍。
        );
        assert_eq!(material.extension.uniform.previous, first_current); // 历史必须精确保留更新前全部二十个vec4。
        assert_eq!(material.extension.uniform.current.time_and_visual.x, 2.0); // 当前时间必须采用本帧值。
        assert_eq!(material.extension.uniform.previous.ripple_limits.z, 0.0); // 新波出现前的历史帧必须保持零有效槽。
        assert_eq!(material.extension.uniform.current.ripple_limits.z, 1.0); // 当前帧必须立即暴露一枚统一波源。
        assert_eq!(
            Vec2::new(
                material.extension.uniform.current.surface_settings.y,
                material.extension.uniform.current.surface_settings.z,
            ),
            Vec2::Y
        ); // 当前帧必须携带新湖面流向。
        assert_eq!(material.extension.uniform.current.surface_settings.w, 2.0); // 当前帧必须携带新湖面流速。
        let ripple_start_frame = material.extension.uniform.current; // 保存波源首帧供下一次更新验证历史连续性。
        let ripple_next = PlayerWaterRipple {
            center_local_xz: ripple_start.center_local_xz, // 后续帧保持同一事件中心。
            age: 0.1,                                      // 推进到零点一秒年龄。
            amplitude: ripple_start.amplitude,             // 后续帧保持同一事件强度。
        }; // 完成统一波源后续帧测试值构造。
        update_dynamic_water_material(
            &mut material,  // 更新同一独占扩展材质。
            2.1,            // 推进零点一秒。
            &[ripple_next], // 上传同一波源的连续年龄。
            -Vec2::X,       // 使用第三个方向验证上一帧完整历史。
            0.5,            // 使用第三个流速验证上一帧完整历史。
        );
        assert_eq!(material.extension.uniform.previous, ripple_start_frame); // 预通道必须看到精确的波源首帧参数。
        assert_eq!(material.extension.uniform.current.ripples[0].z, 0.1); // 当前帧必须编码连续零点一秒年龄。
        assert_eq!(
            Vec2::new(
                material.extension.uniform.previous.surface_settings.y,
                material.extension.uniform.previous.surface_settings.z,
            ),
            Vec2::Y
        ); // 历史帧必须连同上一流向保存。
        assert_eq!(material.extension.uniform.previous.surface_settings.w, 2.0); // 历史帧必须连同上一流速保存。
    } // 完成材质历史推进回归。
} // 结束GPU水面材质测试模块。
