// 通用实例标签渲染夹具：实际覆盖材质storage、MeshTag、独立投影与透明像素边缘。
// 无角色、生命值或游戏UI语义，SDK测试不依赖游戏生产资产。
#import bevy_pbr::mesh_functions
#import bevy_pbr::mesh_view_bindings::view

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<storage, read> instance_colors: array<vec4<f32>>;
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> opacity: f32;

struct LabelInput {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

struct LabelOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vertex(in: LabelInput) -> LabelOutput {
    var out: LabelOutput;
    let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
    let center_world = (world_from_local * vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    let position_world = center_world
        + view.world_from_view[0].xyz * in.position.x
        + view.world_from_view[1].xyz * in.position.y;
#ifdef WORLD_UI_PASS
    out.clip_position = view.unjittered_clip_from_world * vec4<f32>(position_world, 1.0);
    let center_clip = view.unjittered_clip_from_world * vec4<f32>(center_world, 1.0);
    let viewport = view.viewport.zw;
#else
    out.clip_position = view.clip_from_world * vec4<f32>(position_world, 1.0);
    let center_clip = view.clip_from_world * vec4<f32>(center_world, 1.0);
    let viewport = view.main_pass_viewport.zw;
#endif
    // 两种实际pass各按自己的像素尺寸向外扩一像素；片元覆盖率不会被四边形裁掉。
    let delta = out.clip_position.xy / out.clip_position.w - center_clip.xy / center_clip.w;
    let half_pixels = max(abs(delta) * viewport * 0.5, vec2<f32>(0.0001));
    out.clip_position = vec4<f32>(
        out.clip_position.xy + sign(delta) * 2.0 / viewport * out.clip_position.w,
        out.clip_position.zw,
    );
    out.uv = (in.uv - vec2<f32>(0.5))
        * (vec2<f32>(1.0) + vec2<f32>(1.0) / half_pixels) + vec2<f32>(0.5);
    out.color = instance_colors[mesh_functions::get_tag(in.instance_index)];
    return out;
}

fn coverage(x: f32, width: f32) -> f32 {
    return clamp(x / width + 0.5, 0.0, 1.0)
        - clamp((x - 1.0) / width + 0.5, 0.0, 1.0);
}

@fragment
fn fragment(in: LabelOutput) -> @location(0) vec4<f32> {
    // 保留实际导数与透明混合绑定，避免空白shader或另一项文字掩盖实例管线错误。
    let width = max(fwidth(in.uv), vec2<f32>(0.000001));
    let alpha = coverage(in.uv.x, width.x) * coverage(in.uv.y, width.y)
        * clamp(in.color.a * opacity, 0.0, 1.0);
    return vec4<f32>(in.color.rgb, alpha);
}
