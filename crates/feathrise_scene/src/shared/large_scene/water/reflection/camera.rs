//! 水面SSR只授权当前唯一透视视图，并通过共享画质租约取得HDR与深度，不直接改相机附件。

mod components; // RenderApp提取标记。
mod resources; // 唯一租约和场景身份。
#[cfg(test)] // 无窗口验证资格与所有权交接。
mod tests; // 质量、F12投影、切换场景和共享租约。

use bevy::pbr::ScreenSpaceTransmission; // 尊重用户关闭屏幕传输的steps=0配置。
use bevy::prelude::*; // 相机查询和主世界调度。
use bevy::render::extract_component::ExtractComponentPlugin; // 标记同步撤销时清理渲染世界旧资格。
use feathrise_atmosphere::weather::{WeatherAuthoringPreview, WeatherPreviewCamera}; // 与天气采用同一个F12锚点选择。
use feathrise_scene_core::view::PrimarySceneView; // 普通辅助RTT无反射资格。
use feathrise_scene_content::ActiveDynamicScene; // 同DynamicSceneRoot下保留真实场景身份。
use feathrise_settings::{
    EffectsQuality, RenderSettings, GraphicsRuntimeLeaseRegistry, GraphicsRuntimeLeaseSystems,
    GraphicsRuntimeRequirements,
}; // 共用深度附件所有权，兼容SSAO/TAA/RTX。
use feathrise_scene_core::state::ScenePhase; // 非玩法场景撤销旧输入。
pub(super) use components::WaterReflectionView;
use resources::WaterReflectionCameraState; // 唯一票据缓存。 // 渲染准备与捕获使用这一资格标记。

pub(super) fn install(app: &mut App) {
    // 插件由水面反射入口安装一次。
    // Bevy 0.20 将透射从 Camera3d 默认字段改为显式组件。保留玩法与作者视图的原默认能力。
    app.register_required_components::<PrimarySceneView, ScreenSpaceTransmission>()
        .register_required_components::<WeatherPreviewCamera, ScreenSpaceTransmission>()
        .init_resource::<WaterReflectionCameraState>() // 默认无目标和租约。
        .init_resource::<GraphicsRuntimeLeaseRegistry>() // 正式设置插件已初始化，独立测试也可安全使用。
        .add_plugins(ExtractComponentPlugin::<WaterReflectionView>::default()) // 仅提取显式资格，不复制相机配置。
        .add_systems(
            PostUpdate,
            maintain.before(GraphicsRuntimeLeaseSystems::RestoreReleased),
        ); // 租约在恢复/普通设置/最终合并前维护。
} // 完成主世界安装。

#[allow(clippy::too_many_arguments, clippy::type_complexity)] // 每类输入分别表达场景、作者锚点、相机与所有权。
fn maintain(
    // 稳定帧仅检查少量相机，不反复插入标记或刷新租约。
    mut commands: Commands,               // 只插拔自己的WaterReflectionView。
    scene: Option<Res<State<ScenePhase>>>, // 最小宿主缺失场景时回退。
    dynamic: Option<Res<ActiveDynamicScene>>, // 动态场景没有真实ID时禁止捕获。
    settings: Option<Res<RenderSettings>>,  // 缺设置按未启用回退。
    preview: Option<Res<WeatherAuthoringPreview>>, // F12打开时不同时复制后台玩法视图。
    cameras: Query<
        (
            Entity,
            &Camera,
            &Projection,
            &ScreenSpaceTransmission,
            Has<WeatherPreviewCamera>,
            Has<PrimarySceneView>,
        ),
        With<Camera3d>,
    >, // 不带正式相机附件的辅助实体不会进入候选。
    mut leases: ResMut<GraphicsRuntimeLeaseRegistry>, // 可与RTX/SSAO等要求按或合并。
    mut state: ResMut<WaterReflectionCameraState>, // 始终最多一份本功能租约。
) {
    // 不读取或修改DepthPrepass现状，避免抢走其他功能所有权。
    let context = scene.as_deref().and_then(|scene| match **scene {
        // 菜单/加载阶段没有有效场景来源。
        ScenePhase::Active => dynamic
            .as_ref()
            .and_then(|dynamic| dynamic.scene_id.clone())
            .map(|id| (**scene, Some(id))), // 动态真实ID参与换代。
        _ => None, // 退出场景时立即撤销。
    }); // 完成当前身份。
    let changed_scene = state.context.is_some() && state.context != context; // 切换场景同实体必须空一帧丢弃旧捕获资格。
    if state.context != context {
        state.context = context.clone();
    } // 稳定场景不重复写状态。
    let preview_active = preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 缺F12资源时保持玩法锚点。
    let enabled = !changed_scene
        && context.is_some()
        && settings
            .as_deref()
            .is_some_and(|settings| settings.graphics.effects_quality != EffectsQuality::Low); // 中高极高启用，低档不支付色深捕获。
    let mut selected = None; // 任意重复合法锚点都保守关闭，不能复制两份。
    if enabled {
        // 没有场景或低档时不查询视图候选。
        for (entity, camera, projection, transmission, is_preview, is_gameplay) in &cameras {
            // 相机数量固定小规模。
            if !camera.is_active
                || !matches!(projection, Projection::Perspective(_))
                || transmission.steps == 0
                || is_preview != preview_active
                || (!is_preview && !is_gameplay)
            {
                continue;
            } // 排除正交、用户关闭及非当前锚点。
            if selected.is_some() {
                selected = None;
                break;
            } // 不静默挑重复视图。
            selected = Some(entity); // 唯一当前透视目标。
        } // 完成候选检查。
    } // 完成资格门。
    if state.lease.as_ref().map(|(entity, _)| *entity) == selected {
        return;
    } // 稳定帧零组件命令和零租约刷新。
    if let Some((entity, lease)) = state.lease.take() {
        // 相机切换或禁用先归还旧资格。
        leases.release(lease); // 只释放自己持有的票据。
        commands.entity(entity).try_remove::<WaterReflectionView>(); // 已despawn相机也安全处理，不碰任何其他附件。
    } // 完成旧视图撤销。
    if let Some(entity) = selected {
        // 新目标取得最小深度要求。
        let mut requirements = GraphicsRuntimeRequirements::new(entity); // 不申请MSAA/官方SSR/Solari等其他功能。
        requirements.require_depth_prepass = true; // 水面射线只需同视图不透明深度。
        requirements.require_hdr = true; // 捕获与水PBR必须同属线性HDR，避免F12非HDR颜色被重复色调映射。
        let lease = leases.acquire(requirements); // 与设置/光追租约统一合并。
        commands.entity(entity).insert(WaterReflectionView); // 同帧RenderApp只看到新的唯一资格。
        state.lease = Some((entity, lease)); // 保存准确可撤销票据。
    } // 完成新视图接管。
} // 完成水面相机生命周期。
