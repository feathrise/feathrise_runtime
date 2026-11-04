//! 无光照水高度材质，顶点直接复用实际水面宏观波、水平位移和玩家波。
use super::super::{shaders::DYNAMIC_WATER_VERTEX_SHADER_HANDLE, types::DynamicWaterUniform};
use bevy::asset::{load_internal_asset, uuid_handle}; // 固定shader身份随crate打包。
use bevy::mesh::MeshVertexBufferLayoutRef; // 专用化只修改剔除模式。
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey}; // 独立Opaque材质不污染原水PBR。
use bevy::prelude::*; // 资产、类型路径和shader句柄。
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
}; // 与原水一致的100号uniform。
use bevy::shader::ShaderRef; // 指定实际水面顶点入口和高度片元入口。 // 唯一生产波参数和GPU顶点算法。

const HEIGHT_SHADER: Handle<Shader> = uuid_handle!("75f0ded3-a061-4bd1-8cbb-0671cbac9f21"); // 高度片元固定身份。

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)] // 仅640字节固定参数，无额外网格或大纹理。
pub(in super::super) struct RainHeightMaterial {
    // Opaque深度自然保留同像素最高实际水面。
    #[uniform(100)] // 复用water.wgsl同一binding而不维护第二套波形。
    pub(super) uniform: DynamicWaterUniform, // 与原水材质本帧参数逐字一致。
    #[uniform(105)] // 基础透明度也必须保留，完全不可见水体不能生成覆盖。
    pub(super) coverage: Vec4, // X原材质Alpha，其余保留。
} // 完成捕获材质。

impl Material for RainHeightMaterial {
    // 数值图不进入灯光、雾或透明混合。
    fn vertex_shader() -> ShaderRef {
        DYNAMIC_WATER_VERTEX_SHADER_HANDLE.clone().into()
    } // 复用真实顶点位移和世界变换。
    fn fragment_shader() -> ShaderRef {
        HEIGHT_SHADER.clone().into()
    } // 仅输出分段高度与coverage。
    fn enable_prepass() -> bool {
        false
    } // 捕获相机自己的Opaque深度已足够，不增加预通道。
    fn enable_shadows() -> bool {
        false
    } // 不进入任何阴影阶段。
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // 原整网格水可以双面，捕获同样不能按绕序漏面。
        descriptor.primitive.cull_mode = None; // 真实可见的斜面和镜像变换水面都保留。
        Ok(()) // 保持标准Opaque深度写入与更近测试。
    } // 完成专用化。
} // 完成材质接口。

pub(super) fn register_shader(app: &mut App) {
    // 水顶点shader已由上层先登记。
    load_internal_asset!(app, HEIGHT_SHADER, "height.wgsl", Shader::from_wgsl); // 高度shader随程序打包。
} // 完成shader安装。
