//! GPU草地AlphaMask阶段自定义绘制命令。
//! 每个LOD窗口绑定一次紧凑实例流，再按由近到远的八个深度桶执行间接索引绘制。

use bevy::{
    ecs::system::{SystemParamItem, lifetimeless::*}, // 自定义RenderCommand的无生命周期系统参数。
    pbr::{
        RenderMeshInstances,              // 由阶段主实体定位对应原型网格资产ID。
        SetMeshBindGroup,                 // 绑定当前草地窗口的标准网格变换数据。
        SetMeshViewBindGroup,             // 绑定相机、视图与光照基础数据。
        SetMeshViewBindingArrayBindGroup, // 绑定Bevy 0.18视图数组数据。
        SetPrepassViewBindGroup,          // 阴影通道绑定方向光级联视图和全局时间。
        SetPrepassViewEmptyBindGroup,     // 阴影预通道group1保持Bevy空布局约定。
        Shadow,                           // 近景草额外进入Bevy方向光阴影深度阶段。
    },
    render::{
        mesh::{
            RenderMesh,               // 读取已经上传GPU的原型网格。
            RenderMeshBufferInfo,     // 读取索引格式并拒绝非索引间接ABI。
            allocator::MeshAllocator, // 读取原型网格共享顶点和索引缓冲切片。
        },
        render_asset::RenderAssets, // 从资产ID取得已经上传GPU的原型网格。
        render_phase::{
            PhaseItem,           // 由阶段项取得渲染实体和主世界实体。
            RenderCommand,       // 实现全局绑定组和草地间接绘制命令。
            RenderCommandResult, // 在资源等待时安全跳过当前阶段项。
            SetItemPipeline,     // 设置当前视图专用草地渲染管线。
            TrackedRenderPass,   // 在Bevy跟踪的AlphaMask编码器中绑定与绘制。
        },
    },
};

use super::interaction::GpuBladeInteractionGpu; // 所有LOD窗口共享同一风场、足迹和基础颜色绑定组。
use crate::components::{
    GpuBladePatch,           // 读取候选遥测和LOD编号。
    GpuBladePreparedBuffers, // 读取紧凑实例、深度桶间接命令和GPU输出生命周期。
    GpuBladeProfileBankId,   // 按窗口会话身份选择对应四槽 profile 与纹理页绑定组。
};
use crate::constants::{
    GPU_GRASS_DEPTH_BUCKET_COUNT, // 当前每个LOD固定执行八个由近到远的深度桶。
    GPU_GRASS_DEPTH_BUCKET_STATE_BYTES, // 每条间接命令按固定三十二字节状态步长排列。
};
use crate::resources::{
    GpuBladeRenderFrameCounters, // 只有真正编码绘制的窗口才计入F3实绘统计。
    GpuBladeTimingQueries,       // 在当前LOD绘制前后写GPU时间戳和管线统计查询。
};

/// AlphaMask阶段绘制命令：设置标准管线和绑定组后执行当前LOD的八个间接深度桶。
pub(super) type DrawGpuBlade = (
    SetItemPipeline,                     // 设置当前视图专用草地渲染管线。
    SetMeshViewBindGroup<0>,             // 绑定相机、视图与光照基础数据。
    SetMeshViewBindingArrayBindGroup<1>, // 绑定 Bevy 0.18 的视图数组数据。
    SetMeshBindGroup<2>,                 // 绑定当前LOD窗口实体的网格变换数据。
    SetGpuBladeInteractionBindGroup<3>,  // 绑定全部近中远LOD共享的动态uniform、原草纹理和采样器。
    DrawGpuBladeInstanced,               // 绑定紧凑实例槽并执行八次draw_indexed_indirect。
);

/// Shadow阶段绘制命令：复用LOD0紧凑实例与风场绑定，只写方向光级联深度。
pub(super) type DrawGpuBladeShadow = (
    SetItemPipeline,                    // 设置近景草方向光深度专用管线。
    SetPrepassViewBindGroup<0>,         // group0绑定当前方向光级联视图和全局时间。
    SetPrepassViewEmptyBindGroup<1>,    // group1绑定Bevy预通道空组。
    SetMeshBindGroup<2>,                // group2绑定当前近景草窗口网格uniform。
    SetGpuBladeInteractionBindGroup<3>, // group3绑定与主画面相同的风场、足迹和内容profile。
    DrawGpuBladeShadowInstanced,        // 复用Compute紧凑输出和八条间接命令写入阴影图。
);

/// 把持久全局草地动态uniform与基础颜色纹理绑定到专用管线group3。
pub(super) struct SetGpuBladeInteractionBindGroup<const I: usize>;

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetGpuBladeInteractionBindGroup<I> {
    type Param = SRes<GpuBladeInteractionGpu>; // 所有绘制项共享同一批次目录和全局交互缓冲。
    type ViewQuery = (); // 风场与玩家足迹不依赖具体相机视图。
    type ItemQuery = Read<GpuBladeProfileBankId>; // 每个窗口只读取四字节会话批次身份。

    #[inline]
    fn render<'w>(
        _item: &P,                                  // 当前阶段项不参与全局绑定组选择。
        _view: (),                                  // 不读取视图组件。
        bank_id: Option<&'w GpuBladeProfileBankId>, // 取得当前草窗所属四槽批次。
        gpu: SystemParamItem<'w, '_, Self::Param>,  // 取得准备阶段创建或更新的持久GPU资源。
        pass: &mut TrackedRenderPass<'w>,           // 当前AlphaMask草地绘制编码器。
    ) -> RenderCommandResult {
        let gpu = gpu.into_inner(); // 解开只读系统参数包装以访问可选绑定组。
        let Some(bank_id) = bank_id else {
            return RenderCommandResult::Skip; // 提取关系失效时禁止猜测批次并读错外观。
        }; // 取得已由 GpuBladePatch require 契约保证存在的批次组件。
        let Some(bind_group) = gpu.bind_group(*bank_id) else {
            return RenderCommandResult::Skip; // 只有越界或本批纹理等待上传的窗口被跳过。
        };
        pass.set_bind_group(I, bind_group, &[]); // group3没有动态offset，同批窗口复用同一绑定。
        RenderCommandResult::Success // 通知后续间接绘制命令继续执行。
    }
}

/// 读取原型网格切片、紧凑实例缓冲和GPU生成的八条间接参数执行当前LOD绘制。
pub(super) struct DrawGpuBladeInstanced;

impl<P: PhaseItem> RenderCommand<P> for DrawGpuBladeInstanced {
    type Param = (
        SRes<RenderAssets<RenderMesh>>, // 读取原型网格的索引格式和索引数量。
        SRes<RenderMeshInstances>,      // 由阶段主实体定位对应网格资产 ID。
        SRes<MeshAllocator>,            // 获取共享 GPU 顶点和索引缓冲区切片。
        SRes<GpuBladeRenderFrameCounters>, // 只有真正执行到本命令的窗口才计入F3实绘数量。
        SRes<GpuBladeTimingQueries>, // 支持时在当前LOD间接绘制前后写GPU时间戳并统计顶点与片元调用量。
    );
    type ViewQuery = (); // 绘制本身不额外读取视图组件。
    type ItemQuery = (Read<GpuBladePatch>, Read<GpuBladePreparedBuffers>); // 同时取得候选遥测和当前LOD窗口GPU输出。

    #[inline]
    fn render<'w>(
        item: &P,  // 当前AlphaMask阶段草地绘制项。
        _view: (), // 视图绑定已由前置命令完成。
        patch_and_buffers: Option<(&'w GpuBladePatch, &'w GpuBladePreparedBuffers)>, // 当前LOD候选参数和持久GPU输出。
        (meshes, render_mesh_instances, mesh_allocator, render_stats, timing_queries): SystemParamItem<
            'w,
            '_,
            Self::Param,
        >, // 渲染资源集合与当前帧无锁实绘计数器。
        pass: &mut TrackedRenderPass<'w>, // 当前3D AlphaMask阶段编码器。
    ) -> RenderCommandResult {
        let Some((patch, prepared_buffers)) = patch_and_buffers.filter(|(patch, buffers)| {
            super::output_readiness::draw_output_is_current(
                patch.instance_count(),
                buffers.output_valid,
            )
        }) else {
            // Prepare 可在 Queue 后使分页修订失效，绘制前再次拒绝旧输出。
            return RenderCommandResult::Skip; // 提取参数或Compute输出缺失时安全跳过该绘制项。
        }; // 取得候选遥测与间接实例资源。
        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(item.main_entity())
        else {
            return RenderCommandResult::Skip; // 主世界网格实例已失效时跳过。
        }; // 取得原型网格资产 ID。
        let mesh_asset_id = mesh_instance.mesh_asset_id(); // 0.19共享实例资产ID必须通过原子安全访问器取得。
        let Some(gpu_mesh) = meshes.into_inner().get(mesh_asset_id) else {
            return RenderCommandResult::Skip; // GPU 网格尚未就绪时跳过。
        }; // 取得索引信息。
        let mesh_allocator = mesh_allocator.into_inner(); // 释放系统参数包装，便于连续借用缓冲切片。
        let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_asset_id) else {
            return RenderCommandResult::Skip; // 顶点缓冲切片不存在时跳过。
        }; // 取得原型顶点范围。
        pass.set_vertex_buffer(0, vertex_slice.buffer.slice(..)); // 绑定 Bevy 共享顶点缓冲区。
        pass.set_vertex_buffer(1, prepared_buffers.instances.slice(..)); // 第二槽直接读取Compute写出的连续二十字节实例。
        let timing_queries = timing_queries.into_inner(); // 解开一次只读系统参数包装，供绘制前后配对访问同一查询资源。
        match &gpu_mesh.buffer_info {
            RenderMeshBufferInfo::Indexed {
                index_format, // 原型网格索引格式。
                .. // index_count、first_index与base_vertex已经预写在每条间接状态前20字节。
            } => {
                let Some(index_slice) =
                    mesh_allocator.mesh_index_slice(&mesh_asset_id)
                else {
                    return RenderCommandResult::Skip; // 索引切片不存在时跳过。
                }; // 取得原型索引范围。
                pass.set_index_buffer(index_slice.buffer.slice(..), *index_format); // 绑定 Bevy 共享索引缓冲区。
                let timing_scope = crate::gpu_timing::begin_draw_measurement(
                    timing_queries, // 取得固定查询集和本帧无锁写入掩码。
                    pass, // 查询直接写在草地AlphaMask间接绘制所在通道。
                    patch.profile_slot().index(), // 不同profile使用彼此独立的计时和统计查询槽。
                    patch.lod_level(), // 近中远零至二编号直接映射固定查询槽。
                ); // 开始当前LOD的GPU耗时与管线调用统计。
                for depth_bucket_index in 0..GPU_GRASS_DEPTH_BUCKET_COUNT {
                    let indirect_offset = u64::from(depth_bucket_index)
                        * GPU_GRASS_DEPTH_BUCKET_STATE_BYTES; // 零至七号桶严格对应由近到远的固定间接记录。
                    pass.draw_indexed_indirect(
                        &prepared_buffers.depth_bucket_draws, // GPU已经按平方距离压入当前桶的连续实例分区。
                        indirect_offset, // 三十二字节步长跳过上一桶的参数与原子状态。
                    ); // 空桶由零instance_count自然快速跳过，不需要CPU回读或条件分支。
                } // 完成当前LOD环带内部从近到远的八次深度友好绘制。
                crate::gpu_timing::end_draw_measurement(
                    timing_queries, // 与开始阶段使用完全相同的查询资源。
                    pass, // 在同一AlphaMask通道内结束统计并写绘制结束时间戳。
                    timing_scope, // 只结束本次真实启用的查询类型。
                ); // 完成当前LOD的精确GPU绘制测量。
            }
            RenderMeshBufferInfo::NonIndexed => {
                return RenderCommandResult::Skip; // 当前深度桶状态前20字节固定使用索引间接ABI，不允许错误回退到候选直绘。
            }
        } // 完成原型网格类型分支。
        render_stats.into_inner().record_draw(
            patch.instance_count(),
            GPU_GRASS_DEPTH_BUCKET_COUNT,
            patch.profile_slot().index(), // 不同profile的同级窗口分别补齐内容完整性位。
            patch.lod_level(),
        ); // 当前LOD八条命令编码成功后记录真实命令、候选上限和近中远身份。
        RenderCommandResult::Success // 通知渲染阶段本绘制命令已经编码成功。
    }
}

/// 近景草阴影绘制只复用主画面已经生成的紧凑实例，不重复计时或触发Compute。
pub(super) struct DrawGpuBladeShadowInstanced;

impl RenderCommand<Shadow> for DrawGpuBladeShadowInstanced {
    type Param = (
        SRes<RenderAssets<RenderMesh>>, // 读取LOD0草原型的索引格式与GPU资产状态。
        SRes<RenderMeshInstances>,      // 由Shadow阶段主实体定位近景草原型资产ID。
        SRes<MeshAllocator>,            // 读取Bevy共享原型顶点与索引缓冲切片。
        SRes<GpuBladeRenderFrameCounters>, // 记录真正进入方向光阴影编码器的间接命令数量。
    ); // 阴影命令不写主画面绘制计时，避免四级联重复占用固定查询槽。
    type ViewQuery = (); // 方向光视图绑定已由前置预通道命令完成。
    type ItemQuery = (Read<GpuBladePatch>, Read<GpuBladePreparedBuffers>); // 读取LOD0身份和已生成的紧凑输出。

    #[inline]
    fn render<'w>(
        item: &Shadow, // 当前方向光级联Shadow阶段中的近景草项。
        _view: (),     // 视图uniform和全局时间已由SetPrepassViewBindGroup绑定。
        patch_and_buffers: Option<(&'w GpuBladePatch, &'w GpuBladePreparedBuffers)>, // 当前近景草窗口与持久紧凑输出。
        (meshes, render_mesh_instances, mesh_allocator, render_stats): SystemParamItem<
            'w,
            '_,
            Self::Param,
        >, // 取得原型网格、共享GPU缓冲和阴影诊断计数器。
        pass: &mut TrackedRenderPass<'w>, // 当前方向光级联阴影深度编码器。
    ) -> RenderCommandResult {
        let Some((_patch, prepared_buffers)) = patch_and_buffers.filter(|(patch, buffers)| {
            super::output_readiness::draw_output_is_current(
                patch.instance_count(),
                buffers.output_valid,
            )
        }) else {
            // 阴影同样不得消费排队后失效的旧高度输出。
            return RenderCommandResult::Skip; // 窗口或Compute输出在排队后失效时安全跳过。
        }; // 取得本级联复用的近景草紧凑输出。
        let Some(mesh_instance) = render_mesh_instances.render_mesh_queue_data(item.main_entity())
        else {
            return RenderCommandResult::Skip; // 主世界近景草网格实例已经失效时跳过。
        }; // 取得LOD0草原型资产ID。
        let mesh_asset_id = mesh_instance.mesh_asset_id(); // 0.19共享实例资产ID必须通过原子安全访问器取得。
        let Some(gpu_mesh) = meshes.into_inner().get(mesh_asset_id) else {
            return RenderCommandResult::Skip; // 原型网格尚未上传GPU时等待下一帧。
        }; // 取得原型索引格式。
        let mesh_allocator = mesh_allocator.into_inner(); // 解开共享网格分配器只读包装。
        let Some(vertex_slice) = mesh_allocator.mesh_vertex_slice(&mesh_asset_id) else {
            return RenderCommandResult::Skip; // 原型顶点缓冲切片不存在时安全跳过。
        }; // 取得近景草原型顶点范围。
        pass.set_vertex_buffer(0, vertex_slice.buffer.slice(..)); // slot0绑定原型草叶顶点。
        pass.set_vertex_buffer(1, prepared_buffers.instances.slice(..)); // slot1复用Compute生成的二十字节紧凑实例。
        let RenderMeshBufferInfo::Indexed { index_format, .. } = &gpu_mesh.buffer_info else {
            return RenderCommandResult::Skip; // 草地八桶间接状态固定使用索引绘制ABI。
        }; // 读取原型索引格式。
        let Some(index_slice) = mesh_allocator.mesh_index_slice(&mesh_asset_id) else {
            return RenderCommandResult::Skip; // 原型索引缓冲切片不存在时安全跳过。
        }; // 取得近景草原型索引范围。
        pass.set_index_buffer(index_slice.buffer.slice(..), *index_format); // 绑定Bevy共享索引缓冲。
        for depth_bucket_index in 0..GPU_GRASS_DEPTH_BUCKET_COUNT {
            let indirect_offset =
                u64::from(depth_bucket_index) * GPU_GRASS_DEPTH_BUCKET_STATE_BYTES; // 与主画面使用同一八桶三十二字节间接记录。
            pass.draw_indexed_indirect(
                &prepared_buffers.depth_bucket_draws, // 复用Compute已写入的LOD0实例数量与分桶起点。
                indirect_offset,                      // 逐桶跳到对应间接索引绘制参数。
            ); // 空桶通过零实例参数自然跳过，不进行CPU回读。
        } // 完成当前方向光级联的全部近景草阴影间接绘制。
        render_stats
            .into_inner()
            .record_shadow_draw(GPU_GRASS_DEPTH_BUCKET_COUNT); // 当前级联八条间接命令全部成功编码后发布真实阴影证据。
        RenderCommandResult::Success // 通知Shadow阶段当前近景草深度已经编码。
    } // 结束单个方向光级联近景草阴影绘制。
} // 结束近景草阴影RenderCommand实现。
