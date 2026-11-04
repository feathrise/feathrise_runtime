//! 基于Bevy Solari 0.19.1语义扩展GBuffer解析，为动态低模高度场惰性重建真实三角guide与虚拟平滑面起点。
//! 【功能设计】A/B槽完整保留标准平滑PBR，使大小地图和RR guide沿用正式语义；G槽低十九位保存同序三角编号且RGB9E5指数恒零。
//! 真正发射主射线时才按编号读取三角，并按Ray Tracing Gems II第4章抬升虚拟起点；DI同次解析复用几何法线判断相机侧。
//! 第四轮逐面终止线能量因子已因实机产生棱角且不改善黑边而撤回，禁止几何法线再次进入项目Solari BRDF。
//! 第八轮连续视线法线和第九轮几何光源侧门均被实机排除；第十轮实机确认恢复第七轮相机侧基线并隔离主表面镜面可保持平滑且消除黄边。
//! https://doi.org/10.1007/978-1-4842-7185-8_4

#define_import_path feathrise_raytracing::gbuffer_shadow_terminator

#import bevy_pbr::pbr_deferred_types::{unpack_24bit_normal, unpack_flags}
#import bevy_pbr::rgb9e5::rgb9e5_to_vec3_
#import bevy_pbr::utils::octahedral_decode
#import bevy_render::view::{View, depth_ndc_to_view_z}
#import bevy_solari::scene_bindings::{geometry_ids, transforms, load_vertices, transform_positions, RAY_T_MIN, ResolvedMaterial}

const DEFERRED_FLAGS_DYNAMIC_TERRAIN_BIT: u32 = 1u << 3u; // 与地表Deferred输出严格共用Bevy 0.19.1未占用的第四个flag位。
const DEFERRED_FLAGS_CLIPMAP_TERRAIN_BIT: u32 = 1u << 4u; // Clipmap 必须把真实光栅 X/Z 投到固定代理表面。
const DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK: u32 = 0x7FFFFu; // G槽低十九位完整覆盖固定三角预算，高十三位必须保持零。

struct ResolvedGPixel {
    world_position: vec3<f32>, // 深度重建的真实光栅表面位置，继续服务BRDF与历史。
    world_normal: vec3<f32>, // 全部材质都从标准A槽读取平滑/草土微法线。
    material: ResolvedMaterial, // 普通材质字段保持Solari 0.19.1原布局。
    primitive_index: u32, // 动态地形从G槽低十九位恢复同序TLAS三角编号。
    dynamic_terrain: u32, // 精确零或一，避免普通材质误入三角缓冲读取路径。
    clipmap_terrain: u32, // GLB 保留原有三维投影，只有粗高度代理改用 X/Z 重心。
    instance_index: u32, // G槽19..22位路由到当前整批TLAS中的真实高度实例。
}

struct ResolvedPrimaryRaySurface { // 一次三角读取同时服务射线起点与DI相机侧判断。
    ray_origin: vec3<f32>, // 普通材质为官方法线偏移，动态地形为虚拟平滑面外侧起点。
    view_side_normal: vec3<f32>, // 普通材质为平滑法线，动态地形为真实三角几何法线。
}

fn gpixel_resolve(gpixel: vec4<u32>, depth: f32, pixel_id: vec2<u32>, view_size: vec2<f32>, world_from_clip: mat4x4<f32>) -> ResolvedGPixel {
    let world_position = reconstruct_world_position(pixel_id, depth, view_size, world_from_clip); // 保留官方深度反投影。
    let world_normal = octahedral_decode(unpack_24bit_normal(gpixel.a)); // 大小地图、RR guide与项目Solari统一读取标准平滑法线。
    let deferred_flags = unpack_flags(gpixel.a); // 高八位独立读取项目动态地形标记。
    let dynamic_terrain = u32((deferred_flags & DEFERRED_FLAGS_DYNAMIC_TERRAIN_BIT) != 0u); // 普通标准材质得到零。
    let clipmap_terrain = u32((deferred_flags & DEFERRED_FLAGS_CLIPMAP_TERRAIN_BIT) != 0u);
    let primitive_index = gpixel.g & DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK; // 只有动态地形会消费该值，普通像素不读取三角缓冲。

    let base_rough = unpack4x8unorm(gpixel.r); // 保留官方基础色与感知粗糙度解码。
    let base_color = pow(base_rough.rgb, vec3(2.2)); // 保留官方近似sRGB到线性转换。
    let perceptual_roughness = base_rough.a; // 保留作者感知粗糙度。
    let roughness = perceptual_roughness * perceptual_roughness; // 保留Solari使用的平方粗糙度。
    let props = unpack4x8unorm(gpixel.b); // 保留reflectance、metallic与其余标准属性。
    let reflectance = props.r; // 保留官方反射率通道。
    let metallic = props.g; // 保留官方金属度通道。
    var emissive = vec3(0.0); // 动态地形正式约束为非自发光，因此其G通道只表达低十九位三角身份。
    if dynamic_terrain == 0u { // 普通材质仍完整解释原RGB9E5发光数据。
        emissive = rgb9e5_to_vec3_(gpixel.g); // 不改变角色、植被或任何其他Deferred材质。
    }
    let material = ResolvedMaterial(base_color, emissive, reflectance, perceptual_roughness, roughness, metallic); // 重建官方材质结构。

    let instance_index=(gpixel.g >> 19u) & 15u;
    return ResolvedGPixel(world_position, world_normal, material, primitive_index, dynamic_terrain, clipmap_terrain, instance_index); // 普通材质不消费路由。
}

/// 把一个有效局部顶点法线变换到世界空间，并与当前三角几何半球保持一致。
fn dynamic_terrain_vertex_normal(world_from_local: mat3x3<f32>, local_normal: vec3<f32>, geometric_normal: vec3<f32>) -> vec3<f32> {
    let transformed = world_from_local * local_normal; // 与Solari 0.19.1代理命中解析保持相同变换约定。
    let length_squared = dot(transformed, transformed); // 防御损坏法线直接归一化为NaN。
    if length_squared <= 1.0e-12 { // 合法高度场不会进入，但坏资产仍使用几何半球安全回退。
        return geometric_normal; // 退化顶点不参与虚拟平滑偏移。
    }
    var normal = transformed * inverseSqrt(length_squared); // 一次倒平方根取得单位世界法线。
    if dot(normal, geometric_normal) < 0.0 { // 作者法线若翻到三角背面，不允许把射线起点推入实体。
        normal = -normal; // 只统一半球，不改变有效平滑角度。
    }
    return normal; // 返回可参与三角重心混合的顶点法线。
}

/// 为真正要发射的主射线惰性解析虚拟平滑面起点和真实相机侧；普通像素完全复用Solari官方法线。
fn primary_ray_surface(surface: ResolvedGPixel) -> ResolvedPrimaryRaySurface {
    let fallback_origin = surface.world_position + (surface.world_normal * RAY_T_MIN); // 普通材质与任何坏数据保持官方起点。
    let fallback_surface = ResolvedPrimaryRaySurface(fallback_origin, surface.world_normal); // 坏动态数据同样安全回退到标准平滑语义。
    if surface.dynamic_terrain == 0u { // 绝大多数像素不读取TLAS代理顶点缓冲。
        return fallback_surface; // 全部非动态地形路径逐值不变。
    }

    if surface.instance_index >= arrayLength(&geometry_ids) || surface.instance_index >= arrayLength(&transforms) {return fallback_surface;}
    let instance_geometry = geometry_ids[surface.instance_index]; // 洞穴加入TLAS后不能借用其他实例的三角。
    if surface.primitive_index >= instance_geometry.triangle_count { // 防御光栅与代理身份意外漂移。
        return fallback_surface; // 越界时禁止访问绑定数组并退回原始安全偏移。
    }
    let vertices = load_vertices(instance_geometry, surface.primitive_index); // 只在主射线发射点读取当前三角三个压缩顶点。
    let transform = transforms[surface.instance_index];
    let world_vertices = transform_positions(transform, vertices); // 得到与深度重建位置同坐标系的三个角点。
    let edge_01 = world_vertices[1] - world_vertices[0]; // 三角第一条世界边。
    let edge_02 = world_vertices[2] - world_vertices[0]; // 三角第二条世界边。
    let geometric_cross = cross(edge_01, edge_02); // 叉积同时提供面法线与退化面积证据。
    let geometric_length_squared = dot(geometric_cross, geometric_cross); // 避免零面积三角归一化。
    if geometric_length_squared <= 1.0e-12 { // 准入校验本应拒绝退化索引，仍保留GPU侧边界。
        return fallback_surface; // 不让单个坏三角污染整帧光照。
    }
    var geometric_normal = geometric_cross * inverseSqrt(geometric_length_squared); // 恢复真实TLAS三角面法线。
    if dot(geometric_normal, surface.world_normal) < 0.0 { // 索引绕序与屏幕正面方向可能相反。
        geometric_normal = -geometric_normal; // 选择与平滑GBuffer法线一致的外侧半球。
    }

    var projection_01 = edge_01;
    var projection_02 = edge_02;
    var point_edge = surface.world_position - world_vertices[0]; // GLB 默认仍用三维面内投影。
    if surface.clipmap_terrain != 0u {
        projection_01.y = 0.0;
        projection_02.y = 0.0;
        point_edge.y = 0.0; // 光栅 Y 可能高于或低于粗代理，三维投影会跳到错误重心。
    }
    let edge_00 = dot(projection_01, projection_01);
    let edge_01_dot_02 = dot(projection_01, projection_02);
    let edge_22 = dot(projection_02, projection_02);
    let point_dot_01 = dot(point_edge, projection_01);
    let point_dot_02 = dot(point_edge, projection_02);
    let barycentric_denominator = edge_00 * edge_22 - edge_01_dot_02 * edge_01_dot_02; // 非退化三角的正分母。
    if abs(barycentric_denominator) <= 1.0e-12 { // 极端世界尺度下继续防御数值退化。
        return fallback_surface; // 无可靠重心坐标时不猜测虚拟表面。
    }
    let barycentric_y = (edge_22 * point_dot_01 - edge_01_dot_02 * point_dot_02) / barycentric_denominator; // 第二角权重。
    let barycentric_z = (edge_00 * point_dot_02 - edge_01_dot_02 * point_dot_01) / barycentric_denominator; // 第三角权重。
    var barycentrics = max(vec3(1.0 - barycentric_y - barycentric_z, barycentric_y, barycentric_z), vec3(0.0)); // 钳制深度量化造成的微小越界。
    let barycentric_sum = dot(barycentrics, vec3(1.0)); // 重新归一化以恢复严格凸组合。
    if barycentric_sum <= 1.0e-6 { // 合法片元不会出现全零权重。
        return fallback_surface; // 防止除零并保留官方路径。
    }
    barycentrics /= barycentric_sum; // 得到稳定的当前片元三角权重。
    var proxy_position = surface.world_position;
    if surface.clipmap_terrain != 0u {
        proxy_position = barycentrics.x * world_vertices[0] + barycentrics.y * world_vertices[1] + barycentrics.z * world_vertices[2];
    } // 只把射线起点放到代理，BRDF/RR/history 的真实光栅位置保持不变。

    let world_from_local = mat3x3<f32>(transform[0].xyz, transform[1].xyz, transform[2].xyz); // 与Solari代理命中法线使用同一线性变换。
    let normal_0 = dynamic_terrain_vertex_normal(world_from_local, vertices[0].normal, geometric_normal); // 首角平滑法线。
    let normal_1 = dynamic_terrain_vertex_normal(world_from_local, vertices[1].normal, geometric_normal); // 第二角平滑法线。
    let normal_2 = dynamic_terrain_vertex_normal(world_from_local, vertices[2].normal, geometric_normal); // 第三角平滑法线。
    let corner_delta_0 = proxy_position - world_vertices[0]; // 当前代理落点相对首角的向量。
    let corner_delta_1 = proxy_position - world_vertices[1]; // 当前代理落点相对第二角的向量。
    let corner_delta_2 = proxy_position - world_vertices[2]; // 当前代理落点相对第三角的向量。
    let correction_0 = -min(0.0, dot(corner_delta_0, normal_0)) * normal_0; // 只修正落在首角法线内侧的分量。
    let correction_1 = -min(0.0, dot(corner_delta_1, normal_1)) * normal_1; // 只修正落在第二角法线内侧的分量。
    let correction_2 = -min(0.0, dot(corner_delta_2, normal_2)) * normal_2; // 只修正落在第三角法线内侧的分量。
    let smooth_surface_origin = proxy_position
        + barycentrics.x * correction_0
        + barycentrics.y * correction_1
        + barycentrics.z * correction_2; // 等价于章节伪代码，但显式消去理论上互相抵消的切平面项以降低浮点漂移。
    let ray_origin = smooth_surface_origin + (geometric_normal * RAY_T_MIN); // 虚拟平滑面解决凸面自遮挡，真实几何小偏移继续解决数值自交。
    return ResolvedPrimaryRaySurface(ray_origin, geometric_normal); // DI复用同次解析的几何法线，不重复读取三角顶点。
}

/// 仅需要起点的GI与光泽路径复用公共解析结果，不读取或复制第二套三角数据。
fn primary_ray_origin(surface: ResolvedGPixel) -> vec3<f32> { // 保留三个root既有调用接口以缩小兼容改动。
    return primary_ray_surface(surface).ray_origin; // 动态地形惰性读取一次，普通材质立即返回官方偏移。
} // 完成主射线起点兼容包装。

fn reconstruct_world_position(pixel_id: vec2<u32>, depth: f32, view_size: vec2<f32>, world_from_clip: mat4x4<f32>) -> vec3<f32> {
    let uv = (vec2<f32>(pixel_id) + 0.5) / view_size; // 保留Solari 0.19.1像素中心约定。
    let xy_ndc = (uv - vec2(0.5)) * vec2(2.0, -2.0); // 保留纹理Y轴到NDC的翻转。
    let world_pos = world_from_clip * vec4(xy_ndc, depth, 1.0); // 从当前或上一帧逆矩阵反投影。
    return world_pos.xyz / world_pos.w; // 完成齐次除法。
}

// 切平面偏差超过0.3%或法线夹角超过90度时拒绝复用，逐值保留Solari 0.19.1官方策略。
fn pixel_dissimilar(depth: f32, world_position: vec3<f32>, other_world_position: vec3<f32>, normal: vec3<f32>, other_normal: vec3<f32>, view: View) -> bool {
    // https://developer.download.nvidia.com/video/gputechconf/gtc/2020/presentations/s22699-fast-denoising-with-self-stabilizing-recurrent-blurs.pdf#page=45
    let tangent_plane_distance = abs(dot(normal, other_world_position - world_position)); // 使用平滑法线继续判断历史平面一致性。
    let view_z = -depth_ndc_to_view_z(depth, view.clip_from_view, view.view_from_clip); // 恢复正视空间距离用于相对阈值。

    return tangent_plane_distance / view_z > 0.003 || dot(normal, other_normal) < 0.0; // 保留官方拒绝条件。
}

fn permute_pixel(pixel_id: vec2<u32>, frame_index: u32, view_size: vec2<f32>) -> vec2<u32> {
    let r = frame_index; // 保留官方四乘四时序置换种子。
    let offset = vec2(r & 3u, (r >> 2u) & 3u); // 从帧号取得二维偏移。
    var shifted_pixel_id = pixel_id + offset; // 进入置换坐标系。
    shifted_pixel_id ^= vec2(3u); // 翻转低两位取得邻域置换。
    shifted_pixel_id -= offset; // 返回原坐标系。
    return min(shifted_pixel_id, vec2<u32>(view_size - 1.0)); // 钳制到有效纹理范围。
}
