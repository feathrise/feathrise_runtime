//! 唯一局部水高度相机，图像始终固定512²，只移动世界范围并显式发布有效性。
use super::{components::RainMaskCamera, resources::RainMaskCache};
use bevy::asset::RenderAssetUsages; // 数值图只在GPU保存，不分配CPU像素。
use bevy::camera::{Hdr, RenderTarget, ScalingMode, visibility::RenderLayers}; // 独立顶视目标和渲染层。
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping}; // 高度是数值数据。
use bevy::image::ImageSampler; // 最近采样不把岸外零覆盖插成水。
use bevy::prelude::*; // 相机、图片、变换与ECS接口。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages}; // 固定RGBA16F目标。
use feathrise_atmosphere::{WeatherAuthoringPreview, WeatherOutput, WeatherPreviewCamera}; // 与雨丝保持同一个降雨锚点。
use feathrise_scene_core::view::PrimarySceneView; // 玩法主相机身份。
use feathrise_scene_core::constants::render_layers::RAIN_WATER_MASK_LAYER; // 独占17号层，不进入主图或阴影。
use feathrise_environment::rain_water::RainWaterMask; // 消费者共享同帧图像与范围。
use feathrise_map_content::{ActiveDynamicMap, MapId}; // 同场景枚举切图也必须撤销旧图。
use feathrise_scene_core::state::ScenePhase; // 菜单/加载阶段停止捕获。 // 独立相机与代理完整性。

const RESOLUTION: u32 = 512; // 2MiB固定图像，无整地图分辨率增长。
const SPAN: f32 = feathrise_environment::rain_water::RAIN_WATER_SPAN; // 与雨丝片元共享跨度，雨盘及存活风漂移落在256m半径内。

pub(super) fn initialize(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mask: Res<RainWaterMask>,
) {
    // 启动时只创建一次GPU目标与禁用相机。
    let mut image = Image::new_uninit(
        Extent3d {
            width: RESOLUTION,
            height: RESOLUTION,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        TextureFormat::Rgba16Float,
        RenderAssetUsages::RENDER_WORLD,
    ); // 不分配或上传CPU像素，首次有效帧由相机清零。
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_DST; // 作为离屏目标并给Hanabi采样。
    image.sampler = ImageSampler::nearest(); // coverage与高低位数值必须最近采样。
    images
        .insert(mask.image.id(), image)
        .expect("固定UUID雨水高度图身份有效"); // 固定全局身份供场景与天气共享。
    commands.spawn((
        Name::new("RainWaterHeightCamera"), // 调试身份明确，不会匹配玩法相机。
        RainMaskCamera,                     // 唯一捕获相机标记。
        Camera3d::default(),                // 独立Opaque深度自动保留最高实际水面。
        Camera {
            is_active: false,
            order: -100,
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        }, // 等天气资格后启用，始终早于正常相机。
        RenderTarget::Image(mask.image.clone().into()), // 固定共享图像作为最终目标。
        Projection::Orthographic(OrthographicProjection {
            near: 0.1,
            far: 4096.0,
            scaling_mode: ScalingMode::Fixed {
                width: SPAN,
                height: SPAN,
            },
            ..OrthographicProjection::default_3d()
        }), // 范围固定，无需随窗口尺寸重分配。
        Transform::default(),               // 首次有效帧统一写入位置和GlobalTransform。
        Hdr,                                // 保留负高度与分段高位，不能压到0至1。
        Msaa::Off,                          // 一像素一份真实最高水面，不对数值图做MSAA混合。
        Tonemapping::None,                  // 不改变高度编码数值。
        DebandDither::Disabled,             // 关闭颜色抖动，Bevy 0.20 不再对数值目标逐帧告警。
        RenderLayers::layer(RAIN_WATER_MASK_LAYER), // 只画水体代理，不画天空、建筑或普通地形。
    )); // 相机不安装天空、Bloom、天气跟随或UI标记。
} // 完成固定目标初始化。

#[allow(clippy::too_many_arguments)] // 天气、地图、锚点与输出分别保持明确身份。
pub(super) fn update(
    output: Option<Res<WeatherOutput>>,     // 缺天气输出即没有降雨。
    scene: Option<Res<State<ScenePhase>>>,   // 没有正式场景身份不能采样旧水。
    dynamic: Option<Res<ActiveDynamicMap>>, // 动态地图的真实身份。
    preview: Option<Res<WeatherAuthoringPreview>>, // 当前F12天气锚点选择。
    anchors: Query<
        (
            &Camera,
            &GlobalTransform,
            &Projection,
            Has<WeatherPreviewCamera>,
            Has<PrimarySceneView>,
        ),
        Without<RainMaskCamera>,
    >, // 与可写捕获相机显式互斥。
    mut cameras: Query<(&mut Camera, &mut Transform, &mut GlobalTransform), With<RainMaskCamera>>, // 同帧手动发布顶视变换。
    cache: Res<RainMaskCache>,       // 代理批次未完整时明确禁用旧图。
    mut mask: ResMut<RainWaterMask>, // 消费者只读取这个完整范围与active。
    mut previous_context: Local<Option<(ScenePhase, Option<MapId>)>>, // 同枚举切图有一帧无效门。
) {
    // 调度位于水参数更新后、相机和可见性更新前。
    let context = scene.as_deref().and_then(|scene| match **scene {
        ScenePhase::Active => dynamic
            .as_ref()
            .and_then(|map| map.map_id.clone())
            .map(|id| (**scene, Some(id))),
        _ => None,
    }); // 仅正式玩法地图具有捕获资格。
    let changed_map = previous_context.is_some() && *previous_context != context; // 新图不继承旧图位置和像素信用。
    *previous_context = context.clone(); // 小型地图身份不会复制资源数据。
    let preview_active = preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 与雨丝同源作者/玩法相机选择。
    let rainy = output
        .as_ref()
        .is_some_and(|output| output.precipitation.is_finite() && output.precipitation > 0.01); // 雨停立即禁用，水面本身不受影响。
    let mut selected = None; // 任何重复合法锚点都保守退出。
    if rainy
        && context.is_some()
        && !changed_map
        && cache.queued.is_empty()
        && cache.clipmap_queued.is_empty()
        && (!cache.proxies.is_empty() || !cache.clipmap_proxies.is_empty())
    {
        // 无水不运行空GPU通道；未完成代理或换图帧不能发布半批几何图。
        for (camera, transform, projection, is_preview, is_main) in &anchors {
            // 仅检查少量相机，不读取场景Mesh。
            if !camera.is_active
                || is_preview != preview_active
                || (!is_preview && !is_main)
                || !matches!(projection, Projection::Perspective(_))
            {
                continue;
            } // 与雨丝相同的透视锚点资格。
            if selected.is_some() {
                selected = None;
                break;
            } // 重复相机不随机选择。
            selected = Some((transform.translation(), camera.order)); // 发布真实最新世界坐标和绘制顺序。
        } // 完成唯一锚点选择。
    } // 完成天气和地图守卫。
    let selected = selected.filter(|(point, _)| point.is_finite()); // 坏变换不能污染相机和采样范围。
    let Ok((mut camera, mut transform, mut global)) = cameras.single_mut() else {
        mask.active = false;
        return;
    }; // 相机缺失时消费者明确失败关闭采样。
    if camera.is_active != selected.is_some() {
        camera.is_active = selected.is_some();
    } // 只在启停变化时标记相机改变。
    if let Some((anchor, order)) = selected {
        // 同帧对齐捕获相机与Hanabi世界XZ范围。
        let center = Vec2::new(anchor.x.floor(), anchor.z.floor()); // 一米网格对齐减少亚像素范围抖动。
        let position = Vec3::new(center.x, anchor.y + 2048.0, center.y); // 覆盖锚点上下约2048米，远超雨丝存活高度范围。
        let desired = Transform::from_translation(position).looking_to(Vec3::NEG_Y, Vec3::NEG_Z); // 图像顶边对应世界负Z，UV=(XZ-min)/size。
        transform.set_if_neq(desired); // 相机不挂父节点，世界与局部变换一致。
        global.set_if_neq(GlobalTransform::from(desired)); // TransformPropagate已完成，显式供本帧提取/Frustum读取。
        let desired_order = order.saturating_sub(1); // 必须先画高度图，再绘制消费者雨丝。
        if camera.order != desired_order {
            camera.order = desired_order;
        } // 正常帧不重复改相机顺序。
        let bounds = Vec4::new(center.x - SPAN * 0.5, center.y - SPAN * 0.5, SPAN, SPAN); // RGBA图与本帧捕获范围原子对应。
        if mask.bounds != bounds {
            mask.bounds = bounds;
        } // 稳定锚点零属性范围更新。
    } // 完成完整相机与范围发布。
    if mask.active != selected.is_some() {
        mask.active = selected.is_some();
    } // 雨停、切图、无相机同帧取消采样旧图。
} // 完成局部高度图资格更新。
