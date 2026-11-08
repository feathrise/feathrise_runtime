
// 单帧固定二十个vec4；字段顺序与Rust `DynamicWaterGpuFrame` 完全一致。
struct DynamicWaterFrame {
    time_and_visual: vec4<f32>,
    ripple_shape: vec4<f32>,
    ripple_limits: vec4<f32>,
    surface_settings: vec4<f32>,
    wave_directions_amplitudes: array<vec4<f32>, 4>,
    wave_speeds: vec4<f32>,
    wave_steepness: vec4<f32>,
    micro_normal_a: vec4<f32>,
    micro_normal_b: vec4<f32>,
    ripples: array<vec4<f32>, 8>,
}

// 当前帧与历史帧固定组成四十个vec4，即640字节。
struct DynamicWaterUniform {
    current: DynamicWaterFrame,
    previous: DynamicWaterFrame,
}

// 正式水面只采用非bindless材质路径，固定使用扩展材质binding 100。
@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<uniform> dynamic_water_uniform: DynamicWaterUniform;

// 两组可选滚动切线法线由材质绑定；未配置图片时Rust把对应强度写成严格零。
@group(constants::MATERIAL_BIND_GROUP) @binding(101)
var dynamic_water_micro_normal_a: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(102)
var dynamic_water_micro_sampler_a: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(103)
var dynamic_water_micro_normal_b: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(104)
var dynamic_water_micro_sampler_b: sampler;

// 顶点位移同时返回局部法线和波谷信号，让全部渲染通道复用唯一波形。
struct DynamicWaterSample {
    local_position: vec3<f32>,
    local_normal: vec3<f32>,
    visual_signal: f32,
}

// 单个统一长期波包返回高度、解析导数和明暗信号，所有事件和通道最终统一限幅。
struct DynamicWaterRippleContribution {
    height: f32,
    height_dx: f32,
    height_dz: f32,
    visual_signal: f32,
}

// 与Rust纯逻辑使用相同的圆周常量，避免同心波相位漂移。
const WATER_TAU: f32 = 6.283185307179586;

// Bevy共享Globals每3600秒回绕；把任意作者速度量化到“每小时整数相位圈”后，回绕前后严格同相。
// 量化误差最多半圈/小时，远小于可感知流速差，同时避免为每个纯视觉水材质逐帧重传时间。
const WATER_GLOBAL_TIME_WRAP_SECONDS: f32 = 3600.0;

fn water_wrap_safe_phase_offset(elapsed: f32, angular_speed: f32) -> f32 {
    let cycles_per_wrap = round(
        angular_speed * WATER_GLOBAL_TIME_WRAP_SECONDS / WATER_TAU,
    );
    return fract(
        elapsed / WATER_GLOBAL_TIME_WRAP_SECONDS * cycles_per_wrap,
    ) * WATER_TAU; // 相位始终限制在一圈内，长时间和高速水流也不损失f32三角精度。
}

// 生成样条用UV1.y、整网格赋水用uniform流向x槽写入同一高位哨兵；常规单位流向和普通UV1都不会误命中。
const RIBBON_STYLE_SENTINEL: f32 = 16.0;

// 将零到一权重转换为两端无折角的三次平滑曲线。
fn water_smoothstep_unit(value: f32) -> f32 {
    let clamped = clamp(value, 0.0, 1.0);
    return clamped * clamped * (3.0 - 2.0 * clamped);
}

// 横向UV的V=0与V=1对应两岸埋边，只在材质指定岸带内恢复波幅。
fn water_bank_fade(uv: vec2<f32>, fade_width: f32) -> f32 {
    let nearest_bank_uv_distance = max(min(uv.y, 1.0 - uv.y), 0.0);
    return water_smoothstep_unit(nearest_bank_uv_distance / fade_width);
}

// 构造一个严格为零的波包贡献，供未激活、过期和空间范围外的统一快返使用。
fn empty_player_ripple_contribution() -> DynamicWaterRippleContribution {
    return DynamicWaterRippleContribution(0.0, 0.0, 0.0, 0.0);
}

// 采样一个有限径向波包；入水、脚步、起跳与落水八槽共用唯一波形和解析导数实现。
fn sample_player_ripple(
    rest_local_position: vec3<f32>,
    center_local_xz: vec2<f32>,
    age: f32,
    amplitude: f32,
    lifetime: f32,
    speed: f32,
    band_half_width: f32,
    wavelength: f32,
    attack_duration: f32,
    visual_reference_amplitude: f32,
) -> DynamicWaterRippleContribution {
    if age < 0.0 || age >= lifetime {
        return empty_player_ripple_contribution();
    }

    let center_delta = rest_local_position.xz - center_local_xz;
    let distance = length(center_delta);
    let front_radius = age * speed;
    let signed_distance_from_front = distance - front_radius;
    if abs(signed_distance_from_front) > band_half_width {
        return empty_player_ripple_contribution();
    }

    let band_ratio = clamp(abs(signed_distance_from_front) / band_half_width, 0.0, 1.0);
    let band_fade = 1.0 - water_smoothstep_unit(band_ratio);
    let radial_phase = signed_distance_from_front * WATER_TAU / wavelength;
    let radial_cos = cos(radial_phase);
    let attack_fade = water_smoothstep_unit(age / attack_duration);
    let lifetime_fade = clamp(1.0 - age / lifetime, 0.0, 1.0);
    let temporal_fade = attack_fade * lifetime_fade;
    let animated_wave = radial_cos * band_fade * temporal_fade;
    let height = animated_wave * amplitude;
    let visual_signal = animated_wave * (amplitude / visual_reference_amplitude);
    var height_dx = 0.0;
    var height_dz = 0.0;

    // 解析导数与同一个有限波包严格共用空间包络，避免额外网格法线重算。
    if distance > 0.000001 {
        let smoothstep_derivative = 6.0 * band_ratio * (1.0 - band_ratio);
        let signed_direction = select(-1.0, 1.0, signed_distance_from_front >= 0.0);
        let band_derivative = -smoothstep_derivative * signed_direction / band_half_width;
        let radial_derivative = (
            -sin(radial_phase) * WATER_TAU / wavelength * band_fade +
            radial_cos * band_derivative
        ) * temporal_fade * amplitude;
        let planar_direction = center_delta / distance;
        height_dx = radial_derivative * planar_direction.x;
        height_dz = radial_derivative * planar_direction.y;
    }
    return DynamicWaterRippleContribution(height, height_dx, height_dz, visual_signal);
}

// 对指定帧参数计算局部波高、解析导数与波谷乘色信号。
fn sample_dynamic_water(
    previous_frame: bool,
    elapsed: f32,
    rest_local_position: vec3<f32>,
    rest_local_normal: vec3<f32>,
    rest_local_tangent: vec3<f32>,
    rest_uv: vec2<f32>,
    rest_uv_b: vec2<f32>,
    rest_vertex_alpha: f32,
) -> DynamicWaterSample {
    // Naga不允许把uniform指针传进函数，也不能把带Offset布局的整帧结构复制到函数变量；
    // 这里只选择实际会参与计算的vec4字段，保证DX12与Vulkan使用同一合法布局。
    let time_and_visual = select(
        dynamic_water_uniform.current.time_and_visual,
        dynamic_water_uniform.previous.time_and_visual,
        previous_frame,
    );
    let ripple_shape = select(
        dynamic_water_uniform.current.ripple_shape,
        dynamic_water_uniform.previous.ripple_shape,
        previous_frame,
    );
    let ripple_limits = select(
        dynamic_water_uniform.current.ripple_limits,
        dynamic_water_uniform.previous.ripple_limits,
        previous_frame,
    );
    let surface_settings = select(
        dynamic_water_uniform.current.surface_settings,
        dynamic_water_uniform.previous.surface_settings,
        previous_frame,
    );
    let wave_speeds = select(
        dynamic_water_uniform.current.wave_speeds,
        dynamic_water_uniform.previous.wave_speeds,
        previous_frame,
    );
    let wave_steepness = select(
        dynamic_water_uniform.current.wave_steepness,
        dynamic_water_uniform.previous.wave_steepness,
        previous_frame,
    );
    let bank_fade = water_bank_fade(rest_uv, surface_settings.x);

    // 整网格水在原本只接受单位流向的x槽写入`16 + style`，使Shader直接把原UV0解释为沿程米和横向零到一。
    let encoded_whole_mesh_style = surface_settings.y - RIBBON_STYLE_SENTINEL;
    let whole_mesh_mode = encoded_whole_mesh_style >= -0.5 && encoded_whole_mesh_style <= 2.5;
    // 普通湖面仍使用每层统一流向；零向量按旧版东西向安全回退。
    var flow_direction = surface_settings.yz;
    if whole_mesh_mode || dot(flow_direction, flow_direction) <= 0.000001 {
        flow_direction = vec2<f32>(1.0, 0.0);
    } else {
        flow_direction = normalize(flow_direction);
    }
    let cross_direction = vec2<f32>(-flow_direction.y, flow_direction.x);
    let encoded_ribbon_style = rest_uv_b.y - RIBBON_STYLE_SENTINEL; // 先从专用UV1.y解码样条样式。
    let spline_ribbon_mode = encoded_ribbon_style >= -0.5 && encoded_ribbon_style <= 2.5; // 只接受哨兵附近的河流、急流和瀑布三种样式。
    let ribbon_mode = whole_mesh_mode || spline_ribbon_mode; // 整网格和旧样条共用后续稳定沿程流场。
    let spline_ribbon_style = select(0.0, max(encoded_ribbon_style, 0.0), spline_ribbon_mode);
    let ribbon_style = select(spline_ribbon_style, max(encoded_whole_mesh_style, 0.0), whole_mesh_mode); // uniform哨兵优先，不读可能存在的普通UV1。
    let rapid_weight = clamp(ribbon_style, 0.0, 1.0);
    let waterfall_weight = clamp(ribbon_style - 1.0, 0.0, 1.0);
    let surface_flow_speed = max(surface_settings.w, 0.0); // 湖面保存统一作者速度，样条描述则保存沿程截面的稳定平均速度。
    let local_flow_speed = select(surface_flow_speed, max(rest_uv_b.x, 0.0), spline_ribbon_mode && !whole_mesh_mode); // 只有旧样条从UV1.x读局部速度，整网格始终使用uniform作者速度。
    let visual_flow_speed = min(surface_flow_speed, 8.0); // 八米每秒上限避免极端作者值在常见帧率下退化成时间频闪。
    var primary_phase = dot(rest_local_position.xz, flow_direction * 0.105 + cross_direction * 0.035)
        - water_wrap_safe_phase_offset(elapsed, 1.10 * local_flow_speed);
    var crossing_phase = dot(rest_local_position.xz, flow_direction * 0.045 - cross_direction * 0.140)
        + water_wrap_safe_phase_offset(elapsed, 0.75 * local_flow_speed);
    var primary_gradient = vec3<f32>(flow_direction.x * 0.105 + cross_direction.x * 0.035, 0.0, flow_direction.y * 0.105 + cross_direction.y * 0.035);
    var crossing_gradient = vec3<f32>(flow_direction.x * 0.045 - cross_direction.x * 0.140, 0.0, flow_direction.y * 0.045 - cross_direction.y * 0.140);
    var base_normal = rest_local_normal;
    if dot(base_normal, base_normal) <= 0.000001 {
        base_normal = vec3<f32>(0.0, 1.0, 0.0);
    } else {
        base_normal = normalize(base_normal);
    }
    let has_valid_flow_tangent = dot(rest_local_tangent, rest_local_tangent) > 0.000001; // 缺TANGENT时不得把固定+X误当作整网格真实下游方向。
    var flow_tangent = rest_local_tangent;
    if !has_valid_flow_tangent {
        flow_tangent = vec3<f32>(flow_direction.x, 0.0, flow_direction.y);
    } else {
        flow_tangent = normalize(flow_tangent);
    }
    var cross_tangent = cross(base_normal, flow_tangent);
    if dot(cross_tangent, cross_tangent) <= 0.000001 {
        cross_tangent = vec3<f32>(cross_direction.x, 0.0, cross_direction.y);
    } else {
        cross_tangent = normalize(cross_tangent);
    }
    if ribbon_mode {
        let longitudinal_frequency = 0.105 + rapid_weight * 0.055 + waterfall_weight * 0.080;
        let crossing_frequency = 3.0 + rapid_weight * 2.0 + waterfall_weight * 3.0;
        let crossing_longitudinal_frequency = 0.045 + rapid_weight * 0.035 + waterfall_weight * 0.060;
        primary_phase = rest_uv.x * longitudinal_frequency
            - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * longitudinal_frequency); // 主波按近似原速度移动，且Globals回绕前后同相。
        crossing_phase = rest_uv.x * crossing_longitudinal_frequency
            - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * crossing_longitudinal_frequency)
            - (rest_uv.y - 0.5) * crossing_frequency; // 斜向次波独立量化自己的相位速度，避免多波长共用距离造成跳变。
        if has_valid_flow_tangent {
            primary_gradient = flow_tangent * longitudinal_frequency; // 合法网格切线继续提供顶点阶段的沿+U解析法线梯度。
            crossing_gradient = flow_tangent * crossing_longitudinal_frequency - cross_tangent * crossing_frequency * 0.12; // 横向梯度沿合法切线坐标架计算。
        } else {
            primary_gradient = vec3<f32>(0.0, 0.0, 0.0); // 无切线仍按UV0计算相位和波高位移，但不制造固定世界方向的假法线。
            crossing_gradient = vec3<f32>(0.0, 0.0, 0.0); // 主画面会在统一片元导数区尝试从世界位置与UV0恢复真实+V轴。
        }
    }
    let painted_shore_fade = pow(
        clamp(rest_vertex_alpha, 0.0, 1.0),
        max(time_and_visual.z, 0.1),
    ); // R8静水直接复用实际岸带顶点alpha，透明边缘和波幅不会形成两套轮廓。
    let edge_fade = select(painted_shore_fade, bank_fade, ribbon_mode);
    var background_height = 0.0;
    var horizontal_displacement = vec2<f32>(0.0, 0.0);
    var height_gradient = vec3<f32>(0.0, 0.0, 0.0);
    if ribbon_mode {
        let primary_wave = select(
            dynamic_water_uniform.current.wave_directions_amplitudes[0],
            dynamic_water_uniform.previous.wave_directions_amplitudes[0],
            previous_frame,
        );
        let crossing_wave = select(
            dynamic_water_uniform.current.wave_directions_amplitudes[1],
            dynamic_water_uniform.previous.wave_directions_amplitudes[1],
            previous_frame,
        );
        let primary_amplitude = primary_wave.z * (1.0 + rapid_weight * 0.70 + waterfall_weight * 1.20);
        let crossing_amplitude = crossing_wave.z * (1.0 + rapid_weight * 0.80 + waterfall_weight * 1.40);
        background_height = (
            sin(primary_phase) * primary_amplitude +
            sin(crossing_phase) * crossing_amplitude
        ) * edge_fade;
        height_gradient = (
            primary_gradient * cos(primary_phase) * primary_amplitude +
            crossing_gradient * cos(crossing_phase) * crossing_amplitude
        ) * edge_fade;
    } else {
        // 静水固定遍历四组数据驱动风浪；水流为零也不会停止这些独立相位。
        for (var wave_index = 0u; wave_index < 4u; wave_index += 1u) {
            let wave = select(
                dynamic_water_uniform.current.wave_directions_amplitudes[wave_index],
                dynamic_water_uniform.previous.wave_directions_amplitudes[wave_index],
                previous_frame,
            );
            let phase = dot(rest_local_position.xz, wave.xy) * wave.w
                - water_wrap_safe_phase_offset(
                    elapsed,
                    wave_speeds[wave_index] * wave.w,
                );
            let sine_phase = sin(phase);
            let cosine_phase = cos(phase);
            background_height += sine_phase * wave.z * edge_fade;
            height_gradient += vec3<f32>(wave.x, 0.0, wave.y)
                * (cosine_phase * wave.z * wave.w * edge_fade);
            horizontal_displacement += wave.xy
                * (cosine_phase * wave.z * wave_steepness[wave_index] * edge_fade);
        }
    }

    // 顶点阶段只保留低频解析动态法线；无需反射的可见流纹由片元阶段按同一米制平移独立重建。
    if ribbon_mode && has_valid_flow_tangent {
        let lateral_position = rest_uv.y - 0.5; // 横向中心为零，给两组流纹加入相反斜率以打破整齐横条。
        let broad_frequency = WATER_TAU / 12.0;
        let detail_frequency = WATER_TAU / 8.0;
        let broad_flow_phase = rest_uv.x * broad_frequency
            - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * broad_frequency)
            - lateral_position * 1.6; // 十二米主流纹在两米纵向采样下约有六点每周期。
        let detail_flow_phase = rest_uv.x * detail_frequency
            - water_wrap_safe_phase_offset(elapsed, visual_flow_speed * detail_frequency)
            + lateral_position * 4.2; // 八米次流纹独立保证每小时边界连续。
        let normal_style_scale = 1.0 + rapid_weight * 0.65 + waterfall_weight * 1.10; // 急流和瀑布使用更明显的沿程动态法线。
        let broad_normal_gradient = cos(broad_flow_phase) * WATER_TAU / 12.0 * 0.026; // 用二点六厘米虚拟波高生成可见但不改轮廓的主法线坡度。
        let detail_normal_gradient = cos(detail_flow_phase) * WATER_TAU / 8.0 * 0.012; // 一点二厘米次波只增加细节，不制造额外几何抖动。
        height_gradient += flow_tangent
            * (broad_normal_gradient * 0.68 + detail_normal_gradient * 0.32)
            * normal_style_scale
            * edge_fade; // 只改变解析法线并保持两岸固定，让普通受光下也能观察流动方向。
    }

    // 固定遍历八槽统一玩家波源；有效计数之外的零槽不参与波形。
    var player_height = 0.0;
    var player_height_dx = 0.0;
    var player_height_dz = 0.0;
    var visual_signal = 0.0;
    let active_ripple_count = u32(clamp(ripple_limits.z, 0.0, 8.0));
    for (var ripple_index = 0u; ripple_index < 8u; ripple_index += 1u) {
        if ripple_index >= active_ripple_count {
            continue;
        }
        let ripple = select(
            dynamic_water_uniform.current.ripples[ripple_index],
            dynamic_water_uniform.previous.ripples[ripple_index],
            previous_frame,
        );
        let contribution = sample_player_ripple(
            rest_local_position,
            ripple.xy,
            ripple.z,
            ripple.w,
            ripple_shape.x,
            ripple_shape.y,
            ripple_shape.z,
            ripple_shape.w,
            ripple_limits.x,
            ripple_limits.w,
        );
        player_height += contribution.height;
        player_height_dx += contribution.height_dx;
        player_height_dz += contribution.height_dz;
        visual_signal += contribution.visual_signal;
    }

    // 多波源按旧语义先限幅再乘岸边权重；发生限幅时导数同步归零避免法线尖刺。
    let max_player_height = ripple_limits.y;
    let clamped_player_height = clamp(player_height, -max_player_height, max_player_height);
    if abs(player_height) < max_player_height {
        height_gradient += vec3<f32>(player_height_dx, 0.0, player_height_dz) * edge_fade;
    }
    let total_height = background_height + clamped_player_height * edge_fade;
    let clamped_visual_signal = clamp(visual_signal, -1.0, 1.0) * edge_fade;

    // 瀑布沿每个顶点的静止基准法线鼓动，弯折幕布不再被整张水体平均方向拉扯。
    var displacement_axis = vec3<f32>(0.0, 1.0, 0.0);
    if waterfall_weight > 0.5 {
        displacement_axis = base_normal;
    }

    return DynamicWaterSample(
        rest_local_position
            + displacement_axis * total_height
            + vec3<f32>(horizontal_displacement.x, 0.0, horizontal_displacement.y),
        normalize(base_normal - height_gradient),
        clamped_visual_signal,
    ); // 直接构造返回，避免结构体布局装饰落入Vulkan Function变量。
}

// 顶点颜色只保留玩家负半波压暗；样条沿程流纹迁移到逐像素片元阶段，避免两米网格插值抹平细节。
fn dynamic_water_vertex_color(base_color: vec4<f32>, visual_signal: f32) -> vec4<f32> {
    let trough_strength = clamp(-visual_signal, 0.0, 1.0);
    let darkening = 1.0 - trough_strength * dynamic_water_uniform.current.time_and_visual.y;
    return base_color * vec4<f32>(darkening, darkening, darkening, 1.0); // 保持作者Alpha，片元流纹不会在此重复乘色。
}
