// 时钟ECS系统只消费虚拟时间增量；应用自行决定何时恢复环境日期。

use bevy::prelude::*; // Time、Virtual、Res 与 ResMut 用于时钟系统参数。
use feathrise_settings::EnvironmentPreviewSettings; // F1可持久化暂停只冻结世界时钟，不争用统一玩法暂停资源。

use crate::WorldClock; // 全局世界时钟资源。
use crate::time::constants::GAME_TIME_SCALE; // 固定二十四倍时间倍率。

/// 按暂停感知的虚拟时间推进世界时钟。
pub(crate) fn advance_world_clock(
    time: Res<Time<Virtual>>,      // ESC、TAB、F1 等界面会统一暂停该虚拟时间。
    settings: Res<EnvironmentPreviewSettings>,   // F1世界时间暂停开关允许关闭面板后继续固定太阳轨迹。
    mut clock: ResMut<WorldClock>, // 写入本帧推进后的环境时刻和日期。
) {
    if time.is_paused() || settings.world_time_paused {
        return; // 界面暂停或独立世界时间暂停都保持时钟不变，但后者不阻止角色与镜头继续运行。
    }
    clock.advance_seconds(time.delta_secs() * GAME_TIME_SCALE); // 每个现实秒推进二十四个游戏秒。
}

#[cfg(test)] // ECS 推进测试验证 Time<Virtual> 与固定倍率的连接。
mod tests {
    use super::*; // 测试直接注册内部推进系统。
    use std::time::Duration; // Time::advance_by 使用标准时长。

    #[test] // 一现实秒必须精确推进二十四游戏秒。
    fn virtual_second_advances_twenty_four_game_seconds() {
        let mut app = App::new(); // 使用最小 ECS 应用隔离验证时钟系统。
        app.init_resource::<Time<Virtual>>(); // 手动提供推进系统依赖的虚拟时间资源。
        app.init_resource::<EnvironmentPreviewSettings>(); // 默认关闭F1世界时间暂停，验证正式二十四倍推进。
        app.init_resource::<WorldClock>(); // 插入六月十五日正午默认时钟。
        app.add_systems(Update, advance_world_clock); // 只注册被测自动推进系统。
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_secs(1)); // 模拟一秒未暂停虚拟时间。
        app.update(); // 执行一次 Update 调度。
        let clock = app.world().resource::<WorldClock>(); // 读取推进后的时钟。
        assert!((clock.hour_wrapped() - (12.0 + 24.0 / 3600.0)).abs() < 0.000_001); // 一秒后增加二十四游戏秒。
    }

    #[test] // 暂停调用当帧即使仍保留非零delta，也不得推进世界时间。
    fn paused_virtual_time_does_not_advance_world_clock() {
        let mut app = App::new(); // 构造最小ECS应用隔离验证暂停守卫。
        app.init_resource::<Time<Virtual>>(); // 插入可手动推进与暂停的虚拟时间资源。
        app.init_resource::<EnvironmentPreviewSettings>(); // 默认设置不额外冻结世界时钟。
        app.init_resource::<WorldClock>(); // 插入六月十五日正午默认时钟。
        app.add_systems(Update, advance_world_clock); // 注册正式世界时钟推进系统。
        {
            let mut time = app.world_mut().resource_mut::<Time<Virtual>>(); // 取得测试虚拟时间。
            time.advance_by(Duration::from_secs(1)); // 先制造仍可能留在当前帧的非零delta。
            time.pause(); // 模拟ESC、TAB或F1在当前Update内请求暂停。
        }
        app.update(); // 执行带暂停状态和旧delta的推进系统。
        let clock = app.world().resource::<WorldClock>(); // 读取暂停后的世界时钟。
        assert_eq!(clock.hour_wrapped(), 12.0); // 暂停状态必须保持正午不变。
    }

    #[test] // 独立F1暂停必须只冻结世界时钟，即使虚拟时间仍在正常推进。
    fn test_setting_pauses_world_clock_without_pausing_virtual_time() {
        let mut app = App::new(); // 构造最小ECS应用隔离验证调试暂停开关。
        app.init_resource::<Time<Virtual>>(); // 保持虚拟时间运行以模拟角色和镜头仍可操作。
        app.init_resource::<EnvironmentPreviewSettings>(); // 插入包含世界时间暂停字段的统一设置资源。
        app.world_mut()
            .resource_mut::<EnvironmentPreviewSettings>()
            .world_time_paused = true; // 开启只冻结天体轨迹的F1设置。
        app.init_resource::<WorldClock>(); // 插入六月十五日正午默认时钟。
        app.add_systems(Update, advance_world_clock); // 注册正式世界时钟推进系统。
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_secs(1)); // 模拟角色和镜头仍可消费的一秒虚拟时间。
        app.update(); // 执行带独立暂停设置的推进系统。
        {
            let time = app.world().resource::<Time<Virtual>>(); // 读取未被本设置接管的虚拟时间状态。
            let clock = app.world().resource::<WorldClock>(); // 读取应保持固定的世界时钟。
            assert!(!time.is_paused()); // 独立世界时间暂停不能冻结整个游戏。
            assert_eq!(clock.hour_wrapped(), 12.0); // 即使虚拟时间有增量，太阳时刻仍必须保持正午。
        } // 释放只读借用，继续模拟用户关闭独立暂停开关。
        app.world_mut()
            .resource_mut::<EnvironmentPreviewSettings>()
            .world_time_paused = false; // 关闭F1独立暂停后应从当前固定时刻恢复推进。
        app.world_mut()
            .resource_mut::<Time<Virtual>>()
            .advance_by(Duration::from_secs(1)); // 再提供一秒虚拟时间验证恢复路径。
        app.update(); // 执行恢复世界时钟推进后的首帧。
        let resumed_clock = app.world().resource::<WorldClock>(); // 读取恢复后的世界时刻。
        assert!(resumed_clock.hour_wrapped() > 12.0); // 关闭开关后必须从原正午继续前进而非永久冻结。
    } // 结束独立世界时间暂停测试。

    #[test] // 关闭面板后的首个未暂停帧即使 delta 为零，也必须把二十四点哨兵换到次日零点。
    fn unpaused_zero_delta_normalizes_twenty_four_hour_sentinel() {
        let mut app = App::new(); // 构造最小 ECS 应用复现恢复推进首帧。
        app.init_resource::<Time<Virtual>>(); // 默认虚拟时间保持未暂停且增量为零。
        app.init_resource::<EnvironmentPreviewSettings>(); // 默认关闭F1世界时间暂停，允许二十四点哨兵正常归一化。
        app.init_resource::<WorldClock>(); // 插入六月十五日正午默认时钟。
        app.world_mut().resource_mut::<WorldClock>().set_hour(24.0); // 模拟暂停面板把滑块停在二十四点右端。
        app.add_systems(Update, advance_world_clock); // 注册正式世界时钟推进系统。
        app.update(); // 执行未暂停但零增量的恢复首帧。
        let clock = app.world().resource::<WorldClock>(); // 读取规范化后的日期和时刻。
        assert_eq!((clock.month(), clock.day()), (6, 16)); // 二十四点哨兵必须推进到次日。
        assert_eq!(clock.hour_for_display(), 0.0); // 恢复后显示必须回到零点左端。
    }

}
