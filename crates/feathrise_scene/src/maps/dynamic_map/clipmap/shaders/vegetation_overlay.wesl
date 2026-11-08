// F12 植被覆盖直接画在复用正式高度位移后的地形三角面上。

import bevy_pbr::render::forward_io::{Vertex, decompress_vertex};
import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::forward_io::FragmentOutput;
import bevy_pbr::render::mesh_functions;
import bevy_pbr::render::view_transformations::position_world_to_clip;
import feathrise_scene::maps::dynamic_map::clipmap::shaders::height as clipmap_height;

struct OverlayParams {
    brush: vec4<f32>, // xy 世界中心、z 外圈半径、w 强度比例。
    color: vec4<f32>, // 线性刷圈颜色。
    options: vec4<f32>, // x 是否显示密度，y 权威最高高度Mip。
    missing_pages: array<vec4<u32>, 16>, // 每级至多四个未就绪页编码。
}
@group(constants::MATERIAL_BIND_GROUP) @binding(126) var<uniform> overlay: OverlayParams; // 与120..125高度绑定分开。
@group(constants::MATERIAL_BIND_GROUP) @binding(127) var density: texture_2d<f32>; // 已有颜色密度覆盖。
@group(constants::MATERIAL_BIND_GROUP) @binding(128) var density_sampler: sampler; // 保留平滑预览。

fn known_height(world: vec2<f32>, lod: vec2<f32>) -> f32 {
    if !clipmap_height::inside_map(world) { return 1.0; } // 地图外顶点与正式地形一样为零，片元稍后裁切。
    let bounds = clipmap_height::clipmap_params.map_bounds; // 只读vec4，避免Naga把uniform结构的Offset装饰带入Function存储类。
    let grid = clipmap_height::clipmap_params.grid; // 与正式高度同一映射，不复制完整uniform结构。
    let level = u32(min(clipmap_height::geometry_lod(lod), overlay.options.y)); // 缺页使用权威规格；旧空白图集最高Mip=0不能掩盖新父页等待。
    let scale = exp2(f32(level)); // 高度页 Mip。
    let logical = clamp((world - bounds.xy) / grid.xy, vec2<f32>(0.0), grid.zw) / scale; // 世界格点。
    let counts = vec2<u32>(ceil(grid.zw / (256.0 * scale))); // 非方形地图页范围。
    let tile = min(vec2<u32>(floor(logical / 256.0)), counts - vec2<u32>(1u)); // 最远边归末页。
    let code = (tile.x | (tile.y << 16u)) + 1u; // 与草地和 CPU 查询相同缺页协议。
    return select(1.0, 0.0, any(overlay.missing_pages[level] == vec4<u32>(code))); // 隐式平页不在缺页集合。
}

@vertex
fn vertex(vertex_in: Vertex) -> VertexOutput {
    let input = decompress_vertex(vertex_in, vertex_in.instance_index);
    let transform = mesh_functions::get_world_from_local(input.instance_index); // 与地形完全相同的环带变换。
    let flat = mesh_functions::mesh_position_local_to_world(transform, vec4<f32>(input.position, 1.0)); // 正式局部网格。
    @if(VERTEX_UVS_B) let lod = input.uv_b; // 接缝采样层与几何层。
    @if(!VERTEX_UVS_B) let lod = vec2<f32>(1.0, 0.0); // 管线安全占位。
    let source = clipmap_height::stable_world_xz(clipmap_height::source_position(flat.xyz).xz,
        clipmap_height::source_position(transform[3].xyz).xz, lod);
    let world_position = clipmap_height::world_position(vec3<f32>(source.x, clipmap_height::height_at(source, lod), source.y));
    var out: VertexOutput;
    out.position = position_world_to_clip(world_position.xyz);
    out.world_position = world_position;
    out.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    @if(VERTEX_UVS_A) out.uv = input.uv;
    @if(VERTEX_UVS_B) out.uv_b = vec2<f32>(known_height(source, lod), 0.0);
    @if(VERTEX_TANGENTS) out.world_tangent = vec4<f32>(0.0);
    @if(VERTEX_COLORS) out.color = input.color;
    @if(VERTEX_OUTPUT_INSTANCE_INDEX) out.instance_index = input.instance_index;
    @if(VISIBILITY_RANGE_DITHER) out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(input.instance_index, transform[3]);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    @if(VERTEX_UVS_B) if in.uv_b.x < 0.99999 { discard; } // 未知页影响的三角面不显示错误平面刷印。
    let world = in.world_position.xz;
    let source = clipmap_height::source_position(in.world_position.xyz).xz;
    if !clipmap_height::inside_surface(source) { discard; }
    var color = vec4<f32>(0.0); // 无印无圈时完全透明。
    if overlay.options.x > 0.5 {
        let bounds = clipmap_height::clipmap_params.map_bounds; // 复用正式地形世界映射。
        let uv = (source - bounds.xy) / (bounds.zw - bounds.xy); // 每个对象按自己的作者轴采样。
        color = textureSample(density, density_sampler, uv); // 预览图不修改作者密度。
    }
    let offset = world - overlay.brush.xy; // 刷圈与涂抹共用水平距离。
    let distance = length(offset); // 曲面只改变高度，不改变作者半径语义。
    let width = max(fwidth(distance) * 1.5, 0.015); // 三像素左右线宽，近处有最小米制宽度。
    let outer = abs(distance - overlay.brush.z); // 外圈表示真实覆盖半径。
    let inner = abs(distance - overlay.brush.z * overlay.brush.w); // 强度越大内圈越大。
    let cross_size = clamp(overlay.brush.z * 0.12, 0.3, 8.0); // 与旧笔刷相同中心标记尺寸。
    let cross_distance = min(max(abs(offset.x), abs(offset.y) - cross_size), max(abs(offset.y), abs(offset.x) - cross_size)); // 两条有限长轴线的距离。
    let line = 1.0 - smoothstep(width * 0.5, width, min(min(outer, inner), cross_distance)); // 抗锯齿曲面刷圈。
    if overlay.brush.z > 0.0 {
        let alpha = line * overlay.color.a; // 只在有效拾取时覆盖刷圈。
        let combined = alpha + color.a * (1.0 - alpha); // 正确非预乘透明合成。
        color = vec4<f32>((overlay.color.rgb * alpha + color.rgb * color.a * (1.0 - alpha)) / max(combined, 0.00001), combined); // 保留密度背景。
    }
    return FragmentOutput(color); // 直接返回避免同类Function结构Offset装饰；透明覆盖只存在于F12。
}
