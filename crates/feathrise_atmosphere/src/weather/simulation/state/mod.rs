//! 天气状态纯逻辑：公开查询、快照规范化、调试模式映射和确定性自动推进。

use crate::weather::climate::WeatherClimateProfile; // 自动抽取只接受场景显式登记的气候配置。
use crate::weather::constants::{
    // 导入状态默认值、损坏快照上限与晨雾强制消散时刻。
    DEFAULT_TRANSITION_GAME_SECONDS, // 外部过渡请求的默认持续时间。
    DEFAULT_WEATHER_SEED,            // 新游戏和零种子快照共用的确定性种子。
    GAME_SECONDS_PER_HOUR,           // 晨雾按距十点的剩余小时换算过渡时长。
    INITIAL_CLEAR_GAME_SECONDS,      // 新游戏晴天至少稳定四游戏小时。
    MAX_SNAPSHOT_TRANSITION_GAME_SECONDS, // 损坏快照过渡时长安全上限。
    MAX_STABLE_GAME_SECONDS,         // 损坏快照稳定期安全上限。
    MIST_DISPERSAL_END_HOUR,         // 十点后薄雾必须完全清除。
    MIST_DISPERSAL_START_HOUR,       // 九点半开始薄雾主动转晴。
    MIST_GENERATION_START_HOUR,      // 三点前不允许保留异常薄雾过程。
}; // 完成状态逻辑常量导入。
use crate::weather::resources::WeatherState; // 为私有字段天气状态资源实现公开安全接口。
use crate::weather::types::{WeatherKind, WeatherRegion, WeatherSnapshot}; // 状态方法读写公开天气值类型与存档快照。

use super::selection::{
    // 状态推进复用独立可测试的抽取纯函数。
    deterministic_random_unit,    // 序号随机样本保证读档后天气序列一致。
    safe_next_step,               // 任意请求只能沿合法状态图前进一步。
    select_next_weather,          // 自动天气从自身与相邻节点按权重抽取。
    stable_duration_game_seconds, // 新稳定期按天气种类生成持续时长。
    transition_duration_game_seconds, // 自动相邻过渡使用十至三十分钟规则。
}; // 完成天气抽取函数导入。

impl Default for WeatherSnapshot {
    fn default() -> Self {
        Self {
            surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            current: WeatherKind::Clear, // 新游戏与旧存档从晴天开始。
            target: WeatherKind::Clear, // 稳定期目标与当前天气一致。
            queued_target: None,  // 默认没有跨节点剧情天气请求。
            queued_transition_duration_game_seconds: 0.0, // 没有排队目标时不保存后续单段时长。
            pending_minimum_hold_game_seconds: 0.0, // 默认自动天气没有剧情最低维持时间。
            transition_progress: 1.0, // 没有进行中过渡时进度视为完整。
            transition_duration_game_seconds: 0.0, // 稳定期没有过渡总时长。
            stable_remaining_game_seconds: INITIAL_CLEAR_GAME_SECONDS, // 初始晴天至少保持四游戏小时。
            region: WeatherRegion::Global, // 新会话默认使用中性全局区域。
            seed: DEFAULT_WEATHER_SEED,    // 使用固定项目种子确保可复现。
            sequence: 0,                   // 新天气序列尚未消费任何随机值。
        } // 返回安全默认天气快照。
    } // 结束天气快照默认实现。
} // 结束天气快照Default实现。

impl Default for WeatherState {
    fn default() -> Self {
        Self::from_snapshot(WeatherSnapshot::default()) // 资源默认值复用快照规范化入口保持单一口径。
    } // 结束天气状态默认构造。
} // 结束天气状态Default实现。

impl WeatherState {
    /// 从任意快照构造并规范化天气状态。
    pub fn from_snapshot(snapshot: WeatherSnapshot) -> Self {
        let mut state = Self {
            surface_wetness: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            puddle_amount: 0.0,   // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            surface_effect_time: 0.0, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            current: WeatherKind::Clear, // 先放入合法当前天气再调用统一恢复接口。
            target: WeatherKind::Clear, // 先放入合法稳定目标。
            queued_target: None,  // 初始临时状态没有跨节点最终目标。
            queued_transition_duration_game_seconds: 0.0, // 初始临时状态不保存排队过渡时长。
            pending_minimum_hold_game_seconds: 0.0, // 初始临时状态没有剧情最低维持时间。
            transition_progress: 1.0, // 初始临时状态不包含过渡。
            transition_duration_game_seconds: 0.0, // 初始临时状态没有过渡时长。
            stable_remaining_game_seconds: INITIAL_CLEAR_GAME_SECONDS, // 初始临时状态保持安全晴天时长。
            region: WeatherRegion::Global, // 初始临时状态使用中性全局区域。
            seed: DEFAULT_WEATHER_SEED,    // 初始临时状态使用非零确定性种子。
            sequence: 0,                   // 初始临时状态尚未消费随机值。
        }; // 完成合法临时状态构造。
        state.restore(snapshot); // 所有外部快照统一经过损坏值修复。
        state // 返回满足状态图和数值不变量的天气状态。
    } // 结束从快照构造。

    /// 返回当前天气。
    pub const fn current(&self) -> WeatherKind {
        self.current // 私有字段始终保存合法枚举值。
    } // 结束当前天气查询。

    /// 返回当前过渡目标天气。
    pub const fn target(&self) -> WeatherKind {
        self.target // 目标与当前相同或为状态图直接相邻节点。
    } // 结束目标天气查询。

    /// 返回零至一的过渡进度。
    pub fn transition_progress(&self) -> f32 {
        self.transition_progress // 状态写入接口保证结果有限且已钳制。
    } // 结束过渡进度查询。

    /// 返回当前过渡总游戏秒数。
    pub fn transition_duration_game_seconds(&self) -> f32 {
        self.transition_duration_game_seconds // 稳定期为零，过渡期为规范化正数。
    } // 结束过渡总时长查询。

    /// 返回稳定天气剩余游戏秒数。
    pub fn stable_remaining_game_seconds(&self) -> f32 {
        self.stable_remaining_game_seconds // 过渡期通常为零，稳定期随虚拟时间递减。
    } // 结束稳定剩余时长查询。

    /// 返回当前天气区域。
    pub const fn region(&self) -> WeatherRegion {
        self.region // 所有地图维持中性全局天气连续性。
    } // 结束天气区域查询。

    /// 返回确定性随机种子。
    pub const fn seed(&self) -> u64 {
        self.seed // 零种子在恢复和重设时会替换为项目默认值。
    } // 结束天气随机种子查询。

    /// 返回已消费的确定性随机序号。
    pub const fn sequence(&self) -> u64 {
        self.sequence // 存档保存该值以继续相同天气序列。
    } // 结束随机序号查询。

    /// 返回当前是否处于两个不同天气之间的过渡期。
    pub fn is_transitioning(&self) -> bool {
        self.current != self.target && self.transition_progress < 1.0 // 不同节点且进度未完成才是有效过渡。
    } // 结束过渡状态查询。

    /// 生成可以直接写入角色存档的天气快照。
    pub fn snapshot(&self) -> WeatherSnapshot {
        WeatherSnapshot {
            surface_wetness: self.surface_wetness, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            puddle_amount: self.puddle_amount, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            surface_effect_time: self.surface_effect_time, // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            current: self.current,                         // 保存当前天气节点。
            target: self.target,                           // 保存合法相邻目标节点。
            queued_target: self.queued_target,             // 保存跨节点剧情请求尚未到达的最终天气。
            queued_transition_duration_game_seconds: self.queued_transition_duration_game_seconds, // 保存后续路径每一段的过渡时长。
            pending_minimum_hold_game_seconds: self.pending_minimum_hold_game_seconds, // 保存尚未并入最终稳定期的剧情最低维持时间。
            transition_progress: self.transition_progress, // 保存连续过渡比例。
            transition_duration_game_seconds: self.transition_duration_game_seconds, // 保存完整过渡时长。
            stable_remaining_game_seconds: self.stable_remaining_game_seconds, // 保存稳定期剩余时长。
            region: self.region, // 保存天气区域以兼容加载阶段尚未进入目标地图。
            seed: self.seed,     // 保存确定性随机基础种子。
            sequence: self.sequence, // 保存已消费随机序号。
        } // 返回完整轻量天气快照。
    } // 结束天气快照生成。

    /// 从存档快照恢复并修复非有限数值、非法状态边和异常超长持续时间。
    pub fn restore(&mut self, snapshot: WeatherSnapshot) {
        self.surface_wetness = super::surface::unit(snapshot.surface_wetness); // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        self.puddle_amount = super::surface::unit(snapshot.puddle_amount); // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        self.surface_effect_time = super::surface::normalize_time(snapshot.surface_effect_time); // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
        self.current = snapshot.current; // 枚举反序列化已经保证当前天气种类合法。
        let routed_target = safe_next_step(snapshot.current, snapshot.target); // 非相邻目标只能恢复到合法下一节点。
        self.target = routed_target; // 保存规范化后的当前过渡目标。
        let target_was_rerouted = routed_target != snapshot.target; // 路径修复时旧进度不再对应新的目标。
        self.queued_target = snapshot
            .queued_target
            .or(target_was_rerouted.then_some(snapshot.target)) // 旧快照若保存了非法直跳目标，则把原目标保留为最终意图。
            .filter(|queued| *queued != routed_target); // 已由当前相邻目标覆盖的排队目标无需重复执行。
        self.queued_transition_duration_game_seconds = if self.queued_target.is_some() {
            normalize_transition_duration(snapshot.queued_transition_duration_game_seconds)
            // 新快照保存后续各段时长，旧快照的零值回退为默认二十五分钟。
        } else {
            0.0 // 没有排队最终目标时不保留无意义的后续单段时长。
        }; // 完成排队过渡时长规范化。
        self.pending_minimum_hold_game_seconds =
            normalize_hold_duration(snapshot.pending_minimum_hold_game_seconds); // 在途剧情最低维持时间随快照恢复并钳制。
        self.transition_progress = if target_was_rerouted {
            0.0 // 改变目标节点后从新合法过渡起点重新开始。
        } else if snapshot.transition_progress.is_finite() {
            snapshot.transition_progress.clamp(0.0, 1.0) // 有限进度统一钳制到零至一。
        } else {
            0.0 // 非有限过渡进度回退到过渡起点。
        }; // 完成过渡进度规范化。
        self.transition_duration_game_seconds =
            if snapshot.transition_duration_game_seconds.is_finite() {
                snapshot
                    .transition_duration_game_seconds
                    .clamp(0.0, MAX_SNAPSHOT_TRANSITION_GAME_SECONDS) // 有限时长限制在零至六游戏小时。
            } else {
                DEFAULT_TRANSITION_GAME_SECONDS // 非有限过渡时长回退到二十五游戏分钟。
            }; // 完成过渡总时长规范化。
        self.stable_remaining_game_seconds = if snapshot.stable_remaining_game_seconds.is_finite() {
            snapshot
                .stable_remaining_game_seconds
                .clamp(0.0, MAX_STABLE_GAME_SECONDS) // 有限稳定期限制在零至三十游戏日。
        } else {
            INITIAL_CLEAR_GAME_SECONDS // 非有限稳定期回退到四游戏小时安全值。
        }; // 完成稳定剩余时长规范化。
        self.region = snapshot.region; // 区域枚举本身不需要数值钳制。
        self.seed = normalize_seed(snapshot.seed); // 零种子恢复为项目默认确定性序列。
        self.sequence = snapshot.sequence; // 任意序号都可通过wrapping算法继续稳定运行。
        if self.current == self.target {
            self.finish_as_stable_state(); // 相同节点快照统一规范化为完成的稳定期并丢弃损坏排队状态。
        } else if self.transition_duration_game_seconds <= f32::EPSILON
            || self.transition_progress >= 1.0
        {
            self.current = self.target; // 零时长或已完成过渡直接落到目标节点。
            if !self.start_queued_transition_if_needed() {
                self.finish_as_stable_state(); // 没有后续路径时清理过渡字段并保留快照稳定剩余时长。
            } // 已有排队路径时直接进入下一相邻段，不插入错误稳定期。
        } else if self.transition_duration_game_seconds < 1.0 {
            self.transition_duration_game_seconds = 1.0; // 有效过渡至少保留一游戏秒避免除零和极端进度跳变。
        } // 结束快照过渡状态规范化。
    } // 结束天气快照恢复。

    /// 重置为新游戏共用的至少四小时晴天。
    pub fn reset(&mut self) {
        *self = Self::default(); // 通过Default保持新游戏初始口径只有一个来源。
    } // 结束天气状态重置。

    /// 沿合法状态图请求向目标天气过渡。
    pub fn force_transition(
        &mut self,                      // 修改当前合法过渡与可选多段最终意图。
        requested: WeatherKind,         // 剧情最终希望到达的天气。
        duration_game_seconds: f32,     // 每一相邻段使用的过渡游戏秒数。
        minimum_hold_game_seconds: f32, // 最终目标到达后至少维持的游戏秒数。
    ) {
        let active_final_target = self.queued_target.unwrap_or(self.target); // 多段路径用排队目标表示真正剧情意图。
        if self.is_transitioning() && active_final_target == requested {
            self.pending_minimum_hold_game_seconds = self
                .pending_minimum_hold_game_seconds
                .max(normalize_hold_duration(minimum_hold_game_seconds)); // 重复命令只允许延长最低维持时间。
            return; // 重复发送同一剧情天气命令保持现有进度，避免视觉输出突然回跳到过渡起点。
        } // 结束相同在途天气命令幂等判断。
        let duration = normalize_transition_duration(duration_game_seconds); // 外部时长统一限制在一秒至六游戏小时。
        self.pending_minimum_hold_game_seconds = normalize_hold_duration(minimum_hold_game_seconds); // 新剧情请求替换旧目标并记录最终最低维持时间。
        if self.is_transitioning() {
            self.queued_target = (requested != self.target).then_some(requested); // 当前相邻段不中断；新目标在抵达现目标后继续或反向执行。
            self.queued_transition_duration_game_seconds = if self.queued_target.is_some() {
                duration // 后续每一相邻段使用新命令指定的规范化时长。
            } else {
                0.0 // 新目标就是当前在途直接目标时，只取消旧排队意图。
            }; // 完成中途改目标的后续路径替换。
            return; // 保留当前连续画面和进度，避免从离散current重新起步造成视觉回跳。
        } // 结束进行中过渡的平滑重定向。
        let next = safe_next_step(self.current, requested); // 晴转大雨等请求先路由到合法中间节点。
        if next == self.current {
            self.finish_as_stable_state(); // 稳定剩余时间保持原值且清理过渡字段。
            return; // 无需构造零距离过渡。
        } // 结束相同目标处理。
        self.queued_target = (next != requested).then_some(requested); // 非相邻请求保存最终意图供中间节点完成后继续。
        self.queued_transition_duration_game_seconds = if self.queued_target.is_some() {
            duration // 多段路径的后续各段沿用同一剧情过渡时长。
        } else {
            0.0 // 单段过渡不需要保存额外排队时长。
        }; // 完成跨节点最终意图记录。
        self.start_transition(next, duration); // 从当前节点重新开始合法相邻过渡。
    } // 结束外部天气过渡请求。

    /// 更换确定性天气随机种子并从序号零重新开始。
    pub fn reseed(&mut self, seed: u64) {
        self.seed = normalize_seed(seed); // 零种子替换为项目默认值避免误认为未初始化。
        self.sequence = 0; // 新种子从首个随机样本开始消费。
    } // 结束天气随机序列重设。

    /// 立即进入指定天气并为它生成新的稳定期。
    pub(crate) fn apply_immediate(&mut self, kind: WeatherKind, hour: f32) {
        self.queued_target = None; // 立即切换取消任何尚未完成的多段剧情过渡。
        self.queued_transition_duration_game_seconds = 0.0; // 清除对应后续单段时长。
        self.pending_minimum_hold_game_seconds = 0.0; // F1立即切换不继承剧情天气的最低维持时间。
        self.current = kind; // 立即切换不经过状态图中间节点和视觉过渡。
        self.target = kind; // 当前与目标相同表示切换已经完成。
        self.transition_progress = 1.0; // F1标签显示立即切换已完整生效。
        self.transition_duration_game_seconds = 0.0; // 立即切换没有过渡总时长。
        self.stable_remaining_game_seconds = stable_duration_game_seconds(
            kind,                    // 按目标天气种类选择对应持续范围。
            self.next_random_unit(), // 消费确定性持续时长样本以支持存读档复现。
            hour,                    // 薄雾立即切换仍受九点半消散上限约束。
        ); // 保存目标天气的新稳定期。
        self.enforce_mist_time_constraints(hour); // 十点后立即应用薄雾时仍遵守晨雾生命期规则。
    } // 结束天气立即应用。

    /// 按当前季节、时刻、状态图与地图气候配置重新抽取下一目标。
    pub(crate) fn reroll(&mut self, month: u8, hour: f32, climate: WeatherClimateProfile) {
        self.queued_target = None; // 手动重抽接管自动状态并取消旧剧情最终目标。
        self.queued_transition_duration_game_seconds = 0.0; // 清除旧剧情路径时长。
        self.pending_minimum_hold_game_seconds = 0.0; // 自动重抽取消旧剧情最低维持时间。
        let next = select_next_weather(
            self.current,            // 重新抽取仍只允许当前节点自身和相邻节点。
            month,                   // 当前月份决定季节权重。
            hour,                    // 当前时刻决定薄雾是否进入候选池。
            climate,                 // 显式配置提供季节权重与地图局部修正。
            self.next_random_unit(), // 消费一个确定性天气选择样本。
        ); // 完成手动重新抽取。
        if next == self.current {
            self.finish_as_stable_state(); // 自身重抽取消尚未完成的旧过渡。
            self.stable_remaining_game_seconds = stable_duration_game_seconds(
                self.current,            // 按当前天气生成新的稳定时长。
                self.next_random_unit(), // 消费确定性持续时长样本。
                hour,                    // 薄雾稳定期仍受九点半上限约束。
            ); // 保存自身重抽后的稳定期。
            return; // 自身重抽无需构造天气过渡。
        } // 结束自身重抽处理。
        let duration = transition_duration_game_seconds(
            self.current,            // 当前天气决定过渡起点。
            next,                    // 抽取结果是合法相邻目标。
            self.next_random_unit(), // 消费确定性过渡时长样本。
        ); // 得到手动重抽的自动过渡时长。
        self.start_transition(next, duration); // 开始新的合法相邻过渡并取消旧稳定期。
    } // 结束天气重新抽取。

    /// 将场景映射后的天气区写入状态，不重置当前天气过程。
    pub(crate) fn set_region(&mut self, region: WeatherRegion) {
        self.region = region; // 共享区域内地图切换保持同一份天气状态。
    } // 结束天气区域同步。

    /// 按指定游戏秒、月份和时刻推进自动天气。
    pub(crate) fn advance_game_seconds(
        &mut self,                      // 修改当前状态、剩余时间和随机序号。
        game_seconds: f32,              // 已乘世界时间倍率且感知暂停的游戏秒增量。
        month: u8,                      // 当前月份决定下一天气季节权重。
        hour: f32,                      // 当前时刻决定晨雾生成和强制消散。
        climate: WeatherClimateProfile, // 当前正式地图显式登记的季节权重与局部修正。
    ) {
        self.enforce_mist_time_constraints(hour); // 即使暂停或零增量，拖动时刻到十点后也必须清除晨雾。
        if !game_seconds.is_finite() || game_seconds <= 0.0 {
            return; // 非有限或非正增量不消费稳定期与随机序列。
        } // 结束无效推进增量守卫。
        let mut remaining = game_seconds.min(MAX_STABLE_GAME_SECONDS); // 防止单帧损坏增量造成无界状态循环。
        for _ in 0..256 {
            if remaining <= f32::EPSILON {
                break; // 当前帧游戏秒已全部消费。
            } // 结束剩余增量判断。
            if self.is_transitioning() {
                let duration = self.transition_duration_game_seconds.max(1.0); // 有效过渡至少一游戏秒避免除零。
                let seconds_until_complete = (1.0 - self.transition_progress) * duration; // 根据剩余比例计算完成所需游戏秒。
                self.advance_surface(remaining.min(seconds_until_complete)); // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
                if remaining < seconds_until_complete {
                    self.transition_progress =
                        (self.transition_progress + remaining / duration).clamp(0.0, 1.0); // 只推进本帧可以覆盖的过渡比例。
                    remaining = 0.0; // 当前帧增量已经全部用于过渡。
                    continue; // 退出前继续让循环统一检查剩余值。
                } // 结束部分过渡推进。
                remaining = (remaining - seconds_until_complete).max(0.0); // 消费完成当前过渡所需游戏秒。
                self.current = self.target; // 完成过渡后当前天气进入目标节点。
                self.transition_progress = 1.0; // 稳定期统一使用完整进度。
                self.transition_duration_game_seconds = 0.0; // 完成后清除过渡总时长。
                if self.start_queued_transition_if_needed() {
                    continue; // 多段剧情过渡直接进入下一相邻节点，不插入数小时稳定等待。
                } // 结束排队剧情路径继续判断。
                self.stable_remaining_game_seconds = stable_duration_game_seconds(
                    self.current,            // 根据刚进入的天气种类生成稳定时长。
                    self.next_random_unit(), // 消费一个确定性随机样本。
                    hour,                    // 薄雾稳定期根据当前时刻截断到九点半。
                )
                .max(self.pending_minimum_hold_game_seconds); // 最终剧情目标至少保持命令指定的游戏秒数。
                self.pending_minimum_hold_game_seconds = 0.0; // 最低维持时间已经并入稳定剩余值，后续恢复自动天气。
                self.enforce_mist_time_constraints(hour); // 大步进完成薄雾时再次检查当前时刻限制。
                continue; // 使用尚未消费的增量继续推进稳定期。
            } // 结束过渡期推进。
            self.advance_surface(remaining.min(self.stable_remaining_game_seconds)); // 地表湿润、积水与暂停时间统一处理，预览不污染自然存档。
            if self.stable_remaining_game_seconds > remaining {
                self.stable_remaining_game_seconds -= remaining; // 当前帧只减少稳定期剩余时间。
                remaining = 0.0; // 当前帧增量已经全部消费。
                continue; // 退出前继续让循环统一检查剩余值。
            } // 结束部分稳定期推进。
            remaining = (remaining - self.stable_remaining_game_seconds).max(0.0); // 消费当前稳定期剩余游戏秒。
            self.stable_remaining_game_seconds = 0.0; // 稳定期到期，准备抽取下一节点。
            let next = select_next_weather(
                self.current,            // 只从当前节点自身和直接相邻节点中抽取。
                month,                   // 当前月份应用季节基础权重。
                hour,                    // 当前时刻控制薄雾是否进入候选池。
                climate,                 // 使用当前地图气候配置完成下一次正常抽取。
                self.next_random_unit(), // 消费一个确定性天气选择样本。
            ); // 完成下一天气抽取。
            if next == self.current {
                self.stable_remaining_game_seconds = stable_duration_game_seconds(
                    self.current,            // 自身重抽只延长当前天气稳定期。
                    self.next_random_unit(), // 消费一个确定性持续时长样本。
                    hour,                    // 薄雾延长仍必须受九点半消散上限约束。
                ); // 保存延长后的稳定期。
                continue; // 使用尚未消费的增量继续推进新稳定期。
            } // 结束天气自身重抽处理。
            let transition_duration = transition_duration_game_seconds(
                self.current,            // 根据当前天气判断是否属于雨势快速变化。
                next,                    // 根据相邻目标天气判断过渡类别。
                self.next_random_unit(), // 消费一个确定性普通过渡时长样本。
            ); // 得到十至三十游戏分钟过渡时长。
            self.start_transition(next, transition_duration); // 开始合法相邻天气过渡。
        } // 结束单帧天气状态消费循环。
    } // 结束自动天气推进。

    /// 消费一个确定性随机样本并递增可存档序号。
    fn next_random_unit(&mut self) -> f32 {
        self.sequence = self.sequence.wrapping_add(1); // wrapping保证极端长运行也不会整数溢出崩溃。
        deterministic_random_unit(self.seed, self.sequence) // 使用种子和新序号生成稳定单位样本。
    } // 结束确定性随机样本消费。

    /// 开始从当前节点到直接相邻目标的过渡。
    fn start_transition(&mut self, target: WeatherKind, duration_game_seconds: f32) {
        self.target = safe_next_step(self.current, target); // 防御性保证内部调用也不能创建非法状态边。
        self.transition_progress = 0.0; // 新过渡从起点天气完整效果开始。
        self.transition_duration_game_seconds = duration_game_seconds.max(1.0); // 过渡至少一游戏秒避免除零。
        self.stable_remaining_game_seconds = 0.0; // 过渡期间不同时消费稳定期。
    } // 结束天气过渡启动。

    /// 将当前状态规范化为没有过渡的稳定节点。
    fn finish_as_stable_state(&mut self) {
        self.target = self.current; // 稳定期目标与当前天气一致。
        self.queued_target = None; // 稳定状态不保留尚未执行的剧情最终目标。
        self.queued_transition_duration_game_seconds = 0.0; // 稳定状态没有排队过渡时长。
        self.stable_remaining_game_seconds = self
            .stable_remaining_game_seconds
            .max(self.pending_minimum_hold_game_seconds); // 直接请求当前天气时也要兑现最低维持时间。
        self.pending_minimum_hold_game_seconds = 0.0; // 最低维持时间并入稳定剩余后清除在途字段。
        self.transition_progress = 1.0; // 稳定期在UI中显示过渡已完成。
        self.transition_duration_game_seconds = 0.0; // 稳定期没有过渡总时长。
    } // 结束稳定状态规范化。

    /// 中间节点完成后按状态图继续执行保存的最终天气意图。
    fn start_queued_transition_if_needed(&mut self) -> bool {
        let Some(final_target) = self.queued_target else {
            return false; // 普通自动天气与单段请求完成后直接进入稳定期。
        }; // 结束排队最终目标存在性判断。
        if self.current == final_target {
            self.queued_target = None; // 已到达最终节点时清除剧情意图。
            self.queued_transition_duration_game_seconds = 0.0; // 最终节点不再需要后续时长。
            return false; // 调用方应为最终天气生成正常稳定期。
        } // 结束最终节点完成判断。
        let next = safe_next_step(self.current, final_target); // 沿合法状态图取得下一相邻节点。
        let duration = normalize_transition_duration(
            self.queued_transition_duration_game_seconds, // 所有后续段沿用请求时规范化的时长。
        ); // 完成后续单段时长安全读取。
        if next == final_target {
            self.queued_target = None; // 最后一段已经把最终意图放入直接目标，不再重复排队。
            self.queued_transition_duration_game_seconds = 0.0; // 清除最后一段之后不会再读取的时长。
        } // 结束最后一段排队状态清理。
        self.start_transition(next, duration); // 立即开始下一相邻段，保持整条剧情天气过程连续。
        true // 告知调用方本轮不能生成中间节点稳定期。
    } // 结束排队剧情天气继续执行。

    /// 根据当前时刻强制修复不应继续存在的晨雾过程。
    fn enforce_mist_time_constraints(&mut self, hour: f32) {
        if !hour.is_finite() {
            return; // 非有限时刻不能参与晨雾窗口计算，保留当前合法状态。
        } // 结束无效时刻守卫。
        let outside_lifetime =
            !(MIST_GENERATION_START_HOUR..MIST_DISPERSAL_END_HOUR).contains(&hour); // 三点前或十点后不允许薄雾继续存在。
        if outside_lifetime
            && (self.current == WeatherKind::Mist || self.target == WeatherKind::Mist)
        {
            self.current = if self.current == WeatherKind::Mist {
                WeatherKind::Clear // 异常薄雾当前节点直接清理为可用晴天。
            } else {
                self.current // 尚未进入薄雾的异常目标直接取消并保留当前天气。
            }; // 完成异常薄雾当前节点修复。
            self.pending_minimum_hold_game_seconds = 0.0; // 晨雾硬性生命期优先于剧情维持时间，不能把时长错误转移给晴天。
            self.finish_as_stable_state(); // 清除所有薄雾过渡字段。
            self.stable_remaining_game_seconds = self
                .stable_remaining_game_seconds
                .max(INITIAL_CLEAR_GAME_SECONDS); // 时刻跳转后至少给修复天气四小时稳定期。
            return; // 完成硬性晨雾生命期修复。
        } // 结束晨雾硬清理处理。
        if hour < MIST_DISPERSAL_START_HOUR {
            return; // 九点半前保留合法晨雾稳定期和自然相邻过渡。
        } // 结束主动散雾开始时刻守卫。
        self.reverse_incoming_mist_transition(); // 正在形成的薄雾先无跳变地反向，不能继续加重到十点再硬切。
        if self.current != WeatherKind::Mist {
            return; // 没有薄雾端点时无需安排十点截止路径。
        } // 结束薄雾端点存在性判断。
        let seconds_until_clear =
            ((MIST_DISPERSAL_END_HOUR - hour) * GAME_SECONDS_PER_HOUR).max(1.0); // F1可直接拖到九点半后的任意时刻，过渡必须在十点前用完剩余时间。
        self.pending_minimum_hold_game_seconds = 0.0; // 晨雾生命期优先于尚未兑现的剧情维持时间。
        if self.target == WeatherKind::Clear && self.is_transitioning() {
            self.queued_target = None; // 直接散雾已经是最短路径，不保留旧剧情最终目标。
            self.queued_transition_duration_game_seconds = 0.0; // 直接转晴不需要第二段时长。
            let remaining_progress = (1.0 - self.transition_progress).max(f32::EPSILON); // 保留当前视觉混合比例，不能因拖动时刻让雾量跳回起点。
            self.transition_duration_game_seconds = (seconds_until_clear / remaining_progress)
                .clamp(1.0, MAX_SNAPSHOT_TRANSITION_GAME_SECONDS); // 调整总时长并保持可安全存档；极端接近完成后向回拖时间时允许提前散尽，不重新加重雾量。
        } else if self.is_transitioning() {
            let remaining_progress = (1.0 - self.transition_progress).max(f32::EPSILON); // 记录薄雾到当前相邻目标尚未完成的比例。
            let segment_duration = (seconds_until_clear / (remaining_progress + 1.0)).max(1.0); // 当前段剩余比例与下一整段使用同一速率，并共同在十点前完成。
            self.transition_duration_game_seconds = segment_duration; // 保留当前目标和进度，只调整离雾段速度而不重启画面。
            self.queued_target = Some(WeatherKind::Clear); // 先平滑抵达当前相邻节点，再继续转晴。
            self.queued_transition_duration_game_seconds = segment_duration; // 下一相邻段沿用相同单位混合速度。
            self.stable_remaining_game_seconds = 0.0; // 两段之间不能插入普通稳定等待。
        } else {
            self.queued_target = None; // 稳定薄雾直接采用最短转晴路径。
            self.queued_transition_duration_game_seconds = 0.0; // 清除可能残留的旧剧情路径时长。
            self.start_transition(WeatherKind::Clear, seconds_until_clear); // 从当前时刻起平滑散雾，避免九点五十九分仍重开完整半小时。
        } // 结束九点半后薄雾连续散尽路径安排。
    } // 结束晨雾时刻约束。

    /// 把九点半后仍在形成的薄雾过渡无跳变地反向为离雾过渡。
    fn reverse_incoming_mist_transition(&mut self) {
        if self.current == WeatherKind::Mist || self.target != WeatherKind::Mist {
            return; // 稳定薄雾和已经离开薄雾的路径交给截止时间逻辑处理。
        } // 结束入雾过渡筛选。
        self.pending_minimum_hold_game_seconds = 0.0; // 晨雾硬性生命期取消原目标的剧情最低维持要求。
        self.queued_target = None; // 原先入雾后的跨节点意图不能越过十点截止点。
        self.queued_transition_duration_game_seconds = 0.0; // 清除与旧最终意图绑定的后续单段时长。
        if !self.is_transitioning() || self.transition_progress <= f32::EPSILON {
            self.finish_as_stable_state(); // 尚未产生可见薄雾时直接取消目标，画面精确保持原天气。
            self.stable_remaining_game_seconds = self.stable_remaining_game_seconds.max(1.0); // 至少保留一游戏秒，避免同一大步进立刻重新抽取。
            return; // 完成零进度入雾取消。
        } // 结束无可见薄雾快速取消。
        let previous_source = self.current; // 保存晴天或阴天起点作为反向离雾目标。
        self.current = WeatherKind::Mist; // 交换插值端点后从薄雾端反向走回原天气。
        self.target = previous_source; // 晴↔雾、阴↔雾都是合法对称状态边。
        self.transition_progress = (1.0 - self.transition_progress).clamp(0.0, 1.0); // 平滑曲线关于中点对称，交换端点和互补进度可精确保持当前画面。
        self.stable_remaining_game_seconds = 0.0; // 反向过渡期间不消费稳定期。
    } // 结束入雾过渡连续反向。
} // 结束天气状态公开与内部逻辑实现。

mod normalization;

use normalization::*;

#[cfg(test)]
mod tests;
