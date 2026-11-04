//! 主世界候选票据、逐字节门和跨层原子完成回归。

use std::collections::BTreeMap; // 构造稳定两层候选批次。
use std::sync::Arc; // 测试载荷与生产回读使用相同共享字节类型。

use bevy::prelude::{Handle, Image, Vec2}; // 私有缓存测试只需要空图片句柄和安全世界范围。

use super::*; // 直接验证主状态机纯结果消费与比较函数。
use crate::blade_backend::density::atlas::build_layer_density_atlas; // 构造合法1×1紧凑图集供候选状态使用。
use crate::blade_backend::density::resources::{
    CachedBladeDensity, PendingBladeDensity, PendingBladeDensityBatch, PendingBladeDensityState,
    VegetationGpuBaseJob, VegetationGpuMipCoordinator, VegetationGpuMipInbox, VegetationGpuMipJob,
    VegetationGpuMipOutcome, VegetationGpuMipRequest, VegetationGpuMipResult,
}; // 使用生产候选、请求和结果类型锁定身份门。
use crate::{ActiveVegetationLayout, VegetationLayerId}; // 测试层ID和安全空布局。

/// 构造一个指定状态的合法1×1候选层。
fn pending_layer(
    layer: VegetationLayerId,        // 候选所属稳定层。
    revision: u64,                   // 候选所属活动版本。
    expected: &[u8],                 // GPU回读必须精确匹配的RGBA字节。
    state: PendingBladeDensityState, // 当前串行阶段。
) -> PendingBladeDensity {
    let layout = ActiveVegetationLayout {
        revision,             // 图集内容不依赖版本但缓存身份需要。
        texel_size: 1.0,      // 空层安全零图仍要求合法尺度。
        ..Default::default()  // 不插入真实块即可得到1×1零图。
    };
    let atlas = build_layer_density_atlas(&layout, layer).expect("empty layer atlas should build"); // 使用正式CPU参考构造。
    PendingBladeDensity {
        cached: CachedBladeDensity {
            revision,                          // 与批次版本一致。
            image: Handle::<Image>::default(), // 纯状态测试不访问Assets表。
            atlas: Arc::new(atlas),            // 保持生产缓存形状。
            world_min: Vec2::ZERO,             // 空层安全映射下界。
            world_max: Vec2::ONE,              // 空层安全映射上界。
        },
        expected: Arc::from(expected), // 比较使用独立不可变载荷。
        base: VegetationGpuBaseJob::Uploaded, // 纯状态测试不创建正式竞争输入。
        state,                         // 调用方选择等待、请求或通过。
    }
}

/// 构造当前请求和单层待回读协调器。
fn requested_fixture() -> (
    VegetationLayerId,           // 当前层身份。
    VegetationGpuMipCoordinator, // 含一层Requested候选的协调器。
    VegetationGpuMipRequest,     // 与候选身份一致的提取请求。
) {
    let layer = VegetationLayerId(41); // 任意非零稳定层ID。
    let ticket = 9; // 当前批次票据。
    let revision = 7; // 当前活动版本。
    let mut layers = BTreeMap::new(); // 单层仍使用生产稳定映射。
    layers.insert(
        layer,
        pending_layer(
            layer,
            revision,
            &[0, 0, 0, 255],
            PendingBladeDensityState::Requested,
        ),
    );
    let coordinator = VegetationGpuMipCoordinator {
        next_ticket: ticket, // 当前票据已经分配。
        update_index: 3,     // 任意未超时Update。
        pending: Some(PendingBladeDensityBatch {
            ticket,                      // 结果必须匹配该票据。
            revision,                    // 结果必须匹配该版本。
            configuration_generation: 1, // 任意稳定测试配置代次。
            layers,                      // 当前仅一个串行层。
            started_update: 2,           // 尚未超过等待上限。
            failure: None,               // 初始无错误。
        }),
    };
    let request = VegetationGpuMipRequest {
        job: Some(VegetationGpuMipJob {
            ticket,                               // 与批次一致。
            revision,                             // 与批次一致。
            layer,                                // 与Requested层一致。
            image: Handle::<Image>::default(),    // 纯结果测试不解析RenderAsset。
            width: 1,                             // 空层基础级宽。
            height: 1,                            // 空层基础级高。
            mip_level_count: 1,                   // 1×1完整链只有一级。
            base: VegetationGpuBaseJob::Uploaded, // 当前测试只消费版本化结果。
        }),
    };
    (layer, coordinator, request) // 返回可单独注入收件箱的当前状态。
}

#[test]
fn stale_ticket_cannot_advance_or_clear_current_request() {
    let (layer, mut coordinator, mut request) = requested_fixture(); // 当前票据为九。
    let mut inbox = VegetationGpuMipInbox::default(); // 模拟取消批次迟到回调。
    inbox.results.push_back(VegetationGpuMipResult {
        ticket: 8,                                                             // 旧票据必须被拒绝。
        revision: 7, // 即使版本相同也不能冒充当前批次。
        layer,       // 层相同仍不足以通过身份门。
        outcome: VegetationGpuMipOutcome::Complete(Arc::from([0, 0, 0, 255])), // 字节本身正确。
    });
    consume_gpu_results(&mut coordinator, &mut request, &mut inbox); // 执行生产结果消费。
    let pending = coordinator.pending.as_ref().unwrap(); // 当前批次仍存在。
    assert_eq!(
        pending.layers[&layer].state,
        PendingBladeDensityState::Requested
    ); // 旧票据不能把当前层标为通过。
    assert!(request.job.is_some()); // 当前请求也不能被迟到回调清除。
}

#[test]
fn exact_current_result_validates_layer_and_releases_serial_request() {
    let (layer, mut coordinator, mut request) = requested_fixture(); // 构造当前请求身份。
    let mut inbox = VegetationGpuMipInbox::default(); // 模拟RenderApp完成队列。
    inbox.results.push_back(VegetationGpuMipResult {
        ticket: 9,                                                             // 与当前批次一致。
        revision: 7, // 与当前活动版本一致。
        layer,       // 与当前Requested层一致。
        outcome: VegetationGpuMipOutcome::Complete(Arc::from([0, 0, 0, 255])), // 完整1×1参考字节一致。
    });
    consume_gpu_results(&mut coordinator, &mut request, &mut inbox); // 执行生产严格比较。
    let pending = coordinator.pending.as_ref().unwrap(); // 批次等待最终原子采用。
    assert_eq!(
        pending.layers[&layer].state,
        PendingBladeDensityState::Validated
    ); // 只有逐字节完全相同才进入通过态。
    assert!(pending.failure.is_none()); // 正确结果不写失败原因。
    assert!(request.job.is_none()); // 当前层完成后允许发布下一稳定层。
}

#[test]
fn one_byte_difference_fails_the_whole_candidate_batch() {
    let (layer, mut coordinator, mut request) = requested_fixture(); // 构造当前请求身份。
    let mut inbox = VegetationGpuMipInbox::default(); // 模拟GPU返回Alpha差异。
    inbox.results.push_back(VegetationGpuMipResult {
        ticket: 9,                                                             // 当前票据。
        revision: 7,                                                           // 当前版本。
        layer,                                                                 // 当前层。
        outcome: VegetationGpuMipOutcome::Complete(Arc::from([0, 0, 0, 254])), // 只差最后一个Alpha字节。
    });
    consume_gpu_results(&mut coordinator, &mut request, &mut inbox); // 无容差比较必须拒绝。
    let failure = coordinator
        .pending
        .as_ref()
        .and_then(|pending| pending.failure.as_deref())
        .expect("one byte mismatch must fail the batch"); // 整批保留可读首差异原因。
    assert!(failure.contains("first byte mismatch at 3")); // 诊断精确到RGBA紧凑字节位置。
    assert!(request.job.is_none()); // 失败层不再重复提交同请求。
}

#[test]
fn batch_completion_requires_every_bound_layer() {
    let revision = 4; // 两层共享一个活动版本。
    let first = VegetationLayerId(2); // 稳定较小层。
    let second = VegetationLayerId(8); // 稳定较大层。
    let mut layers = BTreeMap::new(); // 生产批次按ID排序。
    layers.insert(
        first,
        pending_layer(
            first,
            revision,
            &[0, 0, 0, 255],
            PendingBladeDensityState::Validated,
        ),
    );
    layers.insert(
        second,
        pending_layer(
            second,
            revision,
            &[0, 0, 0, 255],
            PendingBladeDensityState::Requested,
        ),
    );
    let mut batch = PendingBladeDensityBatch {
        ticket: 5,                   // 两层共享批次票据。
        revision,                    // 两层共享活动版本。
        configuration_generation: 1, // 两层共享正式配置代次。
        layers,                      // 一个通过、一个仍在途。
        started_update: 1,           // 超时与本测试无关。
        failure: None,               // 没有错误也不能部分提交。
    };
    assert!(!batch.is_complete()); // 任一层未通过时整批不得采用。
    batch.layers.get_mut(&second).unwrap().state = PendingBladeDensityState::Validated; // 模拟第二层严格通过。
    assert!(batch.is_complete()); // 全部绑定层通过后才允许原子提交。
}

#[test]
fn byte_comparison_rejects_length_before_content() {
    assert!(
        compare_gpu_mip_bytes(&[1, 2], &[1])
            .unwrap_err()
            .contains("length")
    ); // 短回读不能部分成功。
    assert!(compare_gpu_mip_bytes(&[1, 2], &[1, 2]).is_ok()); // 完全相同字节通过。
}
