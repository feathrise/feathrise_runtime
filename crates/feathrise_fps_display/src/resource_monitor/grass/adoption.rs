//! F3叶片解析密度采用状态的中文短报告。
//! 【功能设计】面板只解释后端真实窗口身份，不据此替项目宣布最终画面验收或切换正式迁移状态。

use feathrise_environment::vegetation::{
    VegetationBladeAdoptionPhase, VegetationBladeAdoptionStatus,
}; // 公共后端提供内容无关阶段和真实窗口分子/分母。

/// 格式化当前分页可见窗口的解析密度采用证据。
pub(super) fn format_blade_adoption_report(
    has_visible_windows: bool, // 当前分页必须先存在对应profile窗口。
    status: Option<&VegetationBladeAdoptionStatus>, // 旧插件组合缺少资源时安全显示直接密度。
) -> String {
    if !has_visible_windows {
        return "未启用".to_string(); // 离图后不展示全局资源可能保留的上一帧状态。
    }
    let Some(status) = status else {
        return "状态资源未安装（直接密度资产）".to_string(); // 独立旧样例仍可只安装GPU绘制插件。
    };
    let phase = match status.phase() {
        VegetationBladeAdoptionPhase::Unbound => "未绑定（直接密度资产）", // HN-02等未迁移地图保持原句柄。
        VegetationBladeAdoptionPhase::FallbackRetained => {
            "等待解析布局（严格/上一有效密度托底）" // 初次加载和换版都不提前切换。
        }
        VegetationBladeAdoptionPhase::Validating => {
            "GPU逐字节校验中（严格/上一有效密度托底）" // 候选在途仍不可见。
        }
        VegetationBladeAdoptionPhase::CurrentLayoutAdopted => "当前解析布局已采用", // 仅全部可见窗口身份匹配。
        VegetationBladeAdoptionPhase::RejectedKeepingFallback => {
            "候选拒绝（严格/上一有效密度托底）" // 明确区分失败与仍在等待。
        }
    };
    format!(
        "{phase} | revision {} | 层 {} | 采用窗 {}/绑定窗 {}",
        status.active_revision(),      // 记录当前活动布局代次。
        status.bound_layer_count(),    // 同层四档窗口只计一个层。
        status.adopted_window_count(), // 当前版本真正采用窗口数。
        status.bound_window_count(),   // 当前世界全部显式绑定窗口数。
    ) // 形成可供截图和严格烟雾断言的稳定短行。
}
