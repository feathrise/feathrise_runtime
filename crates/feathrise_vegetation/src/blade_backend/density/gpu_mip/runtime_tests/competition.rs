//! 真实 RenderApp 正式竞争输入、三种模式与局部基础级复制烟雾。

use std::sync::Arc; // 同组多个目标层复用同一份未决输入，贴近生产三层批次。
use std::time::{Duration, Instant}; // 每个GPU作业保持有界等待并短暂让出CPU。

use super::take_result; // 复用父模块已经完成初始化的真实RenderApp版本化收件箱。
use crate::blade_backend::density::atlas::build_layer_density_atlas; // CPU完整Mip仍是逐字节唯一权威。
use crate::blade_backend::density::gpu_compose::{
    VegetationGpuCompetitionInput, build_gpu_competition_input,
}; // 主世界编码未决正式需求并由RenderGraph实际解析。
use crate::blade_backend::density::resources::{
    VegetationGpuBaseJob, VegetationGpuMipJob, VegetationGpuMipOutcome, VegetationGpuMipRequest,
}; // 每层候选携带共享输入、目标索引和可选上一有效GPU图。
use crate::{
    ActiveVegetationLayout, ExplicitVegetationPlacement, MarkVegetationDirty,
    VegetationCandidateSource, VegetationClaimSampling, VegetationCompetition,
    VegetationCompetitionGroup, VegetationCompetitionGroupId, VegetationCompetitionMode,
    VegetationDensityField, VegetationDirtyTile, VegetationFootprint, VegetationFootprintScale,
    VegetationFootprintShape, VegetationLayerConfig, VegetationLayerId, VegetationLayoutCommit,
    VegetationLayoutState, VegetationPlugin, VegetationPrototypeId, VegetationRealization,
    VegetationSourceField, VegetationWeight,
};
use bevy::prelude::{App, Assets, IVec2, Image, Rect, Transform, Vec3}; // 构造两个64纹素块的正式布局与候选图片。 // fixture只组合公共正式层、组、源、足迹和脏区API。

mod constants; // 稳定组、层、票据和fixture范围从执行流程中拆出，便于审计身份映射。
use constants::*; // 本模块只消费P5.2b2b烟雾专用身份，不向生产API泄漏。

/// 在已经通过P5.2b2a Mip烟雾的同一RenderApp上继续验证正式竞争和局部更新。
pub(super) fn validate_gpu_competition(app: &mut App, backend: wgpu::Backend) {
    let mut layout_app = competition_layout_app(); // 无渲染小应用生成真实CPU权威布局和提交快照。
    let full_active = layout_app
        .world()
        .resource::<ActiveVegetationLayout>()
        .clone(); // revision1同时包含负块与正块。
    let full_commit = layout_app
        .world()
        .resource::<VegetationLayoutCommit>()
        .clone(); // 首次提交必须包含两个块。
    let full_state = layout_app
        .world()
        .resource::<VegetationLayoutState>()
        .clone(); // GPU编码读取与CPU合成同一配置和源代次。
    assert_eq!(
        full_commit.dirty_tiles(),
        &[
            VegetationDirtyTile(IVec2::new(-1, 0)),
            VegetationDirtyTile(IVec2::ZERO),
        ]
    ); // 整图输入顺序固定为Y/X稳定序。

    let additive_input = competition_input(
        &full_state,
        &full_active,
        ADD_LAYER,
        full_commit.dirty_tiles(),
    ); // 单层Additive仍必须在GPU乘入显式足迹影响。
    let additive_image = run_competition_job(
        app,
        backend,
        &full_active,
        ADD_LAYER,
        FULL_TICKET_ADDITIVE,
        additive_input,
        full_commit.dirty_tiles(),
        None,
    ); // 保留已验证GPU图片句柄供局部版本复制Mip0。

    let weighted_input = competition_input(
        &full_state,
        &full_active,
        WEIGHT_SMALL_LAYER,
        full_commit.dirty_tiles(),
    ); // 三个目标层共享同一未决样本和稳定描述。
    run_competition_job(
        app,
        backend,
        &full_active,
        WEIGHT_SMALL_LAYER,
        FULL_TICKET_WEIGHTED,
        weighted_input.clone(),
        full_commit.dirty_tiles(),
        None,
    ); // 小需求目标必须走封顶返回分支。
    run_competition_job(
        app,
        backend,
        &full_active,
        WEIGHT_LARGE_LAYER,
        FULL_TICKET_WEIGHTED,
        weighted_input,
        full_commit.dirty_tiles(),
        None,
    ); // 同票据同Arc只更换目标uniform并复用四个GPU存储缓冲。

    let priority_input = competition_input(
        &full_state,
        &full_active,
        PRIORITY_LOW_LAYER,
        full_commit.dirty_tiles(),
    ); // 实际密度和SourceCellOverlap所有权保持两列独立输入。
    run_competition_job(
        app,
        backend,
        &full_active,
        PRIORITY_LOW_LAYER,
        FULL_TICKET_PRIORITY,
        priority_input.clone(),
        full_commit.dirty_tiles(),
        None,
    ); // 低层在任一高层声明处归零，空声明处保留自身密度。
    run_competition_job(
        app,
        backend,
        &full_active,
        PRIORITY_FIRST_LAYER,
        FULL_TICKET_PRIORITY,
        priority_input,
        full_commit.dirty_tiles(),
        None,
    ); // 高层同级选择必须与CPU世界稳定selector逐字节一致。

    replace_additive_right_source(&mut layout_app); // 只改变右块源并发布revision2单块提交。
    let local_active = layout_app
        .world()
        .resource::<ActiveVegetationLayout>()
        .clone(); // 左块应由核心复用，右块重新合成。
    let local_commit = layout_app
        .world()
        .resource::<VegetationLayoutCommit>()
        .clone(); // 最新提交只允许包含右块。
    let local_state = layout_app
        .world()
        .resource::<VegetationLayoutState>()
        .clone(); // 编码器读取已替换源代次。
    assert_eq!(
        local_commit.dirty_tiles(),
        &[VegetationDirtyTile(IVec2::ZERO)]
    ); // 局部GPU竞争只能覆盖右侧64×64基础级。
    let local_input = competition_input(
        &local_state,
        &local_active,
        ADD_LAYER,
        local_commit.dirty_tiles(),
    ); // 输入缓冲只包含一个Z切片和4096个未决纹素。
    run_competition_job(
        app,
        backend,
        &local_active,
        ADD_LAYER,
        LOCAL_TICKET_ADDITIVE,
        local_input,
        local_commit.dirty_tiles(),
        Some(additive_image),
    ); // 全Mip逐字节匹配证明左块来自旧GPU复制、右块来自新竞争、父Mip重新生成。
}

/// 建立两个一米纹素块并通过真实公共布局系统发布首次完整版本。
fn competition_layout_app() -> App {
    let mut app = App::new(); // 纯布局应用不创建窗口、RenderApp或图形设备。
    app.add_plugins(VegetationPlugin); // 使用生产消息收集、预算合成和原子提交系统。
    let bounds = fixture_bounds(); // 七层和全部数值源共享两个块的世界范围。
    let groups = vec![
        VegetationCompetitionGroup {
            id: ADD_GROUP,                             // 单层叠加组。
            mode: VegetationCompetitionMode::Additive, // 直接保持自身密度。
            selection_cell_size: 1.0,                  // Additive仍提供合法固定值。
            selection_seed: 0xB200,                    // 固定世界种子。
        },
        VegetationCompetitionGroup {
            id: WEIGHTED_GROUP, // 三层容量竞争组。
            mode: VegetationCompetitionMode::WeightedPartition { capacity: 0.7 }, // 高层先消费、同级再按权重封顶。
            selection_cell_size: 1.0, // Weighted不读取selector但配置必须合法。
            selection_seed: 0xB210,   // 保留稳定技术身份。
        },
        VegetationCompetitionGroup {
            id: PRIORITY_GROUP, // 三层严格覆盖组。
            mode: VegetationCompetitionMode::PriorityOverride {
                claim_threshold: 0.0, // 任意非零高层源单元都声明所有权。
            },
            selection_cell_size: 3.0, // 非单位选择格暴露误用图集局部坐标。
            selection_seed: 0xB220,   // 正负世界块使用同一稳定哈希。
        },
        VegetationCompetitionGroup {
            id: EXCLUDER_GROUP,                        // 稀疏显式上游组。
            mode: VegetationCompetitionMode::Additive, // 唯一候选无需组内竞争。
            selection_cell_size: 1.0,                  // 合法固定值。
            selection_seed: 0xB230,                    // 固定候选启用口径。
        },
    ];
    let layers = vec![
        dense_layer(
            ADD_LAYER,
            ADD_GROUP,
            0,
            1.0,
            VegetationClaimSampling::Point,
            bounds,
        ),
        dense_layer(
            WEIGHT_SMALL_LAYER,
            WEIGHTED_GROUP,
            0,
            1.0,
            VegetationClaimSampling::Point,
            bounds,
        ),
        dense_layer(
            WEIGHT_LARGE_LAYER,
            WEIGHTED_GROUP,
            0,
            3.0,
            VegetationClaimSampling::Point,
            bounds,
        ),
        dense_layer(
            WEIGHT_HIGH_LAYER,
            WEIGHTED_GROUP,
            10,
            1.0,
            VegetationClaimSampling::Point,
            bounds,
        ),
        dense_layer(
            PRIORITY_LOW_LAYER,
            PRIORITY_GROUP,
            0,
            1.0,
            VegetationClaimSampling::Point,
            bounds,
        ),
        dense_layer(
            PRIORITY_FIRST_LAYER,
            PRIORITY_GROUP,
            100,
            1.0,
            VegetationClaimSampling::SourceCellOverlap,
            bounds,
        ),
        dense_layer(
            PRIORITY_SECOND_LAYER,
            PRIORITY_GROUP,
            100,
            2.0,
            VegetationClaimSampling::SourceCellOverlap,
            bounds,
        ),
        excluder_layer(bounds), // 右块中心的圆形足迹同时影响前三组。
    ];
    {
        let mut state = app.world_mut().resource_mut::<VegetationLayoutState>();
        state.tile_budget_per_frame = 2; // 首次两个块在一次Update完成并只发布一个revision。
        state
            .set_configuration(groups, layers)
            .expect("runtime competition configuration should validate");
        for layer in [
            ADD_LAYER,
            WEIGHT_SMALL_LAYER,
            WEIGHT_LARGE_LAYER,
            WEIGHT_HIGH_LAYER,
            PRIORITY_LOW_LAYER,
            PRIORITY_FIRST_LAYER,
            PRIORITY_SECOND_LAYER,
        ] {
            state.set_source_field(layer, fixture_source(layer, bounds, false)); // 每层保持独立非均匀未决数值。
        }
        state.set_source_field(
            EXCLUDER_LAYER,
            VegetationSourceField::constant(bounds, 1.0)
                .expect("constant explicit source should validate"),
        ); // 唯一显式候选始终启用。
    }
    mark_dirty(&mut app, IVec2::new(-1, 0), IVec2::ZERO); // 首次版本覆盖两个相邻块。
    app.update(); // 真实生产系统合成CPU权威并发布revision1。
    assert_eq!(app.world().resource::<ActiveVegetationLayout>().revision, 1); // fixture必须在GPU输入编码前完整就绪。
    app
}

/// 创建一个普通密集层；内容语义只由测试常量名表达。
fn dense_layer(
    id: VegetationLayerId,                   // 稳定目标层身份。
    group: VegetationCompetitionGroupId,     // 当前层所属竞争组。
    priority: i32,                           // 数值越大越先获得容量或所有权。
    weight: f32,                             // 同级竞争的受检正权重。
    claim_sampling: VegetationClaimSampling, // 点采样或源单元相交所有权。
    bounds: Rect,                            // 两个块共享的正式世界范围。
) -> VegetationLayerConfig {
    VegetationLayerConfig {
        id,                                     // 无限层身份与GPU目标索引解耦。
        prototype: VegetationPrototypeId(id.0), // 烟雾不实例化外观但仍使用稳定原型ID。
        source: VegetationCandidateSource::DenseGrid {
            spacing: 1.0, // 每个输出纹素一个规则候选。
            seed: id.0,   // 每层保持独立稳定分布身份。
        },
        density: VegetationDensityField {
            world_bounds: bounds, // 范围外严格为零。
            gain: 1.0,            // 未决源值不在CPU预放大。
        },
        competition: VegetationCompetition {
            group,    // 引用调用点正式组。
            priority, // GPU读取二补码原位恢复。
            weight: VegetationWeight::new(weight).expect("fixture weight should validate"),
            claim_sampling,              // 所有权输入独立于实际密度。
            excludes_groups: Vec::new(), // 三个目标组不形成反向排斥环。
        },
        footprint: VegetationFootprint {
            shape: VegetationFootprintShape::Circle { radius: 0.25 }, // 仅提供合法候选轮廓。
            scale: VegetationFootprintScale::WorldSpace,              // 固定世界测试尺寸。
            exclusion_margin: 0.0,                                    // 当前层不向其他组输出足迹。
            recovery_width: 0.0,                                      // 组内竞争处理空间分配。
        },
        realization: VegetationRealization::GpuOnly, // 正式叶片密度目标。
    }
}

/// 创建一个右块显式圆形排斥候选。
fn excluder_layer(bounds: Rect) -> VegetationLayerConfig {
    VegetationLayerConfig {
        id: EXCLUDER_LAYER,                                 // 稳定稀疏层身份。
        prototype: VegetationPrototypeId(EXCLUDER_LAYER.0), // 不实例化外观但保留原型契约。
        source: VegetationCandidateSource::ExplicitPlacements(vec![ExplicitVegetationPlacement {
            instance_id: 0, // 稳定实例选择值必然小于满密度。
            transform: Transform::from_translation(Vec3::new(16.0, 0.0, 32.0)), // 右块内部中心。
            density: 1.0,   // 候选自身完整启用。
        }]),
        density: VegetationDensityField {
            world_bounds: bounds, // 候选中心位于正式范围内。
            gain: 1.0,            // 常量源保持满密度。
        },
        competition: VegetationCompetition {
            group: EXCLUDER_GROUP,                          // 独立无上游叠加组。
            priority: 0,                                    // 唯一层无需比较。
            weight: VegetationWeight::default(),            // 唯一层默认等权。
            claim_sampling: VegetationClaimSampling::Point, // 显式中心只执行点启用。
            excludes_groups: vec![ADD_GROUP, WEIGHTED_GROUP, PRIORITY_GROUP], // 同一足迹验证三种模式的后乘影响。
        },
        footprint: VegetationFootprint {
            shape: VegetationFootprintShape::Circle { radius: 2.0 }, // 两米硬圆内部归零。
            scale: VegetationFootprintScale::WorldSpace,             // 忽略单位Transform缩放。
            exclusion_margin: 1.0,                                   // 总硬净空三米。
            recovery_width: 2.0, // 两米软恢复暴露浮点影响和八位量化。
        },
        realization: VegetationRealization::Entities, // 上游只提供显式足迹，不进入叶片目标。
    }
}

/// 为一层生成129×65端点式非均匀未决源。
fn fixture_source(
    layer: VegetationLayerId, // 当前层决定数值模式。
    bounds: Rect,             // -64..64 × 0..64恰好一米端点间距。
    modified_right: bool,     // 局部版本只改变Additive右块。
) -> VegetationSourceField {
    let values = (0..65)
        .flat_map(|y| {
            (0..129).map(move |x| {
                if layer == ADD_LAYER {
                    let byte = ((x * 37 + y * 19 + 11) % 256) as u8; // 全八位周期暴露量化和寻址差异。
                    if modified_right && x >= 64 {
                        f32::from(u8::MAX - byte) / 255.0 // 局部版本只反转非负世界半区。
                    } else {
                        f32::from(byte) / 255.0 // 首次版本保持原始八位端点。
                    }
                } else if layer == WEIGHT_SMALL_LAYER {
                    if (x + y) % 5 == 0 { 0.08 } else { 0.12 } // 小需求层应在权重水填充中先封顶。
                } else if layer == WEIGHT_LARGE_LAYER {
                    0.82 - (x % 7) as f32 * 0.03 // 大需求层消费重分后的多数容量。
                } else if layer == WEIGHT_HIGH_LAYER {
                    0.05 + (y % 4) as f32 * 0.02 // 高优先级先消耗约0.05至0.11容量。
                } else if layer == PRIORITY_LOW_LAYER {
                    0.35 + (x % 9) as f32 * 0.04 // 无作物声明处保留非均匀低层密度。
                } else if layer == PRIORITY_FIRST_LAYER {
                    if (x / 7 + y / 5) % 3 == 0 { 0.75 } else { 0.0 } // 第一高层形成离散声明单元。
                } else if layer == PRIORITY_SECOND_LAYER {
                    if (x / 11 + y / 3) % 4 <= 1 { 0.55 } else { 0.0 } // 第二高层与第一层部分重叠并拥有双权重。
                } else {
                    0.0 // 调用方不应为显式排斥层请求密集字段。
                }
            })
        })
        .collect::<Vec<_>>();
    VegetationSourceField::new(bounds, 129, 65, values)
        .expect("runtime competition source should validate") // 一米端点字段决定布局texel_size为一。
}

/// 替换Additive源并只请求右块，形成真实相邻revision局部提交。
fn replace_additive_right_source(app: &mut App) {
    let bounds = fixture_bounds(); // 新源尺寸和世界映射保持不变以允许图集原位复制。
    app.world_mut()
        .resource_mut::<VegetationLayoutState>()
        .set_source_field(ADD_LAYER, fixture_source(ADD_LAYER, bounds, true)); // 只改变正半区数值。
    mark_dirty(app, IVec2::ZERO, IVec2::ZERO); // 当前提交精确覆盖右块。
    app.update(); // 单块在预算二以内同帧发布revision2。
    assert_eq!(app.world().resource::<ActiveVegetationLayout>().revision, 2); // 局部GPU作业必须消费相邻新版本。
}

/// 通过生产消息入口请求一个闭区间块范围。
fn mark_dirty(app: &mut App, minimum: IVec2, maximum: IVec2) {
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<MarkVegetationDirty>>()
        .write(MarkVegetationDirty {
            min_tile: minimum, // 请求闭区间最小块。
            max_tile: maximum, // 请求闭区间最大块。
        });
}

/// 为一个组建立可跨目标层复用的正式未决输入。
fn competition_input(
    layout: &VegetationLayoutState,       // 当前配置与源代次。
    active: &ActiveVegetationLayout,      // 当前CPU权威块和版本。
    layer: VegetationLayerId,             // 用于选择目标竞争组。
    update_tiles: &[VegetationDirtyTile], // 完整或局部提交计划。
) -> Arc<VegetationGpuCompetitionInput> {
    let atlas =
        build_layer_density_atlas(active, layer).expect("runtime competition atlas should build"); // 图集几何与CPU权威同源。
    build_gpu_competition_input(layout, &atlas, layer, update_tiles)
        .expect("runtime competition input should encode") // 返回尚未解析的实际/所有权需求和上下文。
}

/// 提交一层正式GPU竞争作业并逐字节比较基础级和全部子Mip。
#[allow(clippy::too_many_arguments)] // 作业身份、CPU权威、共享输入、脏区和旧图各自是独立验收维度。
fn run_competition_job(
    app: &mut App,                                        // 已初始化真实RenderApp。
    backend: wgpu::Backend,                               // 失败信息保留本次正式后端。
    active: &ActiveVegetationLayout,                      // 当前CPU权威活动版本。
    layer: VegetationLayerId,                             // 当前串行目标层。
    ticket: u64,                                          // 同组目标层可共享票据。
    input: Arc<VegetationGpuCompetitionInput>,            // 尚未解析的组输入。
    update_tiles: &[VegetationDirtyTile],                 // 完整加载或当前提交块。
    previous_image: Option<bevy::prelude::Handle<Image>>, // 局部作业从上一已验证GPU图复制Mip0。
) -> bevy::prelude::Handle<Image> {
    let atlas = build_layer_density_atlas(active, layer)
        .expect("runtime competition reference atlas should build");
    assert!(input.matches_plan(
        atlas.base_width().unwrap(),  // 共享输入宽度必须与当前目标图集一致。
        atlas.base_height().unwrap(), // 共享输入高度必须与当前目标图集一致。
        update_tiles,                 // 完整或局部块序列必须逐项一致。
    )); // 同组另一目标层不得复用错几何或错提交计划的大输入。
    let expected = atlas
        .rgba_mip_bytes()
        .expect("runtime competition CPU mip bytes should encode"); // 全部Mip是无容差权威。
    let candidate = atlas
        .to_gpu_competition_candidate_image()
        .expect("runtime competition zero candidate should encode"); // CPU不得预填任何基础级答案。
    let width = atlas.base_width().unwrap(); // 正式候选基础级真实宽。
    let height = atlas.base_height().unwrap(); // 正式候选基础级真实高。
    let mip_level_count = atlas.mip_level_count().unwrap(); // 128×64完整链共八级。
    let target_layer_index = input
        .target_layer_index(layer)
        .expect("runtime competition target should belong to encoded group");
    let image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(candidate); // 候选与上一有效图始终使用不同强句柄。
    app.world_mut()
        .resource_mut::<VegetationGpuMipRequest>()
        .job = Some(VegetationGpuMipJob {
        ticket,                    // 同组不同层允许复用票据和大输入缓冲。
        revision: active.revision, // 结果不得跨布局版本。
        layer,                     // 目标层同时参与RenderApp去重键。
        image: image.clone(),      // RenderAsset正常上传全零候选。
        width,                     // Prepare核对GPU纹理宽度。
        height,                    // Prepare核对GPU纹理高度。
        mip_level_count,           // Prepare核对完整链级数。
        base: VegetationGpuBaseJob::Competition {
            input,              // 四个存储缓冲只含未决正式输入。
            target_layer_index, // 当前作业只写这一层候选Mip0。
            previous_image,     // 局部作业先复制旧Mip0；全量为None。
        },
    });
    let deadline = Instant::now() + Duration::from_secs(30); // 每层管线、Compute、复制和映射等待有界。
    let result = loop {
        app.update(); // 驱动提取、RenderAsset、基础竞争、Mip和异步回读。
        if let Some(result) = take_result(app, ticket, active.revision, layer) {
            break result; // 三重身份匹配后才能消费当前结果。
        }
        assert!(
            Instant::now() < deadline,
            "GPU competition layer {layer:?} did not finish on {backend:?} within 30 seconds"
        );
        std::thread::sleep(Duration::from_millis(5)); // 给WGPU映射回调线程短暂执行机会。
    };
    match result.outcome {
        VegetationGpuMipOutcome::Complete(actual) => assert_eq!(
            actual.as_ref(),
            expected.as_slice(),
            "GPU competition layer {layer:?} diverged on {backend:?}"
        ), // 基础级竞争、足迹、局部复制和全部父Mip任何一字节差异都失败。
        VegetationGpuMipOutcome::Failed(reason) => {
            panic!("GPU competition layer {layer:?} failed on {backend:?}: {reason}");
        }
    }
    image // 调用方可把已验证GPU纹理作为下一相邻版本的只读复制源。
}
