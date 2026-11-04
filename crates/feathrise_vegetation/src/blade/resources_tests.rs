//! GPU草地资源、动态输入和帧计数器回归测试。

use super::*; // 作为resources私有子模块访问固定uniform字段和内部计数接口。
use crate::constants::{
    GPU_GRASS_COMPACT_INSTANCE_BYTES, GPU_GRASS_DEPTH_BUCKETS_BYTES, GPU_GRASS_DRAW_STATE_BYTES,
    GPU_GRASS_TILE_DISPATCH_BYTES, GPU_GRASS_TILE_INDEX_BYTES,
}; // 显存断言直接引用后端唯一字节契约，避免依赖父模块偶然导入。
use crate::types::GpuBladeProfileSlot;
use bevy::prelude::{Vec2, Vec4}; // 测试直接构造风场、足迹和profile向量，不依赖资源入口的私有导入。 // 锁定公共四槽profile索引契约。

#[test] // 未进入草地图时公开流送快照必须是明确的停用状态而不是资源等待。
fn streaming_stats_default_to_an_inactive_empty_map() {
    let stats = GpuBladeStreamingStats::default(); // 插件安装阶段构建主世界全零流送快照。
    assert_eq!(stats.phase, GpuBladeStreamingPhase::Inactive); // F3不得把尚未启用误报为首载卡住。
    assert!(stats.map_code.is_empty()); // 没有正式地图时不得伪造消费项目内容归属。
    assert_eq!(stats.resident_assets, 0); // 未进入地图时不得声称持有任何草地强句柄。
    assert_eq!(stats.resident_input_bytes, 0); // 未进入地图时不得显示旧地图数值纹理显存。
    assert_eq!(stats.peak_speed, 0.0); // 未进入地图时不得保留旧会话最坏移动速度。
    assert_eq!(stats.activation, 0.0); // 未进入地图时全局覆盖率保持完全隐藏。
    assert_eq!(stats.prediction_offset, Vec2::ZERO); // 无玩家时预测偏移必须保持有限零值。
}

#[test] // 锁定主世界、Rust编码器和WGSL共享的固定动态uniform体积。
fn interaction_uniform_is_exactly_176_bytes() {
    assert_eq!(std::mem::size_of::<GpuBladeInteraction>(), 176); // 三个vec4（含连续风相位）加八个vec4足迹源，仍与草株数量无关。
}

#[test] // 公共默认表不得暗含任一游戏项目的草种预设。
fn grass_profiles_default_to_neutral_values() {
    let profiles = GpuBladeProfiles::default(); // 构造主世界与RenderApp共用的固定四槽表。
    for slot in [
        GpuBladeProfileSlot::SLOT_0,
        GpuBladeProfileSlot::SLOT_1,
        GpuBladeProfileSlot::SLOT_2,
        GpuBladeProfileSlot::SLOT_3,
    ] {
        assert_eq!(profiles.get(slot), GpuBladeProfile::DEFAULT); // 每个槽位都从同一中性公开默认值开始。
    }
} // 完成公共默认表无项目语义回归。

#[test] // 消费者必须能按受检槽位热切换profile，并可靠恢复公共默认表。
fn grass_profiles_support_slot_updates_and_reset() {
    let mut profiles = GpuBladeProfiles::default(); // 从四槽中性表开始模拟运行时配置。
    let custom = GpuBladeProfile::new(
        Vec4::new(1.1, 1.6, 0.04, 0.08),   // 自定义高度、下扎和宽度参数。
        Vec4::new(0.9, 1.2, 0.5, 0.8),     // 自定义宽度随机与收尖参数。
        Vec4::new(0.3, 0.4, 0.7, 0.9),     // 自定义风弯曲和局部交互参数。
        Vec4::new(0.5, 1.0, 0.5, 0.0),     // 自定义图集右半区域。
        Vec4::new(0.01, 0.02, 0.005, 0.0), // 自定义根部颜色。
        Vec4::new(0.08, 0.16, 0.04, 0.0),  // 自定义叶尖颜色。
        Vec4::new(0.9, 1.0, 1.1, 1.0),     // 自定义冷色调和覆盖标志。
        Vec4::new(1.1, 1.0, 0.8, 0.0),     // 自定义暖色调和远景标志。
    );

    profiles.set(GpuBladeProfileSlot::SLOT_2, custom); // 只替换明确选中的第三个槽。
    assert_eq!(profiles.get(GpuBladeProfileSlot::SLOT_2), custom); // 目标槽必须立即返回新值。
    assert_eq!(
        profiles.get(GpuBladeProfileSlot::SLOT_1),
        GpuBladeProfile::DEFAULT
    ); // 未触碰槽位不得受热切换污染。

    profiles.reset(); // 恢复公共中性表，验证地图或场景离开时的可逆生命周期。
    assert_eq!(profiles, GpuBladeProfiles::default()); // 重置结果必须逐位等于默认构造。
} // 完成四槽profile热切换与恢复回归。

#[test] // 验证无效源过滤、固定容量和粗包围圆不会漏掉合法玩家足迹。
fn interaction_sources_build_a_bounded_continuous_prefix() {
    let mut interaction = GpuBladeInteraction::default(); // 从默认微风和零局部源开始。
    interaction.set_sources(&[
        GpuBladeInteractionSource::new(Vec2::new(-2.0, 1.0), 1.0, 1.0), // 左侧合法玩家源。
        GpuBladeInteractionSource::new(Vec2::new(50.0, 50.0), 0.0, 1.0), // 零半径源必须过滤。
        GpuBladeInteractionSource::new(Vec2::new(3.0, -1.0), 2.0, 0.5), // 右侧合法历史足迹。
    ]); // 完成包含一个无效中间项的输入。
    assert_eq!(interaction.active_source_count(), 2); // 合法源必须压成连续前缀而不是保留空洞。
    assert_eq!(interaction.sources[0], Vec4::new(-2.0, 1.0, 1.0, 1.0)); // 第一合法源保持原世界参数。
    assert_eq!(interaction.sources[1], Vec4::new(3.0, -1.0, 2.0, 0.5)); // 第二合法源紧跟其后。
    let bounds_center = Vec2::new(
        interaction.interaction_bounds.x, // 从内部uniform读取包围圆世界X中心。
        interaction.interaction_bounds.y, // 从内部uniform读取包围圆世界Z中心。
    ); // 避免测试额外依赖glam swizzle trait导入。
    let bounds_radius = interaction.interaction_bounds.z; // 读取覆盖全部源圆的保守半径。
    for source in &interaction.sources[..interaction.active_source_count()] {
        let source_center = Vec2::new(source.x, source.y); // 当前有效源世界X/Z中心。
        assert!(bounds_center.distance(source_center) + source.z <= bounds_radius + 0.000_1); // 包围圆必须完整覆盖每个局部源自身半径，不能漏掉边缘单根草。
    }
    interaction.clear_sources(); // 模拟离开草地图或玩家暂时缺失。
    assert_eq!(interaction.active_source_count(), 0); // 清理后shader必须走零源快速路径。
    assert_eq!(
        interaction.wind_tip_strength(),
        GPU_GRASS_DEFAULT_WIND_TIP_STRENGTH
    ); // 清足迹不得同时关闭或重置已经提高的默认世界风场。
}

#[cfg(feature = "diagnostics")]
#[test] // 可选诊断显存口径必须从GPU后端真实容量派生且只覆盖明确的窗口专用长期缓冲。
fn readback_stats_derive_exact_dedicated_window_buffer_bytes() {
    let stats = GpuBladeGpuReadbackStats {
        sampled_windows: 3,    // 模拟近中远三个实际回读窗口。
        compact_capacity: 100, // 模拟三窗紧凑实例缓冲合计一百个二十字节槽。
        total_tiles: 50,       // 模拟三窗活跃列表容量合计五十个u32瓦片槽。
        ..Default::default()
    }; // 其他GPU计数不影响长期缓冲体积。
    assert_eq!(
        stats.dedicated_window_buffer_bytes(),
        100 * GPU_GRASS_COMPACT_INSTANCE_BYTES
            + 50 * GPU_GRASS_TILE_INDEX_BYTES
            + 3 * (GPU_GRASS_DRAW_STATE_BYTES
                + GPU_GRASS_DEPTH_BUCKETS_BYTES
                + GPU_GRASS_TILE_DISPATCH_BYTES * 2),
    ); // 紧凑实例、瓦片索引、真实派发参数及隔离分类绑定的scratch必须全部且只计一次。
}

#[test] // F1和外部地图配置都必须通过同一风力安全上限且不得影响当前足迹。
fn wind_tip_strength_is_clamped_without_clearing_sources() {
    let mut interaction = GpuBladeInteraction::default(); // 从正式十六厘米默认风和零足迹开始。
    interaction.set_sources(&[GpuBladeInteractionSource::new(
        Vec2::new(2.0, 3.0), // 建立一个可验证保留的世界空间足迹中心。
        1.0,                 // 使用合法一米影响半径。
        0.8,                 // 使用合法八成弯曲强度。
    )]); // 完成单个局部交互源写入。
    interaction.set_wind_tip_strength(10.0); // 模拟越过F1轨道右端的异常外部配置。
    assert_eq!(
        interaction.wind_tip_strength(),
        GPU_GRASS_MAX_WIND_TIP_STRENGTH
    ); // 最终摆幅必须钳制在三十五厘米。
    interaction.set_wind_tip_strength(f32::NAN); // 模拟手工配置或外部调用传入非有限值。
    assert_eq!(interaction.wind_tip_strength(), 0.0); // NaN必须安全回退为无风，不能污染GPU uniform。
    assert_eq!(interaction.active_source_count(), 1); // 调整世界风力不能清除玩家当前踩踏源。
} // 结束草地风力安全接口测试。

#[test] // 草地和树木必须能读取同一份经过安全归一化的世界风向。
fn wind_direction_getter_returns_normalized_world_direction() {
    let mut interaction = GpuBladeInteraction::default(); // 从正式默认风场开始验证公开只读接口。
    interaction.set_wind(Vec2::new(3.0, 4.0), 0.2, 1.1); // 使用可精确判断方向的三四五向量覆盖默认值。
    assert!(interaction.wind_direction().distance(Vec2::new(0.6, 0.8)) < 0.000_1); // 树木读取的方向必须与GPU草地实际上传方向一致。
} // 完成统一世界风向公开接口回归。

#[test] // 天气相位补偿不应覆盖当前玩家足迹或内部风速。
fn wind_phase_offset_preserves_local_sources_and_speed() {
    let mut interaction = GpuBladeInteraction::default(); // 从正式十六厘米和1.10倍率风场开始。
    interaction.set_sources(&[GpuBladeInteractionSource::new(Vec2::X, 1.0, 0.5)]); // 保留一项局部踩踏。
    interaction.set_wind_phase_offset(-2.5); // 天气变速时推进统一补偿。
    assert_eq!(interaction.wind_phase_offset(), -2.5); // GPU草地和Mesh读取同一偏移。
    assert_eq!(interaction.wind_speed(), 1.10); // 相位补偿不能被误当作风速。
    assert_eq!(interaction.active_source_count(), 1); // 玩家踩踏仍然有效。
    interaction.set_wind_phase_offset(f32::NAN); // 损坏相位不应污染顶点或历史位置。
    assert_eq!(interaction.wind_phase_offset(), 0.0); // 非有限值按零偏移回退。
} // 完成风相位与局部交互隔离验证。

#[test] // 公开风场和交互入口不得让NaN或无穷值进入固定GPU uniform。
fn wind_and_interaction_sources_reject_non_finite_values() {
    let mut interaction = GpuBladeInteraction::default(); // 从全部字段有限的正式默认值开始。
    interaction.set_wind(Vec2::new(f32::NAN, f32::INFINITY), f32::NAN, f32::INFINITY); // 同时覆盖非法方向、摆幅和动画速度。
    assert_eq!(interaction.wind, Vec4::new(1.0, 0.0, 0.0, 0.0)); // 非法风场必须确定回退为世界X方向静止无风。
    assert_eq!(interaction.wind_direction(), Vec2::X); // 树木等外部消费者同样只能看到安全回退方向。
    interaction.set_sources(&[
        GpuBladeInteractionSource::new(Vec2::new(f32::NAN, 1.0), 1.0, 1.0), // 非有限位置整项拒绝。
        GpuBladeInteractionSource::new(Vec2::ZERO, f32::INFINITY, 1.0),     // 非有限半径整项拒绝。
        GpuBladeInteractionSource::new(Vec2::ZERO, 1.0, f32::NAN),          // 非有限强度整项拒绝。
        GpuBladeInteractionSource::new(Vec2::new(2.0, 3.0), 1.5, 0.75), // 尾部合法源仍应压入连续首槽。
    ]); // 完成混合非法输入构造。
    assert_eq!(interaction.active_source_count(), 1); // 只有唯一合法源允许进入GPU循环。
    assert_eq!(interaction.sources[0], Vec4::new(2.0, 3.0, 1.5, 0.75)); // 合法尾部源必须保持完整参数。
    assert!(interaction.interaction_bounds.is_finite()); // 交互粗包围圆的全部分量必须保持有限。
} // 完成GPU动态输入有限值回归测试。

#[test] // 覆盖输入准备、Compute、排队和实绘四段关键流水线。
fn render_counters_publish_one_complete_frame_then_reset() {
    let counters = GpuBladeRenderFrameCounters::default(); // 创建全零渲染帧计数器。
    counters.record_input_window(true, true); // 模拟一个高度与植被密度输入完整窗口。
    counters.record_input_window(false, true); // 模拟一个仍等待高度纹理窗口。
    counters.record_prepared_bind_group(0, 0); // 模拟普通草近圈绑定组已经建立并记录内容与LOD身份。
    counters.record_resident_reused_window(); // 模拟Shader热重载期间继续绘制上一份有效驻留结果。
    counters.record_compute_pipeline_ready(); // 模拟四个Compute阶段全部完成异步编译。
    counters.record_compute_pipeline_ready(); // 第二个固定阶段同样必须进入快照。
    counters.record_compute_pipeline_ready(); // 第三个固定阶段同样必须进入快照。
    counters.record_compute_pipeline_ready(); // 第四个固定阶段同样必须进入快照。
    counters.record_target_view(); // 模拟一个玩法主视图。
    counters.record_visible_window(); // 模拟一个可见草窗口。
    counters.record_queued_draw(591_361); // 模拟近圈进入AlphaMask阶段。
    counters.record_queued_shadow_cascade(); // 模拟普通草近圈进入一个方向光级联的Shadow阶段。
    counters.record_pipeline_ready(0, 0); // 模拟普通草近圈管线已经可执行并记录内容与LOD身份。
    counters.record_draw(591_361, 8, 0, 0); // 模拟普通草近圈真正编码八个深度桶实例draw。
    counters.record_shadow_draw(8); // 模拟该级联真正编码八条近景草阴影间接命令。

    let snapshot = counters.snapshot_and_reset(); // 发布刚完成的完整渲染帧。
    assert_eq!(snapshot.extracted_windows, 2); // 两个窗口均应计入提取。
    assert_eq!(snapshot.input_ready_windows, 1); // 只有一个窗口两类叶片输入完全就绪。
    assert_eq!(snapshot.height_waiting_windows, 1); // 高度等待原因必须单独保留。
    assert_eq!(snapshot.resident_reused_windows, 1); // 驻留结果复用必须与首次加载的零输出等待明确区分。
    assert_eq!(snapshot.compute_pipeline_ready_stages, 4); // 正常帧必须明确看到四个Compute阶段全部就绪。
    assert_eq!(snapshot.compute_pipeline_waiting_stages, 0); // 稳态帧不应残留异步创建阶段。
    assert_eq!(snapshot.compute_pipeline_error_stages, 0); // 稳态帧不应隐藏着色器编译或设备错误。
    assert_eq!(snapshot.queued_draws, 1); // 近圈应产生一个AlphaMask阶段绘制项。
    assert_eq!(snapshot.queued_shadow_cascades, 1); // 近景普通草应进入一个方向光级联阴影阶段。
    assert_eq!(snapshot.drawn_shadow_commands, 8); // 一个阴影级联必须编码八条深度桶间接命令。
    assert_eq!(snapshot.prepared_lod_mask, 0b001); // 绑定组身份必须准确保留为近圈。
    assert_eq!(snapshot.prepared_profile_lod_mask, 0b000_001); // 普通草近景必须占内容LOD位零。
    assert_eq!(snapshot.pipeline_ready_lod_mask, 0b001); // 管线身份必须准确保留为近圈。
    assert_eq!(snapshot.pipeline_ready_profile_lod_mask, 0b000_001); // 管线就绪同样保留内容身份。
    assert_eq!(snapshot.drawn_draws, 8); // 一个LOD绘制项必须展开为八条近到远间接命令。
    assert_eq!(snapshot.drawn_candidates, 591_361); // 实绘候选数必须与记录值一致。
    assert_eq!(snapshot.drawn_lod_mask, 0b001); // 实绘身份必须准确保留为近圈。
    assert_eq!(snapshot.drawn_profile_lod_mask, 0b000_001); // 实绘普通草近景占内容LOD位零。
    assert_eq!(
        counters.snapshot_and_reset(),
        GpuBladeRenderStats::default()
    ); // 取走后下一帧必须从零开始。
} // 完成完整帧快照回归测试。

#[test]
fn duplicate_lod_windows_cannot_complete_identity_masks() {
    let counters = GpuBladeRenderFrameCounters::default();
    for lod_level in [0, 0, 2] {
        counters.record_prepared_bind_group(0, lod_level); // 数量达到三，但故意重复普通草近景并缺少中景。
        counters.record_pipeline_ready(0, lod_level);
        counters.record_draw(1, 8, 0, lod_level);
    }
    let snapshot = counters.snapshot_and_reset();
    assert_eq!(snapshot.prepared_bind_groups, 3); // 聚合数量单独看会产生假完整。
    assert_eq!(snapshot.drawn_draws, 24); // 间接命令数同样可能被重复窗口凑满。
    assert_eq!(snapshot.prepared_lod_mask, 0b101); // 身份掩码明确暴露LOD1缺失。
    assert_eq!(snapshot.pipeline_ready_lod_mask, 0b101);
    assert_eq!(snapshot.drawn_lod_mask, 0b101);
} // 完成“缺一档、重复另一档”诊断回归。

#[test]
fn ordinary_grass_lod_mask_cannot_impersonate_wheat_lods() {
    let counters = GpuBladeRenderFrameCounters::default();
    for lod_level in 0..3 {
        counters.record_prepared_bind_group(0, lod_level); // 只准备普通草三档，故意完全缺失麦田。
        counters.record_pipeline_ready(0, lod_level);
        counters.record_draw(1, 8, 0, lod_level);
    }
    let grass_only = counters.snapshot_and_reset();
    assert_eq!(grass_only.prepared_lod_mask, 0b111); // 聚合旧掩码单独看已完整。
    assert_eq!(grass_only.prepared_profile_lod_mask, 0b000_111); // 内容掩码明确只有普通草低三位。
    assert_eq!(grass_only.pipeline_ready_profile_lod_mask, 0b000_111);
    assert_eq!(grass_only.drawn_profile_lod_mask, 0b000_111);

    for lod_level in 0..3 {
        counters.record_prepared_bind_group(1, lod_level); // 再单独准备麦田三档。
        counters.record_pipeline_ready(1, lod_level);
        counters.record_draw(1, 8, 1, lod_level);
    }
    let wheat_only = counters.snapshot_and_reset();
    assert_eq!(wheat_only.prepared_profile_lod_mask, 0b111_0000_0000); // 八级内容步长下麦田三档使用位八至十。
    assert_eq!(wheat_only.pipeline_ready_profile_lod_mask, 0b111_0000_0000);
    assert_eq!(wheat_only.drawn_profile_lod_mask, 0b111_0000_0000);
} // 完成草麦同LOD不能互相冒充的严格内容完整性回归。
