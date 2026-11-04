// 天气调试纯逻辑：实现天气 UI 顺序、中文名称和运行时强制预览资源接口。

use crate::weather::resources::WeatherDebugOverride; // 为私有调试覆盖字段实现安全公开方法。
use crate::weather::types::WeatherKind; // 离散模式索引与中文名称围绕五类公开天气计算。

impl WeatherKind {
    /// 所有首版天气的稳定UI顺序。
    pub const ALL: [Self; 5] = [
        Self::Clear,     // 模式一对应晴天。
        Self::Overcast,  // 模式二对应阴天。
        Self::LightRain, // 模式三对应小雨。
        Self::HeavyRain, // 模式四对应大雨。
        Self::Mist,      // 模式五对应薄雾。
    ]; // 结束所有天气静态顺序。

    /// 返回面向中文测试面板的天气名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Clear => "晴",       // 晴朗天气短标签。
            Self::Overcast => "阴",    // 阴天天气短标签。
            Self::LightRain => "小雨", // 小雨天气短标签。
            Self::HeavyRain => "大雨", // 大雨天气短标签。
            Self::Mist => "薄雾",      // 薄雾天气短标签。
        } // 返回当前天气中文名称。
    } // 结束天气名称查询。
} // 结束天气种类公开辅助接口。

impl Default for WeatherDebugOverride {
    fn default() -> Self {
        Self {
            forced: None,                   // 默认使用自动天气状态输出。
            preview: 1.0,                   // 选择强制天气后默认显示完整目标效果。
            preview_override_active: false, // 自动天气默认显示状态机真实过渡进度，而不是固定预览值。
        } // 返回安全调试覆盖默认值。
    } // 结束天气调试覆盖默认构造。
} // 结束天气调试覆盖Default实现。

impl WeatherDebugOverride {
    /// 返回当前强制天气；None表示自动模式。
    pub const fn forced(&self) -> Option<WeatherKind> {
        self.forced // 私有字段只通过模式索引或clear安全修改。
    } // 结束强制天气查询。

    /// 返回零至一的强制天气预览比例。
    pub fn preview(&self) -> f32 {
        self.preview // 设置接口保证结果有限且已钳制。
    } // 结束预览比例查询。

    /// 返回自动模式是否正在使用 F12 手动过渡预览。
    pub const fn preview_override_active(&self) -> bool {
        self.preview_override_active // 强制天气始终使用preview，该标记只区分自动状态机真实进度与手动预览。
    } // 结束自动天气手动预览状态查询。

    /// 返回 F12 离散天气模式索引：零自动，一至五对应 WeatherKind::ALL。
    pub fn mode_index(&self) -> usize {
        self.forced
            .and_then(|forced| WeatherKind::ALL.iter().position(|kind| *kind == forced)) // 查找强制天气在稳定UI顺序中的位置。
            .map_or(0, |index| index + 1) // 自动模式为零，天气模式从一开始。
    } // 结束调试天气模式索引查询。

    /// 按 F12 离散索引设置自动或强制天气模式。
    pub fn set_mode_index(&mut self, mode_index: usize) {
        self.forced = mode_index
            .checked_sub(1) // 索引零无法减一，因此自然映射为自动模式。
            .and_then(|index| WeatherKind::ALL.get(index).copied()); // 一至五映射天气，越界索引安全回到自动模式。
        if self.forced.is_none() {
            self.preview_override_active = false; // 切回自动模式时先恢复状态机真实进度，随后拖预览才重新接管。
        } // 结束自动模式预览释放。
    } // 结束调试天气模式索引设置。

    /// 设置强制天气效果预览比例。
    pub fn set_preview(&mut self, preview: f32) {
        self.preview = if preview.is_finite() {
            preview.clamp(0.0, 1.0) // 有限UI输入钳制到零至一。
        } else {
            0.0 // 非有限输入回退到自动天气输出，避免污染光照数值。
        }; // 完成调试预览比例规范化。
        self.preview_override_active = true; // 用户主动拖动后，自动模式也应按该比例检查当前到目标的连续混合。
    } // 结束调试天气预览设置。

    /// 清除强制天气并保留当前预览滑块位置。
    pub fn clear(&mut self) {
        self.forced = None; // 恢复WeatherState自动输出且不写入RenderSettings。
        self.preview_override_active = false; // 同时释放自动模式手动预览，恢复真实天气进度。
    } // 结束天气强制覆盖清理。

    /// 游戏恢复推进时释放自动模式的 F12 过渡预览。
    pub(crate) fn release_automatic_preview(&mut self) {
        if self.forced.is_none() {
            self.preview_override_active = false; // 强制天气可在关面板后继续观察，自动预览只在暂停调试期间接管。
        } // 结束自动模式判断。
    } // 结束自动天气预览释放。
} // 结束天气调试覆盖公开接口。
