#import bevy_pbr::{
    mesh_functions,
    prepass_io::Vertex,
    view_transformations::position_world_to_clip,
}
#import feathrise_scene::shared::large_map::water::water::{
    dynamic_water_uniform,
    sample_dynamic_water,
    dynamic_water_vertex_color,
}
#import bevy_pbr::mesh_view_bindings::globals

// 与Bevy prepass_io位置完全一致，并让预通道与Equal主通道共享位置不变量保证。
struct DynamicWaterPrepassVertexOutput {
    @builtin(position) @invariant position: vec4<f32>,
#ifdef VERTEX_UVS_A
    @location(0) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(1) uv_b: vec2<f32>,
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    @location(2) world_normal: vec3<f32>,
#ifdef VERTEX_TANGENTS
    @location(3) world_tangent: vec4<f32>,
#endif
#endif
    @location(4) world_position: vec4<f32>,
#ifdef MOTION_VECTOR_PREPASS
    @location(5) previous_world_position: vec4<f32>,
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(6) unclipped_depth: f32,
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(7) instance_index: u32,
#endif
#ifdef VERTEX_COLORS
    @location(8) color: vec4<f32>,
#endif
#ifdef VISIBILITY_RANGE_DITHER
    @location(9) @interpolate(flat) visibility_range_dither: i32,
#endif
}

// 深度、法线和延迟使用当前波形，运动向量使用保存的上一帧波源年龄与时间。
@vertex
fn vertex(vertex: Vertex) -> DynamicWaterPrepassVertexOutput {
    var out: DynamicWaterPrepassVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var rest_local_normal = vec3<f32>(0.0, 1.0, 0.0);
    var rest_local_tangent = vec3<f32>(0.0, 0.0, 0.0); // 与前向顶点一致：缺TANGENT只禁用方向性解析法线，不禁用波高和位移。
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    rest_local_normal = vertex.normal;
#endif
#ifdef VERTEX_TANGENTS
    rest_local_tangent = vertex.tangent.xyz;
#endif
#endif
    var rest_uv_b = vec2<f32>(-1.0, 0.0);
#ifdef VERTEX_UVS_B
    rest_uv_b = vertex.uv_b;
#endif
    var rest_vertex_alpha = 1.0;
#ifdef VERTEX_COLORS
    rest_vertex_alpha = vertex.color.a;
#endif
    let water_sample = sample_dynamic_water(
        false,
        globals.time, // 当前预通道与主通道共享RenderWorld时间。
        vertex.position,
        rest_local_normal,
        rest_local_tangent,
        vertex.uv,
        rest_uv_b,
        rest_vertex_alpha,
    );

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(water_sample.local_position, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        water_sample.local_normal,
        vertex.instance_index,
    );
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
#endif
#endif

#ifdef VERTEX_COLORS
    out.color = dynamic_water_vertex_color(
        vertex.color,
        water_sample.visual_signal, // 预通道只保存玩家波谷顶点色，透明河流逐像素流纹属于前向主通道。
    );
#endif

#ifdef MOTION_VECTOR_PREPASS
    let previous_world_from_local = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    let previous_global_time = select(
        globals.time - globals.delta_time,
        globals.time + 3600.0 - globals.delta_time,
        globals.time < globals.delta_time,
    ); // Bevy全局时间每小时回绕；上一帧时间必须跨回绕仍保持连续。
    let previous_water_sample = sample_dynamic_water(
        true,
        previous_global_time, // 运动向量使用共享上一帧时间，不依赖CPU逐材质更新时间。
        vertex.position,
        rest_local_normal,
        rest_local_tangent,
        vertex.uv,
        rest_uv_b,
        rest_vertex_alpha,
    );
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        previous_world_from_local,
        vec4<f32>(previous_water_sample.local_position, 1.0),
    );
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
