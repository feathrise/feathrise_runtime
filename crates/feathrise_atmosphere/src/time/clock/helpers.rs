// 时钟纯逻辑：实现普通年份日期推进、24:00 哨兵与存档规范化。

use crate::time::constants::{SECONDS_PER_DAY, SECONDS_PER_HOUR}; // 所有时间换算共享统一秒数常量。
use crate::{WorldClock, WorldTimeSnapshot}; // 为公开资源与快照实现稳定操作接口。

/// 默认环境月份。
const DEFAULT_MONTH: u8 = 6; // 六月用于展示接近夏至的初始日照。
/// 默认环境日期。
const DEFAULT_DAY: u8 = 15; // 月中避免月份切换时落入不同月长边界。
/// 默认环境小时。
const DEFAULT_HOUR: f32 = 12.0; // 新游戏和旧存档默认从正午开始，便于观察场景。

impl Default for WorldTimeSnapshot {
    fn default() -> Self {
        Self {
            month: DEFAULT_MONTH, // 旧存档缺失月份时使用六月。
            day: DEFAULT_DAY,     // 旧存档缺失日期时使用十五日。
            seconds_since_midnight: DEFAULT_HOUR * SECONDS_PER_HOUR, // 旧存档缺失时刻时使用十二点。
        }
    }
}

impl Default for WorldClock {
    fn default() -> Self {
        Self::from_snapshot(WorldTimeSnapshot::default()) // 默认资源与默认存档快照保持唯一口径。
    }
}

impl WorldClock {
    /// 从快照构造并规范化世界时钟。
    pub fn from_snapshot(snapshot: WorldTimeSnapshot) -> Self {
        let mut clock = Self {
            month: DEFAULT_MONTH, // 先放入合法月份，随后由 restore 统一覆盖。
            day: DEFAULT_DAY,     // 先放入合法日期，避免构造期间出现无效状态。
            seconds_since_midnight: DEFAULT_HOUR * SECONDS_PER_HOUR, // 先放入合法正午时刻。
        };
        clock.restore(snapshot); // 所有外部快照都经过同一个规范化入口。
        clock // 返回已经满足不变量的时钟。
    }

    /// 返回当前月份。
    pub fn month(&self) -> u8 {
        self.month // 字段始终已经被规范化到一至十二月。
    }

    /// 返回当前当月日期。
    pub fn day(&self) -> u8 {
        self.day // 字段始终不超过当前月份的天数。
    }

    /// 返回测试面板显示用小时；暂停设置到 24:00 时保留右端哨兵。
    pub fn hour_for_display(&self) -> f32 {
        self.seconds_since_midnight / SECONDS_PER_HOUR // 86400 秒会精确显示为二十四点。
    }

    /// 返回光照和天文计算使用的零至二十四小时，不包含 24:00 哨兵。
    pub fn hour_wrapped(&self) -> f32 {
        if self.seconds_since_midnight >= SECONDS_PER_DAY {
            0.0 // 24:00 的光照与下一日 00:00 完全一致。
        } else {
            self.seconds_since_midnight / SECONDS_PER_HOUR // 普通时刻直接换算为小数小时。
        }
    }

    /// 设置测试面板时刻，输入按整分钟吸附并允许精确到达 24:00。
    pub fn set_hour(&mut self, hour: f32) {
        let finite_hour = if hour.is_finite() { hour } else { 0.0 }; // 非有限 UI 输入安全回退到午夜。
        let clamped_hour = finite_hour.clamp(0.0, 24.0); // 滑块和外部调用都不能越出一天边界。
        let rounded_minutes = (clamped_hour * 60.0).round(); // 时间滑块按整分钟调整，避免显示抖动。
        self.seconds_since_midnight = (rounded_minutes * 60.0).clamp(0.0, SECONDS_PER_DAY); // 保存包含 24:00 的最终秒数。
    }

    /// 设置月份并统一落在该月十五日，保留当前时刻。
    pub fn set_month_midpoint(&mut self, month: u8) {
        self.set_date(month, DEFAULT_DAY); // 复用公开日期规范化入口，保证月份滑块与存档口径一致。
    }

    /// 设置环境月份和当月日期，并按普通非闰年日历规范化输入。
    pub fn set_date(&mut self, month: u8, day: u8) {
        self.month = month.clamp(1, 12); // 越界月份就近钳制到一月或十二月。
        self.day = day.clamp(1, days_in_month(self.month)); // 日期按规范化后的月份天数钳制。
    }

    /// 返回中文季节名称；月份采用气象季节划分。
    pub fn season_name(&self) -> &'static str {
        match self.month {
            3..=5 => "春季",  // 三月至五月为春季。
            6..=8 => "夏季",  // 六月至八月为夏季。
            9..=11 => "秋季", // 九月至十一月为秋季。
            _ => "冬季",      // 十二月至二月为冬季。
        }
    }

    /// 生成可直接写入存档的轻量快照。
    pub fn snapshot(&self) -> WorldTimeSnapshot {
        WorldTimeSnapshot {
            month: self.month,                                   // 保存规范化后的月份。
            day: self.day,                                       // 保存规范化后的日期。
            seconds_since_midnight: self.seconds_since_midnight, // 保留可能存在的 24:00 哨兵。
        }
    }

    /// 从存档快照恢复并修复越界或损坏值。
    pub fn restore(&mut self, snapshot: WorldTimeSnapshot) {
        self.month = snapshot.month.clamp(1, 12); // 月份安全钳制到普通年份范围。
        self.day = snapshot.day.clamp(1, days_in_month(self.month)); // 日期按恢复后的月份钳制。
        self.seconds_since_midnight = if snapshot.seconds_since_midnight.is_finite() {
            snapshot.seconds_since_midnight.clamp(0.0, SECONDS_PER_DAY) // 有限秒数允许包含 24:00 哨兵。
        } else {
            DEFAULT_HOUR * SECONDS_PER_HOUR // NaN 或无穷值回退到稳定正午。
        };
    }

    /// 重置为新游戏和旧存档共用的默认世界时间。
    pub fn reset(&mut self) {
        *self = Self::default(); // 通过 Default 保持初始口径只有一个来源。
    }

    /// 推进指定游戏秒数，并在需要时跨日、跨月和跨年循环。
    pub fn advance_seconds(&mut self, game_seconds: f32) {
        if !game_seconds.is_finite() {
            return; // 非有限增量不能参与运算，也不消费面板保留的二十四点哨兵。
        }
        if self.seconds_since_midnight >= SECONDS_PER_DAY {
            self.seconds_since_midnight = 0.0; // 未暂停首帧即使虚拟时间增量为零，也先把 24:00 规范化为次日零点。
            self.advance_date(); // 哨兵代表上一日结束，因此必须先换日。
        }
        if game_seconds <= 0.0 {
            return; // 完成哨兵规范化后，普通非正数增量不再改变当日时刻。
        }
        let mut remaining = game_seconds; // 大增量可能一次跨越多天，需要循环消费。
        while self.seconds_since_midnight + remaining >= SECONDS_PER_DAY {
            let until_midnight = SECONDS_PER_DAY - self.seconds_since_midnight; // 计算当前日期剩余秒数。
            remaining -= until_midnight; // 消费至午夜的时间，再继续处理完整天数。
            self.seconds_since_midnight = 0.0; // 跨日后从新一天零点继续累计。
            self.advance_date(); // 日期随每次跨过午夜推进一次。
        }
        self.seconds_since_midnight += remaining; // 保存未满一天的剩余秒数。
    }

    /// 返回普通年份中的日序号，范围一至三百六十五。
    pub fn day_of_year(&self) -> u16 {
        let elapsed_before_month: u16 = (1..self.month)
            .map(|month| u16::from(days_in_month(month))) // 累加当前月之前所有月份的天数。
            .sum(); // 得到当年已经完整经过的天数。
        elapsed_before_month + u16::from(self.day) // 加上当前月日期得到一基日序号。
    }

    /// 将日期推进一天，并在年末循环回一月一日。
    fn advance_date(&mut self) {
        if self.day < days_in_month(self.month) {
            self.day += 1; // 尚未到月末时只增加当月日期。
        } else {
            self.day = 1; // 月末之后进入下一月一日。
            self.month = if self.month < 12 { self.month + 1 } else { 1 }; // 十二月之后循环回一月，不推动剧情年份。
        }
    }
}

/// 返回普通非闰年指定月份的天数。
pub(crate) const fn days_in_month(month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30, // 四、六、九、十一月各三十日。
        2 => 28,              // 环境月份不记录年份，因此固定采用普通年份二十八日。
        _ => 31,              // 其余合法月份均为三十一日；非法月份也使用安全上限。
    }
}

#[cfg(test)] // 时钟纯逻辑测试不需要启动 Bevy 应用。
mod tests {
    use super::*; // 测试复用私有月份天数和默认常量。
    use crate::time::constants::GAME_TIME_SCALE; // 验证现实时间到游戏时间的固定二十四倍换算。

    #[test] // 默认世界时间必须与设计文档和旧存档回退值一致。
    fn default_clock_is_june_fifteenth_at_noon() {
        let clock = WorldClock::default(); // 构造新游戏共用的默认时钟。
        assert_eq!((clock.month(), clock.day()), (6, 15)); // 默认日期固定为六月十五日。
        assert_eq!(clock.hour_for_display(), 12.0); // 默认时刻固定为正午十二点。
    }

    #[test] // 固定二十四倍倍率必须满足一百五十现实秒一游戏小时和一现实小时一游戏日。
    fn fixed_time_scale_matches_hour_and_day_acceptance_values() {
        let mut hourly_clock = WorldClock::default(); // 从六月十五日正午验证一小时换算。
        hourly_clock.advance_seconds(150.0 * GAME_TIME_SCALE); // 一百五十现实秒换算为三千六百游戏秒。
        assert_eq!(hourly_clock.hour_wrapped(), 13.0); // 游戏时刻必须从十二点推进到十三点。

        let mut daily_clock = WorldClock::default(); // 使用独立时钟验证完整游戏日换算。
        daily_clock.advance_seconds(3_600.0 * GAME_TIME_SCALE); // 一现实小时换算为八万六千四百游戏秒。
        assert_eq!((daily_clock.month(), daily_clock.day()), (6, 16)); // 日期必须从六月十五日推进到十六日。
        assert_eq!(daily_clock.hour_wrapped(), 12.0); // 完整一日后仍保持正午时刻。
    }

    #[test] // 时间滑块必须能精确抵达右端并在恢复推进时换日。
    fn twenty_four_hour_sentinel_wraps_on_next_advance() {
        let mut clock = WorldClock::default(); // 从合法日期开始验证哨兵行为。
        clock.set_hour(24.0); // 模拟 F1 面板拖到最右端。
        assert_eq!(clock.hour_for_display(), 24.0); // 暂停期间显示值必须保留二十四点。
        assert_eq!(clock.hour_wrapped(), 0.0); // 光照计算必须把二十四点视为午夜。
        clock.advance_seconds(24.0); // 恢复游戏后一现实秒对应推进二十四游戏秒。
        assert_eq!((clock.month(), clock.day()), (6, 16)); // 首次推进先进入下一日。
        assert!((clock.hour_wrapped() - 24.0 / 3600.0).abs() < 0.000_001); // 换日后保存剩余二十四秒。
    }

    #[test] // 未暂停恢复首帧可能没有虚拟时间增量，但仍必须消费二十四点哨兵。
    fn twenty_four_hour_sentinel_wraps_with_zero_delta() {
        let mut clock = WorldClock::default(); // 从六月十五日构造可观察换日的合法时钟。
        clock.set_hour(24.0); // 模拟测试面板暂停期间停在最右端二十四点。
        clock.advance_seconds(0.0); // 模拟关闭面板后的首个未暂停帧仍然获得零增量。
        assert_eq!((clock.month(), clock.day()), (6, 16)); // 哨兵必须先规范化到次日。
        assert_eq!(clock.hour_for_display(), 0.0); // 规范化后面板显示应回到左端零点。
    }

    #[test] // 普通年份月份天数和年末循环必须稳定。
    fn date_progression_handles_february_and_year_end() {
        let mut february = WorldClock::from_snapshot(WorldTimeSnapshot {
            month: 2,                                      // 从二月月末开始。
            day: 28,                                       // 普通年份二月最后一天。
            seconds_since_midnight: SECONDS_PER_DAY - 1.0, // 距午夜只差一秒。
        });
        february.advance_seconds(1.0); // 精确跨过二月末午夜。
        assert_eq!((february.month(), february.day()), (3, 1)); // 下一天必须是三月一日。
        let mut year_end = WorldClock::from_snapshot(WorldTimeSnapshot {
            month: 12,                                     // 从十二月月末开始。
            day: 31,                                       // 普通年份最后一天。
            seconds_since_midnight: SECONDS_PER_DAY - 1.0, // 距年末只差一秒。
        });
        year_end.advance_seconds(1.0); // 跨过环境年份边界。
        assert_eq!((year_end.month(), year_end.day()), (1, 1)); // 环境月份循环回一月，不保存剧情年份。
    }

    #[test] // 损坏存档必须被修复为可用日期和有限时刻。
    fn restore_clamps_corrupted_snapshot() {
        let mut clock = WorldClock::default(); // 从默认合法状态开始恢复损坏数据。
        clock.restore(WorldTimeSnapshot {
            month: 99,                        // 模拟越界月份。
            day: 99,                          // 模拟越界日期。
            seconds_since_midnight: f32::NAN, // 模拟非法 JSON 来源或运行期损坏值。
        });
        assert_eq!((clock.month(), clock.day()), (12, 31)); // 月份和日期应就近钳制到十二月三十一日。
        assert_eq!(clock.hour_for_display(), 12.0); // 非有限时刻回退到正午。
    }

    #[test] // 公开日期设置接口必须使用普通年份月长规范化输入。
    fn set_date_clamps_month_and_day_to_calendar() {
        let mut clock = WorldClock::default(); // 从默认合法日期开始验证公开设置接口。
        clock.set_date(2, 31); // 二月不存在三十一日，应钳制到普通年份月末。
        assert_eq!((clock.month(), clock.day()), (2, 28)); // 普通年份二月最后一天固定为二十八日。
        clock.set_date(0, 0); // 同时传入小于合法范围的月份和日期。
        assert_eq!((clock.month(), clock.day()), (1, 1)); // 损坏输入应就近钳制到一月一日。
    }

    #[test] // 一次大增量应能够跨越多个月份而不丢失余数。
    fn large_advance_crosses_multiple_days() {
        let mut clock = WorldClock::from_snapshot(WorldTimeSnapshot {
            month: 1,                    // 从一月末开始便于覆盖跨月。
            day: 31,                     // 当前月最后一天。
            seconds_since_midnight: 0.0, // 从午夜开始计算完整天数。
        });
        clock.advance_seconds(SECONDS_PER_DAY * 2.0 + SECONDS_PER_HOUR); // 推进两天再加一小时。
        assert_eq!((clock.month(), clock.day()), (2, 2)); // 一月三十一日加两天进入二月二日。
        assert_eq!(clock.hour_wrapped(), 1.0); // 完整天数之后的一小时余数必须保留。
    }
}
