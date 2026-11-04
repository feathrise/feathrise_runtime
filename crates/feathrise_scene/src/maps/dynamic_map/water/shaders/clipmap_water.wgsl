// Geometry Clipmap 静水共享正式高度绑定，并以单层 R8/水位裁切同一环带。
#define_import_path feathrise_scene::clipmap_water
#import feathrise_scene::clipmap_height

struct ClipmapWaterParams {
    surface: vec4<f32>, // x静止水位，y恢复完整波幅的安全水深，z权威最高高度Mip，w三维水面标记。
    missing_pages: array<vec4<u32>, 16>, // 各Mip最多四个当前非平缺页。
}

@group(#{MATERIAL_BIND_GROUP}) @binding(124)
var<uniform> clipmap_water_params: ClipmapWaterParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(125)
var clipmap_water_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(126)
var clipmap_water_mask_sampler: sampler;
struct WaterTerrainHoles {rects:array<vec4<f32>,32>,info:vec4<f32>}
@group(#{MATERIAL_BIND_GROUP}) @binding(129) var<uniform> water_terrain_holes:WaterTerrainHoles;

fn known_height(world: vec2<f32>, lod: vec2<f32>) -> f32 {
    if !clipmap_height::inside_map(world) {
        return 1.0; // 地图外由统一边界裁切；这里不把它误报成分页等待。
    }
    let bounds = clipmap_height::clipmap_params.map_bounds;
    let grid = clipmap_height::clipmap_params.grid;
    let level = u32(min(clipmap_height::geometry_lod(lod), clipmap_water_params.surface.z));
    let scale = exp2(f32(level));
    let logical = clamp((world - bounds.xy) / grid.xy, vec2<f32>(0.0), grid.zw) / scale;
    let counts = vec2<u32>(ceil(grid.zw / (256.0 * scale)));
    let tile = min(vec2<u32>(floor(logical / 256.0)), counts - vec2<u32>(1u));
    let code = (tile.x | (tile.y << 16u)) + 1u;
    return select(
        1.0,
        0.0,
        any(clipmap_water_params.missing_pages[level] == vec4<u32>(code)),
    ); // 合法隐式平页不在缺页表中，继续按Y=0参与水深判断。
}

fn map_uv(world: vec2<f32>) -> vec2<f32> {
    let bounds = clipmap_height::clipmap_params.map_bounds;
    return (world - bounds.xy) / (bounds.zw - bounds.xy);
}

// 作者R8把首、末纹素中心分别放在地图两端；普通textureSample则把纹素中心放在
// (i + 0.5) / size。这里显式换算，保证GPU岸线与CPU互动、点选和笔刷使用同一网格。
fn endpoint_aligned_mask_uv(map_uv_value: vec2<f32>) -> vec2<f32> {
    let dimensions = vec2<f32>(textureDimensions(clipmap_water_mask));
    let last = max(dimensions - vec2<f32>(1.0), vec2<f32>(1.0));
    return (clamp(map_uv_value, vec2<f32>(0.0), vec2<f32>(1.0)) * last + vec2<f32>(0.5))
        / dimensions;
}

fn water_level() -> f32 {
    return clipmap_water_params.surface.x;
}

// 安全水深只限制波幅，不能裁掉整段浅水岸带；水陆交线仍由零水深和真实地形确定。
// 线性上界保证浅水处的最大负波位移随可用水深缩小，不抬水、不改河床或作者水位。
fn shore_wave_fade(terrain_height: f32, known: f32) -> f32 {
    if known < 0.99999 {
        return 0.0;
    }
    let depth = water_level() - terrain_height;
    return clamp(depth / max(clipmap_water_params.surface.y, 0.0001), 0.0, 1.0);
}

// map_uv_value由未受波浪水平位移影响的环带顶点插值，避免岸线随波逐帧游动。
// uv_b.x是实际可见三角面内插后的地形Y，uv_b.y是三个顶点的缺页有效性。
// 导数只在统一片元入口调用，用水深与R8强度共同生成稳定抗锯齿岸缘。
fn surface_coverage(map_uv_value: vec2<f32>, uv_b: vec2<f32>) -> f32 {
    if clipmap_water_params.surface.w<0.5 {
        let bounds=clipmap_height::clipmap_params.map_bounds;
        let world=mix(bounds.xy,bounds.zw,map_uv_value);
        for(var i=0u;i<min(u32(water_terrain_holes.info.x),32u);i+=1u){
            let h=water_terrain_holes.rects[i];if all(world>h.xy)&&all(world<h.zw){return 0.0;}
        }
    }
    if any(map_uv_value < vec2<f32>(0.0))
        || any(map_uv_value > vec2<f32>(1.0))
        || uv_b.y < 0.99999
    {
        return 0.0;
    }
    let mask = textureSample(
        clipmap_water_mask,
        clipmap_water_mask_sampler,
        endpoint_aligned_mask_uv(map_uv_value),
    ).r;
    let mask_width = max(fwidth(mask) * 1.5, 1.0 / 255.0);
    let mask_coverage = smoothstep(0.0, mask_width, mask) * mask;
    let depth = clipmap_water_params.surface.x - uv_b.x;
    let depth_width = max(fwidth(depth) * 1.5, 0.0001); // 只保留数值下限，固定厘米岸带会在缓坡放大成米级淡出。
    // 从真实水陆交线向水中做像素抗锯齿，不能把至少0.20m的深水安全区当作可见岸线。
    let depth_coverage = smoothstep(0.0, depth_width, depth);
    return clamp(mask_coverage * depth_coverage, 0.0, 1.0);
}
