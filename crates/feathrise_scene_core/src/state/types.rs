//! 公共场景生命周期、加载阶段与事务身份；应用语义由各自私有代码决定。

/// 加载事务的展示归属；共享资源就绪门，应用与作者宿主分别提供自己的界面。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SceneTransitionPresentation {
    #[default]
    Application,
    Authoring,
}

/// 场景加载阶段，用于加载进度页显示。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LoadingStage {
    #[default] // 应用可在进入公共资产门前完成自己的前置准备，SDK不认识PAK或其他打包格式。
    ApplicationPreparation,
    CriticalAssets,
    SceneInit,
    Collision,
    SceneAssets,
    DynamicEntities,
    Complete,
    Failed,
}

/// 每次加载进度的唯一身份；相同地图与相同驻留token再次进入时也不能复用旧完成回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadingTransactionId(u64);

impl LoadingTransactionId {
    pub(super) fn next() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// 唯一运行状态机；具体地图身份仍由地图注册表与驻留token持有。
/// Idle并不表示游戏菜单，独立编辑器开始页和没有打开场景的工程也使用它。
#[derive(bevy::prelude::States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ScenePhase {
    #[default]
    Idle,
    Loading,
    Active,
}

impl ScenePhase {
    pub fn active_phases() -> &'static [Self] { &[Self::Active] }

    /// 缺少作者名称时只提供中性生命周期提示，游戏显示词由其自己的枚举提供。
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Idle => "空场景",
            Self::Loading => "加载中",
            Self::Active => "场景",
        }
    }
}
