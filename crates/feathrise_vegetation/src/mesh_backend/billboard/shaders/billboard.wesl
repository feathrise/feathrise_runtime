
// Rust侧固定UVec4配置：x为模式、y为资产正面轴、z/w保留。
@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<uniform> vegetation_billboard_configuration: vec4<u32>;

const VEGETATION_BILLBOARD_MODE_FIXED: u32 = 0u; // 固定模式保留作者世界矩阵。
const VEGETATION_BILLBOARD_MODE_CYLINDRICAL: u32 = 1u; // 圆柱模式锁定世界Y轴。
const VEGETATION_BILLBOARD_MODE_SPHERICAL: u32 = 2u; // 全向模式使用完整相机方向。
const VEGETATION_BILLBOARD_AXIS_POSITIVE_X: u32 = 0u; // 局部正X是资产正面。
const VEGETATION_BILLBOARD_AXIS_NEGATIVE_X: u32 = 1u; // 局部负X是资产正面。
const VEGETATION_BILLBOARD_AXIS_POSITIVE_Z: u32 = 2u; // 局部正Z是资产正面。
const VEGETATION_BILLBOARD_AXIS_NEGATIVE_Z: u32 = 3u; // 局部负Z是资产正面。
const VEGETATION_BILLBOARD_EPSILON: f32 = 0.000001; // 退化方向与零缩放统一安全阈值。

// 根据当前渲染视图为一个实例构造世界朝向，同时保留平移、逐轴缩放与镜像奇偶性。
fn vegetation_billboard_world_from_local(
    source_world_from_local: mat4x4<f32>, // ECS传播得到的作者世界矩阵。
    camera_world_position: vec3<f32>, // Bevy为主视图及阴影视图提供的LOD观察位置。
) -> mat4x4<f32> {
    let mode = vegetation_billboard_configuration.x; // 每份共享材质固定一种朝向模式。
    if mode == VEGETATION_BILLBOARD_MODE_FIXED {
        return source_world_from_local; // 固定交叉面完全保留原变换。
    }
    if mode != VEGETATION_BILLBOARD_MODE_CYLINDRICAL &&
        mode != VEGETATION_BILLBOARD_MODE_SPHERICAL {
        return source_world_from_local; // 未知编码安全回退作者姿态。
    }

    let world_origin = source_world_from_local[3].xyz; // 节点锚点始终由ECS世界矩阵决定。
    var front_direction = camera_world_position - world_origin; // 正面从节点指向当前视图观察位置。
    if mode == VEGETATION_BILLBOARD_MODE_CYLINDRICAL {
        front_direction.y = 0.0; // 圆柱公告板禁止随俯仰压倒植被平面。
    }
    let front_length_squared = dot(front_direction, front_direction); // 避免直接normalize零向量。
    if front_length_squared <= VEGETATION_BILLBOARD_EPSILON {
        return source_world_from_local; // 相机与节点重合时确定性保留作者姿态。
    }
    front_direction *= inverseSqrt(front_length_squared); // 得到有限单位正面方向。

    var up_candidate = vec3<f32>(0.0, 1.0, 0.0) -
        front_direction * front_direction.y; // 把世界Y投影到正面法平面，尽力保持竖直。
    var up_length_squared = dot(up_candidate, up_candidate); // 全向相机正上/下方时可能退化。
    if up_length_squared <= VEGETATION_BILLBOARD_EPSILON {
        let fallback_up = select(
            vec3<f32>(0.0, 0.0, 1.0), // 正面接近世界X时改用世界Z参考。
            vec3<f32>(1.0, 0.0, 0.0), // 其他极点方向优先改用世界X参考。
            abs(front_direction.x) < 0.9,
        ); // 选择不会与当前正面平行的稳定备用轴。
        up_candidate = fallback_up -
            front_direction * dot(fallback_up, front_direction); // 同样投影到正面法平面。
        up_length_squared = dot(up_candidate, up_candidate); // 备用轴按选择规则必定非退化。
    }
    let up_reference = up_candidate * inverseSqrt(up_length_squared); // 得到正交竖直参考。

    var world_axis_x = vec3<f32>(1.0, 0.0, 0.0); // 局部X的世界方向默认单位轴。
    var world_axis_z = vec3<f32>(0.0, 0.0, 1.0); // 局部Z的世界方向默认单位轴。
    let front_axis = vegetation_billboard_configuration.y; // 读取资产显式正面轴。
    if front_axis == VEGETATION_BILLBOARD_AXIS_POSITIVE_X {
        world_axis_x = front_direction; // 局部正X直接指向相机。
        world_axis_z = normalize(cross(world_axis_x, up_reference)); // 保持右手坐标系。
    } else if front_axis == VEGETATION_BILLBOARD_AXIS_NEGATIVE_X {
        world_axis_x = -front_direction; // 局部负X指向相机等价于正X反向。
        world_axis_z = normalize(cross(world_axis_x, up_reference)); // 保持右手坐标系。
    } else if front_axis == VEGETATION_BILLBOARD_AXIS_POSITIVE_Z {
        world_axis_z = front_direction; // 局部正Z直接指向相机。
        world_axis_x = normalize(cross(up_reference, world_axis_z)); // 保持右手坐标系。
    } else if front_axis == VEGETATION_BILLBOARD_AXIS_NEGATIVE_Z {
        world_axis_z = -front_direction; // 局部负Z指向相机等价于正Z反向。
        world_axis_x = normalize(cross(up_reference, world_axis_z)); // 保持右手坐标系。
    } else {
        return source_world_from_local; // 未知正面轴编码安全回退作者姿态。
    }
    let world_axis_y = normalize(cross(world_axis_z, world_axis_x)); // 重建严格正交的局部Y世界方向。

    let source_x = source_world_from_local[0].xyz; // 作者矩阵局部X列包含旋转、缩放与潜在镜像。
    let source_y = source_world_from_local[1].xyz; // 作者矩阵局部Y列包含旋转、缩放与潜在镜像。
    let source_z = source_world_from_local[2].xyz; // 作者矩阵局部Z列包含旋转、缩放与潜在镜像。
    let source_scale_lengths = vec3<f32>(
        length(source_x), // 提取局部X世界缩放长度。
        length(source_y), // 提取局部Y世界缩放长度。
        length(source_z), // 提取局部Z世界缩放长度。
    ); // 完成逐轴缩放提取。
    if any(source_scale_lengths <= vec3<f32>(VEGETATION_BILLBOARD_EPSILON)) {
        return source_world_from_local; // 零缩放或非法层级不构造不可逆世界矩阵。
    }
    let source_determinant = dot(cross(source_x, source_y), source_z); // 保留原矩阵镜像奇偶性。
    let reflected_x = select(-1.0, 1.0, source_determinant >= 0.0); // 把总镜像符号稳定归入X轴。
    let signed_scale = vec3<f32>(
        source_scale_lengths.x * reflected_x, // 局部X同时承载原镜像符号。
        source_scale_lengths.y, // 局部Y保留正缩放长度。
        source_scale_lengths.z, // 局部Z保留正缩放长度。
    ); // 完成可重建右手或镜像矩阵的逐轴缩放。

    return mat4x4<f32>(
        vec4<f32>(world_axis_x * signed_scale.x, 0.0), // 第一列为朝向后的局部X轴。
        vec4<f32>(world_axis_y * signed_scale.y, 0.0), // 第二列为朝向后的局部Y轴。
        vec4<f32>(world_axis_z * signed_scale.z, 0.0), // 第三列为朝向后的局部Z轴。
        vec4<f32>(world_origin, 1.0), // 第四列逐字保留ECS世界锚点。
    ); // 返回GPU当前视图专属世界矩阵。
}

// 用GPU公告板矩阵的逆转置等价式转换法线，支持非均匀缩放且避免矩阵求逆。
fn vegetation_billboard_normal_local_to_world(
    billboard_world_from_local: mat4x4<f32>, // 已按当前视图定向的世界矩阵。
    local_normal: vec3<f32>, // Mesh导入的局部法线。
) -> vec3<f32> {
    if all(local_normal == vec3<f32>(0.0)) {
        return local_normal; // 无效零法线保持Bevy默认行为并避免normalize产生NaN。
    }
    let axis_x = billboard_world_from_local[0].xyz; // 含符号缩放的世界X列。
    let axis_y = billboard_world_from_local[1].xyz; // 含缩放的世界Y列。
    let axis_z = billboard_world_from_local[2].xyz; // 含缩放的世界Z列。
    let transformed =
        axis_x * (local_normal.x / dot(axis_x, axis_x)) + // 等价于正交X轴乘局部分量除以缩放。
        axis_y * (local_normal.y / dot(axis_y, axis_y)) + // 等价于正交Y轴乘局部分量除以缩放。
        axis_z * (local_normal.z / dot(axis_z, axis_z)); // 等价于正交Z轴乘局部分量除以缩放。
    return normalize(transformed); // PBR按MikkTSpace约定需要重新归一化世界法线。
}
