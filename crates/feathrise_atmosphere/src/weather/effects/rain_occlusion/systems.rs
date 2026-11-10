//! 唯一雨向相机生命周期：固定局部窗口、变化刷新、场景身份隔离及低频缓存重试。
use super::{
    components::RainOcclusionCamera,
    constants::RESOLUTION,
    helpers,
    resources::{CaptureState, RainOcclusionMap},
};
use crate::{WeatherAuthoringPreview, WeatherOutput, WeatherPreviewCamera}; // 同源天气输出和作者相机。
use bevy::asset::RenderAssetUsages; // 目标仅GPU持有，不生成CPU像素。
use bevy::camera::{RenderTarget, visibility::RenderLayers}; // 独立无颜色相机。
use bevy::core_pipeline::prepass::{DeferredPrepass, DepthPrepass}; // 两类源材质均需合法队列，F12退出不能把Deferred源投到不存在的phase。
use bevy::image::ImageSampler; // 数值编码不能插值。
use bevy::prelude::*; // ECS、相机与资产。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages}; // 固定GPU目标。
use feathrise_scene_core::view::PrimarySceneView; // 玩法天气锚点。
use feathrise_scene_core::constants::render_layers::RAIN_OCCLUSION_LAYER; // 标准世界模型专用层。
use feathrise_scene_content::ActiveDynamicScene; // 同枚举切换场景的真实身份。
use feathrise_scene_core::state::ScenePhase; // 非玩法禁止旧图继续作用。 // 捕获需求与固定数学。

pub(super) fn initialize(mut commands: Commands) {
    // 启动只创建一个禁用相机，降雨时才分配目标。
    commands.spawn((
        // 专用相机不安装玩家、天气预览、UI或后处理标记。
        Name::new("RainOcclusionDepthCamera"), // 调试身份。
        RainOcclusionCamera,                   // GPU转换只处理此相机。
        Camera3d {
            depth_texture_usages: (TextureUsages::RENDER_ATTACHMENT
                | TextureUsages::TEXTURE_BINDING)
                .into(),
            ..default()
        }, // 转换pass需要直接读取深度。
        Camera {
            is_active: false,
            order: -110,
            ..default()
        }, // 先于玩法和F12视图捕获。
        RenderTarget::None {
            size: UVec2::splat(RESOLUTION),
        }, // 不运行颜色绘制。
        Projection::Orthographic(helpers::projection()), // 深度沿雨向而非玩家视线。
        DepthPrepass,                          // 实际材质透明裁切复用Bevy标准流程。
        DeferredPrepass, // F12与玩法可切换源材质Forward/Deferred；独立捕获同时具备两类预通道，仍无主颜色目标。
        Msaa::Off,       // 单采样保持每像素唯一遮挡面。
        RenderLayers::layer(RAIN_OCCLUSION_LAYER), // 不包含地形、草地、云、水和UI。
        Transform::default(), // 第一次有效需求同帧设置世界变换。
    )); // 仅此一个额外捕获视图。
} // 初始化结束。

fn new_target(images: &mut Assets<Image>) -> Handle<Image> {
    // 每个场景/降雨会话只分配一次固定图片。
    let mut image = Image::new_uninit(
        // wgpu保证首次读取未写像素为零，A=0表示无效。
        Extent3d {
            width: RESOLUTION,
            height: RESOLUTION + 1,
            depth_or_array_layers: 1,
        }, // 首行固定矩阵元数据。
        TextureDimension::D2,            // 单张局部二维图。
        TextureFormat::Rgba16Float,      // 可绑定Hanabi，整数拆位保持完整f32精度。
        RenderAssetUsages::RENDER_WORLD, // 无CPU像素副本。
    ); // 图片描述结束。
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST; // 转换写入、雨丝读取。
    image.sampler = ImageSampler::nearest(); // 禁止对编码位做线性过滤。
    images.add(image) // 新身份杜绝跨场景使用旧像素。
} // 目标分配结束。

#[allow(clippy::too_many_arguments, clippy::type_complexity)] // 天气、资产变化和相机所有权分别建模。
pub(super) fn update(
    // 变换传播后读取源，同帧手动写独立相机世界变换。
    output: Res<WeatherOutput>,                    // 与雨滴同源风速。
    scene: Option<Res<State<ScenePhase>>>,          // 无场景的独立测试可运行。
    dynamic: Option<Res<ActiveDynamicScene>>,        // 真实动态场景身份。
    preview: Option<Res<WeatherAuthoringPreview>>, // F12选择与雨丝一致。
    time: Option<Res<Time<Real>>>,                 // 暂停时仍允许缓存重试。
    anchors: Query<
        (
            &Camera,
            &GlobalTransform,
            &Projection,
            Has<WeatherPreviewCamera>,
        ),
        (
            Or<(With<PrimarySceneView>, With<WeatherPreviewCamera>)>,
            Without<RainOcclusionCamera>,
        ),
    >, // 只读少量合格天气相机。
    mut cameras: Query<
        (&mut Camera, &mut Transform, &mut GlobalTransform),
        With<RainOcclusionCamera>,
    >, // 唯一捕获视图。
    changed: Query<
        (),
        (
            With<super::casters::OwnedRainLayer>,
            Without<RainOcclusionCamera>,
            Or<(
                Changed<GlobalTransform>,
                Changed<Mesh3d>,
                Changed<MeshMaterial3d<StandardMaterial>>,
                Changed<InheritedVisibility>,
                Changed<RenderLayers>,
            )>,
        ),
    >, // 源与可写相机显式互斥，Changed变更检查也读取组件访问权限。
    mut removed: RemovedComponents<MeshMaterial3d<StandardMaterial>>, // 删除屋顶立即刷新。
    mut removed_casters: RemovedComponents<super::casters::OwnedRainLayer>, // 改规划归属或移到预览层时立即撤销缓存遮挡。
    mut removed_meshes: RemovedComponents<Mesh3d>, // 单独撤销网格也不能残留旧屋顶。
    mut mesh_events: MessageReader<AssetEvent<Mesh>>, // 热重载同一Mesh句柄仍需刷新。
    mut material_events: MessageReader<AssetEvent<StandardMaterial>>, // 同一材质透明度变化立即刷新。
    mut images: ResMut<Assets<Image>>,                                // 只在目标身份改变时分配。
    mut map: ResMut<RainOcclusionMap>, // 同帧发布捕获矩阵和图像身份。
    mut state: ResMut<CaptureState>,   // 缓存不随场景实体数量增长。
) {
    // 捕获同步开始。
    let removed_source = removed.read().next().is_some()
        || removed_casters.read().next().is_some()
        || removed_meshes.read().next().is_some(); // 源删除与资格撤销合并为一次重画。
    removed.clear(); // 消费余下事件，不逐项构建工作列表。
    removed_casters.clear(); // 不保留资格撤销事件积压。
    removed_meshes.clear(); // 固定状态无需逐项建立列表。
    let asset_changed =
        mesh_events.read().next().is_some() || material_events.read().next().is_some(); // 最多检查两类事件头。
    mesh_events.clear(); // 本帧变化合并为一次重画。
    material_events.clear(); // 材质热更新同样合并。
    let context = match scene.as_deref().map(State::get) {
        // 读取最新正式场景身份。
        Some(ScenePhase::Active) => dynamic
            .as_ref()
            .and_then(|map| map.scene_id.clone())
            .map(|id| (ScenePhase::Active, Some(id))), // 清单未采用不捕获。
        None => Some((ScenePhase::Active, None)), // 最小无状态测试沿用玩法资格。
        _ => None, // 菜单和加载撤销旧图。
    }; // 场景判定结束。
    let context_changed = state.context != context; // 同场景枚举换场景也失效。
    if context_changed {
        state.context = context.clone();
        state.target = None;
    } // 新图不能继承旧像素。
    let preview_active = preview
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 与雨幕唯一相机一致。
    let mut candidates = anchors.iter().filter(|(camera, _, projection, authoring)| {
        camera.is_active
            && *authoring == preview_active
            && super::super::supports_camera_local_weather(*authoring, Some(*projection))
    }); // 无普通辅助相机。
    let selected = candidates.next().filter(|_| candidates.next().is_none()); // 重复锚点明确无效，不随机挑选。
    let desired = selected
        .filter(|_| {
            context.is_some()
                && !state.casters_pending
                && output.precipitation.is_finite()
                && output.precipitation > 0.01
        })
        .and_then(|(camera, global, _, _)| {
            // 层级资格批次必须完成，不发布半份屋顶图。
            let center = super::super::rain::rain_emitter_position(&global.compute_transform())
                - Vec3::Y * 18.0; // 与雨盘前移中心同源，移回地面附近。
            helpers::capture_view(
                center,
                super::super::rain::rain_velocity(output.wind, output.wind_direction),
            )
            .map(|(transform, matrix)| (transform, matrix, camera.order.saturating_sub(2))) // 同源斜风雨。
        }); // 得到完整捕获需求。
    let Ok((mut camera, mut transform, mut global)) = cameras.single_mut() else {
        if map.active || map.image != Handle::default() {
            *map = RainOcclusionMap::default();
        }
        state.target = None;
        state.warm_frames = 0;
        return;
    }; // 捕获相机自身丢失也结束旧会话。
    let Some((desired_transform, matrix, order)) = desired else {
        // 停雨或离开玩法。
        if camera.is_active {
            camera.is_active = false;
        } // 不再提交深度绘制。
        if map.active || map.image != Handle::default() {
            *map = RainOcclusionMap::default();
        } // 1像素默认图使Hanabi本帧停止读取旧屋顶。
        state.target = None; // 下次降雨重新捕获，避免长期旧图。
        state.warm_frames = 0; // 重置启动重试。
        return; // 无GPU工作。
    }; // 有效需求分支。
    let fresh = state.target.is_none(); // 新图或新雨会话。
    if fresh {
        state.target = Some(new_target(&mut images));
    } // 无CPU像素生成。
    let target = state.target.as_ref().unwrap().clone(); // 已在上一行保证存在。
    let dirty = fresh
        || context_changed
        || matrix != map.world_to_rain
        || !changed.is_empty()
        || removed_source
        || asset_changed; // 稳定源不重画。
    if dirty {
        state.warm_frames = 8;
    } // 给异步网格/材质管线短暂连续重试。
    let now = time
        .as_deref()
        .map_or(state.last_capture + 0.5, Time::elapsed_secs_f64); // 无时钟测试确定性刷新。
    let redraw = dirty || state.warm_frames > 0 || now - state.last_capture >= 0.5; // 稳定场景每秒最多两次兜底刷新，迟到纹理不会永远缓存为空。
    state.warm_frames = state.warm_frames.saturating_sub(1); // 有限启动重试不永久强制绘制。
    if redraw {
        state.last_capture = now;
    } // 更新缓存年龄。
    if camera.is_active != redraw {
        camera.is_active = redraw;
    } // 缓存帧跳过整个额外预通道。
    if camera.order != order {
        camera.order = order;
    } // 生产严格先于消费相机。
    transform.set_if_neq(desired_transform); // 稳定帧不制造变更。
    global.set_if_neq(GlobalTransform::from(desired_transform)); // 传播已完成，当前帧立即更新视锥所需变换。
    if map.image != target || map.world_to_rain != matrix || !map.active {
        // 固定需求变化才触发提取。
        *map = RainOcclusionMap {
            image: target,
            world_to_rain: matrix,
            active: true,
        }; // 完整主世界需求，GPU随后原子输出矩阵与深度。
    } // 需求发布结束。
} // 捕获同步结束。
