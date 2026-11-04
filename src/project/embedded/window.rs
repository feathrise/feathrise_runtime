//! 同一Winit窗口的作者租约：编辑嵌入，预览归还原生顶层；World、窗口实体与Surface均不重建。

use super::{
    EmbeddedProjectState,
    resources::{GameWindowLease, NativeGeometry, NativeHandoff, NativePlacement, NativeStage, Runtime, Viewport},
    transport::CAPABILITIES,
};
use bevy::{
    ecs::system::NonSendMarker,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow, RawHandleWrapper, WindowCloseRequested, WindowMode,
        WindowResized, WindowScaleFactorChanged},
};
use feathrise_editor_protocol::{Message, Mode};
use std::time::{Duration, Instant};

const NATIVE_HANDOFF_TIMEOUT: Duration = Duration::from_secs(10);

impl GameWindowLease {
    fn new(window: &Window, cursor: &CursorOptions) -> Self {
        Self {
            mode: window.mode.clone(),
            applied_mode: WindowMode::Windowed,
            mode_pending: false,
            resolution: window.resolution.clone(),
            position: window.position.clone(),
            decorations: window.decorations,
            cursor: cursor.clone(),
            native_style: None,
            editor_viewport: None,
            placement: None,
            restoring_placement: false,
            native_geometry: None,
            geometry_request: None,
        }
    }

    fn capture_geometry_request(&mut self, window: &Window) {
        let Some(published) = &self.native_geometry else { return; };
        let size = window.physical_size();
        let size_changed = size != published.resolution.physical_size();
        let position_changed = window.position != published.position;
        if !size_changed && !position_changed { return; }
        let request = self.geometry_request.get_or_insert_with(Default::default);
        if size_changed { request.size = Some(size); }
        if position_changed { request.position = Some(window.position.clone()); }
        request.issued = false;
    }

    fn remember_native_geometry(&mut self, window: &Window, minimized: bool) {
        // PostUpdate读回不能丢弃尚未采用的工程请求；MIN的0也不能冒充一次resize的成功回执。
        if !minimized && let Some(request) = &mut self.geometry_request {
            if request.size == Some(window.physical_size()) { request.size = None; }
            if request.position.as_ref() == Some(&window.position) { request.position = None; }
            if request.size.is_none() && request.position.is_none() { self.geometry_request = None; }
        }
        self.native_geometry = Some(NativeGeometry {
            resolution: window.resolution.clone(), position: window.position.clone(),
        });
    }

    fn capture_game(&mut self, window: &Window, cursor: &CursorOptions, capture_cursor: bool) {
        let was_windowed = matches!(&self.mode, WindowMode::Windowed);
        // 后台暂缓的全屏请求继续保留；SDK写回实际模式不能反过来覆盖玩家的请求。
        if !self.mode_pending || window.mode != self.applied_mode {
            self.mode = window.mode.clone();
            self.mode_pending = false;
        }
        self.decorations = window.decorations;
        let normal = self.placement.as_ref().is_none_or(|placement| !placement.minimized && !placement.maximized);
        if was_windowed && matches!(&window.mode, WindowMode::Windowed) && normal {
            self.resolution = window.resolution.clone();
            self.position = window.position.clone();
        } else {
            self.resolution.set_scale_factor_override(window.resolution.scale_factor_override());
        }
        // 失焦后的None是安全释放事实，不是工程希望下次预览采用的新鼠标策略。
        if capture_cursor && window.focused { self.cursor = cursor.clone(); }
    }

    fn capture_editor_request(&mut self, window: &Window) {
        // PostStartup/工程设置仍可表达全屏请求；编辑租约只把实际窗口临时压成Windowed。
        if !matches!(&window.mode, WindowMode::Windowed) { self.mode = window.mode.clone(); }
        if window.decorations { self.decorations = true; }
    }
}

pub(super) fn begin_handoff(world: &mut World, runtime: &mut Runtime, request_id: u64, target: Mode) -> Result<(), String> {
    if runtime.native_handoff.is_some() || runtime.native_result.is_some() {
        return Err("前一原生窗口交接尚未结清".into());
    }
    let (entity, window, cursor) = world.query_filtered::<(Entity, &Window, &CursorOptions), With<PrimaryWindow>>()
        .single(world).map_err(|_| "原生交接缺少唯一工程主窗口")?;
    let lease = runtime.window_lease.as_mut().ok_or("工程窗口租约尚未就绪")?;
    if runtime.native_mode == Mode::Playing {
        #[cfg(windows)]
        if let Some(hwnd) = runtime.hwnd {
            bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
                let native = windows.get_window(entity).ok_or("窗口交接缺少本工程Winit窗口")?;
                capture_placement(native, hwnd as windows_sys::Win32::Foundation::HWND, lease)
            })?;
        }
        lease.capture_game(window, cursor, !runtime.input_suspended);
    }
    #[cfg(not(windows))]
    let _ = entity;
    runtime.native_handoff = Some(NativeHandoff {
        request_id, previous: runtime.native_mode, target,
        stage: NativeStage::Prepare, stage_frame: runtime.frames, started: Instant::now(),
        failure: None, restore_focus: false, source_visible: window.visible,
    });
    // 交接帧不允许作者旧按下或玩法旧按下穿过所有权边界。
    suspend_input(world, runtime);
    Ok(())
}

/// 原生X/Alt+F4与HUD返回共用可靠模式事务；只有宿主Stop才结束工程App。
pub(super) fn close_requested(
    mut closed: MessageReader<WindowCloseRequested>,
    primary: Query<Entity, With<PrimaryWindow>>,
    windows: Query<&Window>,
    mut commands: Commands,
    mut runtime: NonSendMut<Runtime>,
    mut state: ResMut<EmbeddedProjectState>,
    mut cursors: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if runtime.stopping { return; }
    let primary = primary.single().ok();
    for event in closed.read() {
        if Some(event.window) == primary {
            if runtime.native_mode == Mode::Playing
                || runtime.native_handoff.as_ref().is_some_and(|handoff| handoff.target == Mode::Playing)
            {
                runtime.window_return_requested = true;
                runtime.input_suspended = true;
                state.transitioning = true;
                observe_cursor_owner(&mut runtime, false);
                for mut cursor in &mut cursors { release_cursor(&mut cursor); }
                // 本reader只推进自身游标；InputSystems之后的生产清理会丢弃同批次键鼠，
                // WindowCloseRequested仍完整保留给生命周期。此前已被玩法消费的Alt不回滚。
            }
        } else if windows.contains(event.window) {
            // 默认关闭只对工程主窗口改为返回；项目自行创建的辅助窗口仍保持原关闭行为。
            commands.entity(event.window).despawn();
        }
    }
}

/// 在相机、UI和提取前发布本帧真实客户区与DPI；不能等Last再改渲染尺寸。
pub(super) fn synchronize(
    _thread: NonSendMarker,
    mut runtime: NonSendMut<Runtime>,
    state: Res<EmbeddedProjectState>,
    monitors: Option<Res<bevy::winit::WinitMonitors>>,
    mut windows: Query<(Entity, &mut Window, &mut CursorOptions, Option<&RawHandleWrapper>), With<PrimaryWindow>>,
    mut resized: MessageWriter<WindowResized>,
    mut rescaled: MessageWriter<WindowScaleFactorChanged>,
    mut exit: MessageWriter<AppExit>,
) {
    if runtime.failed { return; }
    let Ok((entity, mut window, mut cursor, Some(raw))) = windows.single_mut() else { return; };
    if runtime.stopping { release_cursor(&mut cursor); return; }
    let previous = (window.physical_size(), window.scale_factor());
    match update_native(&mut runtime, entity, state.world_id, &mut window, &cursor, raw, monitors.as_deref()) {
        Ok(()) => {
            publish_size_events(entity, previous, &window, &mut resized, &mut rescaled);
            runtime.input_suspended = state.mode != Mode::Playing || state.transitioning || runtime.native_mode != Mode::Playing
                || runtime.native_handoff.is_some() || !window.visible || !window.focused;
            #[cfg(windows)]
            {
                let owns = !runtime.input_suspended && !runtime.native_child
                    && runtime.hwnd.is_some_and(|hwnd| native_cursor_owner(hwnd as windows_sys::Win32::Foundation::HWND));
                observe_cursor_owner(&mut runtime, owns);
            }
            if runtime.input_suspended { release_cursor(&mut cursor); }
        }
        Err(error) => fail(&mut runtime, error, &mut exit),
    }
}

/// 工程Update表达的新请求在本帧相机计算前采用；编辑态只保存逻辑请求，不改玩家设置。
pub(super) fn capture_requests(
    _thread: NonSendMarker,
    mut runtime: NonSendMut<Runtime>,
    state: Res<EmbeddedProjectState>,
    monitors: Option<Res<bevy::winit::WinitMonitors>>,
    mut windows: Query<(Entity, &mut Window, &mut CursorOptions), With<PrimaryWindow>>,
    mut resized: MessageWriter<WindowResized>,
    mut rescaled: MessageWriter<WindowScaleFactorChanged>,
    mut exit: MessageWriter<AppExit>,
) {
    if runtime.failed { return; }
    let Ok((entity, mut window, mut cursor)) = windows.single_mut() else { return; };
    if runtime.stopping { release_cursor(&mut cursor); return; }
    let previous = (window.physical_size(), window.scale_factor());
    let editing = runtime.native_child;
    let playing = runtime.native_mode == Mode::Playing;
    let handoff = runtime.native_handoff.is_some();
    let capture_cursor = !runtime.input_suspended;
    let hwnd = runtime.hwnd;
    let Some(lease) = runtime.window_lease.as_mut() else { return; };
    if editing {
        lease.capture_editor_request(&window);
        publish_editor(&mut window, lease);
    } else if !handoff && playing && !state.transitioning {
        #[cfg(windows)]
        if let Some(monitors) = monitors.as_deref() {
            let result = bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
                let native = windows.get_window(entity).ok_or("真实Winit工程窗口已消失")?;
                if let Some(hwnd) = hwnd { capture_placement(native, hwnd as windows_sys::Win32::Foundation::HWND, lease)?; }
                lease.capture_geometry_request(&window);
                lease.capture_game(&window, &cursor, capture_cursor);
                apply_game_request(native, &mut window, lease, monitors, native.has_focus())?;
                publish_native(native, &mut window, lease);
                Ok::<_, String>(())
            });
            if let Err(error) = result { fail(&mut runtime, error, &mut exit); }
        }
    }
    if runtime.input_suspended || handoff { release_cursor(&mut cursor); }
    #[cfg(windows)]
    if !editing && !handoff && playing && !runtime.failed {
        let allowed = state.mode == Mode::Playing && !state.transitioning;
        let result = bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
            let native = windows.get_window(entity).ok_or("鼠标交接缺少本工程Winit窗口")?;
            prepare_cursor_anchor(&mut runtime, native, &mut cursor, allowed)
        });
        if let Err(error) = result { fail(&mut runtime, error, &mut exit); }
    } else {
        observe_cursor_owner(&mut runtime, false);
    }
    publish_size_events(entity, previous, &window, &mut resized, &mut rescaled);
    #[cfg(not(windows))]
    let _ = (entity, state, monitors, exit, hwnd);
}

fn release_cursor(cursor: &mut CursorOptions) {
    cursor.grab_mode = CursorGrabMode::None;
    cursor.visible = true;
}

/// 返回意图一经接受即暂停新输入；模式事实只在原生采用完成时改变。
pub(super) fn suspend_input(world: &mut World, runtime: &mut Runtime) {
    runtime.input_suspended = true;
    observe_cursor_owner(runtime, false);
    if let Some(mut state) = world.get_resource_mut::<EmbeddedProjectState>() { state.transitioning = true; }
    if let Some(mut editor) = world.get_resource_mut::<feathrise_scene_core::input::EditorInputState>() {
        editor.map_editor_open = true;
    }
    super::systems::reset_mode_input(world, Mode::Editing);
}

fn observe_cursor_owner(runtime: &mut Runtime, owns: bool) {
    if !owns {
        runtime.cursor_anchor_pending = false;
        runtime.cursor_last_grab = None;
    } else if !runtime.cursor_focus_owned {
        // 只在输入真正归还本游戏的边沿排一次交接；稳定帧不重置玩家抓取。
        runtime.cursor_anchor_pending = true;
    }
    runtime.cursor_focus_owned = owns;
}

fn observe_grab_request(runtime: &mut Runtime, grab: CursorGrabMode) {
    if grab == CursorGrabMode::None {
        runtime.cursor_anchor_pending = false;
    } else if runtime.cursor_last_grab != Some(grab) {
        // Alt/UI可在同一真实焦点下释放并重新抓取；Winit会把新Locked锚在当前屏幕点。
        // 新请求边沿也须检查client，不能只等焦点变化；稳定请求绝不重复重定位。
        runtime.cursor_anchor_pending = true;
    }
    runtime.cursor_last_grab = Some(grab);
}

#[cfg(any(windows, test))]
fn cursor_anchor_needed(pending: bool, owns: bool, grab: CursorGrabMode,
    point: [i32; 2], client: [i32; 4]) -> bool
{
    let [left, top, right, bottom] = client;
    pending && owns && grab != CursorGrabMode::None && left < right && top < bottom
        && !(point[0] >= left && point[0] < right && point[1] >= top && point[1] < bottom)
}

#[cfg(windows)]
fn native_cursor_owner(hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    use windows_sys::Win32::UI::{Input::KeyboardAndMouse::{GetActiveWindow, GetFocus}, WindowsAndMessaging::*};
    unsafe {
        IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 && GetForegroundWindow() == hwnd
            && GetActiveWindow() == hwnd && GetFocus() == hwnd
    }
}

#[cfg(windows)]
fn prepare_cursor_anchor(runtime: &mut Runtime, native: &winit::window::Window,
    cursor: &mut CursorOptions, allowed: bool) -> Result<(), String>
{
    use windows_sys::Win32::{Foundation::{POINT, RECT}, Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::{GetClientRect, GetCursorPos}};
    let hwnd = runtime.hwnd.ok_or("鼠标交接缺少已核验工程窗口")? as windows_sys::Win32::Foundation::HWND;
    let owns = allowed && native_cursor_owner(hwnd);
    observe_cursor_owner(runtime, owns);
    if !owns {
        runtime.input_suspended = true;
        release_cursor(cursor);
        return Ok(());
    }
    observe_grab_request(runtime, cursor.grab_mode);
    if !runtime.cursor_anchor_pending || cursor.grab_mode == CursorGrabMode::None { return Ok(()); }
    let mut rect = RECT::default();
    let mut point = POINT::default();
    let mut origin = POINT::default();
    unsafe {
        if GetClientRect(hwnd, &mut rect) == 0 || ClientToScreen(hwnd, &mut origin) == 0
            || GetCursorPos(&mut point) == 0
        { return Err("读取鼠标交接的真实客户区失败".into()); }
    }
    let client = [origin.x + rect.left, origin.y + rect.top, origin.x + rect.right, origin.y + rect.bottom];
    if cursor_anchor_needed(true, owns, cursor.grab_mode, [point.x, point.y], client) {
        // Winit Locked采用当前屏幕位置的1×1锚点；旧作者按钮可能在新client外。
        // 仅所有权/新抓取请求边沿且工程确实请求抓取时，先释放本窗、移入本client，再恢复原请求。
        native.set_cursor_grab(winit::window::CursorGrabMode::None).map_err(|error| error.to_string())?;
        if !native_cursor_owner(hwnd) {
            observe_cursor_owner(runtime, false);
            runtime.input_suspended = true;
            release_cursor(cursor);
            return Ok(());
        }
        let moved = native.set_cursor_position(winit::dpi::PhysicalPosition::new(
            (rect.right - rect.left) / 2, (rect.bottom - rect.top) / 2));
        // Alt+Tab竞态只释放、不在后台恢复抓取或移动第二次；下次真实获焦会重新排交接。
        if !native_cursor_owner(hwnd) {
            observe_cursor_owner(runtime, false);
            runtime.input_suspended = true;
            release_cursor(cursor);
            return Ok(());
        }
        let restored = match cursor.grab_mode {
            CursorGrabMode::Locked => native.set_cursor_grab(winit::window::CursorGrabMode::Locked)
                .or_else(|_| native.set_cursor_grab(winit::window::CursorGrabMode::Confined)),
            CursorGrabMode::Confined => native.set_cursor_grab(winit::window::CursorGrabMode::Confined)
                .or_else(|_| native.set_cursor_grab(winit::window::CursorGrabMode::None)),
            CursorGrabMode::None => unreachable!("未请求抓取时不得进入鼠标锚点交接"),
        };
        moved.map_err(|error| error.to_string())?;
        restored.map_err(|error| error.to_string())?;
        runtime.cursor_reanchors = runtime.cursor_reanchors.saturating_add(1);
    }
    // 已在客户区内也消费这次边沿；后续稳定帧不重定位，新的真实抓取请求再单独排队。
    runtime.cursor_anchor_pending = false;
    Ok(())
}

fn publish_size_events(entity: Entity, previous: (UVec2, f32), window: &Window,
    resized: &mut MessageWriter<WindowResized>, rescaled: &mut MessageWriter<WindowScaleFactorChanged>)
{
    // CameraUpdateSystems按窗口事件失效；直接改Window而不发事件会沿用旧裁剪与附件尺寸。
    if previous.0 != window.physical_size() {
        resized.write(WindowResized { window: entity, width: window.width(), height: window.height() });
    }
    if previous.1 != window.scale_factor() {
        rescaled.write(WindowScaleFactorChanged { window: entity, scale_factor: window.scale_factor() as f64 });
    }
}

/// 只在模式实际完成的边沿归还工程鼠标策略；稳定Playing帧仍由工程控制器拥有。
pub(super) fn restore_mode_cursor(world: &mut World, runtime: &mut Runtime, mode: Mode) {
    let focused = world.query_filtered::<&Window, With<PrimaryWindow>>().single(world)
        .is_ok_and(|window| window.visible && window.focused);
    runtime.input_suspended = mode != Mode::Playing || !focused;
    if runtime.input_suspended { return; }
    let Some(lease) = &runtime.window_lease else { return; };
    let desired = lease.cursor.clone();
    for mut cursor in world.query_filtered::<&mut CursorOptions, With<PrimaryWindow>>().iter_mut(world) {
        cursor.grab_mode = desired.grab_mode;
        cursor.visible = desired.visible;
    }
}

fn fail(runtime: &mut Runtime, error: String, exit: &mut MessageWriter<AppExit>) {
    eprintln!("工程窗口承载失败：{error}");
    let request_id = runtime.native_handoff.as_ref().map_or(0, |handoff| handoff.request_id);
    let _ = runtime.transport.send(Message::Failure { request_id, code: "native-window".into(), message: error });
    runtime.failed = true;
    exit.write(AppExit::error());
}

fn publish_editor(window: &mut Window, lease: &GameWindowLease) {
    window.mode = WindowMode::Windowed;
    window.decorations = false;
    if let Some(viewport) = &lease.editor_viewport {
        window.position = bevy::window::WindowPosition::At(IVec2::new(viewport.x, viewport.y));
        window.resolution.set_physical_resolution(viewport.width.max(1), viewport.height.max(1));
    }
    window.focused = false;
}

#[cfg(windows)]
fn update_native(
    runtime: &mut Runtime, entity: Entity, world_id: u64,
    window: &mut Window, cursor: &CursorOptions, raw: &RawHandleWrapper,
    monitors: Option<&bevy::winit::WinitMonitors>,
) -> Result<(), String> {
    use raw_window_handle::RawWindowHandle;
    use windows_sys::Win32::UI::{HiDpi::*, WindowsAndMessaging::*};
    let RawWindowHandle::Win32(raw) = raw.get_window_handle() else { return Err("工程视口不是Win32窗口".into()); };
    let hwnd = raw.hwnd.get() as usize;
    let child = hwnd as windows_sys::Win32::Foundation::HWND;
    let parent = runtime.config.parent as windows_sys::Win32::Foundation::HWND;
    let mut parent_pid = 0;
    // HWND、PID与DPI模式每帧固定开销核验；原生预览也不能逃脱所属工程会话。
    unsafe {
        if IsWindow(parent) == 0 { return Err("作者父窗口已经退出".into()); }
        GetWindowThreadProcessId(parent, &mut parent_pid);
        if parent_pid != runtime.config.parent_pid || parent_pid == std::process::id() { return Err("作者父窗口进程身份不一致".into()); }
        if runtime.hwnd.is_some_and(|previous| previous != hwnd) { return Err("工程窗口句柄在会话内被替换".into()); }
        if AreDpiAwarenessContextsEqual(GetWindowDpiAwarenessContext(parent), GetWindowDpiAwarenessContext(child)) == 0 {
            return Err("工程与作者窗口DPI模式不同".into());
        }
    }
    runtime.hwnd = Some(hwnd);
    let monitors = monitors.ok_or("真实Winit显示器资源尚未就绪")?;
    bevy::winit::WINIT_WINDOWS.with_borrow(|windows| {
        let native = windows.get_window(entity).ok_or("真实Winit工程窗口已消失")?;
        if runtime.window_lease.is_none() { runtime.window_lease = Some(GameWindowLease::new(window, cursor)); }
        if !runtime.ready && !runtime.native_child && runtime.native_handoff.is_none() {
            let lease = runtime.window_lease.as_mut().unwrap();
            // 工程可能在PostStartup请求全屏：先保留请求并真正退出，再嵌入，不能用父局部坐标恢复桌面placement。
            let was_fullscreen = native.fullscreen().is_some();
            native.set_visible(false);
            native.set_fullscreen(None);
            window.mode = WindowMode::Windowed;
            if was_fullscreen { return Ok(()); }
            if lease.placement.is_none() { capture_placement(native, child, lease)?; }
            if !normal_window(native, child) {
                normalize_window(native, child, lease.placement.as_ref().unwrap(), true)?;
                window.visible = false;
                return Ok(());
            }
            lease.native_style = Some(unsafe { (GetWindowLongPtrW(child, GWL_STYLE), GetWindowLongPtrW(child, GWL_EXSTYLE)) });
            native.set_decorations(false);
            attach(child, parent)?;
            runtime.native_child = true;
        }
        if let Some(mut handoff) = runtime.native_handoff.take() {
            let outcome = advance_handoff(runtime, &mut handoff, native, window, child, parent, monitors);
            match outcome {
                Ok(true) => {
                    runtime.native_mode = handoff.target;
                    runtime.native_result = Some((handoff.request_id, handoff.failure.map_or(Ok(()), Err)));
                }
                result => {
                    let expired = handoff.started.elapsed() > NATIVE_HANDOFF_TIMEOUT;
                    #[cfg(feature = "validation")]
                    if expired && handoff.failure.is_none() {
                        record_handoff_timeout(runtime, &handoff, native, window, child, parent, monitors, world_id);
                    }
                    let error = result.err().or_else(|| expired
                        .then(|| "原生窗口交接未在10秒内完成，恢复原承载".to_string()));
                    if let Some(error) = error {
                        if let Some(previous) = &handoff.failure {
                            let error = format!("{previous}；恢复原承载也失败：{error}");
                            runtime.native_handoff = Some(handoff);
                            return Err(error);
                        }
                        handoff.failure = Some(error);
                        handoff.target = handoff.previous;
                        handoff.stage = NativeStage::Prepare;
                        handoff.stage_frame = runtime.frames;
                        handoff.started = Instant::now();
                        runtime.window_return_requested = false;
                    }
                    runtime.native_handoff = Some(handoff);
                }
            }
        }
        if runtime.native_child {
            let viewport = runtime.viewport.clone();
            let visible = runtime.hello && viewport.visible && viewport.width > 0 && viewport.height > 0;
            native.set_visible(visible);
            synchronize_child(runtime, child, parent, &viewport, visible)?;
            let lease = runtime.window_lease.as_mut().unwrap();
            lease.editor_viewport = Some(viewport);
            publish_editor(window, lease);
            window.visible = visible;
            window.resolution.set_scale_factor_override(Some(unsafe { GetDpiForWindow(parent).max(96) } as f32 / 96.));
        } else {
            let lease = runtime.window_lease.as_mut().unwrap();
            if runtime.native_handoff.is_none() && runtime.native_mode == Mode::Playing && lease.mode_pending {
                apply_game_request(native, window, lease, monitors, native.has_focus())?;
            }
            publish_native(native, window, lease);
        }
        if runtime.hello && runtime.native_child && !runtime.ready {
            let build_identity = runtime.transport.build_identity.clone();
            let mut capabilities: Vec<String> = CAPABILITIES.iter().map(|value| (*value).into()).collect();
            if runtime.hotpatch_available { capabilities.push(feathrise_editor_protocol::FUNCTION_HOTPATCH_CAPABILITY.into()); }
            runtime.transport.send(Message::Ready { pid: std::process::id(), hwnd: hwnd as u64, world_id, build_identity, capabilities })?;
            runtime.ready = true;
        }
        Ok(())
    })
}

/// 只在显式验收的首次原始交接超时留证；采样发生在回滚改写stage/租约之前，绝不改变成功判据。
#[cfg(all(windows, feature = "validation"))]
fn record_handoff_timeout(runtime: &Runtime, handoff: &NativeHandoff, native: &winit::window::Window,
    window: &Window, hwnd: windows_sys::Win32::Foundation::HWND,
    parent: windows_sys::Win32::Foundation::HWND, monitors: &bevy::winit::WinitMonitors, world_id: u64)
{
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::GetWindowRgnBox,
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*}};
    static RECORDED: AtomicBool = AtomicBool::new(false);
    if RECORDED.swap(true, Ordering::Relaxed) { return; }
    let Some(lease) = runtime.window_lease.as_ref() else { return; };
    let mut client = RECT::default(); let mut bounds = RECT::default(); let mut region = RECT::default();
    let client_read = unsafe { GetClientRect(hwnd, &mut client) } != 0;
    let bounds_read = unsafe { GetWindowRect(hwnd, &mut bounds) } != 0;
    let region_type = unsafe { GetWindowRgnBox(hwnd, &mut region) };
    let actual_parent = unsafe { GetParent(hwnd) }; let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let enabled = unsafe { IsWindowEnabled(hwnd) } != 0;
    let visible = unsafe { IsWindowVisible(hwnd) } != 0;
    let minimized = unsafe { IsIconic(hwnd) } != 0;
    let size = native.inner_size(); let requested = requested_fullscreen(native, lease, monitors);
    let fullscreen_matches = requested.as_ref().ok().map(|requested| native.fullscreen() == *requested);
    let expected_visible = handoff.failure.is_none() || handoff.previous != Mode::Playing || handoff.source_visible;
    let stage = match handoff.stage {
        NativeStage::Prepare => "Prepare", NativeStage::AwaitWindowed => "AwaitWindowed",
        NativeStage::AwaitRestored => "AwaitRestored", NativeStage::AwaitAttach => "AwaitAttach",
        NativeStage::AwaitGameRestored => "AwaitGameRestored", NativeStage::AwaitApplied => "AwaitApplied",
        NativeStage::AwaitVisible => "AwaitVisible",
    };
    let placement = |saved: NativePlacement| serde_json::json!({"normal":saved.normal,
        "minimized_position":saved.minimized_position,"maximized_position":saved.maximized_position,
        "flags":saved.flags,"show_command":saved.show_command,"minimized":saved.minimized,"maximized":saved.maximized});
    let current_placement = read_placement(hwnd).map(&placement);
    let current_monitor = native.current_monitor().map(|monitor| {
        let position = monitor.position(); let size = monitor.size();
        serde_json::json!({"position":[position.x,position.y],"size":[size.width,size.height],"scale_factor":monitor.scale_factor()})
    });
    let requested_extent = requested.as_ref().ok().and_then(|requested| match requested {
        Some(winit::window::Fullscreen::Borderless(monitor)) => monitor.clone().or_else(|| native.current_monitor()).map(|monitor| {
            let position = monitor.position(); let size = monitor.size();
            serde_json::json!({"kind":"borderless","position":[position.x,position.y],"size":[size.width,size.height]})
        }),
        Some(winit::window::Fullscreen::Exclusive(video)) => {
            let size = video.size(); Some(serde_json::json!({"kind":"exclusive","size":[size.width,size.height]}))
        }
        None => Some(serde_json::json!({"kind":"windowed","size":[lease.resolution.physical_width(),lease.resolution.physical_height()]})),
    });
    // 每块独立构造再装入同一DTO，避免巨大json宏展开依赖提高整个crate的递归上限。
    let mut report = serde_json::json!({"source":"native-handoff-timeout-readback","not_success":true,
        "pid":std::process::id(),"world_id":world_id,"project_id":runtime.config.identity.project_id,
        "session":runtime.config.identity.session,"generation":runtime.transport.identity.generation,
        "request_id":handoff.request_id,"previous":handoff.previous,"target":handoff.target,"stage":stage,
        "frame":runtime.frames,"stage_frame":handoff.stage_frame,"elapsed_ms":handoff.started.elapsed().as_millis() as u64});
    report["prior_failure"] = serde_json::json!(handoff.failure);
    report["native_mode"] = serde_json::json!(runtime.native_mode);
    report["native_child"] = serde_json::json!(runtime.native_child);
    report["expected_visible"] = serde_json::json!(expected_visible);
    report["source_visible"] = serde_json::json!(handoff.source_visible);
    report["restore_focus"] = serde_json::json!(handoff.restore_focus);
    let mut actual = serde_json::json!({"hwnd":hwnd as usize as u64,"parent":actual_parent as usize as u64,"root":root as usize as u64,
            "author_parent":parent as usize as u64,"author_root":unsafe { GetAncestor(parent, GA_ROOT) } as usize as u64,
            "foreground":unsafe { GetForegroundWindow() } as usize as u64,"active":unsafe { GetActiveWindow() } as usize as u64,
            "focus":unsafe { GetFocus() } as usize as u64,"style":style,"extended_style":unsafe { GetWindowLongPtrW(hwnd,GWL_EXSTYLE) } as u32,
            "enabled":enabled,"visible":visible,"minimized":minimized,"maximized":unsafe { IsZoomed(hwnd) } != 0});
    actual["client_read"] = serde_json::json!(client_read);
    actual["client_rect"] = serde_json::json!([client.left,client.top,client.right,client.bottom]);
    actual["bounds_read"] = serde_json::json!(bounds_read);
    actual["window_rect"] = serde_json::json!([bounds.left,bounds.top,bounds.right,bounds.bottom]);
    actual["hrgn_type"] = serde_json::json!(region_type);
    actual["hrgn_box"] = serde_json::json!([region.left,region.top,region.right,region.bottom]);
    actual["placement"] = serde_json::json!(current_placement);
    report["actual"] = actual;
    report["winit"] = serde_json::json!({"inner_size":[size.width,size.height],"outer_size":[native.outer_size().width,native.outer_size().height],
            "outer_position":native.outer_position().ok().map(|position| [position.x,position.y]),
            "fullscreen":format!("{:?}",native.fullscreen()),"decorations":native.is_decorated(),
            "visible":native.is_visible(),"focused":native.has_focus(),"minimized":native.is_minimized(),"maximized":native.is_maximized(),
            "scale_factor":native.scale_factor(),"current_monitor":current_monitor});
    report["bevy"] = serde_json::json!({"mode":format!("{:?}",window.mode),"size":[window.physical_width(),window.physical_height()],
            "position":format!("{:?}",window.position),"decorations":window.decorations,"visible":window.visible,"focused":window.focused,
            "scale_factor":window.scale_factor(),"scale_factor_override":window.resolution.scale_factor_override()});
    report["lease"] = serde_json::json!({"mode":format!("{:?}",lease.mode),"applied_mode":format!("{:?}",lease.applied_mode),"mode_pending":lease.mode_pending,
            "resolution":[lease.resolution.physical_width(),lease.resolution.physical_height()],"scale_factor":lease.resolution.scale_factor(),
            "scale_factor_override":lease.resolution.scale_factor_override(),"position":format!("{:?}",lease.position),
            "decorations":lease.decorations,"native_style":lease.native_style,"restoring_placement":lease.restoring_placement,
            "placement":lease.placement.map(placement),"requested_fullscreen":format!("{requested:?}"),"requested_extent":requested_extent,
            "geometry_request":lease.geometry_request.as_ref().map(|request| serde_json::json!({"size":request.size.map(|size| [size.x,size.y]),
                "position":format!("{:?}",request.position),"issued":request.issued}))});
    report["checks"] = serde_json::json!({"activation_allowed":activation_allowed(hwnd,parent),"normal_window":normal_window(native,hwnd),
            "later_frame":runtime.frames > handoff.stage_frame,"root_is_self":root == hwnd,"parent_is_null":actual_parent.is_null(),
            "not_child":style & WS_CHILD == 0,"enabled":enabled,"no_author_hrgn":region_type == 0,
            "fullscreen_matches":fullscreen_matches,"decorations_match":native.is_decorated() == lease.decorations,
            "client_matches_inner_size":client_read && client.right-client.left == size.width as i32 && client.bottom-client.top == size.height as i32,
            "nonzero_inner_size":size.width > 0 && size.height > 0,"windowed_size_matches":size.width == lease.resolution.physical_width() && size.height == lease.resolution.physical_height(),
            "visible_matches":visible == expected_visible && native.is_visible() == Some(expected_visible),
            "placement_matches":placement_state_matches(native,hwnd,lease),
            "playing_applied_hidden":playing_applied(native,lease,hwnd,monitors,false),
            "playing_applied_visible":playing_applied(native,lease,hwnd,monitors,true)});
    let directory = std::env::var_os("NEW_EDITOR_EMBEDDED_GAME_SMOKE_DIR").map(std::path::PathBuf::from);
    let request_id = handoff.request_id;
    // 最多一个纯数据IO任务；磁盘、JSON编码和错误日志均不占窗口/渲染帧，也不追加无界队列。
    bevy::tasks::IoTaskPool::get().spawn(async move {
        let result = (|| -> Result<(), String> {
            let Some(directory) = directory else {
                eprintln!("FEATHRISE_NATIVE_HANDOFF_TIMEOUT {report}"); return Ok(());
            };
            if !directory.is_absolute() { return Err("原生交接诊断输出不是绝对夹具目录".into()); }
            let name = format!("{}-{request_id}-native-handoff-timeout",std::process::id());
            let pending = directory.join(format!("{name}.pending.json"));
            std::fs::write(&pending,serde_json::to_vec_pretty(&report).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
            std::fs::rename(pending,directory.join(format!("{name}.json"))).map_err(|error|error.to_string())
        })();
        if let Err(error) = result { eprintln!("FEATHRISE_NATIVE_HANDOFF_TIMEOUT_DIAGNOSTIC_FAILED {error}"); }
    }).detach();
}

#[cfg(windows)]
fn advance_handoff(
    runtime: &mut Runtime, handoff: &mut NativeHandoff, native: &winit::window::Window, window: &mut Window,
    hwnd: windows_sys::Win32::Foundation::HWND, parent: windows_sys::Win32::Foundation::HWND,
    monitors: &bevy::winit::WinitMonitors,
) -> Result<bool, String> {
    use windows_sys::Win32::UI::{Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::*};
    if handoff.failure.is_some() {
        // 原承载已经完整匹配时只读结清失败；不能为“回滚”重新激活健康游戏。
        let unchanged = if handoff.previous == Mode::Playing && !runtime.native_child {
            let lease = runtime.window_lease.as_ref().unwrap();
            playing_applied(native, lease, hwnd, monitors, handoff.source_visible)?
                && native.is_visible() == Some(handoff.source_visible)
                && (unsafe { IsWindowVisible(hwnd) } != 0) == handoff.source_visible
                && placement_state_matches(native, hwnd, lease)?
        } else if handoff.previous == Mode::Editing && runtime.native_child {
            let viewport = runtime.window_lease.as_ref().and_then(|lease| lease.editor_viewport.as_ref())
                .unwrap_or(&runtime.viewport);
            let visible = runtime.hello && viewport.visible && viewport.width > 0 && viewport.height > 0;
            editing_applied(native, hwnd, parent, viewport, visible)?
        } else { false };
        if unchanged {
            runtime.window_lease.as_mut().unwrap().restoring_placement = false;
            return Ok(true);
        }
    }
    match (handoff.target, handoff.stage) {
        (Mode::Playing, NativeStage::Prepare) => {
            // 只在明确交接且作者/工程仍在前台时激活一次；Alt+Tab后的迟到请求不抢焦点。
            if !activation_allowed(hwnd, parent) { return Ok(false); }
            native.set_visible(false);
            window.visible = false;
            let lease = runtime.window_lease.as_mut().unwrap();
            if runtime.native_child {
                let (style, extended) = lease.native_style.ok_or("缺少原生顶层样式租约")?;
                set_style(hwnd, GWL_STYLE, style & !(WS_CHILD as isize))?;
                set_style(hwnd, GWL_EXSTYLE, extended)?;
                set_parent(hwnd, std::ptr::null_mut())?;
                runtime.native_child = false;
                if unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) } == 0
                { return Err("恢复原生窗口边框失败".into()); }
                if unsafe { windows_sys::Win32::Graphics::Gdi::SetWindowRgn(hwnd, std::ptr::null_mut(), 1) } == 0 {
                    return Err("清除作者窗口裁剪失败".into());
                }
                unsafe { EnableWindow(hwnd, 1); }
                runtime.last_viewport = None;
                runtime.region_mode = None;
            }
            if native.fullscreen().is_some() { native.set_fullscreen(None); window.mode = WindowMode::Windowed; }
            lease.restoring_placement = true;
            restore_placement(native, hwnd, lease)?;
            handoff.stage = NativeStage::AwaitGameRestored;
            handoff.stage_frame = runtime.frames;
        }
        (Mode::Playing, NativeStage::AwaitGameRestored) => {
            if !normal_window(native, hwnd) { return Ok(false); }
            if !activation_allowed(hwnd, parent) { return Ok(false); }
            let lease = runtime.window_lease.as_mut().unwrap();
            finish_placement(native, lease);
            apply_game_request(native, window, lease, monitors, true)?;
            handoff.stage = NativeStage::AwaitApplied;
            handoff.stage_frame = runtime.frames;
        }
        (Mode::Playing, NativeStage::AwaitApplied) => {
            if runtime.frames <= handoff.stage_frame || !activation_allowed(hwnd, parent) { return Ok(false); }
            let lease = runtime.window_lease.as_mut().unwrap();
            if !playing_applied(native, lease, hwnd, monitors, false)? { return Ok(false); }
            let visible = handoff.failure.is_none() || handoff.previous != Mode::Playing || handoff.source_visible;
            native.set_visible(visible);
            window.visible = visible;
            let minimized = lease.placement.as_ref().is_some_and(|placement| placement.minimized);
            if minimized { native.set_minimized(true); }
            // 不持续执行：原生窗口随后完全服从自身Winit焦点与游戏CursorOptions策略。
            if visible && !minimized {
                unsafe { SetForegroundWindow(hwnd); windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(hwnd); }
            }
            publish_native(native, window, lease);
            handoff.stage = NativeStage::AwaitVisible;
            handoff.stage_frame = runtime.frames;
        }
        (Mode::Playing, NativeStage::AwaitVisible) => {
            if runtime.frames <= handoff.stage_frame { return Ok(false); }
            let lease = runtime.window_lease.as_mut().unwrap();
            // 显示/激活API返回不等于窗口已显示；下一First独立读回后才允许ModeApplied。
            // 用户此时Alt+Tab只影响输入许可，不再重试激活或抢回前台。
            let visible = handoff.failure.is_none() || handoff.previous != Mode::Playing || handoff.source_visible;
            if !playing_applied(native, lease, hwnd, monitors, visible)?
                || native.is_visible() != Some(visible) || (unsafe { IsWindowVisible(hwnd) } != 0) != visible
                || !placement_state_matches(native, hwnd, lease)?
            { return Ok(false); }
            lease.restoring_placement = false;
            return Ok(true);
        }
        (Mode::Editing, NativeStage::Prepare) => {
            if !runtime.native_child {
                // Winit可能回放带SW_SHOW/MAXIMIZE的SavedWindow；后台只等待，不抢其他应用前台。
                if runtime.native_mode == Mode::Playing && !activation_allowed(hwnd, parent) { return Ok(false); }
                handoff.restore_focus = activation_allowed(hwnd, parent);
                let lease = runtime.window_lease.as_mut().unwrap();
                if runtime.native_mode == Mode::Playing && !lease.restoring_placement {
                    capture_placement(native, hwnd, lease)?;
                }
                let placement = lease.placement.as_ref().ok_or("缺少工程normal placement租约")?;
                normalize_window(native, hwnd, placement, true)?;
                window.visible = false;
            }
            handoff.stage = NativeStage::AwaitRestored;
            handoff.stage_frame = runtime.frames;
        }
        (Mode::Editing, NativeStage::AwaitRestored) => {
            if !normal_window(native, hwnd) { return Ok(false); }
            if runtime.native_mode == Mode::Playing && native.fullscreen().is_some()
                && !activation_allowed(hwnd, parent) { return Ok(false); }
            // 先归一化真实MIN/MAX及Winit缓存，再让同一顶层窗口退出全屏。
            // 此时隐藏调用不会再通过Winit的MAX分支重新显示并激活窗口。
            if !runtime.native_child {
                native.set_visible(false);
                native.set_fullscreen(None);
                window.mode = WindowMode::Windowed;
            }
            handoff.stage = NativeStage::AwaitWindowed;
            handoff.stage_frame = runtime.frames;
        }
        (Mode::Editing, NativeStage::AwaitWindowed) => {
            if runtime.frames <= handoff.stage_frame || native.fullscreen().is_some() { return Ok(false); }
            if !runtime.native_child && !normal_window(native, hwnd) {
                let placement = runtime.window_lease.as_ref().and_then(|lease| lease.placement.as_ref())
                    .ok_or("缺少退出全屏后的normal placement租约")?;
                normalize_window(native, hwnd, placement, true)?;
            }
            handoff.stage = NativeStage::AwaitAttach;
        }
        (Mode::Editing, NativeStage::AwaitAttach) => {
            // 以原生状态和Winit实际缓存为证据，不用“已等一帧”推定最小化/最大化已经退出。
            if !normal_window(native, hwnd) || native.fullscreen().is_some() { return Ok(false); }
            if !runtime.native_child {
                native.set_visible(false);
                window.visible = false;
                let lease = runtime.window_lease.as_mut().unwrap();
                lease.native_style = Some(unsafe { (GetWindowLongPtrW(hwnd, GWL_STYLE), GetWindowLongPtrW(hwnd, GWL_EXSTYLE)) });
                native.set_decorations(false);
                runtime.last_viewport = None;
                runtime.region_mode = None;
            }
            // 回滚也重新核对并安装父链，不能仅凭转场前的native_child布尔值推定恢复成功。
            attach(hwnd, parent)?;
            runtime.native_child = true;
            handoff.stage = NativeStage::AwaitApplied;
            handoff.stage_frame = runtime.frames;
        }
        (Mode::Editing, NativeStage::AwaitApplied) => {
            if runtime.frames <= handoff.stage_frame || !runtime.native_child || unsafe { GetAncestor(hwnd, GA_PARENT) } != parent {
                return Ok(false);
            }
            let viewport = runtime.viewport.clone();
            let visible = runtime.hello && viewport.visible && viewport.width > 0 && viewport.height > 0;
            native.set_visible(visible);
            synchronize_child(runtime, hwnd, parent, &viewport, visible)?;
            if !editing_applied(native, hwnd, parent, &viewport, visible)? { return Ok(false); }
            if handoff.restore_focus && activation_allowed(hwnd, parent) {
                let root = unsafe { GetAncestor(parent, GA_ROOT) };
                let mut root_pid = 0;
                unsafe {
                    GetWindowThreadProcessId(root, &mut root_pid);
                    AllowSetForegroundWindow(root_pid);
                    AllowSetForegroundWindow(runtime.config.parent_pid);
                    SetForegroundWindow(root);
                    windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(parent);
                }
            }
            handoff.stage = NativeStage::AwaitVisible;
            handoff.stage_frame = runtime.frames;
        }
        (Mode::Editing, NativeStage::AwaitVisible) => {
            if runtime.frames <= handoff.stage_frame { return Ok(false); }
            let viewport = runtime.viewport.clone();
            let visible = runtime.hello && viewport.visible && viewport.width > 0 && viewport.height > 0;
            native.set_visible(visible);
            synchronize_child(runtime, hwnd, parent, &viewport, visible)?;
            if !editing_applied(native, hwnd, parent, &viewport, visible)? { return Ok(false); }
            let foreground = unsafe { GetForegroundWindow() };
            let root = unsafe { GetAncestor(parent, GA_ROOT) };
            if foreground == hwnd || (handoff.restore_focus && foreground == root
                && unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() } != parent)
            { return Ok(false); }
            return Ok(true);
        }
        _ => return Err("原生窗口交接阶段与目标不一致".into()),
    }
    Ok(false)
}

#[cfg(windows)]
fn activation_allowed(hwnd: windows_sys::Win32::Foundation::HWND, parent: windows_sys::Win32::Foundation::HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let foreground = unsafe { GetForegroundWindow() };
    foreground == hwnd || foreground == unsafe { GetAncestor(parent, GA_ROOT) }
}

#[cfg(windows)]
fn read_placement(hwnd: windows_sys::Win32::Foundation::HWND) -> Result<NativePlacement, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    if unsafe { GetWindowPlacement(hwnd, &mut placement) } == 0 { return Err("读取工程normal placement失败".into()); }
    let minimized = unsafe { IsIconic(hwnd) } != 0;
    Ok(NativePlacement {
        normal: [placement.rcNormalPosition.left, placement.rcNormalPosition.top,
            placement.rcNormalPosition.right, placement.rcNormalPosition.bottom],
        minimized_position: [placement.ptMinPosition.x, placement.ptMinPosition.y],
        maximized_position: [placement.ptMaxPosition.x, placement.ptMaxPosition.y],
        flags: placement.flags, show_command: placement.showCmd, minimized,
        maximized: unsafe { IsZoomed(hwnd) } != 0 || (minimized && placement.flags & WPF_RESTORETOMAXIMIZED != 0),
    })
}

#[cfg(windows)]
fn capture_placement(native: &winit::window::Window, hwnd: windows_sys::Win32::Foundation::HWND,
    lease: &mut GameWindowLease) -> Result<(), String>
{
    if lease.restoring_placement { return Ok(()); }
    let current = read_placement(hwnd)?;
    if native.fullscreen().is_none() {
        lease.placement = Some(current);
    } else if let Some(saved) = lease.placement.as_mut() {
        // fullscreen的normal rect可能已是整屏；保留进入前的位置/MAX恢复目标，只更新当前MIN意图。
        saved.minimized = current.minimized;
    }
    Ok(())
}

#[cfg(windows)]
fn placement_for_normalize(saved: &NativePlacement) -> windows_sys::Win32::UI::WindowsAndMessaging::WINDOWPLACEMENT {
    use windows_sys::Win32::{Foundation::{POINT, RECT}, UI::WindowsAndMessaging::*};
    WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        flags: saved.flags & !(WPF_RESTORETOMAXIMIZED | WPF_ASYNCWINDOWPLACEMENT),
        showCmd: SW_SHOWNOACTIVATE as u32,
        ptMinPosition: POINT { x: saved.minimized_position[0], y: saved.minimized_position[1] },
        ptMaxPosition: POINT { x: saved.maximized_position[0], y: saved.maximized_position[1] },
        rcNormalPosition: RECT { left: saved.normal[0], top: saved.normal[1], right: saved.normal[2], bottom: saved.normal[3] },
    }
}

#[cfg(windows)]
fn normalize_window(native: &winit::window::Window, hwnd: windows_sys::Win32::Foundation::HWND,
    saved: &NativePlacement, keep_hidden: bool) -> Result<(), String>
{
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let placement = placement_for_normalize(saved);
    // 先保存过的normal rect原样交回Win32；不能用最大化客户区、作者尺寸或屏幕/工作区互换坐标。
    if unsafe { SetWindowPlacement(hwnd, &placement) } == 0 { return Err("无激活恢复工程normal placement失败".into()); }
    if unsafe { IsIconic(hwnd) } == 0 {
        // Winit在此API内先用真实IsIconic校准MIN；已恢复时old/new都false，不走激活的SW_RESTORE。
        native.set_minimized(false);
    }
    if keep_hidden { unsafe { ShowWindow(hwnd, SW_HIDE); } }
    Ok(())
}

#[cfg(windows)]
fn normal_window(native: &winit::window::Window, hwnd: windows_sys::Win32::Foundation::HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed};
    (unsafe { IsIconic(hwnd) == 0 && IsZoomed(hwnd) == 0 })
        && native.is_minimized() == Some(false) && !native.is_maximized()
}

#[cfg(windows)]
fn restore_placement(native: &winit::window::Window, hwnd: windows_sys::Win32::Foundation::HWND,
    lease: &mut GameWindowLease) -> Result<(), String>
{
    if let Some(saved) = lease.placement {
        native.set_decorations(lease.decorations);
        normalize_window(native, hwnd, &saved, true)?;
    }
    Ok(())
}

#[cfg(windows)]
fn finish_placement(native: &winit::window::Window, lease: &mut GameWindowLease) {
    if let Some(saved) = lease.placement {
        let size = native.inner_size();
        // 归还时先读取真实normal客户区，避免旧MAX/MIN尺寸进入下一次全屏的SavedWindow。
        lease.resolution.set_physical_resolution(size.width.max(1), size.height.max(1));
        if let Ok(position) = native.outer_position() {
            lease.position = bevy::window::WindowPosition::At(IVec2::new(position.x, position.y));
        }
        if saved.maximized { native.set_maximized(true); }
    }
}

#[cfg(windows)]
fn placement_state_matches(native: &winit::window::Window, hwnd: windows_sys::Win32::Foundation::HWND,
    lease: &GameWindowLease) -> Result<bool, String>
{
    let Some(saved) = lease.placement else { return Ok(true); };
    let current = read_placement(hwnd)?;
    Ok(saved.minimized == current.minimized
        && (native.fullscreen().is_some() || saved.maximized == current.maximized))
}

#[cfg(windows)]
fn playing_applied(native: &winit::window::Window, lease: &GameWindowLease,
    hwnd: windows_sys::Win32::Foundation::HWND, monitors: &bevy::winit::WinitMonitors,
    require_visible: bool) -> Result<bool, String>
{
    use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::GetWindowRgnBox,
        UI::{Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*}};
    let requested = requested_fullscreen(native, lease, monitors)?;
    let mut client = RECT::default();
    let mut bounds = RECT::default();
    let mut region = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 || unsafe { GetWindowRect(hwnd, &mut bounds) } == 0 {
        return Err("读取预览真实客户区失败".into());
    }
    let size = native.inner_size();
    let minimized = unsafe { IsIconic(hwnd) } != 0;
    if unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd || !unsafe { GetParent(hwnd) }.is_null()
        || unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_CHILD != 0
        || unsafe { IsWindowEnabled(hwnd) } == 0 || unsafe { GetWindowRgnBox(hwnd, &mut region) } != 0
        || native.fullscreen() != requested || native.is_decorated() != lease.decorations
        || (!minimized && (size.width == 0 || size.height == 0
            || client.right - client.left != size.width as i32 || client.bottom - client.top != size.height as i32))
        || (require_visible && (unsafe { IsWindowVisible(hwnd) } == 0 || native.is_visible() != Some(true)))
    { return Ok(false); }
    if require_visible && lease.placement.as_ref().is_some_and(|placement| placement.minimized != minimized) {
        return Ok(false);
    }
    // 显示后的最小化是原生用户操作，不重激活；非最小化时再独立核对全屏真正占用的显示器范围。
    if unsafe { IsIconic(hwnd) } == 0 {
        match requested {
            Some(winit::window::Fullscreen::Borderless(monitor)) => {
                let monitor = monitor.or_else(|| native.current_monitor()).ok_or("全屏显示器已不可用")?;
                let position = monitor.position();
                let expected = monitor.size();
                if bounds.left != position.x || bounds.top != position.y
                    || bounds.right - bounds.left != expected.width as i32
                    || bounds.bottom - bounds.top != expected.height as i32
                    || size != expected
                { return Ok(false); }
            }
            Some(winit::window::Fullscreen::Exclusive(video)) => {
                if size != video.size() { return Ok(false); }
            }
            None => {
                if !native.is_maximized() && (size.width != lease.resolution.physical_width()
                    || size.height != lease.resolution.physical_height())
                { return Ok(false); }
            }
        }
    }
    Ok(true)
}

#[cfg(windows)]
fn editing_applied(native: &winit::window::Window, hwnd: windows_sys::Win32::Foundation::HWND,
    parent: windows_sys::Win32::Foundation::HWND, viewport: &Viewport, visible: bool) -> Result<bool, String>
{
    use windows_sys::Win32::{Foundation::{POINT, RECT}, Graphics::Gdi::{GetWindowRgnBox, ScreenToClient},
        UI::{Input::KeyboardAndMouse::IsWindowEnabled, WindowsAndMessaging::*}};
    let mut client = RECT::default();
    let mut bounds = RECT::default();
    let mut region = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 || unsafe { GetWindowRect(hwnd, &mut bounds) } == 0 {
        return Err("读取返回编辑的真实客户区失败".into());
    }
    let mut origin = POINT { x: bounds.left, y: bounds.top };
    if unsafe { ScreenToClient(parent, &mut origin) } == 0 { return Err("读取返回编辑的父局部坐标失败".into()); }
    Ok(native.fullscreen().is_none() && !native.is_decorated()
        && normal_window(native, hwnd)
        && unsafe { GetParent(hwnd) } == parent && unsafe { GetAncestor(hwnd, GA_PARENT) } == parent
        && unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32 & WS_CHILD != 0
        && unsafe { IsWindowEnabled(hwnd) } == 0
        && (unsafe { IsWindowVisible(hwnd) } != 0) == visible
        && native.is_visible() == Some(visible)
        && origin.x == viewport.x && origin.y == viewport.y
        && client.right - client.left == viewport.width.max(1) as i32
        && client.bottom - client.top == viewport.height.max(1) as i32
        && unsafe { GetWindowRgnBox(hwnd, &mut region) } != 0)
}

#[cfg(windows)]
fn requested_fullscreen(native: &winit::window::Window, lease: &GameWindowLease, monitors: &bevy::winit::WinitMonitors)
    -> Result<Option<winit::window::Fullscreen>, String>
{
    use bevy::winit::{get_selected_videomode, select_monitor};
    use winit::window::Fullscreen;
    match &lease.mode {
        WindowMode::Windowed => Ok(None),
        WindowMode::BorderlessFullscreen(selection) => Ok(Some(Fullscreen::Borderless(select_monitor(
            monitors, native.primary_monitor(), native.current_monitor(), selection,
        )))),
        WindowMode::Fullscreen(selection, video) => {
            let monitor = select_monitor(monitors, native.primary_monitor(), native.current_monitor(), selection)
                .ok_or("工程请求的全屏显示器不可用")?;
            let video = get_selected_videomode(&monitor, video).ok_or("工程请求的独占全屏模式不可用")?;
            Ok(Some(Fullscreen::Exclusive(video)))
        }
    }
}

#[cfg(windows)]
fn apply_game_request(native: &winit::window::Window, window: &mut Window, lease: &mut GameWindowLease,
    monitors: &bevy::winit::WinitMonitors, may_activate: bool) -> Result<(), String>
{
    let desired = requested_fullscreen(native, lease, monitors)?;
    if native.fullscreen() != desired && !may_activate {
        // 普通后台设置等工程重新前台再采用；不能借全屏API抢走Alt+Tab目标的焦点。
        lease.mode_pending = true;
        window.mode = lease.applied_mode.clone();
        return Ok(());
    }
    if native.is_decorated() != lease.decorations { native.set_decorations(lease.decorations); }
    if native.fullscreen().is_some() && native.fullscreen() != desired { native.set_fullscreen(None); }
    if native.fullscreen().is_none() && !lease.restoring_placement && native.is_minimized() != Some(true)
        && let Some(request) = &mut lease.geometry_request && !request.issued
    {
        // normal lease只用于转场恢复；稳定Playing只采用与上次发布事实不同的真实工程写入。
        // 原生MAX/MIN/还原事实不会生成命令，更不能每帧用旧normal尺寸把系统最小化撤销。
        if let Some(position) = request.position.as_ref() {
            let position = bevy::winit::winit_window_position(position, &lease.resolution,
                monitors, native.primary_monitor(), native.current_monitor());
            request.position = position.map(|position| bevy::window::WindowPosition::At(IVec2::new(position.x, position.y)));
            if let Some(position) = position && native.outer_position().ok() != Some(position) {
                native.set_outer_position(position);
            }
        }
        if let Some(size) = request.size {
            let size = winit::dpi::PhysicalSize::new(size.x, size.y);
            if native.inner_size() != size { let _ = native.request_inner_size(size); }
        }
        request.issued = true;
    }
    if native.fullscreen() != desired {
        if native.fullscreen().is_none() && !lease.restoring_placement {
            use raw_window_handle::HasWindowHandle;
            let raw_window_handle::RawWindowHandle::Win32(raw) = native.window_handle().map_err(|error| error.to_string())?.as_raw() else {
                return Err("全屏租约缺少Win32窗口".into());
            };
            capture_placement(native, raw.hwnd.get() as windows_sys::Win32::Foundation::HWND, lease)?;
        }
        native.set_fullscreen(desired);
    }
    lease.applied_mode = lease.mode.clone();
    lease.mode_pending = false;
    window.mode = lease.mode.clone();
    window.decorations = lease.decorations;
    Ok(())
}

#[cfg(windows)]
fn publish_native(native: &winit::window::Window, window: &mut Window, lease: &mut GameWindowLease) {
    let minimized = native.is_minimized() == Some(true);
    let size = native.inner_size();
    window.resolution.set_scale_factor(native.scale_factor() as f32);
    window.resolution.set_scale_factor_override(lease.resolution.scale_factor_override());
    // MIN/隐藏过程中的真实0尺寸也完整发布和记住，不能夹成1×1后误当成工程resize。
    window.resolution.set_physical_resolution(size.width, size.height);
    if let Ok(position) = native.outer_position() {
        window.position = bevy::window::WindowPosition::At(IVec2::new(position.x, position.y));
    }
    // visible是工程可写的窗口请求，稳定Playing不能用旧原生读回覆盖Update刚提出的隐藏/显示。
    // 初次显示仅由转场成功边沿执行；随后交给原Window/Winit可见性生命周期。
    window.focused = native.has_focus() && !minimized;
    lease.remember_native_geometry(window, minimized);
}

#[cfg(windows)]
fn set_style(hwnd: windows_sys::Win32::Foundation::HWND, index: i32, value: isize) -> Result<(), String> {
    use windows_sys::Win32::{Foundation::{GetLastError, SetLastError}, UI::WindowsAndMessaging::SetWindowLongPtrW};
    unsafe {
        SetLastError(0);
        if SetWindowLongPtrW(hwnd, index, value) == 0 && GetLastError() != 0 { return Err(std::io::Error::last_os_error().to_string()); }
    }
    Ok(())
}

#[cfg(windows)]
fn set_parent(hwnd: windows_sys::Win32::Foundation::HWND, parent: windows_sys::Win32::Foundation::HWND) -> Result<(), String> {
    use windows_sys::Win32::{Foundation::{GetLastError, SetLastError}, UI::WindowsAndMessaging::SetParent};
    unsafe {
        SetLastError(0);
        if SetParent(hwnd, parent).is_null() && GetLastError() != 0 { return Err(std::io::Error::last_os_error().to_string()); }
    }
    Ok(())
}

#[cfg(windows)]
fn attach(hwnd: windows_sys::Win32::Foundation::HWND, parent: windows_sys::Win32::Foundation::HWND) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let desired = (style & !(WS_OVERLAPPEDWINDOW | WS_POPUP | WS_MINIMIZE | WS_MAXIMIZE)) | WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
    set_style(hwnd, GWL_STYLE, desired as isize)?;
    set_parent(hwnd, parent)?;
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(hwnd, 0); }
    Ok(())
}

#[cfg(windows)]
fn synchronize_child(runtime: &mut Runtime, hwnd: windows_sys::Win32::Foundation::HWND,
    parent: windows_sys::Win32::Foundation::HWND, viewport: &Viewport, visible: bool) -> Result<(), String>
{
    use windows_sys::Win32::{Foundation::{POINT, RECT}, Graphics::Gdi::{GetWindowRgnBox, ScreenToClient}};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    if unsafe { GetAncestor(hwnd, GA_PARENT) } != parent { return Err("编辑视口真实父链发生变化".into()); }
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let desired = (style & !(WS_OVERLAPPEDWINDOW | WS_POPUP | WS_MINIMIZE | WS_MAXIMIZE)) | WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN;
    let extended = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let desired_extended = (extended & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW;
    let changed = style != desired || extended != desired_extended;
    if style != desired { set_style(hwnd, GWL_STYLE, desired as isize)?; }
    if extended != desired_extended { set_style(hwnd, GWL_EXSTYLE, desired_extended as isize)?; }
    let mut client = RECT::default();
    let mut bounds = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) } == 0 || unsafe { GetWindowRect(hwnd, &mut bounds) } == 0 {
        return Err("读取编辑租约的真实客户区失败".into());
    }
    let mut origin = POINT { x: bounds.left, y: bounds.top };
    if unsafe { ScreenToClient(parent, &mut origin) } == 0 { return Err("读取编辑视口父局部坐标失败".into()); }
    let geometry_changed = origin.x != viewport.x || origin.y != viewport.y
        || client.right - client.left != viewport.width.max(1) as i32
        || client.bottom - client.top != viewport.height.max(1) as i32;
    let viewport_changed = runtime.last_viewport.as_ref() != Some(viewport);
    let mut region_bounds = RECT::default();
    let region_missing = unsafe { GetWindowRgnBox(hwnd, &mut region_bounds) } == 0;
    if changed || geometry_changed || viewport_changed {
        if unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), viewport.x, viewport.y,
            viewport.width.max(1) as i32, viewport.height.max(1) as i32,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | if visible { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW },
        ) } == 0 { return Err(std::io::Error::last_os_error().to_string()); }
    }
    if changed || viewport_changed || region_missing || runtime.region_mode != Some(Mode::Editing) {
        super::native_region::install(hwnd, viewport.width, viewport.height, Mode::Editing,
            &viewport.excluded_rects, &viewport.excluded_shapes)?;
        runtime.last_viewport = Some(viewport.clone());
        runtime.region_mode = Some(Mode::Editing);
    }
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(hwnd, 0); }
    Ok(())
}

/// 显式验收以真实原生客户区/前台为准，不能再拿作者SetViewport的焦点充当预览输入事实。
#[cfg(all(windows, feature = "validation"))]
pub(super) fn capture_viewport(runtime: &Runtime, mode: Mode) -> Viewport {
    if mode == Mode::Editing {
        return runtime.window_lease.as_ref().and_then(|lease| lease.editor_viewport.clone())
            .unwrap_or_else(|| runtime.viewport.clone());
    }
    use windows_sys::Win32::{Foundation::RECT, UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*}};
    let mut viewport = Viewport::default();
    if let Some(hwnd) = runtime.hwnd {
        let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
        let mut rect = RECT::default();
        unsafe {
            if GetClientRect(hwnd, &mut rect) != 0 {
                viewport.width = (rect.right - rect.left).max(0) as u32;
                viewport.height = (rect.bottom - rect.top).max(0) as u32;
            }
            viewport.visible = IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0;
            viewport.focused = viewport.visible && GetForegroundWindow() == hwnd
                && GetActiveWindow() == hwnd && GetFocus() == hwnd;
        }
    }
    viewport
}

#[cfg(all(windows, feature = "validation"))]
pub(super) fn evidence(runtime: &Runtime, mode: Mode) -> Result<serde_json::Value, String> {
    let hwnd = runtime.hwnd.ok_or("工程窗口尚未登记")? as windows_sys::Win32::Foundation::HWND;
    let viewport = capture_viewport(runtime, mode);
    if mode == Mode::Editing {
        return super::native_region::evidence(hwnd, viewport.width, viewport.height, mode,
            &viewport.excluded_rects, &viewport.excluded_shapes);
    }
    use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::GetWindowRgnBox,
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*}};
    let mut window_rect = RECT::default();
    let mut region_rect = RECT::default();
    let mut pid = 0;
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let extended = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let parent = unsafe { GetParent(hwnd) };
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let enabled = unsafe { IsWindowEnabled(hwnd) } != 0;
    let region_type = unsafe { GetWindowRgnBox(hwnd, &mut region_rect) };
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid); GetWindowRect(hwnd, &mut window_rect); }
    if pid != std::process::id() || style & WS_CHILD != 0 || !parent.is_null() || root != hwnd
        || !enabled || region_type != 0
    { return Err("真实预览窗口尚未完整恢复顶层承载/输入/无作者裁剪".into()); }
    let applied_mode = runtime.window_lease.as_ref().map(|lease| &lease.applied_mode);
    Ok(serde_json::json!({"source":"native-Win32-window-readback","verified":true,
        "presentation":"native","mode":mode,"hwnd":hwnd as usize as u64,"pid":pid,
        "parent_hwnd":parent as usize as u64,"parent_pid":0,"root_hwnd":root as usize as u64,
        "is_child":false,"style":style,"extended_style":extended,"window_enabled":enabled,
        "width":viewport.width,"height":viewport.height,"visible":viewport.visible,"focused":viewport.focused,
        "active_hwnd":unsafe { GetActiveWindow() } as usize as u64,
        "focus_hwnd":unsafe { GetFocus() } as usize as u64,"minimized":unsafe { IsIconic(hwnd) } != 0,
        "window_rect":[window_rect.left,window_rect.top,window_rect.right,window_rect.bottom],
        "window_mode":format!("{applied_mode:?}"),"fullscreen":applied_mode.is_some_and(|mode| !matches!(mode,WindowMode::Windowed)),
        "cursor_handoff":{"owned":runtime.cursor_focus_owned,"pending":runtime.cursor_anchor_pending,
            "reanchors":runtime.cursor_reanchors},
        "region_equal":true,"region_type":region_type,"samples":[],"excluded_rects":[],
        "requested_excluded_rects":[],"excluded_shapes":[],"requested_excluded_shapes":[]}))
}

/// Winit的Last样式刷新可能重写WS_CHILD；仅用First冻结的租约收尾，不再改相机尺寸。
pub(super) fn finalize(_thread: NonSendMarker, mut runtime: NonSendMut<Runtime>, mut exit: MessageWriter<AppExit>) {
    if runtime.failed || runtime.stopping || !runtime.native_child { return; }
    #[cfg(windows)]
    if let Some(hwnd) = runtime.hwnd {
        let viewport = runtime.window_lease.as_ref().and_then(|lease| lease.editor_viewport.clone());
        if let Some(viewport) = viewport {
            let visible = runtime.hello && viewport.visible && viewport.width > 0 && viewport.height > 0;
            let parent = runtime.config.parent as windows_sys::Win32::Foundation::HWND;
            if let Err(error) = synchronize_child(&mut runtime, hwnd as windows_sys::Win32::Foundation::HWND, parent, &viewport, visible) {
                fail(&mut runtime, error, &mut exit);
            }
        }
    }
}

#[cfg(not(windows))]
fn update_native(_: &mut Runtime, _: Entity, _: u64, _: &mut Window, _: &CursorOptions,
    _: &RawHandleWrapper, _: Option<&bevy::winit::WinitMonitors>) -> Result<(), String>
{ Err("当前嵌入视口仅支持Windows".into()) }

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::window::{CursorGrabMode, MonitorSelection, WindowResolution};

    #[test]
    fn native_maximize_minimize_and_restore_facts_do_not_become_geometry_requests() {
        let mut window = Window { resolution: WindowResolution::new(960, 640), ..default() };
        let mut lease = GameWindowLease::new(&window, &CursorOptions::default());
        // 生产发布与生产请求捕获走同一顺序；0尺寸保留在事实快照，normal恢复资料仍独立。
        for (size, position, minimized) in [
            ([1920, 1040], [-8, -8], false),
            ([0, 0], [-32000, -32000], true),
            ([960, 640], [100, 100], false),
        ] {
            window.resolution.set_physical_resolution(size[0], size[1]);
            window.position = bevy::window::WindowPosition::At(IVec2::new(position[0], position[1]));
            lease.remember_native_geometry(&window, minimized);
            lease.capture_geometry_request(&window);
            assert!(lease.geometry_request.is_none());
            assert_eq!(lease.native_geometry.as_ref().unwrap().resolution.physical_size(), window.physical_size());
        }
        assert_eq!(lease.resolution.physical_size(), UVec2::new(960, 640));
    }

    #[test]
    fn game_geometry_write_survives_native_publication_until_matching_readback() {
        let mut actual = Window { resolution: WindowResolution::new(960, 640), ..default() };
        let mut lease = GameWindowLease::new(&actual, &CursorOptions::default());
        lease.remember_native_geometry(&actual, false);
        let mut requested = actual.clone();
        requested.resolution.set_physical_resolution(1100, 700);
        requested.position = bevy::window::WindowPosition::At(IVec2::new(200, 150));
        lease.capture_geometry_request(&requested);
        assert_eq!(lease.geometry_request.as_ref().unwrap().size, Some(UVec2::new(1100, 700)));
        lease.geometry_request.as_mut().unwrap().issued = true;
        lease.remember_native_geometry(&actual, false);
        lease.capture_geometry_request(&actual);
        assert!(lease.geometry_request.as_ref().unwrap().issued, "发布旧事实不能重发或丢弃新请求");
        actual = requested;
        lease.remember_native_geometry(&actual, false);
        assert!(lease.geometry_request.is_none());
        lease.capture_geometry_request(&actual);
        assert!(lease.geometry_request.is_none());
    }

    #[test]
    fn same_focus_release_then_grab_rearms_once_without_repeating_on_stable_frames() {
        let (mut runtime, _, _) = super::super::transport::test_runtime();
        observe_cursor_owner(&mut runtime, true);
        observe_grab_request(&mut runtime, CursorGrabMode::Locked);
        runtime.cursor_anchor_pending = false; // 模拟已完成的第一次原生交接。
        observe_grab_request(&mut runtime, CursorGrabMode::Locked);
        assert!(!runtime.cursor_anchor_pending);
        observe_grab_request(&mut runtime, CursorGrabMode::None);
        assert!(!runtime.cursor_anchor_pending);
        assert!(runtime.cursor_focus_owned);
        observe_grab_request(&mut runtime, CursorGrabMode::Locked);
        assert!(runtime.cursor_anchor_pending);
        runtime.cursor_anchor_pending = false;
        observe_grab_request(&mut runtime, CursorGrabMode::Confined);
        assert!(runtime.cursor_anchor_pending);
        observe_cursor_owner(&mut runtime, false);
        assert!(!runtime.cursor_anchor_pending);
        assert_eq!(runtime.cursor_last_grab, None);
    }

    #[test]
    fn cursor_anchor_requires_a_focus_edge_real_ownership_and_a_game_grab_outside_the_client() {
        let client = [100, 200, 500, 600];
        let author_button = [300, 30];
        assert!(cursor_anchor_needed(true, true, CursorGrabMode::Locked, author_button, client));
        assert!(cursor_anchor_needed(true, true, CursorGrabMode::Confined, author_button, client));
        assert!(!cursor_anchor_needed(true, true, CursorGrabMode::None, author_button, client));
        assert!(!cursor_anchor_needed(true, false, CursorGrabMode::Locked, author_button, client));
        assert!(!cursor_anchor_needed(false, true, CursorGrabMode::Locked, author_button, client));
        assert!(!cursor_anchor_needed(true, true, CursorGrabMode::Locked, [100, 200], client));
        assert!(cursor_anchor_needed(true, true, CursorGrabMode::Locked, [500, 200], client));
        assert!(!cursor_anchor_needed(true, true, CursorGrabMode::Locked, author_button, [100, 200, 100, 600]));
    }

    #[test]
    fn editor_lease_keeps_the_project_fullscreen_and_dpi_request() {
        let game = Window {
            mode: WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
            resolution: WindowResolution::new(1280, 720).with_scale_factor_override(1.25),
            ..default()
        };
        let mut lease = GameWindowLease::new(&game, &CursorOptions::default());
        lease.editor_viewport = Some(Viewport { width: 640, height: 360, ..default() });
        let mut editor = game.clone();
        publish_editor(&mut editor, &lease);
        editor.resolution.set_scale_factor_override(Some(2.0));
        lease.capture_editor_request(&editor);
        assert!(matches!(editor.mode, WindowMode::Windowed));
        assert!(matches!(lease.mode, WindowMode::BorderlessFullscreen(MonitorSelection::Primary)));
        assert_eq!(lease.resolution.physical_width(), 1280);
        assert_eq!(lease.resolution.scale_factor_override(), Some(1.25));
        assert!(lease.decorations);
    }

    #[test]
    fn leaving_fullscreen_does_not_save_the_desktop_extent_as_the_windowed_size() {
        let mut window = Window { resolution: WindowResolution::new(960, 640), focused: true, ..default() };
        let mut lease = GameWindowLease::new(&window, &CursorOptions::default());
        window.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Primary);
        window.resolution.set_physical_resolution(1920, 1080);
        lease.capture_game(&window, &CursorOptions::default(), true);
        window.mode = WindowMode::Windowed;
        lease.capture_game(&window, &CursorOptions::default(), true);
        assert!(matches!(lease.mode, WindowMode::Windowed));
        assert_eq!(lease.resolution.physical_size(), UVec2::new(960, 640));
        window.resolution.set_physical_resolution(1100, 700);
        lease.capture_game(&window, &CursorOptions::default(), true);
        assert_eq!(lease.resolution.physical_size(), UVec2::new(1100, 700));
    }

    #[test]
    fn background_fullscreen_and_cursor_requests_survive_the_temporary_actual_state() {
        let window = Window { focused: false, ..default() };
        let cursor = CursorOptions { grab_mode: CursorGrabMode::Locked, visible: false, ..default() };
        let mut lease = GameWindowLease::new(&window, &cursor);
        lease.mode = WindowMode::BorderlessFullscreen(MonitorSelection::Primary);
        lease.mode_pending = true;
        lease.capture_game(&window, &CursorOptions::default(), true);
        assert!(lease.mode_pending);
        assert!(matches!(lease.mode, WindowMode::BorderlessFullscreen(MonitorSelection::Primary)));
        assert_eq!(lease.cursor.grab_mode, CursorGrabMode::Locked);
        assert!(!lease.cursor.visible);
    }
}
