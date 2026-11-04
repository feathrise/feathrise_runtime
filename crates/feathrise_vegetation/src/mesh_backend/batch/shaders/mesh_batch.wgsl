// 通用分块 Mesh 批次顶点入口。
// 【功能设计】逐株矩阵和 LOD 宽域来自实例流，材质、光照、AlphaMask、Deferred 与阴影仍由原 Bevy PBR 管线拥有。

#import bevy_pbr::mesh_functions
#import bevy_pbr::view_transformations::position_world_to_clip
#import bevy_pbr::mesh_functions::get_world_from_local
#import bevy_pbr::mesh_view_bindings::view

#ifdef VEGETATION_BATCH_WIND
#import feathrise_environment::mesh_wind::wind::{
    tree_wind_uniform,
    displace_tree_wind_local,
}
#ifdef VEGETATION_BATCH_PREPASS
#import bevy_render::globals::Globals
@group(0) @binding(1) var<uniform> vegetation_batch_globals: Globals;
#else
#import bevy_pbr::mesh_view_bindings::globals
#endif
#endif

#ifdef VEGETATION_BATCH_BILLBOARD
#import feathrise_vegetation::mesh_billboard::{
    vegetation_billboard_world_from_local,
    vegetation_billboard_normal_local_to_world,
}
#endif

struct VegetationBatchVertex {
    @builtin(vertex_index) vertex_index: u32,
    @location(0) position: vec3<f32>,
#ifdef VEGETATION_BATCH_PREPASS
#ifdef VERTEX_UVS_A
    @location(1) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(2) uv_b: vec2<f32>,
#endif
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    @location(3) normal: vec3<f32>,
#endif
#ifdef VERTEX_TANGENTS
    @location(4) tangent: vec4<f32>,
#endif
#endif
#ifdef VERTEX_COLORS
    @location(7) color: vec4<f32>,
#endif
#else
#ifdef VERTEX_NORMALS
    @location(1) normal: vec3<f32>,
#endif
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_TANGENTS
    @location(4) tangent: vec4<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#endif
    @location(8) instance_x: vec4<f32>,
    @location(9) instance_y: vec4<f32>,
    @location(10) instance_z: vec4<f32>,
    @location(11) instance_w: vec4<f32>,
    @location(12) lod_range: vec4<f32>,
    @location(13) metadata: vec4<u32>,
}

// 三维作者格直接作为候选输入，真正的逐株变换由GPU计算。
fn volume_cell_seed(vertex:VegetationBatchVertex)->u32 {
    var h=u32(vertex.instance_w.w)^vertex.metadata.w; // 稳定作者格身份；实际根随显示LOD投射时不会重新抽签。
    h=(h^(h>>16u))*2246822519u;h=(h^(h>>13u))*3266489917u;return h^(h>>16u);
}
fn volume_cell_selected(vertex:VegetationBatchVertex)->bool {
    let step=max(vertex.instance_z.x,0.01);let cell=max(vertex.instance_y.w,0.01);
    let probability=clamp(vertex.instance_x.w*cell*cell/(step*step),0.0,1.0);
    return f32(volume_cell_seed(vertex))/4294967295.0<probability;
}

fn vegetation_batch_source_world_from_local(vertex: VegetationBatchVertex) -> mat4x4<f32> {
    if (vertex.metadata.z & 0x80000000u)!=0u {
        let n=normalize(vertex.instance_y.xyz);
        let axis=select(vec3<f32>(0,1,0),vec3<f32>(1,0,0),abs(n.y)>0.95);
        let tangent=normalize(cross(axis,n));let bitangent=cross(tangent,n);
        let h=volume_cell_seed(vertex);let angle=f32(h&65535u)/65535.0*6.2831853;
        let scale=mix(vertex.instance_z.y,vertex.instance_z.z,f32(h>>16u)/65535.0);
        let x=(tangent*cos(angle)+bitangent*sin(angle))*scale;
        let z=(bitangent*cos(angle)-tangent*sin(angle))*scale;
        return get_world_from_local(vertex.metadata.x) * mat4x4<f32>(vec4<f32>(x,0),vec4<f32>(n*scale,0),vec4<f32>(z,0),vec4<f32>(vertex.instance_x.xyz-n*vertex.instance_z.w,1))*get_world_from_local(vertex.metadata.z & 0x7fffffffu);
    }
    return get_world_from_local(vertex.metadata.x) * mat4x4<f32>(vertex.instance_x, vertex.instance_y, vertex.instance_z, vertex.instance_w);
}

fn vegetation_batch_world_from_local(
    vertex: VegetationBatchVertex,
    source_world_from_local: mat4x4<f32>,
) -> mat4x4<f32> {
#ifdef VEGETATION_BATCH_BILLBOARD
    return vegetation_billboard_world_from_local(
        source_world_from_local,
        view.lod_view_world_position,
    );
#else
    return source_world_from_local;
#endif
}

fn vegetation_batch_normal_local_to_world(
    world_from_local: mat4x4<f32>,
    local_normal: vec3<f32>,
) -> vec3<f32> {
#ifdef VEGETATION_BATCH_BILLBOARD
    return vegetation_billboard_normal_local_to_world(world_from_local, local_normal);
#else
    if all(local_normal == vec3<f32>(0.0)) {
        return local_normal;
    }
    let axis_x = world_from_local[0].xyz;
    let axis_y = world_from_local[1].xyz;
    let axis_z = world_from_local[2].xyz;
    let transformed =
        axis_x * (local_normal.x / max(dot(axis_x, axis_x), 0.000001)) +
        axis_y * (local_normal.y / max(dot(axis_y, axis_y), 0.000001)) +
        axis_z * (local_normal.z / max(dot(axis_z, axis_z), 0.000001));
    return normalize(transformed);
#endif
}

fn vegetation_batch_tangent_local_to_world(
    world_from_local: mat4x4<f32>,
    local_tangent: vec4<f32>,
    determinant_positive: bool,
) -> vec4<f32> {
    let tangent = normalize(
        world_from_local[0].xyz * local_tangent.x +
        world_from_local[1].xyz * local_tangent.y +
        world_from_local[2].xyz * local_tangent.z,
    );
    return vec4<f32>(tangent, local_tangent.w * select(-1.0, 1.0, determinant_positive));
}

#ifdef VEGETATION_BATCH_WIND
fn vegetation_batch_current_time() -> f32 {
#ifdef VEGETATION_BATCH_PREPASS
    return vegetation_batch_globals.time;
#else
    return globals.time;
#endif
}
#endif

fn vegetation_batch_local_position(
    local_position: vec3<f32>,
    world_from_local: mat4x4<f32>,
    previous: bool,
) -> vec3<f32> {
#ifdef VEGETATION_BATCH_WIND
    #ifdef VEGETATION_BATCH_PREPASS
    if previous {
        return displace_tree_wind_local(
            tree_wind_uniform.previous,
            local_position,
            world_from_local,
            vegetation_batch_current_time() - vegetation_batch_globals.delta_time,
            tree_wind_uniform.aabb_min.y,
            tree_wind_uniform.aabb_max.y,
        );
    }
    #endif
    return displace_tree_wind_local(
        tree_wind_uniform.current,
        local_position,
        world_from_local,
        vegetation_batch_current_time(),
        tree_wind_uniform.aabb_min.y,
        tree_wind_uniform.aabb_max.y,
    );
#else
    return local_position;
#endif
}

fn vegetation_batch_dither_level(lod_range: vec4<f32>, world_origin: vec3<f32>) -> i32 {
    let camera_distance = length(view.lod_view_world_position - world_origin);
    if camera_distance < lod_range.z {
        let width = lod_range.y - lod_range.x;
        if width <= 0.000001 {
            return select(-16, 0, camera_distance >= lod_range.y);
        }
        let level = i32(round((camera_distance - lod_range.x) / width * 16.0));
        return -16 + clamp(level, 0, 16);
    }
    let width = lod_range.w - lod_range.z;
    if width <= 0.000001 {
        return select(0, 16, camera_distance >= lod_range.w);
    }
    let level = i32(round((camera_distance - lod_range.z) / width * 16.0));
    return clamp(level, 0, 16);
}

#ifdef VEGETATION_BATCH_PREPASS
struct VegetationBatchVertexOutput {
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

@vertex
fn vertex(vertex: VegetationBatchVertex) -> VegetationBatchVertexOutput {
    var out: VegetationBatchVertexOutput;
    let source_world_from_local = vegetation_batch_source_world_from_local(vertex);
    let world_from_local = vegetation_batch_world_from_local(vertex, source_world_from_local);
    let local_position = vegetation_batch_local_position(
        vertex.position,
        source_world_from_local,
        false,
    );
    out.world_position = world_from_local * vec4<f32>(local_position, 1.0);
    out.position = position_world_to_clip(out.world_position.xyz);
    if (vertex.metadata.z & 0x80000000u)!=0u && !volume_cell_selected(vertex){out.position=vec4<f32>(2,2,2,1);}
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
#ifdef VERTEX_NORMALS
    out.world_normal = vegetation_batch_normal_local_to_world(world_from_local, vertex.normal);
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = vegetation_batch_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.metadata.y != 0u,
    );
#endif
#endif
#ifdef MOTION_VECTOR_PREPASS
    let previous_local_position = vegetation_batch_local_position(
        vertex.position,
        source_world_from_local,
        true,
    );
    out.previous_world_position = world_from_local * vec4<f32>(previous_local_position, 1.0);
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.metadata.x;
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = vegetation_batch_dither_level(
        vertex.lod_range,
        source_world_from_local[3].xyz,
    );
#endif
    return out;
}
#else
struct VegetationBatchVertexOutput {
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

@vertex
fn vertex(vertex: VegetationBatchVertex) -> VegetationBatchVertexOutput {
    var out: VegetationBatchVertexOutput;
    let source_world_from_local = vegetation_batch_source_world_from_local(vertex);
    let world_from_local = vegetation_batch_world_from_local(vertex, source_world_from_local);
    let local_position = vegetation_batch_local_position(
        vertex.position,
        source_world_from_local,
        false,
    );
    out.world_position = world_from_local * vec4<f32>(local_position, 1.0);
    out.position = position_world_to_clip(out.world_position.xyz);
    if (vertex.metadata.z & 0x80000000u)!=0u && !volume_cell_selected(vertex){out.position=vec4<f32>(2,2,2,1);}
#ifdef VERTEX_NORMALS
    out.world_normal = vegetation_batch_normal_local_to_world(world_from_local, vertex.normal);
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = vegetation_batch_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.metadata.y != 0u,
    );
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.metadata.x;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = vegetation_batch_dither_level(
        vertex.lod_range,
        source_world_from_local[3].xyz,
    );
#endif
    return out;
}
#endif
