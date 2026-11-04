//! DLSS帧生成资格与租约回归；无需GPU或SDK初始化。
use super::*;

fn independent_app() -> (App, Entity) {
    let mut app = App::new(); // 只运行资格和身份系统，禁止接触原生SDK。
    let mut settings = RenderSettings::default(); // 两种超分默认关闭。
    settings
        .graphics
        .set_frame_generation_mode(FrameGenerationMode::Dlss2x); // 正式setter协调VSync。
    app.insert_resource(settings)
        .insert_resource(AttachmentState {
            attached: true,
            result: 0,
        }) // 用已支持设备事实隔离纯ECS行为。
        .init_resource::<DlssRuntimeStatus>() // 默认未生效，不能成为启用前提。
        .init_resource::<Fsr3RuntimeStatus>() // 默认关闭。
        .init_resource::<LeaseState>() // 独立所有权。
        .init_resource::<EligibilityState>() // 本帧阻断。
        .init_resource::<WindowStabilityState>() // 交换链稳定边界。
        .init_resource::<RenderRequest>() // 提取请求。
        .init_resource::<GraphicsRuntimeLeaseRegistry>() // 使用生产租约注册表。
        .add_systems(
            Update,
            (maintain_request_and_lease, publish_input_identity).chain(),
        ); // 与正式依赖顺序一致。
    app.world_mut().spawn((
        PrimaryWindow,
        Window {
            present_mode: PresentMode::AutoNoVsync,
            ..default()
        },
    )); // 实际窗口同样无VSync。
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), PrimarySceneView))
        .id(); // 唯一玩法相机。
    (app, camera) // 调用方可检查精确目标身份。
}

#[test]
fn native_frame_generation_acquires_inputs_and_releases_without_changing_preferences() {
    let (mut app, camera) = independent_app(); // 未加载任何DLSS/FSR上下文。
    app.update(); // 首次窗口出现必须先稳定。
    assert!(!app.world().resource::<RenderRequest>().enabled); // 不绕过交换链边界。
    app.update(); // 第二帧可以独立取得输入。
    assert!(app.world().resource::<RenderRequest>().enabled); // 关闭超分仍可请求实际输入准备。
    let registry = app.world().resource::<GraphicsRuntimeLeaseRegistry>(); // 不模拟附件要求。
    assert_eq!(
        registry.requirements_for_camera(camera),
        Some(GraphicsRuntimeRequirements::frame_generation(camera))
    ); // 使用公共工厂的完整约束。
    assert_eq!(registry.active_lease_count(), 1); // 单一租约。
    app.update(); // 稳定帧不得再次申请。
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        1
    );
    assert!(
        !app.world()
            .resource::<RenderSettings>()
            .graphics
            .dlss_mode
            .is_enabled()
    ); // 开启FG没有偷偷开启超分。
    assert!(
        !app.world()
            .resource::<RenderSettings>()
            .graphics
            .fsr_mode
            .is_enabled()
    ); // 原生请求保留。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_frame_generation_mode(FrameGenerationMode::Off); // 正式关闭。
    app.update(); // 释放而非留给其他后端兜底。
    assert!(!app.world().resource::<RenderRequest>().enabled);
    assert_eq!(
        app.world()
            .resource::<GraphicsRuntimeLeaseRegistry>()
            .active_lease_count(),
        0
    );
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_frame_generation_mode(FrameGenerationMode::Dlss2x); // 同一已加载后端重新开启。
    app.update();
    assert!(app.world().resource::<RenderRequest>().enabled); // 不要求重启或重新启用超分。
}

#[test]
fn same_size_dlaa_native_and_fsr_inputs_have_distinct_stable_identities() {
    use feathrise_settings::{DlssMode, FsrMode}; // 质量身份无需改变窗口尺寸。
    let (mut app, _) = independent_app();
    app.update();
    let native = app.world().resource::<RenderRequest>().input_identity;
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_dlss_mode(DlssMode::Dlaa); // DLAA内部仍为原生尺寸。
    app.update();
    let dlaa = app.world().resource::<RenderRequest>().input_identity;
    assert_ne!(native, dlaa); // 进入相同规格的reset事务。
    app.world_mut()
        .resource_mut::<RenderSettings>()
        .graphics
        .set_fsr_mode(FsrMode::Quality); // 跨厂商选择保留FG。
    app.update();
    let fsr = app.world().resource::<RenderRequest>().input_identity;
    assert_ne!(fsr, dlaa);
    app.update();
    assert_eq!(app.world().resource::<RenderRequest>().input_identity, fsr); // 无逐帧抖动或计数，稳定帧不重置。
    assert!(app.world().resource::<RenderRequest>().enabled); // 资格与重建器来源解耦。
}

#[test]
fn window_signature_requires_one_unchanged_frame_after_each_transition() {
    let mut stability = WindowStabilityState::default();
    let mut window = Window::default();
    assert!(!stability.observe(&window)); // 首次出现先让Bevy建立交换链。
    assert!(stability.observe(&window)); // 连续一帧未变化后才允许FG。

    window.present_mode = PresentMode::AutoNoVsync;
    assert!(!stability.observe(&window)); // 呈现模式切换重新阻断。
    assert!(stability.observe(&window)); // 新交换链稳定后恢复资格。
}

#[test]
fn only_non_vsync_present_modes_are_eligible() {
    assert!(present_mode_supports_frame_generation(
        PresentMode::AutoNoVsync
    ));
    assert!(present_mode_supports_frame_generation(
        PresentMode::Immediate
    ));
    assert!(present_mode_supports_frame_generation(PresentMode::Mailbox));
    assert!(!present_mode_supports_frame_generation(
        PresentMode::AutoVsync
    ));
    assert!(!present_mode_supports_frame_generation(PresentMode::Fifo));
    assert!(!present_mode_supports_frame_generation(
        PresentMode::FifoRelaxed
    ));
}
