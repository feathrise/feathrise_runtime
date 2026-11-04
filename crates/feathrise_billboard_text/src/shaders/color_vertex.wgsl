// Billboard 文字主颜色顶点入口。
//
// 该输出结构与 Bevy 0.19 的 forward_io::VertexOutput 保持相同 location ABI，
// clip_from_world/unjittered_clip_from_world由实际pass管线选择，普通回退不会继承独立投影。

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    mesh_view_bindings::view,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> billboard_projection_mode: f32;
#ifdef BILLBOARD_TEMPORAL_GUIDE
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<storage, read> guide_gate: vec4<f32>; // RenderWorld仅在实际分流成功时暂停引导。
#endif

struct BillboardVertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_TANGENTS
    @location(4) world_tangent: vec4<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(6) @interpolate(flat) instance_index: u32,
#endif
#ifdef VISIBILITY_RANGE_DITHER
    @location(7) @interpolate(flat) visibility_range_dither: i32,
#endif
}

fn billboard_project(world_position: vec3<f32>) -> vec4<f32> {
    // 独立WorldUI直接使用输出分辨率，普通管线与回退必须继续匹配场景抖动。
#ifdef WORLD_UI_PASS
    return view.unjittered_clip_from_world * vec4<f32>(world_position, 1.0); // 仅独立绘制管线使用无抖动投影。
#else
    return view.clip_from_world * vec4<f32>(world_position, 1.0);
#endif
}

@vertex
fn vertex(vertex: Vertex) -> BillboardVertexOutput {
    var out: BillboardVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    out.position = billboard_project(out.world_position.xyz);
#ifdef BILLBOARD_TEMPORAL_GUIDE
    if guide_gate.x > 0.5 { out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0); } // 分流成功后将引导裁出视锥，失败保留正常场景引导。
#endif

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index,
        world_from_local[3],
    );
#endif

    return out;
}
