use bevy::prelude::*;

/// FPS与F3共用的应用字体；宿主在Startup前提供句柄，默认值不选择或加载游戏资产路径。
#[derive(Resource, Default, Clone)]
pub struct FpsDisplayFont(pub Handle<Font>);

#[derive(Resource)]
pub(super) struct FpsRefreshTimer(pub Timer);

#[cfg(any(feature = "dlss_frame_generation", feature = "fsr3_frame_generation"))] // 任一帧生成构建保存固定大小采样状态。
#[derive(Default)] // Local自动初始化，首个有效计数只建立基线。
pub(super) struct PresentationFpsSample {
    // 不保存历史数组，不随运行时长增长。
    pub baseline: Option<(f64, u64, super::types::PresentationIdentity)>, // 起点时间、实际计数及后端/交换链/尺寸身份。
    pub last_observed: Option<(f64, u64)>, // 每帧检测计数倒退或长时间暂停，不能只在UI刷新时检查。
    pub fps: Option<f64>,                  // 最近完整半秒区间的呈现速率；无有效区间时为空。
} // PresentationFpsSample结束。

#[derive(Resource)]
pub(super) struct ResourceMonitorRefreshTimer(pub Timer);

/// F3 资源监控的五个顶层功能组。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ResourceMonitorGroup {
    #[default]
    Overview, // 通用世界总览不显示二级页签。
    OrdinaryGrass,     // 普通草保持独立顶层入口。
    WetlandVegetation, // 湿地草与同类植被素材的诊断入口。
    Crops,             // 麦田和杂粮田共用作物二级页签。
    Landscape,         // 树木、灌木和岩石共用景观物二级页签。
}

impl ResourceMonitorGroup {
    pub(super) const ALL: [Self; 5] = [
        Self::Overview,          // 第一组保持打开 F3 时最容易理解的世界总览。
        Self::OrdinaryGrass,     // 第二组独占普通草 GPU 技术诊断。
        Self::WetlandVegetation, // 第三组承载湿地植被诊断。
        Self::Crops,             // 第四组汇总两类全图作物。
        Self::Landscape,         // 第五组汇总固定景观内容。
    ]; // 固定顺序驱动顶层 UI 和结构测试。

    /// 返回顶层按钮短中文名。
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Overview => "总览",              // 通用世界资源。
            Self::OrdinaryGrass => "普通草",       // 零号 GPU 叶片 profile。
            Self::WetlandVegetation => "湿地植被", // 按当前素材预设读取诊断。
            Self::Crops => "作物",                 // 麦田和杂粮田。
            Self::Landscape => "景观物",           // 树木、灌木和岩石。
        } // 完成五组穷尽匹配。
    } // 完成顶层标签转换。

    /// 返回点击顶层组时默认打开的叶子页。
    pub(super) const fn default_page(self) -> ResourceMonitorPage {
        match self {
            Self::Overview => ResourceMonitorPage::Overview, // 总览组只有总览页。
            Self::OrdinaryGrass => ResourceMonitorPage::OrdinaryGrass, // 普通草组只有普通草页。
            Self::WetlandVegetation => ResourceMonitorPage::WetlandGrass, // 默认显示仍保留随包预设的湿地草。
            Self::Crops => ResourceMonitorPage::Wheat,       // 作物组默认从麦田开始。
            Self::Landscape => ResourceMonitorPage::Trees,   // 景观物组默认从树木开始。
        } // 完成五组默认叶子页映射。
    } // 完成默认页转换。

    /// 返回当前组是否需要显示二级页签行。
    pub(super) const fn has_subtabs(self) -> bool {
        matches!(
            self,
            Self::WetlandVegetation | Self::Crops | Self::Landscape
        ) // 故道植被、作物和景观物三个多叶子组占用第二行高度。
    } // 完成二级行显示判断。
}

/// F3 资源监控的十个互斥叶子分页。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ResourceMonitorPage {
    #[default]
    Overview, // 总览只采样通用世界、玩法和资产计数。
    OrdinaryGrass, // 普通草只格式化零号GPU植被profile。
    Reeds,         // 芦苇只格式化二号GPU植被profile。
    WetlandGrass,  // 湿地草在HN-02会话内复用零号GPU植被profile并独立显示。
    SandbarGrass,  // 沙洲矮草在HN-02会话内复用三号GPU植被profile并独立显示。
    Wheat,         // 麦田只格式化一号GPU植被profile。
    MixedGrain,    // 杂粮田只格式化三号 GPU 植被 profile。
    Trees,         // 树木只采样树根、LOD、资产成本和 PBR 分箱。
    Shrubs,        // 灌木只采样固定实例、四档 LOD、风材质和源整包成本。
    Rocks,         // 岩石只采样固定实例、四档 LOD、近距碰撞和源整包成本。
}

impl ResourceMonitorPage {
    pub(super) const ALL: [Self; 10] = [
        Self::Overview,      // 第一页保持打开F3时最容易理解的世界总览。
        Self::OrdinaryGrass, // 第二页独占普通草技术诊断。
        Self::Reeds,         // 第三页独占HN-02芦苇技术诊断。
        Self::WetlandGrass,  // 第四页独占HN-02受控湿地草技术诊断。
        Self::SandbarGrass,  // 第五页独占HN-02西滩沙洲矮草技术诊断。
        Self::Wheat,         // 第六页独占麦田技术诊断。
        Self::MixedGrain,    // 第七页独占杂粮田技术诊断。
        Self::Trees,         // 第八页独占树木技术与性能诊断。
        Self::Shrubs,        // 第九页独占灌木技术与性能诊断。
        Self::Rocks,         // 第十页独占岩石技术与性能诊断。
    ]; // 固定顺序同时驱动正文生成和结构测试。

    pub(super) const SUBTABS: [Self; 8] = [
        Self::Reeds,        // 故道植被组第一叶子页。
        Self::WetlandGrass, // 故道植被组第二叶子页。
        Self::SandbarGrass, // 故道植被组第三叶子页。
        Self::Wheat,        // 作物组第一叶子页。
        Self::MixedGrain,   // 作物组第二叶子页。
        Self::Trees,        // 景观物组第一叶子页。
        Self::Shrubs,       // 景观物组第二叶子页。
        Self::Rocks,        // 景观物组第三叶子页。
    ]; // 单页顶层组不重复生成无意义二级按钮。

    /// 返回分页按钮和内容标题共用的短中文名。
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Overview => "总览",         // 通用世界资源。
            Self::OrdinaryGrass => "普通草",  // 零号GPU植被profile。
            Self::Reeds => "芦苇",            // 二号GPU植被profile。
            Self::WetlandGrass => "湿地草",   // HN-02会话内零号GPU植被profile。
            Self::SandbarGrass => "沙洲矮草", // HN-02会话内三号GPU植被profile。
            Self::Wheat => "麦田",            // 一号GPU植被profile。
            Self::MixedGrain => "杂粮田",     // 三号 GPU 植被 profile。
            Self::Trees => "树木",            // 树木 LOD 与性能。
            Self::Shrubs => "灌木",           // 灌木固定实例与性能。
            Self::Rocks => "岩石",            // 岩石固定实例与性能。
        } // 完成十页穷尽匹配。
    } // 完成分页标签转换。

    /// 返回当前叶子页所属顶层组。
    pub(super) const fn group(self) -> ResourceMonitorGroup {
        match self {
            Self::Overview => ResourceMonitorGroup::Overview, // 总览归总览组。
            Self::OrdinaryGrass => ResourceMonitorGroup::OrdinaryGrass, // 普通草归独立组。
            Self::Reeds | Self::WetlandGrass | Self::SandbarGrass => {
                ResourceMonitorGroup::WetlandVegetation
            } // HN-02三类叶片植被归故道植被组。
            Self::Wheat | Self::MixedGrain => ResourceMonitorGroup::Crops, // 两种田地归作物组。
            Self::Trees | Self::Shrubs | Self::Rocks => ResourceMonitorGroup::Landscape, // 三类固定景观归景观物组。
        } // 完成十页所属组穷尽映射。
    } // 完成页到组转换。
}

#[derive(Resource, Default)]
pub(super) struct ResourceMonitorState {
    pub visible: bool,
    pub force_refresh: bool,
    pub group: ResourceMonitorGroup, // 顶层组决定二级页签行的内容。
    pub page: ResourceMonitorPage,   // 只刷新当前页，隐藏页不遍历ECS也不构建长字符串。
}

impl ResourceMonitorState {
    /// 返回当前F3状态是否需要通用Mesh精确RenderApp分箱。
    pub(super) const fn needs_mesh_render_monitor(&self) -> bool {
        self.visible
            && matches!(
                self.page,
                ResourceMonitorPage::Trees
                    | ResourceMonitorPage::Shrubs
                    | ResourceMonitorPage::Rocks
            ) // 只有正在查看的三类固定景观页需要按层、原型、LOD与部件精确取样。
    }

    /// 返回当前F3状态是否需要旧树木公共监控的七Mesh汇总。
    pub(super) const fn needs_tree_render_monitor(&self) -> bool {
        self.visible && matches!(self.page, ResourceMonitorPage::Trees) // 仅树木页消费该兼容快照，灌木和岩石使用通用Mesh统计。
    }
}

#[cfg(test)] // 分组映射纯值回归不启动窗口或 ECS 调度。
mod tests {
    use super::*; // 直接验证生产组、叶子页和默认状态契约。

    #[test] // 每个叶子页必须归属唯一顶层组，组默认页必须反向匹配。
    fn resource_monitor_pages_keep_one_consistent_group() {
        for page in ResourceMonitorPage::ALL {
            let group = page.group(); // 读取生产叶子页所属组。
            assert!(
                ResourceMonitorGroup::ALL.contains(&group),
                "叶子页必须归属五个正式组之一"
            ); // 禁止未来新增页面遗漏顶层入口。
        } // 完成十个叶子页映射扫描。
        for group in ResourceMonitorGroup::ALL {
            assert_eq!(group.default_page().group(), group); // 点击顶层组后默认页必须仍属于该组。
        } // 完成五个组默认页反向核对。
        assert_eq!(ResourceMonitorPage::SUBTABS.len(), 8); // 故道三页、作物两页和景观物三页固定为八个二级按钮。
    } // 完成两级分页映射回归。

    #[test] // 精确Mesh分箱不得在普通游戏或非景观物诊断页常驻运行。
    fn mesh_render_monitor_only_runs_for_visible_landscape_pages() {
        let mut state = ResourceMonitorState::default(); // 默认F3隐藏且停留总览页。
        assert!(!state.needs_mesh_render_monitor()); // 启动后的普通地图帧必须保持零监控成本。

        state.visible = true; // 模拟用户按F3打开默认总览页。
        assert!(!state.needs_mesh_render_monitor()); // 总览只读通用快照，不提取数千Mesh身份。
        assert!(!state.needs_tree_render_monitor()); // 总览同样不得扫描七个树木Mesh。

        for page in [
            ResourceMonitorPage::Trees,
            ResourceMonitorPage::Shrubs,
            ResourceMonitorPage::Rocks,
        ] {
            state.page = page; // 依次模拟三类固定景观技术页。
            assert!(state.needs_mesh_render_monitor()); // 当前可见页必须允许取得精确PBR分箱。
            assert_eq!(
                state.needs_tree_render_monitor(),
                page == ResourceMonitorPage::Trees
            ); // 旧树木汇总只服务树木页，灌木和岩石不得重复扫描。
        }

        state.visible = false; // 模拟从岩石页直接关闭F3。
        assert!(!state.needs_mesh_render_monitor()); // 关闭边沿必须立即撤销后续采样授权。
        assert!(!state.needs_tree_render_monitor()); // 树木兼容监控也必须保持关闭。
    }
} // 结束资源监控分页纯值测试。
