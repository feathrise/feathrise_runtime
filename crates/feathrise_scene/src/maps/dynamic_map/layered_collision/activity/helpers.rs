//! 活动门纯规则：区域与足迹承托判断，以及独占平移锁位运算。
use super::*; // 仅使用父域引入的数学和合同类型，不访问 ECS 世界。
pub(super) fn activity_reason(
    region: &TerrainCollisionRegion,
    position: Vec3,
    cached: Option<usize>,
) -> (Option<TerrainActivityPaused>, Option<usize>) {
    // 纯规则便于无窗口测试。
    if !region.enabled {
        return (None, None);
    } // 旧模式完全旁路。
    if !region.contains(position) {
        return (Some(TerrainActivityPaused::OutsideRange), None);
    } // 不用外扩支持块扩大活动范围。
    if !region.support_polygon_contains(position.xz()) {
        // 已采用圆边界独立于实时玩家中心和旧块缓存。
        return (Some(TerrainActivityPaused::AwaitingSupport), None); // 不能把裁掉的块角当成已就绪地面。
    } // 先验真实外环，再允许缓存 AABB 快路径。
    let contains = |&(min, max): &(Vec2, Vec2)| {
        position.x >= min.x && position.z >= min.y && position.x <= max.x && position.z <= max.y
    }; // 检查当前已采用世界边界。
    let block = cached
        .filter(|&index| region.supported_bounds.get(index).is_some_and(contains))
        .or_else(|| region.supported_bounds.iter().position(contains)); // 常态只检查一个块，跨块才查找。
    (
        block
            .is_none()
            .then_some(TerrainActivityPaused::AwaitingSupport),
        block,
    ) // 有范围资格但无地面时等待。
} // 结束纯活动判定。

pub(super) fn footprint_supported(
    region: &TerrainCollisionRegion,
    position: Vec3,
    half: Vec2,
    cached: Option<usize>,
) -> bool {
    // 同块常态 O(1)，跨边才逐角复核实际采用块。
    let mut min = position.xz() - half; // 足迹使用水平包围盒，与实体高度无关。
    let mut max = position.xz() + half; // 地图边界不是暂时缺少的碰撞覆盖。
    if let Some([lo, hi]) = map_bounds(region) {
        // 仅实际地图边界允许裁掉足迹，不能拿已加载块边缘裁切。
        if !position.is_finite() || position.xz().cmplt(lo).any() || position.xz().cmpgt(hi).any() {
            // 图外中心没有承托，不伪造可恢复状态。
            return false; // 不自动传送到边内。
        } // 中心位于地图内才能检查其地图内足迹。
        min = min.max(lo); // 左下图外部分永远不生成，不进入等待条件。
        max = max.min(hi); // 右上同理，剩余四角仍需真实已采用覆盖。
    } // 没有地图边界的旧合同维持原样。
    let corners = [min, max, Vec2::new(min.x, max.y), Vec2::new(max.x, min.y)]; // 凸外环包含四角才包含整个矩形足迹。
    if !corners
        .iter()
        .all(|&point| region.support_polygon_contains(point))
    {
        return false;
    } // 缓存块范围再大，也不能绕过裁圆后的真实边界。
    if cached
        .and_then(|index| region.supported_bounds.get(index))
        .is_some_and(|&(lo, hi)| min.cmpge(lo).all() && max.cmple(hi).all())
    {
        return true;
    } // 整个足迹落在上一支持块时不用四次查找。
    corners.iter().all(|point| {
        region
            .supported_bounds
            .iter()
            .any(|&(lo, hi)| point.cmpge(lo).all() && point.cmple(hi).all())
    }) // 边缘可跨块，外扩支持足迹不扩大中心的活动资格。
} // 结束足迹承托判断。

fn map_bounds(region: &TerrainCollisionRegion) -> Option<[Vec2; 2]> {
    // 仅分层已采用地面提供权威地图边界。
    let rows = region
        .enabled
        .then_some(region.supported_rows.as_ref())
        .flatten()?; // GLB/关闭模式完全旁路。
    let [lo, hi] = rows.bounds; // 这是整张地图范围，不是当前LOD覆盖包围盒。
    (lo.is_finite() && hi.is_finite() && lo.cmplt(hi).all()).then_some([lo, hi]) // 无效合同不能提供放宽检查。
} // 常数成本，不读取高度或触发后台任务。

pub(super) fn footprint_crosses_map_edge(
    region: &TerrainCollisionRegion,
    position: Vec3,
    half: Vec2,
) -> bool {
    // 地图边缘历史暂停不能因旧缓存命中而跳过新规则。
    map_bounds(region).is_some_and(|[lo, hi]| {
        (position.xz() - half).cmplt(lo).any() || (position.xz() + half).cmpgt(hi).any()
    }) // 固定两次向量比较，不扫描地图。
} // 仅暂停且足迹跨真实地图边界者需要重新检查。

pub(super) fn constrain_map_velocity(
    region: &TerrainCollisionRegion,
    position: Vec3,
    mut velocity: Vec3,
    dt: f32,
) -> Vec3 {
    // 物理前阻止本步跨出地图的分量。
    let Some([lo, hi]) = map_bounds(region) else {
        return velocity;
    }; // 非分层行为不变。
    if !position.is_finite() || !velocity.is_finite() || !dt.is_finite() || dt <= 0.0 {
        return velocity;
    } // 无效输入交给原承托保护。
    let predicted = position + velocity * dt; // 与承托门使用同一个预测时长。
    for (axis, horizontal) in [(0, 0), (2, 1)] {
        // 仅X/Z，绝不限制正常跳跃或重力。
        if (velocity[axis] < 0.0 && predicted[axis] < lo[horizontal])
            || (velocity[axis] > 0.0 && predicted[axis] > hi[horizontal])
        {
            // 分轴保留沿边滑动与退回方向。
            velocity[axis] = 0.0; // 不添加平移锁，下一帧的向内输入可以立即生效。
        } // 零向外分量避免浮点投影把中心推进地图外。
    } // 固定两轴运算。
    velocity // 保留其他系统已写入的其余速度分量。
} // 不创建碰撞体、不写Transform、不修改活动范围。

pub(super) fn constrain_map_prediction(
    region: &TerrainCollisionRegion,
    position: Vec3,
    mut predicted: Vec3,
) -> Vec3 {
    // 修复旧等待缓存中的图外目标。
    let Some([lo, hi]) = map_bounds(region) else {
        return predicted;
    }; // GLB与尚无地面版本不改预测。
    if position.is_finite()
        && predicted.is_finite()
        && position.xz().cmpge(lo).all()
        && position.xz().cmple(hi).all()
    {
        // 只为图内实体裁预测，不替图外中心伪造承托。
        predicted.x = predicted.x.clamp(lo.x, hi.x); // 图内目标未变，仍可检查真实缺页。
        predicted.z = predicted.z.clamp(lo.y, hi.y); // 对角旧预测同时裁两轴。
    } // 不修改高度或实体真实位置。
    predicted // 后续完整足迹仍查已采用行和碰撞块。
} // 允许历史边缘等待在下一次检查解除。

pub(super) fn lock_translation(axes: LockedAxes) -> LockedAxes {
    // 平移是本域唯一拥有的锁位。
    LockedAxes::from_bits(axes.to_bits() | LockedAxes::TRANSLATION_LOCKED.to_bits()) // 原有旋转锁不变。
} // 结束追加锁。
pub(super) fn restore_translation(current: LockedAxes, previous: LockedAxes) -> LockedAxes {
    // 归还进入前的平移约束。
    let mask = LockedAxes::TRANSLATION_LOCKED.to_bits(); // 只覆盖平移三位。
    LockedAxes::from_bits((current.to_bits() & !mask) | (previous.to_bits() & mask)) // 保留外部当下旋转锁和原有平移锁。
} // 结束恢复锁。
