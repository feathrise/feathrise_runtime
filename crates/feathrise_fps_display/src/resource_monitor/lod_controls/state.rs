//! F3 与 F4 共享的九类植物、作物和景观物四档 LOD 调试状态。
//! 【功能设计】两块监控面板必须读写同一份非持久化状态，避免一块面板重新打开另一块已经关闭的档位。

use bevy::prelude::Resource; // 共享调试状态作为本次进程内唯一 ECS 资源存在。

use crate::resources::ResourceMonitorPage; // F3 私有页面通过显式映射取得公开内容身份。

/// 每类内容固定拥有四个 LOD 档位。
pub const LOD_LEVEL_COUNT: usize = 4; // 与 GPU 叶片、树木和固定景观的正式四档契约保持一致。

/// 四个低位全部置一表示当前内容四档全部开启。
pub const ALL_LODS_ENABLED: u8 = 0b1111; // 位段只作为内部紧凑状态，界面始终显示可读中文按钮。

/// 九类可调内容在资源中的固定槽位数量。
const CONTROLLED_CONTENT_COUNT: usize = 9; // 普通草、故道三类叶片、双作物和三类 Mesh 内容互不共享开关。

/// F3 与 F4 可以共同控制的稳定内容身份。
#[derive(Clone, Copy, Debug, PartialEq, Eq)] // Copy 身份可安全跨 UI、主世界应用系统和测试传递。
pub enum LodControlContent {
    OrdinaryGrass, // 普通草使用 GPU 叶片零号 profile 的四个窗口。
    Reeds,         // 芦苇使用 GPU 叶片二号 profile 的四个窗口。
    WetlandGrass,  // 湿地草在 HN-02 会话内复用零号 profile。
    SandbarGrass,  // 沙洲矮草在 HN-02 会话内复用三号 profile。
    Wheat,         // 麦田使用 GPU 叶片一号 profile 的四个窗口。
    MixedGrain,    // 杂粮田使用 GPU 叶片三号 profile 的四个窗口。
    Trees,         // 树木使用高、中、低和公告板四档。
    Shrubs,        // 灌木使用高、中、低和超低模四档。
    Rocks,         // 岩石使用高、中、低和超低模四档。
} // 完成跨监控面板的稳定内容枚举。

impl LodControlContent {
    /// 返回全部受控内容的稳定顺序，供默认状态和纯逻辑回归遍历。
    pub const ALL: [Self; CONTROLLED_CONTENT_COUNT] = [
        Self::OrdinaryGrass, // 零号槽保持普通草历史身份。
        Self::Reeds,         // 一号槽保持芦苇历史身份。
        Self::WetlandGrass,  // 二号槽保持湿地草历史身份。
        Self::SandbarGrass,  // 三号槽保持沙洲矮草历史身份。
        Self::Wheat,         // 四号槽保持麦田历史身份。
        Self::MixedGrain,    // 五号槽保持杂粮田历史身份。
        Self::Trees,         // 六号槽保持树木历史身份。
        Self::Shrubs,        // 七号槽保持灌木历史身份。
        Self::Rocks,         // 八号槽保持岩石历史身份。
    ]; // 完成稳定内容数组。

    /// 返回当前内容在紧凑掩码数组中的稳定槽位。
    pub(super) const fn slot(self) -> usize {
        match self {
            Self::OrdinaryGrass => 0, // 普通草独立保存四档状态。
            Self::Reeds => 1,         // 芦苇独立保存四档状态。
            Self::WetlandGrass => 2,  // 湿地草独立保存四档状态。
            Self::SandbarGrass => 3,  // 沙洲矮草独立保存四档状态。
            Self::Wheat => 4,         // 麦田独立保存四档状态。
            Self::MixedGrain => 5,    // 杂粮田独立保存四档状态。
            Self::Trees => 6,         // 树木独立保存四档状态。
            Self::Shrubs => 7,        // 灌木独立保存四档状态。
            Self::Rocks => 8,         // 岩石独立保存四档状态。
        } // 完成内容到槽位的穷尽映射。
    } // 返回稳定数组下标。

    /// 返回当前内容指定档位的用户可读名称。
    pub const fn level_label(self, level: usize) -> &'static str {
        match self {
            Self::OrdinaryGrass
            | Self::Reeds
            | Self::WetlandGrass
            | Self::SandbarGrass
            | Self::Wheat
            | Self::MixedGrain => match level {
                0 => "近景", // GPU 叶片零档。
                1 => "中景", // GPU 叶片一档。
                2 => "远景", // GPU 叶片二档。
                _ => "超远", // GPU 叶片三档。
            }, // 完成六类 GPU 叶片统一名称。
            Self::Trees => match level {
                0 => "高模",   // 树木高档树干与树冠。
                1 => "中模",   // 树木中档树干与树冠。
                2 => "低模",   // 树木低档树干与树冠。
                _ => "公告板", // 树木最远完整公告板。
            }, // 完成树木四档名称。
            Self::Shrubs | Self::Rocks => match level {
                0 => "高模",   // 固定景观高档。
                1 => "中模",   // 固定景观中档。
                2 => "低模",   // 固定景观低档。
                _ => "超低模", // 固定景观最远代理档。
            }, // 完成灌木和岩石四档名称。
        } // 完成内容与档位名称组合。
    } // 返回可直接显示的短中文名。
} // 完成共享内容身份方法。

/// 当前进程内九类内容的四档 LOD 调试状态。
#[derive(Resource, Clone, Debug, PartialEq, Eq)] // 资源变化检测只在真实按钮操作后触发应用系统。
pub struct LodControlState {
    enabled_masks: [u8; CONTROLLED_CONTENT_COUNT], // 每类内容只使用低四位保存四档开关。
} // 完成非持久化共享调试状态定义。

impl Default for LodControlState {
    fn default() -> Self {
        Self {
            enabled_masks: [ALL_LODS_ENABLED; CONTROLLED_CONTENT_COUNT], // 游戏启动时九类内容全部恢复正式渲染。
        } // 完成默认全开状态。
    } // 返回不会写入 settings.json 或地图文件的本次进程状态。
} // 完成共享状态默认值。

impl LodControlState {
    /// 返回当前内容指定档位是否开启。
    pub fn is_enabled(&self, content: LodControlContent, level: usize) -> bool {
        let bit = 1_u8 << level.min(LOD_LEVEL_COUNT - 1); // 越界档位保守钳到第四档，禁止位移溢出。
        self.enabled_masks[content.slot()] & bit != 0 // 读取当前内容低四位中的对应档位。
    } // 完成单档状态读取。

    /// 返回当前内容完整四档掩码。
    pub fn mask(&self, content: LodControlContent) -> u8 {
        self.enabled_masks[content.slot()] // 每种内容都返回自身独立的低四位状态。
    } // 完成当前内容掩码读取。

    /// 切换当前内容的一个档位。
    pub fn toggle(&mut self, content: LodControlContent, level: usize) {
        let bit = 1_u8 << level.min(LOD_LEVEL_COUNT - 1); // 只允许修改正式四档范围内的一位。
        self.enabled_masks[content.slot()] ^= bit; // 允许关闭最后一档以测量该类内容完全退出后的基线。
    } // 完成单档切换。

    /// 只保留当前内容指定档位。
    pub fn solo(&mut self, content: LodControlContent, level: usize) {
        self.enabled_masks[content.slot()] = 1_u8 << level.min(LOD_LEVEL_COUNT - 1); // 单独查看不扩大该档原有距离范围。
    } // 完成单档隔离。

    /// 恢复当前内容四档全部开启。
    pub fn enable_all(&mut self, content: LodControlContent) {
        self.enabled_masks[content.slot()] = ALL_LODS_ENABLED; // 一键恢复当前内容正式四档渲染组合。
    } // 完成当前内容全开。
} // 完成共享状态操作接口。

/// 把 F3 私有叶子页映射到共享内容身份。
pub(in crate::resource_monitor) const fn content_for_page(page: ResourceMonitorPage) -> Option<LodControlContent> {
    match page {
        ResourceMonitorPage::OrdinaryGrass => Some(LodControlContent::OrdinaryGrass), // 普通草页。
        ResourceMonitorPage::Reeds => Some(LodControlContent::Reeds),                 // 芦苇页。
        ResourceMonitorPage::WetlandGrass => Some(LodControlContent::WetlandGrass),   // 湿地草页。
        ResourceMonitorPage::SandbarGrass => Some(LodControlContent::SandbarGrass), // 沙洲矮草页。
        ResourceMonitorPage::Wheat => Some(LodControlContent::Wheat),               // 麦田页。
        ResourceMonitorPage::MixedGrain => Some(LodControlContent::MixedGrain),     // 杂粮田页。
        ResourceMonitorPage::Trees => Some(LodControlContent::Trees),               // 树木页。
        ResourceMonitorPage::Shrubs => Some(LodControlContent::Shrubs),             // 灌木页。
        ResourceMonitorPage::Rocks => Some(LodControlContent::Rocks),               // 岩石页。
        ResourceMonitorPage::Overview => None, // 总览只统计，不控制任何具体渲染档位。
    } // 完成十页穷尽映射。
} // 返回当前 F3 页面可选共享内容身份。

#[cfg(test)] // 纯状态测试不启动窗口或渲染器。
mod tests {
    use super::*; // 直接验证生产状态方法和九类内容隔离契约。

    #[test] // 默认九类内容都必须启用四档。
    fn lod_controls_default_to_all_enabled() {
        let state = LodControlState::default(); // 创建游戏启动时的正式默认状态。
        for content in LodControlContent::ALL {
            assert_eq!(state.mask(content), ALL_LODS_ENABLED); // 每类内容都必须独立全开。
        } // 完成九类默认状态扫描。
    } // 完成默认状态回归。

    #[test] // 单独查看和独立开关不能串改其他内容。
    fn lod_controls_isolate_one_content_and_restore_all() {
        let mut state = LodControlState::default(); // 从九类四档全开开始。
        state.solo(LodControlContent::OrdinaryGrass, 3); // 普通草只看第四档。
        assert_eq!(state.mask(LodControlContent::OrdinaryGrass), 0b1000); // 只保留超远草档。
        assert_eq!(state.mask(LodControlContent::Trees), ALL_LODS_ENABLED); // 树木不得被普通草操作串改。
        assert_eq!(state.mask(LodControlContent::Reeds), ALL_LODS_ENABLED); // 芦苇同样不得被普通草操作串改。
        state.toggle(LodControlContent::OrdinaryGrass, 0); // 重新单独打开近景草。
        assert_eq!(state.mask(LodControlContent::OrdinaryGrass), 0b1001); // 近景与超远同时开启。
        state.enable_all(LodControlContent::OrdinaryGrass); // 使用用户友好的一键恢复。
        assert_eq!(
            state.mask(LodControlContent::OrdinaryGrass), // 读取普通草恢复后的完整掩码。
            ALL_LODS_ENABLED,                             // 四档全部开启才算恢复正式组合。
        ); // 完成普通草恢复断言。
    } // 完成内容隔离与恢复回归。
} // 结束 LOD 调试状态测试模块。
