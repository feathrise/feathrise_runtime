//! 通用地形地表的PBR扩展片元着色器。
//! 【功能设计】HN-01保留固定八层兼容路径；动态地图使用每像素四槽索引与运行时长度存储表，
//! 地图引用层数不进入管线变体。所有平铺和权重定位都使用世界坐标，不依赖Blender UV。

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
}
#import bevy_pbr::mesh_view_bindings::view
#import feathrise_environment::surface_weather::surface_weather
#import feathrise_environment::surface_weather::rain_shelter_exposure
#import feathrise_environment::surface_rain::{sample_surface_rain_slope, sample_surface_wind_slope}
#import feathrise_environment::wet_surface::{evaluate_wet_surface, wet_surface_noise, water_reflection_roughness, WATER_SURFACE_REFLECTANCE}
#ifdef SPARSE_CLIPMAP
#import feathrise_scene::clipmap_height
#endif

#ifdef PREPASS_PIPELINE
#import bevy_pbr::{
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_deferred_functions::deferred_output,
    pbr_deferred_types::{pack_24bit_normal_and_flags, unpack_flags},
    utils::octahedral_encode,
}
#else
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
}
#endif

struct TerrainSurfaceLayer {
    base_color: vec4<f32>, // 线性RGBA乘色，纯参数层直接作为最终基础色。
    pbr_and_texture: vec4<f32>, // x粗糙度、y金属度、z平铺米数、w切片编号加一或零。
}

@group(#{MATERIAL_BIND_GROUP}) @binding(116) // 独立作者积水图不改变四槽地表权重。
var puddle_mask: texture_2d<f32>; // 线性R8潜力，未启用时不取样回退纹理。
@group(#{MATERIAL_BIND_GROUP}) @binding(117) // 与Rust积水绑定保持一致。
var puddle_sampler: sampler; // 使用显式梯度保证条件分支内合法采样。

struct TerrainSurfaceIndexedLayer {
    base_color: vec4<f32>, // 线性乘色。
    pbr_and_texture: vec4<f32>, // x粗糙度、y金属度、z平铺米数、w全局数组切片。
    normal_height_projection: vec4<f32>, // x法线强度、y高度混合、z投影编码、w翻转法线Y。
}

struct TerrainSurfaceParams {
    world_bounds: vec4<f32>, // xy/zw分别为权重图对应的世界X/Z最小与最大边界。
    counts: vec4<f32>, // x有效层数、y权重图数、z残余层编号、w草土层编号。
    appearance: vec4<f32>, // x草土微法线强度、y世界变化格米数、z为动态地形光追元数据开关、w索引模式。
    shading: vec4<f32>, // x大于0.5时使用片元导数得到逐三角形几何法线，否则保留作者平滑法线。
    volume_shading: vec4<f32>, // 三维投影与来源积水绑定分别保存，避免互相覆盖。
    source_to_world: mat4x4<f32>,
    world_to_source: mat4x4<f32>,
    layers: array<TerrainSurfaceLayer, 8>, // 两张RGBA权重图对应的固定八层参数表。
}

const DEFERRED_FLAGS_DYNAMIC_TERRAIN_BIT: u32 = 1u << 3u; // Bevy 0.19.1只占flags低三位，本项目以第四位标识动态地形三角元数据。
const DEFERRED_FLAGS_CLIPMAP_TERRAIN_BIT: u32 = 1u << 4u; // 固定代理与移动环带使用不同三角序号，独立标记 X/Z 映射。
const DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK: u32 = 0x7FFFFu; // 十九位完整覆盖固定524,288三角预算，并保持RGB9E5最高五位为零。

struct SurfaceVariation {
    value: f32, // 零至一的世界稳定宏观综合色变化。
    gradient: vec2<f32>, // 同一连续变化场的世界X/Z解析梯度。
}

struct BlendedSurface {
    base_color: vec4<f32>, // 所有有效地表层在线性空间混合后的基础色。
    pbr: vec3<f32>, // x环境遮蔽、y感知粗糙度、z金属度。
    grass_weight: f32, // 仅用于限制草土微法线的最终归一化权重。
}

struct IndexedSurfaceWeights {
    indices: vec4<u32>, // 纯函数四槽返回，不把接口数组布局引入动态通道临时变量。
    weights: vec4<f32>, // 手动双线性聚合后再归一化的权重。
}

struct IndexedLayerSample {
    base_color: vec4<f32>,
    pbr_height: vec4<f32>, // xyz=AO/粗糙度/金属度，w=高度。
    normal: vec3<f32>,
    height_strength: f32,
}

struct IndexedBlendedSurface {
    base_color: vec4<f32>,
    pbr: vec3<f32>,
    normal: vec3<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> terrain_surface: TerrainSurfaceParams;

@group(#{MATERIAL_BIND_GROUP}) @binding(101)
var weight_map_0: texture_2d<f32>;

@group(#{MATERIAL_BIND_GROUP}) @binding(102)
var weight_sampler_0: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(103)
var weight_map_1: texture_2d<f32>;

@group(#{MATERIAL_BIND_GROUP}) @binding(104)
var weight_sampler_1: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(105)
var base_color_array: texture_2d_array<f32>;

@group(#{MATERIAL_BIND_GROUP}) @binding(106)
var base_color_sampler: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(107)
var arm_array: texture_2d_array<f32>;

@group(#{MATERIAL_BIND_GROUP}) @binding(108)
var arm_sampler: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(109)
var normal_array: texture_2d_array<f32>;

@group(#{MATERIAL_BIND_GROUP}) @binding(110)
var normal_sampler: sampler;

@group(#{MATERIAL_BIND_GROUP}) @binding(111)
var index_map: texture_2d<u32>;

@group(#{MATERIAL_BIND_GROUP}) @binding(112)
var<storage, read> indexed_layers: array<TerrainSurfaceIndexedLayer>;
@group(#{MATERIAL_BIND_GROUP}) @binding(113)
var<storage, read> volume_layers: array<vec4<u32>>;

@group(#{MATERIAL_BIND_GROUP}) @binding(114)
var<storage, read> volume_fields: array<vec4<u32>>;
@group(#{MATERIAL_BIND_GROUP}) @binding(115)
var<uniform> volume_origin_step: vec4<f32>;
struct VolumePaint {keys:vec4<f32>, weights:vec4<f32>, puddles:f32, found:bool}
// 查询独立原格作者数据，远景简化、洞壁重网格均不改变笔触坐标。
fn volume_paint_at(world:vec3<f32>, normal:vec3<f32>)->VolumePaint {
    // Naga的结构类型带Offset装饰；查询累加只用标量/向量，返回时再直接构造结果。
    if volume_origin_step.w<=0.0 {return VolumePaint(vec4<f32>(0.0),vec4<f32>(0.0),0.0,false);}
    var result_keys=vec4<f32>(0.0);
    var result_weights=vec4<f32>(0.0);
    var result_puddles=0.0;
    var result_found=false;
    let point=world-volume_origin_step.xyz;
    let cell=vec3<i32>(floor(point/volume_origin_step.w));
    let header=volume_fields[0];
    var distance=3.0*volume_origin_step.w*volume_origin_step.w;
    for(var z=0;z<=1;z=z+1){for(var y=0;y<=1;y=y+1){for(var x=0;x<=1;x=x+1){
        let key=bitcast<vec3<u32>>(cell+vec3<i32>(x,y,z));
        let base=((key.x*73856093u)^(key.y*19349663u)^(key.z*83492791u)^header.y)&header.x;
        for(var probe=0u;probe<8u;probe=probe+1u){
            let offset=1u+((base+probe)&header.x)*5u;
            let stored=volume_fields[offset];
            if stored.w==0u {break;}
            if all(stored.xyz==key) {
                let p=bitcast<vec3<f32>>(volume_fields[offset+1u].xyz);
                let n=bitcast<vec3<f32>>(volume_fields[offset+2u].xyz);
                let delta=point-p;let d=dot(delta,delta);
                if d<distance && dot(n,normal)>0.25 {
                    distance=d;
                    result_keys=vec4<f32>(volume_fields[offset+3u]);
                    result_weights=bitcast<vec4<f32>>(volume_fields[offset+4u]);
                    result_puddles=bitcast<f32>(volume_fields[offset+2u].w);
                    result_found=true;
                }
                break;
            }
        }
    }}}
    return VolumePaint(result_keys,result_weights,result_puddles,result_found);
}

fn volume_layer_index(key: u32) -> u32 {
    if key == 0u { return 0xffffffffu; }
    let header = volume_layers[0];
    if key == 16777214u { return header.z; }
    if key == 16777215u { return header.w; }
    let base = ((key * 2654435761u) ^ header.y) & header.x;
    for (var probe = 0u; probe < 8u; probe = probe + 1u) {
        let value = volume_layers[1u + ((base + probe) & header.x)];
        if value.x == key { return value.y; }
        if value.x == 0u { break; }
    }
    return 0xffffffffu; // 已删除预设不能借用第一层，当前片元恢复自动/原地表。
}

fn volume_candidate_index(base: vec4<u32>, painted: vec4<u32>, index: u32) -> u32 {
    if index < 4u { return base[index]; }
    return painted[index - 4u];
}

fn volume_candidate_weight(base: vec4<f32>, painted: vec4<f32>, index: u32) -> f32 {
    if index < 4u { return base[index]; }
    return painted[index - 4u];
}

fn merge_volume_weights(base: IndexedSurfaceWeights, keys: vec4<f32>, painted: vec4<f32>) -> IndexedSurfaceWeights {
    // 保留原四槽与三维四槽全部八个候选；向量动态通道不把ArrayStride带进Function变量。
    let base_indices = vec4<u32>(base.indices[0], base.indices[1], base.indices[2], base.indices[3]);
    var painted_indices = vec4<u32>(0u);
    var painted_weights = vec4<f32>(0.0);
    var cover = 0.0;
    for (var i = 0u; i < 4u; i = i + 1u) {
        painted_indices[i] = volume_layer_index(u32(max(keys[i], 0.0) + 0.5));
        painted_weights[i] = select(0.0, max(painted[i], 0.0), keys[i] > 0.5 && painted_indices[i] != 0xffffffffu);
        cover = cover + painted_weights[i];
    }
    var base_weights = vec4<f32>(base.weights[0], base.weights[1], base.weights[2], base.weights[3])
        * (1.0 - clamp(cover, 0.0, 1.0));
    for (var i = 0u; i < 8u; i = i + 1u) {
        for (var j = i + 1u; j < 8u; j = j + 1u) {
            if volume_candidate_index(base_indices, painted_indices, i) == volume_candidate_index(base_indices, painted_indices, j) {
                let combined = volume_candidate_weight(base_weights, painted_weights, i)
                    + volume_candidate_weight(base_weights, painted_weights, j);
                if i < 4u { base_weights[i] = combined; } else { painted_weights[i - 4u] = combined; }
                if j < 4u { base_weights[j] = 0.0; } else { painted_weights[j - 4u] = 0.0; }
            }
        }
    }
    var result_indices = vec4<u32>(0u);
    var result_weights = vec4<f32>(0.0);
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        var best = 0u;
        // 严格大于保留原来的稳定顺序：同权重时原地表槽先于三维槽，槽内顺序不变。
        for (var i = 1u; i < 8u; i = i + 1u) {
            if volume_candidate_weight(base_weights, painted_weights, i) > volume_candidate_weight(base_weights, painted_weights, best) { best = i; }
        }
        result_indices[slot] = volume_candidate_index(base_indices, painted_indices, best);
        result_weights[slot] = volume_candidate_weight(base_weights, painted_weights, best);
        if best < 4u { base_weights[best] = 0.0; } else { painted_weights[best - 4u] = 0.0; }
    }
    return IndexedSurfaceWeights(result_indices, result_weights); // 最终仍只取贡献最高四层，三维与XZ共同使用原PBR采样预算。
}

/// 读取RGBA向量的动态通道；固定八次循环不会创建材质管线变体。
fn channel(value: vec4<f32>, index: u32) -> f32 {
    switch index {
        case 0u: { return value.r; }
        case 1u: { return value.g; }
        case 2u: { return value.b; }
        default: { return value.a; }
    }
}

// 两列向量按原八层顺序保存权重，避免函数返回/参数数组携带接口ArrayStride。
fn surface_weight(weights: mat2x4<f32>, index: u32) -> f32 {
    return weights[index / 4u][index % 4u];
}

fn normalize_surface_weights(source: mat2x4<f32>, layer_count: u32, residual_index: u32) -> mat2x4<f32> {
    var weights = source;
    weights[residual_index / 4u][residual_index % 4u] = 0.0; // 残余层永远由一减其他层重建，不接受纹理同槽污染。
    var explicit_sum = 0.0; // 统计全部非残余层权重。
    for (var index = 0u; index < layer_count; index = index + 1u) {
        explicit_sum = explicit_sum + surface_weight(weights, index); // 离线整数守恒通常使该值不超过一。
    }
    let residual = max(1.0 - explicit_sum, 0.0); // 过滤和浮点误差由统一归一化继续修正。
    weights[residual_index / 4u][residual_index % 4u] = residual;
    var total = explicit_sum + residual; // 得到当前片元全部有效权重和。
    if total <= 0.000001 {
        weights[residual_index / 4u][residual_index % 4u] = 1.0; // 防御损坏纹理产生全黑洞。
        total = 1.0; // 保证后续除法始终安全。
    }
    for (var index = 0u; index < layer_count; index = index + 1u) {
        weights[index / 4u][index % 4u] = surface_weight(weights, index) / total; // 采样后再归一化保证PBR参数严格凸组合。
    }
    return weights;
}

/// 用世界X/Z与显式梯度读取离线完整mip权重，并重建指定残余层。
fn sample_surface_weights(
    world_xz: vec2<f32>,
    world_dx: vec2<f32>,
    world_dy: vec2<f32>,
) -> mat2x4<f32> {
    var weights = mat2x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0)); // 未使用层和地图外区域先归零。
    let world_min = terrain_surface.world_bounds.xy; // 取得地图世界左下角。
    let world_max = terrain_surface.world_bounds.zw; // 取得地图世界右上角。
    let world_extent = max(world_max - world_min, vec2<f32>(0.0001)); // 防御配置损坏导致除零。
    let uv = (world_xz - world_min) / world_extent; // HN-01正X/正Z与离线权重图正U/正V一致。
    let residual_index = min(u32(terrain_surface.counts.z + 0.5), 7u); // 小整数浮点安全恢复为数组编号。
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        weights[residual_index / 4u][residual_index % 4u] = 1.0; // 地图覆盖外只显示默认杂土，不从边缘纹素外推。
        return weights; // 跳过所有正式纹理采样。
    }
    let sample_uv = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)); // 精确端点保持在合法采样范围。
    let uv_dx = world_dx / world_extent; // 将每片元世界梯度转换为权重图归一化梯度。
    let uv_dy = world_dy / world_extent; // 完整mip链据此稳定过滤远景权重。
    let first = textureSampleGrad(weight_map_0, weight_sampler_0, sample_uv, uv_dx, uv_dy); // HN-01唯一权重纹理采样。
    var second = vec4<f32>(0.0); // 单权重图地图不触碰内部零纹理采样。
    if terrain_surface.counts.y > 1.5 {
        second = textureSampleGrad(weight_map_1, weight_sampler_1, sample_uv, uv_dx, uv_dy); // 六至八层地图才读取第二张RGBA。
    }
    let layer_count = min(u32(terrain_surface.counts.x + 0.5), 8u); // 钳制有效层数避免损坏uniform越界。
    for (var index = 0u; index < layer_count; index = index + 1u) {
        if index < 4u {
            weights[0][index] = max(channel(first, index), 0.0); // 第一张RGBA映射层零至三。
        } else {
            weights[1][index - 4u] = max(channel(second, index - 4u), 0.0); // 第二张RGBA映射层四至七。
        }
    }
    return normalize_surface_weights(weights, layer_count, residual_index); // 固定八层仍只重建一次残余并按原层序归一化。
}

/// 采样单层的基础色与ARM；纯参数层不执行数组纹理访问。
fn sample_layer(
    layer: TerrainSurfaceLayer,
    world_xz: vec2<f32>,
    world_dx: vec2<f32>,
    world_dy: vec2<f32>,
) -> BlendedSurface {
    var base = layer.base_color; // 纯参数层直接使用uniform线性色。
    var ao = 1.0; // 没有ARM纹理时保持无遮挡。
    var roughness = clamp(layer.pbr_and_texture.x, 0.0, 1.0); // 读取层感知粗糙度。
    var metallic = clamp(layer.pbr_and_texture.y, 0.0, 1.0); // 读取层金属度。
    if layer.pbr_and_texture.w > 0.5 {
        let tile_meters = max(layer.pbr_and_texture.z, 0.01); // 防御错误地图参数造成除零。
        let tiled_uv = fract(world_xz / tile_meters); // 绝对世界X/Z提供跨primitive连续平铺。
        let tiled_dx = world_dx / tile_meters; // 显式世界梯度驱动数组纹理mip选择。
        let tiled_dy = world_dy / tile_meters; // 不需要Blender导出TEXCOORD属性。
        let slice = i32(layer.pbr_and_texture.w - 1.0 + 0.5); // 编码值减一恢复二维数组切片。
        let sampled_base = textureSampleGrad(
            base_color_array,
            base_color_sampler,
            tiled_uv,
            slice,
            tiled_dx,
            tiled_dy,
        ); // 每个纹理层只执行一次基础色数组采样。
        let sampled_arm = textureSampleGrad(
            arm_array,
            arm_sampler,
            tiled_uv,
            slice,
            tiled_dx,
            tiled_dy,
        ); // 同号ARM切片避免独立纹理绑定爆炸。
        base = base * sampled_base; // 作者乘色与sRGB数组解码后的线性色相乘。
        ao = clamp(sampled_arm.r, 0.0, 1.0); // R通道为环境遮蔽。
        roughness = clamp(roughness * sampled_arm.g, 0.0, 1.0); // G通道为感知粗糙度。
        metallic = clamp(metallic * sampled_arm.b, 0.0, 1.0); // B通道为金属度。
    }
    return BlendedSurface(base, vec3<f32>(ao, roughness, metallic), 0.0); // 草土权重由上层索引逻辑填写。
}

/// 混合当前片元全部有效层；HN-01只有湿土和草土进入纹理采样分支。
fn blend_surface(
    weights: mat2x4<f32>,
    world_xz: vec2<f32>,
    world_dx: vec2<f32>,
    world_dy: vec2<f32>,
) -> BlendedSurface {
    // 三项按原层序独立累加；只在返回时构造结构，避免Function变量携带Offset装饰。
    var base_color = vec4<f32>(0.0);
    var pbr = vec3<f32>(0.0);
    var grass_weight = 0.0;
    let layer_count = min(u32(terrain_surface.counts.x + 0.5), 8u); // 与权重读取使用相同有效层上限。
    let grass_index = min(u32(terrain_surface.counts.w + 0.5), 7u); // 取得唯一草土层编号。
    for (var index = 0u; index < layer_count; index = index + 1u) {
        let weight = surface_weight(weights, index); // 保留原八层顺序，不增加纹理采样或材质变体。
        if weight > 0.000001 {
            let sampled = sample_layer(terrain_surface.layers[index], world_xz, world_dx, world_dy); // uniform分支仅为实际纹理层采样。
            base_color = base_color + sampled.base_color * weight; // 在线性空间混合基础色与透明度。
            pbr = pbr + sampled.pbr * weight; // AO、粗糙度和金属度使用同一归一化权重。
        }
        if index == grass_index {
            grass_weight = weight; // 微法线只受草土最终权重影响，不读取植被密度图。
        }
    }
    return BlendedSurface(base_color, pbr, grass_weight); // 返回一次完成的标准PBR输入参数。
}

fn indexed_channel(value: vec4<u32>, slot: u32) -> u32 {
    switch slot {
        case 0u: { return value.r; }
        case 1u: { return value.g; }
        case 2u: { return value.b; }
        default: { return value.a; }
    }
}

fn indexed_weights_from_vectors(indices: vec4<u32>, weights: vec4<f32>) -> IndexedSurfaceWeights {
    return IndexedSurfaceWeights(indices, weights); // 返回接口保持四槽，构造时不创建带布局装饰的Function临时变量。
}

fn indexed_candidate_index(first: vec4<u32>, second: vec4<u32>, third: vec4<u32>, fourth: vec4<u32>, index: u32) -> u32 {
    let slot = index % 4u;
    switch index / 4u {
        case 0u: { return first[slot]; }
        case 1u: { return second[slot]; }
        case 2u: { return third[slot]; }
        default: { return fourth[slot]; }
    }
}

fn indexed_candidate_weight(weights: mat4x4<f32>, index: u32) -> f32 {
    return weights[index / 4u][index % 4u];
}

/// 在与索引图匹配的整数 Mip 上手动双线性聚合四个纹素的十六个候选。
fn sample_indexed_surface_weights(
    world_xz: vec2<f32>,
    world_dx: vec2<f32>,
    world_dy: vec2<f32>,
) -> IndexedSurfaceWeights {
    var output_indices = vec4<u32>(0u);
    var output_weights = vec4<f32>(0.0);
    let world_min = terrain_surface.world_bounds.xy;
    let world_extent = max(terrain_surface.world_bounds.zw - world_min, vec2<f32>(0.0001));
    let uv = (world_xz - world_min) / world_extent;
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) {
        return indexed_weights_from_vectors(output_indices, output_weights); // 范围外保持裸地，不隐式复制第一层。
    }
    let base_size = vec2<f32>(textureDimensions(index_map, 0));
    let uv_dx = world_dx / world_extent;
    let uv_dy = world_dy / world_extent;
    let footprint = max(length(uv_dx * base_size), length(uv_dy * base_size));
    let max_mip = max(i32(textureNumLevels(index_map)) - 1, 0);
    let mip = clamp(i32(floor(log2(max(footprint, 1.0)))), 0, max_mip);
    let dimensions = textureDimensions(index_map, mip);
    let pixel = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * vec2<f32>(dimensions) - vec2<f32>(0.5);
    let base_coord = vec2<i32>(floor(pixel));
    let fraction = fract(pixel);
    // 四邻纹素的十六候选完整保留；ID用四向量，权重用四列矩阵，避免Function数组ArrayStride。
    var candidate_first = vec4<u32>(0u);
    var candidate_second = vec4<u32>(0u);
    var candidate_third = vec4<u32>(0u);
    var candidate_fourth = vec4<u32>(0u);
    var candidate_weights = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var candidate_count = 0u;
    for (var offset_y = 0u; offset_y < 2u; offset_y = offset_y + 1u) {
        for (var offset_x = 0u; offset_x < 2u; offset_x = offset_x + 1u) {
            let raw_coord = base_coord + vec2<i32>(i32(offset_x), i32(offset_y));
            let coord = clamp(raw_coord, vec2<i32>(0), vec2<i32>(dimensions) - vec2<i32>(1));
            let bilinear_x = select(1.0 - fraction.x, fraction.x, offset_x == 1u);
            let bilinear_y = select(1.0 - fraction.y, fraction.y, offset_y == 1u);
            let bilinear = bilinear_x * bilinear_y;
            let texel_indices = textureLoad(index_map, coord, mip);
            let texel_weights = textureLoad(weight_map_0, coord, mip);
            for (var slot = 0u; slot < 4u; slot = slot + 1u) {
                let contribution = channel(texel_weights, slot) * bilinear;
                if contribution <= 0.0000001 {
                    continue;
                }
                let material = indexed_channel(texel_indices, slot);
                var found = 16u;
                for (var candidate = 0u; candidate < candidate_count; candidate = candidate + 1u) {
                    if indexed_candidate_index(candidate_first, candidate_second, candidate_third, candidate_fourth, candidate) == material {
                        found = candidate;
                        break;
                    }
                }
                if found < 16u {
                    candidate_weights[found / 4u][found % 4u] = indexed_candidate_weight(candidate_weights, found) + contribution;
                } else if candidate_count < 16u {
                    let slot_in_vector = candidate_count % 4u;
                    switch candidate_count / 4u {
                        case 0u: { candidate_first[slot_in_vector] = material; }
                        case 1u: { candidate_second[slot_in_vector] = material; }
                        case 2u: { candidate_third[slot_in_vector] = material; }
                        default: { candidate_fourth[slot_in_vector] = material; }
                    }
                    candidate_weights[candidate_count / 4u][candidate_count % 4u] = contribution;
                    candidate_count = candidate_count + 1u;
                }
            }
        }
    }
    var total = 0.0;
    for (var output = 0u; output < 4u; output = output + 1u) {
        var best_position = 16u;
        var best_weight = 0.0;
        var best_index = 0xffffffffu;
        for (var candidate = 0u; candidate < candidate_count; candidate = candidate + 1u) {
            let value = indexed_candidate_weight(candidate_weights, candidate);
            let index = indexed_candidate_index(candidate_first, candidate_second, candidate_third, candidate_fourth, candidate);
            if value > best_weight || (abs(value - best_weight) <= 0.0000001 && value > 0.0 && index < best_index) {
                best_position = candidate;
                best_weight = value;
                best_index = index;
            }
        }
        if best_position < 16u {
            output_indices[output] = best_index;
            output_weights[output] = best_weight;
            total = total + best_weight;
            candidate_weights[best_position / 4u][best_position % 4u] = 0.0;
        }
    }
    if total <= 0.000001 {
        return indexed_weights_from_vectors(output_indices, output_weights); // 未涂绘纹素保留零权重，不能被新第一层覆盖。
    }
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        output_weights[slot] = output_weights[slot] / total;
    }
    return indexed_weights_from_vectors(output_indices, output_weights);
}

fn decode_indexed_normal(value: vec4<f32>, flip_y: bool) -> vec3<f32> {
    var normal = value.xyz * 2.0 - vec3<f32>(1.0);
    if flip_y {
        normal.y = -normal.y;
    }
    return normalize(vec3<f32>(normal.xy, max(normal.z, 0.001)));
}

fn xz_normal_to_world(tangent_normal: vec3<f32>, geometry_normal: vec3<f32>) -> vec3<f32> {
    var tangent = vec3<f32>(1.0, 0.0, 0.0) - geometry_normal * geometry_normal.x;
    if dot(tangent, tangent) <= 0.000001 {
        tangent = vec3<f32>(0.0, 0.0, 1.0);
    }
    tangent = normalize(tangent);
    let bitangent = normalize(cross(tangent, geometry_normal));
    return normalize(tangent * tangent_normal.x + bitangent * tangent_normal.y + geometry_normal * tangent_normal.z);
}

fn sample_indexed_layer(
    local_index: u32,
    world_position: vec3<f32>,
    world_dx: vec3<f32>,
    world_dy: vec3<f32>,
    geometry_normal: vec3<f32>,
) -> IndexedLayerSample {
    // 存储表自身长度是动态地图层数的精确 u32 权威，避免把任意层数往返经过 f32 uniform。
    let layer_count = max(arrayLength(&indexed_layers), 1u);
    let layer = indexed_layers[min(local_index, layer_count - 1u)];
    if layer.pbr_and_texture.w < 0.0 {
        return IndexedLayerSample(layer.base_color, vec4<f32>(1.0, layer.pbr_and_texture.x, layer.pbr_and_texture.y, 0.5), geometry_normal, 0.0);
    } // 没有岩土贴图的地图使用显式纯参数回退，不索引不存在的纹理切片。
    let tile_meters = max(layer.pbr_and_texture.z, 0.01);
    let slice = i32(layer.pbr_and_texture.w + 0.5);
    let flip_y = layer.normal_height_projection.w > 0.5;
    var base: vec4<f32>;
    var arm_height: vec4<f32>;
    var mapped_normal: vec3<f32>;
    // 【功能设计】挖洞不能把接管列的地表变成白模；朝上的缓坡仍与Clipmap使用同一预设UV。
    // 陡壁及倒扣才自动使用世界三向投影，避免原world_xz纹理沿洞壁拉伸；作者triplanar预设始终保留。
    let volume_wall = terrain_surface.volume_shading.x > 0.5 && geometry_normal.y < 0.70710678;
    if layer.normal_height_projection.z > 0.5 || volume_wall {
        var axis_weights = pow(abs(geometry_normal), vec3<f32>(4.0));
        axis_weights = axis_weights / max(axis_weights.x + axis_weights.y + axis_weights.z, 0.0001);
        if layer.normal_height_projection.z <= 0.5 {
            // 从缓坡的原XZ投影连续过渡到竖壁三向投影，不能在45度边缘出现纹理跳变。
            let wall_blend = 1.0 - smoothstep(0.0, 0.70710678, geometry_normal.y);
            axis_weights = mix(vec3<f32>(0.0, 1.0, 0.0), axis_weights, wall_blend);
        }
        let uv_x = world_position.zy / tile_meters;
        let uv_y = world_position.xz / tile_meters;
        let uv_z = world_position.xy / tile_meters;
        let base_x = textureSampleGrad(base_color_array, base_color_sampler, uv_x, slice, world_dx.zy / tile_meters, world_dy.zy / tile_meters);
        let base_y = textureSampleGrad(base_color_array, base_color_sampler, uv_y, slice, world_dx.xz / tile_meters, world_dy.xz / tile_meters);
        let base_z = textureSampleGrad(base_color_array, base_color_sampler, uv_z, slice, world_dx.xy / tile_meters, world_dy.xy / tile_meters);
        base = base_x * axis_weights.x + base_y * axis_weights.y + base_z * axis_weights.z;
        let arm_x = textureSampleGrad(arm_array, arm_sampler, uv_x, slice, world_dx.zy / tile_meters, world_dy.zy / tile_meters);
        let arm_y = textureSampleGrad(arm_array, arm_sampler, uv_y, slice, world_dx.xz / tile_meters, world_dy.xz / tile_meters);
        let arm_z = textureSampleGrad(arm_array, arm_sampler, uv_z, slice, world_dx.xy / tile_meters, world_dy.xy / tile_meters);
        arm_height = arm_x * axis_weights.x + arm_y * axis_weights.y + arm_z * axis_weights.z;
        let normal_x = decode_indexed_normal(textureSampleGrad(normal_array, normal_sampler, uv_x, slice, world_dx.zy / tile_meters, world_dy.zy / tile_meters), flip_y);
        let normal_y = decode_indexed_normal(textureSampleGrad(normal_array, normal_sampler, uv_y, slice, world_dx.xz / tile_meters, world_dy.xz / tile_meters), flip_y);
        let normal_z = decode_indexed_normal(textureSampleGrad(normal_array, normal_sampler, uv_z, slice, world_dx.xy / tile_meters, world_dy.xy / tile_meters), flip_y);
        let world_normal_x = normalize(vec3<f32>(normal_x.z * select(-1.0, 1.0, geometry_normal.x >= 0.0), normal_x.y, normal_x.x));
        let world_normal_y = xz_normal_to_world(normal_y, geometry_normal);
        let world_normal_z = normalize(vec3<f32>(normal_z.x, normal_z.y, normal_z.z * select(-1.0, 1.0, geometry_normal.z >= 0.0)));
        mapped_normal = normalize(world_normal_x * axis_weights.x + world_normal_y * axis_weights.y + world_normal_z * axis_weights.z);
    } else {
        let uv = world_position.xz / tile_meters;
        base = textureSampleGrad(base_color_array, base_color_sampler, uv, slice, world_dx.xz / tile_meters, world_dy.xz / tile_meters);
        arm_height = textureSampleGrad(arm_array, arm_sampler, uv, slice, world_dx.xz / tile_meters, world_dy.xz / tile_meters);
        let tangent_normal = decode_indexed_normal(textureSampleGrad(normal_array, normal_sampler, uv, slice, world_dx.xz / tile_meters, world_dy.xz / tile_meters), flip_y);
        mapped_normal = xz_normal_to_world(tangent_normal, geometry_normal);
    }
    let normal_strength = clamp(layer.normal_height_projection.x, 0.0, 2.0);
    mapped_normal = normalize(geometry_normal + (mapped_normal - geometry_normal) * normal_strength);
    return IndexedLayerSample(
        layer.base_color * base,
        vec4<f32>(
            clamp(arm_height.r, 0.0, 1.0),
            clamp(layer.pbr_and_texture.x * arm_height.g, 0.0, 1.0),
            clamp(layer.pbr_and_texture.y * arm_height.b, 0.0, 1.0),
            clamp(arm_height.a, 0.0, 1.0),
        ),
        mapped_normal,
        max(layer.normal_height_projection.y, 0.0),
    );
}

// 四个实际纹理样本以矩阵列暂存，混合数学可直接GPU查询；不减少采样或改变四槽顺序。
fn blend_indexed_samples(
    source_weights: vec4<f32>,
    colors: mat4x4<f32>,
    pbr_heights: mat4x4<f32>,
    normals: mat4x3<f32>,
    height_strengths: vec4<f32>,
    geometry_normal: vec3<f32>,
) -> IndexedBlendedSurface {
    let heights = vec4<f32>(pbr_heights[0].w, pbr_heights[1].w, pbr_heights[2].w, pbr_heights[3].w);
    var adjusted_weights = source_weights * max(vec4<f32>(1.0) + (heights - vec4<f32>(0.5)) * 2.0 * height_strengths, vec4<f32>(0.0));
    var total = 0.0;
    total = total + adjusted_weights.x;
    total = total + adjusted_weights.y;
    total = total + adjusted_weights.z;
    total = total + adjusted_weights.w; // 与原循环从零开始累加的浮点顺序一致。
    if total <= 0.000001 {
        adjusted_weights = source_weights;
        total = 1.0; // 保留原作者覆盖量；高度贡献归零不能把部分覆盖重新放大。
    }
    var base_color = vec4<f32>(0.0);
    var pbr = vec3<f32>(0.0);
    var normal = vec3<f32>(0.0);
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        let weight = adjusted_weights[slot] / total;
        base_color = base_color + colors[slot] * weight;
        pbr = pbr + pbr_heights[slot].xyz * weight;
        normal = normal + normals[slot] * weight;
    }
    if dot(normal, normal) <= 0.000001 {
        normal = geometry_normal;
    } else {
        normal = normalize(normal);
    }
    return IndexedBlendedSurface(base_color, pbr, normal);
}

fn blend_indexed_surface(
    source: IndexedSurfaceWeights,
    world_position: vec3<f32>,
    world_dx: vec3<f32>,
    world_dy: vec3<f32>,
    geometry_normal: vec3<f32>,
) -> IndexedBlendedSurface {
    let source_total = source.weights[0] + source.weights[1] + source.weights[2] + source.weights[3]; // 有作者层但局部未涂绘也必须显示裸地。
    if terrain_surface.counts.x < 0.5 && source_total <= 0.000001 { // 空室外层保持白模，地下自动岩土可以独立贡献。
        return IndexedBlendedSurface(vec4<f32>(1.0), vec3<f32>(1.0, 1.0, 0.0), geometry_normal); // 白色粗糙非金属，不采样隐藏预设。
    } // 手动创建第一层后仍由实际绘制权重控制地皮。
    if source_total <= 0.000001 { // 有作者层但局部未涂绘时维持原有中性裸地语义。
        return IndexedBlendedSurface(vec4<f32>(0.5, 0.5, 0.5, 1.0), vec3<f32>(1.0, 1.0, 0.0), geometry_normal); // 未绘制区域不虚构新建层的覆盖。
    } // 两种回退均保持地形几何法线与标准PBR光照。
    var colors = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var pbr_heights = mat4x4<f32>(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    var normals = mat4x3<f32>(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
    var height_strengths = vec4<f32>(0.0);
    for (var slot = 0u; slot < 4u; slot = slot + 1u) {
        let sampled = sample_indexed_layer(source.indices[slot], world_position, world_dx, world_dy, geometry_normal);
        colors[slot] = sampled.base_color;
        pbr_heights[slot] = sampled.pbr_height;
        normals[slot] = sampled.normal;
        height_strengths[slot] = sampled.height_strength;
    }
    return blend_indexed_samples(source.weights, colors, pbr_heights, normals, height_strengths, geometry_normal);
}

/// 将整数世界格映射为确定性零至一值。
fn hash_surface_cell(cell: vec2<f32>) -> f32 {
    var value = fract(vec3<f32>(cell.x, cell.y, cell.x) * vec3<f32>(0.1031, 0.1030, 0.0973)); // 三个互质小数打散二维整数格。
    value = value + dot(value, value.yzx + vec3<f32>(33.33)); // 交叉混合三个通道降低相邻格相关性。
    return fract((value.x + value.y) * value.z); // 折叠为稳定标量随机值。
}

/// 生成一层无硬边值噪声及解析梯度，并按片元脚印带限。
fn sample_surface_variation(
    world_xz: vec2<f32>,
    cell_meters: f32,
    pixel_footprint: f32,
) -> SurfaceVariation {
    let safe_cell_meters = max(cell_meters, 1.0); // 防御损坏配置产生高频噪声。
    let grid_position = world_xz / safe_cell_meters; // 映射到低频世界格。
    let base_cell = floor(grid_position); // 取得当前连续噪声单元左下格。
    let fraction = fract(grid_position); // 保存单元内位置。
    let blend = fraction * fraction * (vec2<f32>(3.0) - fraction * 2.0); // 三次平滑插值消除格边硬折线。
    let blend_derivative = vec2<f32>(6.0) * fraction * (vec2<f32>(1.0) - fraction); // 同一曲线解析导数。
    let value_00 = hash_surface_cell(base_cell); // 左下稳定随机角点。
    let value_10 = hash_surface_cell(base_cell + vec2<f32>(1.0, 0.0)); // 右下稳定随机角点。
    let value_01 = hash_surface_cell(base_cell + vec2<f32>(0.0, 1.0)); // 左上稳定随机角点。
    let value_11 = hash_surface_cell(base_cell + vec2<f32>(1.0, 1.0)); // 右上稳定随机角点。
    let lower = mix(value_00, value_10, blend.x); // 沿世界X混合下边。
    let upper = mix(value_01, value_11, blend.x); // 沿世界X混合上边。
    let raw_value = mix(lower, upper, blend.y); // 沿世界Z得到连续综合色变化。
    let raw_gradient = vec2<f32>(
        mix(value_10 - value_00, value_11 - value_01, blend.y) * blend_derivative.x,
        mix(value_01 - value_00, value_11 - value_10, blend.x) * blend_derivative.y,
    ) / safe_cell_meters; // 求出世界每米X/Z解析梯度。
    let band_limit = 1.0 - smoothstep(safe_cell_meters * 0.18, safe_cell_meters * 0.65, pixel_footprint); // 远景逐步移除不可解析细节。
    return SurfaceVariation(mix(0.5, raw_value, band_limit), raw_gradient * band_limit); // 极远处回归中值和平坦法线。
}

/// 仅在草土权重范围内复用世界噪声梯度生成克制微法线。
fn apply_grass_micro_normal(
    normal: vec3<f32>,
    variation_gradient: vec2<f32>,
    cell_meters: f32,
    grass_weight: f32,
    strength: f32,
) -> vec3<f32> {
    let slope = variation_gradient * max(cell_meters, 1.0) * 2.2; // 把每米梯度恢复为格尺度无关坡度。
    let perturbation = (terrain_surface.source_to_world * vec4<f32>(-slope.x, 0.0, -slope.y, 0.0)).xyz * max(strength, 0.0) * grass_weight; // 源噪声坡度跟随子地图朝向，非草土层不会得到扰动。
    return normalize(normal + perturbation); // Blender平滑网格法线仍是基础受光方向。
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
#ifdef TERRAIN_SURFACE_PRIMITIVE_INDEX
    @builtin(primitive_index) primitive_index: u32, // 光栅三角顺序与动态目标唯一TLAS代理的索引顺序严格一致。
#endif
) -> FragmentOutput {
#ifdef SPARSE_CLIPMAP
    if !clipmap_height::inside_surface(clipmap_height::source_position(in.world_position.xyz).xz) {
        discard; // 有限新地图的主画面与阴影、深度通道使用同一世界边界。
    }
#endif
    var geometry_in = in;
#ifdef VERTEX_COLORS
#ifdef VERTEX_UVS_B
    if terrain_surface.volume_shading.x > 0.5 && terrain_surface.volume_shading.y > 0.5 { geometry_in.color = vec4<f32>(1.0); }
#endif
#endif
    var pbr_input = pbr_input_from_standard_material(geometry_in, is_front); // 体积顶点颜色承载权重，不乘到基础PBR色/透明度。
    let world_position = in.world_position.xyz;
    if terrain_surface.shading.x > 0.5 { // F12场景页平直模式只改变受光法线，不改网格、碰撞、UV或作者数据。
        let unnormalized_face_normal = cross(dpdx(world_position), dpdy(world_position)); // 同一光栅三角形内导数恒定，可直接取得几何面法线。
        if dot(unnormalized_face_normal, unnormalized_face_normal) > 0.000000000001 { // 极端退化投影继续使用原法线，禁止归一化零向量。
            var face_normal = normalize(unnormalized_face_normal); // 得到当前三角形的单位世界法线。
            if dot(face_normal, pbr_input.world_normal) < 0.0 { // 屏幕Y方向或双面材质可能让叉积朝向与当前可见面相反。
                face_normal = -face_normal; // 对齐Bevy已经处理正反面的几何法线方向。
            }
            pbr_input.world_normal = face_normal; // 阴影偏移、积水坡度与直接光读取同一平直几何法线。
            pbr_input.N = face_normal; // 无索引法线贴图时直接使用逐三角形受光方向。
        }
    }
    let terrain_source_position = (terrain_surface.world_to_source * vec4<f32>(world_position, 1.0)).xyz;
    let world_xz = terrain_source_position.xz; // 权重、积水和平铺统一读取源图X/Z，纹理跟随实例移动。
    let source_normal_matrix = transpose(mat3x3<f32>(terrain_surface.source_to_world[0].xyz, terrain_surface.source_to_world[1].xyz, terrain_surface.source_to_world[2].xyz));
    let source_normal = normalize(source_normal_matrix * pbr_input.N); // 非均匀缩放的法线必须使用协变转换。
    let volume_paint = volume_paint_at(terrain_source_position, source_normal); // 三维作者字段和积水均在来源坐标查询。
    let world_dx = dpdx(world_xz); // 所有纹理分支之前取得合法片元梯度。
    let world_dy = dpdy(world_xz); // 与X梯度共同驱动权重和数组完整mip链。
    if terrain_surface.appearance.w < 1.5 { // w=2为旧Blender地形直通，跳过所有地表采样并保留原PbrInput。
    if terrain_surface.appearance.w > 0.5 {
        let world_position_dx = dpdx(terrain_source_position);
        let world_position_dy = dpdy(terrain_source_position);
        let sampled_weights = sample_indexed_surface_weights(world_xz, world_dx, world_dy);
        var indexed_indices = sampled_weights.indices;
        var indexed_weights = sampled_weights.weights;
#ifdef VERTEX_COLORS
#ifdef VERTEX_UVS_B
        if terrain_surface.volume_shading.x > 0.5 && terrain_surface.volume_shading.y > 0.5 {
            let merged = merge_volume_weights(IndexedSurfaceWeights(indexed_indices, indexed_weights), vec4<f32>(in.uv, in.uv_b), in.color);
            indexed_indices = merged.indices;
            indexed_weights = merged.weights;
        }
#endif
#endif
        if terrain_surface.volume_shading.x > 0.5 && volume_origin_step.w > 0.0 {
            let merged = merge_volume_weights(IndexedSurfaceWeights(indexed_indices, indexed_weights), volume_paint.keys, volume_paint.weights);
            indexed_indices = merged.indices;
            indexed_weights = merged.weights;
        }
        let indexed = blend_indexed_surface(IndexedSurfaceWeights(indexed_indices, indexed_weights), terrain_source_position, world_position_dx, world_position_dy, source_normal);
        pbr_input.material.base_color = vec4<f32>(indexed.base_color.rgb, pbr_input.material.base_color.a * indexed.base_color.a);
        pbr_input.material.perceptual_roughness = clamp(indexed.pbr.y, 0.0, 1.0);
        pbr_input.material.metallic = clamp(indexed.pbr.z, 0.0, 1.0);
        pbr_input.diffuse_occlusion = pbr_input.diffuse_occlusion * clamp(indexed.pbr.x, 0.0, 1.0);
        let world_normal_matrix = transpose(mat3x3<f32>(terrain_surface.world_to_source[0].xyz, terrain_surface.world_to_source[1].xyz, terrain_surface.world_to_source[2].xyz));
        pbr_input.N = normalize(world_normal_matrix * indexed.normal);
    } else {
        let weights = sample_surface_weights(world_xz, world_dx, world_dy); // HN-01固定一次权重纹理采样。
        let blended = blend_surface(weights, world_xz, world_dx, world_dy); // HN-01额外读取湿土、草土各自基础色与ARM。
        pbr_input.material.base_color = vec4<f32>(blended.base_color.rgb, pbr_input.material.base_color.a * blended.base_color.a); // 白模只保留标准透明度契约。
        pbr_input.material.perceptual_roughness = clamp(blended.pbr.y, 0.0, 1.0); // 写入五层混合后的感知粗糙度。
        pbr_input.material.metallic = clamp(blended.pbr.z, 0.0, 1.0); // 写入五层混合后的金属度。
        pbr_input.diffuse_occlusion = pbr_input.diffuse_occlusion * clamp(blended.pbr.x, 0.0, 1.0); // ARM红通道只调制间接漫反射遮蔽。
        if blended.grass_weight > 0.0001 && terrain_surface.appearance.x > 0.0001 {
            let cell_meters = max(terrain_surface.appearance.y, 1.0); // 读取草土世界变化格尺度。
            let pixel_footprint = max(length(world_dx), length(world_dy)); // 估算当前片元覆盖世界米数。
            let variation = sample_surface_variation(world_xz, cell_meters, pixel_footprint); // 一次ALU噪声同时支持综合色和微法线。
            let grass_tint = 0.94 + variation.value * 0.12; // 在草土内部保留原有低幅世界稳定综合色。
            pbr_input.material.base_color = vec4<f32>(
                pbr_input.material.base_color.rgb * mix(1.0, grass_tint, blended.grass_weight),
                pbr_input.material.base_color.a,
            ); // 综合色严格按草土权重淡入，不影响湿土、道路和府城。
            pbr_input.N = apply_grass_micro_normal(
                pbr_input.N,
                variation.gradient,
                cell_meters,
                blended.grass_weight,
                terrain_surface.appearance.x,
            ); // 只修改光照法线，不改变碰撞、阴影用网格法线或几何。
        }
    }

    } // 固定层、索引层和原材质直通在此汇合，共用同一雨天外观。
    if surface_weather.weather.x > 0.0001 || surface_weather.weather.y > 0.0001 || surface_weather.options.y > 0.5 { // 干燥且非作者预览时完全跳过新增采样和噪声。
        var potential = 0.0; // 无作者积水图时只有普遍湿润，不凭空制造水坑。
        let source_mask = terrain_surface.shading.y > 0.5;
        let bounds = select(surface_weather.bounds, terrain_surface.world_bounds, source_mask); // 每个来源使用自己的纹理范围，天气和时间仍共享父世界。
        let extent = max(bounds.zw - bounds.xy, vec2<f32>(0.001)); // 无地图回退保持有限除法。
        let uv = (world_xz - bounds.xy) / extent; // 首尾纹素与作者世界端点一致。
        if select(surface_weather.options.x, terrain_surface.shading.z, source_mask) > 0.5 && all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0)) { // 地图外禁止Clamp导致遮罩延伸。
            let size = vec2<f32>(textureDimensions(puddle_mask)); // R8首尾像素中心映射，避免半像素偏移。
            let mask_uv = (uv * (size - vec2<f32>(1.0)) + vec2<f32>(0.5)) / size; // 与作者R8端点约定一致。
            let mask_scale = (size - vec2<f32>(1.0)) / (size * extent); // 显式梯度支持距离Mip并防止导数均匀性错误。
            potential = textureSampleGrad(puddle_mask, puddle_sampler, mask_uv, world_dx * mask_scale, world_dy * mask_scale).r; // 唯一新增遮罩采样。
        } // 完成受世界范围保护的潜力读取。
        if volume_origin_step.w>0.0 {
            let native_top = terrain_surface.volume_shading.w > 0.5 && source_normal.y > 0.5
                && abs(terrain_source_position.y-terrain_surface.volume_shading.z) <= volume_origin_step.w*0.5;
            if !native_top || volume_paint.puddles>0.0 {potential=volume_paint.puddles;}
        } // 顶面未写三维积水时沿用对象XZ；材质/种植笔触不能擦掉另一个积水源。
        var noise = 0.5; // 远景用平均值防止固定高频噪声闪烁。
        let pixel_meters = max(length(world_dx), length(world_dy)); // 上方一致控制流的导数无需在分支内重新求值。
        if potential > 0.0001 && surface_weather.weather.y > 0.0001 { // 只在实际潜力区域评估边缘噪声。
            let detail = 1.0 - smoothstep(0.25, 1.0, pixel_meters); // 亚像素边缘平滑退回作者遮罩。
            if detail > 0.0001 { noise = mix(0.5, wet_surface_noise(world_xz * 1.5), detail); } // 固定四角噪声，没有随水洼数量增长的循环。
        } // 完成局部积水边缘变化。
        let exposure = rain_shelter_exposure(world_position); // 显式无碰撞屋檐也控制局部受雨，作者蓝图仍显示原潜力。
        let wet = evaluate_wet_surface(pbr_input.material.perceptual_roughness, pbr_input.N, normalize(pbr_input.world_normal), potential, surface_weather.weather.x * exposure, surface_weather.weather.y * exposure, noise); // 局部视觉遮挡不污染区域蓄水历史。
        pbr_input.material.base_color = vec4<f32>(pbr_input.material.base_color.rgb * wet.color_scale, pbr_input.material.base_color.a); // 保留纹理、透明度和原色调。
        let water_roughness = water_reflection_roughness(wet.roughness, max(surface_weather.weather.z, wet.puddle * 0.25), surface_weather.wind_quality.z, pixel_meters); // 雨后仍有水膜时保留较弱抗闪，避免湿痕期间远景反射突然变锐。
        pbr_input.material.perceptual_roughness = mix(wet.roughness, water_roughness, wet.puddle); // 只展宽水膜镜面，湿土保留独立材质差异。
        pbr_input.material.reflectance = mix(pbr_input.material.reflectance, vec3<f32>(WATER_SURFACE_REFLECTANCE), wet.puddle); // 水膜以真实非金属F0进入标准Fresnel，原地表反射在水膜外保持原值。
        pbr_input.material.metallic = mix(pbr_input.material.metallic, 0.0, wet.puddle); // 水膜是非金属，不能靠金属度伪造反射。
        pbr_input.N = wet.normal; // 水膜覆盖处压平底层高频法线。
        if wet.puddle > 0.0001 { // 雨纹仅作用于真正显示的积水区域。
            let camera_distance = distance(world_position, view.world_position); // 同一距离驱动雨滴和风纹质量门。
            let slope = sample_surface_rain_slope(world_xz, surface_weather.weather.w, surface_weather.weather.z * wet.puddle, surface_weather.wind_quality.w, camera_distance, pixel_meters, surface_weather.options.zw); // 与河湖同源的暂停时钟、质量设置和固定九候选。
            let wind_slope = sample_surface_wind_slope(world_xz, surface_weather.weather.w, surface_weather.wind_quality.xy, surface_weather.wind_quality.z, surface_weather.wind_quality.w, camera_distance, pixel_meters); // 浅积水采用同方向低幅风纹，不改变地形或积水覆盖。
            pbr_input.N = normalize(pbr_input.N + vec3<f32>(slope.x + wind_slope.x * 0.35, 0.0, slope.y + wind_slope.y * 0.35) * wet.puddle); // 水膜风纹弱于河湖且边缘连续消退。
        } // 完成积水雨滴细纹。
        if surface_weather.options.y > 0.5 { // 作者仅在积水工具激活时查看蓝色潜力，晴天同样可画。
            pbr_input.material.base_color = vec4<f32>(mix(pbr_input.material.base_color.rgb, vec3<f32>(0.025, 0.40, 0.85), potential * 0.75), pbr_input.material.base_color.a); // 预览覆盖不写回作者源或天气状态。
            pbr_input.material.perceptual_roughness = max(pbr_input.material.perceptual_roughness, 0.50); // 编辑时降低镜面干扰，切工具恢复真实雨景。
        } // 完成作者潜力预览。
    } // 完成前向与Deferred共用的地表雨天PBR接入。

#ifdef PREPASS_PIPELINE
    var out = deferred_output(in, pbr_input); // 先生成完整标准Deferred字段，动态会话随后只重排已正式保留的元数据通道。
#ifdef TERRAIN_SURFACE_PRIMITIVE_INDEX
#ifdef SPARSE_CLIPMAP
    if terrain_surface.appearance.z > 0.5 && clipmap_height::raytracing_proxy_enabled() {
#else
    if terrain_surface.appearance.z > 0.5 { // 只有动态地图光追生命周期创建的非自发光会话副本发布三角身份。
#endif
        let surface_normal = normalize(pbr_input.N); // A槽保存当前平滑或F12平直模式后的最终地表法线。
        let deferred_flags = unpack_flags(out.deferred.a) | DEFERRED_FLAGS_DYNAMIC_TERRAIN_BIT; // 只使用Bevy未占用的第四位标记动态地形。
        out.deferred.a = pack_24bit_normal_and_flags(octahedral_encode(surface_normal), deferred_flags); // 当前法线与官方低三位逐值保留，仅附加项目身份位。
#ifdef SPARSE_CLIPMAP
        out.deferred.a = pack_24bit_normal_and_flags(octahedral_encode(surface_normal), deferred_flags | DEFERRED_FLAGS_CLIPMAP_TERRAIN_BIT);
        out.deferred.g = (clipmap_height::raytracing_triangle_index(clipmap_height::source_position(in.world_position.xyz).xz) & DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK)
            | (clipmap_height::raytracing_instance_index() << 19u); // 四位实例路由；RGB9E5指数仍为零。
        // 上方已核对代理租约；未拥有租约的地表保持标准 Deferred 输出。
#else
        out.deferred.g = primitive_index & DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK; // G槽低十九位保存同序三角编号，高十三位恒零；普通Deferred按RGB9E5解码时伪发光最多约3.05e-5。
#endif
    }
#endif
#else
    let lit_color = apply_pbr_lighting(pbr_input); // 继续使用标准直接光、环境光和阴影。
    let out = FragmentOutput(main_pass_post_lighting_processing(pbr_input, lit_color)); // SSA构造标准前向输出，保留雾/曝光且避免Function结构布局。
#endif

    return out; // 地表无discard且永远是单个Opaque primitive。
}
