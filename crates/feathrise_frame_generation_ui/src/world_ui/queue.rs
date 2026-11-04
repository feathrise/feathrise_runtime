//! 世界 UI 在 queue 完成后、排序和 batching 前分流；下一帧先归还 retained 项保持 Bevy 缓存正确。
use super::request::WorldUiRequest as RenderRequest; // 时间重建与帧生成共用独立世界标签资格。
use super::{
    components::WorldUiLayer,
    resources::{MAX_WORLD_UI_BYTES, MAX_WORLD_UI_ITEMS, WorldUiEntities, WorldUiFrameGate},
    types::WorldUi3d,
};
use crate::ui::WorldUiCompositeReady; // 高清绘制只依赖空间合成能力，不依赖SDK在线。
use bevy::image::ToExtents; // 将已验证的二维尺寸转为单层纹理附件规格。
use bevy::{
    camera::{CompositingSpace, MainPassResolutionOverride},
    core_pipeline::{core_3d::Transparent3d, oit::OrderIndependentTransparencySettings},
    diagnostic::FrameCount,
    pbr::DrawMaterial,
    prelude::*,
    render::{
        Extract,
        camera::ExtractedCamera,
        render_phase::{DrawFunctions, PhaseItemExtraIndex, ViewSortedRenderPhases},
        render_resource::{
            PipelineCache, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
        },
        renderer::RenderDevice,
        sync_world::MainEntity,
        texture::TextureCache,
        view::{ExtractedView, ViewTarget},
    },
}; // 公开视图、材质和缓存 API，不复制私有 PBR 布局。
use feathrise_scene_core::view::FrameGenerationUi; // 只处理业务显式声明的世界 UI 网格。
use std::sync::atomic::AtomicBool; // 本帧绘制成功事实由 Core3d 非阻塞发布。 // 独立 phase 的有界输入与输出。

pub(super) fn extract_entities(
    mut output: ResMut<WorldUiEntities>,
    entities: Extract<Query<Entity, (With<FrameGenerationUi>, With<Mesh3d>)>>,
) {
    // 不遍历整个透明场景，仅查询显式 UI archetype。
    output.items.clear(); // 重用有限 Vec 容量。
    output.over_budget = false; // 本帧重新判断固定数量上限。
    for entity in entities.iter().take(MAX_WORLD_UI_ITEMS + 1) {
        // 最多读取预算加一即可决定完整回退。
        if output.items.len() == MAX_WORLD_UI_ITEMS {
            output.over_budget = true;
            output.items.clear();
            break;
        } // 不对 UI 做部分剥离，超限全部保持普通路径。
        output
            .items
            .push((Entity::PLACEHOLDER, MainEntity::from(entity))); // Bevy 0.19 queue_material_meshes 明确使用 PLACEHOLDER/main_entity 成对键。
    } // 有界精确标记提取结束。
} // extract_entities 结束。

pub(super) fn restore_retained_items(
    mut original: ResMut<ViewSortedRenderPhases<Transparent3d>>,
    mut overlay: ResMut<ViewSortedRenderPhases<WorldUi3d>>,
) {
    // 必须早于 queue_material_meshes 的 dirty/removal 判断。
    for (view, phase) in overlay.iter_mut() {
        // 该候选只会持有一个主视图的至多固定预算项。
        if let Some(destination) = original.get_mut(view) {
            // 原视图已消失时不复活其场景队列。
            for (_, mut item) in phase.items.drain(..) {
                // 归还上一帧实际借用的 retained 记录。
                item.item.draw_function = item.original_draw_function; // 恢复原 Transparent3d 的 DrawFunctions 身份。
                item.item.pipeline = item.original_pipeline; // 原队列必须继续使用普通抖动投影。
                item.item.batch_range = 0..1; // 不跨 phase 复用上帧已生成的实例批次范围。
                item.item.extra_index = PhaseItemExtraIndex::None; // 当前主 phase batching 重新计算自己的间接索引。
                destination.add_retained(item.item); // Bevy queue 随后能正常处理隐藏、销毁、材质和视图变化。
            } // 原始项归还结束。
        } else {
            phase.clear();
        } // 已销毁相机的 UI phase 直接释放。
        phase.transient_items.clear(); // 此处只借用标准 Material 的 retained 项。
    } // 上帧有限视图处理结束。
    overlay.retain(|view, _| original.contains_key(view)); // 不积累历史地图和相机的空 phase。
} // restore_retained_items 结束。

#[allow(clippy::too_many_arguments, clippy::type_complexity)] // 每个输入都对应分流事务的一个独立所有权或就绪条件。
pub(super) fn split_world_ui(
    // 位于 QueueSweep 之后且 PhaseSort/PrepareResourcesBatchPhases 之前。
    mut commands: Commands, // 最后一步发布完整纹理元数据，下一阶段自动应用。
    request: Res<RenderRequest>, // 从时间重建能力或就绪FG请求解析的唯一高清主相机。
    entities: Res<WorldUiEntities>, // 显式 UI 实体的有界精确索引。
    readiness: (
        Res<WorldUiCompositeReady>,
        Res<super::resources::WorldUiDepth>,
        ResMut<super::resources::WorldUiPipelines>,
        Res<bevy::render::render_asset::RenderAssets<bevy::render::storage::GpuShaderBuffer>>,
    ), // 合成、深度、投影和引导缓冲必须共同就绪。
    views: Query<(
        Entity,
        &ExtractedView,
        &ExtractedCamera,
        &ViewTarget,
        &Msaa,
        Option<&MainPassResolutionOverride>,
        Has<OrderIndependentTransparencySettings>,
    )>, // 保留原相机、深度兼容和颜色规格。
    frame: Res<FrameCount>, // 捕获只能由本帧后续 UI 消费。
    device: Res<RenderDevice>, // 纹理创建仍归 Bevy/wgpu 管理。
    mut textures: ResMut<TextureCache>, // 只缓存最近规格的一张完整透明颜色。
    cache: Res<PipelineCache>, // 所有被移动项的管线必须已经异步编译完成。
    original_draws: Res<DrawFunctions<Transparent3d>>, // 只复用标准 PBR Material 绘制契约。
    draws: Res<DrawFunctions<WorldUi3d>>, // 新 phase 的公开 DrawMaterial 函数身份。
    mut original: ResMut<ViewSortedRenderPhases<Transparent3d>>, // 分流之前仍包含完整普通透明绘制。
    mut overlay: ResMut<ViewSortedRenderPhases<WorldUi3d>>, // 新 phase 独立排序和批处理，不能在 batching 后挪项。
    mut gate: ResMut<WorldUiFrameGate>, // 当前可见标签未成功分离时必须明确阻断 Prepare。
    mut last_rejection: Local<Option<&'static str>>, // 只在状态改变时说明回退，不持续刷屏。
) {
    // 世界 UI 分流事务入口。
    let (ready, depth_mapping, mut variants, buffers) = readiness; // 完整分流前提。
    *gate = WorldUiFrameGate {
        frame: frame.0,
        camera: request.camera,
        required: false,
        split: false,
    }; // 每帧覆盖事实，绝不沿用旧视图成功状态。
    if request.camera.is_none() {
        // 没有需要高清世界标签的唯一主视图才关闭。
        return;
    } // 关闭时完整保留普通标签。
    let Some((entity, extracted, camera, target, msaa, resolution, oit)) =
        views.iter().find(|(_, extracted, ..)| {
            Some(extracted.retained_view_entity.main_entity.id()) == request.camera
        })
    else {
        return;
    }; // 不猜辅助相机。
    let Some(phase) = original.get_mut(&extracted.retained_view_entity) else {
        return;
    }; // 缺失主 phase 时不创建半套输入。
    gate.required = entities.over_budget
        || entities
            .items
            .iter()
            .any(|key| phase.items.contains_key(key)); // 只检查明确 UI 集合的 Hash 键，不扫描其他透明场景。
    if !gate.required || !ready.0 {
        return;
    } // 有标签但管线未就绪时保持普通颜色，同时让 Prepare 看到未完成分离。
    if cache.get_render_pipeline(depth_mapping.pipeline).is_none() {
        return;
    } // 不在深度管线编译期间移走名牌。
    if buffers
        .get(feathrise_billboard_text::BILLBOARD_GUIDE_GATE.id())
        .is_none()
    {
        return;
    } // 引导不能暂停时绝不把字形留在无UI场景。
    // SDK失败或关闭不影响高清文字绘制；SDK捕获权限仍由独立的UiFramePermission控制。
    if entities.over_budget {
        report_rejection(&mut last_rejection, "世界 UI 实体超过固定 4096 项预算");
        return;
    } // 超预算不允许部分剥离。
    if entities.items.is_empty() {
        return;
    } // 没有指定 UI 时不分配纹理。
    let output_size = camera.physical_target_size.unwrap_or(UVec2::ZERO); // 窗口物理附件尺寸。
    let internal_size = resolution.map_or(output_size, |size| size.0); // 主透明和主深度实际使用的有效 viewport。
    let valid = output_size.min_element() > 0 && internal_size.min_element() > 0 && internal_size.cmple(output_size).all() // 内部区域必须完整落在附件内。
        && camera.physical_viewport_size == Some(output_size) && camera.viewport.as_ref().is_none_or(|view| view.physical_position == UVec2::ZERO && view.physical_size == output_size) // 首版只接受唯一全窗口视口。
        && camera.target.is_some() && views.iter().filter(|(_, _, other, ..)| other.target == camera.target).take(2).count() == 1 // 多相机共享窗口不能取得原子场景/UI 所有权。
        && msaa.samples() == 1 && !oit && target.main_texture_format() == TextureFormat::Rgba16Float && matches!(target.compositing_space, None | Some(CompositingSpace::Linear)) // MSAA/OIT 或不同颜色契约全部回退原 phase。
        && target.main_texture().size().width == output_size.x && target.main_texture().size().height == output_size.y // 与现有主深度及合成目标共用附件尺寸。
        && output_size.max_element() <= device.limits().max_texture_dimension_2d && u64::from(output_size.x) * u64::from(output_size.y) * 12 <= MAX_WORLD_UI_BYTES; // RGBA16F颜色加Depth32Float重映射共12字节/像素，沿用固定总显存预算。
    if !valid {
        report_rejection(
            &mut last_rejection,
            "世界 UI 分离只支持单相机 SDR/HDR线性载体、单采样、非 OIT 和有界全窗口附件",
        );
        return;
    } // 任何不兼容组合均完整保留原绘制。
    let standard_draw = original_draws.read().id::<DrawMaterial>(); // 自定义 DrawFunction 不冒充标准材质。
    let mut count = 0; // 只统计主视图实际可见的指定 UI。
    for key in &entities.items {
        // Hash 查找只触及标记集合，不扫描全部水、草、粒子。
        if let Some(item) = phase.items.get(key) {
            // 隐藏和未准备材质的实体留给 Bevy 正常处理。
            if item.draw_function != standard_draw
                || super::pipeline::prepare_variant(item.pipeline, &cache, &mut variants).is_none()
            {
                return;
            } // 所有分流项完全就绪才开始改变 phase。
            count += 1; // 本帧可绘制的 UI 数量。
        } // 当前标记的主视图可见性结束。
    } // 分流前管线预检结束。
    if count == 0 {
        return;
    } // 不创建空的 GPU 捕获纹理。
    let texture = textures.get(
        &device,
        TextureDescriptor {
            // 主世界创建和运行帧均不做同步 GPU 等待。
            label: Some("frame_generation_world_ui_linear"), // RenderDoc 能明确识别名牌/血条/伤害数字的独立透明层。
            size: output_size.to_extents(),                  // 与借用主 depth 的附件规格严格一致。
            mip_level_count: 1,                              // 字形层只做一次明确的空间采样。
            sample_count: 1,                                 // 与分流前单采样门一致。
            dimension: TextureDimension::D2,                 // 完整二维窗口附件。
            format: TextureFormat::Rgba16Float,              // 原 HDR PBR 材质管线可直接复用。
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING, // 只绘制与采样，不交给 SDK 或 CPU 回读。
            view_formats: &[], // 不允许隐式颜色格式重解释。
        },
    ); // 单张纹理取得完成。
    let mapped_depth = textures.get(
        &device,
        TextureDescriptor {
            // 与颜色一起原子发布并复用缓存。
            label: Some("world_ui_output_depth"), // 单张输出深度。
            size: output_size.to_extents(),       // 与高清名牌使用同一坐标范围。
            mip_level_count: 1,                   // 不生成mip。
            sample_count: 1,                      // 与分流要求一致。
            dimension: TextureDimension::D2,      // 二维窗口附件。
            format: bevy::core_pipeline::core_3d::CORE_3D_DEPTH_FORMAT, // 原材质深度格式。
            usage: TextureUsages::RENDER_ATTACHMENT, // GPU映射写入并供名牌深度测试。
            view_formats: &[],                    // 不允许格式重解释。
        },
    ); // 深度纹理取得完成。
    let draw = draws.read().id::<super::pipeline::DrawWorldUi>(); // 独立和回退分别选择正确投影。
    let destination = overlay.entry(extracted.retained_view_entity).or_default(); // 只为唯一玩法视图创建独立 phase。
    for key in &entities.items {
        // 移动发生在排序/batching 之前，swap_remove 不会破坏已生成实例范围。
        if let Some(mut item) = phase.items.swap_remove(key) {
            // 只移动精确匹配的可见 UI 网格。
            let original_draw_function = item.draw_function; // 保存下一帧归还原 retained phase 的身份。
            let original_pipeline = item.pipeline; // 保存原管线，防止回退继承无抖动投影。
            item.pipeline = variants.0[&original_pipeline]; // 所有派生管线已完成预检。
            item.draw_function = draw; // 独立 phase 需要自己的 DrawFunctions 索引。
            destination.add_retained(WorldUi3d {
                item,
                original_draw_function,
                original_pipeline, // 失败回退和下一帧恢复共用原始身份。
            }); // 后续公开 batching 系统为本 phase 独立生成实例和间接参数。
        } // 当前项分流结束。
    } // 原子分流结束。
    commands.entity(entity).insert(WorldUiLayer {
        // 只有纹理和全部绘制管线已准备才发布完整本帧契约。
        texture,                             // 保留强引用至本帧 UI 合成完成。
        mapped_depth,                        // 独立输出深度，不改写主场景。
        depth_drawn: AtomicBool::new(false), // 禁止复用上一帧映射。
        internal_size,                       // 保留主视图内部viewport，绘制失败时按原空间回退。
        render_size: output_size, // 独立UI使用完整输出viewport，避免DLSS超性能下二次放大低分辨率字形。
        output_size,              // UI 合成恢复到完整物理窗口。
        camera: extracted.retained_view_entity.main_entity.id(), // 唯一主世界相机身份。
        frame: frame.0,           // 未绘制或旧帧纹理不得被重用。
        drawn: AtomicBool::new(false), // 独立绘制完成前始终不可消费。
        failed: AtomicBool::new(false), // 只有已分流后的实际绘制错误才标记本帧事务失败。
    }); // Layer 发布结束。
    gate.split = true; // 这里只确认分流完整；Prepare 还必须核验实际 Layer.drawn。
    *last_rejection = None; // 恢复兼容规格后允许下一次状态变化准确说明。
} // split_world_ui 结束。

fn report_rejection(previous: &mut Option<&'static str>, reason: &'static str) {
    // 拒绝分流时仍保持完整普通绘制。
    if *previous != Some(reason) {
        warn!("帧生成世界 UI 保持普通渲染：{reason}");
        *previous = Some(reason);
    } // 稳定帧不输出重复诊断。
} // 回退诊断结束。
