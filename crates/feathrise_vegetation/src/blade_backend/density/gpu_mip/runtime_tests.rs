//! 显式门控的真实Bevy RenderApp候选Mip与版本化回读烟雾。

use std::sync::Arc; // 运行时烟雾夹具按正式活动布局共享不可变解析块。
use std::time::{Duration, Instant}; // 手动Update轮询有明确总超时且每轮短暂让出CPU。

use bevy::prelude::{App, Assets, DefaultPlugins, IVec2, Image, PluginGroup, Rect, WindowPlugin}; // 构造无窗口完整渲染应用和128×64活动布局。
use bevy::render::pipelined_rendering::PipelinedRenderingPlugin; // 测试禁用独立渲染线程以保持手动Update确定性。
use bevy::render::renderer::RenderAdapterInfo; // 核对Bevy实际选择了要求的Vulkan或DX12。
use bevy::render::{RenderApp, RenderPlugin}; // 安装同步编译渲染子应用并读取真实适配器。
use bevy::window::ExitCondition; // 无窗口应用不得因窗口计数自动退出。
use bevy::winit::WinitPlugin; // 测试不创建操作系统窗口或事件循环。

use super::*; // 调用生产安装入口并使用正式调度元数据。
use crate::blade_backend::density::atlas::build_layer_density_atlas; // 候选基础级和CPU完整链来自正式参考路径。
use crate::blade_backend::density::resources::{
    VegetationGpuBaseJob, VegetationGpuMipInbox, VegetationGpuMipJob, VegetationGpuMipOutcome,
    VegetationGpuMipRequest, VegetationGpuMipResult,
}; // 主世界直接提交一个候选并读取版本化回调结果。
use crate::layout::{VEGETATION_TILE_SIDE, VEGETATION_TILE_TEXEL_COUNT}; //  fixture严格跨两个64×64公共块。
use crate::{
    ActiveVegetationLayout, VegetationDirtyTile, VegetationLayerId, VegetationResolvedLayer,
    VegetationResolvedTile,
}; // 构造包含正负块与非均匀八位端点的活动布局。

mod competition; // P5.2b2b正式未决输入、三种竞争模式和局部基础级复制使用同一RenderApp继续验收。

/// 只有显式设置该变量时才创建完整Bevy RenderApp和真实GPU设备。
const RENDERAPP_GPU_SMOKE_ENV: &str = "VEGETATION_LAYOUT_RENDERAPP_MIP_SMOKE"; // 普通CI和单元测试保持无设备依赖。

#[test]
fn render_app_generates_and_reads_back_complete_versioned_mips() {
    if std::env::var_os(RENDERAPP_GPU_SMOKE_ENV).is_none() {
        return; // 默认测试不隐式初始化显卡或占用图形驱动。
    }
    let expected_backend = requested_backend(); // 门控开启后必须明确指定正式Vulkan或DX12。
    let layer = VegetationLayerId(0x52B2); // 独立于P5.2b1设备测试的稳定运行时层ID。
    let revision = 17; // 非零活动版本用于核对跨世界身份。
    let ticket = 23; // 非零票据用于核对回调没有丢失或改写。
    let atlas = runtime_fixture_atlas(layer, revision); // 128×64非均匀输入生成八级完整链。
    let expected = atlas
        .rgba_mip_bytes()
        .expect("runtime smoke CPU reference bytes should encode"); // GPU结果逐字节权威。
    let candidate = atlas
        .to_gpu_candidate_image()
        .expect("runtime smoke candidate image should encode"); // 子Mip显式为零，必须由RenderGraph覆盖。
    let width = atlas.base_width().unwrap(); // 候选基础级真实宽。
    let height = atlas.base_height().unwrap(); // 候选基础级真实高。
    let mip_level_count = atlas.mip_level_count().unwrap(); // 128×64完整链应有八级。

    let mut app = App::new(); // 使用完整默认插件建立真实Bevy RenderApp。
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,                    // 无头测试不创建交换链窗口。
                exit_condition: ExitCondition::DontExit, // 手动Update循环自行决定结束。
                ..Default::default()                     // 其他窗口策略保持Bevy默认。
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true, // 烟雾只验证运行时资源和命令，不测试异步编译延迟。
                ..Default::default()                    // 适配器选择继续遵循显式WGPU_BACKEND。
            })
            .disable::<WinitPlugin>() // 避免无显示环境初始化操作系统事件循环。
            .disable::<PipelinedRenderingPlugin>(), // 渲染子应用留在当前线程供手动Update驱动。
    );
    app.init_resource::<VegetationGpuMipRequest>() // ExtractResourcePlugin要求主世界始终存在请求。
        .init_resource::<VegetationGpuMipInbox>(); // 回调发布目标在首次提取前显式建立。
    assert!(install(&mut app)); // 使用生产安装入口注册内部Shader、管线、节点和map_async系统。
    let image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(candidate); // 候选先进入主世界图片表，后续由正常RenderAsset流程上传。
    app.world_mut()
        .resource_mut::<VegetationGpuMipRequest>()
        .job = Some(VegetationGpuMipJob {
        ticket,                               // 结果必须原样返回。
        revision,                             // 结果必须原样返回。
        layer,                                // 结果必须原样返回。
        image,                                // RenderApp从该强句柄解析候选GpuImage。
        width,                                // 验证GPU基础级宽。
        height,                               // 验证GPU基础级高。
        mip_level_count,                      // 验证完整链级数。
        base: VegetationGpuBaseJob::Uploaded, // 本项继续隔离验证P5.2b2a基础级上传与Mip闭环。
    });
    app.finish(); // RenderPlugin在finish阶段创建并注入真实设备、队列和适配器。
    app.cleanup(); // 应用插件cleanup后才进入手动Update循环。

    let adapter_info = app
        .get_sub_app(RenderApp)
        .expect("runtime smoke should retain non-pipelined RenderApp")
        .world()
        .resource::<RenderAdapterInfo>(); // 读取Bevy实际选择的设备身份。
    assert_eq!(
        adapter_info.backend, expected_backend,
        "Bevy RenderApp did not honor the explicitly requested backend"
    ); // 防止Vulkan与DX12烟雾静默跑到同一个后端。
    println!(
        "vegetation RenderApp mip smoke: adapter='{}', backend={:?}, driver='{}'",
        adapter_info.name, adapter_info.backend, adapter_info.driver
    ); // 日志保存正式双后端机器证据。

    let deadline = Instant::now() + Duration::from_secs(30); // 管线、上传、Compute、复制和回调总等待有界。
    let result = loop {
        app.update(); // 驱动主世界提取、RenderAsset准备、根图提交和下一帧映射。
        if let Some(result) = take_result(&mut app, ticket, revision, layer) {
            break result; // 当前身份完整结果到达即可验收。
        }
        assert!(
            Instant::now() < deadline,
            "Bevy RenderApp GPU mip result did not arrive within 30 seconds"
        ); // 驱动或设备丢失不能让门控测试永久挂住。
        std::thread::sleep(Duration::from_millis(5)); // 让WGPU回调线程获得执行机会且不长时间阻塞更新。
    };
    match result.outcome {
        VegetationGpuMipOutcome::Complete(actual) => {
            assert_eq!(
                actual.as_ref(),
                expected.as_slice(),
                "Bevy RenderApp GPU mip bytes diverged on {expected_backend:?}"
            ); // 基础级和七个GPU子级全部RGBA字节无容差一致。
        }
        VegetationGpuMipOutcome::Failed(reason) => {
            panic!("Bevy RenderApp GPU mip runtime failed on {expected_backend:?}: {reason}");
        } // 任何管线、资源、映射或解码失败都使烟雾失败。
    }
    competition::validate_gpu_competition(&mut app, expected_backend); // 复用同一真实设备继续执行正式基础级竞争和局部提交闭环。
}

/// 从主世界收件箱取出当前身份结果并保留其他迟到回调。
fn take_result(
    app: &mut App,            // 手动驱动的完整应用。
    ticket: u64,              // 当前批次票据。
    revision: u64,            // 当前活动版本。
    layer: VegetationLayerId, // 当前串行层。
) -> Option<VegetationGpuMipResult> {
    let mut inbox = app.world_mut().resource_mut::<VegetationGpuMipInbox>(); // 只在两次Update之间短借用。
    let index = inbox.results.iter().position(|result| {
        result.ticket == ticket && result.revision == revision && result.layer == layer // 三重身份必须完全一致。
    })?;
    inbox.results.remove(index) // VecDeque按小队列下标移除当前结果。
}

/// 解析门控要求的唯一GPU后端。
fn requested_backend() -> wgpu::Backend {
    let backend = std::env::var("WGPU_BACKEND")
        .expect("RenderApp GPU mip smoke requires WGPU_BACKEND=vulkan or WGPU_BACKEND=dx12")
        .to_ascii_lowercase(); // 接受PowerShell设置时的大小写差异。
    match backend.as_str() {
        "vulkan" => wgpu::Backend::Vulkan, // Bevy实例必须实际选择Vulkan。
        "dx12" => wgpu::Backend::Dx12,     // Bevy实例必须实际选择DX12。
        other => panic!(
            "unsupported WGPU_BACKEND='{other}' for RenderApp GPU mip smoke; use 'vulkan' or 'dx12'"
        ), // P5.2b2a只验收Windows正式双后端。
    }
}

/// 构造跨负坐标、块边界和全部八位端点的128×64运行时参考图集。
fn runtime_fixture_atlas(
    layer: VegetationLayerId, // 测试绑定层。
    revision: u64,            // 测试活动版本。
) -> crate::blade_backend::density::atlas::ResolvedDensityAtlas {
    let left = VegetationDirtyTile(IVec2::new(-1, 0)); // 负块验证图集偏移不依赖正象限。
    let right = VegetationDirtyTile(IVec2::ZERO); // 相邻块让高Mip跨越64像素边界平均。
    let mut layout = ActiveVegetationLayout {
        revision,             // 候选身份与图集内容同一次活动快照。
        texel_size: 1.0,      // 每个基础纹素覆盖一米。
        ..Default::default()  // 只插入当前测试两块。
    };
    let left_density = (0..VEGETATION_TILE_TEXEL_COUNT)
        .map(|index| ((index * 37 + index / VEGETATION_TILE_SIDE * 11) % 256) as f32 / 255.0)
        .collect::<Vec<_>>(); // 非对称周期暴露X/Y寻址和整数恢复误差。
    let right_density = (0..VEGETATION_TILE_TEXEL_COUNT)
        .map(|index| (255 - ((index * 73 + 19) % 256)) as f32 / 255.0)
        .collect::<Vec<_>>(); // 第二块使用不同周期防止边界平均偶然恒定。
    layout.tiles.insert(
        left,
        Arc::new(resolved_runtime_tile(left.0, layer, left_density)),
    ); // 左半基础级来自正式解析块结构。
    layout.tiles.insert(
        right,
        Arc::new(resolved_runtime_tile(right.0, layer, right_density)),
    ); // 右半基础级来自正式解析块结构。
    build_layer_density_atlas(&layout, layer).expect("runtime smoke atlas should build") // P5.2a CPU完整链是唯一权威。
}

/// 构造一块已经通过核心竞争解析的运行时测试数据。
fn resolved_runtime_tile(
    coordinate: IVec2,        // 块世界坐标。
    layer: VegetationLayerId, // 当前唯一测试层。
    density: Vec<f32>,        // 恰好4096项归一化概率。
) -> VegetationResolvedTile {
    VegetationResolvedTile {
        coordinate: VegetationDirtyTile(coordinate), // 坐标决定128×64图集中的水平位置。
        world_bounds: Rect::from_corners(
            coordinate.as_vec2() * VEGETATION_TILE_SIDE as f32, // 块左下世界角。
            (coordinate + IVec2::ONE).as_vec2() * VEGETATION_TILE_SIDE as f32, // 块右上闭开外边。
        ),
        layers: vec![VegetationResolvedLayer { layer, density }], // 本烟雾只需要一个解析层。
    }
}
