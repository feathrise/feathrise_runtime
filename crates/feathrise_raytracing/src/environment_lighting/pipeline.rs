//! Solari环境IBL全屏管线与MeshView两组布局专用化。

use bevy::{
    asset::{AssetServer, Handle, load_embedded_asset}, // 读取本crate内嵌片元Shader。
    core_pipeline::FullscreenShader,                   // 复用标准全屏三角顶点入口。
    pbr::{MeshPipeline, MeshPipelineViewLayoutKey}, // 复用环境图、GBuffer、深度与视图Uniform布局。
    prelude::{Commands, Component, Res, Resource, default}, // RenderStartup资源和逐视图管线ID。
    render::render_resource::{
        BlendComponent,
        BlendFactor,
        BlendOperation,
        BlendState,
        CachedRenderPipelineId, // 原位RGB加法状态。
        ColorTargetState,
        ColorWrites,
        FragmentState,
        RenderPipelineDescriptor, // 片元管线描述。
        SpecializedRenderPipeline,
        TextureFormat, // 按视图小键缓存HDR变体。
    },
    shader::{Shader, ShaderDefVal}, // 嵌入Shader句柄与条件编译定义。
};

/// 活动视图已经专用化并可由Core3d节点查询的环境管线ID。
#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub(super) struct SolariEnvironmentPipelineId(pub(super) CachedRenderPipelineId);

/// 持久保存MeshView布局、设备绑定数组能力与全屏Shader句柄。
#[derive(Resource)]
pub(super) struct SolariEnvironmentPipeline {
    mesh_pipeline: MeshPipeline, // 提供与Bevy PBR完全一致的主布局、绑定数组布局和设备能力。
    fullscreen_shader: FullscreenShader, // 标准全屏三角不引入额外顶点缓冲。
    fragment_shader: Handle<Shader>, // 当前crate内嵌的环境IBL片元入口。
}

/// 环境项以`src + dst`写入HDR RGB，并保持主目标Alpha逐位不变。
pub(super) fn environment_color_target(format: TextureFormat) -> ColorTargetState {
    ColorTargetState {
        format, // 沿用当前视图真实HDR目标格式。
        blend: Some(BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::One,   // Shader输出已经包含曝光后的环境能量。
                dst_factor: BlendFactor::One,   // 保留Solari、草叶Forward与阴影兼容后的既有颜色。
                operation: BlendOperation::Add, // 只追加缺失环境项，不覆盖主颜色。
            },
            alpha: BlendComponent {
                src_factor: BlendFactor::Zero,  // 环境项不拥有覆盖度。
                dst_factor: BlendFactor::One,   // 主目标Alpha保持不变。
                operation: BlendOperation::Add, // 零加原值实现Alpha恒等。
            },
        }),
        write_mask: ColorWrites::RED | ColorWrites::GREEN | ColorWrites::BLUE, // 禁止写Alpha。
    }
}

/// MeshPipeline就绪后创建一次模板；具体视图变体继续由PipelineCache异步编译。
pub(super) fn init_solari_environment_pipeline(
    mut commands: Commands,                   // 插入只读持久模板资源。
    asset_server: Res<AssetServer>,           // 解析内嵌Shader句柄。
    mesh_pipeline: Res<MeshPipeline>,         // 取得真实设备的绑定数组选择与全部View布局。
    fullscreen_shader: Res<FullscreenShader>, // 使用Bevy统一全屏顶点状态。
) {
    commands.insert_resource(SolariEnvironmentPipeline {
        mesh_pipeline: mesh_pipeline.clone(), // 稳定持有布局描述，不逐帧查询或重建。
        fullscreen_shader: fullscreen_shader.clone(), // 复用标准顶点Shader资源。
        fragment_shader: load_embedded_asset!(
            asset_server.as_ref(),
            "solari_environment_lighting.wesl"
        ), // 嵌入资源由AssetServer统一热路径解析。
    }); // 完成环境管线模板登记。
}

/// 只保留会改变两组MeshView布局或HDR目标格式的逐视图键。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SolariEnvironmentPipelineKey {
    pub(super) mesh_view_layout_key: MeshPipelineViewLayoutKey, // 深度、Deferred与环境绑定布局身份。
    pub(super) target_format: TextureFormat,                    // HDR附件格式变化必须使用独立管线。
}

impl SpecializedRenderPipeline for SolariEnvironmentPipeline {
    type Key = SolariEnvironmentPipelineKey; // 小键由Prepare阶段从当前视图产生。

    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let view_layout = self
            .mesh_pipeline
            .view_layouts
            .get_view_layout(key.mesh_view_layout_key); // 与本帧MeshViewBindGroup使用完全相同的两组布局。
        let mut shader_defs = vec![
            ShaderDefVal::Bool("DEPTH_PREPASS".into(), true), // 片元从预通道深度恢复世界位置。
            ShaderDefVal::Bool("DEFERRED_PREPASS".into(), true), // 片元从GBuffer恢复PBR材质。
            ShaderDefVal::Bool("ENVIRONMENT_MAP".into(), true), // 编译Bevy标准环境贴图采样函数和group1绑定。
            ShaderDefVal::Bool("STANDARD_MATERIAL_CLEARCOAT".into(), true), // 对齐Bevy Deferred固定清漆字段ABI。
        ];
        if self.mesh_pipeline.binding_arrays_are_usable {
            shader_defs.push("MULTIPLE_LIGHT_PROBES_IN_ARRAY".into()); // Vulkan绑定数组设备同时支持局部反射探针与全局天空回退。
        }
        RenderPipelineDescriptor {
            label: Some("solari_environment_lighting_pipeline".into()), // GPU诊断可直接识别本兼容层。
            layout: vec![
                view_layout.main_layout, // group0含View、LightProbes、GBuffer、深度与cluster索引。
                view_layout.binding_array_layout, // group1含漫反射、镜面Cube及共享采样器。
            ],
            vertex: self.fullscreen_shader.to_vertex_state(), // 绘制单个全屏三角。
            fragment: Some(FragmentState {
                shader: self.fragment_shader.clone(), // 使用项目环境补光片元入口。
                shader_defs,                          // 只启用本节点实际消费的标准PBR分支。
                targets: vec![Some(environment_color_target(key.target_format))], // 原位HDR加法且不改Alpha。
                ..default()
            }),
            ..default()
        }
    }
}
