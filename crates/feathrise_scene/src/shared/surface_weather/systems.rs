//! 独立积水纹理受检采用与同帧天气GPU桥；禁止主线程全图复制或逐像素计算。

use bevy::prelude::*; // 资产、状态、句柄和固定向量。
use bevy::render::render_resource::{TextureDimension, TextureFormat}; // 只接受线性二维R8作者图。
use feathrise_atmosphere::weather::{WeatherAuthoringPreview, WeatherOutput}; // 唯一天气输出与F12锚点身份。
use feathrise_environment::terrain_surface::{
    SurfacePuddleMask, SurfaceWeatherGpu, load_terrain_surface_image, terrain_surface_image_failed,
}; // 公共GPU状态和标准异步纹理加载。
use feathrise_map_content::MapPuddleSaveHandoff; // 保存完成后用实际基级像素证明正式文件属于本次画布。
use feathrise_map_content::{
    ActiveDynamicMap, MapId, MapManifestAsset, MapPuddleDescriptor,
    MapPuddleLiveOverride,
}; // 文件权威和作者完整草稿。
use feathrise_settings::{EffectsQuality, RenderSettings}; // 复用现有持久特效质量档。
use feathrise_scene_core::state::ScenePhase; // 当前玩法地图而非相机位置决定身份。
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
}; // 主线程仅轮询一次，不等待后台像素比较。

use super::resources::PuddleRuntimeState; // 当前候选与上一完整结果。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut, InstanceFrame};
use super::resources::PuddleFileCache;

fn current_map(scene: ScenePhase, dynamic: &ActiveDynamicMap) -> Option<MapId> {
    // 动态地图不能只比较共用GameScene枚举。
    match scene {
        // 静态地图保留正式ID，菜单和加载态没有积水域。
        ScenePhase::Active => dynamic.map_id.clone(),   // 真实动态清单身份。
        _ => None,                                              // 菜单、加载或未就绪场景安全清空。
    } // 完成地图身份解析。
} // 完成场景到地图映射。

fn validate_descriptor(map: &MapId, descriptor: &MapPuddleDescriptor) -> Result<(), String> {
    // 独立作者文件不能指向另一张地图。
    descriptor.validate()?; // 复用编辑器共同的尺寸/后缀/安全路径校验。
    let prefix = format!("maps/{}/", map.as_str()); // 当前地图唯一私有根。
    if !descriptor.source_path.starts_with(&prefix) || !descriptor.texture_path.starts_with(&prefix)
    {
        // 不能通过合法maps前缀跨图串用。
        return Err("积水作者源和纹理必须位于当前地图私有目录".into()); // 保留旧有效文件或无积水回退。
    } // 完成精确地图身份保护。
    Ok(()) // 整份规格受检成功。
} // 完成正式读取防护。

fn validate_image(image: &Image, descriptor: &MapPuddleDescriptor) -> bool {
    // 元数据校验是常量工作，不扫描像素。
    let texture = &image.texture_descriptor; // 借用已由ImageLoader后台解码的描述符。
    texture.format == TextureFormat::R8Unorm // 潜力必须线性，RGBA/sRGB不能静默读取红通道。
        && texture.dimension == TextureDimension::D2 // 不是体纹理。
        && texture.size.depth_or_array_layers == 1 // 不是图层数组。
        && texture.size.width == descriptor.resolution[0] // 作者源与GPU图必须同宽。
        && texture.size.height == descriptor.resolution[1] // 作者源与GPU图必须同高。
} // 完成固定元数据门。

fn publish_mask(mask: &mut InstanceResMut<SurfacePuddleMask>, image: Option<Handle<Image>>) {
    // 保留ResMut包装，比较时不得提前解引用成可变资源而制造假变更。
    if mask.image != image {
        mask.image = image;
    } // 强句柄身份相同就保持资源未变。
} // 完成遮罩句柄发布。

fn report_error(state: &mut PuddleRuntimeState, error: String) {
    // 热更失败持续显示旧完整结果。
    if state.error.as_ref() != Some(&error) {
        // 每种实际失败只记录一次。
        warn!("积水遮罩保留上一完整版本：{error}"); // 日志准确指出回退而非声称加载成功。
        state.error = Some(error); // 等待后续修复热更新。
    } // 完成去重诊断。
} // 完成错误发布。

#[allow(clippy::too_many_arguments)] // 各资源分别代表当前身份、权威、完整GPU输入，保持读写边界可审计。
pub(crate) fn resolve_puddle_mask(
    // 每帧只查询一个当前地图、一份候选和一份作者覆盖。
    scene: InstanceRes<State<ScenePhase>>,              // 当前正式玩法阶段。
    dynamic: InstanceRes<ActiveDynamicMap>,            // 同枚举动态地图的真实ID。
    registry: crate::maps::dynamic_map::sources::MapRuntimeCatalog,         // 已校验目录中的清单句柄。
    manifests: Res<Assets<MapManifestAsset>>,  // 完整清单加载后才解析积水字段。
    asset_server: Res<AssetServer>,            // 后台加载KTX，不在系统中读文件。
    mut images: ResMut<Assets<Image>>,         // 只移动已解码Image，不克隆整图数据。
    live: crate::maps::dynamic_map::author_sources::TerrainAuthorRes<MapPuddleLiveOverride>,          // 编辑器只发布完整、身份有效的草稿图片。
    mut handoff: crate::maps::dynamic_map::author_sources::TerrainAuthorResMut<MapPuddleSaveHandoff>, // 只有精确文件版本采用后才清除保存回执。
    authoring: Option<Res<WeatherAuthoringPreview>>, // F12可预览不同于当前玩法图的作者地图。
    mut state: InstanceResMut<PuddleRuntimeState>,     // 一个候选与一个活动文件版本。
    mask: Option<InstanceResMut<SurfacePuddleMask>>,   // 无环境渲染插件的逻辑宿主安全跳过。
    mut cache: Option<ResMut<PuddleFileCache>>,
    frame: InstanceFrame,
) {
    // 所有状态切换保持纹理和世界范围同次发布。
    let Some(mut mask) = mask else {
        return;
    }; // 无渲染宿主无需加载图片。
    let gameplay_map = current_map(*scene.get(), &dynamic); // 先取得正式身份。
    let author_active = frame.instance().is_none() && authoring
        .as_deref()
        .is_some_and(WeatherAuthoringPreview::active); // 只在真正F12预览时接受异图覆盖。
    let map_id = if author_active && live.map_id.is_some() {
        live.map_id.clone()
    } else {
        gameplay_map
    }; // 未保存跨图预览以明确作者身份为准。
    if state.map_id != map_id {
        // 同GameScene切到另一MapId同样必须失效。
        let comparison = state.comparison.take(); // 换图时在途任务继续占用唯一槽位，防止快速换图无限启动工作。
        let comparison_ticket = state.comparison_ticket.take(); // 保留旧身份以便完成后明确丢弃。
        *state = PuddleRuntimeState {
            map_id: map_id.clone(),
            ..default()
        }; // 丢弃旧源、候选和世界范围。
        state.comparison = comparison;
        state.comparison_ticket = comparison_ticket; // 旧任务结束前不再发起另一份比较。
        publish_mask(&mut mask, None); // 不能让旧地图纹理出现在下一地图。
    } // 完成换图失效。
    let Some(map_id) = map_id else {
        // 菜单和加载态无积水范围。
        publish_mask(&mut mask, None); // 空身份明确禁用。
        state.preview = false; // 不遗留作者蓝色覆盖。
        return; // 不读取或加载任何地图纹理。
    }; // 完成玩法/作者身份门。
    if live.map_id.as_ref() == Some(&map_id) {
        // Some身份+None纹理表示作者显式空图，不能回退磁盘旧图。
        let valid_bounds = live.world_min.is_finite()
            && live.world_max.is_finite()
            && live.world_min.cmplt(live.world_max).all(); // 常量范围验证。
        if valid_bounds {
            // 编辑器已经完成后台图像构建与修订复核。
            state.bounds = Vec4::new(
                live.world_min.x,
                live.world_min.y,
                live.world_max.x,
                live.world_max.y,
            ); // 与作者纹理原子采用。
            state.preview = author_active && live.preview; // 关闭F12后保留真实积水但不保留蓝色调试层。
            publish_mask(&mut mask, live.mask.clone()); // 不复制Arc源或Image像素。
        } // 非法草稿保持上一完整结果。
        if handoff.map_id.as_ref() != Some(&map_id) {
            return;
        } // 普通作者草稿不加载磁盘，保存交接则继续处理新正式文件。
    } // 完成完整作者覆盖。
    state.preview = false; // 返回文件权威时始终显示真实材质。
    let Some(manifest) = registry
        .handles(&map_id)
        .and_then(|handles| manifests.get(&handles.manifest))
    else {
        return;
    }; // 文件未到保留同图旧完整结果。
    if manifest.id != map_id {
        return;
    } // 拒绝迟到或错误MapId的清单。
    let Some(bounds) = manifest.world_bounds() else {
        return;
    }; // 无效范围不能进入Shader。
    let requested_bounds = Vec4::new(bounds.min.x, bounds.min.y, bounds.max.x, bounds.max.y); // 当前清单完整范围。
    let Some(descriptor) = manifest.puddles.as_ref() else {
        // 旧图和显式移除均表示不积水。
        if handoff.map_id.as_ref() == Some(&map_id) {
            return;
        } // 首次保存的清单热更尚未到达，不能用旧None清空已保存作者画面。
        state.requested = None;
        state.error = None; // 明确移除积水也解除旧候选失败，允许完整无积水来源通过就绪门。
        state.source = None;
        state.file_mask = None; // 释放完整旧文件与候选强引用。
        state.file_descriptor = None; // 清理已采用规格，避免下一份候选误用旧范围身份。
        state.bounds = requested_bounds; // 仍保留合法范围供将来作者接入。
        publish_mask(&mut mask, None); // 湿润仍由天气独立驱动。
        return; // 不为旧地图制造全图纹理。
    }; // 完成可选作者字段解析。
    let saving = handoff.map_id.as_ref() == Some(&map_id); // 保存交接期间旧同规格文件不能抢占最新作者图。
    if saving
        && (handoff.texture_path.as_ref() != Some(&descriptor.texture_path)
            || handoff.expected_mask.is_none())
    {
        return;
    } // 等待对应保存的完整清单，不以旧路径或缺内容证明接管。
    if let Some(task) = state.comparison.as_mut() {
        // 在途工作最多一份，每帧只轮询一次。
        if let Poll::Ready((image, matches)) =
            Pin::new(task).poll(&mut Context::from_waker(Waker::noop()))
        {
            // 不阻塞帧关键路径。
            state.comparison = None; // 完成后释放单任务槽位。
            if let Some((ticket_map, revision, path, specification, ticket_bounds)) =
                state.comparison_ticket.take()
            {
                // 获取完成结果准确身份。
                if saving
                    && handoff.revision == revision
                    && ticket_map == map_id
                    && handoff.texture_path.as_ref() == Some(&path)
                    && descriptor == &specification
                    && ticket_bounds == requested_bounds
                {
                    // 拒收换图、重保存和清单修订迟到结果。
                    if matches && validate_image(&image, descriptor) {
                        // 完整像素与本次保存相同且元数据仍合法。
                        state.file_mask = Some(images.add(image));
                        if let Some(cache) = cache.as_mut() { cache.insert(descriptor, state.file_mask.as_ref().unwrap().id()); }
                        state.file_descriptor = Some(descriptor.clone()); // O(1)采用后台归还图像。
                        state.file_bounds = requested_bounds;
                        state.error = None; // 与完整纹理同次采用范围。
                        handoff.map_id = None;
                        handoff.expected_mask = None;
                        handoff.texture_path = None; // 成功回执允许编辑器归还live覆盖。
                        if let Some(source) = state.source.as_ref() {
                            images.remove(source.id());
                        } // 本次比较期间到达的无保存身份候选不能在同帧再次覆盖已证明版本。
                        state.bounds = requested_bounds; // 同次发布正式版本完整范围。
                        publish_mask(&mut mask, state.file_mask.clone()); // 回执与完整独立图像采用不可分开。
                        return; // 完成本次交接，后续真实热更下一帧按普通文件链消费。
                    } else {
                        report_error(&mut state, "正式积水纹理尚未匹配本次保存画布".into());
                    } // 旧排队文件或坏文件不得清除最新作者图。
                } else if saving && ticket_map == map_id && handoff.revision == revision {
                    // 同次保存期间清单范围或规格变化使旧比较失效。
                    asset_server.reload(descriptor.texture_path.clone()); // 请求当前版本重新验证，避免已移走源图后保存回执永远悬挂。
                } // 其他迟到结果直接丢弃，不影响新保存已经请求的加载。
            } // 完成保存票据消费。
        } // 未就绪立即继续，不等待后台。
    } // 完成单次非阻塞轮询。
    if state.requested.as_ref() != Some(descriptor) {
        // 规格真正变化时才校验字符串和请求异步加载。
        if let Err(error) = validate_descriptor(&map_id, descriptor) {
            report_error(&mut state, error);
            return;
        } // 坏热更保持旧完整纹理。
        let source = load_terrain_surface_image(
            &asset_server,
            descriptor.texture_path.clone(),
            false,
            false,
            "map_puddle_source",
        ); // 独立R8采用线性Clamp和Mip过滤。
        state.requested = Some(descriptor.clone()); // 候选保存准确的尺寸与私有路径身份。
        state.source = Some(source); // 原源句柄一直持有，以接收下一次热更新。
        state.error = None; // 新规格可重新尝试。
    } // 完成候选请求。
    if state.comparison.is_none()
        && let Some(source) = state.source.clone()
    {
        // 后台槽位被占用时不重复移交源图。
        if let Some(image) = images.get(&source) {
            // 只有后台解码完整Image才能进入元数据门。
            if validate_image(image, descriptor) {
                // 损坏格式或尺寸不替换旧完整副本。
                if let Some(image) = images.remove(source.id()) {
                    // O(1)转移像素所有权，不在帧路径克隆16MiB纹理。
                    if saving && let Some(expected) = handoff.expected_mask.clone() {
                        // 保存交接必须证明实际像素，而不只依赖异步事件先后顺序。
                        let count =
                            descriptor.resolution[0] as usize * descriptor.resolution[1] as usize; // 只比较基级R8，后续Mip不属于原始画布。
                        state.comparison_ticket = Some((
                            map_id.clone(),
                            handoff.revision,
                            descriptor.texture_path.clone(),
                            descriptor.clone(),
                            requested_bounds,
                        )); // 固定大小票据跟随唯一任务。
                        state.comparison =
                            Some(bevy::tasks::AsyncComputeTaskPool::get().spawn(async move {
                                // 完整图与Arc源移动到后台，不深复制像素。
                                let matches = expected.len() == count
                                    && image.data.as_ref().is_some_and(|bytes| {
                                        bytes.get(..count) == Some(expected.as_slice())
                                    }); // 后台完成有界最大16MiB精确内容比较。
                                (image, matches) // 把原图所有权和结果归还主线程原子采用。
                            })); // 最大一份任务，后续源修订留在Assets等待。
                        return; // 当前帧继续保留完整作者图，禁止提前回旧文件。
                    } // 普通热更无需保存内容证明。
                    state.file_mask = Some(images.add(image)); // 新独立资产身份冻结本次完整版本，坏源热更不能篡改它。
                    if let Some(cache) = cache.as_mut() { cache.insert(descriptor, state.file_mask.as_ref().unwrap().id()); }
                    state.file_descriptor = Some(descriptor.clone()); // 仅完整图像通过后登记采用规格。
                    state.file_bounds = requested_bounds; // 纹理与世界范围同次采用。
                    state.error = None; // 修复热更已完整通过。
                } // 后续源热更仍会重新插入原AssetServer身份。
            } else {
                // 非法新源不会泄漏到活动材质句柄。
                images.remove(source.id()); // 丢弃该坏候选，等待下一次真实热更新。
                report_error(&mut state, "纹理必须为与作者分辨率一致的二维线性R8".into()); // 保留独立活动版本。
            } // 完成当前源消费。
        } else if terrain_surface_image_failed(&asset_server, &source) {
            // 未加载不算失败。
            report_error(&mut state, "积水纹理加载失败".into()); // 同图旧结果持续可用。
        } // 完成失败/等待区分。
    } // 完成单一候选处理。
    if !saving && let Some(cache) = cache.as_deref()
        && let Some((specification, image)) = cache.paths.get(&descriptor.texture_path)
        && specification == descriptor && let Some(image) = images.get_strong_handle(*image) {
        state.file_mask = Some(image); state.file_descriptor = Some(descriptor.clone());
        state.file_bounds = requested_bounds; state.error = None;
    } // 每个引用取得同一个受检完整版本，不争抢或深复制AssetServer返回的像素所有权。
    if handoff.map_id.as_ref() != Some(&map_id)
        && state.file_descriptor.as_ref() == Some(descriptor)
        && state.file_mask.is_some()
    {
        // 保存未回执时旧文件不可覆盖作者图；普通同规格允许仅范围变化。
        state.file_bounds = requested_bounds; // 仅范围热更必须生效；新规格等待中仍保持旧图原范围。
        state.bounds = state.file_bounds; // 仅完整匹配当前规格的文件版可以替换作者最近完整结果。
        publish_mask(&mut mask, state.file_mask.clone()); // 撤销未保存同规格草稿时恢复已采用文件版。
    } // 完成同图范围单独修订。
    // 首次保存或改规格后源仍在加载时，保留当前完整作者遮罩及其范围；失败候选也不能清空它。
} // 完成地图积水权威桥。

pub(super) fn prune_puddle_file_cache(mut cache: ResMut<PuddleFileCache>, mut events: MessageReader<AssetEvent<Image>>) {
    for event in events.read().take(128) {
        if let AssetEvent::Unused { id } | AssetEvent::Removed { id } = event
            && let Some(path) = cache.ids.remove(id)
            && cache.paths.get(&path).is_some_and(|(_, current)| current == id) { cache.paths.remove(&path); }
    }
}

pub(super) fn publish_surface_weather(
    // 每帧固定64字节，不遍历地形、水面或像素。
    output: Res<WeatherOutput>,           // 本帧自然/作者天气已统一解析。
    state: Res<PuddleRuntimeState>,       // 当前完整遮罩世界范围和预览身份。
    mask: Option<Res<SurfacePuddleMask>>, // 无渲染宿主可以缺失。
    settings: Option<Res<RenderSettings>>,  // 最小宿主默认高档。
    gpu: Option<ResMut<SurfaceWeatherGpu>>, // 固定共享GPU资源。
) {
    // PostUpdate保证作者Update变化本帧即可生效。
    let Some(mut gpu) = gpu else {
        return;
    }; // 逻辑测试不要求渲染插件。
    let active = state.map_id.is_some(); // 主菜单不显示天气材质变化。
    let quality = match settings
        .as_deref()
        .map_or(EffectsQuality::High, |settings| {
            settings.graphics.effects_quality
        }) {
        // 质量不改变积水历史。
        EffectsQuality::Low => 0.0,
        EffectsQuality::Medium => 1.0,
        EffectsQuality::High => 2.0,
        EffectsQuality::Ultra => 3.0, // 共享Shader固定质量编码。
    }; // 完成质量读取。
    let profile =
        feathrise_environment::terrain_surface::surface_effects_quality_profile(quality); // P4从唯一CPU/GPU预算表派生雨纹参数。
    let next = SurfaceWeatherGpu {
        // 将当前完整状态一次性组装。
        weather: if active {
            Vec4::new(
                output.surface_wetness,
                output.puddle_amount,
                output.precipitation,
                output.surface_effect_time,
            )
        } else {
            Vec4::ZERO
        }, // 无地图时天气回退为关闭。
        wind_quality: Vec4::new(
            output.wind_direction.x,
            output.wind_direction.y,
            output.wind,
            quality,
        ), // 河湖风雨细浪与植被同源。
        bounds: state.bounds, // 与当前强遮罩句柄同一版本。
        options: Vec4::new(
            if mask.as_deref().is_some_and(|mask| mask.image.is_some()) {
                1.0
            } else {
                0.0
            },
            if state.preview { 1.0 } else { 0.0 },
            profile.rain_range,
            profile.rain_density,
        ), // 开关与同档雨纹距离/密度同次发布，仍为64字节。
    }; // 完成64字节输入。
    if *gpu != next {
        *gpu = next;
    } // 暂停稳定帧不触发重复GPU上传。
} // 完成同帧天气发布。
