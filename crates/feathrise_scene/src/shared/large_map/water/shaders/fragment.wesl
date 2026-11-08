//! 正式动态水面的透明StandardMaterial PBR片元入口。
//! 【功能设计】旧GLB水面与水平静水完整保留作者颜色、纹理、透明度和PBR；uniform标记的整网格水使用UV0，
//! 旧样条河流继续使用UV0/UV1生成下游流纹。缺TANGENT时由统一片元导数恢复UV0世界坐标架，退化面则安全跳过微法线。


// 逐像素流动结果只保存标准PBR输入需要的有限标量，不创建新纹理或额外材质绑定。
import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::render::forward_io::FragmentOutput;
import bevy_pbr::render::pbr_fragment::pbr_input_from_standard_material;
import bevy_pbr::render::pbr_functions::alpha_discard;
@if(!PREPASS_PIPELINE) import bevy_pbr::render::pbr_functions::apply_pbr_lighting;
@if(!PREPASS_PIPELINE) import bevy_pbr::render::pbr_functions::main_pass_post_lighting_processing;
import bevy_pbr::render::pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT;
import feathrise_scene::shared::large_map::water::shaders::water::dynamic_water_uniform;
import feathrise_scene::shared::large_map::water::shaders::water::dynamic_water_micro_normal_a;
import feathrise_scene::shared::large_map::water::shaders::water::dynamic_water_micro_sampler_a;
import feathrise_scene::shared::large_map::water::shaders::water::dynamic_water_micro_normal_b;
import feathrise_scene::shared::large_map::water::shaders::water::dynamic_water_micro_sampler_b;
import feathrise_scene::shared::large_map::water::shaders::water::RIBBON_STYLE_SENTINEL;
import feathrise_scene::shared::large_map::water::shaders::water::water_bank_fade;
import feathrise_scene::shared::large_map::water::shaders::water::water_wrap_safe_phase_offset;
import bevy_pbr::render::mesh_view_bindings::globals;
import bevy_pbr::render::mesh_view_bindings::view;
import feathrise_environment::terrain_surface::shaders::surface_weather::surface_weather;
import feathrise_environment::terrain_surface::shaders::surface_weather::rain_shelter_exposure;
import feathrise_scene::shared::large_map::water::shaders::reflection::sample_water_reflection;
import feathrise_scene::shared::large_map::water::shaders::reflection::water_reflection_fresnel;
import feathrise_environment::terrain_surface::shaders::rain::sample_surface_rain_slope;
import feathrise_environment::terrain_surface::shaders::rain::sample_surface_wind_slope;
import feathrise_environment::terrain_surface::shaders::wet_surface::water_reflection_roughness;
@if(CLIPMAP_WATER) import feathrise_scene::maps::dynamic_map::water::shaders::clipmap_water;

struct RibbonFlowSample {
    crest: f32, // 连续浅色流纹强度，零至一。
    trough: f32, // 连续暗色流纹强度，零至一。
    foam: f32, // 高流速、急流和瀑布逐级增强的有限白沫强度。
    tangent_slope: f32, // 沿局部下游切线的微法线坡度。
    cross_slope: f32, // 沿局部横向轴的微法线坡度。
}

// 世界空间流向坐标架以UV0的+U为下游、+V为横向；valid为假时调用方必须保留标准PBR法线。
struct RibbonFlowFrame {
    tangent_u: vec3<f32>, // 沿UV0正U方向的世界空间单位切线。
    tangent_v: vec3<f32>, // 沿UV0正V方向并与法线、正U轴正交的世界空间单位切线。
    valid: bool, // UV雅可比或世界导数退化时为假，禁止归一化零向量。
}

// 返回统一的无效值，所有退化出口都不会把固定世界轴伪装成河流方向。
fn invalid_ribbon_flow_frame() -> RibbonFlowFrame {
    return RibbonFlowFrame(vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 0.0), false);
}

// 使用同一四像素导数区的世界位置与UV0雅可比恢复+U/+V坐标架；本函数内部不调用导数内建函数。
fn ribbon_flow_frame_from_derivatives(
    base_normal: vec3<f32>, // StandardMaterial完成双面与法线贴图处理后的世界法线。
    world_position_dx: vec3<f32>, // 当前四像素组内世界位置对屏幕x的导数。
    world_position_dy: vec3<f32>, // 当前四像素组内世界位置对屏幕y的导数。
    uv_dx: vec2<f32>, // UV0对屏幕x的导数。
    uv_dy: vec2<f32>, // UV0对屏幕y的导数。
) -> RibbonFlowFrame {
    let uv_determinant = uv_dx.x * uv_dy.y - uv_dy.x * uv_dx.y; // 二维雅可比行列式同时保留UV镜像方向。
    let uv_gradient_area = dot(uv_dx, uv_dx) * dot(uv_dy, uv_dy); // 相对阈值适配近景与远景不同UV脚印。
    if uv_gradient_area <= 1e-20 || uv_determinant * uv_determinant <= uv_gradient_area * 1e-8 {
        return invalid_ribbon_flow_frame(); // UV缺失、常量或近共线时不能可靠反演，直接保留原法线。
    }
    let normal_length_squared = dot(base_normal, base_normal); // 防御异常材质输入，避免normalize零法线。
    if normal_length_squared <= 1e-8 {
        return invalid_ribbon_flow_frame(); // 世界法线退化时不继续构造切线坐标架。
    }
    let normal = normalize(base_normal); // 后续Gram-Schmidt统一使用有限单位法线。
    let position_u = (
        world_position_dx * uv_dy.y - world_position_dy * uv_dx.y
    ) / uv_determinant; // 反演雅可比得到世界位置对UV0.u的偏导。
    let position_v = (
        world_position_dy * uv_dx.x - world_position_dx * uv_dy.x
    ) / uv_determinant; // 反演雅可比得到世界位置对UV0.v的偏导。
    let tangent_u_candidate = position_u - normal * dot(position_u, normal); // 消除数值误差造成的法向分量。
    if dot(tangent_u_candidate, tangent_u_candidate) <= 1e-8 {
        return invalid_ribbon_flow_frame(); // +U世界导数退化时没有可信下游方向。
    }
    let tangent_u = normalize(tangent_u_candidate); // 固定第一轴为UV0正U，保证动画方向与沿程编码一致。
    let tangent_v_candidate = position_v
        - normal * dot(position_v, normal)
        - tangent_u * dot(position_v, tangent_u); // 直接从+V偏导正交化，不猜测三角形手性。
    if dot(tangent_v_candidate, tangent_v_candidate) <= 1e-8 {
        return invalid_ribbon_flow_frame(); // +V世界导数与法线或+U共线时安全跳过微法线。
    }
    return RibbonFlowFrame(tangent_u, normalize(tangent_v_candidate), true); // 返回带UV方向符号的稳定正交世界坐标架。
}

// 与水面共享库相同的圆周常量；片元文件保持可独立检查且不依赖私有常量导出。
const FLOW_TAU: f32 = 6.283185307179586;

// 在世界XZ上采样一层滚动切线法线；参数x/yz/w依次为平铺米数、滚动速度和有效强度。
fn sample_water_micro_normal(
    normal_texture: texture_2d<f32>,
    normal_sampler: sampler,
    parameters: vec4<f32>,
    world_xz: vec2<f32>,
    elapsed: f32,
) -> vec3<f32> {
    let tile_meters = max(parameters.x, 0.01);
    let uv = fract((world_xz + parameters.yz * elapsed) / tile_meters);
    let encoded = textureSample(normal_texture, normal_sampler, uv).xyz * 2.0 - 1.0;
    let safe_z = max(encoded.z, 0.08);
    return vec3<f32>(
        clamp(encoded.x / safe_z, -2.0, 2.0) * parameters.w,
        clamp(encoded.y / safe_z, -2.0, 2.0) * parameters.w,
        1.0,
    );
}

// 使用米制沿程UV构造随整河稳定速度平移的流场；局部速度只改变湍动强度，不乘绝对时间。
fn sample_ribbon_flow(
    uv: vec2<f32>, // x为上游至下游累计米数，y为跨河零至一位置。
    uv_b: vec2<f32>, // x为局部作者流速，y为普通河、急流或瀑布样式。
    elapsed: f32, // 全局连续秒数，只参与稳定沿程平移。
    average_flow_speed: f32, // 整河平均速度，避免逐顶点速度梯度随时间压缩相位。
    bank_fade_width: f32, // 与几何波完全相同的岸边UV衰减宽度。
    along_pixel_meters: f32, // 当前片元脚印覆盖的沿程米数，用于远景带限。
    lateral_pixel_ratio: f32, // 当前片元脚印覆盖的横向UV比例，用于宽河俯视抗锯齿。
) -> RibbonFlowSample {
    let style = max(uv_b.y - RIBBON_STYLE_SENTINEL, 0.0); // 从专用高位哨兵解码普通河、急流或瀑布样式。
    let rapid_weight = clamp(style, 0.0, 1.0); // 样式一完整启用急流增强。
    let waterfall_weight = clamp(style - 1.0, 0.0, 1.0); // 样式二额外启用瀑布增强。
    let local_flow_speed = max(uv_b.x, 0.0); // 局部负速度不得反向或放大效果。
    let speed_energy = smoothstep(2.0, 10.0, local_flow_speed); // 普通河的高作者流速也会出现可辨认湍动。
    let visual_flow_speed = min(max(average_flow_speed, 0.0), 8.0); // 有限视觉速度避免常见帧率下时间频闪。
    let lateral_position = uv.y - 0.5; // 河心为零，便于两侧产生不同流线弯曲。
    let bank_visibility = water_bank_fade(uv, bank_fade_width); // 两岸恢复原材质，避免透明边界闪烁。
    let broad_lod = min(
        1.0 - smoothstep(3.0, 6.0, along_pixel_meters),
        1.0 - smoothstep(0.12, 0.30, lateral_pixel_ratio),
    ); // 十四米水团只在纵横片元脚印都能解析时保留。
    let primary_lod = min(
        1.0 - smoothstep(0.75, 2.5, along_pixel_meters),
        1.0 - smoothstep(0.04, 0.12, lateral_pixel_ratio),
    ); // 主长流线在缩成亚像素前逐步退场，远景交给大尺度水团。
    let detail_lod = min(
        1.0 - smoothstep(0.5, 1.5, along_pixel_meters),
        1.0 - smoothstep(0.025, 0.07, lateral_pixel_ratio),
    ); // 次流线和微法线使用更严格的纵横双轴带限，抑制俯视摩尔纹。
    if bank_visibility <= 0.0001 || broad_lod <= 0.0001 {
        return RibbonFlowSample(0.0, 0.0, 0.0, 0.0, 0.0); // 岸边或远到无法解析时跳过全部三角函数并完整恢复原PBR水面。
    }

    let primary_warp_frequency = FLOW_TAU / 17.0;
    let detail_warp_frequency = FLOW_TAU / 7.5;
    let primary_warp = sin(
        uv.x * primary_warp_frequency
            - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * primary_warp_frequency)
            + lateral_position * 8.0,
    ) * 0.72; // 十七米主扰动让流线持续弯曲，并在Bevy每小时回绕时保持同相。
    let detail_warp = sin(
        uv.x * detail_warp_frequency
            - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * 0.72 * detail_warp_frequency)
            - lateral_position * 14.0,
    ) * 0.28 * detail_lod; // 七点五米次扰动只在近中景打破重复。
    let flow_warp = primary_warp + detail_warp; // 两个有界正弦组成无纹理连续域扰动。

    let primary_lane_base = max(
        1.0 - abs(sin(lateral_position * 12.0 + flow_warp)),
        0.0,
    ); // 先取得零至一第一组流线脊值，后续固定乘法替代通用pow。
    let primary_lane = primary_lane_base * primary_lane_base * primary_lod; // 平方形成俯视仍可见的宽长流线，并按双轴脚印退场。
    let secondary_lane_base = max(
        1.0 - abs(sin(lateral_position * 24.0 - flow_warp * 1.35 + 1.7)),
        0.0,
    ); // 取得第二组更细流线脊值。
    let secondary_lane_squared = secondary_lane_base * secondary_lane_base; // 第一次平方收窄次流线。
    let secondary_lane = secondary_lane_squared * secondary_lane_squared * detail_lod; // 固定四次乘法只在近中景保留第二组流线。
    let primary_pulse = smoothstep(
        0.18,
        0.88,
        0.5 + 0.5 * sin(
            uv.x * FLOW_TAU / 12.0
                - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * FLOW_TAU / 12.0)
                + lateral_position * 2.3,
        ),
    ); // 十二米纵向包络把第一组流线切成连续长条而非无限平行线。
    let secondary_pulse = smoothstep(
        0.28,
        0.92,
        0.5 + 0.5 * sin(
            uv.x * FLOW_TAU / 6.5
                - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * 0.72 * FLOW_TAU / 6.5)
                - lateral_position * 5.1,
        ),
    ); // 六点五米包络为细流线提供错开的局部湍动。
    let streak = clamp(
        max(primary_lane * primary_pulse, secondary_lane * secondary_pulse * 0.82),
        0.0,
        1.0,
    ); // 两组长短流线取最大值，避免简单相加把河面整体抬亮。
    let body_wave = 0.5 + 0.5 * sin(
        uv.x * FLOW_TAU / 14.0
            - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * FLOW_TAU / 14.0)
            - lateral_position * 4.0
            + flow_warp * 0.48,
    ); // 大尺度明暗水团保证顶视和弱反射环境仍能读出移动方向。

    let flow_visibility = clamp(
        bank_visibility
            * broad_lod
            * (0.72 + speed_energy * 0.24 + rapid_weight * 0.18 + waterfall_weight * 0.24),
        0.0,
        1.0,
    ); // 普通河已有明确基线，高速、急流和瀑布再逐级增强而不越界。
    let crest = clamp(
        (body_wave * 0.26 + streak * 0.96) * flow_visibility,
        0.0,
        1.0,
    ); // 浅色长流线叠加少量大尺度水团，形成连续下游参照。
    let trough = clamp(
        (1.0 - body_wave) * (0.55 + (1.0 - streak) * 0.45) * flow_visibility,
        0.0,
        1.0,
    ); // 暗色水团在无高光时承担主要运动辨识，不依赖环境反射。

    let foam_seed = smoothstep(
        0.58 - speed_energy * 0.14 - rapid_weight * 0.10 - waterfall_weight * 0.08,
        0.96 - speed_energy * 0.08 - rapid_weight * 0.10 - waterfall_weight * 0.08,
        streak * 0.82 + body_wave * 0.24,
    ); // 高速普通河扩大稀疏破碎区，急流和瀑布再增加泡沫密度而不整片刷白。
    let foam_strength = clamp(
        0.025 + speed_energy * 0.22 + rapid_weight * 0.45 + waterfall_weight * 0.25,
        0.0,
        0.90,
    ); // A地图高速普通河已有可见稀疏白沫，急流和瀑布仍保持清晰层级。
    let foam = foam_seed * foam_strength * bank_visibility * detail_lod; // 泡沫同样在岸边和远景平滑归零。

    let normal_phase_a = uv.x * FLOW_TAU / 4.5
        - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * FLOW_TAU / 4.5)
        + lateral_position * 23.0
        + flow_warp; // 第一组片元法线沿同一平移流场连续传播。
    let normal_phase_b = uv.x * FLOW_TAU / 2.8
        - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * 1.12 * FLOW_TAU / 2.8)
        - lateral_position * 35.0
        - flow_warp * 1.2; // 第二组斜向片元法线打破单一镜面波纹。
    let normal_strength = bank_visibility
        * detail_lod
        * (0.045 + speed_energy * 0.030 + rapid_weight * 0.040 + waterfall_weight * 0.055); // 法线强度随水势有限增加。
    let tangent_slope = (
        cos(normal_phase_a) * 0.62 + cos(normal_phase_b) * 0.38
    ) * normal_strength; // 沿下游方向组合两层微法线坡度。
    let cross_slope = (
        cos(normal_phase_a) * 0.44 - cos(normal_phase_b) * 0.56
    ) * normal_strength; // 横向坡度与纵向错相形成局部水面破碎感。

    return RibbonFlowSample(crest, trough, foam, tangent_slope, cross_slope); // 返回一次ALU采样供颜色、粗糙度和法线共同复用。
}

@if(!PREPASS_PIPELINE) @fragment
fn fragment(
    in: VertexOutput, // 继续消费Bevy StandardMaterial定义的世界位置、UV、切线和顶点色。
    @builtin(front_facing) is_front: bool, // 双面水面仍由标准PBR正确翻转受光方向。
) -> FragmentOutput {
    @if(CLIPMAP_WATER && VERTEX_UVS_A && VERTEX_UVS_B) let clipmap_coverage = clipmap_water::surface_coverage(in.uv, in.uv_b);
    @if(CLIPMAP_WATER && VERTEX_UVS_A && VERTEX_UVS_B) if clipmap_coverage <= 0.001 {
        discard; // R8、真实三角面水深或分页状态任一无效都不能显示平面水。
    }
    @if(CLIPMAP_WATER && VERTEX_UVS_A && !VERTEX_UVS_B) discard; // Clipmap正式环带必须携带UV_B中的几何Mip与片元裁切数据。
    @if(CLIPMAP_WATER && !VERTEX_UVS_A) discard; // Clipmap正式环带必须携带稳定地图UV，岸线不能从波后世界坐标反推。
    var pbr_input = pbr_input_from_standard_material(in, is_front); // 先建立作者纹理、顶点色、Alpha和完整标准PBR输入。
    @if(CLIPMAP_WATER) pbr_input.material.base_color.a *= clipmap_coverage; // R8强度和水深共同形成抗锯齿岸缘。
    let rain_pixel_meters = max(length(dpdx(in.world_position.xyz)), length(dpdy(in.world_position.xyz))); // 在任何非一致分支之前取得脚印，雨滴函数内部不调用导数。
    var rain_shore = 1.0; // 原GLB无顶点色时由实际网格轮廓裁切。
    @if(CLIPMAP_WATER) rain_shore = clipmap_coverage; // 雨水只命中当前真实可见水域，不把整张环带当水面。
    @if(VERTEX_COLORS) rain_shore *= clamp(in.color.a, 0.0, 1.0); // Clipmap同时保留R8/水深裁切；GLB初值为1，仍只使用实际岸带透明度。

    // 两层世界稳定微法线与宏观几何波独立滚动；没有图片时对应uniform强度为零。
    let micro_a = sample_water_micro_normal(
        dynamic_water_micro_normal_a,
        dynamic_water_micro_sampler_a,
        dynamic_water_uniform.current.micro_normal_a,
        in.world_position.xz,
        dynamic_water_uniform.current.time_and_visual.x,
    );
    let micro_b = sample_water_micro_normal(
        dynamic_water_micro_normal_b,
        dynamic_water_micro_sampler_b,
        dynamic_water_uniform.current.micro_normal_b,
        in.world_position.xz,
        dynamic_water_uniform.current.time_and_visual.x,
    );
    let micro_slope = micro_a.xy + micro_b.xy;
    if dot(micro_slope, micro_slope) > 0.00000001 {
        let base_normal = normalize(pbr_input.N);
        let tangent_x_candidate = vec3<f32>(1.0, 0.0, 0.0)
            - base_normal * base_normal.x;
        if dot(tangent_x_candidate, tangent_x_candidate) > 0.000001 {
            let tangent_x = normalize(tangent_x_candidate);
            let tangent_z = normalize(cross(tangent_x, base_normal));
            pbr_input.N = normalize(
                base_normal + tangent_x * micro_slope.x + tangent_z * micro_slope.y,
            );
        }
    }

    @if(VERTEX_UVS_A) let world_position_dx = dpdx(in.world_position.xyz); // 所有四像素导数在动态水身份分支前统一求值，满足WGSL导数一致性约束。
    @if(VERTEX_UVS_A) let world_position_dy = dpdy(in.world_position.xyz); // 保存屏幕y方向世界位置变化，供UV0雅可比反演复用。
    @if(VERTEX_UVS_A) let uv_dx = dpdx(in.uv); // 一次取得UV0的屏幕x导数，带限与缺切线坐标架共用同一结果。
    @if(VERTEX_UVS_A) let uv_dy = dpdy(in.uv); // 一次取得UV0的屏幕y导数，避免在非统一控制流中调用导数内建函数。
    @if(VERTEX_UVS_A) let along_pixel_meters = max(abs(uv_dx.x), abs(uv_dy.x)); // 估算沿程米制脚印。
    @if(VERTEX_UVS_A) let lateral_pixel_ratio = max(abs(uv_dx.y), abs(uv_dy.y)); // 估算跨河UV脚印，宽河远景流线不会缩成闪烁亚像素。
    @if(VERTEX_UVS_A) let encoded_whole_mesh_style = dynamic_water_uniform.current.surface_settings.y - RIBBON_STYLE_SENTINEL; // 整网格不增加UV1，从原单位流向x槽解码强制样式。
    @if(VERTEX_UVS_A) let whole_mesh_mode = encoded_whole_mesh_style >= -0.5 && encoded_whole_mesh_style <= 2.5;
    @if(VERTEX_UVS_A) var ribbon_mode = whole_mesh_mode; // 整网格只要有UV0就能进入流场，不要求原GLB具有第二套UV。
    @if(VERTEX_UVS_A) var flow_uv_b = vec2<f32>(
        dynamic_water_uniform.current.surface_settings.w,
        RIBBON_STYLE_SENTINEL + max(encoded_whole_mesh_style, 0.0),
    ); // 为共用流场合成“uniform作者流速 + 整网格样式”，不写回Mesh。
    @if(VERTEX_UVS_A && VERTEX_UVS_B && !CLIPMAP_WATER) let encoded_ribbon_style = in.uv_b.y - RIBBON_STYLE_SENTINEL; // 常规第二套UV不会满足专用样条样式范围。
    @if(VERTEX_UVS_A && VERTEX_UVS_B && !CLIPMAP_WATER) let spline_ribbon_mode = encoded_ribbon_style >= -0.5 && encoded_ribbon_style <= 2.5;
    @if(VERTEX_UVS_A && VERTEX_UVS_B && !CLIPMAP_WATER) if !whole_mesh_mode && spline_ribbon_mode {
        ribbon_mode = true; // 未强制整网格时，保留旧样条UV1身份、样式和局部速度。
        flow_uv_b = in.uv_b;
    }
    @if(VERTEX_UVS_A) if ribbon_mode {
        rain_shore = rain_shore * water_bank_fade(in.uv, dynamic_water_uniform.current.surface_settings.x); // 样条与整网格延续现有同源岸线衰减。
        let flow = sample_ribbon_flow(
            in.uv, // 使用网格冻结的累计米和横向位置。
            flow_uv_b, // 旧样条使用UV1，整网格使用uniform合成的速度与样式。
            globals.time, // 与顶点位移共用RenderWorld共享时间，纯视觉水面无需CPU逐材质更新。
            dynamic_water_uniform.current.surface_settings.w, // 样条描述符保存整河稳定平均速度。
            dynamic_water_uniform.current.surface_settings.x, // 与几何波共用岸边固定宽度。
            along_pixel_meters, // 远景带限阻止程序流线产生摩尔纹。
            lateral_pixel_ratio, // 横向带限阻止俯视宽河出现细线摩尔纹。
        ); // 完成当前样条片元的唯一流场采样。
        let original_alpha = pbr_input.material.base_color.a; // 明暗、泡沫和法线都不得修改作者透明度。
        var flowed_color = pbr_input.material.base_color.rgb
            * (1.0 - flow.trough * 0.24); // 暗流纹提供不依赖高光的明确移动参照。
        flowed_color = flowed_color * (1.0 + flow.crest * 0.30)
            + vec3<f32>(0.012, 0.021, 0.027) * flow.crest; // 浅流纹保持原水色并只加入少量冷色散射。
        let foam_color = vec3<f32>(0.70, 0.80, 0.84); // 线性空间泡沫色避免直接使用纯白导致曝光溢出。
        flowed_color = mix(flowed_color, foam_color, clamp(flow.foam, 0.0, 0.82)); // 普通河稀疏、急流与瀑布更明显。
        pbr_input.material.base_color = vec4<f32>(flowed_color, original_alpha); // 恢复完全不变的作者Alpha并发布逐像素水色。
        pbr_input.material.perceptual_roughness = clamp(
            pbr_input.material.perceptual_roughness
                + flow.foam * 0.26
                + flow.trough * 0.035
                - flow.crest * 0.065,
            0.04,
            1.0,
        ); // 泡沫更粗糙、明亮流线略光滑，同时继续使用作者基础粗糙度。

        let base_normal = pbr_input.N; // 标准PBR已把网格或法线贴图结果转换到世界空间。
        var flow_tangent = vec3<f32>(0.0, 0.0, 0.0); // 有效网格切线优先，缺失或退化时再使用UV0导数恢复。
        var flow_cross = vec3<f32>(0.0, 0.0, 0.0); // 横向轴与下游轴同时验证成功后才参与微法线。
        var has_flow_frame = false; // 两条路径都失败时保持标准PBR法线不变。
        @if(VERTEX_TANGENTS) let tangent_candidate = in.world_tangent.xyz
            - base_normal * dot(in.world_tangent.xyz, base_normal); // 对动态顶点法线重新正交化局部下游切线。
        @if(VERTEX_TANGENTS) if dot(tangent_candidate, tangent_candidate) > 0.000001 {
            let mesh_flow_tangent = normalize(tangent_candidate); // 合法VERTEX_TANGENTS始终优先，保持现有资产的切线路径。
            let handedness = select(-1.0, 1.0, in.world_tangent.w >= 0.0); // 保留网格切线坐标系手性。
            let mesh_flow_cross = cross(base_normal, mesh_flow_tangent) * handedness; // 先保留原切线坐标系手性，再验证横轴长度。
            if dot(mesh_flow_cross, mesh_flow_cross) > 0.000001 {
                flow_tangent = mesh_flow_tangent; // 发布已正交化的网格+U轴。
                flow_cross = normalize(mesh_flow_cross); // 发布与原切线手性一致的网格横轴。
                has_flow_frame = true; // 阻止合法网格切线被导数回退覆盖。
            }
        }
        if !has_flow_frame {
            let derivative_frame = ribbon_flow_frame_from_derivatives(
                base_normal,
                world_position_dx,
                world_position_dy,
                uv_dx,
                uv_dy,
            ); // 无TANGENT或网格切线退化时，从已统一求值的世界位置/UV0导数恢复真实方向。
            if derivative_frame.valid {
                flow_tangent = derivative_frame.tangent_u; // UV0正U继续代表下游累计米方向。
                flow_cross = derivative_frame.tangent_v; // UV0正V直接代表跨河方向，不猜固定世界轴。
                has_flow_frame = true; // 只有完整雅可比与两轴都有效时才启用方向性微法线。
            }
        }
        if has_flow_frame {
            pbr_input.N = normalize(
                base_normal
                    - flow_tangent * flow.tangent_slope
                    - flow_cross * flow.cross_slope,
            ); // 只扰动受光法线，不改变几何、碰撞、岸线或透明排序；退化时原法线保持不变。
        }
    }

    let rain_upward = smoothstep(0.45, 0.85, in.world_normal.y); // 垂直瀑布与水面背面不生成水平圆环。
    let rain_amount = clamp(surface_weather.weather.z, 0.0, 1.0) * rain_shore * rain_upward * rain_shelter_exposure(in.world_position.xyz); // 无碰撞桥面代理同样阻止桥下水面受雨。
    let rain_distance = distance(in.world_position.xyz, view.world_position); // 每个视图使用自身相机，不绑定玩法主相机位置。
    let rain_slope = sample_surface_rain_slope(in.world_position.xz, surface_weather.weather.w, rain_amount, surface_weather.wind_quality.w, rain_distance, rain_pixel_meters, surface_weather.options.zw); // 雨纹距离/密度来自统一质量表，仍只有九候选。
    let wind_slope = sample_surface_wind_slope(in.world_position.xz, surface_weather.weather.w, surface_weather.wind_quality.xy, surface_weather.wind_quality.z * rain_amount, surface_weather.wind_quality.w, rain_distance, rain_pixel_meters); // 地表与河湖共用分档风纹，不改变河流位移和玩家波源。
    let weather_slope = rain_slope + wind_slope; // 雨滴、冲击冠纹与定向风纹只合并一次。
    pbr_input.N = normalize(pbr_input.N + vec3<f32>(weather_slope.x, 0.0, weather_slope.y)); // 保留既有流纹及玩家大波，最后统一归一化防止增亮溢出。
    pbr_input.material.perceptual_roughness = water_reflection_roughness(pbr_input.material.perceptual_roughness, rain_amount, surface_weather.wind_quality.z, rain_pixel_meters); // 雨天展宽环境镜面并抑制远景闪点，晴天精确保留作者与流纹结果。

    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color,
    ); // 继续执行StandardMaterial的Mask与透明裁切契约。
    var out: FragmentOutput; // 前向透明水面继续使用Bevy标准输出结构。
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        let reflection = sample_water_reflection(in.world_position.xyz, pbr_input.N, pbr_input.material.perceptual_roughness, surface_weather.wind_quality.w); // 波浪、微法线和雨纹处理后再追踪真实倒影。
        let original_specular_occlusion = pbr_input.specular_occlusion; // 保存作者/标准光照的镜面可见度。
        pbr_input.specular_occlusion *= 1.0 - reflection.confidence; // 只扣除将由场景命中替换的天空环境镜面，保留直接光。
        out.color = apply_pbr_lighting(pbr_input); // 受光水面保留直接光、环境光、阴影和反射贡献。
        let reflection_view = view.world_position - in.world_position.xyz; // 当前绘制视图，不绑定固定玩法相机。
        if reflection.confidence > 0.0001 && dot(reflection_view, reflection_view) > 0.000001 { // 无命中时精确保留原光照与Alpha。
            let fresnel = water_reflection_fresnel(pbr_input.N, reflection_view, pbr_input.material.reflectance, pbr_input.material.base_color.rgb, pbr_input.material.metallic); // 沿用作者PBR反射率控制能量，避免切换命中时覆盖预设反射强度。
            out.color = vec4<f32>(out.color.rgb + reflection.radiance * fresnel * reflection.confidence * original_specular_occlusion, out.color.a); // 场景HDR已经曝光，不再乘曝光；透明度继续由作者决定。
        }
    } else {
        out.color = pbr_input.material.base_color; // 若作者明确使用Unlit，仍保留标准无光照语义。
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color); // 保留雾、透明预乘、曝光、色调映射和去色带处理。
    return out; // 返回与原StandardMaterial透明排序和混合完全兼容的最终片元。
}
