//! 固定渲染比例空间放大的全屏管线。

use bevy::{
    asset::{AssetServer, Handle, load_embedded_asset},
    core_pipeline::FullscreenShader,
    prelude::{Commands, Res, Resource},
    render::render_resource::{
        BindGroupLayoutDescriptor, BindGroupLayoutEntries, ColorTargetState, ColorWrites,
        FragmentState, RenderPipelineDescriptor, ShaderStages, SpecializedRenderPipeline,
        TextureFormat, TextureSampleType, binding_types::texture_2d, binding_types::uniform_buffer,
    },
    shader::Shader,
};

use crate::components::RenderScaleView;

/// 按当前视图 LDR/HDR 中间目标格式专用化的空间放大管线模板。
#[derive(Resource)]
pub(super) struct RenderScalePipeline {
    pub(super) layout: BindGroupLayoutDescriptor, // 源颜色纹理与动态逐视图尺寸 uniform。
    fullscreen_shader: FullscreenShader,          // Bevy 公共无顶点缓冲全屏三角形。
    fragment_shader: Handle<Shader>,              // 四次 textureLoad 手工双线性放大着色器。
}

/// RenderDevice 可用后建立一次管线模板；具体 LDR/HDR 变体保持异步专用化。
pub(super) fn init_render_scale_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    fullscreen_shader: Res<FullscreenShader>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "render_scale_bind_group_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: false }), // 手工四采样兼容 LDR 与不保证线性过滤的 HDR 目标。
                uniform_buffer::<RenderScaleView>(true), // 多窗口视图通过动态偏移共享一份 uniform 缓冲。
            ),
        ),
    );
    commands.insert_resource(RenderScalePipeline {
        layout,
        fullscreen_shader: fullscreen_shader.clone(),
        fragment_shader: load_embedded_asset!(asset_server.as_ref(), "render_scale.wesl"),
    });
}

impl SpecializedRenderPipeline for RenderScalePipeline {
    type Key = TextureFormat; // Bloom 切换 HDR 时只重新专用化颜色附件格式。

    fn specialize(&self, target_format: Self::Key) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("render_scale_spatial_upscale_pipeline".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen_shader.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.fragment_shader.clone(),
                targets: vec![Some(ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    /// 把 Bevy 预处理导入替换为真实接口声明后，使用当前 wgpu 同版本 Naga 验证生产 WGSL。
    #[test]
    fn spatial_upscale_shader_is_valid_wgsl() {
        let shader = include_str!("render_scale.wesl").replacen(
            "import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;",
            r#"struct FullscreenVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};"#,
            1,
        ); // 只替换 Bevy 资产预处理指令，其余文本与运行时生产着色器完全一致。
        let module = wgpu::naga::front::wgsl::parse_str(&shader)
            .unwrap_or_else(|error| panic!("固定渲染比例 WGSL 语法错误：{error}"));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("固定渲染比例 WGSL 语义错误：{error}"));
    }
}
