#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct RenderScaleView {
    render_size: vec2<u32>,
    output_size: vec2<u32>,
};

@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(0) @binding(1) var<uniform> settings: RenderScaleView;

// 手工双线性过滤避免依赖 HDR 纹理的可过滤特性，并严格夹紧在已绘制的左上主通道区域内。
fn load_render_texel(coordinate: vec2<i32>, maximum: vec2<i32>) -> vec4<f32> {
    return textureLoad(source_texture, clamp(coordinate, vec2<i32>(0), maximum), 0);
}

@fragment
fn fs_main(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let texture_size = textureDimensions(source_texture);
    let render_size = max(min(settings.render_size, texture_size), vec2<u32>(1));
    let output_uv = in.uv; // 全屏三角形直接提供 0..1 输出坐标，不依赖可能滞后一帧的缓存输出尺寸。
    let source_position = output_uv * vec2<f32>(render_size) - vec2<f32>(0.5);
    let base = vec2<i32>(floor(source_position));
    let fraction = fract(source_position);
    let maximum = vec2<i32>(render_size) - vec2<i32>(1);

    let top_left = load_render_texel(base, maximum);
    let top_right = load_render_texel(base + vec2<i32>(1, 0), maximum);
    let bottom_left = load_render_texel(base + vec2<i32>(0, 1), maximum);
    let bottom_right = load_render_texel(base + vec2<i32>(1, 1), maximum);
    let top = mix(top_left, top_right, fraction.x);
    let bottom = mix(bottom_left, bottom_right, fraction.x);
    return mix(top, bottom, fraction.y);
}
