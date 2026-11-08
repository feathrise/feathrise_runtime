//! GPU草地基础渲染管线的创建与专用化。
//! 本模块复用Bevy标准MeshPipeline，只追加紧凑实例流和全局交互绑定组。

use bevy::{
    core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT, // 近景草阴影写入与Bevy方向光阴影图一致的深度格式。
    mesh::{MeshVertexBufferLayoutRef, VertexBufferLayout}, // 根据原型网格布局专用化管线并追加实例槽。
    pbr::{MeshPipeline, MeshPipelineKey, PrepassPipeline}, // 主画面复用MeshPipeline，阴影复用Bevy预通道视图和网格布局。
    prelude::*, // Commands、Resource与资产句柄等常用类型。
    render::render_resource::{
        BindGroupLayoutDescriptor,    // 保存可由PipelineCache解析的草地group3声明。
        BindGroupLayoutEntries,       // 按binding0至4顺序构造当前/上一帧交互与内容profile布局。
        CompareFunction,              // 阴影图沿用Bevy反向Z的大于等于深度比较。
        DepthBiasState,               // 草地阴影深度偏移保持零并交由方向光采样偏移控制。
        DepthStencilState,            // 近景草阴影管线只写深度，不产生颜色附件。
        Face,                         // 禁用草叶背面剔除。
        FragmentState,                // 不支持原生深度裁剪控制时用片元深度回退。
        MultisampleState,             // 方向光阴影贴图使用单采样状态。
        PrimitiveState,               // 阴影管线设置双面草叶和方向光正交深度裁剪。
        RenderPipelineDescriptor,     // 覆写标准MeshPipeline的着色器与实例布局。
        SamplerBindingType,           // 原草颜色纹理使用过滤采样器。
        ShaderStages,                 // 交互uniform和基础纹理供顶点与片元阶段读取。
        SpecializedMeshPipeline,      // 按视图、MSAA、HDR和拓扑缓存管线变体。
        SpecializedMeshPipelineError, // 透传标准网格布局专用化错误。
        StencilFaceState,             // 阴影深度不读取或写入模板。
        StencilState,                 // 明确构造关闭的模板状态。
        TextureSampleType,            // 基础草色纹理为可过滤浮点采样类型。
        VertexAttribute,              // 声明紧凑实例根位置与打包外观属性。
        VertexFormat,                 // 根位置使用Float32x3，外观使用Uint32。
        VertexState,                  // 阴影管线指定草地专用顶点入口。
        VertexStepMode,               // 第二顶点槽按实例推进。
        binding_types::{sampler, texture_2d, uniform_buffer}, // 以Bevy类型安全辅助函数声明group3。
    },
};

use crate::constants::GPU_GRASS_COMPACT_INSTANCE_BYTES; // Rust实例步长与WGSL紧凑输出布局共用常量。
use crate::resources::{GpuBladeInteraction, GpuBladeProfiles}; // group3两个uniform声明分别匹配风场足迹与固定内容profile表。
use crate::shaders::GPU_GRASS_DRAW_SHADER_HANDLE; // crate 内部固定句柄消除宿主 shader 路径要求。

/// GPU 草地管线资源：复用 Bevy MeshPipeline，仅替换顶点与片元着色器。
#[derive(Resource)]
pub(super) struct GpuBladePipeline {
    pub(super) shader: Handle<Shader>, // 预通道与阴影必须复用同一形变源码，禁止形成第三套草叶轮廓。
    mesh_pipeline: MeshPipeline,       // Bevy负责视图、深度和网格绑定布局的基础管线。
    pub(super) interaction_layout: BindGroupLayoutDescriptor, // 交互模块与专用化管线共享group3声明。
}

/// 近景草方向光阴影管线：复用主画面的紧凑实例流和风场形变，但只生成深度。
#[derive(Resource)]
pub(super) struct GpuBladeShadowPipeline {
    shader: Handle<Shader>, // 与主画面共用同一顶点形变源码，保证阴影跟随风摆和踩踏后的真实轮廓。
    prepass_pipeline: PrepassPipeline, // 复用Bevy阴影视图、空绑定组和标准网格绑定布局。
    interaction_layout: BindGroupLayoutDescriptor, // group3继续绑定风场、足迹和内容profile，不复制逐实例数据。
}

/// 渲染子应用启动时创建 GPU 草地基础管线资源。
pub(super) fn init_gpu_grass_pipeline(
    mut commands: Commands,           // 向渲染世界插入草地基础管线资源。
    mesh_pipeline: Res<MeshPipeline>, // 克隆 Bevy 标准 3D 网格管线配置。
) {
    let interaction_layout = BindGroupLayoutDescriptor::new(
        "gpu_grass_interaction_bind_group_layout", // 图形调试器中明确标识草地动态形变输入。
        &BindGroupLayoutEntries::sequential(
            ShaderStages::VERTEX | ShaderStages::FRAGMENT, // uniform仅供顶点使用，原草纹理仅供片元使用；统一布局保持单个全局绑定组。
            (
                uniform_buffer::<GpuBladeInteraction>(false), // binding0为不使用动态offset的全局动态数据。
                texture_2d(TextureSampleType::Float { filterable: true }), // binding1为自动执行sRGB到线性解码的原草颜色纹理。
                sampler(SamplerBindingType::Filtering), // binding2复用图片资产的过滤采样器，避免近景纹理锯齿。
                uniform_buffer::<GpuBladeProfiles>(false), // binding3固定保存普通草、麦田和后续两种内容profile。
                uniform_buffer::<GpuBladeInteraction>(false), // binding4保存上一完整渲染帧的风场与足迹，供运动矢量重建真实旧姿态。
            ),
        ),
    ); // 完成与WGSL group3 binding0至4严格一致的声明式布局。
    commands.insert_resource(GpuBladePipeline {
        shader: GPU_GRASS_DRAW_SHADER_HANDLE.clone(), // 使用插件构建阶段注册的内置 WGSL 句柄。
        mesh_pipeline: mesh_pipeline.clone(),         // 保留Bevy 0.19当前视图和网格绑定约定。
        interaction_layout, // 专用化管线在标准view、array、mesh三组后追加草地group3。
    }); // 完成管线资源初始化。
}

/// Bevy预通道资源在RenderStartup完成后创建近景草阴影专用化模板。
pub(super) fn init_gpu_grass_shadow_pipeline(
    mut commands: Commands, // 首个渲染帧准备阶段向RenderWorld补入阴影管线资源。
    main_pipeline: Res<GpuBladePipeline>, // 复用已经加载的WGSL句柄和全局交互布局。
    prepass_pipeline: Res<PrepassPipeline>, // 读取Bevy已经初始化完成的阴影视图与网格布局。
    existing: Option<Res<GpuBladeShadowPipeline>>, // 热重载和后续帧不得重复覆盖专用化模板。
) {
    if existing.is_some() {
        return; // 阴影管线资源已经建立后保持稳定，后续帧只复用异步专用化缓存。
    } // 完成一次性初始化门。
    commands.insert_resource(GpuBladeShadowPipeline {
        shader: main_pipeline.shader.clone(), // 主画面和阴影共用同一热加载WGSL资产。
        prepass_pipeline: prepass_pipeline.clone(), // 保存Bevy方向光阴影所需的预通道布局与硬件能力。
        interaction_layout: main_pipeline.interaction_layout.clone(), // 阴影顶点读取与主画面完全相同的风和内容profile。
    }); // 完成近景草阴影专用化模板安装。
} // 结束近景草阴影管线延迟初始化。

impl SpecializedMeshPipeline for GpuBladePipeline {
    type Key = MeshPipelineKey; // 使用 Bevy 标准网格管线键避免自建目标格式分支。

    fn specialize(
        &self,
        key: Self::Key,                     // 当前视图 MSAA、HDR 与网格拓扑组合。
        layout: &MeshVertexBufferLayoutRef, // 原型网格的 position/normal/uv 布局。
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mesh_vertex_layout = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0), // 顶点形变必须取得原型局部位置。
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),   // 叶片宽轴、风摆和光照共用正式法线。
            Mesh::ATTRIBUTE_UV_0.at_shader_location(2),     // 根尖比例与图集采样必须为 Float32x2。
        ])?; // 缺少任一属性时返回可诊断专用化错误，禁止创建不匹配的 wgpu 管线。
        let mut descriptor = self.mesh_pipeline.specialize(key, layout)?; // 先生成与当前 Bevy 版本匹配的标准描述符。
        descriptor.label = Some("gpu_grass_minimal_pipeline".into()); // 让图形调试器可直接识别草地管线。
        descriptor.vertex.shader = self.shader.clone(); // 顶点阶段只解码Compute输出的世界根位置和外观。
        descriptor.vertex.buffers = vec![mesh_vertex_layout]; // 自定义 WGSL 只声明三项原型属性，不沿用标准 PBR 的可选缺省布局。
        descriptor.vertex.buffers.push(VertexBufferLayout {
            array_stride: GPU_GRASS_COMPACT_INSTANCE_BYTES, // WGSL storage输出和实例顶点流共用严格二十字节步长。
            step_mode: VertexStepMode::Instance, // 每个存活草簇读取一条紧凑记录，当前近17、中14、远21顶点共享同一实例值。
            attributes: vec![
                VertexAttribute {
                    format: VertexFormat::Float32x3, // xyz为Compute已经贴地的世界草根位置。
                    offset: 0,                       // 紧凑记录前三个f32从字节零开始。
                    shader_location: 3,              // 原型position、normal与uv已占用location0至2。
                },
                VertexAttribute {
                    format: VertexFormat::Uint32, // 四个八位通道打包cos、sin、缩放、稳定外观与材质级别。
                    offset: 12,                   // packed_appearance紧跟三个f32。
                    shader_location: 4,           // WGSL用整数输入无损解包外观。
                },
                VertexAttribute {
                    format: VertexFormat::Uint32,
                    offset: 16,
                    shader_location: 5,
                },
            ],
        }); // 完成第二顶点槽的二十字节实例布局。
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = self.shader.clone(); // 使用同一WGSL执行植被光照与完整不透明草叶输出。
        } // 完成片元着色器替换。
        if let Some(depth_stencil) = descriptor.depth_stencil.as_mut() {
            // Deferred预通道已经提交了同一草叶的权威深度；主通道overlay只恢复植被颜色，禁止重写历史输入。
            depth_stencil.depth_write_enabled = Some(gpu_blade_main_writes_depth(key)); // 保留Bevy的GreaterEqual，只切换写权限。
        } // 普通前向视图仍由主颜色阶段写深度，Deferred颜色overlay则只读既有深度。
        descriptor.layout.push(self.interaction_layout.clone()); // 在标准view0、数组1、mesh2后追加草地动态group3。
        descriptor.primitive.cull_mode = None::<Face>; // 原始单片分段草双面可见，避免背面观察时草叶消失。
        Ok(descriptor) // 返回已替换着色器的专用管线。
    }
}

/// 返回当前主颜色管线是否拥有草叶深度写权限。
#[inline]
fn gpu_blade_main_writes_depth(key: MeshPipelineKey) -> bool {
    !key.contains(MeshPipelineKey::DEFERRED_PREPASS) // Deferred视图的权威深度已经由AlphaMask3dDeferred提交。
}

impl SpecializedMeshPipeline for GpuBladeShadowPipeline {
    type Key = MeshPipelineKey; // 只按方向光深度模式与当前近景草原型拓扑缓存管线。

    fn specialize(
        &self,
        key: Self::Key,                     // 包含方向光正交深度与原型拓扑。
        layout: &MeshVertexBufferLayoutRef, // 近景草原型的position、normal和uv布局。
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mesh_vertex_layout = layout.0.get_layout(&[
            Mesh::ATTRIBUTE_POSITION.at_shader_location(0), // 草叶局部位置继续位于location0。
            Mesh::ATTRIBUTE_NORMAL.at_shader_location(1), // 阴影形变与主画面共用资产法线恢复草簇叶片宽轴。
            Mesh::ATTRIBUTE_UV_0.at_shader_location(2),   // 根尖UV继续决定株高、下扎和弯曲比例。
        ])?; // 校验近景草原型具备阴影形变需要的全部顶点属性。
        let instance_vertex_layout = VertexBufferLayout {
            array_stride: GPU_GRASS_COMPACT_INSTANCE_BYTES, // 直接消费Compute已经生成的二十字节紧凑实例。
            step_mode: VertexStepMode::Instance, // 每个草实例读取一次世界根位置和打包外观。
            attributes: vec![
                VertexAttribute {
                    format: VertexFormat::Float32x3, // xyz为已经贴合真实地形高度的草根世界坐标。
                    offset: 0,                       // 紧凑记录从字节零开始保存三个f32。
                    shader_location: 3,              // 接在原型position、normal和uv之后。
                }, // 完成草根实例属性声明。
                VertexAttribute {
                    format: VertexFormat::Uint32, // 旋转、缩放、内容profile和稳定外观保持无损整数打包。
                    offset: 12,                   // 打包字段紧跟十二字节世界根坐标。
                    shader_location: 4,           // 与主画面草地顶点入口保持同一location。
                },
                VertexAttribute {
                    format: VertexFormat::Uint32,
                    offset: 16,
                    shader_location: 5,
                }, // 完成外观实例属性声明。
            ], // 完成阴影实例槽的两项属性。
        }; // 完成二十字节实例顶点槽声明。
        let emulate_unclipped_depth = key.contains(MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO)
            && !self.prepass_pipeline.depth_clip_control_supported; // 老旧后端没有原生深度裁剪控制时启用片元深度回退。
        let mut shader_defs = vec!["GPU_GRASS_SHADOW".into()]; // 让WGSL只保留阴影入口和预通道全局绑定。
        if emulate_unclipped_depth {
            shader_defs.push("GPU_GRASS_SHADOW_UNCLIPPED_DEPTH_EMULATION".into()); // 用片元深度保存方向光正交视锥外的合法投影。
        } // 完成硬件深度裁剪能力分支。
        let fragment = emulate_unclipped_depth.then(|| FragmentState {

            constants: Vec::new(),
            shader: self.shader.clone(), // 回退片元入口仍位于同一热加载WGSL资产。
            shader_defs: shader_defs.clone(), // 片元和顶点必须共享深度回退宏。
            entry_point: Some("shadow_fragment".into()), // 只输出未裁剪方向光深度，不采样草色。
            targets: Vec::new(),         // 阴影通道没有任何颜色附件。
        }); // 原生支持深度裁剪控制时完全省略片元阶段。
        Ok(RenderPipelineDescriptor {
            label: Some("gpu_grass_near_shadow_pipeline".into()), // 图形调试器明确区分近景草主画面与阴影管线。
            layout: vec![
                self.prepass_pipeline.view_layout_no_motion_vectors.clone(), // group0绑定方向光阴影视图与全局时间。
                self.prepass_pipeline.empty_layout.clone(), // group1保持Bevy阴影预通道的空绑定组约定。
                self.prepass_pipeline.mesh_layouts.model_only.clone(), // group2绑定草窗标准网格uniform。
                self.interaction_layout.clone(), // group3绑定风场、足迹、综合色图和内容profile。
            ], // 完成与阴影绘制命令严格一致的四组布局。
            vertex: VertexState {

                constants: Vec::new(),
                shader: self.shader.clone(),               // 复用主画面同一草叶形变源码。
                shader_defs,                               // 选择阴影入口所需的预处理分支。
                entry_point: Some("shadow_vertex".into()), // 只输出方向光裁剪位置和可选回退深度。
                buffers: vec![mesh_vertex_layout, instance_vertex_layout], // 原型槽加Compute紧凑实例槽。
            }, // 完成近景草阴影顶点阶段。
            fragment, // 只有缺少原生深度裁剪控制的后端才执行最小片元阶段。
            primitive: PrimitiveState {
                topology: key.primitive_topology(), // 跟随近景草原型的三角形拓扑。
                cull_mode: None::<Face>,            // 单片草叶双面都必须能向地表投射阴影。
                unclipped_depth: key.contains(MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO)
                    && self.prepass_pipeline.depth_clip_control_supported, // 支持时直接启用方向光正交阴影所需的无限深度。
                ..default() // 正面绕序和多边形模式保持Bevy默认值。
            }, // 完成阴影图几何状态。
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,    // 与Bevy方向光阴影图附件格式一致。
                depth_write_enabled: Some(true), // 近景草显式覆盖模板中的深度写入状态。
                depth_compare: Some(CompareFunction::GreaterEqual), // 沿用Bevy反向Z比较规则。
                stencil: StencilState {
                    front: StencilFaceState::IGNORE, // 阴影图不使用正面模板。
                    back: StencilFaceState::IGNORE,  // 阴影图不使用背面模板。
                    read_mask: 0,                    // 禁止模板读取。
                    write_mask: 0,                   // 禁止模板写入。
                }, // 完成关闭模板状态。
                bias: DepthBiasState {
                    constant: 0,      // 草地几何不额外改变全局方向光深度偏移。
                    slope_scale: 0.0, // 双面薄叶不叠加自定义斜率偏移。
                    clamp: 0.0,       // 不对零偏移执行钳制。
                }, // 完成草地阴影管线局部深度偏移。
            }), // 完成仅深度输出状态。
            multisample: MultisampleState::default(), // 方向光阴影图保持单采样。
            ..default()  // 推送常量与工作组初始化策略沿用Bevy默认值。
        }) // 返回可被Shadow阶段缓存的近景草深度管线。
    } // 结束单个近景草阴影管线专用化。
} // 结束近景草阴影专用化实现。

#[cfg(test)] // 测试模块保持在实现之后，锁定Deferred颜色overlay不得再次改写权威预通道深度。
mod tests {
    use super::*; // 复用主草叶管线使用的Bevy MeshPipelineKey。

    #[test] // 只有Deferred视图的第二次颜色绘制需要关闭深度写入，普通光栅主通道仍拥有深度。
    fn deferred_forward_overlay_preserves_prepass_depth() {
        assert!(!gpu_blade_main_writes_depth(
            MeshPipelineKey::DEFERRED_PREPASS
        )); // Deferred颜色overlay不得覆盖已经进入Solari/RR历史的深度。
        assert!(gpu_blade_main_writes_depth(MeshPipelineKey::empty())); // 普通光栅主颜色阶段仍拥有草叶深度写入。
    } // 完成两类视图的深度所有权边界测试。
} // 结束主草叶管线纯逻辑测试模块。
