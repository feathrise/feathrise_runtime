// Solari会跳过Bevy标准Deferred照明，本Shader只补回同一EnvironmentMapLight的IBL项。
// GPU草叶已有Forward最终色所以整项跳过；动态地形保留第十轮无主表面镜面的实机稳定基线。

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_pbr::{
    clustered_forward,
    environment_map,
    lighting,
    lighting::{LAYER_BASE, LAYER_CLEARCOAT},
    mesh_view_bindings as view_bindings,
    pbr_deferred_functions::pbr_input_from_deferred_gbuffer,
    pbr_deferred_types::{DEFERRED_FLAGS_UNLIT_BIT, unpack_flags},
    pbr_functions,
    prepass_utils::prepass_depth,
}

const DEFERRED_FLAGS_DYNAMIC_TERRAIN_BIT: u32 = 1u << 3u; // 与动态地表标准GBuffer第四位契约一致。
const DEFERRED_FLAGS_GPU_VEGETATION_BIT: u32 = 1u << 7u; // 与GPU blade Deferred首表面类别契约一致。

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<u32>(in.position.xy); // 附件绝对像素坐标与MeshView预通道纹理一致。
    let depth = prepass_depth(in.position, 0u); // Solari正式视图固定单采样。
    if depth == 0.0 {
        return vec4(0.0); // 天空背景由Skybox自行绘制，禁止把环境光加到无表面像素。
    }
    let gpixel = textureLoad(view_bindings::deferred_prepass_texture, vec2<i32>(pixel), 0); // 读取标准Rgba32Uint GBuffer。
    let deferred_flags = unpack_flags(gpixel.a); // 高八位同时承载官方与项目类别标记。
    if (deferred_flags & DEFERRED_FLAGS_UNLIT_BIT) != 0u
        || (deferred_flags & DEFERRED_FLAGS_GPU_VEGETATION_BIT) != 0u
    {
        return vec4(0.0); // Unlit不受环境光；草叶Forward已经消费环境项，禁止双算。
    }

    let frag_coord = vec4(in.position.xy, depth, 1.0); // 官方Deferred解析器使用绝对片元坐标恢复NDC。
    let pbr_input = pbr_input_from_deferred_gbuffer(frag_coord, gpixel); // 逐字段复用Bevy 0.19.1材质解码。
    let base_color = pbr_input.material.base_color.rgb; // 线性基础色。
    let metallic = pbr_input.material.metallic; // 标准金属度。
    let reflectance = pbr_input.material.reflectance; // 介质F0作者参数。
    let perceptual_roughness = pbr_input.material.perceptual_roughness; // 镜面Mip选择使用感知粗糙度。
    let roughness = lighting::perceptualRoughnessToRoughness(perceptual_roughness); // BRDF使用平方粗糙度并保持官方下限。
    let N = pbr_input.N; // GBuffer平滑/微表面世界法线。
    let V = pbr_input.V; // 官方透视或正交观察方向。
    let NdotV = max(dot(N, V), 0.0001); // 对齐标准PBR的有限BRDF输入。
    let R = reflect(-V, N); // 预过滤镜面Cube使用确定性反射方向。
    let diffuse_color = pbr_functions::calculate_diffuse_color(base_color, metallic, 0.0, 0.0); // Deferred不承载透射材质。
    let F_ab = lighting::F_AB(perceptual_roughness, NdotV); // 使用Bevy同源split-sum近似系数。

    var lighting_input: lighting::LightingInput; // 只填充环境贴图函数实际需要的标准字段。
    lighting_input.layers[LAYER_BASE].NdotV = NdotV; // 基础层观察余弦。
    lighting_input.layers[LAYER_BASE].N = N; // 基础层世界法线。
    lighting_input.layers[LAYER_BASE].R = R; // 基础层镜面采样方向。
    lighting_input.layers[LAYER_BASE].perceptual_roughness = perceptual_roughness; // 镜面Mip层级。
    lighting_input.layers[LAYER_BASE].roughness = roughness; // 粗糙反射方向混合。
    lighting_input.P = pbr_input.world_position.xyz; // 局部反射探针查询和视图环境回退的世界位置。
    lighting_input.V = V; // 多重散射与清漆层共用观察方向。
    lighting_input.diffuse_color = diffuse_color; // 金属材质不产生Lambert天空补光。
    lighting_input.metallic = metallic; // 介质与金属多重散射混合权重。
    lighting_input.F0_dielectric = pbr_functions::calculate_F0_dielectric(reflectance); // 介质法向反射率。
    lighting_input.F0_metallic = base_color; // 金属法向反射率来自基础色。
    lighting_input.F_ab = F_ab; // split-sum积分系数。
    lighting_input.layers[LAYER_CLEARCOAT].NdotV = NdotV; // Deferred清漆法线与基础GBuffer法线一致。
    lighting_input.layers[LAYER_CLEARCOAT].N = N; // 清漆世界法线。
    lighting_input.layers[LAYER_CLEARCOAT].R = R; // 清漆反射方向。
    lighting_input.layers[LAYER_CLEARCOAT].perceptual_roughness = pbr_input.material.clearcoat_perceptual_roughness; // 清漆作者粗糙度。
    lighting_input.layers[LAYER_CLEARCOAT].roughness = lighting::perceptualRoughnessToRoughness(pbr_input.material.clearcoat_perceptual_roughness); // 清漆BRDF平方粗糙度。
    lighting_input.clearcoat_strength = pbr_input.material.clearcoat; // 标准GBuffer四位清漆强度。

    let cluster_index = clustered_forward::view_fragment_cluster_index(in.position.xy, depth, false); // 与Deferred PBR相同的局部探针分簇。
    var clusterable_object_index_ranges =
        clustered_forward::unpack_clusterable_object_index_ranges(cluster_index); // 绑定数组设备可先采局部探针再回退天空。
    let environment_light = environment_map::environment_map_light(
        &lighting_input,
        &clusterable_object_index_ranges,
        false,
    ); // `false`表示本Solari路径尚未取得任何其他环境漫反射来源。

    var indirect_light = environment_light.diffuse * pbr_input.diffuse_occlusion; // 独立天空补光强度已编码进漫反射Cube比例。
    if (deferred_flags & DEFERRED_FLAGS_DYNAMIC_TERRAIN_BIT) == 0u {
        indirect_light += environment_light.specular * pbr_input.specular_occlusion; // 普通材质恢复独立天空反射与GGX Mip。
    }
    return vec4(view_bindings::view.exposure * indirect_light, 0.0); // 与标准PBR在HDR线性空间采用同一曝光并交给硬件加法。
}
