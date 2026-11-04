//! 遮雨CPU判定与GPU逐轴slab算法一致，边界不依赖物理Collider。

use super::{RainShelterBox, RainShelterState, RainShelterVolume};
use bevy::prelude::*; // 使用同源浮点矩阵和向量。 // 作者输入和正式批次。

impl RainShelterVolume {
    // 构造时验证所有数值，避免NaN污染全帧。
    pub fn from_box(source: RainShelterBox, transform: &GlobalTransform) -> Option<Self> {
        // 常量规模8角以内计算。
        if !source.center.is_finite()
            || !source.half_extents.is_finite()
            || source.half_extents.min_element() <= 0.0
        {
            return None;
        } // 拒绝负值、零值和非法输入。
        let local_to_world = transform.to_matrix() * Mat4::from_translation(source.center); // 中心跟随父节点变换。
        let determinant = local_to_world.determinant(); // 退化缩放没有可靠逆变换。
        if !local_to_world.is_finite() || !determinant.is_finite() || determinant.abs() < 1e-8 {
            return None;
        } // 拒绝奇异变换。
        let world_to_local = local_to_world.inverse(); // 射线参数仍保持世界米。
        if !world_to_local.is_finite() {
            return None;
        } // 极小尺度逆变换也必须有限。
        let center = local_to_world.transform_point3(Vec3::ZERO); // 获取世界盒中心。
        let extent_y = local_to_world.x_axis.y.abs() * source.half_extents.x
            + local_to_world.y_axis.y.abs() * source.half_extents.y
            + local_to_world.z_axis.y.abs() * source.half_extents.z; // 精确求OBB世界最大Y，不扫描顶点。
        let max_y = center.y + extent_y; // 顶部快速拒绝屋顶上方片元。
        if !max_y.is_finite() {
            return None;
        } // 极端作者值不能绕过有限性检查。
        let extent_x = local_to_world.x_axis.x.abs() * source.half_extents.x
            + local_to_world.y_axis.x.abs() * source.half_extents.y
            + local_to_world.z_axis.x.abs() * source.half_extents.z; // 绝对矩阵求真实OBB的世界X投影半径。
        let extent_z = local_to_world.x_axis.z.abs() * source.half_extents.x
            + local_to_world.y_axis.z.abs() * source.half_extents.y
            + local_to_world.z_axis.z.abs() * source.half_extents.z; // 世界Z投影半径不扫描八个角点。
        let world_bounds = Vec4::new(
            center.x - extent_x,
            center.z - extent_z,
            center.x + extent_x,
            center.z + extent_z,
        ); // 垂直射线XZ恒定，范围外立即拒绝。
        if !world_bounds.is_finite() {
            return None;
        } // 极端缩放不能生成非有限宽相位。
        Some(Self {
            world_to_local,
            half_extents: source.half_extents.extend(max_y),
            world_bounds,
        }) // 96字节完整结果一次移交。
    } // 完成代理构造。

    pub fn blocks_rain(&self, point: Vec3) -> bool {
        // 世界垂直受雨，不宣称模拟斜风雨的屋檐漂入。
        if !point.is_finite() || point.y + 0.03 >= self.half_extents.w {
            return false;
        } // 屋顶上面仍能受雨。
        if point.x < self.world_bounds.x
            || point.z < self.world_bounds.y
            || point.x > self.world_bounds.z
            || point.z > self.world_bounds.w
        {
            return false;
        } // 屋外片元先做四次便宜范围比较。
        let origin = self.world_to_local.transform_point3(point + Vec3::Y * 0.03); // 厘米偏移防止表面自遮挡。
        let direction = self.world_to_local.transform_vector3(Vec3::Y); // 不归一化，保留128世界米上限。
        let mut near = 0.0_f32; // 射线只向上。
        let mut far = 128.0_f32; // 超高天空建筑不会无限投射遮雨。
        for axis in 0..3 {
            // 固定三轴，与WGSL分支一致。
            let radius = self.half_extents[axis]; // 当前局部轴半径。
            if direction[axis].abs() < 1e-6 {
                // 平行射线不能除以零。
                if origin[axis].abs() > radius {
                    return false;
                } // 平行且位于盒外。
            } else {
                // 非平行轴收紧交集。
                let first = (-radius - origin[axis]) / direction[axis]; // 一侧平面距离。
                let second = (radius - origin[axis]) / direction[axis]; // 另一侧平面距离。
                near = near.max(first.min(second)); // 更新入口。
                far = far.min(first.max(second)); // 更新出口。
                if near > far {
                    return false;
                } // 无共同区间。
            } // 完成当前轴。
        } // 仅三个轴，不依赖场景规模。
        true // 有限向上射线击中盒子。
    } // 完成单体同源判定。
} // 完成体行为。

impl RainShelterState {
    // 所有雨效共享统一受雨率。
    pub fn exposure(&self, point: Vec3) -> f32 {
        // 当前二值遮挡保留浮点接口方便后续边缘过渡。
        if !point.is_finite() || self.gpu.header.x <= 0.0 || point.y + 0.03 >= self.gpu.header.y {
            return 1.0;
        } // 无代理/屋顶上方常量早退。
        let count = (self.gpu.header.x as usize).min(32); // 防御宿主非法直接写入。
        if self.gpu.shelters[..count]
            .iter()
            .any(|shelter| shelter.blocks_rain(point))
        {
            0.0
        } else {
            1.0
        } // 最多32次小OBB测试。
    } // 完成共享受雨率。
} // 完成资源行为。
