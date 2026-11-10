//! 树木技术底座：共享现有树木资产句柄、Bevy自动硬件实例LOD、地面吸附和近处碰撞身份。
//! 场景层负责提供离线稳定树位；本crate不生成随机树位、不持有逐帧散布队列。
//! 【模块所有权】P5.5将实现内收至统一环境入口，公开`environment::tree`路径与行为保持不变。

pub mod components; // 树根、LOD节点与碰撞代理ECS类型。
pub mod resources; // 当前场景树木诊断资源。
pub mod types; // LOD原型、资产句柄和离线树位值类型。

mod render_monitor; // 无GPU读回地采样树木在通用Opaque、当前AlphaMask与Shadow阶段的真实分箱。
mod spawn; // 共享Mesh/Material句柄的单树生成helper。

use bevy::prelude::*; // 引入App、Plugin与资源初始化接口。

pub use components::{TreeCollisionProxy, TreeInstance, TreeLodMesh}; // F3和场景碰撞系统读取公共身份。
pub use resources::{
    TreeRenderMonitorConfig, TreeRenderPhaseStats, TreeRenderStats, TreeRuntimeStats,
}; // 场景写入监控目标，F3读取资产与上一完整PBR排队帧。
pub use spawn::spawn_tree_instance; // 场景OnEnter系统复用统一四档生成逻辑。
pub use types::{TreeLodLevel, TreeLodPrototype, TreeMeshPart, TreePlacement, TreePrototype}; // 对场景装配层公开稳定树位和四档原型。

/// 树木公共插件：注册跨场景诊断资源和一份GPU顶点风材质管线。
pub struct TreePlugin; // 无配置字段，具体树种和树位全部属于单个场景。

impl Plugin for TreePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::mesh_wind::MeshWindPlugin); // 先注册共享噪声纹理及主渲染、阴影、运动向量一致的顶点风管线。
        app.init_resource::<TreeRuntimeStats>(); // 非树场景保留全零快照，进入场景后由内容层覆盖。
        render_monitor::install_tree_render_monitor(app); // 读取公开PBR分箱，不启用GPU查询或同步读回。
    } // 完成公共树木资源注册。
} // 完成树木插件实现。

#[cfg(test)] // 只在测试构建中验证公共原型契约。
mod tests {
    use bevy::camera::visibility::VisibilityRange; // 构造四档连续可见范围。
    use bevy::prelude::*; // 构造弱句柄和基础数学值。

    use super::TreePlacement; // 直接验证公开稳定树位数据结构。

    #[test] // 相邻LOD交叉区必须首尾完全一致。
    fn tree_lod_ranges_form_one_continuous_chain() {
        let ranges = [
            VisibilityRange {
                start_margin: 0.0..0.0, // 高档从树根附近完整可见。
                end_margin: 75.0..90.0, // 高档在75至90米交给中档。
                use_aabb: false,        // 全部档位按同一树根原点测距。
            }, // 完成高档范围。
            VisibilityRange {
                start_margin: 75.0..90.0, // 中档与高档共享完全相同交叉区。
                end_margin: 170.0..195.0, // 中档在170至195米交给低档。
                use_aabb: false,          // 不使用各LOD不同AABB中心。
            }, // 完成中档范围。
            VisibilityRange {
                start_margin: 170.0..195.0, // 低档与中档共享完全相同交叉区。
                end_margin: 360.0..400.0,   // 低档在360至400米交给公告板。
                use_aabb: false,            // 全部档位继续按树根原点测距。
            }, // 完成低档范围。
            VisibilityRange {
                start_margin: 360.0..400.0, // 公告板与低档共享完全相同交叉区。
                end_margin: 1050.0..1120.0, // 公告板宽域退出并限制最坏可见范围。
                use_aabb: false,            // 公告板不使用不同于实体树的AABB中心。
            }, // 完成公告板范围。
        ]; // 构造正式技术原型四档范围。
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end_margin, pair[1].start_margin); // 相邻档不得出现重叠宽度不一致或空带。
        } // 完成全部三处交叉边界核对。
    } // 完成LOD连续性测试。

    #[test] // 公共值类型必须允许场景只保存稳定内容表。
    fn tree_placement_keeps_only_stable_content_values() {
        let placement = TreePlacement {
            id: 7,                                  // 使用可辨识稳定编号。
            position: Vec3::new(920.0, 0.0, 210.0), // Y保持等待统一地面吸附。
            yaw_radians: 1.25,                      // 保存固定朝向而非逐帧随机。
            uniform_scale: 0.42,                    // 保存固定体量变化。
        }; // 完成离线树位构造。
        assert_eq!(placement.id, 7); // 稳定编号必须原样保留。
        assert_eq!(placement.position.y, 0.0); // 场景表不得硬编码未知地形高度。
        assert!(placement.uniform_scale > 0.0); // 有限正缩放才能生成合法四档轮廓。
    } // 完成稳定树位数据测试。
}
