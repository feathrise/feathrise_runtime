#import bevy_pbr::{
    mesh_functions,
    mesh_view_bindings::view,
    prepass_io::Vertex,
    view_transformations::position_world_to_clip,
}
#import feathrise_vegetation::mesh_billboard::{
    vegetation_billboard_world_from_local,
    vegetation_billboard_normal_local_to_world,
}

// 与Bevy prepass_io位置完全一致，深度、法线、延迟与主通道共享同一GPU朝向。
struct VegetationBillboardPrepassVertexOutput {
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

// 预通道与主画面使用相同视图位置；历史位置保留上一ECS矩阵并采用当前朝向以避免CPU相机历史上传。
@vertex
fn vertex(vertex: Vertex) -> VegetationBillboardPrepassVertexOutput {
    var out: VegetationBillboardPrepassVertexOutput; // 初始化与Bevy预通道兼容的顶点输出。
    let source_world_from_local =
        mesh_functions::get_world_from_local(vertex.instance_index); // 读取当前ECS作者世界矩阵。
    let billboard_world_from_local = vegetation_billboard_world_from_local(
        source_world_from_local, // 保留当前节点锚点和逐轴缩放。
        view.lod_view_world_position, // 与主PBR及阴影视图共享同一LOD观察位置。
    ); // 完成当前预通道视图GPU朝向。

    out.world_position = mesh_functions::mesh_position_local_to_world(
        billboard_world_from_local, // 深度与法线预通道必须使用定向后的矩阵。
        vec4<f32>(vertex.position, 1.0), // CPU Mesh顶点保持只读。
    ); // 完成当前世界顶点位置。
    out.position = position_world_to_clip(out.world_position.xyz); // 使用当前预通道视图投影。
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z; // 保存正交仿真裁切前深度。
    out.position.z = min(out.position.z, 1.0); // 与Bevy标准预通道保持相同裁切口径。
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv; // AlphaMask预通道继续读取原第一套UV。
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b; // 第二套UV保持原样。
#endif

#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    out.world_normal = vegetation_billboard_normal_local_to_world(
        billboard_world_from_local, // 法线预通道必须跟随GPU定向后的矩阵。
        vertex.normal, // 保留导入局部法线。
    ); // 完成当前世界法线。
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        billboard_world_from_local, // 延迟法线贴图切线与主PBR保持一致。
        vertex.tangent, // 保留导入MikkTSpace局部切线。
        vertex.instance_index, // 原实例标志继续提供镜像符号。
    ); // 完成当前世界切线。
#endif
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color; // 顶点色在预通道中原样传递。
#endif

#ifdef MOTION_VECTOR_PREPASS
    let previous_source_world_from_local =
        mesh_functions::get_previous_world_from_local(vertex.instance_index); // 读取Bevy保存的上一帧ECS矩阵。
    let previous_billboard_world_from_local = vegetation_billboard_world_from_local(
        previous_source_world_from_local, // 保留根节点自身移动的历史位置。
        view.lod_view_world_position, // 使用当前视图朝向近似，避免新增逐视图CPU历史上传与同步。
    ); // 完成上一ECS矩阵的稳定公告板重建。
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        previous_billboard_world_from_local, // 运动向量仍能表达实例根移动和相机矩阵变化。
        vec4<f32>(vertex.position, 1.0), // 使用同一只读局部顶点。
    ); // 完成上一世界顶点位置。
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index; // 延迟片元继续读取同一实例数据。
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, // 保持当前实例的宽域LOD身份。
        source_world_from_local[3], // 公告板朝向不改变锚点距离。
    ); // 完成LOD交叉抖动级别读取。
#endif

    return out; // 返回与Bevy标准预通道片元阶段兼容的数据。
}
