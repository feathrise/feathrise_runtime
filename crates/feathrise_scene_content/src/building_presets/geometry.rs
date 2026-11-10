//! 建筑占地、门前净空与正门标记的局部到世界纯几何。

use bevy::prelude::*; // Vec2、Vec3 与 Rect 提供无 ECS 的数学值。

use super::{BuildingConnectionPointDescriptor, BuildingEntranceDescriptor, BuildingFootprintRect}; // 几何只解释已校验的建筑局部描述。

const SAT_EPSILON: f32 = 0.000_1; // 共边或只接触角点允许相邻建筑准确贴边。

/// 世界 X/Z 平面中的凸四边形；角点按局部矩形环顺序保存。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildingWorldQuad {
    /// 四个世界 X/Z 角点；非均匀缩放的旋转门区也能保留真实平行四边形。
    pub corners: [Vec2; 4],
}

impl BuildingWorldQuad {
    /// 把模型局部轴对齐矩形通过实例缩放、Y 轴旋转和平移变换到世界。
    pub fn from_local_rect(
        rect: BuildingFootprintRect, // 模型局部 X/Z 矩形。
        translation: [f32; 3],       // 实例世界根位置。
        yaw_degrees: f32,            // 实例世界 Y 轴角度。
        scale: [f32; 3],             // 实例三轴缩放。
    ) -> Self {
        let local = local_rect_corners(rect.center, rect.half_extents, 0.0); // 占地本身与模型局部 X/Z 轴对齐。
        Self::from_local_corners(local, translation, yaw_degrees, scale) // 四角必须与实际 Transform 使用同一 S→R→T 顺序。
    }

    /// 把正门朝向下的局部净空矩形变换到世界。
    pub fn from_entrance_clearance(
        entrance: BuildingEntranceDescriptor, // 门槛、朝向和门前半尺寸。
        translation: [f32; 3],                // 实例世界根位置。
        yaw_degrees: f32,                     // 实例世界 Y 轴角度。
        scale: [f32; 3],                      // 实例非均匀缩放也应影响净空位置与尺寸。
    ) -> Self {
        let local_door = Vec2::new(entrance.position[0], entrance.position[2]); // 门槛投影到模型局部 X/Z。
        let radians = entrance.outward_yaw_degrees.to_radians(); // 局部零度明确朝 +Z。
        let outward = Vec2::new(radians.sin(), radians.cos()); // 与 Bevy 绕 +Y 旋转的 X/Z 方向一致。
        let center = local_door + outward * entrance.clearance_half_extents[1]; // 净空从门槛向外完整延伸，不把门后空间当门前。
        let local = local_rect_corners(
            center.to_array(),               // 门前矩形中心转换回持久数据使用的数组形状。
            entrance.clearance_half_extents, // 横向与向外纵向半尺寸。
            entrance.outward_yaw_degrees,    // 局部矩形纵轴与正门朝向一致。
        );
        Self::from_local_corners(local, translation, yaw_degrees, scale) // 非均匀缩放后仍保留真实四角而不近似成 AABB。
    }

    /// 返回覆盖四边形的世界轴对齐包围，供空间网格做宽相位查询。
    pub fn aabb(self) -> Rect {
        let mut min = self.corners[0]; // 已校验矩形始终拥有四个有限角点。
        let mut max = self.corners[0]; // 首角同时初始化上下界。
        for corner in self.corners.iter().skip(1) {
            min = min.min(*corner); // 逐轴收紧世界下界。
            max = max.max(*corner); // 逐轴扩张世界上界。
        }
        Rect { min, max } // 宽相位只能增加候选，精确结果仍由 SAT 决定。
    }

    /// 返回中心与四角五个固定地基采样点。
    pub fn foundation_samples(self) -> [Vec2; 5] {
        let center = self
            .corners
            .iter()
            .copied()
            .fold(Vec2::ZERO, |sum, corner| sum + corner)
            * 0.25; // 仿射四边形的角点均值就是实例占地中心。
        [
            center,          // 中心先采样，后续跟随地形偏移以它为基准。
            self.corners[0], // 四角用于识别坡差和悬空。
            self.corners[1], // 第二角保持环顺序。
            self.corners[2], // 第三角保持环顺序。
            self.corners[3], // 第四角保持环顺序。
        ]
    }

    /// 判断全部角点是否位于场景 X/Z 闭区间内。
    pub fn inside(self, bounds: Rect) -> bool {
        self.corners.iter().all(|corner| {
            corner.x >= bounds.min.x
                && corner.x <= bounds.max.x
                && corner.y >= bounds.min.y
                && corner.y <= bounds.max.y // 角点全在凸矩形场景内即可证明整个建筑区域在界内。
        })
    }

    /// 判断一个世界 X/Z 点是否位于四边形内部或边界上。
    pub fn contains(self, point: Vec2) -> bool {
        let mut sign = 0.0_f32; // 首条非共线边决定凸多边形的统一绕向。
        for index in 0..4 {
            let start = self.corners[index]; // 当前有向边起点。
            let end = self.corners[(index + 1) % 4]; // 最后一条边闭合回首角。
            let edge = end - start; // 保留正负缩放后实际角点顺序。
            let relative = point - start; // 点相对当前边起点的位置。
            let cross = edge.perp_dot(relative); // 二维叉积符号表示点位于有向边哪一侧。
            if cross.abs() <= SAT_EPSILON {
                continue; // 边界点视为净空内部，其他边继续确认没有越出。
            }
            if sign == 0.0 {
                sign = cross.signum(); // 记录第一条有效边的侧向。
            } else if cross.signum() != sign {
                return false; // 凸四边形任一边异号即可证明点在外部。
            }
        }
        true // 全部有效边同号或点落在边界上。
    }

    /// 使用两组边法线执行凸四边形分离轴检测；只接触边界不算占地重叠。
    pub fn overlaps(self, other: Self) -> bool {
        !has_separating_axis(self, other) && !has_separating_axis(other, self) // 任一多边形边法线分离即可安全摆放。
    }

    /// 使用模型实际 S→R→T 顺序把四个局部点转换到世界 X/Z。
    fn from_local_corners(
        local: [Vec2; 4],      // 模型局部 X/Z 四角。
        translation: [f32; 3], // 实例世界根位置。
        yaw_degrees: f32,      // 实例世界 Y 轴角度。
        scale: [f32; 3],       // 实例三轴缩放。
    ) -> Self {
        let rotation = Quat::from_rotation_y(yaw_degrees.to_radians()); // 与摆放 Transform 完全一致。
        let origin = Vec3::from_array(translation); // 世界根平移保留 Y，但最后只取 X/Z。
        let scale = Vec3::from_array(scale); // 局部点在旋转前逐轴缩放。
        let corners = local.map(|point| {
            let world = origin + rotation * (Vec3::new(point.x, 0.0, point.y) * scale); // Bevy Transform 先缩放再旋转再平移。
            Vec2::new(world.x, world.z) // 空间索引只处理场景水平平面。
        });
        Self { corners } // 角点顺序在正负缩放下可能翻转，SAT 对方向不敏感。
    }
}

/// 生成可带局部旋转的矩形四角。
fn local_rect_corners(center: [f32; 2], half_extents: [f32; 2], yaw_degrees: f32) -> [Vec2; 4] {
    let center = Vec2::from_array(center); // 作者数组转换为局部向量。
    let half = Vec2::from_array(half_extents); // 半尺寸直接构造四个相对角。
    let radians = yaw_degrees.to_radians(); // 局部门口方向按角度旋转。
    let axis_x = Vec2::new(radians.cos(), -radians.sin()); // 绕 +Y 后局部 +X 在 X/Z 平面的方向。
    let axis_z = Vec2::new(radians.sin(), radians.cos()); // 绕 +Y 后局部 +Z 在 X/Z 平面的方向。
    [
        center - axis_x * half.x - axis_z * half.y, // 局部左后角。
        center + axis_x * half.x - axis_z * half.y, // 局部右后角。
        center + axis_x * half.x + axis_z * half.y, // 局部右前角。
        center - axis_x * half.x + axis_z * half.y, // 局部左前角。
    ]
}

/// 检查 `source` 任一边法线能否分离两个凸四边形。
fn has_separating_axis(source: BuildingWorldQuad, other: BuildingWorldQuad) -> bool {
    (0..4).any(|index| {
        let edge = source.corners[(index + 1) % 4] - source.corners[index]; // 当前有向边。
        let axis = Vec2::new(-edge.y, edge.x); // 未归一化法线足以比较投影区间。
        if axis.length_squared() <= f32::EPSILON {
            return false; // 已校验矩形不会退化，仍防御异常缩放导致的零边。
        }
        let (source_min, source_max) = project(source.corners, axis); // 当前多边形投影范围。
        let (other_min, other_max) = project(other.corners, axis); // 另一多边形投影范围。
        source_max <= other_min + SAT_EPSILON || other_max <= source_min + SAT_EPSILON // 共边允许，真实穿入才算重叠。
    })
}

/// 把四角投影到一个未归一化轴并返回标量闭区间。
fn project(corners: [Vec2; 4], axis: Vec2) -> (f32, f32) {
    let first = corners[0].dot(axis); // 首角初始化范围。
    corners
        .iter()
        .skip(1)
        .fold((first, first), |range, corner| {
            let value = corner.dot(axis); // 当前角的轴向标量。
            (range.0.min(value), range.1.max(value)) // 逐角收紧最小值并扩张最大值。
        })
}

/// 把正门门槛位置与朝外方向转换到实例世界空间。
pub(super) fn world_entrance_marker(
    entrance: &BuildingEntranceDescriptor, // 当前预设正门描述。
    translation: [f32; 3],                 // 实例世界根位置。
    yaw_degrees: f32,                      // 实例世界 Y 轴旋转。
    scale: [f32; 3],                       // 门槛位置随模型缩放。
) -> (Vec3, Vec3) {
    let instance_rotation = Quat::from_rotation_y(yaw_degrees.to_radians()); // 与实际摆放 Transform 一致。
    let position = Vec3::from_array(translation)
        + instance_rotation * (Vec3::from_array(entrance.position) * Vec3::from_array(scale)); // 模型局部门槛经 S→R→T 得到世界位置。
    let local_outward = Quat::from_rotation_y(entrance.outward_yaw_degrees.to_radians()) * Vec3::Z; // 局部零度朝 +Z。
    let outward = (instance_rotation * local_outward).normalize_or_zero(); // 方向不受缩放长度影响并保持有限。
    (position, outward) // 预览箭头和未来沿街朝向读取同一结果。
}

/// 把地基平面的模块连接点转换到实例世界空间。
pub(super) fn world_connection_marker(
    connection: &BuildingConnectionPointDescriptor, // 局部 X/Z 与朝外方向。
    model_base_y: f32,                              // 连接点首期固定使用地基基准 Y。
    translation: [f32; 3],                          // 实例世界根位置。
    yaw_degrees: f32,                               // 实例世界 Y 轴旋转。
    scale: [f32; 3],                                // 位置使用与模型一致的三轴缩放。
) -> (Vec3, Vec3) {
    let instance_rotation = Quat::from_rotation_y(yaw_degrees.to_radians()); // 与摆放 Transform 完全一致。
    let local_position = Vec3::new(connection.position[0], model_base_y, connection.position[1]); // 数据明确只有地基平面 X/Z，自身不声称支持楼层连接。
    let position = Vec3::from_array(translation)
        + instance_rotation * (local_position * Vec3::from_array(scale)); // S→R→T 得到连接点世界位置。
    let local_outward =
        Quat::from_rotation_y(connection.outward_yaw_degrees.to_radians()) * Vec3::Z; // 零度朝局部 +Z。
    let outward = (instance_rotation * local_outward).normalize_or_zero(); // 方向只随旋转，不随缩放长度。
    (position, outward)
}

#[cfg(test)]
mod tests {
    use super::*; // 纯几何回归不建立 Bevy App。

    #[test]
    fn footprint_rotation_and_nonuniform_scale_match_instance_transform() {
        let rect = BuildingFootprintRect {
            center: [1.0, 0.0],       // 局部中心向 +X 偏移一米。
            half_extents: [2.0, 1.0], // 局部宽四米、深两米。
        };
        let quad = BuildingWorldQuad::from_local_rect(
            rect,              // 使用真实公共矩形描述。
            [10.0, 3.0, 20.0], // 世界根位于明确非零位置。
            90.0,              // 局部 +X 旋到世界 -Z。
            [2.0, 1.0, 3.0],   // X/Z 使用不同缩放验证顺序。
        );
        let aabb = quad.aabb(); // 四角结果应覆盖旋转后的八米乘六米范围。
        assert!((aabb.min.x - 7.0).abs() < 0.001); // 局部 Z 三倍缩放成为世界 X 半宽三米。
        assert!((aabb.max.x - 13.0).abs() < 0.001); // 世界 X 上界对称于根 X。
        assert!((aabb.min.y - 14.0).abs() < 0.001); // 偏移中心和 X 半宽共同旋到世界 -Z。
        assert!((aabb.max.y - 22.0).abs() < 0.001); // 世界 Z 上界保留正确 S→R→T 顺序。
    }

    #[test]
    fn touching_footprints_are_allowed_but_real_overlap_is_rejected() {
        let rect = BuildingFootprintRect::default(); // 四米见方占地半尺寸为两米。
        let first = BuildingWorldQuad::from_local_rect(rect, [0.0, 0.0, 0.0], 0.0, [1.0; 3]); // 第一栋位于原点。
        let touching = BuildingWorldQuad::from_local_rect(rect, [4.0, 0.0, 0.0], 0.0, [1.0; 3]); // 第二栋只在边界接触。
        let overlapping = BuildingWorldQuad::from_local_rect(rect, [3.9, 0.0, 0.0], 0.0, [1.0; 3]); // 第二栋真实进入十厘米。
        assert!(!first.overlaps(touching)); // 共边允许紧密排列成街巷。
        assert!(first.overlaps(overlapping)); // 实体占地穿插必须拒绝。
    }

    #[test]
    fn entrance_clearance_starts_at_door_and_extends_outward() {
        let entrance = BuildingEntranceDescriptor::default(); // 门槛位于局部 +Z 两米且朝 +Z。
        let quad = BuildingWorldQuad::from_entrance_clearance(
            entrance,        // 默认净空纵向半尺寸一点五米。
            [0.0, 0.0, 0.0], // 模型根位于世界原点。
            0.0,             // 不追加实例旋转。
            [1.0, 1.0, 1.0], // 单位缩放便于直接核对。
        );
        let aabb = quad.aabb(); // 门前净空应从门槛 Z=2 延伸到 Z=5。
        assert!((aabb.min.y - 2.0).abs() < 0.001); // 内侧边与门槛重合。
        assert!((aabb.max.y - 5.0).abs() < 0.001); // 完整三米深空间位于门外。
    }

    #[test]
    fn rotated_quad_contains_interior_and_boundary_but_rejects_exterior() {
        let quad = BuildingWorldQuad::from_local_rect(
            BuildingFootprintRect::default(), // 四米见方便于核对旋转后范围。
            [4.0, 0.0, -3.0],                 // 使用非零世界根验证平移。
            45.0,                             // 点测试不能退化成 AABB 判断。
            [1.0; 3],                         // 单位缩放保持测试焦点明确。
        );
        assert!(quad.contains(Vec2::new(4.0, -3.0))); // 世界根位于内部中心。
        assert!(quad.contains(quad.corners[0])); // 精确角点属于闭净空范围。
        assert!(!quad.contains(Vec2::new(8.0, -3.0))); // AABB 外点必须拒绝。
    }
}
