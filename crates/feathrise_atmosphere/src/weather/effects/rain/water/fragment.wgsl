// 从实际展开雨丝片元恢复世界坐标，避免只判断中心导致下半截穿水。
if all(abs(in.uv) < vec2<f32>(1e19)) { // 顶点阶段用1e20原点表示禁用，片元绝不访问仅VERTEX可见的属性缓冲。
    let water_origin = in.uv; // 当前雨丝所有顶点传递相同窗口原点，暂停也读取本帧属性。
    let rain_ndc = (in.position.xy - view.main_pass_viewport.xy) / max(view.main_pass_viewport.zw, vec2<f32>(1.0)) * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0); // 使用SR内部viewport而非最终窗口尺寸。
    let rain_world_h = view.world_from_clip * vec4<f32>(rain_ndc, in.position.z, 1.0); // 片元深度对应实际雨丝几何，不是粒子中心。
    if abs(rain_world_h.w) > 0.000001 { // 非法投影不扩散为NaN裁切。
        let rain_world = rain_world_h.xyz / rain_world_h.w; // 世界米制位置。
        let water_uv = (rain_world.xz - water_origin) / RAIN_WATER_SPAN; // 顶视up=-Z，跨度由共享Rust常量生成，纹理Y向+Z。
        if all(water_uv >= vec2<f32>(0.0)) && all(water_uv < vec2<f32>(1.0)) { // 范围外不能Clamp到湖边，避免岸上误挡雨。
            let water_pixel = vec2<i32>(water_uv * vec2<f32>(textureDimensions(material_texture_0))); // 最近像素保留明确覆盖，无混合空像素假高度。
            let water_sample = textureLoad(material_texture_0, water_pixel, 0); // 每片元至多读取一次局部GPU高度图，无CPU回读。
            let water_height = (water_sample.r + water_sample.g) * 16.0; // 两段编码避免高海拔半精度损失整个米级波形。
            if water_sample.b > 0.5 && rain_world.y < water_height + 0.015 { discard; } // 裁去水下部分并保留水面之上雨丝，厘米偏移覆盖量化误差。
        } // 完成局部真实水面测试。
    } // 完成片元世界位置重建。
} // 原有水面涟漪、水花与玩家反馈仍由各自系统处理。
