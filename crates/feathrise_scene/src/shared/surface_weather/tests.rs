//! 无窗口场景桥回归：身份失效、作者空图、完整纹理移交、热更回退和暂停变更检测。
use super::{resources::PuddleRuntimeState, systems}; // 直接运行生产桥系统。
use bevy::{
    asset::{AssetPlugin, RenderAssetUsages},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
}; // 最小资产宿主不创建GPU设备。
use feathrise_atmosphere::WeatherOutput; // 固定天气输出模拟暂停帧。
use feathrise_environment::terrain_surface::{SurfacePuddleMask, SurfaceWeatherGpu}; // 检查正式发布资源。
use feathrise_map_content::MapPuddleSaveHandoff;
use feathrise_map_content::{
    ActiveDynamicMap, MapContentHandles, MapContentRegistry, MapId, MapManifestAsset,
    MapPuddleDescriptor, MapPuddleLiveOverride,
}; // 真实清单协议与作者覆盖。
use feathrise_scene_core::state::ScenePhase; // 同场景枚举中切换真实地图。 // 保存回执必须由真实内容验证后清除。

#[derive(Resource, Default)] // 记录真正消费系统看见的变化，避免依赖App帧尾清理时机。
struct ChangeCounts(u32, u32); // 分别记录遮罩换版与天气GPU发布次数。

fn count_changes(
    mask: Res<SurfacePuddleMask>,
    gpu: Res<SurfaceWeatherGpu>,
    mut counts: ResMut<ChangeCounts>,
) {
    // 与上传消费者使用相同变更检测语义。
    counts.0 += u32::from(mask.is_changed()); // 统计实际遮罩变更帧。
    counts.1 += u32::from(gpu.is_changed()); // 统计实际GPU数据变更帧。
} // 结束消费者侧变更记录。

#[test] // P4切档只更新同一个64字节GPU参数，不影响积水历史或触发纹理换版。
fn surface_quality_bridge_matches_cpu_budget_without_changing_weather() {
    // 运行真正发布系统，避免只验证静态预算表。
    use feathrise_environment::terrain_surface::surface_effects_quality_profile;
    use feathrise_settings::{EffectsQuality, RenderSettings}; // 使用正式持久设置枚举。 // CPU水花读取的唯一预算函数。
    let (mut app, _, _) = app(); // 最小完整地图与表面天气桥。
    app.init_resource::<RenderSettings>(); // 模拟用户正常切换特效质量。
    {
        let mut output = app.world_mut().resource_mut::<WeatherOutput>();
        output.surface_wetness = 0.8;
        output.puddle_amount = 0.6;
        output.precipitation = 0.5;
    } // 固定历史和雨势，不让切档伪造天气。
    for (index, tier) in [
        EffectsQuality::Low,
        EffectsQuality::Medium,
        EffectsQuality::High,
        EffectsQuality::Ultra,
    ]
    .into_iter()
    .enumerate()
    {
        // 覆盖所有运行时档位。
        app.world_mut()
            .resource_mut::<RenderSettings>()
            .graphics
            .effects_quality = tier; // 同正式设置入口修改数据。
        app.update(); // 经过真正的同帧发布桥。
        let gpu = *app.world().resource::<SurfaceWeatherGpu>(); // 读取Shader将消费的完整数据。
        let profile = surface_effects_quality_profile(index as f32); // 水滴池使用相同输入。
        assert_eq!(gpu.options.z, profile.rain_range); // 距离不能存在CPU/GPU两套档位。
        assert_eq!(gpu.options.w, profile.rain_density); // 密度必须来自同一表。
        assert_eq!(gpu.weather, Vec4::new(0.8, 0.6, 0.5, 0.0)); // 水量和湿润度不能随质量变动。
        let uploads = app.world().resource::<ChangeCounts>().1; // 当前真实发布次数。
        app.update(); // 不再修改任何输入的暂停稳定帧。
        assert_eq!(app.world().resource::<ChangeCounts>().1, uploads); // 档位稳定后不重复上传。
    } // 完成四档同源接线回归。
} // 结束质量与历史隔离测试。

fn descriptor() -> MapPuddleDescriptor {
    // 小尺寸独立作者图规格。
    MapPuddleDescriptor {
        source_path: "maps/HN-01/puddles/potential.r8".into(),
        texture_path: "maps/HN-01/puddles/potential.ktx2".into(),
        resolution: [2, 2],
    } // 测试私有合法路径。
} // 结束规格构造。

fn image(size: u32, format: TextureFormat) -> Image {
    // 元数据错误必须在正式桥被拦截。
    let pixel = if format == TextureFormat::R8Unorm {
        vec![127]
    } else {
        vec![127, 127, 127, 255]
    }; // 为每种测试格式提供合法像素大小。
    Image::new_fill(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &pixel,
        format,
        RenderAssetUsages::default(),
    ) // 保留CPU数据用于检查零复制移交。
} // 结束图片构造。

fn app() -> (App, Handle<Image>, Handle<MapManifestAsset>) {
    // 建立只含单一地图与预加载候选的最小生产宿主。
    let mut app = App::new(); // 不使用窗口和RenderApp。
    app.add_plugins((MinimalPlugins, AssetPlugin::default())); // 初始化后台资产所需任务池和AssetServer。
    app.init_asset::<Image>().init_asset::<MapManifestAsset>(); // 使用真实强句柄与资产事件生命周期。
    app.insert_resource(State::new(ScenePhase::Active)); // 同一场景枚举承载不同真实地图。
    let map = MapId("HN-01".into()); // 当前地图身份。
    app.insert_resource(ActiveDynamicMap {
        map_id: Some(map.clone()),
    }); // 指向首张地图。
    let manifest: MapManifestAsset = serde_json::from_value(
        serde_json::json!({ // 使用公开Serde构造完整清单，避免依赖磁盘资产。
            "schema_version": 1, "id": "HN-01", "display_name": "测试", "runtime_scene": "DynamicMapScene", // 正式地图身份。
            "world_min": [0.0, 0.0], "world_max": [10.0, 10.0], "default_spawn": [0.0, 0.0, 0.0], // 首尾端点范围。
            "terrain": { "kind": "gltf", "glb_path": "maps/HN-01/terrain.glb", "resolution": [2, 2], "base_height": 0.0, "height_scale": 1.0 }, // 仅满足真实协议的地形规格。
            "puddles": descriptor() // 正式积水描述。
        }),
    )
    .expect("测试清单"); // 解析失败应直接报告夹具错误。
    let manifest = app
        .world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .add(manifest); // 登记完整清单。
    let handles = MapContentHandles {
        characters_path: None,
        manifest: manifest.clone(),
        portals: default(),
        placements: default(),
        surface: None,
        vegetation: None,
        water: None,
        boundaries: None,
    }; // 桥只消费清单句柄。
    app.insert_resource(MapContentRegistry {
        catalog: default(),
        revision: 1,
        maps: [(map.clone(), handles)].into(),
        last_error: None,
    }); // 安装真实注册表。
    let source = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(image(2, TextureFormat::R8Unorm)); // 预置已经后台完成的合法源。
    app.insert_resource(PuddleRuntimeState {
        map_id: Some(map),
        requested: Some(descriptor()),
        source: Some(source.clone()),
        ..default()
    }); // 绕过磁盘IO，直接测试正式采用分支。
    app.init_resource::<MapPuddleLiveOverride>()
        .init_resource::<SurfacePuddleMask>()
        .init_resource::<SurfaceWeatherGpu>()
        .init_resource::<WeatherOutput>(); // 安装桥所需公共资源。
    app.init_resource::<MapPuddleSaveHandoff>(); // 未保存时没有交接请求。
    app.add_systems(Update, systems::resolve_puddle_mask); // 使用生产身份和纹理采用逻辑。
    app.add_systems(PostUpdate, systems::publish_surface_weather); // 使用生产变化检测和固定GPU数据合成。
    app.init_resource::<ChangeCounts>().add_systems(
        PostUpdate,
        count_changes.after(systems::publish_surface_weather),
    ); // 在真实发布之后记录消费者观察到的事件。
    (app, source, manifest) // 返回可构造热更的稳定句柄。
} // 结束最小宿主构造。

#[test] // 一次采用不复制像素，错误热更也不能改写旧独立图像。
fn image_transfer_keeps_allocation_and_rejects_bad_hot_reload() {
    // 覆盖两类最常见文件损坏。
    let (mut app, source, _) = app(); // 获取真实源资产。
    let pointer = app
        .world()
        .resource::<Assets<Image>>()
        .get(&source)
        .unwrap()
        .data
        .as_ref()
        .unwrap()
        .as_ptr() as usize; // 记录原始像素分配地址。
    app.update(); // 正式桥移交源资产。
    let adopted = app
        .world()
        .resource::<SurfacePuddleMask>()
        .image
        .clone()
        .unwrap(); // 取得独立活动身份。
    assert_ne!(adopted.id(), source.id()); // 源热更不能直接篡改活动纹理。
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&source)
            .is_none()
    ); // 源像素已被完整移动。
    assert_eq!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&adopted)
            .unwrap()
            .data
            .as_ref()
            .unwrap()
            .as_ptr() as usize,
        pointer
    ); // 大图像素没有深复制。
    for bad in [
        image(2, TextureFormat::Rgba8Unorm),
        image(3, TextureFormat::R8Unorm),
    ] {
        // 分别模拟错格式和错尺寸热更。
        app.world_mut()
            .resource_mut::<Assets<Image>>()
            .insert(source.id(), bad)
            .expect("源热更"); // 热更只写原源身份。
        app.update(); // 正式元数据门拒绝坏候选。
        assert_eq!(
            app.world().resource::<SurfacePuddleMask>().image.as_ref(),
            Some(&adopted)
        ); // 活动图保持旧完整版本。
        assert_eq!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&adopted)
                .unwrap()
                .data
                .as_ref()
                .unwrap()
                .as_ptr() as usize,
            pointer
        ); // 活动像素仍未被覆盖或复制。
    } // 完成错误候选回归。
} // 结束独立移交测试。

#[test] // 作者Some身份加None图像必须明确清空，跨图不能残留上张地图遮罩。
fn explicit_empty_live_override_and_map_change_clear_mask() {
    // 覆盖作者空图和动态地图身份失效。
    let (mut app, _, _) = app(); // 建立合法地图图像。
    app.update(); // 首次采用磁盘版。
    app.insert_resource(MapPuddleLiveOverride {
        map_id: Some(MapId("HN-01".into())),
        mask: None,
        world_min: Vec2::ZERO,
        world_max: Vec2::splat(10.0),
        ..default()
    }); // 作者显式擦成空图。
    app.update(); // 作者权威覆盖磁盘图。
    assert!(app.world().resource::<SurfacePuddleMask>().image.is_none()); // 不得回退到旧磁盘积水。
    app.insert_resource(MapPuddleLiveOverride::default()); // 撤销作者覆盖。
    app.update(); // 同图恢复完整磁盘版。
    assert!(app.world().resource::<SurfacePuddleMask>().image.is_some()); // 磁盘活动版本仍然有效。
    app.world_mut().resource_mut::<ActiveDynamicMap>().map_id = Some(MapId("HN-02".into())); // 同枚举切到另一个地图。
    app.update(); // 新地图清单尚未到达。
    assert!(app.world().resource::<SurfacePuddleMask>().image.is_none()); // 旧图不得跨地图显示。
    assert!(
        app.world()
            .resource::<PuddleRuntimeState>()
            .file_mask
            .is_none()
    ); // 旧文件身份同时失效。
} // 结束作者与地图边界测试。

#[test] // 仅地图范围热更应及时采用，暂停稳定帧不能虚假标记共享资源变化。
fn bounds_only_hot_reload_and_stable_publish_are_correct() {
    // 检查元数据独立修订和GPU上传门。
    let (mut app, _, manifest) = app(); // 初始化完整候选。
    app.update(); // 完成图像移交。
    let adopted = app.world().resource::<SurfacePuddleMask>().image.clone(); // 保存活动句柄用于验证不重建像素。
    app.world_mut()
        .resource_mut::<Assets<MapManifestAsset>>()
        .get_mut(&manifest)
        .unwrap()
        .world_max = [20.0, 30.0]; // 只修改世界端点。
    app.update(); // 范围与原合法图同次重新发布。
    assert_eq!(
        app.world().resource::<SurfaceWeatherGpu>().bounds,
        Vec4::new(0.0, 0.0, 20.0, 30.0)
    ); // 不得永远沿用旧范围。
    assert_eq!(app.world().resource::<SurfacePuddleMask>().image, adopted); // 仅范围改变无需重新分配图片。
    let counts = app.world().resource::<ChangeCounts>(); // 记录上一帧实际发布总次数。
    let before = (counts.0, counts.1); // 释放资源借用后再推进下一帧。
    app.update(); // 同天气、同图、同范围保持稳定。
    let counts = app.world().resource::<ChangeCounts>(); // 读取消费者所见变化。
    assert_eq!((counts.0, counts.1), before); // helper不得制造假变更，也不得触发重复GPU上传。
} // 结束范围与稳定发布回归。

#[test] // 首次作者保存后正式候选尚未到达，不能把刚画好的完整积水清空。
fn first_saved_live_mask_survives_file_wait_and_invalid_candidate() {
    // 覆盖保存关闭编辑器的交接空窗。
    let (mut app, source, _) = app(); // 建立尚未采用任何文件版的宿主。
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .remove(source.id()); // 模拟正式派生纹理仍在后台加载。
    let live_image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(image(2, TextureFormat::R8Unorm)); // 作者已有完整可见图片。
    app.insert_resource(MapPuddleLiveOverride {
        map_id: Some(MapId("HN-01".into())),
        mask: Some(live_image.clone()),
        world_min: Vec2::ZERO,
        world_max: Vec2::splat(10.0),
        ..default()
    }); // 发布已绘制完成的版本。
    app.update(); // 正式桥采用作者完整版本。
    app.insert_resource(MapPuddleLiveOverride::default()); // 保存关闭F12归还文件权威。
    app.update(); // 文件仍在加载，不得闪退到无积水。
    assert_eq!(
        app.world().resource::<SurfacePuddleMask>().image.as_ref(),
        Some(&live_image)
    ); // 继续显示刚保存的作者版。
    assert_eq!(
        app.world().resource::<SurfaceWeatherGpu>().bounds,
        Vec4::new(0.0, 0.0, 10.0, 10.0)
    ); // 范围不能清零或混用。
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .insert(source.id(), image(3, TextureFormat::R8Unorm))
        .expect("错误派生图"); // 模拟损坏正式候选。
    app.update(); // 错尺寸不能覆盖最近完整作者版。
    assert_eq!(
        app.world().resource::<SurfacePuddleMask>().image.as_ref(),
        Some(&live_image)
    ); // 坏候选继续保留最近完整结果。
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .insert(source.id(), image(2, TextureFormat::R8Unorm))
        .expect("修复派生图"); // 修复后源热更完整到达。
    app.update(); // 现在可以原子移交独立文件版。
    assert_ne!(
        app.world().resource::<SurfacePuddleMask>().image.as_ref(),
        Some(&live_image)
    ); // 完整受检文件接管，保留逻辑不永久粘住作者图。
    assert!(app.world().resource::<SurfacePuddleMask>().image.is_some()); // 全程都有完整可见遮罩。
} // 结束首次保存交接回归。

fn finish_comparison(app: &mut App) {
    // 测试允许等待后台结果，生产仍只单次轮询。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3); // 防止测试在错误任务状态下无限等待。
    while app
        .world()
        .resource::<PuddleRuntimeState>()
        .comparison
        .is_some()
        && std::time::Instant::now() < deadline
    {
        // 等待当前唯一任务完成。
        std::thread::yield_now(); // 让计算池工作线程获得执行机会。
        app.update(); // 通过生产系统非阻塞轮询，不直接取走任务结果。
    } // 完成测试限定等待。
    assert!(
        app.world()
            .resource::<PuddleRuntimeState>()
            .comparison
            .is_none()
    ); // 后台比较应在小夹具上及时完成。
} // 结束测试等待辅助。

#[test] // 同规格旧排队文件不得冒充本次保存，只有精确像素吻合才回执。
fn save_handoff_rejects_old_content_then_adopts_matching_pixels() {
    // 证明保存修订不是单纯依赖文件事件先后。
    let (mut app, source, _) = app(); // 安装旧正式图像。
    app.update(); // 采用旧127像素版本。
    let mut latest = image(2, TextureFormat::R8Unorm); // 新作者画布保持相同规格。
    latest.data.as_mut().unwrap().fill(255); // 仅内容发生变化。
    let live_image = app.world_mut().resource_mut::<Assets<Image>>().add(latest); // 最新完整作者图。
    app.insert_resource(MapPuddleLiveOverride {
        map_id: Some(MapId("HN-01".into())),
        mask: Some(live_image.clone()),
        world_min: Vec2::ZERO,
        world_max: Vec2::splat(10.0),
        ..default()
    }); // 保存完成仍暂时持有live。
    app.insert_resource(MapPuddleSaveHandoff {
        map_id: Some(MapId("HN-01".into())),
        texture_path: Some(descriptor().texture_path),
        expected_mask: Some(std::sync::Arc::new(vec![255; 4])),
        revision: 7,
    }); // 本次保存具有准确内容证明。
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .insert(source.id(), image(2, TextureFormat::R8Unorm))
        .expect("旧队列图像"); // 延迟到达的旧文件元数据完全合法。
    app.update(); // 将旧候选O(1)移交后台检查。
    finish_comparison(&mut app); // 等待生产轮询消费不匹配结果。
    assert_eq!(app.world().resource::<MapPuddleSaveHandoff>().revision, 7); // 保存身份不能被错误结果修改。
    assert!(
        app.world()
            .resource::<MapPuddleSaveHandoff>()
            .map_id
            .is_some()
    ); // 不匹配不能发成功回执。
    assert_eq!(
        app.world().resource::<SurfacePuddleMask>().image.as_ref(),
        Some(&live_image)
    ); // 最新作者图始终保留。
    let mut correct = image(2, TextureFormat::R8Unorm); // 真正本次落盘结果。
    correct.data.as_mut().unwrap().fill(255); // 基级内容与保存Arc完全相同。
    app.world_mut()
        .resource_mut::<Assets<Image>>()
        .insert(source.id(), correct)
        .expect("本次保存图像"); // 文件热更终于到达。
    app.update(); // 移交新图像进行内容证明。
    finish_comparison(&mut app); // 后台完成后由正式桥原子采用。
    assert!(
        app.world()
            .resource::<MapPuddleSaveHandoff>()
            .map_id
            .is_none()
    ); // 只有精确匹配结果才能成功回执。
    let adopted = app
        .world()
        .resource::<SurfacePuddleMask>()
        .image
        .clone()
        .unwrap(); // 获取新独立文件版本。
    assert_ne!(adopted, live_image); // 保存交接真正归还文件权威。
    assert_eq!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&adopted)
            .unwrap()
            .data
            .as_deref(),
        Some([255_u8; 4].as_slice())
    ); // 新版本准确对应保存画布。
} // 结束保存内容证明回归。

#[test] // 即使旧比较内容匹配，较新保存修订也不能被迟到结果清除。
fn late_comparison_cannot_acknowledge_a_new_save_revision() {
    // 锁定后台结果的保存身份守卫。
    let (mut app, _, _) = app(); // 构造正式单图宿主。
    app.update(); // 先采用一个旧完整文件版本。
    app.insert_resource(MapPuddleSaveHandoff {
        map_id: Some(MapId("HN-01".into())),
        texture_path: Some(descriptor().texture_path),
        expected_mask: Some(std::sync::Arc::new(vec![255; 4])),
        revision: 8,
    }); // 最新保存已经替代第七次保存。
    let task = bevy::tasks::AsyncComputeTaskPool::get()
        .spawn(async { (image(2, TextureFormat::R8Unorm), true) }); // 模拟第七次保存内容已匹配但迟到完成。
    {
        // 把旧票据安装到唯一在途槽。
        let mut state = app.world_mut().resource_mut::<PuddleRuntimeState>(); // 只修改测试在途状态。
        state.comparison = Some(task); // 模拟尚未轮询的后台完成结果。
        state.comparison_ticket = Some((
            MapId("HN-01".into()),
            7,
            descriptor().texture_path,
            descriptor(),
            Vec4::new(0.0, 0.0, 10.0, 10.0),
        )); // 旧保存修订与当前第八次不符。
    } // 释放资源借用。
    finish_comparison(&mut app); // 正式系统轮询旧结果并复核身份。
    let handoff = app.world().resource::<MapPuddleSaveHandoff>(); // 最新保存仍需自己的真实内容证明。
    assert_eq!(handoff.revision, 8); // 迟到结果不能修改新保存身份。
    assert!(handoff.map_id.is_some()); // 迟到结果不得发布成功回执。
} // 结束后台迟到拒收回归。
