//! GPU草地全局交互数据与基础颜色纹理绑定。
//! 固定尺寸uniform每帧覆写；同时保留上一完整渲染帧交互，确保动态草叶能生成真实运动矢量。

use bevy::{
    prelude::*, // Resource、Res与ResMut等ECS常用类型。
    render::{
        render_asset::RenderAssets, // 读取已经上传GPU的基础草色图片资产。
        render_resource::{
            BindGroup,        // 跨帧持有group3全局草地绑定组。
            BindGroupEntries, // 按uniform、纹理和采样器顺序构造绑定项。
            PipelineCache,    // 把声明式布局解析为当前设备真实绑定组布局。
            UniformBuffer,    // 持久复用当前与上一帧两份固定176字节风场、相位与足迹GPU缓冲。
        },
        renderer::{RenderDevice, RenderQueue}, // 创建绑定组并排队uniform覆盖写入。
        texture::{FallbackImage, GpuImage},    // 访问消费者图片或无需外部资产的一乘一白色回退纹理。
    },
};

use super::pipeline::GpuBladePipeline; // 读取基础草色纹理句柄和专用管线group3布局。
use crate::components::GpuBladeProfileBankId; // 绘制命令用会话批次身份选择对应四槽绑定组。
use crate::resources::{
    GpuBladeInteraction, GpuBladeProfileBank, GpuBladeProfileBanks, GpuBladeProfiles,
    GpuBladeRenderAssets,
}; // 每帧提取一份风场，并在目录变化时准备兼容批次与任意数量数据驱动批次。

/// RenderApp 中一个四槽 Blade 批次的持久 GPU 常量和完整 group3。
struct GpuBladeProfileBankGpu {
    _profiles_uniform: UniformBuffer<GpuBladeProfiles>, // 绑定组持有底层缓冲引用；字段负责维持其生命周期。
    bind_group: Option<BindGroup>,                      // 当前批次纹理未上传时仅跳过引用它的窗口。
}

/// RenderApp持久复用的草地风场、玩家足迹与基础颜色GPU资源。
#[derive(Resource)]
pub(super) struct GpuBladeInteractionGpu {
    pub(super) uniform: UniformBuffer<GpuBladeInteraction>, // 当前帧主颜色、阴影和预通道姿态读取固定176字节内容。
    previous_uniform: UniformBuffer<GpuBladeInteraction>, // 上一完整渲染帧姿态只供运动矢量旧位置重建。
    last_interaction: Option<GpuBladeInteraction>, // RenderApp本地历史避免主世界为渲染专用上一帧状态承担所有权。
    profile_banks: Vec<GpuBladeProfileBankGpu>, // 下标零为兼容批次，后续下标与公开动态批次 ID 一一对应。
}

impl GpuBladeInteractionGpu {
    /// 返回某个窗口批次已经准备好的绑定组。
    pub(super) fn bind_group(&self, bank_id: GpuBladeProfileBankId) -> Option<&BindGroup> {
        let bank_index = usize::try_from(bank_id.index()).ok()?; // 不假设目标平台 usize 一定覆盖 u32。
        self.profile_banks.get(bank_index)?.bind_group.as_ref() // 目录越界或当前纹理等待上传时只跳过该窗口。
    }
}

/// 渲染子应用启动时建立固定尺寸的全局草地交互GPU资源。
pub(super) fn init_gpu_grass_interaction_gpu(mut commands: Commands) {
    let mut interaction_uniform = UniformBuffer::from(GpuBladeInteraction::default()); // 在首份提取数据到达前使用安全默认微风和零足迹。
    interaction_uniform.set_label(Some("gpu_grass_interaction_uniform")); // GPU调试器可直接定位固定小缓冲。
    let mut previous_interaction_uniform = UniformBuffer::from(GpuBladeInteraction::default()); // 首帧会以真实当前值同时覆盖两份，避免无历史脉冲。
    previous_interaction_uniform.set_label(Some("gpu_grass_previous_interaction_uniform")); // GPU捕获中明确标识运动矢量历史输入。
    commands.insert_resource(GpuBladeInteractionGpu {
        uniform: interaction_uniform, // 延迟到PrepareBindGroups首帧创建真实GPU backing buffer。
        previous_uniform: previous_interaction_uniform, // 与当前缓冲同步创建，绑定组生命周期保持一致。
        last_interaction: None, // 首份提取到达时把当前值同时作为上一帧，不能从默认风场伪造运动。
        profile_banks: Vec::new(), // 首次准备阶段再按提取后的真实目录建立兼容及动态批次。
    }); // 完成跨帧复用的全局草地动态输入资源初始化。
}

/// 返回本帧应上传的历史快照，并把当前值推进为下一完整渲染帧的历史。
fn advance_interaction_history(
    last_interaction: &mut Option<GpuBladeInteraction>, // RenderApp独占的上一帧CPU快照槽。
    current_interaction: GpuBladeInteraction, // 当前ExtractSchedule已经提交的最终动态输入。
) -> GpuBladeInteraction {
    let previous_interaction = last_interaction.unwrap_or(current_interaction); // 首帧无历史时使用当前姿态，避免虚构从默认值跳入场景的速度。
    *last_interaction = Some(current_interaction); // 先返回旧值、再保存当前值，严格维持previous/current次序。
    previous_interaction // 调用方把该固定176字节值写入binding4。
}

/// 每帧只覆写一次全局风场与玩家足迹uniform，并在纹理资产变化后刷新绑定组。
#[allow(clippy::too_many_arguments)] // Bevy系统参数分别承担资源变更检测与GPU对象访问，合并会隐藏调度借用。
pub(super) fn prepare_gpu_grass_interaction_bind_group(
    interaction: Res<GpuBladeInteraction>, // ExtractResource已经取得主世界本帧最终足迹快照。
    vegetation_profiles: Res<GpuBladeProfiles>, // ExtractResource取得普通草、麦田和两个预留槽的内容常量。
    profile_banks: Res<GpuBladeProfileBanks>, // 数据驱动批次数量不设内容硬上限，只在目录变化时整体重建。
    render_assets: Res<GpuBladeRenderAssets>, // 消费者可指定共享图集，None明确使用Bevy白色回退。
    pipeline: Res<GpuBladePipeline>,          // 提供group3声明式布局和内置绘制 shader 句柄。
    images: Res<RenderAssets<GpuImage>>,      // 原草颜色纹理上传或热重载后才能创建当前有效group3。
    fallback_image: Res<FallbackImage>,       // 无外部图集的公共最小示例直接使用一乘一白色纹理。
    pipeline_cache: Res<PipelineCache>,       // 把声明式布局解析为当前设备真实布局。
    render_device: Res<RenderDevice>,         // 首帧创建固定uniform缓冲与绑定组。
    render_queue: Res<RenderQueue>,           // 后续帧仅排队160字节覆盖写入。
    mut gpu: ResMut<GpuBladeInteractionGpu>,  // 原地复用缓冲，仅在图片GPU资产变化时重建绑定组。
) {
    let GpuBladeInteractionGpu {
        uniform,          // 独立借用固定尺寸uniform，避免创建绑定项时阻塞批次数组更新。
        previous_uniform, // 独立上传上一完整渲染帧快照，预通道不会把当前风摆误报为静止。
        last_interaction, // 在本帧上传结束后推进为下一帧历史。
        profile_banks: gpu_profile_banks, // 目录变化时原子替换全部批次GPU资源。
    } = &mut *gpu; // 显式拆开两个字段，使持久资源更新的借用边界清晰可审计。
    let previous_interaction = advance_interaction_history(last_interaction, *interaction); // 首帧同姿态，后续帧严格读取上一份提取结果。
    uniform.set(*interaction); // 把本帧风场、包围圆和最多八个交互源写入CPU侧编码器。
    uniform.write_buffer(&render_device, &render_queue); // 首帧分配，后续固定走queue.write_buffer。
    previous_uniform.set(previous_interaction); // 上一帧方向、强度与足迹变化必须和旧动画时间共同重建旧顶点。
    previous_uniform.write_buffer(&render_device, &render_queue); // 固定160字节覆盖，不随草叶或窗口数量增长。
    let should_rebuild_bind_group = gpu_profile_banks.is_empty()
        || images.is_changed()
        || render_assets.is_changed()
        || vegetation_profiles.is_changed()
        || profile_banks.is_changed(); // 只有图片、兼容配置或动态目录变化时才重建任意数量批次。
    if !should_rebuild_bind_group {
        return; // uniform底层缓冲尺寸固定且图片资产未变化，既有绑定组可安全复用。
    }
    let Some(uniform_binding) = uniform.binding() else {
        gpu_profile_banks.clear(); // 极端设备缓冲创建失败时清空旧组，禁止继续绑定已失效资源。
        return; // 本帧绘制命令会安全跳过，不读取未绑定数据。
    };
    let Some(previous_uniform_binding) = previous_uniform.binding() else {
        gpu_profile_banks.clear(); // 两帧形变输入必须原子就绪，禁止只绑定当前帧后输出错误零运动。
        return; // 下一帧继续尝试建立固定历史缓冲。
    };
    let mut rebuilt_banks = Vec::with_capacity(profile_banks.len().saturating_add(1)); // 精确按兼容批次加动态批次预留，不设置产品上限。
    let legacy_bank = GpuBladeProfileBank::new(
        *vegetation_profiles, // 兼容批次继续读取既有四槽 profile 资源。
        render_assets.blade_base_color.clone(), // 兼容批次继续读取既有共享纹理配置。
    ); // 把旧资源适配成和动态目录完全相同的批次输入。
    rebuilt_banks.push(create_profile_bank_gpu(
        &legacy_bank,              // 批次零始终先建立，旧地图和独立样例无需迁移。
        0,                         // GPU 调试标签明确标记兼容批次下标。
        &uniform_binding,          // 所有批次共享同一当前帧风场缓冲。
        &previous_uniform_binding, // 所有批次共享同一上一帧风场缓冲。
        &pipeline,                 // 所有批次遵守同一 group3 布局和绘制管线。
        &images,                   // 显式纹理尚未上传时为该批次保留空绑定组。
        &fallback_image,           // 无外部纹理时使用公共白色回退。
        &pipeline_cache,           // 解析声明式 group3 布局。
        &render_device,            // 创建 profile uniform 和绑定组。
        &render_queue,             // 首次上传当前四槽 profile 常量。
    )); // 完成兼容批次 GPU 资源创建。
    for (dynamic_index, bank) in profile_banks.banks().iter().enumerate() {
        let bank_index = dynamic_index.saturating_add(1); // 下标零保留给兼容批次，动态目录从一下标开始。
        rebuilt_banks.push(create_profile_bank_gpu(
            bank,                      // 当前数据驱动批次包含四槽外观与同页纹理。
            bank_index,                // 标签和公开批次 ID 使用同一会话下标。
            &uniform_binding,          // 全部批次复用当前帧交互缓冲。
            &previous_uniform_binding, // 全部批次复用上一帧交互缓冲。
            &pipeline,                 // 批次之间不创建重复管线布局。
            &images,                   // 每个批次独立判断自己的纹理是否已上传。
            &fallback_image,           // 无纹理批次使用同一内置回退资源。
            &pipeline_cache,           // 取得已经解析的 group3 布局。
            &render_device,            // 为当前批次创建小 uniform 与绑定组。
            &render_queue,             // 上传当前批次固定四槽内容。
        )); // 一个批次失败不会阻止其他纹理已就绪批次绘制。
    } // 完成任意数量数据驱动批次的线性资产变更路径处理。
    *gpu_profile_banks = rebuilt_banks; // 所有候选批次准备结束后一次替换，避免绘制看到半成品目录。
}

/// 为一个四槽批次创建常量缓冲和 group3；纹理等待期间保留空组供后续重试。
#[allow(clippy::too_many_arguments)] // 参数逐项对应共享GPU输入和批次内容，隐藏进临时资源会扩大状态生命周期。
fn create_profile_bank_gpu(
    bank: &GpuBladeProfileBank, // 当前批次四槽外观与纹理强句柄。
    bank_index: usize,          // 仅用于 GPU 调试标签的会话下标。
    interaction_binding: &bevy::render::render_resource::BindingResource<'_>, // 共享当前交互 uniform 绑定。
    previous_interaction_binding: &bevy::render::render_resource::BindingResource<'_>, // 共享上一帧交互 uniform 绑定。
    pipeline: &GpuBladePipeline,     // 声明式 group3 布局来源。
    images: &RenderAssets<GpuImage>, // 已经上传 GPU 的图片资产目录。
    fallback_image: &FallbackImage,  // 无显式综合色纹理时的白色回退。
    pipeline_cache: &PipelineCache,  // 把管线布局描述解析为真实布局。
    render_device: &RenderDevice,    // 创建 uniform backing buffer 和绑定组。
    render_queue: &RenderQueue,      // 上传四槽 profile 常量。
) -> GpuBladeProfileBankGpu {
    let mut profiles_uniform = UniformBuffer::from(bank.profiles); // 每批固定四槽，不随草株数量增长。
    profiles_uniform.set_label(Some(&format!("gpu_blade_profiles_bank_{bank_index}"))); // GPU 捕获中可定位具体批次。
    profiles_uniform.write_buffer(render_device, render_queue); // 目录变化时一次上传，稳定帧不遍历批次。
    let Some(profiles_binding) = profiles_uniform.binding() else {
        return GpuBladeProfileBankGpu {
            _profiles_uniform: profiles_uniform, // 缺图片时仍显式保留本次缓冲对象直至局部值释放。
            bind_group: None,                    // 缓冲分配失败时只抑制本批次绘制。
        };
    }; // 取得当前批次固定四槽 uniform 绑定。
    let base_color_texture = match bank.blade_base_color.as_ref() {
        Some(handle) => {
            let Some(image) = images.get(handle) else {
                return GpuBladeProfileBankGpu {
                    _profiles_uniform: profiles_uniform, // 保持图片强句柄由主世界批次目录所有。
                    bind_group: None,                    // 显式图片未就绪时禁止错误白色闪现。
                };
            }; // 只有真实 GPU 图片上传后才能创建绑定组。
            image // 取得当前批次图集纹理视图和采样器。
        }
        None => &fallback_image.d2, // 明确无纹理的批次按公共 API 契约使用白色回退。
    }; // 完成当前批次纹理选择。
    let bind_group_label = format!("gpu_blade_interaction_bank_{bank_index}"); // 临时拥有字符串在创建调用期间保持有效。
    let bind_group = render_device.create_bind_group(
        bind_group_label.as_str(), // GPU 捕获中可区分内容批次而非误判为全局四槽。
        &pipeline_cache.get_bind_group_layout(&pipeline.interaction_layout), // 所有批次共用同一绘制 ABI。
        &BindGroupEntries::sequential((
            interaction_binding.clone(),      // binding0 读取当前帧风场和局部交互源。
            &base_color_texture.texture_view, // binding1 读取当前批次共享的综合色纹理页。
            &base_color_texture.sampler,      // binding2 读取当前图片资产自己的过滤采样器。
            profiles_binding,                 // binding3 读取当前批次四个局部 profile。
            previous_interaction_binding.clone(), // binding4 读取上一完整帧交互以重建运动矢量。
        )), // 绑定顺序必须与主画面、预通道和阴影 shader 布局一致。
    ); // 完成当前批次完整 group3。
    GpuBladeProfileBankGpu {
        _profiles_uniform: profiles_uniform, // 绑定组通过底层 GPU 缓冲引用保持该字段必须同寿命存活。
        bind_group: Some(bind_group),        // 纹理与所有 uniform 都就绪后允许窗口绘制。
    }
}

#[cfg(test)] // 历史推进是纯固定值逻辑，不启动RenderApp或申请GPU缓冲。
mod tests {
    use super::*; // 直接验证生产历史函数与内部风场值。

    #[test] // 首帧不得伪造速度，第二帧必须精确返回第一帧完整交互。
    fn interaction_history_advances_after_returning_the_previous_snapshot() {
        let mut history = None; // 模拟RenderApp刚初始化、尚无完整渲染帧。
        let mut first = GpuBladeInteraction::default();
        first.set_wind(Vec2::X, 0.12, 0.8); // 第一帧使用可辨认的风场值。
        let mut second = GpuBladeInteraction::default();
        second.set_wind(Vec2::Y, 0.28, 1.6); // 第二帧同时改变方向、强度与速度。

        assert_eq!(advance_interaction_history(&mut history, first), first); // 首帧历史必须等于当前，禁止初始运动脉冲。
        assert_eq!(advance_interaction_history(&mut history, second), first); // 第二帧必须返回完整第一帧值供旧姿态重建。
        assert_eq!(history, Some(second)); // 调用后当前值才成为下一帧历史。
    }
}
