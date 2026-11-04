// 先检查目标尺寸，1×1全零fallback不会发生越界元数据读取。
let roof_dimensions = textureDimensions(material_texture_1); // 深度区比纹理少一行，首行为原子元数据。
if roof_dimensions.x >= 17u && roof_dimensions.y > 1u { // 未准备目标安全回退为不遮挡。
    let roof_valid = textureLoad(material_texture_1, vec2<i32>(16, 0), 0); // 与同次GPU深度复制一起写入。
    if roof_valid.x > 0.5 && roof_valid.a > 0.5 { // 首次未渲染全零图不参与遮挡。
        let roof_ndc = (in.position.xy - view.main_pass_viewport.xy) / max(view.main_pass_viewport.zw, vec2<f32>(1.0)) * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0); // 使用真实内部渲染视口，兼容超分。
        let roof_world_h = view.world_from_clip * vec4<f32>(roof_ndc, in.position.z, 1.0); // 重建实际雨丝片元而非粒子中心。
        if abs(roof_world_h.w) > 0.000001 { // 非法齐次位置不产生遮挡。
            let roof_matrix = mat4x4<f32>(rain_occlusion_column(0), rain_occlusion_column(1), rain_occlusion_column(2), rain_occlusion_column(3)); // 投影与深度来自同一纹理版本。
            let roof_clip = roof_matrix * vec4<f32>(roof_world_h.xyz / roof_world_h.w, 1.0); // 世界位置投影到雨向相机。
            if roof_clip.w > 0.000001 { // 正交正式相机w为1，损坏数据安全退出。
                let roof_projected = roof_clip.xyz / roof_clip.w; // Bevy反向Z，靠近雨源的表面深度更大。
                let roof_uv = roof_projected.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5); // NDC转纹理UV，翻转Y。
                if all(roof_uv >= vec2<f32>(0.0)) && all(roof_uv < vec2<f32>(1.0)) && roof_projected.z >= 0.0 && roof_projected.z <= 1.0 { // 局部范围外不得Clamp成无限屋顶。
                    let roof_size = vec2<u32>(roof_dimensions.x, roof_dimensions.y - 1u); // 元数据行不能参与空间采样。
                    let roof_pixel = vec2<i32>(roof_uv * vec2<f32>(roof_size)) + vec2<i32>(0, 1); // 最近像素保持天井开口，不线性混合相邻屋檐。
                    let roof_depth = rain_occlusion_decode(textureLoad(material_texture_1, roof_pixel, 0)); // 单次深度读取，无CPU回读。
                    if roof_depth > 0.0 && roof_projected.z < roof_depth - 0.03 / 256.0 { discard; } // 256米深度范围中保留3厘米偏移，裁去屋顶之后雨丝。
                } // 完成局部遮挡比较。
            } // 完成雨向投影。
        } // 完成片元世界位置恢复。
    } // 无有效完整深度时保留原始雨幕。
} // 水面裁切和作者代理继续在各自阶段补充生效。
