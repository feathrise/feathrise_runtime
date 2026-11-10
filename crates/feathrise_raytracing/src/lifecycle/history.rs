//! 只在真实视图或天体光不连续时请求 Solari 丢弃时间历史。

use bevy::{
    camera::Projection, // 投影矩阵快照覆盖FOV、裁剪面、正交/透视与自定义投影变化。
    light::SunDisk,     // 有效光源角直径属于 Solari 时间历史身份。
    prelude::*,         // Camera、GlobalTransform、DirectionalLight与ECS系统参数。
};
use feathrise_atmosphere::{WorldCelestialLight, WorldCelestialLightKind}; // 按固定兼容、太阳和月亮稳定身份跟踪全部玩法天体光。
use bevy_solari::prelude::SolariLighting; // reset是Solari公开的时间历史失效入口。

use crate::{
    components::SolariHistoryState,
    constants::{
        CAMERA_CUT_ANGLE_RADIANS, CAMERA_TELEPORT_DISTANCE_METERS, CELESTIAL_LIGHT_COLOR_JUMP,
        CELESTIAL_LIGHT_DIRECTION_JUMP_RADIANS, CELESTIAL_LIGHT_REFERENCE_FLOOR_LUX,
        CELESTIAL_LIGHT_RELATIVE_JUMP,
    },
    resources::RaytracingRuntime,
    types::{CelestialLightSnapshot, RaytracingPrototypeStage, SolariHistoryResetReason},
};

/// Solari 历史中固定兼容、太阳和月亮三个天体光稳定槽位数。
const CELESTIAL_LIGHT_SLOT_COUNT: usize = 3; // 固定数组避免逐帧分配，也不受 ECS 查询顺序影响。

/// 在相机最终世界变换和物理目标尺寸都收敛后，比较本帧与上一帧历史身份。
pub(crate) fn reset_solari_history_on_discontinuity(
    mut commands: Commands, // 首个Solari帧为目标相机建立轻量历史快照。
    mut runtime: ResMut<RaytracingRuntime>, // 只处理当前会话相机，并把本帧reset原因原子提交给F1诊断。
    celestial_lights: Query<
        (
            &WorldCelestialLight, // 稳定职责决定快照写入固定兼容、太阳或月亮槽位。
            &DirectionalLight,    // 当前时钟与天气合成后的最终照度和颜色。
            &GlobalTransform,     // Transform 传播后的最终光线方向。
            &Visibility,          // 隐藏零照度动态灯不属于本帧 Solari 光源集合。
            Option<&SunDisk>,     // 当前有效光源角直径影响软阴影采样身份。
        ),
        With<WorldCelestialLight>,
    >, // 最终世界方向、光照参数与有效角直径共同定义天体光历史。
    mut cameras: Query<(
        &GlobalTransform,
        &Camera,
        &Projection,
        &mut SolariLighting,
        Option<&mut SolariHistoryState>,
    )>,
) {
    if !matches!(
        runtime.stage,
        RaytracingPrototypeStage::WarmingSolari | RaytracingPrototypeStage::Active
    ) || !runtime.solari_inserted
    {
        runtime.pause_solari_history_tracking(); // 关闭态不增长稳定帧，但保留最近原因和累计次数供审计。
        return; // 光栅与代理准备阶段没有Solari历史，也不扫描其他相机。
    }
    let Some(camera_entity) = runtime.camera else {
        runtime.pause_solari_history_tracking(); // 相机换代帧不能沿用上一会话的稳定帧计数。
        return; // 生命周期协调器会对相机换代执行完整回退。
    };
    let Ok((global, camera, projection, mut solari, previous)) = cameras.get_mut(camera_entity)
    else {
        runtime.pause_solari_history_tracking(); // 实体刚销毁时清除尚未结算的同帧reset原因。
        return; // 相机刚被销毁时由下一Update的统一回退事务处理。
    };

    let (_, rotation, translation) = global.to_scale_rotation_translation();
    let mut current_celestial_lights = [None; CELESTIAL_LIGHT_SLOT_COUNT]; // 每帧只构造三个 Copy 快照，不产生堆分配。
    for (profile, light, light_global, visibility, sun_disk) in &celestial_lights {
        if *visibility == Visibility::Hidden {
            continue; // 与 Bevy 提取一致，隐藏太阳或月亮不进入当前 Solari 光源集合。
        }
        let slot = match profile.kind() {
            WorldCelestialLightKind::LegacyCombined => 0, // 固定 HN 场景单灯占兼容槽位。
            WorldCelestialLightKind::Sun => 1,            // 动态场景太阳灯占固定太阳槽位。
            WorldCelestialLightKind::Moon => 2,           // 动态场景月亮灯占固定月亮槽位。
        };
        current_celestial_lights[slot] = Some(celestial_snapshot(light, light_global, sun_disk)); // 任一灯的照度、颜色、方向或角直径都进入历史身份。
    }
    let current = SolariHistoryState {
        translation,
        rotation,
        target_size: camera.physical_target_size(),
        projection_bits: projection_bits(projection),
        scene_generation: runtime.generation,
        celestial_lights: current_celestial_lights, // 固定槽位同时覆盖单灯固定场景和双灯动态场景。
    };

    let Some(mut previous) = previous else {
        runtime.note_solari_history_reset(SolariHistoryResetReason::InitialHistory); // 首帧无历史必须稳定记录一次，不能被默认reset值隐藏。
        if !solari.reset {
            solari.reset = true; // 外部重建历史组件时也必须从无历史状态重新开始。
        }
        commands.entity(camera_entity).insert(current); // 默认Solari首帧本就reset=true，只保存比较基线。
        runtime.finish_solari_history_frame(); // 初始reset只累计一个帧事件并把连续稳定帧清零。
        return;
    };

    if let Some(reason) = history_discontinuity_reason(&previous, &current) {
        runtime.note_solari_history_reset(reason); // 记录首要不连续字段，F1可直接识别投影或天体光逐帧漂移。
        if !solari.reset {
            solari.reset = true; // 只有真实不连续且当前尚未请求时写入，稳定帧不重复触碰Solari组件。
        }
    }
    if solari.reset && !runtime.has_pending_solari_history_reset() {
        runtime.note_solari_history_reset(SolariHistoryResetReason::ExternalRequest); // 捕获其他系统直接写reset的未知反馈环。
    }
    if *previous != current {
        *previous = current; // 相机连续运动仍逐帧推进比较基线，避免把缓慢移动累计误判为瞬移。
    }
    runtime.finish_solari_history_frame(); // PostUpdate末尾统一决定本帧是reset还是稳定历史帧。
}

fn projection_bits(projection: &Projection) -> [u32; 16] {
    projection
        .get_clip_from_view()
        .to_cols_array()
        .map(f32::to_bits) // 位级快照同时稳定识别有限参数变化和损坏的NaN投影。
}

fn celestial_snapshot(
    light: &DirectionalLight,   // 当前时钟与天气合成后的光照参数。
    global: &GlobalTransform,   // Transform传播后的最终世界旋转。
    sun_disk: Option<&SunDisk>, // 动态场景最终角直径由氛围系统写入同一光实体。
) -> CelestialLightSnapshot {
    let color = light.color.to_linear();
    let (_, rotation, _) = global.to_scale_rotation_translation(); // 忽略不影响方向光的平移与缩放。
    CelestialLightSnapshot {
        illuminance: light.illuminance,
        color: Vec3::new(color.red, color.green, color.blue),
        direction: rotation * Vec3::NEG_Z, // Bevy方向光沿局部-Z前向照射。
        angular_diameter_radians: sun_disk.map_or(0.0, |disk| disk.angular_size), // 无光盘的旧测试灯使用稳定零身份。
    }
}

fn history_discontinuity_reason(
    previous: &SolariHistoryState, // 上一帧已经提交给Solari历史的轻量身份。
    current: &SolariHistoryState,  // 当前帧最终相机、场景与天体光身份。
) -> Option<SolariHistoryResetReason> {
    if previous.scene_generation != current.scene_generation {
        return Some(SolariHistoryResetReason::SceneGenerationChanged); // 切换场景优先于同帧相机位置变化。
    }
    if previous.translation.distance(current.translation) > CAMERA_TELEPORT_DISTANCE_METERS {
        return Some(SolariHistoryResetReason::CameraTranslation); // 正常连续跟随不会累计成瞬移。
    }
    if previous.rotation.angle_between(current.rotation) > CAMERA_CUT_ANGLE_RADIANS {
        return Some(SolariHistoryResetReason::CameraRotation); // 只识别单帧快速切镜。
    }
    if previous.target_size != current.target_size {
        return Some(SolariHistoryResetReason::TargetSizeChanged); // 窗口缩放常会同时改变投影，优先报告更直接的尺寸根因。
    }
    if previous.projection_bits != current.projection_bits {
        return Some(SolariHistoryResetReason::ProjectionChanged); // 尺寸稳定时的逐位变化用于暴露FOV或投影抖动。
    }
    if previous
        .celestial_lights
        .into_iter()
        .zip(current.celestial_lights)
        .any(|(previous, current)| celestial_light_jumped(previous, current))
    {
        return Some(SolariHistoryResetReason::CelestialLightChanged); // 连续昼夜变化低于阈值，不应逐帧命中。
    }
    None // 所有稳定身份均一致或只发生阈值内连续变化。
}

#[cfg(test)]
fn history_discontinuous(previous: &SolariHistoryState, current: &SolariHistoryState) -> bool {
    history_discontinuity_reason(previous, current).is_some() // 既有布尔测试继续验证总体重置边界。
}

fn celestial_light_jumped(
    previous: Option<CelestialLightSnapshot>,
    current: Option<CelestialLightSnapshot>,
) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => {
            let reference = previous
                .illuminance
                .abs()
                .max(current.illuminance.abs())
                .max(CELESTIAL_LIGHT_REFERENCE_FLOOR_LUX); // 晨昏连续近零变化不因百分比分母过小而逐帧重置。
            let relative_jump = (current.illuminance - previous.illuminance).abs() / reference;
            let direction_jump = previous.direction.angle_between(current.direction); // 连续昼夜每帧变化远低于十五度阈值。
            relative_jump > CELESTIAL_LIGHT_RELATIVE_JUMP
                || previous.color.distance(current.color) > CELESTIAL_LIGHT_COLOR_JUMP
                || previous.angular_diameter_radians.to_bits()
                    != current.angular_diameter_radians.to_bits()
                || !direction_jump.is_finite()
                || direction_jump > CELESTIAL_LIGHT_DIRECTION_JUMP_RADIANS
        }
        (None, None) => false,
        _ => true, // 天体光新增或移除会改变Solari采样集合，必须清空旧历史。
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(translation: Vec3) -> SolariHistoryState {
        SolariHistoryState {
            translation,
            rotation: Quat::IDENTITY,
            target_size: Some(UVec2::new(1920, 1080)),
            projection_bits: [0; 16],
            scene_generation: 7,
            celestial_lights: [
                Some(CelestialLightSnapshot {
                    illuminance: 20_000.0,                            // 固定兼容灯测试基线照度。
                    color: Vec3::ONE,                                 // 固定兼容灯测试基线颜色。
                    direction: Vec3::NEG_Z,                           // 固定兼容灯测试基线方向。
                    angular_diameter_radians: 0.533_f32.to_radians(), // 固定兼容灯地球太阳角直径。
                }),
                None, // 基础相机测试不创建动态太阳灯。
                None, // 基础相机测试不创建动态月亮灯。
            ],
        }
    }

    #[test]
    fn stable_or_small_camera_motion_does_not_reset_history() {
        let previous = state(Vec3::ZERO);
        let current = state(Vec3::new(11.9, 0.0, 0.0));
        assert!(!history_discontinuous(&previous, &current));
    }

    #[test]
    fn teleport_projection_resize_and_generation_each_reset_history() {
        let previous = state(Vec3::ZERO);
        assert!(history_discontinuous(
            &previous,
            &state(Vec3::new(12.1, 0.0, 0.0))
        ));

        let mut projection_changed = previous;
        projection_changed.projection_bits[3] = 1;
        assert!(history_discontinuous(&previous, &projection_changed));

        let mut resized = previous;
        resized.target_size = Some(UVec2::new(2560, 1440));
        assert!(history_discontinuous(&previous, &resized));

        let mut regenerated = previous;
        regenerated.scene_generation += 1;
        assert!(history_discontinuous(&previous, &regenerated));
    }

    #[test]
    fn discontinuity_diagnostics_distinguish_each_runtime_identity() {
        let previous = state(Vec3::ZERO); // 使用同一稳定基线逐项只改变一个身份字段。

        let translated = state(Vec3::new(12.1, 0.0, 0.0)); // 超过固定十二米单帧阈值。
        assert_eq!(
            history_discontinuity_reason(&previous, &translated),
            Some(SolariHistoryResetReason::CameraTranslation)
        );

        let mut rotated = previous; // Copy快照允许独立构造旋转不连续。
        rotated.rotation = Quat::from_rotation_y(CAMERA_CUT_ANGLE_RADIANS * 1.1); // 超过固定四十五度阈值。
        assert_eq!(
            history_discontinuity_reason(&previous, &rotated),
            Some(SolariHistoryResetReason::CameraRotation)
        );

        let mut resized = previous; // 尺寸变化应优先于可能伴随发生的投影变化。
        resized.target_size = Some(UVec2::new(2560, 1440)); // 模拟窗口物理尺寸改变。
        assert_eq!(
            history_discontinuity_reason(&previous, &resized),
            Some(SolariHistoryResetReason::TargetSizeChanged)
        );

        let mut projection_changed = previous; // 尺寸稳定时单独制造投影矩阵位差。
        projection_changed.projection_bits[3] = 1; // 暴露相机系统逐帧投影漂移时应显示的原因。
        assert_eq!(
            history_discontinuity_reason(&previous, &projection_changed),
            Some(SolariHistoryResetReason::ProjectionChanged)
        );

        let mut regenerated = previous; // 场景换代必须使用独立诊断原因。
        regenerated.scene_generation += 1; // 模拟切换场景或GLB热重载后的新代次。
        assert_eq!(
            history_discontinuity_reason(&previous, &regenerated),
            Some(SolariHistoryResetReason::SceneGenerationChanged)
        );

        let mut celestial_changed = previous; // 天体光单帧突变不依赖相机变化。
        celestial_changed.celestial_lights[0]
            .as_mut()
            .unwrap()
            .illuminance = 5_000.0; // 相对变化超过固定阈值。
        assert_eq!(
            history_discontinuity_reason(&previous, &celestial_changed),
            Some(SolariHistoryResetReason::CelestialLightChanged)
        );
    }

    #[test]
    fn reset_diagnostics_count_once_per_frame_and_preserve_first_reason() {
        let mut runtime = RaytracingRuntime::default(); // 私有运行时直接验证O(1)计数事务。
        runtime.note_solari_history_reset(SolariHistoryResetReason::MaterialReprepared); // Update先观察到材质重新准备。
        runtime.note_solari_history_reset(SolariHistoryResetReason::ProjectionChanged); // PostUpdate同帧又观察到投影变化。
        runtime.finish_solari_history_frame(); // 两个来源必须只提交一个reset帧。

        assert_eq!(runtime.history_reset_count, 1); // 同帧来源不能重复累计。
        assert_eq!(runtime.history_stable_frames, 0); // reset帧不计入稳定历史。
        assert_eq!(
            runtime.history_last_reset_reason,
            Some(SolariHistoryResetReason::MaterialReprepared)
        ); // 首个真正触发reset的位置保持为最近根因。

        runtime.finish_solari_history_frame(); // 下一帧没有任何来源即为稳定帧。
        assert_eq!(runtime.history_reset_count, 1); // 稳定帧不能改变累计次数。
        assert_eq!(runtime.history_stable_frames, 1); // 连续稳定帧开始增长。

        runtime.pause_solari_history_tracking(); // 模拟关闭请求或完整回退。
        assert_eq!(runtime.history_stable_frames, 0); // 下一会话不能继承旧稳定帧。
        assert_eq!(runtime.history_reset_count, 1); // 过程累计证据必须保留。
        assert_eq!(
            runtime.history_last_reset_reason,
            Some(SolariHistoryResetReason::MaterialReprepared)
        ); // 最近原因在关闭态仍供F1读取。
    }

    #[test]
    fn celestial_threshold_uses_relative_illuminance_and_linear_color() {
        let base = CelestialLightSnapshot {
            illuminance: 10_000.0,
            color: Vec3::ONE,
            direction: Vec3::NEG_Z,
            angular_diameter_radians: 0.533_f32.to_radians(),
        };
        assert!(!celestial_light_jumped(
            Some(base),
            Some(CelestialLightSnapshot {
                illuminance: 11_000.0,
                color: Vec3::new(0.95, 1.0, 1.0),
                direction: Vec3::NEG_Z,
                angular_diameter_radians: base.angular_diameter_radians,
            })
        ));
        assert!(celestial_light_jumped(
            Some(base),
            Some(CelestialLightSnapshot {
                illuminance: 7_000.0,
                color: Vec3::ONE,
                direction: Vec3::NEG_Z,
                angular_diameter_radians: base.angular_diameter_radians,
            })
        ));
        assert!(!celestial_light_jumped(
            Some(CelestialLightSnapshot {
                illuminance: 0.0,
                ..base
            }),
            Some(CelestialLightSnapshot {
                illuminance: 5.0,
                ..base
            })
        )); // 日出日落附近的小幅连续照度变化不能因接近零的分母被误判为突变。
    }

    #[test]
    fn celestial_direction_resets_only_after_a_large_single_frame_jump() {
        let base = CelestialLightSnapshot {
            illuminance: 10_000.0,
            color: Vec3::ONE,
            direction: Vec3::NEG_Z,
            angular_diameter_radians: 0.533_f32.to_radians(),
        };
        let continuous_direction =
            Quat::from_rotation_y(CELESTIAL_LIGHT_DIRECTION_JUMP_RADIANS * 0.9) * Vec3::NEG_Z;
        assert!(!celestial_light_jumped(
            Some(base),
            Some(CelestialLightSnapshot {
                direction: continuous_direction,
                ..base
            })
        )); // 正常昼夜连续运动推进比较基线，不逐帧重置。
        let discontinuous_direction =
            Quat::from_rotation_y(CELESTIAL_LIGHT_DIRECTION_JUMP_RADIANS * 1.1) * Vec3::NEG_Z;
        assert!(celestial_light_jumped(
            Some(base),
            Some(CelestialLightSnapshot {
                direction: discontinuous_direction,
                ..base
            })
        )); // F12调整时刻或时间换代造成的大角度突变必须清空旧历史。
    }

    #[test]
    fn celestial_angular_diameter_change_always_resets_history() {
        let base = CelestialLightSnapshot {
            illuminance: 10_000.0,  // 其他身份保持稳定以隔离角直径。
            color: Vec3::ONE,       // 颜色不参与本断言变化。
            direction: Vec3::NEG_Z, // 方向不参与本断言变化。
            angular_diameter_radians: 0.533_f32.to_radians(), // 地球太阳角直径起点。
        };
        assert!(celestial_light_jumped(
            Some(base),
            Some(CelestialLightSnapshot {
                angular_diameter_radians: 1.0_f32.to_radians(), // F12 修改光源范围后旧软阴影样本失效。
                ..base
            })
        ));
    }

    #[test] // 双方向光的稳定槽位必须同时覆盖日月交接和任一活动灯参数突变。
    fn dynamic_sun_and_moon_slots_each_participate_in_history_identity() {
        let sun = CelestialLightSnapshot {
            illuminance: 1_000.0, // 使用可见太阳照度建立白天历史身份。
            color: Vec3::ONE,     // 日光颜色保持中性以隔离槽位切换。
            direction: Vec3::new(-1.0, 0.1, 0.0).normalize(), // 太阳位于西侧低空。
            angular_diameter_radians: 0.533_f32.to_radians(), // 太阳采用作者角直径。
        }; // 完成动态太阳快照。
        let moon = CelestialLightSnapshot {
            illuminance: 130.0,                 // 月升半小时使用低于午夜峰值的连续照度。
            color: Vec3::new(0.42, 0.52, 0.78), // 月亮使用独立冷色作者值。
            direction: Vec3::new(1.0, 0.1, 0.0).normalize(), // 月亮位于东方低空而非天顶。
            angular_diameter_radians: 0.518_f32.to_radians(), // 月亮采用固定真实角直径。
        }; // 完成动态月亮快照。
        let mut daytime = state(Vec3::ZERO); // 复用稳定相机、投影和场景代次。
        daytime.celestial_lights = [None, Some(sun), None]; // 白天只有太阳槽进入 Solari 光源集合。
        let mut evening = daytime; // 日月交接不改变相机或场景身份。
        evening.celestial_lights = [None, None, Some(moon)]; // 夜间只把月亮写入固定月亮槽。
        assert_eq!(
            history_discontinuity_reason(&daytime, &evening), // 光源集合从太阳换到月亮必须丢弃旧直射历史。
            Some(SolariHistoryResetReason::CelestialLightChanged)
        ); // 完成日月槽位切换断言。

        let mut changed_moon = evening; // 保持太阳槽为空并只修改月亮参数。
        changed_moon.celestial_lights[2]
            .as_mut()
            .unwrap()
            .angular_diameter_radians = 0.6_f32.to_radians(); // 任一活动月亮光源范围变化都必须失效软阴影历史。
        assert_eq!(
            history_discontinuity_reason(&evening, &changed_moon), // 比较固定月亮槽的独立变化。
            Some(SolariHistoryResetReason::CelestialLightChanged)
        ); // 完成月亮槽位历史断言。
    }
}
