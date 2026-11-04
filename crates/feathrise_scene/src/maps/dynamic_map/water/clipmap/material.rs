//! Clipmap 静水组合材质：内层完整复用动态水外观，外层只增加高度页和 R8 裁切。

use bevy::asset::{load_internal_asset, uuid_handle};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, Material, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPipeline, MaterialPipelineKey, MaterialPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::{ShaderRef, load_shader_library};

use crate::maps::dynamic_map::clipmap::{ClipmapHeightExtension, ClipmapHeightUniform};
use crate::shared::large_map::{DynamicWaterMaterial, DynamicWaterUniform};

const MAIN_VERTEX_SHADER: Handle<Shader> = uuid_handle!("efdd7c68-bfee-48f6-b06b-4b76cb0a6c10");
const PREPASS_VERTEX_SHADER: Handle<Shader> = uuid_handle!("efdd7c68-bfee-48f6-b06b-4b76cb0a6c11");
const PREPASS_FRAGMENT_SHADER: Handle<Shader> =
    uuid_handle!("efdd7c68-bfee-48f6-b06b-4b76cb0a6c12");
const RAIN_HEIGHT_SHADER: Handle<Shader> = uuid_handle!("efdd7c68-bfee-48f6-b06b-4b76cb0a6c13");

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub(crate) struct ClipmapWaterUniform {
    pub(crate) surface: Vec4, // x静止水位，y恢复完整波幅的安全水深，z最高高度Mip。
    pub(crate) missing_pages: [UVec4; 16], // 当前同修订局部缺页。
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct ClipmapWaterExtension {
    #[uniform(129)]
    pub(super) holes: crate::maps::dynamic_map::clipmap::material::ClipmapHoleUniform,
    #[uniform(120)]
    height_params: ClipmapHeightUniform,
    #[texture(121, dimension = "2d_array")]
    #[sampler(122)]
    heights: Handle<Image>,
    #[texture(123, sample_type = "u_int")]
    page_table: Handle<Image>,
    #[uniform(124)]
    pub(super) water: ClipmapWaterUniform,
    #[texture(125)]
    #[sampler(126)]
    pub(super) mask: Handle<Image>,
}

impl ClipmapWaterExtension {
    /// 原生静水使用网格UV1携带真实水底，不绑定任何虚构高度场。
    /// 未使用的高度页槽仅绑定已有中性数组以保持正式水管线的完整绑定合同。
    pub(in crate::maps::dynamic_map) fn for_density(mask:Handle<Image>,bounds:(Vec2,Vec2),water_level:f32,
        minimum_depth:f32,fallback:&feathrise_environment::terrain_surface::TerrainSurfaceFallbackImages)->Self {
        Self {holes:crate::maps::dynamic_map::clipmap::material::ClipmapHoleUniform::from_holes(&[]),
            height_params:ClipmapHeightUniform {stitches: Default::default(),map_bounds:Vec4::new(bounds.0.x,bounds.0.y,bounds.1.x,bounds.1.y),
                grid:Vec4::ONE,options:Vec4::ZERO,reserved:Vec4::ZERO,source_to_world:Mat4::IDENTITY,world_to_source:Mat4::IDENTITY},
            heights:fallback.white_base_color_array.clone(),page_table:fallback.zero_index.clone(),
            water:ClipmapWaterUniform {surface:Vec4::new(water_level,minimum_depth,0.,1.),missing_pages:[UVec4::ZERO;16]},mask}
    }
    /// 作者足迹到世界的完整变换；取景不能再叠加相机跟随环带的局部平移。
    pub fn source_frame(&self) -> bevy::math::Mat4 { self.height_params.source_to_world }
    pub(in crate::maps::dynamic_map) fn set_source_frame(&mut self, frame: bevy::math::Mat4) {
        self.height_params.source_to_world = frame;
        self.height_params.world_to_source = frame.inverse();
    }
    /// 同一水材质/遮罩，三维派生网格的UV1直接携带所选水位下方的真实洞底高度。
    pub(crate) fn for_volume(mut self) -> Self {
        self.water.surface.w = 1.0;
        self
    }
    pub(super) fn new(
        height: &ClipmapHeightExtension,
        mask: Handle<Image>,
        water_level: f32,
        minimum_depth: f32,
        max_mip: u32,
        missing_pages: [UVec4; 16],
    ) -> Self {
        Self {
            holes: height.holes,
            height_params: height.params,
            heights: height.heights.clone(),
            page_table: height.page_table.clone(),
            water: ClipmapWaterUniform {
                surface: Vec4::new(
                    water_level,
                    minimum_depth.max(0.0),
                    max_mip.min(15) as f32,
                    0.0,
                ),
                missing_pages,
            },
            mask,
        }
    }

    /// 相机吸附和高度页换代只刷新三个共享绑定及固定缺页表，不重建遮罩或实体。
    pub(super) fn sync_height(
        &mut self,
        height: &ClipmapHeightExtension,
        max_mip: u32,
        missing_pages: [UVec4; 16],
    ) -> bool {
        let changed = self.height_params != height.params
            || self.holes != height.holes
            || self.heights != height.heights
            || self.page_table != height.page_table
            || self.water.surface.z != max_mip.min(15) as f32
            || self.water.missing_pages != missing_pages;
        if changed {
            self.holes = height.holes;
            self.height_params = height.params;
            self.heights = height.heights.clone();
            self.page_table = height.page_table.clone();
            self.water.surface.z = max_mip.min(15) as f32;
            self.water.missing_pages = missing_pages;
        }
        changed
    }
}

impl MaterialExtension for ClipmapWaterExtension {
    fn vertex_shader() -> ShaderRef {
        MAIN_VERTEX_SHADER.clone().into()
    }

    fn prepass_vertex_shader() -> ShaderRef {
        PREPASS_VERTEX_SHADER.clone().into()
    }

    fn deferred_vertex_shader() -> ShaderRef {
        PREPASS_VERTEX_SHADER.clone().into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        PREPASS_FRAGMENT_SHADER.clone().into()
    }

    fn enable_shadows() -> bool {
        false // 透明水接收阴影，但不把共享整张环带投成阴影。
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.vertex.shader_defs.push("CLIPMAP_WATER".into());
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader_defs.push("CLIPMAP_WATER".into());
        }
        Ok(())
    }
}

/// 每个非空静水层共享同一正式环带 Mesh，只持有独立 R8 和外观参数。
pub type ClipmapWaterMaterial = ExtendedMaterial<DynamicWaterMaterial, ClipmapWaterExtension>;

#[cfg(test)]
pub(crate) fn test_clipmap_water_material() -> ClipmapWaterMaterial {
    ClipmapWaterMaterial {
        base: DynamicWaterMaterial::default(),
        extension: ClipmapWaterExtension {
            holes: crate::maps::dynamic_map::clipmap::material::ClipmapHoleUniform::from_holes(&[]),
            height_params: ClipmapHeightUniform {stitches: Default::default(),
                map_bounds: Vec4::new(0.0, 0.0, 64.0, 64.0),
                grid: Vec4::new(1.0, 1.0, 64.0, 64.0),
                options: Vec4::new(0.0, 1.0, 0.0, 0.0),
                reserved: Vec4::ZERO,
                source_to_world: bevy::math::Mat4::IDENTITY,
                world_to_source: bevy::math::Mat4::IDENTITY,
            },
            heights: Handle::default(),
            page_table: Handle::default(),
            water: ClipmapWaterUniform {
                surface: Vec4::new(2.0, 0.2, 0.0, 0.0),
                missing_pages: [UVec4::ZERO; 16],
            },
            mask: Handle::default(),
        },
    }
}

/// 雨水高度相机复用同一环带、波形、高度页和R8裁切，禁止把环带Aabb当作整片水面。
#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub(crate) struct ClipmapRainHeightMaterial {
    // 雨水代理执行同一岸线裁切函数，必须绑定正式洞口，不能只在测试夹具中补齐129号槽。
    #[uniform(129)]
    holes: crate::maps::dynamic_map::clipmap::material::ClipmapHoleUniform,
    #[uniform(100)]
    waves: DynamicWaterUniform,
    #[uniform(120)]
    height_params: ClipmapHeightUniform,
    #[texture(121, dimension = "2d_array")]
    #[sampler(122)]
    heights: Handle<Image>,
    #[texture(123, sample_type = "u_int")]
    page_table: Handle<Image>,
    #[uniform(124)]
    water: ClipmapWaterUniform,
    #[texture(125)]
    #[sampler(126)]
    mask: Handle<Image>,
    #[uniform(127)]
    coverage: Vec4,
}

impl ClipmapRainHeightMaterial {
    pub(crate) fn from_source(source: &ClipmapWaterMaterial) -> Self {
        Self {
            holes: source.extension.holes,
            waves: source.base.extension.uniform,
            height_params: source.extension.height_params,
            heights: source.extension.heights.clone(),
            page_table: source.extension.page_table.clone(),
            water: source.extension.water,
            mask: source.extension.mask.clone(),
            coverage: Vec4::new(source.base.base.base_color.alpha(), 0.0, 0.0, 0.0),
        }
    }

    pub(crate) fn sync_source(&mut self, source: &ClipmapWaterMaterial) -> bool {
        let next = Self::from_source(source);
        let changed = self.holes != next.holes
            || self.waves != next.waves
            || self.height_params != next.height_params
            || self.heights != next.heights
            || self.page_table != next.page_table
            || self.water != next.water
            || self.mask != next.mask
            || self.coverage != next.coverage;
        if changed {
            *self = next;
        }
        changed
    }

    pub(crate) fn disable(&mut self) {
        self.coverage = Vec4::ZERO;
    }
}

impl Material for ClipmapRainHeightMaterial {
    fn vertex_shader() -> ShaderRef {
        MAIN_VERTEX_SHADER.clone().into()
    }

    fn fragment_shader() -> ShaderRef {
        RAIN_HEIGHT_SHADER.clone().into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

pub(crate) struct ClipmapWaterMaterialPlugin;

impl Plugin for ClipmapWaterMaterialPlugin {
    fn build(&self, app: &mut App) {
        load_shader_library!(app, "../shaders/clipmap_water.wgsl");
        load_internal_asset!(
            app,
            MAIN_VERTEX_SHADER,
            "../shaders/clipmap_water_vertex.wgsl",
            Shader::from_wgsl
        );
        load_internal_asset!(
            app,
            PREPASS_VERTEX_SHADER,
            "../shaders/clipmap_water_prepass.wgsl",
            Shader::from_wgsl
        );
        load_internal_asset!(
            app,
            PREPASS_FRAGMENT_SHADER,
            "../shaders/clipmap_water_prepass_fragment.wgsl",
            Shader::from_wgsl
        );
        load_internal_asset!(
            app,
            RAIN_HEIGHT_SHADER,
            "../shaders/clipmap_water_height.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins((
            MaterialPlugin::<ClipmapWaterMaterial>::default(),
            MaterialPlugin::<ClipmapRainHeightMaterial>::default(),
        ));
    }
}

#[cfg(test)]
mod shoreline_tests;

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    fn flatten(source: &str) -> String {
        let prefix = if source.contains("#define_import_path feathrise_scene::clipmap_height") {
            feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false)
        } else {
            String::new()
        }; // 与正式材质加载器使用相同的接缝声明和函数。
        let source = format!("{prefix}\n{source}");
        source
            .lines()
            .filter(|line| {
                let line = line.trim();
                !line.starts_with("#define_import_path") && !line.starts_with("#import ")
            })
            .collect::<Vec<_>>()
            .join("\n")
            .replace("#{MATERIAL_BIND_GROUP}", "2")
            .replace("clipmap_height::", "")
            .replace("clipmap_water::", "")
    }

    #[test]
    fn clipmap_water_coverage_and_rain_height_fragment_are_valid_wgsl() {
        let height = flatten(include_str!("../../clipmap/shaders/height.wgsl"));
        let water = flatten(include_str!("../shaders/clipmap_water.wgsl"));
        let fragment = flatten(include_str!("../shaders/clipmap_water_height.wgsl"))
            .replace("#ifdef VERTEX_UVS_A", "")
            .replace("#ifdef VERTEX_UVS_B", "")
            .replace("#else\n    discard;", "")
            .replace("#endif", "");
        let source = format!(
            "struct VertexOutput {{ @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(1) uv: vec2<f32>, @location(3) uv_b: vec2<f32>, }}\n{height}\n{water}\n{fragment}"
        ); // 仅替代Bevy导入的IO类型，所有高度、缺页、R8、水深和编码正文均来自生产文件。
        let module = wgpu::naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| {
            panic!(
                "Clipmap雨水高度片元语法错误：{}",
                error.emit_to_string(&source)
            )
        });
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("Clipmap雨水高度片元语义错误：{error}"));
    }

    #[test]
    fn clipmap_water_motion_uses_stable_world_position() {
        let main = include_str!("../shaders/clipmap_water_vertex.wgsl");
        let prepass = include_str!("../shaders/clipmap_water_prepass.wgsl");
        let normalized_prepass = prepass.replace("\r\n", "\n");
        let shared = include_str!("../../../../shared/large_map/water/shaders/water.wgsl");
        assert!(main.contains("stable_world_xz"));
        assert!(prepass.contains("stable_world_xz"));
        assert!(normalized_prepass.contains("sample_dynamic_water(\n        true,"));
        assert!(shared.contains("dynamic_water_uniform.previous.time_and_visual"));
        assert!(prepass.contains("previous.local_position"));
        assert!(!prepass.contains("get_previous_world_from_local"));
        assert!(include_str!("../shaders/clipmap_water.wgsl").contains("endpoint_aligned_mask_uv"));
        assert!(
            include_str!("../../../../shared/large_map/water/shaders/fragment.wgsl")
                .contains("surface_coverage(in.uv, in.uv_b)")
        );
    }

    /// 显式门控后让真实驱动编译静水主顶点/岸缘片元和运动预通道；普通测试不初始化显卡。
    #[test]
    fn clipmap_water_gpu_pipelines_compile() {
        let Ok(backend) = std::env::var("CLIPMAP_WATER_GPU_BACKEND") else {
            return;
        };
        let (backends, expected) = match backend.to_ascii_lowercase().as_str() {
            "dx12" => (wgpu::Backends::DX12, wgpu::Backend::Dx12),
            "vulkan" => (wgpu::Backends::VULKAN, wgpu::Backend::Vulkan),
            _ => panic!("CLIPMAP_WATER_GPU_BACKEND必须是dx12或vulkan"),
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::VALIDATION,
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: None,
        });
        let adapter =
            bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            }))
            .expect("指定Clipmap静水GPU后端可用");
        assert_eq!(adapter.get_info().backend, expected);
        let (device, _) =
            bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("创建Clipmap静水测试设备");
        let main_attributes = wgpu::vertex_attr_array![
            0 => Float32x3,
            2 => Float32x2,
            3 => Float32x2
        ]; // 前向IO中的位置、UV0和UV1位置。
        let prepass_attributes = wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x2,
            2 => Float32x2
        ]; // Bevy预通道IO为同一Mesh分配独立Shader位置。

        compile_gpu_pipeline(
            &device,
            "clipmap_water_main_and_coverage",
            production_main_source(),
            &main_attributes,
            &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        );
        compile_gpu_pipeline(
            &device,
            "clipmap_water_motion_prepass",
            production_prepass_source(),
            &prepass_attributes,
            &[
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rg16Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                }),
            ],
        );
        eprintln!(
            "Clipmap water production pipelines: {:?} {:?}",
            adapter.get_info().backend,
            adapter.get_info().name
        );
    }

    fn compile_gpu_pipeline(
        device: &wgpu::Device,
        label: &'static str,
        source: String,
        attributes: &[wgpu::VertexAttribute],
        targets: &[Option<wgpu::ColorTargetState>],
    ) {
        let validation_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(source)),
        });
        let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 28,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets,
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        }); // 创建失败会由WGPU验证层直接令测试失败。
        let validation_error = bevy::tasks::block_on(validation_scope.pop());
        assert!(
            validation_error.is_none(),
            "Clipmap静水GPU管线{label}验证失败：{validation_error:?}"
        ); // 捕获WGPU可见验证错误；显式门控运行还需检查Vulkan SDK验证层输出。
    }

    fn production_main_source() -> String {
        let mut source = common_gpu_fixture(
            "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) world_position: vec4<f32>, @location(1) world_normal: vec3<f32>, @location(2) uv: vec2<f32>, @location(3) uv_b: vec2<f32>, }\n",
            2,
            3,
        );
        for production in [
            include_str!("../../../../shared/large_map/water/shaders/water.wgsl"),
            include_str!("../../clipmap/shaders/height.wgsl"),
            include_str!("../shaders/clipmap_water.wgsl"),
            include_str!("../shaders/clipmap_water_vertex.wgsl"),
            include_str!("../shaders/clipmap_water_height.wgsl"),
        ] {
            source.push_str(&preprocess_gpu_source(
                production,
                &["VERTEX_UVS_A", "VERTEX_UVS_B"],
            ));
        }
        source
    }

    fn production_prepass_source() -> String {
        let mut source = common_gpu_fixture(
            "struct VertexOutput { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) uv_b: vec2<f32>, @location(2) world_normal: vec3<f32>, @location(4) world_position: vec4<f32>, @location(5) previous_world_position: vec4<f32>, }\nstruct FragmentOutput { @location(0) normal: vec4<f32>, @location(1) motion_vector: vec2<f32>, }\nfn calculate_motion_vector(current: vec4<f32>, previous: vec4<f32>) -> vec2<f32> { return current.xy - previous.xy; }\n",
            1,
            2,
        );
        let definitions = [
            "VERTEX_UVS_A",
            "VERTEX_UVS_B",
            "PREPASS_FRAGMENT",
            "NORMAL_PREPASS",
            "NORMAL_PREPASS_OR_DEFERRED_PREPASS",
            "MOTION_VECTOR_PREPASS",
        ];
        for production in [
            include_str!("../../../../shared/large_map/water/shaders/water.wgsl"),
            include_str!("../../clipmap/shaders/height.wgsl"),
            include_str!("../shaders/clipmap_water.wgsl"),
            include_str!("../shaders/clipmap_water_prepass.wgsl"),
            include_str!("../shaders/clipmap_water_prepass_fragment.wgsl"),
        ] {
            source.push_str(&preprocess_gpu_source(production, &definitions));
        }
        source
    }

    pub(super) fn common_gpu_fixture(vertex_output: &str, uv_a_location: u32, uv_b_location: u32) -> String {
        format!(
            "struct Vertex {{ @builtin(instance_index) instance_index: u32, @location(0) position: vec3<f32>, @location({uv_a_location}) uv: vec2<f32>, @location({uv_b_location}) uv_b: vec2<f32>, }}\n{vertex_output}struct Globals {{ time: f32, delta_time: f32, padding: vec2<f32>, }}\n@group(0) @binding(119) var<uniform> globals: Globals;\nfn get_world_from_local(index: u32) -> mat4x4<f32> {{ return mat4x4<f32>(vec4<f32>(1.0,0.0,0.0,0.0),vec4<f32>(0.0,1.0,0.0,0.0),vec4<f32>(0.0,0.0,1.0,0.0),vec4<f32>(0.0,0.0,0.0,1.0)); }}\nfn mesh_position_local_to_world(transform: mat4x4<f32>, position: vec4<f32>) -> vec4<f32> {{ return transform * position; }}\nfn position_world_to_clip(position: vec3<f32>) -> vec4<f32> {{ return vec4<f32>(position, 1.0); }}\n"
        ) // 只替换Bevy宿主IO与变换接口，水波、高度页、R8和运动正文逐字来自生产文件。
    }

    pub(super) fn preprocess_gpu_source(source: &str, definitions: &[&str]) -> String {
        let prefix = if source.contains("#define_import_path feathrise_scene::clipmap_height") {
            feathrise_environment::vegetation::blade::clipmap_stitch_shader_source(false)
        } else {
            String::new()
        };
        let source = format!("{prefix}\n{source}");
        let mut output = String::new();
        let mut active = vec![true];
        let mut multiline_import = false;
        for line in source.lines() {
            let trimmed = line.trim();
            if multiline_import {
                if trimmed == "}" {
                    multiline_import = false;
                }
                continue;
            }
            if trimmed.starts_with("#define_import_path") {
                continue;
            }
            if trimmed.starts_with("#import ") {
                multiline_import = trimmed.ends_with("{");
                continue;
            }
            if let Some(symbol) = trimmed.strip_prefix("#ifdef ") {
                active.push(*active.last().expect("条件栈非空") && definitions.contains(&symbol));
                continue;
            }
            if trimmed == "#else" {
                let previous = active.pop().expect("else必须有对应ifdef");
                let parent = *active.last().expect("条件栈保留根");
                active.push(parent && !previous);
                continue;
            }
            if trimmed == "#endif" {
                active.pop().expect("endif必须有对应ifdef");
                continue;
            }
            if *active.last().expect("条件栈非空") {
                output.push_str(line);
                output.push('\n');
            }
        }
        assert_eq!(active.len(), 1, "生产Shader条件必须完整闭合");
        assert!(!multiline_import, "生产Shader多行import必须完整闭合");
        output
            .replace("#{MATERIAL_BIND_GROUP}", "0")
            .replace("mesh_functions::", "")
            .replace("clipmap_height::", "")
            .replace("clipmap_water::", "")
    }
}
