//! 地面吸附 ECS 组件：由生成方声明根节点离地偏移，由统一系统填入解算结果。

use bevy::prelude::*; // 引入 Component 派生宏等 Bevy ECS 基础类型。
use avian3d::prelude::LayerMask;

/// 应用对贴地请求的显式输入；不从碰撞层位猜测玩家、NPC或其它游戏身份。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroundSnapFilter {
    pub mask: LayerMask,
    pub source: crate::types::GroundSnapSource,
}

impl Default for GroundSnapFilter {
    fn default() -> Self {
        Self { mask: LayerMask::ALL, source: default() }
    }
}

/// 标记一个需要在真实物理地表上初始化的世界实体。
#[derive(Component, Debug, Clone, Copy, PartialEq)] // 组件可供 ECS 查询，调试界面和单元测试读取。
pub struct GroundSnap {
    // 保留一次吸附的配置、请求状态和最后解算高度。
    ground_offset: f32,     // 实体根节点相对地表的 Y 偏移，例如角色胶囊悬浮高度。
    pending: bool,          // true 表示系统需要在当前 X/Z 上方执行向下射线。
    snapped_y: Option<f32>, // 保存最后成功解算的根节点 Y，供掉落物悬浮等后续视觉系统复用。
    miss_reported: bool,    // 防止射线暂时未命中时每帧重复输出警告。
    preserve_surface_layer: bool, // 正式地图角色热重载时保留当前楼层，旧角色继续沿用高空吸附。
} // 结束地面吸附组件定义。

impl GroundSnap {
    // 提供生成、重新请求和读取解算结果的稳定 API。
    /// 创建一个等待首次地面吸附的组件。
    pub fn new(ground_offset: f32) -> Self {
        // 生成方只需指定根节点应该高出地表多少米。
        Self {
            // 初始状态会在地形碰撞就绪后自动执行一次射线。
            ground_offset,        // 保留不同类型实体各自的胶囊、几何中心或悬浮偏移。
            pending: true,        // 新生成实体必须至少执行一次吸附。
            snapped_y: None,      // 在射线命中前没有可用的真实地面高度。
            miss_reported: false, // 初次未命中时允许输出一条可诊断警告。
            preserve_surface_layer: false, // 兼容所有既有生成入口的最高地表行为。
        } // 完成待吸附组件初始化。
    } // 结束组件构造函数。

    /// 请求在实体当前 X/Z 位置重新计算地面高度。
    pub fn request(&mut self) {
        // 地形热更新或实体瞬移后可以显式再次调用。
        self.pending = true; // 恢复待解算状态，让统一系统在下一次 Update 执行。
        self.snapped_y = None; // 旧高度不能继续作为新位置的地面基准。
        self.miss_reported = false; // 新请求允许在真正无地面时再报告一次。
    } // 结束重新吸附请求。

    /// 返回当前是否仍在等待地面射线解算。
    pub fn is_pending(&self) -> bool {
        // 供统一系统快速跳过已经完成的实体。
        self.pending // 直接返回内部请求标志。
    } // 结束待解算状态读取。

    /// 返回最后一次成功吸附后的根节点世界 Y 坐标。
    pub fn snapped_y(&self) -> Option<f32> {
        // 掉落物可用该结果作为浮动动画的基线。
        self.snapped_y // 未命中地面前返回 None，调用方可保留原始高度。
    } // 结束已解算高度读取。

    /// 返回实体根节点相对真实地表的设计偏移。
    ///
    /// 地形编辑器采用新碰撞前用它判断抬高后的地面是否已经穿入角色，
    /// 只在确实需要上移时重新请求吸附，降低地面时仍允许角色自然下落。
    pub fn ground_offset(&self) -> f32 {
        // 统一贴地系统和开发期地形接管均只读同一偏移契约。
        self.ground_offset // 返回生成方声明的根节点离地距离。
    } // 结束稳定偏移读取。
    /// 关联角色随地形改变尺寸时，胶囊中心偏移同步改变，保留当前楼层与已完成状态。
    pub fn set_ground_offset(&mut self,offset:f32) {
        if offset.is_finite()&&offset>0.0 {self.ground_offset=offset;}
    }

    /// 正式地图角色已选择明确承托面，后续请求只在当前脚底附近找同一楼层。
    pub fn preserve_surface_layer(&mut self) {
        self.preserve_surface_layer = true;
    }

    /// 返回射线高度和范围；局部锚点绝不因上方桥面或屋顶改用最高地表。
    pub(crate) fn ray_window(&self, current_root_y: f32) -> (f32, f32) {
        if self.preserve_surface_layer {
            (current_root_y - self.ground_offset + 0.5, 2.0) // 仅修正附近承托面，远离地面交给正常下落。
        } else {
            (
                crate::constants::GROUND_RAY_START_Y,
                crate::constants::GROUND_RAY_DISTANCE,
            )
        }
    }

    /// 调用者已经拥有应保留的根高度时，原子采用该高度并阻止再次从高空吸附。
    pub fn resolve(&mut self, snapped_y: f32) {
        // 射线命中后原子更新请求状态与解算结果。
        self.pending = false; // 一次吸附完成后不再每帧执行物理射线。
        self.snapped_y = Some(snapped_y); // 保存真实根节点高度供后续系统复用。
        self.miss_reported = false; // 成功解算后清理旧的未命中诊断状态。
    } // 结束成功解算状态更新。

    pub(crate) fn should_report_miss(&self) -> bool {
        // 仅在当前请求首次未命中时返回 true。
        !self.miss_reported // 已报告过的持续未命中不再刷屏。
    } // 结束未命中报告判断。

    pub(crate) fn mark_miss_reported(&mut self) {
        // 记录当前请求已经输出过警告。
        self.miss_reported = true; // 后续帧继续安静重试，直到命中或重新请求。
    } // 结束未命中报告记录。
} // 结束 GroundSnap 方法实现。

#[cfg(test)] // 仅在单元测试构建中编译状态回归测试。
mod tests {
    // 验证构造、解算与重新请求的状态机。
    use super::GroundSnap; // 导入待测组件。

    #[test] // 每个新组件必须等待首次吸附。
    fn new_ground_snap_starts_pending() {
        // 验证生成方无需额外设置请求标志。
        let ground_snap = GroundSnap::new(1.25); // 用可辨识的偏移创建测试组件。
        assert!(ground_snap.is_pending()); // 新实体应该在下一次 Update 执行射线。
        assert_eq!(ground_snap.ground_offset(), 1.25); // 根节点偏移必须原样保留。
        assert_eq!(ground_snap.snapped_y(), None); // 尚未命中地面时不应伪造解算高度。
    } // 结束初始状态测试。

    #[test] // 地形热更新或瞬移后必须能重新解算。
    fn request_clears_previous_resolution() {
        // 验证已完成组件可恢复待吸附状态。
        let mut ground_snap = GroundSnap::new(0.5); // 创建一个掉落物风格的偏移配置。
        ground_snap.resolve(3.5); // 模拟射线命中后根节点位于 Y=3.5。
        assert!(!ground_snap.is_pending()); // 成功解算后应停止重复射线。
        assert_eq!(ground_snap.snapped_y(), Some(3.5)); // 成功高度应供后续视觉系统读取。
        ground_snap.request(); // 模拟实体瞬移到新 X/Z 后请求重新吸附。
        assert!(ground_snap.is_pending()); // 重新请求后必须恢复待解算。
        assert_eq!(ground_snap.snapped_y(), None); // 旧位置的高度不能泄漏到新位置。
    } // 结束重新请求状态测试。

    #[test]
    fn map_character_recheck_keeps_current_surface_layer() {
        let mut snap = GroundSnap::new(1.0);
        snap.preserve_surface_layer();
        snap.resolve(4.0);
        snap.request();
        assert_eq!(snap.ray_window(4.0), (3.5, 2.0)); // 原脚底Y=3，不能命中Y=6的桥面。
        assert_eq!(snap.ray_window(8.0), (7.5, 2.0)); // 已移动到新楼层的活体使用实际位置。
        assert_eq!(
            GroundSnap::new(1.0).ray_window(4.0).0,
            crate::constants::GROUND_RAY_START_Y
        );
    }
} // 结束 GroundSnap 单元测试模块。
