// Geometry Clipmap 的世界坐标稀疏高度页采样；所有渲染 pass 引用同一正文。



@group(constants::MATERIAL_BIND_GROUP) @binding(121)
var clipmap_heights: texture_2d_array<f32>; // 每个驻留层覆盖 256×256 作者单元和 257×257 顶点。

@group(constants::MATERIAL_BIND_GROUP) @binding(122)
var clipmap_height_sampler: sampler; // 独立页内进行线性高度插值。

@group(constants::MATERIAL_BIND_GROUP) @binding(123)
var clipmap_page_table: texture_2d<u32>; // RGBA32_UINT：layer+1、minY bits、spanY bits、保留。

struct ClipmapHoles { rects: array<vec4<f32>, 32>, info: vec4<f32> }
@group(constants::MATERIAL_BIND_GROUP) @binding(124)
var<uniform> clipmap_holes: ClipmapHoles;

struct ClipmapProxyAxes { x: array<vec4<f32>, 129>, z: array<vec4<f32>, 129>, info: vec4<f32> }
@group(constants::MATERIAL_BIND_GROUP) @binding(125)
var<uniform> clipmap_proxy_axes: ClipmapProxyAxes;

fn proxy_axis(index:u32, axis:u32) -> f32 {
    if axis==0u { return clipmap_proxy_axes.x[index/4u][index%4u]; }
    return clipmap_proxy_axes.z[index/4u][index%4u];
}

fn proxy_cell(value:f32, cells:u32, axis:u32) -> u32 {
    var lo=0u; var hi=cells;
    // 513点最多十次二分；地图最外端归最后一个单元。
    for (var i=0u; i<10u; i+=1u) {
        if hi-lo<=1u { break; }
        let mid=(lo+hi)/2u;
        if value<proxy_axis(mid,axis) { hi=mid; } else { lo=mid; }
    }
    return min(lo,cells-1u);
}

fn inside_surface(world_xz: vec2<f32>) -> bool {
    if !inside_map(world_xz) { return false; }
    for (var i=0u; i<u32(clipmap_holes.info.x); i+=1u) {
        let hole=clipmap_holes.rects[i];
        if all(world_xz > hole.xy) && all(world_xz < hole.zw) { return false; }
    }
    return true;
}

const TILE_CELLS: f32 = 256.0; // 与稀疏作者块的逻辑单元边长一致。

// 高度、格吸附与遮罩使用源坐标；只有最终位置和法线进入父世界。
fn source_position(world: vec3<f32>) -> vec3<f32> {
    return (clipmap_params.world_to_source * vec4<f32>(world, 1.0)).xyz;
}
fn world_position(source: vec3<f32>) -> vec4<f32> {
    return clipmap_params.source_to_world * vec4<f32>(source, 1.0);
}
fn world_normal(source: vec3<f32>) -> vec3<f32> {
    return normalize((transpose(clipmap_params.world_to_source) * vec4<f32>(source, 0.0)).xyz); // 非均匀缩放必须使用逆转置法线。
}

fn inside_map(world_xz: vec2<f32>) -> bool {
    return all(world_xz >= clipmap_params.map_bounds.xy) &&
           all(world_xz <= clipmap_params.map_bounds.zw); // 地图外片元必须裁切。
}

// 代理 a-c-b / b-c-d 与 CPU Mesh 的固定顺序一致，最后一个短格不能按整 stride 拉伸。
fn raytracing_triangle_index(world_xz: vec2<f32>) -> u32 {
    let stride = max(clipmap_params.reserved.x, 1.0);
    let cells = max(vec2<u32>(clipmap_params.reserved.yz), vec2<u32>(1u));
    if clipmap_proxy_axes.info.x>0.5 {
        let cell=vec2<u32>(proxy_cell(world_xz.x,cells.x,0u),proxy_cell(world_xz.y,cells.y,1u));
        let lower=vec2<f32>(proxy_axis(cell.x,0u),proxy_axis(cell.y,1u));
        let upper=vec2<f32>(proxy_axis(cell.x+1u,0u),proxy_axis(cell.y+1u,1u));
        let fraction=(world_xz-lower)/max(upper-lower,vec2<f32>(1.0e-6));
        return 2u*(cell.y*cells.x+cell.x)+u32(fraction.x+fraction.y>1.0);
    }
    let logical = clamp((world_xz - clipmap_params.map_bounds.xy) / clipmap_params.grid.xy,
                        vec2<f32>(0.0), clipmap_params.grid.zw);
    let cell = min(vec2<u32>(floor(logical / stride)), cells - vec2<u32>(1u));
    let lower = vec2<f32>(cell) * stride;
    let upper = min(vec2<f32>(cell + vec2<u32>(1u)) * stride, clipmap_params.grid.zw);
    let fraction = (logical - lower) / max(upper - lower, vec2<f32>(1.0e-6));
    return 2u * (cell.y * cells.x + cell.x) + u32(fraction.x + fraction.y > 1.0);
}

fn raytracing_proxy_enabled() -> bool {
    return clipmap_params.reserved.w > 0.5;
}

fn raytracing_instance_index() -> u32 {
    return min(u32(clipmap_proxy_axes.info.w),15u); // 实际绑定顺序由整批GPU证明发布。
}

fn geometry_lod(vertex_lod: vec2<f32>) -> f32 {
    return max(vertex_lod.x, 0.0); // CPU已为普通顶点和接缝细边直接写入实际几何Mip指数。
}

fn requested_mip(vertex_lod: vec2<f32>) -> f32 {
    return min(geometry_lod(vertex_lod), clipmap_params.options.x); // 缺少更粗持久父页时复用最高可用页。
}

fn stable_world_xz(flat_world_xz: vec2<f32>, fine_center: vec2<f32>, vertex_lod: vec2<f32>) -> vec2<f32> {
    let step = clipmap_params.grid.xy * exp2(geometry_lod(vertex_lod)); // 每层必须按自己的几何格距吸附。
    // 所有网格沿用真实世界观察点；地图外中心不能被推回各对象边缘而产生假的密集区。
    let level_center = clipmap_params.map_bounds.xy +
        floor((fine_center - clipmap_params.map_bounds.xy) / step + vec2<f32>(0.5)) * step;
    return stitch_position(flat_world_xz + level_center - fine_center,clipmap_params.map_bounds);
}

fn table_row_offset(level: u32) -> u32 {
    var offset = 0u; // 各层按实际 Z 页数纵向紧凑排列，不上传九份 256² 空表。
    var current = 0u;
    loop {
        if current >= level {
            break;
        }
        let scale = exp2(f32(current));
        offset += u32(ceil(clipmap_params.grid.w / (TILE_CELLS * scale)));
        current += 1u;
    }
    return offset;
}

fn base_height_at(world_xz: vec2<f32>, vertex_lod: vec2<f32>) -> f32 {
    if clipmap_params.options.y < 0.5 || !inside_map(world_xz) {
        return 0.0; // 缺页或地图外先使用确定性 Y=0 托底。
    }
    let base_logical = clamp((world_xz - clipmap_params.map_bounds.xy) / clipmap_params.grid.xy,
                             vec2<f32>(0.0), clipmap_params.grid.zw); // 世界 X/Z 转基础作者格坐标。
    let level = u32(requested_mip(vertex_lod)); // 环带层直接选择同级持久父页，不依赖 257 奇数纹理的硬件 Mip。
    let scale = exp2(f32(level));
    let logical = base_logical / scale; // 当前父页层的全图格点坐标。
    let tile_count = vec2<u32>(ceil(clipmap_params.grid.zw / (TILE_CELLS * scale)));
    let table_size = textureDimensions(clipmap_page_table); // x 为基础页列数，y 为所有层紧凑行数。
    let row_offset = table_row_offset(level);
    if table_size.x < tile_count.x || table_size.y < row_offset + tile_count.y {
        return 0.0; // 半上传或旧图页表不得采样到错误地图。
    }
    let tile = min(vec2<u32>(floor(logical / TILE_CELLS)), tile_count - vec2<u32>(1u)); // 地图最远端点归最后一页。
    let entry = textureLoad(clipmap_page_table,
                            vec2<i32>(i32(tile.x), i32(row_offset + tile.y)), 0); // 一次整数 texel 读取当前层驻留页。
    if entry.x == 0u {
        return 0.0; // 未驻留/未编辑作者块不读取高度数组。
    }
    let local = logical - vec2<f32>(tile) * TILE_CELLS; // 当前父页内的 0..256 顶点坐标。
    let page_size = vec2<f32>(textureDimensions(clipmap_heights, 0)); // 一般为 257×257。
    let uv = (local * ((page_size - vec2<f32>(1.0)) / TILE_CELLS) + vec2<f32>(0.5)) / page_size; // 顶点准确落在 texel 中心。
    let raw = textureSampleLevel(clipmap_heights, clipmap_height_sampler, uv,
                                 i32(entry.x - 1u), 0.0).r; // 每层始终是独立 257² 页，避免跨页硬件 Mip 接缝。
    return bitcast<f32>(entry.y) + raw * bitcast<f32>(entry.z); // 每页独立 min/span 保持 R16 精度。
}

fn height_at(world_xz: vec2<f32>, vertex_lod: vec2<f32>) -> f32 {
    return stitch_height(world_xz,base_height_at(world_xz,vertex_lod),clipmap_params.map_bounds);
}

fn normal_at(world_xz: vec2<f32>, vertex_lod: vec2<f32>) -> vec3<f32> {
    if clipmap_params.options.y < 0.5 && !stitches_active() {
        return vec3<f32>(0.0, 1.0, 0.0); // 空白地图无需采样四个邻点。
    }
    let step = max(clipmap_params.grid.xy * exp2(requested_mip(vertex_lod)), vec2<f32>(0.0001)); // 当前级的米制差分间隔。
    let left = height_at(clamp(world_xz - vec2<f32>(step.x, 0.0), clipmap_params.map_bounds.xy,
                               clipmap_params.map_bounds.zw), vertex_lod); // 边界使用单侧差分。
    let right = height_at(clamp(world_xz + vec2<f32>(step.x, 0.0), clipmap_params.map_bounds.xy,
                                clipmap_params.map_bounds.zw), vertex_lod); // 与左侧同一页规则。
    let down = height_at(clamp(world_xz - vec2<f32>(0.0, step.y), clipmap_params.map_bounds.xy,
                               clipmap_params.map_bounds.zw), vertex_lod); // 世界 Z 负向。
    let up = height_at(clamp(world_xz + vec2<f32>(0.0, step.y), clipmap_params.map_bounds.xy,
                             clipmap_params.map_bounds.zw), vertex_lod); // 世界 Z 正向。
    return stitch_normal(world_xz,normalize(vec3<f32>(left - right, 2.0 * step.x, 0.0) +
                     vec3<f32>(0.0, 0.0, down - up) * (step.x / step.y)),clipmap_params.map_bounds);
}
