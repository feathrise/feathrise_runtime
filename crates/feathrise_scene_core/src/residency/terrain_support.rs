//! 已采用地形的活动半径与真实碰撞覆盖合同；供场景、射线和活动门读取，不分类角色或决定游戏状态。
use bevy::prelude::*; // 叶子合同仅依赖 ECS 与数学类型。
use std::{collections::BTreeMap, sync::Arc}; // 后台生成只读逐行覆盖，玩法查询不扫描所有格子。

#[derive(Clone, Debug, PartialEq)] // 真实完整单元的阶梯轮廓，不用凸包冒充凹角地面。
pub struct TerrainSupportRows {
    // 最粗覆盖选区向内细分后覆盖面积保持不变。
    pub origin: [f64; 2],              // 固定场景原点。
    pub spacing: [f64; 2],             // 最粗覆盖单元的世界格距。
    pub bounds: [Vec2; 2],             // 实际场景最小/最大边界，尾格必须裁切。
    pub rows: BTreeMap<i64, [i64; 2]>, // 格行 -> [最左格索引, 最右格的排他索引]。
} // 只由完整后台候选发布，不随当前玩家中心临时重算。

impl TerrainSupportRows {
    // 每次查询最多查三个相邻行，不遍历完整轮廓。
    pub fn contains(&self, point: Vec2) -> bool {
        // 仅判定真实覆盖，不改变活动资格半径。
        if !point.is_finite() || point.cmplt(self.bounds[0]).any() || point.cmpgt(self.bounds[1]).any() // 拒绝场景外和非法点。
            || self.spacing.iter().any(|value| !value.is_finite() || *value <= 0.0)
        {
            return false;
        } // 损坏合同不提供地面。
        let row = ((f64::from(point.y) - self.origin[1]) / self.spacing[1]).floor() as i64; // 有序行索引。
        [row.saturating_sub(1), row, row.saturating_add(1)]
            .into_iter()
            .any(|index| {
                // 邻行只处理f32世界投影落在共享边界的舍入。
                let Some([left, right]) = self.rows.get(&index).copied() else {
                    return false;
                }; // 阶梯空白无地面。
                if left >= right {
                    return false;
                } // 空行不能提供承托。
                let minimum = Vec2::new(
                    (self.origin[0] + left as f64 * self.spacing[0]) as f32,
                    (self.origin[1] + index as f64 * self.spacing[1]) as f32,
                ); // 与碰撞顶点相同的单次世界投影。
                let maximum = Vec2::new(
                    (self.origin[0] + right as f64 * self.spacing[0]) as f32,
                    (self.origin[1] + (index as f64 + 1.0) * self.spacing[1]) as f32,
                ); // 闭边允许相邻行共享端点。
                point.cmpge(minimum).all() && point.cmple(maximum).all() // 不靠epsilon外扩覆盖。
            }) // 实际行区间之外即无地面。
    } // 对数级小查询，不分配或触发加载。
} // 覆盖与形状采用同一个版本。

#[derive(Resource, Default, Clone, Debug)] // 缺省关闭，非 Clipmap 场景与隔离测试完全保持原行为。
pub struct TerrainCollisionRegion {
    // 场景运行时是该资源的唯一发布者。
    pub enabled: bool,                       // 仅自定义多层碰撞模式启用。
    pub revision: u64,                       // 已采用场景、配置、覆盖或玩家中心变化的版本。
    pub center: Vec2,                        // 发布者当前活动锚点 X/Z，而非显示或已采用任务的旧中心。
    pub max_distance_meters: f32,            // 已采用配置的最外活动半径。
    pub supported_bounds: Vec<(Vec2, Vec2)>, // 当前完整已安装碰撞块范围，最多 512 块。
    pub supported_polygon: Vec<Vec2>,        // 已采用碰撞网格的凸外环；空列表兼容既有矩形块合同。
    pub supported_rows: Option<Arc<TerrainSupportRows>>, // 规则网格阶梯近圆覆盖优先，空值兼容旧合同与GLB。
} // 结束共享区域合同。

impl TerrainCollisionRegion {
    pub fn support_polygon_contains(&self, point: Vec2) -> bool {
        if let Some(rows) = &self.supported_rows {
            return rows.contains(point);
        } // 新方案查真实格行，不用凸包填平阶梯凹角。
        // 必须使用已采用外环，不能用当前玩家中心重建一个尚无几何的圆。
        let polygon = &self.supported_polygon; // 发布者保证非空外环为有序凸多边形，无重复末点。
        if polygon.is_empty() {
            return true;
        } // 旧场景及旧矩形夹具维持原承托语义。
        if polygon.len() < 3 || !point.is_finite() {
            return false;
        } // 损坏外环和非法查询均不得提供承托。
        let origin = polygon[0].as_dvec2(); // f64 计算避免大场景坐标的 f32 叉积精度损失。
        let cross = |a: Vec2, b: Vec2| (a.as_dvec2() - origin).perp_dot(b.as_dvec2() - origin); // 扇形有向面积。
        let corner_area = cross(polygon[1], polygon[polygon.len() - 1]); // 发布者保留顶点零为真实凸角，允许其余边插入共线节点。
        if corner_area == 0.0 || !corner_area.is_finite() {
            return false;
        } // 退化外环不能作为完整地面。
        let orientation = corner_area.signum(); // 同时支持顺时针和逆时针发布者。
        if cross(polygon[1], point) * orientation < 0.0
            || cross(polygon[polygon.len() - 1], point) * orientation > 0.0
        {
            return false;
        } // 先排除凸扇形两条外侧射线。
        let mut lo = 1; // 凸多边形按顶点零分扇区，二分避免每个角色逐边扫描整个外环。
        let mut hi = polygon.len() - 1; // 最多 O(log N) 次叉积。
        while hi - lo > 1 {
            // 定位查询点所在扇形。
            let mid = (lo + hi) / 2; // 无分配的固定数组二分。
            if cross(polygon[mid], point) * orientation >= 0.0 {
                lo = mid;
            } else {
                hi = mid;
            } // 两种绕序共用相同判定。
        } // 扇形定位结束。
        let edge = polygon[hi].as_dvec2() - polygon[lo].as_dvec2(); // 真实碰撞外环的对应边。
        edge.perp_dot(point.as_dvec2() - polygon[lo].as_dvec2()) * orientation >= 0.0 // 外环边上允许承托，弦外空白不由块 AABB 冒充。
    } // 外环查询不包含活动资格，角色足迹可独立检查已采用地面。
    // 纯查询不触发加载、构建或补块。
    pub fn contains(&self, position: Vec3) -> bool {
        // 活动半径严格独立于保守外扩块。
        !self.enabled
            || position.xz().distance_squared(self.center) <= self.max_distance_meters.powi(2) // 水平距离不受角色高度影响。
    } // 结束范围判定。
    pub fn supports(&self, position: Vec3) -> bool {
        // 只有实际采用的碰撞区域才能恢复实体。
        !self.enabled
            || (self.contains(position)
                && self.support_polygon_contains(position.xz()) // 裁圆后 AABB 的四角不再有真实地面。
                && self.supported_bounds.iter().any(|&(min, max)| {
                    // 最多检查发布者限定的 512 块。
                    position.x >= min.x
                        && position.z >= min.y
                        && position.x <= max.x
                        && position.z <= max.y // 边界保守包含，不使用高度插值冒充物理。
                })) // 结束完整地面判断。
    } // 结束地面查询。
} // 结束共享资源查询。

#[cfg(test)] // 无窗口纯规则回归。
mod tests {
    // 保护严格半径与实际覆盖的区别。
    use super::*; // 使用正式合同。
    #[test] // 凹角必须无地面，不能用凸包或管理块包围盒填平阶梯。
    fn stepped_rows_reject_empty_corners_and_preserve_grid_edges() {
        // 对应圆形选完整格子的近圆外缘。
        let rows = TerrainSupportRows {
            // 十字形只用于明确区分真实行区间与凸包。
            origin: [0.0; 2],                       // 固定世界原点。
            spacing: [1.0; 2],                      // 一米一个完整覆盖格。
            bounds: [Vec2::ZERO, Vec2::splat(3.0)], // 限定场景边缘。
            rows: BTreeMap::from([(0, [1, 2]), (1, [0, 3]), (2, [1, 2])]), // 相邻行宽度不同形成凹角。
        }; // 凸包会错误放行部分角落，本合同不会。
        assert!(rows.contains(Vec2::splat(1.5))); // 中央实际有地面。
        assert!(rows.contains(Vec2::new(0.5, 1.0))); // 行边界由相邻宽行真实承托。
        assert!(rows.contains(Vec2::new(2.0, 3.0))); // 最后行闭边仍有地面。
        assert!(!rows.contains(Vec2::new(0.75, 0.75))); // 在凸包内但在实际阶梯外。
        assert!(!rows.contains(Vec2::new(2.25, 2.25))); // 对角空白同样拒绝。
        assert!(!rows.contains(Vec2::new(1.5, 3.01))); // 场景外拒绝。
        assert!(!rows.contains(Vec2::splat(f32::NAN))); // 非有限查询不得绕过保护。
        let region = TerrainCollisionRegion {
            // 即使旧包围盒覆盖凹角，也要先检查实际格行。
            enabled: true,             // 启用共享物理地面合同。
            center: Vec2::splat(1.5),  // 当前玩家位置。
            max_distance_meters: 0.25, // 活动范围与物理取整独立。
            supported_bounds: vec![(Vec2::ZERO, Vec2::splat(3.0))], // 模拟较大的管理块。
            supported_rows: Some(Arc::new(rows)), // 完整采用的真实覆盖。
            ..default()                // 旧凸外环不再参与新路径。
        }; // 不建立远处角色的额外碰撞区。
        assert!(region.support_polygon_contains(Vec2::new(0.5, 1.5))); // 物理格子确实存在。
        assert!(!region.supports(Vec3::new(0.5, 0.0, 1.5))); // 活动距离外仍不能恢复。
    } // 场景生成与角色查询使用相同的阶梯覆盖。

    #[test] // 场景末端不足整格时不能把取整部分当成场景。
    fn support_rows_clip_tail_cells_and_match_world_projection() {
        // 同时覆盖负原点和非整除世界格距。
        let rows = TerrainSupportRows {
            // 近似实际不规则场景尾格。
            origin: [-10.0, -20.0], // 世界固定原点。
            spacing: [0.3, 0.7],    // f64权威格距。
            bounds: [Vec2::new(-10.0, -20.0), Vec2::new(-9.25, -18.5)], // X末格只有0.15米。
            rows: BTreeMap::from([(0, [0, 3]), (1, [0, 3]), (2, [0, 3])]), // 逻辑格子可超出场景尾端。
        }; // 查询以真实场景边界裁剪。
        assert!(rows.contains(Vec2::new(-9.25, -18.5))); // 场景末点有地面。
        assert!(!rows.contains(Vec2::new(-9.2, -18.5))); // 逻辑末格场景外部分无地面。
        assert!(rows.contains(Vec2::new(
            (-10.0_f64 + 0.3) as f32,
            (-20.0_f64 + 0.7) as f32
        ))); // 与正式几何单次投影相同的边点。
    } // 不用额外epsilon扩活动或承托范围。
    #[test] // 已采用圆的凸多边形弦边不能由矩形包围盒角落补齐。
    fn polygon_support_rejects_aabb_corners_and_preserves_winding() {
        // 简单菱形精确验证圆边界查询合同。
        let mut region = TerrainCollisionRegion {
            // 独立已采用覆盖，不依赖物理插件。
            enabled: true,             // 启用承托约束。
            max_distance_meters: 10.0, // 活动资格大于测试外环，分离两种限制。
            supported_bounds: vec![(Vec2::splat(-2.0), Vec2::splat(2.0))], // 包围盒角落无实际几何。
            supported_polygon: vec![
                Vec2::new(2.0, 0.0),
                Vec2::new(0.0, 2.0),
                Vec2::new(-2.0, 0.0),
                Vec2::new(0.0, -2.0),
            ], // 逆时针凸环。
            ..default()                // 其余身份值不参与纯查询。
        }; // 构造完毕。
        for _ in 0..2 {
            // 两种绕序必须产生同一结果。
            assert!(region.supports(Vec3::ZERO)); // 中心有真实地面。
            assert!(region.supports(Vec3::new(1.0, 0.0, 1.0))); // 弦边包含。
            assert!(region.supports(Vec3::new(2.0, 0.0, 0.0))); // 圆多边形顶点包含。
            assert!(!region.supports(Vec3::new(1.5, 0.0, 1.5))); // 在 AABB 内但弦外，必须等待。
            assert!(!region.support_polygon_contains(Vec2::new(f32::NAN, 0.0))); // 非有限点不得绕过保护。
            region.supported_polygon.reverse(); // 下一次验证反向绕序。
        } // 绕序测试结束。
        region.center = Vec2::new(100.0, 0.0); // 传送只修改玩法中心，不能搬动旧碰撞外环。
        assert!(!region.supports(Vec3::new(100.0, 0.0, 0.0))); // 新落点必须等新候选采用。
    } // 结束真实外环回归。
    #[test] // 二分扇形接受边上的共线分割节点，不扫描整条环。
    fn polygon_support_handles_collinear_nodes_away_from_origin() {
        // 保持起始顶点为凸角。
        let region = TerrainCollisionRegion {
            // 独立多边形合同测试。
            supported_polygon: vec![
                Vec2::ZERO,
                Vec2::new(2.0, 0.0),
                Vec2::new(2.0, 1.0),
                Vec2::new(2.0, 2.0),
                Vec2::new(0.0, 2.0),
            ], // 右边界含共线接缝节点。
            ..default() // 外环查询不要求启用整个活动门。
        }; // 所有节点均为实际边界节点。
        assert!(region.support_polygon_contains(Vec2::new(2.0, 1.0))); // 共线节点本身包含。
        assert!(region.support_polygon_contains(Vec2::ONE)); // 内部点包含。
        assert!(!region.support_polygon_contains(Vec2::new(2.01, 1.0))); // 外部点拒绝。
    } // 结束共线节点回归。
    #[test] // 外扩地块不能扩大活动范围。
    fn padded_coverage_does_not_extend_activity() {
        // 包含边界与无承托检查。
        let region = TerrainCollisionRegion {
            enabled: true,
            revision: 1,
            center: Vec2::ZERO,
            max_distance_meters: 16.0,
            supported_bounds: vec![(Vec2::splat(-32.0), Vec2::splat(32.0))],
            supported_polygon: Vec::new(), // 旧矩形覆盖夹具不声明圆形裁边。
            supported_rows: None,          // 旧夹具继续使用矩形块语义。
        }; // 模拟保守整块覆盖。
        assert!(region.supports(Vec3::new(16.0, 900.0, 0.0))); // 高度不改变水平活动资格。
        assert!(!region.supports(Vec3::new(17.0, 0.0, 0.0))); // 即使块仍在，半径外不得恢复。
        assert!(TerrainCollisionRegion::default().supports(Vec3::splat(1_000.0))); // 非 Clipmap 场景不受限制。
    } // 结束外扩回归。
    #[test] // 首次进入与传送必须等实际地面。
    fn active_range_without_adopted_blocks_waits() {
        // 空覆盖不能冒充已就绪。
        let region = TerrainCollisionRegion {
            enabled: true,
            max_distance_meters: 64.0,
            ..default()
        }; // 半径有效但未采用地面。
        assert!(region.contains(Vec3::ZERO)); // 落点仍有活动资格。
        assert!(!region.supports(Vec3::ZERO)); // 尚无实体承托。
    } // 结束首帧回归。
} // 结束测试模块。

/// 缺少已采用承托时的逐刚体暂停原因，不表达角色或玩法分类。
#[derive(bevy::prelude::Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainActivityPaused { OutsideRange, AwaitingSupport }
