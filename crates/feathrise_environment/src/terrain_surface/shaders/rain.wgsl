#define_import_path feathrise_environment::surface_rain
// 地表积水与河湖共用的有界雨滴法线；无粒子、碰撞或玩家波源槽位依赖。

fn surface_rain_hash(cell: vec2<f32>) -> vec3<f32> { // 世界单元确定落点与错开时间，镜头移动不改变雨滴身份。
    let coordinates = bitcast<vec2<u32>>(vec2<i32>(cell)); // 世界单元为整数，负坐标按补码保留身份。
    var seed = vec3<u32>(coordinates.x * 1597334677u + coordinates.y * 3812015801u) + vec3<u32>(0u, 1013904223u, 2246822519u); // 无三角函数的三个独立整数种子。
    seed = (seed ^ (seed >> vec3<u32>(16u))) * vec3<u32>(2246822519u); // 整数雪崩避免大世界浮点sin哈希退化。
    seed = (seed ^ (seed >> vec3<u32>(13u))) * vec3<u32>(3266489917u); // 第二次混合减少邻格与相邻事件相关性。
    seed = seed ^ (seed >> vec3<u32>(16u)); // 完成固定指令数的散列。
    return vec3<f32>(seed >> vec3<u32>(8u)) * (1.0 / 16777216.0); // 取24位精确尾数，结果严格小于一。
} // 结束稳定哈希。

struct SurfaceRainEvent { // 环纹和近景水花可共用同一世界事件，不占用玩家波纹槽。
    center: vec2<f32>, // 本次雨滴的世界米制落点。
    age: f32, // 当前事件的暂停感知秒龄，范围零至一。
    seed: f32, // 与出生相位独立的密度及尺度随机量。
} // 结束事件数据。

fn surface_rain_event(cell: vec2<f32>, elapsed: f32) -> SurfaceRainEvent { // 单元每秒更换落点，避免固定点机械重复。
    let stagger = surface_rain_hash(cell).z; // 固定错相使邻格不在整秒同时换代。
    let clock = elapsed + stagger; // 使用同一暂停感知时钟，不依赖帧率或镜头。
    let cycle = floor(clock) % 4096.0; // 事件种子与天气回绕同步，仅4096秒后重复。
    let random = surface_rain_hash(cell + vec2<f32>(cycle * 17.0, cycle * 31.0)); // 每次事件重新选择落点、尺度和强度。
    return SurfaceRainEvent((cell + random.xy) * 0.8, fract(clock), random.z); // 出生时换点，寿命两端均由包络归零。
} // 结束可复用的固定成本雨滴事件。

fn sample_surface_rain_slope( // 调用方仅在实际水面片元调用，并将岸边/遮雨覆盖乘入返回坡度。
    world_xz: vec2<f32>, // 世界米制位置，禁止使用随相机移动的坐标。
    elapsed: f32, // 暂停感知表面效果秒数。
    rain: f32, // 实际降雨量零至一，雪不得传入。
    quality: f32, // 零低、一中、二高、三极高；密度与距离分档而非增加候选循环。
    camera_distance: f32, // 当前片元到相机的世界距离。
    pixel_meters: f32, // 在一致控制流中预先取得的世界位置导数脚印。
    rain_settings: vec2<f32>, // 唯一质量表经64字节共享参数传入的距离/密度。
) -> vec2<f32> { // 输出水平法线坡度，调用方合并并归一化，不直接增亮水色。
    let range = max(rain_settings.x, 0.01); // 主世界负责四档映射，无效关闭值也不产生零分母。
    let visibility = (1.0 - smoothstep(range * 0.65, range, camera_distance)) * (1.0 - smoothstep(0.025, 0.09, pixel_meters)); // 距离与亚像素带限共同抑制摩尔纹。
    if rain <= 0.0001 || visibility <= 0.0001 { // 晴天和远景在哈希/候选计算前退出，低档仍保留核心效果。
        return vec2<f32>(0.0); // 保留原材质无扰动状态。
    } // 结束低成本快速出口。
    let cell_size = 0.8; // 单元边长大于最大圆环半径，3x3邻域完整覆盖全部贡献。
    let grid = floor(world_xz / cell_size); // 固定世界单元身份，负坐标同样连续。
    let density = clamp(rain, 0.0, 1.0) * clamp(rain_settings.y, 0.0, 1.0); // 同表密度只改变接纳权重，最坏候选仍为九个。
    var slope = vec2<f32>(0.0); // 所有贡献仅局部累加，不写全局状态。
    for (var z = -1; z <= 1; z = z + 1) { // 固定三行，不随雨量或地图大小增长。
        for (var x = -1; x <= 1; x = x + 1) { // 固定三列，总计九个雨滴候选。
            let cell = grid + vec2<f32>(f32(x), f32(z)); // 当前邻近单元。
            let event = surface_rain_event(cell, elapsed); // 当前秒事件独立落点，雨量变化不改变相位。
            let enabled = smoothstep(event.seed - 0.08, event.seed + 0.08, density); // 在波形计算前处理质量与雨量接纳。
            if enabled <= 0.0001 { continue; } // 低档稀疏雨纹跳过被拒事件的长度/三角计算。
            let phase = event.age; // 生命周期与未来同源水花共用事件年龄。
            let offset = world_xz - event.center; // 世界米制圆环中心差。
            let radius = length(offset); // 实际水平距离。
            let ring_radius = phase * mix(0.26, 0.42, event.seed); // 每秒扩散26至42厘米，始终小于单元宽度。
            let signed_distance = radius - ring_radius; // 圆环内外法线相反，形成完整的小尺度波峰波谷。
            let ring_width = max(0.018, pixel_meters * 1.2); // 至少覆盖一个像素，避免近景窄环锐利闪烁。
            let ring = sin(clamp(signed_distance / (ring_width * 2.0), -1.0, 1.0) * 3.14159265) * (1.0 - smoothstep(ring_width, ring_width * 2.0, abs(signed_distance))); // 两侧有符号扰动且有限支撑，边界连续归零。
            let life = smoothstep(0.0, 0.12, phase) * (1.0 - smoothstep(0.50, 1.0, phase)); // 出生与消失均连续，时间回绕时圆环贡献为零。
            slope = slope + offset / max(radius, 0.001) * ring * life * enabled * mix(0.025, 0.045, event.seed); // 厘米级短波降低强度，保留玩家大波主次关系。
            if quality >= 1.0 && camera_distance < 12.0 && radius < 0.075 && phase < 0.12 { // 低档、远景和冠纹寿命外不计算额外sin。
                let crown = (1.0 - smoothstep(0.025, 0.075, radius)) * sin(phase / 0.12 * 3.14159265); // 出生短脉冲模拟贴面冲击冠纹，不冒充离水飞溅粒子。
                let crown_lod = 1.0 - smoothstep(6.0, 12.0, camera_distance); // 有限距离内连续淡出。
                slope = slope + offset / max(radius, 0.001) * crown * enabled * crown_lod * 0.09; // 复用同一落点，仍不占玩家槽。
            } // 完成近景冠纹早退分档。
        } // 完成一行三个候选。
    } // 完成唯一固定九候选循环。
    return clamp(slope, vec2<f32>(-0.16), vec2<f32>(0.16)) * visibility * clamp(rain * 4.0, 0.0, 1.0); // 防止密集重叠导致镜面闪白，弱雨连续归零。
} // 结束地面和河湖同源雨滴细纹。

fn sample_surface_wind_slope( // 积水和河湖读取同一风场；只扰动受光法线，不改变物理水位。
    world_xz: vec2<f32>, // 固定世界米制坐标。
    elapsed: f32, // 与雨纹同一暂停感知秒。
    wind: vec2<f32>, // 当前连续天气风向，不绑定相机。
    intensity: f32, // 调用方传入受雨量/水膜覆盖调制的风强。
    quality: f32, // 中低一层，高极高两层，循环数量不随天气增长。
    camera_distance: f32, // 只对当前视图可见尺度求细法线。
    pixel_meters: f32, // 调用方在统一控制流求出的屏幕脚印。
) -> vec2<f32> { // 返回有限世界水平坡度。
    let detail = (1.0 - smoothstep(0.025, 0.18, pixel_meters)) * (1.0 - smoothstep(18.0, 48.0, camera_distance)); // 远距与亚像素前平滑归零。
    let strength = clamp(intensity, 0.0, 1.0) * detail; // 无风无雨时精确归零。
    let wind_length = length(wind); // 方向必须有效才可归一化。
    if strength <= 0.0001 || wind_length <= 0.0001 { return vec2<f32>(0.0); } // 晴天与远景跳过全部波形计算。
    let direction = wind / wind_length; // 风速变化只改幅度，不让波纹相位瞬间换速。
    let cross_direction = vec2<f32>(-direction.y, direction.x); // 第二组固定斜向波打破整齐平行条纹。
    let phase = dot(world_xz, direction) * 5.0 - fract(elapsed * 0.5) * 6.2831853; // 两秒周期整除4096秒天气回绕。
    var slope = direction * cos(phase) * 0.025; // 主风纹保持克制，小于玩家大波。
    if quality >= 2.0 { // 高档才增加第二个三角函数，低档保留核心风向。
        let secondary_direction = normalize(direction * 0.8 + cross_direction * 0.6); // 固定旋转不随时间重新随机。
        let secondary_phase = dot(world_xz, secondary_direction) * 8.0 - fract(elapsed * 0.25) * 6.2831853; // 四秒错周期同样兼容暂停与回绕。
        slope = slope + secondary_direction * sin(secondary_phase) * 0.012; // 有限第二波幅，减轻单条高光重复。
    } // 完成固定一/两层风纹。
    return slope * strength; // 不直接增亮水色，交给已有PBR计算反射。
} // 结束同源风雨细浪。
