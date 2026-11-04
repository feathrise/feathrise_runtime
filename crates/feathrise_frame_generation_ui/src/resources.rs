//! 主世界请求与 RenderApp 同帧权限；失配和未决事务默认恢复普通真实帧。
use crate::types::{FrameGenerationUiBackend, UiFrame};
use bevy::{prelude::*, render::extract_resource::ExtractResource}; // 固定小型请求跨世界提取。 // SDK 无关身份和图像所有权。
#[derive(Resource, Clone, Copy, Debug, Default, ExtractResource)] // 无资格时不改变场景/UI。
pub struct FrameGenerationUiRequest {
    // 两后端独立发布，按所有者释放请求。
    pub enabled: bool,                         // 后端最终资格。
    pub camera: Option<Entity>,                // 与渲染相机精确匹配。
    pub backend_ready: bool, // 原生交换链/provider 可用，不要求上一帧成功以避免循环门。
    backend: Option<FrameGenerationUiBackend>, // 仅阻止关闭的另一后端覆盖有效请求。
} // 请求结束。
impl FrameGenerationUiRequest {
    // 所有者感知的请求更新。
    pub fn update_for(
        &mut self,
        backend: FrameGenerationUiBackend,
        enabled: bool,
        camera: Option<Entity>,
        backend_ready: bool,
    ) {
        // 每帧固定字段赋值。
        if enabled {
            *self = Self {
                enabled,
                camera,
                backend_ready,
                backend: Some(backend),
            };
        }
        // 当前有效后端取得所有权。
        else if self.backend == Some(backend) {
            *self = Self::default();
        } // 只有当前所有者能释放请求。
    } // 更新结束。
} // 请求实现结束。
#[derive(Resource, Default)] // 只接受 Prepare 实际成功后的同帧授权。
pub struct UiFramePermission {
    identity: Option<(u32, Entity)>,
} // 帧号避免未调用 Prepare 时沿用旧许可。
impl UiFramePermission {
    // 后端主动授权，失败无需猜测旧状态。
    pub fn allow(&mut self, frame: u32, camera: Entity) {
        self.identity = Some((frame, camera));
    } // 完整准备成功。
    pub fn deny(&mut self) {
        self.identity = None;
    } // 显式撤销。
    pub fn allows(&self, frame: u32, camera: Entity) -> bool {
        self.identity == Some((frame, camera))
    } // 零号帧也有明确存在性。
} // 许可实现结束。
#[derive(Resource, Default)] // 持有最近完整 GPU 图像，裸句柄消费期限由后端负责。
pub struct UiCaptureResult {
    // 单槽有界结果，不积累历史地图。
    pub(crate) image: Option<UiFrame>, // 捕获完成前不会发布。
    pub(crate) scene_only: Option<(u32, Entity)>, // 仅后端成功接管真实帧合成时允许省略恢复。
} // 捕获资源结束。
impl UiCaptureResult {
    // 身份检查集中，后端不直接修改普通帧所有权。
    pub fn current(&self, frame: u32, camera: Entity) -> Option<&UiFrame> {
        self.image
            .as_ref()
            .filter(|image| image.frame == frame && image.camera == camera)
    } // 拒绝旧帧和辅助相机。
    pub fn keep_scene_only(&mut self, frame: u32, camera: Entity) -> bool {
        // FSR 成功登记后才调用。
        if self.current(frame, camera).is_none() {
            return false;
        } // 没有完整本帧图像不得省略恢复。
        self.scene_only = Some((frame, camera));
        true // 准确绑定同一次捕获。
    } // 场景模式授权结束。
} // 结果实现结束。

#[cfg(test)] // 无GPU回归：身份和后端切换不能复活旧许可。
mod tests {
    // 不构造虚假GPU资源。
    use super::*; // 测试真实请求状态转换。
    #[test] // inactive后端不得撤销active后端，相反调用顺序也必须相同。
    fn inactive_backend_cannot_clear_another_owner() {
        // 两插件同时编译时的请求竞争回归。
        let mut world = World::new();
        let camera = world.spawn_empty().id(); // 有效主世界实体身份。
        let mut request = FrameGenerationUiRequest::default(); // 初始关闭。
        request.update_for(FrameGenerationUiBackend::Dlss, true, Some(camera), true); // DLSS取得UI所有权。
        request.update_for(FrameGenerationUiBackend::Fsr3, false, None, false); // FSR关闭不得清掉DLSS。
        assert!(request.enabled && request.camera == Some(camera)); // 保留当前活跃请求。
        request.update_for(FrameGenerationUiBackend::Fsr3, true, Some(camera), true); // 显式切换到FSR。
        request.update_for(FrameGenerationUiBackend::Dlss, false, None, false); // 旧DLSS释放不覆盖新FSR。
        assert!(request.enabled && request.backend == Some(FrameGenerationUiBackend::Fsr3)); // 所有权属于新后端。
        request.update_for(FrameGenerationUiBackend::Fsr3, false, None, false); // 当前所有者退出。
        assert!(!request.enabled && request.camera.is_none()); // 不留过期相机。
    } // 后端所有权回归结束。
    #[test] // 零号帧、相机销毁换代与失败必须拒绝旧授权。
    fn permission_is_exact_frame_and_camera() {
        // 避免prepared布尔沿用到另一个相机。
        let mut world = World::new();
        let camera = world.spawn_empty().id(); // 第一代玩法相机。
        let mut permission = UiFramePermission::default(); // 没有权限。
        assert!(!permission.allows(0, camera)); // 零号帧不会误当默认成功。
        permission.allow(0, camera);
        assert!(permission.allows(0, camera)); // 本帧完整授权。
        assert!(!permission.allows(1, camera)); // 未Prepare下一帧自动无权。
        world.despawn(camera);
        let replacement = world.spawn_empty().id(); // Entity代次发生变化。
        assert!(!permission.allows(0, replacement)); // 不能复活旧相机事务。
        permission.deny();
        assert!(!permission.allows(0, camera)); // 显式失败撤销。
    } // 同帧授权回归结束。
    #[test] // 没有完整图像时不允许后端夺走普通真实帧恢复责任。
    fn scene_only_requires_complete_capture() {
        // 失败托底回归。
        let camera = World::new().spawn_empty().id(); // 任意有效相机身份。
        let mut capture = UiCaptureResult::default(); // 尚无GPU捕获事实。
        assert!(!capture.keep_scene_only(0, camera)); // 不能凭请求或默认帧号省略UI。
        assert!(capture.scene_only.is_none()); // 保持默认完整真实帧恢复。
    } // 默认回退回归结束。
} // 无窗口事务回归结束。
