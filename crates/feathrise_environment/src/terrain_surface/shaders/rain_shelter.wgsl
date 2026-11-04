// 地表、水面与Hanabi逐滴共用的纯遮雨盒相交函数；不依赖材质绑定。
fn rain_shelter_volume_blocks(world: vec3<f32>, world_to_local: mat4x4<f32>, half_extents: vec4<f32>, world_bounds: vec4<f32>) -> bool { // 参数与CPU代理严格同源。
    let origin = world + vec3<f32>(0.0, 0.03, 0.0); // 屋顶表面自身保留受雨。
    if origin.y >= half_extents.w { return false; } // 最高点上方无矩阵开销。
    if any(world.xz < world_bounds.xy) || any(world.xz > world_bounds.zw) { return false; } // 屋外先用世界XZ包围范围拒绝，只在附近做精确旋转盒射线。
    let local_origin = (world_to_local * vec4<f32>(origin, 1.0)).xyz; // 世界起点转局部。
    let direction = (world_to_local * vec4<f32>(0.0, 1.0, 0.0, 0.0)).xyz; // 不归一化以保留世界米距离。
    var near = 0.0; var far = 128.0; // 与CPU相同的向上128米查询。
    for (var axis = 0u; axis < 3u; axis += 1u) { // 固定三个slab裁剪。
        let radius = half_extents[axis]; // 作者局部半径。
        if abs(direction[axis]) < 0.000001 { // 平行分支避免除零。
            if abs(local_origin[axis]) > radius { return false; } // 平行且盒外拒绝。
        } else { // 非平行求两侧交点。
            let a = (-radius - local_origin[axis]) / direction[axis]; // 负向平面。
            let b = (radius - local_origin[axis]) / direction[axis]; // 正向平面。
            near = max(near, min(a, b)); far = min(far, max(a, b)); // 三轴交集。
            if near > far { return false; } // 空集不遮雨。
        }
    }
    return true; // 向上射线与显式代理相交。
}
