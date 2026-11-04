// Bevy Solari 0.19.1 restir_di.wgsl 的正式DI稳定策略副本，并包含直接光漫反射的掠射视角兼容修正。
// 全屏停用DI同帧spatial reservoir；DI漫反射保留平滑入射Fresnel与余弦，动态地形只以真实三角判断相机可见侧。
// 第九轮已排除光源背面漏光；第十轮实机确认隔离动态地形主表面GGX镜面可保持平滑，并消除随视角扩大的橙黄轮廓。
// https://intro-to-restir.cwyman.org/presentations/2023ReSTIR_Course_Notes.pdf
// https://d1qx31qr3h6wln.cloudfront.net/publications/ReSTIR%20GI.pdf

enable wgpu_ray_query;

#import bevy_core_pipeline::tonemapping::tonemapping_luminance as luminance
#import bevy_pbr::prepass_bindings::PreviousViewUniforms
#import bevy_pbr::pbr_functions::{calculate_diffuse_color, calculate_F0}
#import bevy_pbr::utils::{rand_f, rand_range_u, sample_disk}
#import bevy_render::maths::PI
#import bevy_render::view::View
#import bevy_solari::brdf::{evaluate_specular_brdf, fresnel}
#import feathrise_raytracing::gbuffer_shadow_terminator::{gpixel_resolve, pixel_dissimilar, permute_pixel, primary_ray_origin, primary_ray_surface, ResolvedGPixel}
#import bevy_solari::presample_light_tiles::unpack_resolved_light_sample
#import bevy_solari::sampling::{LightSample, ResolvedLightSample, NULL_LIGHT_ID, calculate_resolved_light_contribution, resolve_and_calculate_light_contribution, resolve_light_sample, trace_light_visibility, balance_heuristic}
#import bevy_solari::scene_bindings::{light_sources, previous_frame_light_id_translations, LIGHT_NOT_PRESENT_THIS_FRAME, ResolvedMaterial}
#import bevy_solari::specular_gi::SPECULAR_GI_FOR_DI_ROUGHNESS_THRESHOLD
#import bevy_solari::realtime_bindings::{view_output, light_tile_samples, light_tile_resolved_samples, di_reservoirs_a, di_reservoirs_b, gbuffer, depth_buffer, motion_vectors, previous_gbuffer, previous_depth_buffer, view, previous_view, constants, ResolvedLightSamplePacked}

const INITIAL_SAMPLES = 8u;
const SPATIAL_REUSE_RADIUS_PIXELS = 30.0;
const CONFIDENCE_WEIGHT_CAP = 20.0;

const DISABLE_DI_SPATIAL_REUSE: bool = true; // 已验稳定基线固定停用同帧邻居reservoir；initial/temporal与两次visibility保持官方路径。

@compute @workgroup_size(8, 8, 1)
fn initial_and_temporal(@builtin(workgroup_id) workgroup_id: vec3<u32>, @builtin(global_invocation_id) global_id: vec3<u32>) {
    if any(global_id.xy >= vec2u(view.main_pass_viewport.zw)) { return; }

    let pixel_index = global_id.x + global_id.y * u32(view.main_pass_viewport.z);
    var rng = pixel_index + constants.frame_index;

    let depth = textureLoad(depth_buffer, global_id.xy, 0);
    if depth == 0.0 {
        store_reservoir_b(global_id.xy, empty_reservoir());
        return;
    }
    let surface = gpixel_resolve(textureLoad(gbuffer, global_id.xy, 0), depth, global_id.xy, view.main_pass_viewport.zw, view.world_from_clip);

    let diffuse_brdf = surface.material.base_color / PI;
    let initial_reservoir = generate_initial_reservoir(surface, diffuse_brdf, workgroup_id.xy, &rng); // 保留平滑法线采样，只把可见性射线起点交给动态地形专用修正。
    let temporal = load_temporal_reservoir(global_id.xy, depth, surface.world_position, surface.world_normal);
    let merge_result = merge_reservoirs(initial_reservoir, surface.world_position, surface.world_normal, diffuse_brdf,
        temporal.reservoir, temporal.world_position, temporal.world_normal, temporal.diffuse_brdf, &rng);

    store_reservoir_b(global_id.xy, merge_result.merged_reservoir);
}

@compute @workgroup_size(8, 8, 1)
fn spatial_and_shade(@builtin(global_invocation_id) global_id: vec3<u32>) {
    if any(global_id.xy >= vec2u(view.main_pass_viewport.zw)) { return; }

    let pixel_index = global_id.x + global_id.y * u32(view.main_pass_viewport.z);
    var rng = pixel_index + constants.frame_index;

    let depth = textureLoad(depth_buffer, global_id.xy, 0);
    if depth == 0.0 {
        store_reservoir_a(global_id.xy, empty_reservoir());
        return;
    }
    let surface = gpixel_resolve(textureLoad(gbuffer, global_id.xy, 0), depth, global_id.xy, view.main_pass_viewport.zw, view.world_from_clip);
    let ray_surface = primary_ray_surface(surface); // 动态地形一次三角读取同时提供最终visibility起点与真实相机侧法线。

    let diffuse_brdf = surface.material.base_color / PI;
    let input_reservoir = load_reservoir_b(global_id.xy);
    let spatial = load_spatial_reservoir(global_id.xy, depth, surface.world_position, surface.world_normal, &rng);
    let merge_result = merge_reservoirs(input_reservoir, surface.world_position, surface.world_normal, diffuse_brdf,
        spatial.reservoir, spatial.world_position, spatial.world_normal, spatial.diffuse_brdf, &rng);
    var combined_reservoir = merge_result.merged_reservoir;

    // More accuracy, less stability
#ifndef BIASED_RESAMPLING
    store_reservoir_a(global_id.xy, combined_reservoir);
#endif

    if reservoir_valid(combined_reservoir) {
        combined_reservoir.unbiased_contribution_weight *= trace_light_visibility(ray_surface.ray_origin, merge_result.selected_light_world_position); // 动态低模地形从虚拟平滑面发射，其他像素逐值保持官方偏移。
    }

    // More stability, less accuracy (shadows extend further out than they should)
#ifdef BIASED_RESAMPLING
    store_reservoir_a(global_id.xy, combined_reservoir);
#endif

    let wo = normalize(view.world_position - surface.world_position);
    var brdf = evaluate_di_diffuse_brdf_without_view_fresnel(wo, merge_result.wi, surface.world_normal, ray_surface.view_side_normal, surface.material); // 动态地形复用同次解析的几何法线判定相机侧，平滑法线仍计算最终漫反射能量。
    // Only consider the specular lobe if the surface is not smooth, else leave it for the specular GI pass to handle
    if surface.dynamic_terrain == 0u && surface.material.roughness > SPECULAR_GI_FOR_DI_ROUGHNESS_THRESHOLD { // 第十轮已验收策略隔离动态地形主表面镜面，普通材质仍使用官方粗糙度分流。
        brdf += evaluate_specular_brdf(wo, merge_result.wi, surface.world_normal, surface.material); // 角色及其他标准材质逐值保持Bevy Solari 0.19.1镜面实现。
    }

    var pixel_color = merge_result.selected_sample_radiance * combined_reservoir.unbiased_contribution_weight;
    pixel_color *= brdf;
    pixel_color += surface.material.emissive;
    pixel_color *= view.exposure;
    textureStore(view_output, global_id.xy, vec4(pixel_color, 1.0));
}

// Bevy后续Solari已不再用逐视线Fresnel压暗漫反射；动态地形另用几何法线判定相机是否位于真实三角可见侧。
fn evaluate_di_diffuse_brdf_without_view_fresnel(wo: vec3<f32>, wi: vec3<f32>, world_normal: vec3<f32>, view_side_normal: vec3<f32>, material: ResolvedMaterial) -> vec3<f32> {
    let NdotL = dot(world_normal, wi); // 入射余弦继续决定直接光照度与正半球有效性。
    let view_NdotV = dot(view_side_normal, wo); // 普通材质仍是平滑法线；动态地形由真实光栅三角判断相机可见侧。
    if NdotL < 0.0001 || view_NdotV < 0.0001 { return vec3(0.0); } // 恢复已消除黑边的第七轮基线，不保留第九轮无效光源侧门。
    let diffuse_color = calculate_diffuse_color(material.base_color, material.metallic, 0.0, 0.0) / PI; // 基础色、金属度与Lambert归一化保持官方输入。
    let F0 = calculate_F0(material.base_color, material.metallic, vec3(material.reflectance)); // reflectance到F0的映射保持Bevy PBR正式公式。
    let incident_layering = 1.0 - fresnel(F0, NdotL); // 保留入射侧Fresnel，避免把本轮修正扩大为完整材质模型替换。
    return diffuse_color * incident_layering * NdotL; // 只移除旧公式额外乘入的视线侧Fresnel透射项。
}

fn generate_initial_reservoir(surface: ResolvedGPixel, diffuse_brdf: vec3<f32>, workgroup_id: vec2<u32>, rng: ptr<function, u32>) -> Reservoir {
    var workgroup_rng = (workgroup_id.x * 5782582u) + workgroup_id.y;
    let light_tile_start = rand_range_u(128u, &workgroup_rng) * 1024u;

    var reservoir = empty_reservoir();
    var weight_sum = 0.0;
    let mis_weight = 1.0 / f32(INITIAL_SAMPLES);

    var reservoir_target_function = 0.0;
    var light_sample_world_position = vec4(0.0);
    var selected_tile_sample = 0u;
    for (var i = 0u; i < INITIAL_SAMPLES; i++) {
        let tile_sample = light_tile_start + rand_range_u(1024u, rng);
        let resolved_light_sample = unpack_resolved_light_sample(light_tile_resolved_samples[tile_sample], view.exposure);
        let light_contribution = calculate_resolved_light_contribution(resolved_light_sample, surface.world_position, surface.world_normal); // BRDF候选仍使用真实光栅位置和平滑受光法线。

        let target_function = luminance(light_contribution.radiance * diffuse_brdf * saturate(dot(light_contribution.wi, surface.world_normal))); // 重要性采样不因射线起点修正产生三角棱角。
        let resampling_weight = mis_weight * (target_function * light_contribution.inverse_pdf);

        weight_sum += resampling_weight;

        if rand_f(rng) < resampling_weight / weight_sum {
            reservoir_target_function = target_function;
            light_sample_world_position = resolved_light_sample.world_position;
            selected_tile_sample = tile_sample;
        }
    }

    if reservoir_target_function != 0.0 {
        reservoir.sample = light_tile_samples[selected_tile_sample];
    }

    if reservoir_valid(reservoir) {
        let inverse_target_function = select(0.0, 1.0 / reservoir_target_function, reservoir_target_function > 0.0);
        reservoir.unbiased_contribution_weight = weight_sum * inverse_target_function;

        reservoir.unbiased_contribution_weight *= trace_light_visibility(primary_ray_origin(surface), light_sample_world_position); // 只有实际选中有效光源后才惰性读取动态地形三角数据。
    }

    reservoir.confidence_weight = 1.0;
    return reservoir;
}

fn load_temporal_reservoir(pixel_id: vec2<u32>, depth: f32, world_position: vec3<f32>, world_normal: vec3<f32>) -> NeighborInfo {
    let motion_vector = textureLoad(motion_vectors, pixel_id, 0).xy;
    let temporal_pixel_id_float = round(vec2<f32>(pixel_id) - (motion_vector * view.main_pass_viewport.zw));
    var point_temporal_pixel_id = vec2<u32>(temporal_pixel_id_float);

    if bool(constants.reset) {
        return NeighborInfo(empty_reservoir(), vec3(0.0), vec3(0.0), vec3(0.0));
    }

    if any(temporal_pixel_id_float < vec2(0.0)) || any(temporal_pixel_id_float >= view.main_pass_viewport.zw) {
        point_temporal_pixel_id = pixel_id;
    }

    let permuted_temporal_pixel_id = permute_pixel(point_temporal_pixel_id, constants.frame_index, view.main_pass_viewport.zw);
    var temporal = load_temporal_reservoir_inner(permuted_temporal_pixel_id, depth, world_position, world_normal);

    // Check if the light selected in the previous frame no longer exists in the current frame (e.g. entity despawned)
    let previous_light_id = temporal.reservoir.sample.light_id >> 16u;
    let triangle_id = temporal.reservoir.sample.light_id & 0xFFFFu;
    let light_id = previous_frame_light_id_translations[previous_light_id];
    if light_id == LIGHT_NOT_PRESENT_THIS_FRAME {
        return NeighborInfo(empty_reservoir(), vec3(0.0), vec3(0.0), vec3(0.0));
    }
    temporal.reservoir.sample.light_id = (light_id << 16u) | triangle_id;

    temporal.reservoir.confidence_weight = min(temporal.reservoir.confidence_weight, CONFIDENCE_WEIGHT_CAP);

    return temporal;
}

fn load_temporal_reservoir_inner(temporal_pixel_id: vec2<u32>, depth: f32, world_position: vec3<f32>, world_normal: vec3<f32>) -> NeighborInfo {
    // Check if the pixel features have changed heavily between the current and previous frame
    let temporal_depth = textureLoad(previous_depth_buffer, temporal_pixel_id, 0);
    let temporal_surface = gpixel_resolve(textureLoad(previous_gbuffer, temporal_pixel_id, 0), temporal_depth, temporal_pixel_id, view.main_pass_viewport.zw, previous_view.world_from_clip);
    let temporal_diffuse_brdf = temporal_surface.material.base_color / PI;
    if pixel_dissimilar(depth, world_position, temporal_surface.world_position, world_normal, temporal_surface.world_normal, view) {
        return NeighborInfo(empty_reservoir(), vec3(0.0), vec3(0.0), vec3(0.0));
    }

    let temporal_reservoir = load_reservoir_a(temporal_pixel_id);
    return NeighborInfo(temporal_reservoir, temporal_surface.world_position, temporal_surface.world_normal, temporal_diffuse_brdf);
}

fn load_spatial_reservoir(pixel_id: vec2<u32>, depth: f32, world_position: vec3<f32>, world_normal: vec3<f32>, rng: ptr<function, u32>) -> NeighborInfo {
    if DISABLE_DI_SPATIAL_REUSE {
        return NeighborInfo(empty_reservoir(), world_position, world_normal, vec3(0.0));
    }
    for (var i = 0u; i < 5u; i++) {
        let spatial_pixel_id = get_neighbor_pixel_id(pixel_id, SPATIAL_REUSE_RADIUS_PIXELS, rng);

        let spatial_depth = textureLoad(depth_buffer, spatial_pixel_id, 0);
        let spatial_surface = gpixel_resolve(textureLoad(gbuffer, spatial_pixel_id, 0), spatial_depth, spatial_pixel_id, view.main_pass_viewport.zw, view.world_from_clip);
        let spatial_diffuse_brdf = spatial_surface.material.base_color / PI;
        if pixel_dissimilar(depth, world_position, spatial_surface.world_position, world_normal, spatial_surface.world_normal, view) {
            continue;
        }

        let spatial_reservoir = load_reservoir_b(spatial_pixel_id);
        return NeighborInfo(spatial_reservoir, spatial_surface.world_position, spatial_surface.world_normal, spatial_diffuse_brdf);
    }

    return NeighborInfo(empty_reservoir(), world_position, world_normal, vec3(0.0));
}

fn get_neighbor_pixel_id(center_pixel_id: vec2<u32>, search_radius: f32, rng: ptr<function, u32>) -> vec2<u32> {
    var spatial_id = vec2<f32>(center_pixel_id) + sample_disk(search_radius, rng);
    spatial_id = clamp(spatial_id, vec2(0.0), view.main_pass_viewport.zw - 1.0);
    return vec2<u32>(spatial_id);
}

struct NeighborInfo {
    reservoir: Reservoir,
    world_position: vec3<f32>,
    world_normal: vec3<f32>,
    diffuse_brdf: vec3<f32>,
}

struct Reservoir {
    sample: LightSample,
    confidence_weight: f32,
    unbiased_contribution_weight: f32,
}

fn empty_reservoir() -> Reservoir {
    return Reservoir(
        LightSample(NULL_LIGHT_ID, 0u),
        0.0,
        0.0,
    );
}

fn reservoir_valid(reservoir: Reservoir) -> bool {
    return reservoir.sample.light_id != NULL_LIGHT_ID;
}

fn pack_reservoir(reservoir: Reservoir) -> vec4<u32> {
    let weights = bitcast<vec2<u32>>(vec2<f32>(reservoir.confidence_weight, reservoir.unbiased_contribution_weight));
    return vec4<u32>(reservoir.sample.light_id, reservoir.sample.seed, weights);
}

fn store_reservoir_a(pixel: vec2<u32>, reservoir: Reservoir) {
    textureStore(di_reservoirs_a, pixel, pack_reservoir(reservoir));
}

fn store_reservoir_b(pixel: vec2<u32>, reservoir: Reservoir) {
    textureStore(di_reservoirs_b, pixel, pack_reservoir(reservoir));
}

fn unpack_reservoir(packed: vec4<u32>) -> Reservoir {
    let weights = bitcast<vec2<f32>>(packed.zw);
    return Reservoir(LightSample(packed.x, packed.y), weights.x, weights.y);
}

fn load_reservoir_a(pixel: vec2<u32>) -> Reservoir {
    return unpack_reservoir(textureLoad(di_reservoirs_a, pixel));
}

fn load_reservoir_b(pixel: vec2<u32>) -> Reservoir {
    return unpack_reservoir(textureLoad(di_reservoirs_b, pixel));
}

struct ReservoirMergeResult {
    merged_reservoir: Reservoir,
    selected_sample_radiance: vec3<f32>,
    wi: vec3<f32>,
    selected_light_world_position: vec4<f32>,
}

fn merge_reservoirs(
    canonical_reservoir: Reservoir,
    canonical_world_position: vec3<f32>,
    canonical_world_normal: vec3<f32>,
    canonical_diffuse_brdf: vec3<f32>,
    other_reservoir: Reservoir,
    other_world_position: vec3<f32>,
    other_world_normal: vec3<f32>,
    other_diffuse_brdf: vec3<f32>,
    rng: ptr<function, u32>,
) -> ReservoirMergeResult {
    // Resolve each light sample once, then evaluate at both positions
    var canonical_resolved: ResolvedLightSample;
    if reservoir_valid(canonical_reservoir) {
        canonical_resolved = resolve_light_sample(canonical_reservoir.sample, light_sources[canonical_reservoir.sample.light_id >> 16u]);
    }
    var other_resolved: ResolvedLightSample;
    if reservoir_valid(other_reservoir) {
        other_resolved = resolve_light_sample(other_reservoir.sample, light_sources[other_reservoir.sample.light_id >> 16u]);
    }

    // Contributions for resampling (evaluate resolved samples at canonical position)
    let canonical_contribution_canonical_sample = resolved_reservoir_contribution(canonical_reservoir, canonical_resolved, canonical_world_position, canonical_world_normal, canonical_diffuse_brdf);
    let canonical_contribution_other_sample = resolved_reservoir_contribution(other_reservoir, other_resolved, canonical_world_position, canonical_world_normal, canonical_diffuse_brdf);

    // Extra contributions for MIS (evaluate resolved samples at other position)
    let other_contribution_canonical_sample = resolved_reservoir_contribution(canonical_reservoir, canonical_resolved, other_world_position, other_world_normal, other_diffuse_brdf);
    let other_contribution_other_sample = resolved_reservoir_contribution(other_reservoir, other_resolved, other_world_position, other_world_normal, other_diffuse_brdf);

    // Resampling weight for canonical sample
    let canonical_sample_mis_weight = balance_heuristic(
        canonical_reservoir.confidence_weight * canonical_contribution_canonical_sample.target_function,
        other_reservoir.confidence_weight * other_contribution_canonical_sample.target_function,
    );
    let canonical_sample_resampling_weight = canonical_sample_mis_weight * canonical_contribution_canonical_sample.target_function * canonical_reservoir.unbiased_contribution_weight;

    // Resampling weight for other sample
    let other_sample_mis_weight = balance_heuristic(
        other_reservoir.confidence_weight * other_contribution_other_sample.target_function,
        canonical_reservoir.confidence_weight * canonical_contribution_other_sample.target_function,
    );
    let other_sample_resampling_weight = other_sample_mis_weight * canonical_contribution_other_sample.target_function * other_reservoir.unbiased_contribution_weight;

    // Perform resampling
    var combined_reservoir = empty_reservoir();
    combined_reservoir.confidence_weight = canonical_reservoir.confidence_weight + other_reservoir.confidence_weight;
    let weight_sum = canonical_sample_resampling_weight + other_sample_resampling_weight;

    if rand_f(rng) < other_sample_resampling_weight / weight_sum {
        combined_reservoir.sample = other_reservoir.sample;

        let inverse_target_function = select(0.0, 1.0 / canonical_contribution_other_sample.target_function, canonical_contribution_other_sample.target_function > 0.0);
        combined_reservoir.unbiased_contribution_weight = weight_sum * inverse_target_function;

        return ReservoirMergeResult(combined_reservoir, canonical_contribution_other_sample.radiance, canonical_contribution_other_sample.wi, other_resolved.world_position);
    } else {
        combined_reservoir.sample = canonical_reservoir.sample;

        let inverse_target_function = select(0.0, 1.0 / canonical_contribution_canonical_sample.target_function, canonical_contribution_canonical_sample.target_function > 0.0);
        combined_reservoir.unbiased_contribution_weight = weight_sum * inverse_target_function;

        return ReservoirMergeResult(combined_reservoir, canonical_contribution_canonical_sample.radiance, canonical_contribution_canonical_sample.wi, canonical_resolved.world_position);
    }
}

struct ReservoirContribution {
    radiance: vec3<f32>,
    target_function: f32,
    wi: vec3<f32>,
}

fn resolved_reservoir_contribution(reservoir: Reservoir, resolved: ResolvedLightSample, world_position: vec3<f32>, world_normal: vec3<f32>, diffuse_brdf: vec3<f32>) -> ReservoirContribution {
    if !reservoir_valid(reservoir) { return ReservoirContribution(vec3(0.0), 0.0, vec3(0.0)); }
    let light_contribution = calculate_resolved_light_contribution(resolved, world_position, world_normal);
    let target_function = luminance(light_contribution.radiance * diffuse_brdf * saturate(dot(light_contribution.wi, world_normal)));
    return ReservoirContribution(light_contribution.radiance, target_function, light_contribution.wi);
}
