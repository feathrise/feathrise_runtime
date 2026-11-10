// 游戏设置 — 窗口目标 3D 相机画质应用系统。

use bevy::{
    anti_alias::{fxaa::Fxaa, taa::TemporalAntiAliasing}, // 单帧 FXAA 与实验性跨帧 TAA 组件。
    camera::{Hdr, RenderTarget},                         // HDR 标记与相机输出目标。
    core_pipeline::{
        prepass::{DepthPrepass, MotionVectorPrepass}, // TAA 历史重投影依赖的两个预通道。
        tonemapping::Tonemapping,                     // Bloom 使用及恢复的色调映射。
    },
    light::ShadowFilteringMethod,              // 窗口相机阴影滤波。
    post_process::bloom::Bloom,                // Bloom 后处理组件。
    prelude::*,                                // ECS 与 3D 相机常用类型。
    render::camera::{MipBias, TemporalJitter}, // TAA 自动使用的纹理锐度与投影抖动组件。
};

use crate::{
    AntiAliasingMode, BloomQuality, RenderSettings, GraphicsRuntimeLeaseRegistry,
    ShadowFilteringQuality,
    components::{
        AuthoredGameplayAmbientOcclusion, AuthoredGameplayContactShadows,
        AuthoredWindowPostProcessing, TaaCameraHistoryPose, TaaSupportOwnership,
    },
}; // 画质模型与相机作者状态备份。

const TAA_HISTORY_TELEPORT_DISTANCE: f32 = 12.0; // 复用项目既有十二米单帧跳变口径识别传送。
const TAA_HISTORY_CUT_ANGLE: f32 = std::f32::consts::FRAC_PI_4; // 单帧旋转超过四十五度按镜头切换清空历史。

/// 将抗锯齿应用到输出窗口的 3D 相机，纹理目标相机保留专用配置。
#[allow(clippy::type_complexity)] // Bevy Query 需要同帧快照全部 TAA 支撑组件与作者所有权，不能拆成有竞态的多次查询。
pub(crate) fn apply_antialiasing(
    mut commands: Commands, // 仅在 FXAA/TAA 组件或 TAA 支撑所有权确需改变时提交命令。
    settings: Res<RenderSettings>, // 当前画质设置。
    runtime_leases: Option<Res<GraphicsRuntimeLeaseRegistry>>, // 独立系统测试未装完整插件时等价于无租约。
    mut cameras: Query<
        (
            Entity,                                    // 当前 3D 相机实体。
            &RenderTarget,                             // 用于隔离窗口与纹理目标。
            &mut Msaa,                     // TAA/FXAA 使用单采样，MSAA 档使用对应采样数。
            Option<&mut Fxaa>,             // 当前可选 FXAA 状态。
            Option<&TemporalAntiAliasing>, // 当前可选 TAA 状态。
            Option<&TemporalJitter>,       // 启用 TAA 前可能存在的作者投影抖动。
            Option<&MipBias>,              // 启用 TAA 前可能存在的作者纹理 mip 偏移。
            Has<DepthPrepass>,             // 记录深度预通道是否由作者预先启用。
            Has<MotionVectorPrepass>,      // 记录运动矢量预通道是否由作者预先启用。
            Option<&TaaSupportOwnership>,  // 本功能已保存的 TAA 支撑组件所有权。
            Option<&AuthoredGameplayAmbientOcclusion>, // 识别深度是否只是由当前 SSAO 临时引入。
            Option<&AuthoredGameplayContactShadows>, // 识别深度是否只是由当前接触阴影临时引入。
        ),
        With<Camera3d>, // 只枚举 3D 相机。
    >,
) {
    let configured_msaa = match settings.graphics.anti_aliasing {
        AntiAliasingMode::Msaa2 => Msaa::Sample2,
        AntiAliasingMode::Msaa4 => Msaa::Sample4,
        AntiAliasingMode::Msaa8 => Msaa::Sample8,
        AntiAliasingMode::Off | AntiAliasingMode::Fxaa | AntiAliasingMode::Taa => Msaa::Off,
    }; // FXAA 与 TAA 都必须关闭多重采样。
    let configured_fxaa = settings.graphics.anti_aliasing == AntiAliasingMode::Fxaa; // 缓存玩家单帧后处理目标。
    let configured_taa = settings.graphics.anti_aliasing == AntiAliasingMode::Taa; // 缓存玩家时间域目标。

    for (
        entity,
        target,
        mut msaa,
        fxaa,
        taa,
        temporal_jitter,
        mip_bias,
        has_depth_prepass,
        has_motion_vector_prepass,
        taa_ownership,
        ambient_occlusion_ownership,
        contact_shadow_ownership,
    ) in &mut cameras
    {
        if !matches!(target, RenderTarget::Window(_)) {
            continue;
        } // 角色预览、小场景等纹理目标不受全局画质干预。
        let runtime_requirements = runtime_leases
            .as_deref()
            .and_then(|registry| registry.requirements_for_camera(entity)); // 每台窗口相机只合并一次当前运行时租约。
        let force_single_sample =
            runtime_requirements.is_some_and(|requirements| requirements.force_single_sample); // Solari与RR都要求关闭MSAA和项目TAA。
        let force_fxaa = runtime_requirements.is_some_and(|requirements| requirements.force_fxaa); // 基础Solari没有RR时才需要单帧FXAA托底。
        let suspend_fxaa =
            runtime_requirements.is_some_and(|requirements| requirements.suspend_fxaa); // RR输出不得再经过第二层FXAA。
        let desired_msaa = if force_single_sample {
            Msaa::Off // Solari等计算路径严格禁止多重采样。
        } else {
            configured_msaa // 非目标相机继续使用玩家当前档位。
        };
        let wants_fxaa = !suspend_fxaa && (force_fxaa || configured_fxaa); // RR优先级高于租约或玩家FXAA，避免二次平滑。
        let wants_taa = !force_single_sample && configured_taa; // 单采样运行时租约暂停项目TAA但不改写玩家请求值。
        if *msaa != desired_msaa {
            *msaa = desired_msaa; // 采样数变化时才让视图管线重新专用化。
        }

        if wants_taa {
            if fxaa.is_some() {
                commands.entity(entity).remove::<Fxaa>(); // TAA 与 FXAA 严格互斥，切换帧先移除单帧后处理。
            }
            if taa_ownership.is_none() {
                commands.entity(entity).insert(TaaSupportOwnership {
                    temporal_jitter_offset: temporal_jitter.map(|jitter| jitter.offset), // 保存作者原有抖动偏移。
                    mip_bias: mip_bias.map(|bias| bias.0), // 保存作者原有纹理 mip 偏移。
                    depth_prepass: ambient_occlusion_ownership.map_or_else(
                        || {
                            contact_shadow_ownership
                                .map_or(has_depth_prepass, |state| state.depth_prepass)
                        }, // 没有 SSAO 时排除接触阴影临时引入的深度预通道。
                        |state| state.depth_prepass, // SSAO 备份已排除更早启用的其他后处理。
                    ),
                    motion_vector_prepass: has_motion_vector_prepass, // 保存作者原有运动矢量预通道资格。
                }); // 只在首次接管本轮 TAA 时记录一次支撑组件基线。
            }
            if taa.is_none() {
                commands
                    .entity(entity)
                    .insert(TemporalAntiAliasing::default()); // Bevy 自动补齐抖动、mip 偏移和两个预通道，并让首帧清空历史。
            }
            continue; // TAA 分支已经完成全部互斥与所有权处理。
        }

        if taa.is_some() {
            commands.entity(entity).remove::<TemporalAntiAliasing>(); // 切回关闭、FXAA 或任一 MSAA 档时停止跨帧累积。
        }
        if let Some(ownership) = taa_ownership {
            restore_taa_support_components(
                &mut commands,             // 通过延迟命令恢复支撑组件。
                entity,                    // 当前窗口相机。
                temporal_jitter,           // 当前可能被 TAA 推进过的投影抖动。
                mip_bias,                  // 当前 TAA 使用的纹理 mip 偏移。
                has_depth_prepass,         // 当前深度预通道存在状态。
                has_motion_vector_prepass, // 当前运动矢量预通道存在状态。
                ownership,                 // 首次启用前保存的作者配置。
            ); // 关闭 TAA 后不得给后续 MSAA/FXAA 留下抖动或无主预通道。
        }
        match (wants_fxaa, fxaa) {
            (true, None) => {
                commands.entity(entity).insert(Fxaa::default()); // 切到 FXAA。
            }
            (true, Some(mut fxaa)) if !fxaa.enabled => {
                fxaa.enabled = true; // 已有但停用的 FXAA 随全局档位启用。
            }
            (false, Some(_)) => {
                commands.entity(entity).remove::<Fxaa>(); // 切回关闭或任一 MSAA 档。
            }
            _ => {} // 稳定帧保持组件不变。
        }
    }
}

/// 关闭 TAA 时恢复作者已有的支撑组件，并只移除本功能临时引入的部分。
fn restore_taa_support_components(
    commands: &mut Commands,                  // 相机延迟组件写入入口。
    entity: Entity,                           // 当前窗口 3D 相机。
    temporal_jitter: Option<&TemporalJitter>, // 当前投影抖动组件。
    mip_bias: Option<&MipBias>,               // 当前纹理 mip 偏移组件。
    has_depth_prepass: bool,                  // 当前是否存在深度预通道。
    has_motion_vector_prepass: bool,          // 当前是否存在运动矢量预通道。
    ownership: &TaaSupportOwnership,          // 启用 TAA 前保存的作者配置。
) {
    match ownership.temporal_jitter_offset {
        Some(offset) if temporal_jitter.is_none_or(|current| current.offset != offset) => {
            commands.entity(entity).insert(TemporalJitter { offset }); // 恢复作者原有抖动值，而不是保留 TAA 最后一帧序列偏移。
        }
        None if temporal_jitter.is_some() => {
            commands.entity(entity).remove::<TemporalJitter>(); // 移除仅由 TAA 自动要求的投影抖动。
        }
        _ => {} // 当前状态已与作者基线一致时不重复写入。
    }
    match ownership.mip_bias {
        Some(value) if mip_bias.is_none_or(|current| current.0 != value) => {
            commands.entity(entity).insert(MipBias(value)); // 恢复作者原有纹理采样偏移。
        }
        None if mip_bias.is_some() => {
            commands.entity(entity).remove::<MipBias>(); // 移除仅由 TAA 自动要求的锐度补偿。
        }
        _ => {} // 当前状态已与作者基线一致时不重复写入。
    }
    match (ownership.depth_prepass, has_depth_prepass) {
        (true, false) => {
            commands.entity(entity).insert(DepthPrepass); // 恢复作者原有深度预通道。
        }
        (false, true) => {
            commands.entity(entity).remove::<DepthPrepass>(); // 移除仅由 TAA 引入的深度预通道。
        }
        _ => {} // 当前状态已与作者基线一致时不重复写入。
    }
    match (ownership.motion_vector_prepass, has_motion_vector_prepass) {
        (true, false) => {
            commands.entity(entity).insert(MotionVectorPrepass); // 恢复作者原有运动矢量预通道。
        }
        (false, true) => {
            commands.entity(entity).remove::<MotionVectorPrepass>(); // 移除仅由 TAA 引入的运动矢量预通道。
        }
        _ => {} // 当前状态已与作者基线一致时不重复写入。
    }
    commands
        .entity(entity)
        .remove::<(TaaSupportOwnership, TaaCameraHistoryPose)>(); // 本轮 TAA 接管结束后清理私有所有权与历史姿态。
}

/// 相机发生同实体传送、大角度镜头切换或投影变化时，清空 TAA 历史避免鬼影。
#[allow(clippy::type_complexity)] // 历史判定必须原子读取相机目标、姿态、投影与 TAA 状态。
pub(crate) fn reset_taa_history_on_camera_discontinuity(
    mut commands: Commands, // 首个 TAA 帧为窗口相机附加轻量历史姿态。
    mut cameras: Query<
        (
            Entity,                            // 当前窗口相机实体。
            &RenderTarget,                     // 纹理目标相机必须保持作者专用行为。
            &GlobalTransform,                  // 读取变换传播后的最新世界姿态。
            Ref<Projection>,                   // FOV 或投影模式变化同样使旧历史失效。
            &mut TemporalAntiAliasing,         // 只在真实不连续时设置一次 reset。
            Option<&mut TaaCameraHistoryPose>, // 本功能保存的上一帧姿态。
        ),
        With<Camera3d>, // 只处理启用了 TAA 的 3D 相机。
    >,
) {
    for (entity, target, global_transform, projection, mut taa, history) in &mut cameras {
        if !matches!(target, RenderTarget::Window(_)) {
            continue; // 角色预览、小场景等纹理目标不参与全局 TAA 历史管理。
        }
        let (_, rotation, translation) = global_transform.to_scale_rotation_translation(); // 从最终世界变换提取历史判定所需姿态。
        let current = TaaCameraHistoryPose {
            translation, // 保存本帧世界坐标供下一帧比较。
            rotation,    // 保存本帧世界旋转供下一帧比较。
        }; // 完成本帧轻量姿态快照。
        let Some(mut history) = history else {
            commands.entity(entity).insert(current); // 首帧建立基线；TAA 默认 reset 已负责清空空历史。
            continue; // 没有旧姿态时不存在可比较的不连续。
        };
        let teleported =
            history.translation.distance(current.translation) > TAA_HISTORY_TELEPORT_DISTANCE; // 十二米以上单帧移动沿用项目传送判定口径。
        let camera_cut = history.rotation.angle_between(current.rotation) > TAA_HISTORY_CUT_ANGLE; // 四十五度以上单帧旋转按镜头切换处理。
        let projection_changed = projection.is_changed() && !projection.is_added(); // 瞄准缩放或投影模式切换不能复用旧历史。
        if (teleported || camera_cut || projection_changed) && !taa.reset {
            taa.reset = true; // 仅在旧历史确实失效且尚未请求重置时写入一次。
        }
        if *history != current {
            *history = current; // 私有 CPU 跟踪组件只在相机姿态真实变化时更新，不触发渲染管线重建。
        }
    }
}

/// 将阴影滤波应用到窗口 3D 相机，不启用依赖 TAA 的 Temporal 模式。
pub(crate) fn apply_shadow_filtering(
    mut commands: Commands,      // 只在滤波组件需要增删时写入。
    settings: Res<RenderSettings>, // 当前画质设置。
    mut cameras: Query<(Entity, &RenderTarget, Option<&mut ShadowFilteringMethod>), With<Camera3d>>, // 只枚举 3D 相机。
) {
    let quality = settings.graphics.shadow_filtering; // 本帧窗口相机共享同一滤波档。
    for (entity, target, filtering) in &mut cameras {
        if !matches!(target, RenderTarget::Window(_)) {
            continue;
        } // 纹理相机保留作者专用滤波。
        match (quality, filtering) {
            (ShadowFilteringQuality::Fast, None) => {
                commands
                    .entity(entity)
                    .insert(ShadowFilteringMethod::Hardware2x2); // 低档显式选择快速硬件滤波。
            }
            (ShadowFilteringQuality::Fast, Some(mut current))
                if *current != ShadowFilteringMethod::Hardware2x2 =>
            {
                *current = ShadowFilteringMethod::Hardware2x2; // 覆盖 Gaussian 或 Temporal。
            }
            (ShadowFilteringQuality::Soft, Some(_)) => {
                commands.entity(entity).remove::<ShadowFilteringMethod>(); // 无组件即使用 Bevy 默认 Gaussian。
            }
            _ => {} // 稳定帧或已为隐式 Gaussian 时不写。
        }
    }
}

/// 将 Bloom 应用到窗口 3D 相机，并在关闭时恢复首次接管前的 HDR 与色调映射。
#[allow(clippy::type_complexity)] // Bloom 接管需在单个 Query 快照中比较 HDR、色调映射和作者备份。
pub(crate) fn apply_bloom(
    mut commands: Commands,      // 只在后处理组件需要增删时写入。
    settings: Res<RenderSettings>, // 当前画质设置。
    runtime_leases: Option<Res<GraphicsRuntimeLeaseRegistry>>, // 独立系统测试未装完整插件时等价于无租约。
    mut cameras: Query<
        (
            Entity,
            &RenderTarget,
            Has<Hdr>,
            Option<&Tonemapping>,
            Option<&mut Bloom>,
            Option<&AuthoredWindowPostProcessing>,
        ),
        With<Camera3d>,
    >, // 只枚举 3D 相机并读取作者状态。
) {
    let desired_bloom = bloom_for_quality(settings.graphics.bloom_quality); // 每帧只构造一次规范 Bloom 参数。
    for (entity, target, has_hdr, tonemapping, bloom, authored) in &mut cameras {
        let window_target = matches!(target, RenderTarget::Window(_)); // 普通设置仍只传播到窗口，明确运行时HDR租约单独授权目标。
        let requires_hdr = runtime_leases
            .as_deref()
            .and_then(|registry| registry.requirements_for_camera(entity))
            .is_some_and(|requirements| requirements.require_hdr); // 纹理目标不会因其他相机租约被误接管。
        if !window_target && !requires_hdr && authored.is_none() {
            continue;
        } // 普通辅助RTT仍完全隔离，只有明确租约及其归还阶段允许接管。
        let desired_bloom = window_target.then_some(desired_bloom.as_ref()).flatten(); // F12水反射只租HDR，绝不继承窗口Bloom画质。
        let needs_takeover =
            desired_bloom.is_some() || bloom.is_some() || authored.is_some() || requires_hdr; // Bloom或运行时HDR任一所有者都需要状态基线。
        if !needs_takeover {
            continue;
        } // 默认关闭且从未接管时不附加任何组件。
        let baseline = authored.copied().unwrap_or(AuthoredWindowPostProcessing {
            hdr: has_hdr,
            tonemapping: tonemapping.copied(),
        }); // 首次接管前先捕获相机作者配置。
        if authored.is_none() {
            commands.entity(entity).insert(baseline); // 每个窗口相机只记录一次作者状态。
        }

        if let Some(desired) = desired_bloom {
            if !has_hdr {
                commands.entity(entity).insert(Hdr); // Bloom 使用 HDR 中间目标。
            }
            if tonemapping.copied() != Some(Tonemapping::TonyMcMapface) {
                commands.entity(entity).insert(Tonemapping::TonyMcMapface); // 使用适合高亮去饱和的显示变换。
            }
            match bloom {
                Some(mut current) if !bloom_matches(&current, desired) => {
                    *current = desired.clone(); // 仅档位参数变化时更新后处理 uniform 与 mip 预算。
                }
                None => {
                    commands.entity(entity).insert(desired.clone()); // 新窗口相机继承当前 Bloom 档。
                }
                _ => {} // 稳定帧保持 Bloom 组件不变。
            }
        } else if requires_hdr {
            if window_target && bloom.is_some() {
                // 非窗口显式HDR租约不拥有作者Bloom，不能移除它。
                commands.entity(entity).remove::<Bloom>(); // 租约只取得HDR，不擅自开启玩家已关闭的Bloom。
            }
            if !has_hdr {
                commands.entity(entity).insert(Hdr); // 保证计算光照写入HDR主目标。
            }
            if tonemapping.copied() != Some(Tonemapping::TonyMcMapface) {
                commands.entity(entity).insert(Tonemapping::TonyMcMapface); // HDR输出使用项目既有稳定显示变换。
            }
        } else {
            if window_target && bloom.is_some() {
                // 释放非窗口HDR只归还HDR/色调映射，不改作者Bloom。
                commands.entity(entity).remove::<Bloom>(); // 关闭全局 Bloom 后处理。
            }
            restore_authored_hdr(&mut commands, entity, has_hdr, baseline.hdr); // 恢复首次接管前的 HDR 所有权。
            restore_authored_tonemapping(
                &mut commands,
                entity,
                tonemapping.copied(),
                baseline.tonemapping,
            ); // 恢复首次接管前的显式色调映射。
            if !window_target {
                commands
                    .entity(entity)
                    .remove::<AuthoredWindowPostProcessing>();
            } // 非窗口租约完整归还后释放快照，后续作者修改与下一轮租约必须使用新基线。
        }
    }
}

/// 按玩家档位构造固定的 Natural Bloom 参数。
fn bloom_for_quality(quality: BloomQuality) -> Option<Bloom> {
    match quality {
        BloomQuality::Off => None,
        BloomQuality::Low => Some(Bloom {
            intensity: 0.08,        // 低档减少散射强度。
            max_mip_dimension: 256, // 低档限制最大 Bloom mip 尺寸。
            ..Bloom::NATURAL        // 其余参数保持自然、能量守恒配置。
        }),
        BloomQuality::High => Some(Bloom {
            intensity: 0.15,        // 高档使用 Natural 标准强度。
            max_mip_dimension: 512, // 高档使用 Natural 标准 mip 上限。
            ..Bloom::NATURAL        // 其余参数保持自然、能量守恒配置。
        }),
    }
}

/// 比较所有会影响 Bloom 管线或 uniform 的字段，稳定帧不触发变更检测。
fn bloom_matches(current: &Bloom, desired: &Bloom) -> bool {
    current.intensity == desired.intensity
        && current.low_frequency_boost == desired.low_frequency_boost
        && current.low_frequency_boost_curvature == desired.low_frequency_boost_curvature
        && current.high_pass_frequency == desired.high_pass_frequency
        && current.prefilter.threshold == desired.prefilter.threshold
        && current.prefilter.threshold_softness == desired.prefilter.threshold_softness
        && current.composite_mode == desired.composite_mode
        && current.max_mip_dimension == desired.max_mip_dimension
        && current.scale == desired.scale
}

/// 恢复相机作者原本的 HDR 标记，仅在状态不一致时提交命令。
fn restore_authored_hdr(
    commands: &mut Commands, // 相机组件写入入口。
    entity: Entity,          // 当前窗口相机。
    current: bool,           // 当前是否存在 HDR。
    authored: bool,          // 首次接管前是否存在 HDR。
) {
    match (authored, current) {
        (true, false) => {
            commands.entity(entity).insert(Hdr); // 恢复作者启用的 HDR。
        }
        (false, true) => {
            commands.entity(entity).remove::<Hdr>(); // 移除仅由 Bloom 引入的 HDR。
        }
        _ => {} // 状态一致时不写。
    }
}

/// 恢复相机作者原本的显式色调映射组件。
fn restore_authored_tonemapping(
    commands: &mut Commands,       // 相机组件写入入口。
    entity: Entity,                // 当前窗口相机。
    current: Option<Tonemapping>,  // 当前显式色调映射。
    authored: Option<Tonemapping>, // 首次接管前的显式色调映射。
) {
    match (authored, current) {
        (Some(desired), Some(current)) if desired != current => {
            commands.entity(entity).insert(desired); // 恢复作者选择的不同映射。
        }
        (Some(desired), None) => {
            commands.entity(entity).insert(desired); // 恢复被移除的作者映射。
        }
        (None, Some(_)) => {
            commands.entity(entity).remove::<Tonemapping>(); // 移除仅由 Bloom 引入的映射。
        }
        _ => {} // 状态一致时不写。
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::WindowRef;

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<RenderSettings>();
        app.add_systems(
            Update,
            (apply_antialiasing, apply_shadow_filtering, apply_bloom),
        );
        app
    }

    #[test]
    fn fxaa_and_msaa_are_exclusive_only_on_window_cameras() {
        let mut app = test_app();
        let window_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::Window(WindowRef::Primary),
                Msaa::Sample4,
            ))
            .id();
        let texture_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::None { size: UVec2::ONE },
                Msaa::Sample8,
            ))
            .id();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .anti_aliasing = AntiAliasingMode::Fxaa;
        app.update();
        assert_eq!(app.world().get::<Msaa>(window_camera), Some(&Msaa::Off));
        assert!(app.world().get::<Fxaa>(window_camera).is_some());
        assert_eq!(
            app.world().get::<Msaa>(texture_camera),
            Some(&Msaa::Sample8)
        );
        assert!(app.world().get::<Fxaa>(texture_camera).is_none());
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .anti_aliasing = AntiAliasingMode::Msaa2;
        app.update();
        assert_eq!(app.world().get::<Msaa>(window_camera), Some(&Msaa::Sample2));
        assert!(app.world().get::<Fxaa>(window_camera).is_none());
    }

    #[test]
    fn fxaa_mode_enables_an_existing_disabled_component() {
        let mut app = test_app();
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::Window(WindowRef::Primary),
                Msaa::Off,
                Fxaa {
                    enabled: false,
                    ..default()
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .anti_aliasing = AntiAliasingMode::Fxaa;
        app.update();
        assert!(app.world().get::<Fxaa>(camera).unwrap().enabled);
    }

    #[test]
    fn taa_is_exclusive_and_restores_only_preexisting_support_components() {
        let mut app = test_app(); // 使用正式抗锯齿应用系统和默认设置资源。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(), // 建立窗口 3D 相机身份及其 required 组件。
                RenderTarget::Window(WindowRef::Primary), // 全局画质应接管主窗口目标。
                Msaa::Sample4,       // 模拟当前高档 MSAA 基线。
                MipBias(0.25),       // 模拟作者预先声明的纹理采样偏移。
                DepthPrepass,        // 模拟作者因其他后处理预先启用深度预通道。
            ))
            .id(); // 保存实体用于逐阶段断言。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .anti_aliasing = AntiAliasingMode::Taa; // 切换到实验性 TAA。
        app.update(); // 应用互斥状态并让 Bevy required 组件落地。
        assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off)); // TAA 必须关闭多重采样。
        assert!(app.world().get::<Fxaa>(camera).is_none()); // TAA 不得与 FXAA 共存。
        assert!(app.world().get::<TemporalAntiAliasing>(camera).is_some()); // 窗口相机必须获得 TAA。
        assert!(app.world().get::<TemporalJitter>(camera).is_some()); // Bevy required 投影抖动必须存在。
        assert_eq!(
            app.world().get::<MipBias>(camera).map(|bias| bias.0),
            Some(0.25)
        ); // 作者 mip 偏移不得被默认值覆盖。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 作者深度预通道必须保留。
        assert!(app.world().get::<MotionVectorPrepass>(camera).is_some()); // TAA 自动补齐运动矢量预通道。
        assert!(app.world().get::<TaaSupportOwnership>(camera).is_some()); // 首次接管必须保存支撑组件所有权。

        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .anti_aliasing = AntiAliasingMode::Msaa2; // 从 TAA 切回正式中档采样。
        app.update(); // 恢复作者支撑组件并移除本功能临时状态。
        assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample2)); // MSAA 采样数必须即时恢复。
        assert!(app.world().get::<TemporalAntiAliasing>(camera).is_none()); // 跨帧累积组件必须移除。
        assert!(app.world().get::<TemporalJitter>(camera).is_none()); // 原本不存在的抖动不得残留。
        assert_eq!(
            app.world().get::<MipBias>(camera).map(|bias| bias.0),
            Some(0.25)
        ); // 作者 mip 偏移必须精确恢复。
        assert!(app.world().get::<DepthPrepass>(camera).is_some()); // 作者深度预通道不能被误删。
        assert!(app.world().get::<MotionVectorPrepass>(camera).is_none()); // 仅由 TAA 引入的运动矢量预通道必须移除。
        assert!(app.world().get::<TaaSupportOwnership>(camera).is_none()); // 结束接管后清理私有所有权。
    }

    #[test]
    fn taa_never_takes_over_texture_target_cameras() {
        let mut app = test_app(); // 使用正式窗口目标过滤逻辑。
        let texture_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                     // 建立专用纹理相机身份。
                RenderTarget::None { size: UVec2::ONE }, // 模拟角色预览或手动纹理目标。
                Msaa::Off,                               // 作者为该纹理目标关闭 MSAA。
                TemporalAntiAliasing::default(),         // 模拟作者自己的专用 TAA 配置。
            ))
            .id(); // 保存实体用于隔离断言。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .anti_aliasing = AntiAliasingMode::Msaa8; // 全局窗口画质切到极高 MSAA。
        app.update(); // 运行正式抗锯齿应用系统。
        assert_eq!(app.world().get::<Msaa>(texture_camera), Some(&Msaa::Off)); // 纹理相机采样数保持作者值。
        assert!(
            app.world()
                .get::<TemporalAntiAliasing>(texture_camera)
                .is_some()
        ); // 作者 TAA 不得被全局设置移除。
        assert!(
            app.world()
                .get::<TaaSupportOwnership>(texture_camera)
                .is_none()
        ); // 隔离相机不得附加全局所有权组件。
    }

    #[test]
    fn stable_taa_frames_do_not_rewrite_pipeline_components() {
        let mut app = test_app(); // 使用正式抗锯齿系统验证稳定帧变更检测。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .anti_aliasing = AntiAliasingMode::Taa; // 在生成相机前选择 TAA，验证新相机继承。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立新的窗口 3D 相机。
                RenderTarget::Window(WindowRef::Primary), // 允许全局画质接管。
                Msaa::Sample4,                            // 模拟作者默认采样状态。
            ))
            .id(); // 保存实体用于变更检测断言。
        app.update(); // 首帧完成 TAA 与 required 组件安装。
        app.world_mut().clear_trackers(); // 清掉首帧合法变更，只观察稳定帧。
        app.update(); // 同一 TAA 设置再次运行不应重写组件。
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<Msaa>()
                .unwrap()
                .is_changed()
        ); // 稳定帧不得重写采样数。
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<TemporalAntiAliasing>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复写入 TAA 组件",
        );
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<TaaSupportOwnership>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复保存 TAA 支撑组件所有权",
        );
    }

    #[test]
    fn taa_history_resets_after_teleport_and_projection_change() {
        let mut app = App::new(); // 历史判定是纯 ECS 系统，不需要完整渲染插件。
        app.add_systems(Update, reset_taa_history_on_camera_discontinuity); // 注册正式历史重置入口。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 建立带默认透视投影的窗口 3D 相机。
                RenderTarget::Window(WindowRef::Primary), // 只允许窗口相机进入全局历史管理。
                GlobalTransform::IDENTITY,                // 初始世界姿态位于原点且无旋转。
                TemporalAntiAliasing { reset: false },    // 模拟已经积累了可复用历史的稳定 TAA。
                TaaCameraHistoryPose {
                    translation: Vec3::ZERO,  // 上一帧位置与初始相机一致。
                    rotation: Quat::IDENTITY, // 上一帧旋转与初始相机一致。
                }, // 提供可比较的历史姿态基线。
            ))
            .id(); // 保存实体用于制造传送和投影变化。
        app.update(); // 初始连续帧不得误报重置。
        assert!(
            !app.world()
                .get::<TemporalAntiAliasing>(camera)
                .unwrap()
                .reset
        ); // 连续姿态保持历史有效。

        app.world_mut()
            .entity_mut(camera)
            .insert(GlobalTransform::from_translation(Vec3::new(20.0, 0.0, 0.0))); // 单帧移动二十米明确超过传送阈值。
        app.update(); // 运行同实体传送判定。
        assert!(
            app.world()
                .get::<TemporalAntiAliasing>(camera)
                .unwrap()
                .reset
        ); // 传送后必须请求清空旧画面。

        app.world_mut()
            .get_mut::<TemporalAntiAliasing>(camera)
            .unwrap()
            .reset = false; // 模拟 Bevy 提取阶段消费并清除上一重置请求。
        let mut projection = app.world_mut().get_mut::<Projection>(camera).unwrap(); // 取得玩法相机透视投影。
        if let Projection::Perspective(perspective) = projection.bypass_change_detection() {
            perspective.fov *= 0.8; // 模拟瞄准缩放导致本帧投影矩阵发生明显变化。
        }
        projection.set_changed(); // 精确通知历史系统投影已经改变。
        app.update(); // 运行投影不连续判定。
        assert!(
            app.world()
                .get::<TemporalAntiAliasing>(camera)
                .unwrap()
                .reset
        ); // FOV 变化不得复用旧历史。
    }

    #[test]
    fn shadow_filtering_switches_window_camera_and_leaves_texture_camera_authored() {
        let mut app = test_app();
        let window_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::Window(WindowRef::Primary),
                ShadowFilteringMethod::Gaussian,
            ))
            .id();
        let texture_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::None { size: UVec2::ONE },
                ShadowFilteringMethod::Temporal,
            ))
            .id();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .shadow_filtering = ShadowFilteringQuality::Fast;
        app.update();
        assert_eq!(
            app.world().get::<ShadowFilteringMethod>(window_camera),
            Some(&ShadowFilteringMethod::Hardware2x2),
        );
        assert_eq!(
            app.world().get::<ShadowFilteringMethod>(texture_camera),
            Some(&ShadowFilteringMethod::Temporal),
        );
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .shadow_filtering = ShadowFilteringQuality::Soft;
        app.update();
        assert!(
            app.world()
                .get::<ShadowFilteringMethod>(window_camera)
                .is_none(),
            "柔和档应回到隐式 Gaussian",
        );
    }

    #[test]
    fn bloom_low_high_and_off_restore_authored_hdr_and_tonemapping() {
        let mut app = test_app();
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::Window(WindowRef::Primary),
                Tonemapping::Reinhard,
            ))
            .id();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .bloom_quality = BloomQuality::Low;
        app.update();
        let low = app.world().get::<Bloom>(camera).unwrap();
        assert_eq!(low.intensity, 0.08);
        assert_eq!(low.max_mip_dimension, 256);
        assert!(app.world().get::<Hdr>(camera).is_some());
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::TonyMcMapface),
        );
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .bloom_quality = BloomQuality::High;
        app.update();
        let high = app.world().get::<Bloom>(camera).unwrap();
        assert_eq!(high.intensity, 0.15);
        assert_eq!(high.max_mip_dimension, 512);
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .bloom_quality = BloomQuality::Off;
        app.update();
        assert!(app.world().get::<Bloom>(camera).is_none());
        assert!(app.world().get::<Hdr>(camera).is_none());
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::Reinhard),
        );
    }

    #[test]
    fn bloom_off_preserves_authored_hdr_and_non_default_tonemapping() {
        let mut app = test_app();
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::Window(WindowRef::Primary),
                Hdr,
                Tonemapping::AcesFitted,
            ))
            .id();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .bloom_quality = BloomQuality::High;
        app.update();
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::TonyMcMapface),
        );
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .bloom_quality = BloomQuality::Off;
        app.update();
        assert!(app.world().get::<Hdr>(camera).is_some());
        assert_eq!(
            app.world().get::<Tonemapping>(camera),
            Some(&Tonemapping::AcesFitted),
        );
    }

    #[test]
    fn new_window_cameras_inherit_bloom_while_texture_cameras_remain_untouched() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .bloom_quality = BloomQuality::High;
        let window_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::Window(WindowRef::Primary),
            ))
            .id();
        let texture_camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::None { size: UVec2::ONE },
                Hdr,
                Tonemapping::AcesFitted,
                Bloom::NATURAL,
            ))
            .id();
        app.update();
        assert!(app.world().get::<Bloom>(window_camera).is_some());
        assert_eq!(
            app.world().get::<Tonemapping>(window_camera),
            Some(&Tonemapping::TonyMcMapface),
        );
        assert_eq!(
            app.world().get::<Tonemapping>(texture_camera),
            Some(&Tonemapping::AcesFitted),
        );
        assert_eq!(
            app.world().get::<Bloom>(texture_camera).unwrap().intensity,
            Bloom::NATURAL.intensity,
        );
        assert!(
            app.world()
                .get::<AuthoredWindowPostProcessing>(texture_camera)
                .is_none(),
        );
    }

    #[test]
    fn stable_bloom_frame_does_not_mark_component_changed_again() {
        let mut app = test_app();
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .bloom_quality = BloomQuality::Low;
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                RenderTarget::Window(WindowRef::Primary),
            ))
            .id();
        app.update();
        app.world_mut().clear_trackers();
        app.update();
        assert!(
            !app.world()
                .entity(camera)
                .get_ref::<Bloom>()
                .unwrap()
                .is_changed(),
            "稳定帧不得重复标记 Bloom 组件变更",
        );
    }
}
