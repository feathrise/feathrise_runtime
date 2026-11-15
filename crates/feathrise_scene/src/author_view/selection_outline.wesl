// 对选中模型的投影遮罩做外向扩张。矩形范围框、网格背边和三角对角线都不会成为物体轮廓。
@if(SELECTION_MASK) import bevy_render::view::View;
@if(SELECTION_MASK) @group(0) @binding(0) var<uniform> view: View;
@if(SELECTION_MASK) @group(0) @binding(1) var<uniform> world_from_model: mat4x4<f32>;
@if(SELECTION_MASK) @vertex
fn mask_vertex(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return view.clip_from_world * world_from_model * vec4<f32>(position, 1.0);
}
@if(SELECTION_MASK) @fragment
fn mask_fragment() -> @location(0) f32 {
    return 1.0;
}

@if(!SELECTION_MASK) @group(0) @binding(0) var coverage: texture_2d<f32>;
@if(!SELECTION_MASK)
fn sample_coverage(p: vec2<i32>, edge: vec2<i32>) -> f32 {
    return textureLoad(coverage, clamp(p, vec2<i32>(0), edge), 0).r;
}
@if(!SELECTION_MASK) @fragment
fn outline_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy);
    let edge = vec2<i32>(textureDimensions(coverage)) - 1;
    if sample_coverage(p, edge) > 0.5 { discard; }
    // 近一圈为实线，外圈四个方向只补柔边。采样次数固定，避免每像素遍历方形区域。
    let cardinal = max(max(sample_coverage(p + vec2<i32>( 1, 0), edge), sample_coverage(p + vec2<i32>(-1, 0), edge)),
                       max(sample_coverage(p + vec2<i32>( 0, 1), edge), sample_coverage(p + vec2<i32>( 0,-1), edge)));
    let diagonal = max(max(sample_coverage(p + vec2<i32>( 1, 1), edge), sample_coverage(p + vec2<i32>(-1, 1), edge)),
                       max(sample_coverage(p + vec2<i32>( 1,-1), edge), sample_coverage(p + vec2<i32>(-1,-1), edge)));
    let outer = max(max(sample_coverage(p + vec2<i32>( 2, 0), edge), sample_coverage(p + vec2<i32>(-2, 0), edge)),
                    max(sample_coverage(p + vec2<i32>( 0, 2), edge), sample_coverage(p + vec2<i32>( 0,-2), edge)));
    let alpha = max(max(cardinal, diagonal * 0.94), outer * 0.63);
    if alpha == 0.0 { discard; }
    return vec4<f32>(1.30, 0.43, 0.018, alpha); // 略抬高亮部，补偿 HDR 色调映射后橙边变暗。
}
