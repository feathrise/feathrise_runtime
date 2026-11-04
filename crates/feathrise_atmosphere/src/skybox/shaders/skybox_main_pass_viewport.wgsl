// 基于Bevy 0.19.1 skybox.wgsl；仅把天空射线寻址改为真实主渲染通道viewport。
#import bevy_render::view::View
#import bevy_pbr::utils::coords_to_viewport_uv

// 与Bevy SkyboxUniforms的ShaderType布局保持逐字段一致。
struct SkyboxUniforms {
    brightness: f32,
    transform: mat4x4<f32>,
#ifdef SIXTEEN_BYTE_ALIGNMENT
    _wasm_padding_8b: u32,
    _wasm_padding_12b: u32,
    _wasm_padding_16b: u32,
#endif
}

// 绑定布局必须与Bevy SkyboxPipeline保持一致，兼容层不新增任何GPU资源。
@group(0) @binding(0) var skybox: texture_cube<f32>;
@group(0) @binding(1) var skybox_sampler: sampler;
@group(0) @binding(2) var<uniform> view: View;
@group(0) @binding(3) var<uniform> uniforms: SkyboxUniforms;

// 从当前片元在实际绘制viewport中的位置重建不受世界大坐标影响的天空方向。
fn coords_to_ray_direction(position: vec2<f32>, viewport: vec4<f32>) -> vec3<f32> {
    // 使用无限反向投影的近裁剪平面，避免在无穷远处生成非有限世界坐标。
    let view_position_homogeneous = view.view_from_clip * vec4(
        coords_to_viewport_uv(position, viewport) * vec2(2.0, -2.0) + vec2(-1.0, 1.0),
        1.0,
        1.0,
    );

    // 先消除齐次分量，再把视空间方向旋转到世界空间。
    var view_ray_direction = view_position_homogeneous.xyz / view_position_homogeneous.w;
    view_ray_direction = (view.world_from_view * vec4(view_ray_direction, 0.0)).xyz;

    // 作者天空旋转最后应用；方向向量的w为零，因此不会继承平移。
    let ray_direction = (uniforms.transform * vec4(view_ray_direction, 0.0)).xyz;
    return normalize(ray_direction);
}

// 全屏三角形只输出光栅位置，不改变Bevy原有深度与裁剪行为。
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn skybox_vertex(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    // 三个顶点覆盖完整裁剪空间，超出部分由固定功能裁剪。
    let clip_position = vec2(
        f32(vertex_index & 1u),
        f32((vertex_index >> 1u) & 1u),
    ) * 4.0 - vec2(1.0);
    return VertexOutput(vec4(clip_position, 0.0, 1.0));
}

@fragment
fn skybox_fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // 关键修正：DLSS/固定比例缩小主通道时，必须用内部viewport而不是最终输出viewport。
    let ray_direction = coords_to_ray_direction(in.position.xy, view.main_pass_viewport);

    // Cube贴图为左手坐标，保持Bevy原有Z轴翻转与亮度计算。
    let out = textureSample(skybox, skybox_sampler, ray_direction * vec3(1.0, 1.0, -1.0));
    return vec4(out.rgb * uniforms.brightness, out.a);
}
