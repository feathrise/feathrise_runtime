// 地表天气纯逻辑：固定成本推进湿润与积水，并隔离作者预览和持久状态。
use super::output::{natural_precipitation_at, weather_profile};
use crate::weather::resources::{WeatherDebugOverride, WeatherOutput, WeatherState}; // 复用唯一天气状态和输出。 // 自然雨量使用正式分阶段曲线。

pub(super) fn unit(value: f32) -> f32 {
    // 防止损坏存档污染地表材质。
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    } // 非有限值安全恢复干燥。
} // 结束归一化。

pub(super) fn normalize_time(value: f32) -> f32 {
    // 保留有限视觉秒并限制长期精度损失。
    if value.is_finite() {
        value.max(0.0).rem_euclid(4096.0)
    } else {
        0.0
    } // 使用统一周期让暂停和读档稳定。
} // 结束时间修复。

fn approach(value: f32, target: f32, seconds: f32, response: f32) -> f32 {
    // 指数响应避免固定帧率依赖。
    unit(target + (value - target) * (-seconds / response).exp()) // 任意正步长均保持有界。
} // 结束连续趋近。

impl WeatherState {
    // 地表历史属于天气区，切换场景不创建另一套资源。
    pub(super) fn advance_surface(&mut self, seconds: f32) {
        // 调用方只传入本段实际消费的游戏秒。
        if !seconds.is_finite() || seconds <= 0.0 {
            return;
        } // 暂停和时刻预览不能消费历史。
        let midpoint = if self.is_transitioning() {
            self.transition_progress()
                + 0.5 * seconds / self.transition_duration_game_seconds().max(1.0)
        } else {
            1.0
        }; // 过渡段按中点近似积分，稳定段保持精确雨量。
        let rain = natural_precipitation_at(self, midpoint); // 永远读取自然雨势，忽略强制预览。
        let wet_target = (rain * 3.0).min(1.0); // 小雨也能充分润湿地面。
        let wet_response = if wet_target > self.surface_wetness {
            240.0
        } else {
            7200.0
        }; // 约数分钟润湿、数小时干燥。
        let old_wetness = self.surface_wetness; // 用解析阈值延迟积水形成，避免大步进立即积水。
        self.surface_wetness = approach(old_wetness, wet_target, seconds, wet_response); // 连续推进湿润。
        let delay = if old_wetness < 0.65 && wet_target > 0.65 {
            // 先达到基本饱和才允许明显积水。
            (wet_response * ((wet_target - old_wetness) / (wet_target - 0.65)).ln()).max(0.0) // 解析求跨阈值所需时间。
        } else if old_wetness < 0.65 {
            seconds
        } else {
            0.0
        }; // 不能达到阈值时整段只湿润。
        let puddle_target = unit((rain - 0.08) / 0.92); // 微量雨不会持续蓄水，小雨只形成局部积水。
        let drying_seconds = seconds.min(delay); // 形成前旧积水仍允许排水。
        self.puddle_amount = approach(self.puddle_amount, 0.0, drying_seconds, 2700.0); // 积水消退快于湿痕。
        self.puddle_amount = approach(
            self.puddle_amount,
            puddle_target,
            seconds - drying_seconds,
            if puddle_target > self.puddle_amount {
                1800.0
            } else {
                2700.0
            },
        ); // 蓄积慢于湿润，雨停自然收缩。
        self.surface_effect_time = normalize_time(
            self.surface_effect_time + seconds / crate::time::constants::GAME_TIME_SCALE,
        ); // 雨纹按视觉秒运行并随游戏暂停。
    } // 结束固定成本地表推进。
} // 结束状态扩展。

pub(super) fn apply_surface_output(
    state: &WeatherState,
    debug: &WeatherDebugOverride,
    output: &mut WeatherOutput,
) {
    // 最终表现不能反写自然存档。
    output.surface_wetness = state.surface_wetness; // 自动模式保留雨后湿痕。
    output.puddle_amount = state.puddle_amount; // 自动模式保留雨后积水。
    output.surface_effect_time = state.surface_effect_time; // 暂停预览显示固定的同源雨纹相位。
    if debug.forced().is_some() || debug.preview_override_active() {
        // 只有主动调试预览才模拟目标地表。
        let target_rain = debug
            .forced()
            .map(weather_profile)
            .map(|profile| profile.precipitation)
            .unwrap_or(output.precipitation); // 强制目标与自动过渡都复用正式雨量。
        let blend = unit(debug.preview()); // 零端准确保留自然历史。
        output.surface_wetness += ((target_rain * 3.0).min(1.0) - output.surface_wetness) * blend; // 暂停时可立即检查湿润外观。
        output.puddle_amount += (unit((target_rain - 0.08) / 0.92) - output.puddle_amount) * blend; // 完整大雨预览同时显示积水。
    } // 结束临时表现覆盖。
} // 结束地表输出合成。

#[cfg(test)] // 独立测试不启动渲染和窗口。
mod tests; // 回归覆盖雨干、暂停、存档和预览隔离。
