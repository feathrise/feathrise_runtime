//! 公共加载事务、就绪证明与地图边界；游戏会话和死亡规则归应用。

use bevy::prelude::*; // Resource 等 ECS 常用类型

use crate::state::ScenePhase; // 唯一公共场景生命周期。
use crate::state::types::{LoadingStage, LoadingTransactionId, SceneTransitionPresentation}; // 阶段与展示归属均为跨界面的纯数据契约。

/// 作者空世界窗口模式；F12是否已被手动关闭由作者会话单独维护。
#[derive(Resource, Default)]
pub struct EditorStartup;

/// 请求一次公共场景生命周期切换；玩家重置、读档或菜单意图由应用按事务ID另行保存。
#[derive(Resource, Debug, Clone)]
pub struct PendingSceneTransition {
    pub target: ScenePhase,
    /// 明确要求丢弃驻留世代；普通地图切换和作者切页可恢复已有世代。
    pub fresh_world: bool,
    target_display_name: String,
    presentation: SceneTransitionPresentation,
    transaction: LoadingTransactionId,
}

impl PendingSceneTransition {
    pub fn new(target: ScenePhase) -> Self {
        Self::named(target, target.display_name())
    }

    /// 名称只用于显示；状态、请求ID及实际地图身份分别参与就绪校验。
    pub fn named(target: ScenePhase, target_display_name: impl Into<String>) -> Self {
        Self {
            target,
            fresh_world: false,
            target_display_name: normalized_display_name(target, target_display_name.into()),
            presentation: SceneTransitionPresentation::Application,
            transaction: LoadingTransactionId::next(),
        }
    }

    /// 作者地图同样走真实加载事务，只把展示交给作者工作台。
    pub fn editor_map(target: ScenePhase, target_display_name: impl Into<String>) -> Self {
        Self {
            presentation: SceneTransitionPresentation::Authoring,
            ..Self::named(target, target_display_name)
        }
    }

    pub fn presentation(&self) -> SceneTransitionPresentation { self.presentation }
    pub fn transaction(&self) -> LoadingTransactionId { self.transaction }

    pub fn with_fresh_world(mut self) -> Self {
        self.fresh_world = true;
        self
    }

    pub fn display_name(&self) -> &str { &self.target_display_name }

    /// 异步目录解析只补全显示名称，不能制造另一个请求或使应用意图失去事务身份。
    pub fn set_target_display_name(&mut self, name: impl Into<String>) {
        self.target_display_name = normalized_display_name(self.target, name.into());
    }
}

fn normalized_display_name(target: ScenePhase, name: String) -> String {
    if name.trim().is_empty() { target.display_name().to_owned() }
    else { name.trim().to_owned() }
}

/// 场景启动门控：地图碰撞与应用领域就绪前暂停世界模拟。
#[derive(Resource, Debug, Clone)]
pub struct SceneStartupGate {
    pub target: ScenePhase,
    pub collision_ready: bool,
    /// 当前地图声明的阻塞式大资源已经完成；未完成时加载遮罩继续覆盖玩法画面。
    pub scene_assets_ready: bool,
    /// 应用显式注册的领域已完成；纯地图没有应用领域时只检查几何与资产。
    pub application_ready: bool,
    /// 当前应用领域的确定失败；应用后续提供新证明可恢复。
    pub application_error: Option<String>,
    pub sub_maps_ready: bool, // 完整子地图几何由场景宿主确认，独立组合根默认无需等待。
    pub sub_maps_error: Option<String>,
    pub ready: bool,
}

impl SceneStartupGate {
    pub fn waiting_for_collision(target: ScenePhase) -> Self {
        Self {
            target,
            collision_ready: false,
            scene_assets_ready: true, // 默认地图没有额外大资源队列；需要分段加载的地图在 OnEnter 显式关闭。
            application_ready: false, // 完成门根据实际注册的应用域决定是否还需等待。
            application_error: None,
            sub_maps_ready: true,
            sub_maps_error: None,
            ready: false,
        }
    }
}

/// 地图发布的最低有效世界边界；跌出后的死亡、传送或其他处理由应用决定。
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct SceneFallBoundary {
    scene: Option<ScenePhase>, // 边界只属于发布它的场景，防止切图后沿用旧地图高度。
    lower_y: f32,             // 低于此世界Y即已离开全部合法地形；具体处置由应用决定。
}

impl SceneFallBoundary {
    /// 为指定有效场景原子发布一个有限世界Y边界。
    pub fn set_for_scene(&mut self, scene: ScenePhase, lower_y: f32) -> bool {
        if !lower_y.is_finite() {
            return false; // 损坏清单不得用NaN或无穷值关闭、错误改变边界判断。
        }
        self.scene = Some(scene); // 先登记场景身份，读取方必须同时匹配。
        self.lower_y = lower_y; // 同一主线程系统内更新边界，不暴露半完成状态。
        true // 返回成功供地图加载器把损坏输入升级为明确失败。
    }

    /// 仅在当前场景身份一致时返回地图边界。
    pub fn lower_y_for_scene(&self, scene: ScenePhase) -> Option<f32> {
        (self.scene == Some(scene)).then_some(self.lower_y) // 切图期间绝不复用上一地图阈值。
    }

    /// 离开或重新准备指定场景时清除旧边界。
    pub fn clear_for_scene(&mut self, scene: ScenePhase) {
        if self.scene == Some(scene) {
            self.scene = None; // 只清除调用方拥有的场景边界，不误删其他场景未来扩展。
        }
    }
}

/// 加载页进度状态：stage 表示当前阶段，completed/total 表示该阶段完成度。
#[derive(Resource, Debug, Clone)]
pub struct LoadingProgress {
    pub stage: LoadingStage,
    pub completed: usize,
    pub total: usize,
    pub label: String,
    /// 切换请求被消费后继续保留的目标地图显示名。
    target_display_name: String,
    presentation: SceneTransitionPresentation, // 请求消费后仍保留界面归属，直到地图完整就绪。
    transaction: LoadingTransactionId,
    target: Option<ScenePhase>,
    resident_completion: Option<(LoadingTransactionId, ScenePhase, crate::residency::ResidentMapToken)>,
}

impl LoadingProgress {
    /// 从场景切换请求初始化加载进度并捕获本次地图名称。
    pub fn for_transition(transition: &PendingSceneTransition) -> Self {
        Self {
            target: Some(transition.target),
            transaction: transition.transaction(),
            target_display_name: transition.display_name().to_owned(), // 名称快照贯穿完整加载遮罩生命周期。
            presentation: transition.presentation(), // 作者展示必须贯穿碰撞、资源与应用完成阶段。
            label: "正在准备场景资源...".to_owned(),
            ..Self::default() // 阶段、计数和初始提示继续使用统一默认值。
        }
    }

    /// 迟到切换请求出现时刷新名称快照，保证非标准入口也不会显示内部动态场景名。
    pub fn remember_transition(&mut self, transition: &PendingSceneTransition) {
        self.transaction = transition.transaction();
        self.target = Some(transition.target);
        self.resident_completion = None; // 迟到的新请求不能继承上一事务的暖恢复证明。
        self.target_display_name = transition.display_name().to_owned(); // 每次加载事务只采用当前请求名称。
        self.presentation = transition.presentation(); // 迟到请求的名称与展示模式一并采用。
    }

    /// 应用完成目录解析后更新提示，不重置已经建立的加载身份或暖恢复证明。
    pub fn set_target_display_name(&mut self, name: impl Into<String>) {
        self.target_display_name = normalized_display_name(self.target.unwrap_or(ScenePhase::Active), name.into());
    }

    pub fn presentation(&self) -> SceneTransitionPresentation {
        self.presentation
    }

    pub fn transaction(&self) -> LoadingTransactionId {
        self.transaction
    }

    /// 仅由完整驻留采用点调用；事务、状态和驻留世代共同证明本次恢复已经完成。
    pub fn complete_resident_restore(
        &mut self,
        transaction: LoadingTransactionId,
        target: ScenePhase,
        token: crate::residency::ResidentMapToken,
    ) -> bool {
        if self.transaction != transaction || self.target != Some(target) {
            return false; // 旧交接即使恢复了相同地图，也不能完成后来创建的加载事务。
        }
        self.resident_completion = Some((transaction, target, token));
        self.stage = LoadingStage::Complete;
        true
    }

    /// 完成回执还必须属于当前完整活动图；不能把UI的Complete标签或旧restored字段当证明。
    pub fn resident_restore_ready(
        &self,
        scene: ScenePhase,
        activity: &crate::residency::SceneResidencyActivity,
    ) -> bool {
        self.resident_completion.is_some_and(|(transaction, target, token)| {
            transaction == self.transaction && self.target == Some(target) && scene == target
                && activity.enabled && !activity.switching
                && activity.active == Some(token) && activity.restored == Some(token)
        })
    }

    /// 执行器与显示层共同消费同一完成证明，不能单凭Complete标签提前关闭加载界面。
    pub fn completion_ready(
        &self,
        scene: ScenePhase,
        gate: Option<&SceneStartupGate>,
        activity: Option<&crate::residency::SceneResidencyActivity>,
    ) -> bool {
        self.stage == LoadingStage::Complete
            && !activity.is_some_and(|activity| activity.switching)
            && match gate {
                Some(gate) => gate.ready && scene == gate.target,
                None => activity.is_some_and(|activity| self.resident_restore_ready(scene, activity)),
            }
    }

    /// 两种界面读取同一份真实阶段进度，避免编辑器维护第二套计时式加载进度。
    pub fn percent(&self) -> f32 {
        let fraction = if self.total == 0 {
            1.0
        } else {
            (self.completed as f32 / self.total as f32).clamp(0.0, 1.0)
        };
        let (start, end) = match self.stage {
            LoadingStage::ApplicationPreparation => (0.0, 20.0),
            LoadingStage::CriticalAssets => (20.0, 70.0),
            LoadingStage::SceneInit => (70.0, 78.0),
            LoadingStage::Collision => (78.0, 82.0),
            LoadingStage::SceneAssets => (82.0, 96.0),
            LoadingStage::DynamicEntities => (96.0, 100.0),
            LoadingStage::Complete => return 100.0,
            LoadingStage::Failed => return 0.0,
        };
        start + (end - start) * fraction
    }

    /// 返回当前加载事务的具体地图名，缺少事务快照时使用场景枚举兼容旧调用。
    pub fn target_display_name(&self, fallback_scene: ScenePhase) -> &str {
        if self.target_display_name.trim().is_empty() {
            fallback_scene.display_name() // 手工测试或旧入口未提供名称时保持既有可读提示。
        } else {
            &self.target_display_name // 编辑器动态地图正常返回清单display_name。
        }
    }
}

impl Default for LoadingProgress {
    fn default() -> Self {
        Self {
            stage: LoadingStage::ApplicationPreparation,
            completed: 0,
            total: 1,
            label: "正在准备场景资源...".to_string(),
            target_display_name: String::new(), // 尚无切换请求时不猜测目标地图。
            presentation: SceneTransitionPresentation::Application, // 未提供请求的旧入口继续兼容应用加载界面。
            transaction: LoadingTransactionId::next(),
            target: None,
            resident_completion: None,
        }
    }
}

#[cfg(test)] // 公共启动门默认语义测试不创建窗口或玩法场景。
mod tests {
    use super::*; // 访问启动门构造函数和地图状态。
    use crate::residency::{ResidentMapToken, SceneResidencyActivity};

    fn restored_activity(token: ResidentMapToken) -> SceneResidencyActivity {
        SceneResidencyActivity { enabled: true, switching: false, active: Some(token), restored: Some(token) }
    }

    #[test]
    fn progress_captures_request_identity_and_name_changes_do_not_replace_it() {
        let mut request = PendingSceneTransition::named(ScenePhase::Active, "待解析地图");
        let mut progress = LoadingProgress::for_transition(&request);
        assert_eq!(progress.transaction(), request.transaction());
        request.set_target_display_name("目录名称");
        progress.set_target_display_name(request.display_name());
        assert_eq!(progress.transaction(), request.transaction());
        assert_eq!(progress.target_display_name(ScenePhase::Active), "目录名称");
    }

    #[test]
    fn later_request_to_same_map_rejects_previous_resident_receipt() {
        let first = PendingSceneTransition::named(ScenePhase::Active, "相同地图");
        let mut progress = LoadingProgress::for_transition(&first);
        let token = ResidentMapToken(7);
        assert!(progress.complete_resident_restore(first.transaction(), ScenePhase::Active, token));
        assert!(progress.resident_restore_ready(ScenePhase::Active, &restored_activity(token)));
        let later = PendingSceneTransition::named(ScenePhase::Active, "相同地图");
        progress.remember_transition(&later);
        assert_ne!(first.transaction(), later.transaction());
        assert!(!progress.complete_resident_restore(first.transaction(), ScenePhase::Active, token));
        // 显示层遗留的Complete值不能代替当前事务、目标和驻留token的采用证明。
        progress.stage = LoadingStage::Complete;
        assert!(!progress.resident_restore_ready(ScenePhase::Active, &restored_activity(token)));
    }

    #[test]
    fn resident_receipt_requires_current_target_and_complete_active_token() {
        let request = PendingSceneTransition::new(ScenePhase::Active);
        let mut progress = LoadingProgress::for_transition(&request);
        let token = ResidentMapToken(7);
        assert!(!progress.complete_resident_restore(request.transaction(), ScenePhase::Loading, token));
        assert!(progress.complete_resident_restore(request.transaction(), ScenePhase::Active, token));
        assert!(!progress.resident_restore_ready(ScenePhase::Loading, &restored_activity(token)));
        for mismatch in 0..4 {
            let mut activity = restored_activity(token);
            match mismatch {
                0 => activity.enabled = false,
                1 => activity.switching = true,
                2 => activity.active = Some(ResidentMapToken(8)),
                _ => activity.restored = Some(ResidentMapToken(8)),
            }
            assert!(!progress.resident_restore_ready(ScenePhase::Active, &activity));
        }
        assert!(progress.resident_restore_ready(ScenePhase::Active, &restored_activity(token)));
    }

    #[test]
    fn editor_presentation_survives_request_consumption_and_does_not_leak_to_game() {
        let editor = PendingSceneTransition::editor_map(ScenePhase::Active, "作者地图");
        let mut progress = LoadingProgress::for_transition(&editor);
        assert!(!editor.fresh_world); // 作者切图不隐式丢弃已有驻留世代。
        assert_eq!(progress.presentation(), SceneTransitionPresentation::Authoring);
        assert_eq!(
            progress.target_display_name(ScenePhase::Active),
            "作者地图",
        );
        let game = PendingSceneTransition::named(ScenePhase::Active, "运行地图");
        progress.remember_transition(&game);
        assert_eq!(progress.presentation(), SceneTransitionPresentation::Application); // 下次传送、读档不继承编辑器展示。
        assert_eq!(
            progress.target_display_name(ScenePhase::Active),
            "运行地图"
        );
    }

    #[test] // 动态场景的内部枚举名不得覆盖编辑器地图清单中的实际名称。
    fn named_transition_keeps_concrete_dynamic_map_name() {
        let transition = PendingSceneTransition::named(
            ScenePhase::Active, // 多张编辑器地图共享同一运行时状态。
            "  新建地图  ",             // 模拟清单名称并验证统一清理首尾空白。
        ); // 构造携带具体地图名称的正式切换请求。
        let progress = LoadingProgress::for_transition(&transition); // 模拟LoadingScene消费请求前保存名称。

        assert_eq!(transition.display_name(), "新建地图"); // 请求应返回清理后的作者名称。
        assert_eq!(
            progress.target_display_name(ScenePhase::Active), // 请求移除后仍从进度资源读取名称。
            "新建地图", // 加载后续阶段不得退回“动态地图”。
        ); // 完成动态地图名称贯穿断言。
    }

    #[test] // 空显示名必须回退场景稳定名称，避免生成空白加载标题。
    fn empty_transition_name_uses_scene_fallback() {
        let transition = PendingSceneTransition::named(
            ScenePhase::Active, // 使用目录驱动玩法场景验证兜底语义。
            "   ",                   // 模拟损坏清单或旧存档中的空名称。
        ); // 构造时应立即把空名称替换为枚举名称。

        assert_eq!(transition.display_name(), "场景"); // UI永远获得非空可读名称。
    }

    #[test] // 公共启动门构造时必须默认放行场景资产，具体地图随后才能按需登记额外工作。
    fn startup_gate_defaults_scene_assets_to_ready() {
        let gate = SceneStartupGate::waiting_for_collision(ScenePhase::Active);
        assert!(!gate.collision_ready); // 地形碰撞仍需地图系统显式放行。
        assert!(gate.scene_assets_ready); // 默认不应在地图系统登记额外工作前提前关闭资产门。
        assert!(!gate.application_ready); // 必须由实际装配的完成门检查。
        assert!(gate.application_error.is_none()); // 未开始准备不冒充确定失败。
        assert!(!gate.ready); // 三类启动条件未完成前场景整体保持未就绪。
    } // 完成默认大资源门回归。

    #[test] // 地图死亡边界必须携带场景身份并拒绝非有限输入。
    fn fall_death_boundary_never_leaks_across_scenes() {
        let mut boundary = SceneFallBoundary::default(); // 初始没有地图发布的覆盖值。
        assert!(
            boundary
                .lower_y_for_scene(ScenePhase::Active)
                .is_none()
        ); // 未发布时由角色域使用兼容阈值。
        assert!(boundary.set_for_scene(ScenePhase::Active, -96.0)); // 动态地图发布高度量程下方边界。
        assert_eq!(
            boundary.lower_y_for_scene(ScenePhase::Active), // 同一场景读取正式边界。
            Some(-96.0), // 合法深河床不会被-20米旧阈值杀死。
        ); // 完成同场景读取断言。
        assert!(
            boundary
                .lower_y_for_scene(ScenePhase::Idle)
                .is_none()
        ); // 静态地图不能继承动态地图阈值。
        assert!(!boundary.set_for_scene(ScenePhase::Active, f32::NAN)); // 损坏边界必须拒绝。
        assert_eq!(
            boundary.lower_y_for_scene(ScenePhase::Active), // 拒绝后仍读取上一完整值。
            Some(-96.0), // 失败写入不能破坏当前有效边界。
        ); // 完成原子失败断言。
        boundary.clear_for_scene(ScenePhase::Active); // 离开动态地图时清理所有权。
        assert!(
            boundary
                .lower_y_for_scene(ScenePhase::Active)
                .is_none()
        ); // 下一张图必须重新发布。
    } // 完成地图死亡边界回归。
} // 结束公共加载资源测试。
