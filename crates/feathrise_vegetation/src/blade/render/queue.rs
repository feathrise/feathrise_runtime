//! GPU草地AlphaMask渲染阶段排队系统。
//! 每个可见LOD窗口生成一个阶段项，实际绘制命令再按八个深度桶发出间接绘制。

use bevy::{
    core_pipeline::{
        core_3d::AlphaMask3d, // 草叶通过抖动AlphaMask参与不透明深度写入。
        prepass::{
            DeferredPrepass, // 标识Solari视图；草叶仍写完整GBuffer，并在主颜色阶段恢复植被专用光照。
            OpaqueNoLightmap3dBatchSetKey, // AlphaMask批次键描述管线、绘制命令和网格缓冲页。
            OpaqueNoLightmap3dBinKey, // 网格资产ID作为不透明遮罩阶段的稳定分箱键。
        },
    },
    ecs::system::SystemParam, // 把排队资源与查询组合成具名参数集，缩短系统入口。
    pbr::{
        ExtractedDirectionalLight, // 只为已经启用阴影的玩法天体方向光排队近景草。
        LightEntity,               // 区分方向光级联与点光/聚光阴影视图。
        MeshPipelineKey,           // 组合视图MSAA、目标格式、阴影深度和网格拓扑专用化键。
        RenderMeshInstances,       // 读取主世界网格资产ID和当前uniform索引。
        Shadow,                    // 写入Bevy方向光阴影深度分箱阶段。
        ShadowBatchSetKey,         // 阴影批次键保存管线、绘制命令和共享缓冲页。
        ShadowBinKey,              // 近景草原型资产ID作为阴影分箱键。
        ViewKeyCache,              // 复用 Bevy 为当前视图生成的完整布局专用化键。
        ViewLightEntities,         // 从唯一玩法草地相机取得方向光级联视图列表。
    },
    prelude::*, // Query、Res与ResMut等ECS常用类型。
    render::{
        mesh::{RenderMesh, allocator::MeshAllocator}, // 读取GPU网格布局和共享缓冲页。
        render_asset::RenderAssets,                   // 只为已经上传GPU的原型网格排队。
        render_phase::{
            BinnedRenderPhaseType,  // 标记每个LOD窗口不可跨圈自动批处理。
            DrawFunctions,          // 查找已经注册的草地绘制命令ID。
            ViewBinnedRenderPhases, // 写入每个保留视图的AlphaMask分箱阶段。
        },
        render_resource::{
            CachedPipelineState,      // 向F3区分已就绪、暂缺Shader和永久失败的渲染管线。
            PipelineCache,            // 请求并查询异步编译的专用渲染管线。
            SpecializedMeshPipelines, // 缓存不同视图与原型拓扑组合。
        },
        view::{ExtractedView, RenderShadowMapVisibleEntities, RenderVisibleEntities}, // 只排队通过视锥和可见性检查的草地窗口，并按保留视图清理旧分箱。
    },
    shader::ShaderCacheError, // 把Shader资产尚未加载的可重试状态归入等待而不是错误。
};

use super::commands::{DrawGpuBlade, DrawGpuBladeShadow}; // 主画面与方向光阴影阶段分别复用同一紧凑实例输出。
use super::pipeline::{GpuBladePipeline, GpuBladeShadowPipeline}; // 分别提供主画面颜色管线和近景草深度管线。
use crate::components::{
    GpuBladePatch,           // 读取候选实例上限、LOD和本帧抑制状态。
    GpuBladePreparedBuffers, // 确认Compute已经建立紧凑实例与间接命令输出。
    GpuBladeView,            // 只让明确标记的玩法相机进入排队。
};
use crate::resources::GpuBladeRenderFrameCounters; // 记录可见、跳过、排队与管线状态供下一帧F3读取。

/// 草地遮罩排队阶段共享的资源与查询集合。
#[derive(SystemParam)]
pub(super) struct GpuBladeQueueParams<'w, 's> {
    draw_functions: Res<'w, DrawFunctions<AlphaMask3d>>, // 查找已注册的GPU草地遮罩绘制命令ID。
    grass_pipeline: Res<'w, GpuBladePipeline>,           // 提供基础MeshPipeline与草地着色器。
    specialized_pipelines: ResMut<'w, SpecializedMeshPipelines<GpuBladePipeline>>, // 获取或创建视图专用管线。
    pipeline_cache: Res<'w, PipelineCache>, // 请求并查询异步编译的专用渲染管线。
    meshes: Res<'w, RenderAssets<RenderMesh>>, // 只为已经上传GPU的原型网格排队。
    render_mesh_instances: Res<'w, RenderMeshInstances>, // 获取主世界网格资产和排序中心。
    mesh_allocator: Res<'w, MeshAllocator>, // 读取原型网格所在顶点与索引缓冲页。
    grass_patches: Query<
        'w,
        's,
        (
            &'static GpuBladePatch, // 读取候选实例上限、LOD和本帧抑制状态。
            Option<&'static GpuBladePreparedBuffers>, // 确认Compute已经建立紧凑输出。
        ),
    >, // 只有具备草窗组件的可见实体才进入后续排队分支。
    render_stats: Res<'w, GpuBladeRenderFrameCounters>, // 发布可见、跳过、管线和排队状态。
    view_key_cache: Res<'w, ViewKeyCache>, // 必须与实际 MeshViewBindGroup 使用同一套可选 binding 位。
    render_phases: ResMut<'w, ViewBinnedRenderPhases<AlphaMask3d>>, // 写入保留视图的遮罩分箱阶段。
    views: Query<
        'w,
        's,
        (
            &'static ExtractedView,         // 读取保留视图实体、HDR和相机信息。
            &'static RenderVisibleEntities, // 只遍历Bevy已经判定可见的草窗实体。
            Has<DeferredPrepass>, // 标识Solari视图，继续保留完整GBuffer，同时允许植被专用前向颜色覆盖严格单面PBR结果。
        ),
        With<GpuBladeView>,
    >, // 只为明确标记的唯一玩法视图生成草地阶段项。
} // 完成排队参数分组，系统入口只保留具名功能参数集。

/// 判断一个GPU植被窗口是否属于获准投射方向光阴影的近景档。
///
/// 【功能设计】全部公开profile都代表已经接入的叶片植物类别，因此阴影资格只按LOD限制；
/// 这样普通草、作物、芦苇和湿地植物都能接地，同时不会让中远景窗口放大阴影预算。
fn casts_near_blade_shadow(patch: &GpuBladePatch) -> bool {
    patch.lod_level() == 0 // 所有合法profile的LOD0投影，中、远和超远级继续禁止倍增阴影顶点量。
} // 结束近景叶片阴影资格判断。

/// 判断当前视图是否执行自定义前向草叶颜色通道。
///
/// 【功能设计】GPU blade 不是TLAS几何，且项目的薄叶外观依赖环境光、包裹漫反射与背透；
/// Solari的严格单面PBR会把密集杂粮压到接近全黑。Deferred阶段仍先写完整深度、运动向量、
/// GBuffer与RR引导，主颜色阶段再以同一草叶表面覆盖最终颜色，不增加TLAS或稳定帧CPU工作。
#[inline]
fn gpu_grass_uses_forward_main_pass(_has_deferred_prepass: bool) -> bool {
    true // 普通光栅与Solari视图都使用同一植被专用最终颜色；Deferred视图额外提供完整RR输入表面。
} // 结束草叶最终颜色路径选择。

/// 近景草方向光阴影排队阶段共享的资源与查询集合。
#[derive(SystemParam)]
#[allow(clippy::type_complexity)] // 阴影排队保留Bevy强类型Query过滤器，避免别名削弱字段诊断信息。
pub(super) struct GpuBladeShadowQueueParams<'w, 's> {
    draw_functions: Res<'w, DrawFunctions<Shadow>>, // 查找已经注册的近景草Shadow绘制命令ID。
    shadow_pipeline: Option<Res<'w, GpuBladeShadowPipeline>>, // 首个渲染帧等待Bevy预通道资源完成后再建立。
    specialized_pipelines: ResMut<'w, SpecializedMeshPipelines<GpuBladeShadowPipeline>>, // 缓存方向光深度和原型拓扑管线。
    pipeline_cache: Res<'w, PipelineCache>, // 异步编译近景草阴影WGSL管线。
    meshes: Res<'w, RenderAssets<RenderMesh>>, // 只排队已经上传GPU的LOD0草原型。
    render_mesh_instances: Res<'w, RenderMeshInstances>, // 取得近景草主实体网格ID和uniform索引。
    mesh_allocator: Res<'w, MeshAllocator>, // 读取LOD0原型所在共享顶点与索引缓冲页。
    render_stats: Res<'w, GpuBladeRenderFrameCounters>, // 向主世界发布级联排队与真实阴影命令证据。
    grass_patches: Query<
        'w,
        's,
        (
            &'static GpuBladePatch, // 检查获准叶片内容、LOD0身份和本帧实例上限。
            Option<&'static GpuBladePreparedBuffers>, // 复用已经生成且有效的紧凑实例与八桶间接参数。
        ),
    >, // 普通网格不会进入项目自有近景草阴影分支。
    shadow_phases: ResMut<'w, ViewBinnedRenderPhases<Shadow>>, // 写入每个方向光级联自己的阴影深度分箱。
    gameplay_views: Query<
        'w,
        's,
        (Entity, &'static ViewLightEntities), // 由唯一玩法草地相机取得其方向光级联视图。
        (With<ExtractedView>, With<GpuBladeView>),
    >, // 小场景和其他辅助相机不重复生成近景草阴影。
    shadow_views: Query<'w, 's, (&'static LightEntity, &'static ExtractedView)>, // 将级联实体映射到保留视图ID。
    directional_lights: Query<
        'w,
        's,
        &'static RenderShadowMapVisibleEntities, // 复用Bevy对当前方向光各级联完成的草窗可见性结果。
        With<ExtractedDirectionalLight>,
    >, // 点光与聚光阴影不在本轮近景草范围内。
} // 完成近景草方向光阴影排队参数分组。

/// 为每个玩法视图把 GPU 草地LOD窗口加入AlphaMask深度写入阶段。
pub(super) fn queue_gpu_grass(
    params: GpuBladeQueueParams, // 具名资源与查询组替代十二项位置参数并保留原借用边界。
) {
    let GpuBladeQueueParams {
        draw_functions,            // 解构草地自定义绘制命令注册表。
        grass_pipeline,            // 解构草地基础MeshPipeline。
        mut specialized_pipelines, // 解构可写专用管线缓存。
        pipeline_cache,            // 解构全局异步管线缓存。
        meshes,                    // 解构GPU网格资产表。
        render_mesh_instances,     // 解构主世界网格实例映射。
        mesh_allocator,            // 解构共享网格缓冲分配器。
        grass_patches,             // 解构草窗组件查询。
        render_stats,              // 解构本帧诊断计数器。
        view_key_cache,            // 解构 Bevy 已准备的完整视图专用化键缓存。
        mut render_phases,         // 解构可写AlphaMask分箱阶段。
        views,                     // 解构唯一玩法视图查询。
    } = params; // 后续排队主体继续使用短语义名称并保持原执行顺序。
    let draw_gpu_grass = draw_functions.read().id::<DrawGpuBlade>(); // 获取本插件绘制命令的稳定 ID。
    for (view, visible_entities, has_deferred_prepass) in &views {
        render_stats.record_target_view(); // 每个带GpuBladeView标记的视图都进入监控分母。
        let Some(phase) = render_phases.get_mut(&view.retained_view_entity) else {
            render_stats.record_missing_phase_view(); // 明确暴露AlphaMask3d阶段尚未建立的视图阻塞。
            continue; // 该视图没有遮罩3D阶段时无需排队。
        }; // 取得当前视图的AlphaMask分箱阶段。
        let Some(visible_meshes) = visible_entities.get::<Mesh3d>() else {
            continue; // 当前玩法视图没有任何Mesh3d可见类别时保持空阶段。
        };
        // Bevy 标准材质队列已在本系统之前统一清理共享 AlphaMask 分箱；这里不得再次遍历全体脏实体，
        // 否则会把刚由 TreeWindMaterial 等标准材质排回的树木和灌木颜色项误删，只留下独立阴影阶段。
        if !gpu_grass_uses_forward_main_pass(has_deferred_prepass) {
            for (render_entity, main_entity) in visible_meshes.iter_visible() {
                if grass_patches.get(*render_entity).is_err() {
                    continue; // 普通网格不是GPU草窗时不得从标准材质主阶段移除。
                } // 只处理本插件拥有的可见叶片窗口。
                render_stats.record_visible_window(); // Deferred排队仍复用主队列的可见窗口统计口径。
                phase.remove(*main_entity); // 关闭overlay策略时清掉跨帧保留的自定义前向颜色项。
            } // 完成当前Deferred视图全部可见GPU草窗的旧主颜色项清理。
            continue; // 关闭overlay策略时草叶最终颜色完全交给Solari；当前黑化修复候选始终启用overlay。
        } // 所有需要植被专用最终颜色的视图继续专用化并排队自定义管线。
        let Some(view_key) = view_key_cache.get(&view.retained_view_entity).copied() else {
            continue; // Bevy 尚未完成该视图的标准布局专用化时等待下一帧，禁止构造不完整绑定布局。
        }; // 复用 tonemap、预通道、环境贴图等全部布局位，确保 group0 与实际绑定组严格兼容。
        for (render_entity, main_entity) in visible_meshes.iter_visible() {
            let Ok((patch, prepared_buffers)) = grass_patches.get(*render_entity) else {
                continue; // 普通可见网格不是草块时立即跳过。
            }; // 取得当前可见草块参数。
            render_stats.record_visible_window(); // 在实例数快返前记录视锥内草窗口，便于区分地表纹理等待。
            if !super::output_readiness::queue_output_is_available(
                patch.instance_count(),
                prepared_buffers.is_some(),
            )
            // Queue 早于 Prepare：只检查缓冲分配，最终 Draw 仍拒绝本帧过期输出。
            {
                continue; // 空批次或Compute输出尚未建立时不创建无意义间接绘制项。
            } // 完成实例数量快返。
            let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(*main_entity)
            else {
                render_stats.record_missing_mesh_instance(); // 显示主世界Mesh3d提取尚未完成的窗口数。
                continue; // 主世界网格尚未同步时等待下一帧。
            }; // 取得该批次的网格资产与世界中心。
            let mesh_asset_id = mesh_instance.mesh_asset_id(); // 0.19通过原子安全访问器读取共享网格资产身份。
            let Some(mesh) = meshes.get(mesh_asset_id) else {
                render_stats.record_missing_gpu_mesh(); // 显示草原型RenderMesh仍在GPU上传的窗口数。
                continue; // 原型网格尚未上传 GPU 时等待下一帧。
            }; // 取得 GPU 网格布局与拓扑。
            let Some(mesh_slabs) = mesh_allocator.mesh_slabs(&mesh_asset_id) else {
                render_stats.record_missing_gpu_mesh(); // 分配器尚未发布共享页时与网格上传等待使用同一诊断门。
                continue;
            }; // 读取Bevy共享网格缓冲页供AlphaMask稳定分箱。
            let pipeline_key = view_key
                | MeshPipelineKey::from_primitive_topology_and_strip_index(
                    mesh.primitive_topology(),
                    mesh.index_format(),
                ); // 为原型真实拓扑和条带索引格式专用化管线。
            let Ok(pipeline) = specialized_pipelines.specialize(
                &pipeline_cache, // 使用全局异步管线缓存。
                &grass_pipeline, // 使用项目自有草地基础管线。
                pipeline_key,    // 区分当前视图和原型拓扑。
                &mesh.layout,    // 保持标准 position/normal/uv 顶点布局。
            ) else {
                render_stats.record_specialization_error(); // 布局不兼容必须在F3可见，不能只表现为远圈消失。
                continue; // 布局不兼容时跳过本帧，由后续日志暴露具体错误。
            }; // 获取已缓存或正在编译的专用管线 ID。
            phase.add(
                OpaqueNoLightmap3dBatchSetKey {
                    pipeline,                        // 使用当前视图、MSAA、HDR和拓扑专用草地管线。
                    draw_function: draw_gpu_grass, // 一次自定义命令绑定紧凑实例并执行每LOD八个间接深度桶。
                    material_bind_group_index: None, // 草色、LOD和光照全部来自实例与视图，不创建逐圈材质绑定。
                    slabs: mesh_slabs, // 原型顶点与可选索引共享页共同参与分箱稳定性判断。
                }, // 完成AlphaMask批次集合键。
                OpaqueNoLightmap3dBinKey {
                    asset_id: mesh_asset_id.into(), // 同一grass.glb原型使用稳定资产分箱键。
                }, // 完成AlphaMask批次内分箱键。
                (*render_entity, *main_entity), // 让绘制命令同时访问渲染实体和主世界网格实例。
                mesh_instance.current_uniform_index, // SetMeshBindGroup继续取得当前草圈标准网格uniform索引。
                BinnedRenderPhaseType::UnbatchableMesh, // 每个LOD窗口拥有独立间接缓冲，禁止Bevy跨窗口自动合并。
            ); // 完成当前草地LOD窗口AlphaMask排队。
            render_stats.record_queued_draw(patch.instance_count()); // 记录AlphaMask阶段接收的阶段项与候选实例数。
            match pipeline_cache.get_render_pipeline_state(pipeline) {
                // 区分已经排队但WGSL管线尚不可执行的情况。
                CachedPipelineState::Ok(_) => render_stats.record_pipeline_ready(
                    patch.profile_slot().index(), // 不同profile槽的同级管线分别证明内容完整性。
                    patch.lod_level(),            // GPU管线已经创建完成并记录对应近中远身份。
                ),
                CachedPipelineState::Queued | CachedPipelineState::Creating(_) => {
                    // 首次进入场景时允许异步创建若干帧。
                    render_stats.record_pipeline_waiting(); // 把排队中的管线单独显示在F3。
                } // 完成管线等待分支。
                CachedPipelineState::Err(
                    ShaderCacheError::ShaderNotLoaded(_)
                    | ShaderCacheError::ShaderImportNotYetAvailable,
                ) => render_stats.record_pipeline_waiting(), // Shader资产或导入尚未进入缓存时下一帧会自动重试。
                CachedPipelineState::Err(_) => render_stats.record_pipeline_error(), // 处理、编译或设备错误才计入永久失败。
            } // 完成当前绘制项的管线状态统计。
        } // 完成当前视图的全部草地LOD窗口排队。
    } // 完成全部 3D 视图排队。
}

/// 把全部GPU叶片植物的LOD0紧凑输出加入唯一玩法相机的方向光级联阴影阶段。
pub(super) fn queue_gpu_grass_shadows(
    params: GpuBladeShadowQueueParams, // 具名参数集封装方向光级联、草窗和阴影管线资源。
) {
    let GpuBladeShadowQueueParams {
        draw_functions,            // 解构Shadow绘制命令注册表。
        shadow_pipeline,           // 解构首帧可缺省的近景草阴影管线。
        mut specialized_pipelines, // 解构可写阴影专用化缓存。
        pipeline_cache,            // 解构全局异步管线缓存。
        meshes,                    // 解构GPU原型网格资产表。
        render_mesh_instances,     // 解构主世界网格实例映射。
        mesh_allocator,            // 解构共享网格缓冲分配器。
        render_stats,              // 解构近景草阴影诊断计数器。
        grass_patches,             // 解构GPU植被窗口及其紧凑输出查询。
        mut shadow_phases,         // 解构可写方向光级联Shadow阶段。
        gameplay_views,            // 解构唯一玩法草地相机及其光照子视图。
        shadow_views,              // 解构级联实体到方向光身份和保留视图的映射。
        directional_lights,        // 解构方向光逐相机逐级联可见实体集合。
    } = params; // 后续排队主体使用短语义名称并保留明确借用边界。
    let Some(shadow_pipeline) = shadow_pipeline else {
        return; // 首个Render帧尚未从Bevy预通道构造模板时等待下一帧。
    }; // 取得可供专用化的近景草阴影管线。
    let draw_gpu_grass_shadow = draw_functions.read().id::<DrawGpuBladeShadow>(); // 获取Shadow阶段稳定绘制命令ID。
    for (_gameplay_view_entity, view_lights) in &gameplay_views {
        for shadow_view_entity in view_lights.lights.iter().copied() {
            let Ok((light_entity, extracted_shadow_view)) = shadow_views.get(shadow_view_entity)
            else {
                continue; // 光照子视图已经失效时等待Bevy下一帧重建列表。
            }; // 取得当前Shadow视图的光源类型和保留视图ID。
            let LightEntity::Directional {
                light_entity: directional_light_entity,
                cascade_index: _,
            } = light_entity
            else {
                continue; // 本轮只给主要太阳/月亮方向光投射近景草阴影。
            }; // 取得当前方向光实体和级联编号。
            let Ok(cascades) = directional_lights.get(*directional_light_entity) else {
                continue; // 方向光级联可见性尚未准备完成时等待下一帧。
            }; // 取得当前方向光对全部玩法相机的可见实体集合。
            let Some((_, visible_entities)) = cascades
                .subviews
                .get(&extracted_shadow_view.retained_view_entity)
            else {
                continue; // 当前方向光尚未为该玩法相机级联发布可见实体时安全等待下一帧。
            }; // 按保留阴影视图直接索引每个级联的可见集合。
            let Some(shadow_phase) =
                shadow_phases.get_mut(&extracted_shadow_view.retained_view_entity)
            else {
                continue; // Bevy尚未为当前级联建立Shadow分箱阶段时等待下一帧。
            }; // 取得当前方向光级联的可写阴影阶段。
            let Some(visible_meshes) = visible_entities.get::<Mesh3d>() else {
                continue; // 当前级联没有Mesh3d可见类别时无需建立空阴影项。
            };
            for (render_entity, main_entity) in visible_meshes.iter_visible() {
                let Ok((patch, prepared_buffers)) = grass_patches.get(*render_entity) else {
                    continue; // 普通场景网格交给Bevy标准材质阴影系统处理。
                }; // 取得当前可见GPU植被窗口。
                if !casts_near_blade_shadow(patch)
                    || !super::output_readiness::queue_output_is_available(
                        patch.instance_count(),
                        prepared_buffers.is_some(),
                    )
                // 阴影也须允许本帧 Prepare 恢复，最终命令严格核对有效性。
                {
                    continue; // 中远级、空窗口或未完成Compute的窗口不进入近景植物阴影预算。
                } // 完成全部植物profile的近景档与有效紧凑输出资格门。
                let Some(mesh_instance) =
                    render_mesh_instances.render_mesh_queue_data(*main_entity)
                else {
                    continue; // LOD0原型主网格实例尚未提取时等待下一帧。
                }; // 取得近景草原型资产和网格uniform索引。
                let mesh_asset_id = mesh_instance.mesh_asset_id(); // 使用0.19原子安全共享实例访问器。
                let Some(mesh) = meshes.get(mesh_asset_id) else {
                    continue; // LOD0原型尚未上传GPU时等待下一帧。
                }; // 取得GPU原型布局和拓扑。
                let pipeline_key = MeshPipelineKey::DEPTH_PREPASS
                    | MeshPipelineKey::UNCLIPPED_DEPTH_ORTHO
                    | MeshPipelineKey::from_primitive_topology_and_strip_index(
                        mesh.primitive_topology(),
                        mesh.index_format(),
                    ); // 方向光阴影固定使用正交未裁剪深度和原型真实拓扑。
                let Ok(pipeline) = specialized_pipelines.specialize(
                    &pipeline_cache,  // 使用Bevy全局异步管线缓存。
                    &shadow_pipeline, // 使用只写深度的近景草阴影模板。
                    pipeline_key,     // 区分方向光深度能力和原型拓扑。
                    &mesh.layout,     // 校验position、normal和uv顶点布局。
                ) else {
                    continue; // 原型布局不兼容时由Bevy管线错误日志暴露具体原因。
                }; // 取得已缓存或正在异步编译的近景草阴影管线ID。
                let Some(mesh_slabs) = mesh_allocator.mesh_slabs(&mesh_asset_id) else {
                    continue; // 网格已上传但共享页尚未建立时等待下一帧。
                }; // 读取原型共享缓冲页用于Shadow稳定分箱。
                shadow_phase.add(
                    ShadowBatchSetKey {
                        pipeline,                             // 当前方向光深度和原型拓扑专用管线。
                        draw_function: draw_gpu_grass_shadow, // 绑定紧凑实例并执行八桶间接阴影绘制。
                        material_bind_group_index: None, // 草阴影只读取全局交互，不创建逐窗口材质绑定。
                        slabs: mesh_slabs,               // 原型顶点与可选索引共享页进入批次键。
                    }, // 完成近景草Shadow批次集合键。
                    ShadowBinKey {
                        asset_id: mesh_asset_id.into(), // LOD0草原型资产作为稳定阴影分箱键。
                    }, // 完成Shadow批次内分箱键。
                    (*render_entity, *main_entity), // 绘制命令访问渲染窗口和主世界网格实例。
                    mesh_instance.current_uniform_index, // SetMeshBindGroup读取当前草窗标准网格uniform。
                    BinnedRenderPhaseType::UnbatchableMesh, // 每个窗口拥有独立间接缓冲，禁止与普通网格自动合并。
                ); // 完成当前方向光级联的近景GPU叶片植物阴影排队。
                render_stats.record_queued_shadow_cascade(); // 记录当前LOD0窗口确实进入一个方向光级联Shadow阶段。
            } // 完成当前级联全部可见实体筛选。
        } // 完成玩法相机关联的全部阴影子视图。
    } // 完成全部明确标记玩法草地相机。
} // 结束全部近景GPU叶片植物的方向光阴影排队。

#[cfg(test)] // 纯逻辑测试锁定“全部植物profile且仅LOD0”阴影预算边界。
mod tests {
    use super::*; // 复用近景草阴影资格判断和GPU草窗构造器。

    #[test] // Deferred/Solari视图必须在完整GBuffer之后恢复植被专用最终颜色，避免严格单面PBR压黑薄叶。
    fn deferred_view_preserves_forward_grass_color_after_full_gbuffer() {
        assert!(gpu_grass_uses_forward_main_pass(false)); // 普通光栅相机继续从AlphaMask3d取得草叶颜色。
        assert!(gpu_grass_uses_forward_main_pass(true)); // Solari相机写完GBuffer后仍由同一植被光照覆盖最终颜色。
    } // 结束草叶最终颜色所有权契约测试。

    #[test] // 共享阶段只能由Bevy标准队列统一清理；自定义草地随后补项，不得再误删树木和灌木。
    fn gpu_blade_queues_rely_on_bevy_shared_phase_cleanup() {
        let render_plugin = include_str!("../render.rs"); // 锁定真实RenderApp系统顺序，不复制测试专用调度。
        let main_queue = include_str!("queue.rs"); // 审计主颜色队列不得重新取得全局脏实体集合。
        let prepass_queue = include_str!("prepass.rs"); // 同一所有权约束也覆盖普通预通道与Deferred阶段。
        let global_dequeue_call = concat!("iter_to_", "dequeue"); // 拆分字面量，避免测试源码自身命中禁用调用。

        assert!(render_plugin.contains(".after(queue_material_meshes)")); // 官方主材质先删除全部过期分箱并排回树木。
        assert!(render_plugin.contains(".after(queue_prepass_material_meshes)")); // 官方预通道同样先完成共享阶段清理。
        assert!(!main_queue.contains(global_dequeue_call)); // GPU草地主队列不得再删除TreeWindMaterial的AlphaMask项。
        assert!(!prepass_queue.contains(global_dequeue_call)); // GPU草地预通道也不得删除其他材质的时间输入项。
    } // 结束共享阶段所有权回归测试。

    #[test] // 四个合法profile的近景档全部投影，中远级继续保持关闭。
    fn all_profile_near_lod_blades_cast_shadow() {
        let ordinary_near = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 1).with_lod_level(0); // SLOT0覆盖普通草或湿地草的近景窗口。
        let ordinary_middle = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 2).with_lod_level(1); // 任意profile的LOD1都必须退出阴影预算。
        let wheat_near = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 3)
            .with_lod_level(0)
            .with_vegetation_config(crate::types::GpuBladeConfig::new(
                crate::types::GpuBladeProfileSlot::SLOT_1, // SLOT1承载当前正式场景麦田内容。
                1.35,                                      // 高秆最高叶尖不影响近景档阴影规则。
                0.72,                                      // 保守水平扫掠只参与剔除。
                1.18,                                      // 典型参考高度只参与屏幕LOD。
                0.04,                                      // 根部下扎不参与阴影资格判断。
            )); // 构造必须投影的麦田LOD0。
        let reed_near = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 4)
            .with_lod_level(0)
            .with_vegetation_config(crate::types::GpuBladeConfig::new(
                crate::types::GpuBladeProfileSlot::SLOT_2, // SLOT2承载HN-02芦苇内容。
                1.90,                                      // 芦苇高度不改变近景档阴影资格。
                0.72,                                      // 保守水平扫掠只参与剔除。
                1.75,                                      // 典型参考高度只参与屏幕LOD。
                0.04,                                      // 根部下扎不参与阴影资格判断。
            )); // 构造现在必须投影的芦苇LOD0。
        let mixed_grain_near = GpuBladePatch::square(Vec3::ZERO, 64.0, 65, 5)
            .with_lod_level(0)
            .with_vegetation_config(crate::types::GpuBladeConfig::new(
                crate::types::GpuBladeProfileSlot::SLOT_3, // SLOT3覆盖杂粮田或HN-02沙洲矮草。
                1.10,                                      // 内容高度不改变近景档阴影资格。
                0.72,                                      // 保守水平扫掠只参与剔除。
                0.95,                                      // 典型参考高度只参与屏幕LOD。
                0.04,                                      // 根部下扎不参与阴影资格判断。
            )); // 构造现在必须投影的杂粮田或沙洲矮草LOD0。

        assert!(casts_near_blade_shadow(&ordinary_near)); // 普通草LOD0必须进入方向光阴影阶段。
        assert!(!casts_near_blade_shadow(&ordinary_middle)); // 普通草LOD1不得增加中景阴影成本。
        assert!(casts_near_blade_shadow(&wheat_near)); // 麦田LOD0必须复用既有GPU叶片阴影管线。
        assert!(casts_near_blade_shadow(&reed_near)); // 芦苇LOD0必须进入同一方向光阴影管线。
        assert!(casts_near_blade_shadow(&mixed_grain_near)); // 杂粮田和沙洲矮草共享的SLOT3近景也必须投影。
    } // 结束近景叶片阴影预算回归测试。

    #[test] // 官方dirty分箱会先删除旧主实体，自定义LOD0阴影只能在其后重新加入。
    fn gpu_blade_shadow_queue_runs_after_bevy_shadow_cleanup() {
        let render_plugin = include_str!("../render.rs"); // 锁定真实RenderApp注册语义，不复制一套测试调度。
        assert!(render_plugin.contains("queue_gpu_grass_shadows")); // 自定义草影排队仍然存在。
        assert!(render_plugin.contains(".after(queue_shadows)")); // 官方清理必须先完成，避免恢复首帧丢草影。
    } // 结束阴影队列相对顺序回归。
} // 结束近景草阴影排队测试模块。
