//! 在Bevy Solari加载pipeline前覆盖GI shader，固化已通过实机长时验证的GI稳定策略。

use std::{path::Path, path::PathBuf}; // 内嵌注册需要稳定虚拟路径与watcher真实源路径。

use bevy::asset::io::embedded::EmbeddedAssetRegistry; // 在Solari插件build后、finish前覆盖同名资源。
use bevy::prelude::App; // 生产保护属于Solari GI，不依赖NVIDIA厂商重建器是否启用。

const RESTIR_GI_EMBEDDED_PATH: &str = "bevy_solari/realtime/restir_gi.wgsl"; // 只覆盖GI root，DI与cache保持官方实现。
const RESTIR_GI_SOURCE_PATH: &str = "src/denoising/shaders/restir_gi_stability.wgsl"; // watcher身份与include_bytes来源保持同一正式稳定策略文件。

/// 安装全屏GI temporal稳定策略；不写任何reset，也不增加运行时CPU工作。
pub(crate) fn install_gi_stability_shader(app: &mut App) {
    let registry = app.world().resource::<EmbeddedAssetRegistry>(); // Solari插件已注册官方同名资源且尚未进入RenderStartup。
    registry.insert_asset(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RESTIR_GI_SOURCE_PATH), // watcher跟踪正式GI稳定策略源码。
        Path::new(RESTIR_GI_EMBEDDED_PATH), // 替换官方GI root，不触碰DI或world-cache root。
        &include_bytes!("shaders/restir_gi_stability.wgsl")[..], // GI副本只在shader内跳过上一帧reservoir。
    ); // 首帧、切镜与尺寸变化继续由官方Solari reset统一失效历史。
}

#[cfg(test)]
mod tests {
    use super::*; // 直接锁定编入二进制的正式GI稳定策略与官方虚拟路径。

    #[test]
    fn gi_stability_shader_disables_temporal_reuse_without_resetting_current_frame_sampling() {
        let gi_source = include_str!("shaders/restir_gi_stability.wgsl"); // 对实际编入二进制的正式源码做契约检查。
        assert_eq!(
            RESTIR_GI_EMBEDDED_PATH,
            "bevy_solari/realtime/restir_gi.wgsl"
        ); // 官方0.19路径漂移时必须显式失败。
        assert!(gi_source.contains("fn initial_and_temporal(")); // GI初始与时间阶段入口保持官方形状。
        assert!(gi_source.contains("fn spatial_and_shade(")); // 同帧空间复用和着色保持官方行为。
        assert!(gi_source.contains("const DISABLE_GI_TEMPORAL_REUSE: bool = true;")); // 正式稳定基线从首个有效帧停用跨帧GI reservoir复用。
        assert!(gi_source.contains(
            "DISABLE_GI_TEMPORAL_REUSE || current_is_gpu_vegetation || bool(constants.reset)"
        )); // 正式策略只截断上一帧GI reservoir，真实reset与植被类别保护仍保持可见。
        let temporal_policy_offset = gi_source
            .find("if DISABLE_GI_TEMPORAL_REUSE")
            .expect("GI稳定策略必须在temporal入口执行"); // 位置契约防止未来把拒绝逻辑移到历史读取之后。
        let motion_load_offset = gi_source
            .find("let motion_vector = textureLoad(motion_vectors")
            .expect("官方GI motion重投影主体必须保留"); // 正式策略仍保留未来升级时可审计的官方temporal主体。
        assert!(temporal_policy_offset < motion_load_offset); // 必须在motion、previous GBuffer和reservoir A读取前短路。
        assert!(
            gi_source
                .contains("gpixel_is_gpu_vegetation(temporal_gpixel) != current_is_gpu_vegetation")
        ); // 裸地不得继承上一帧植被reservoir。
        let temporal_class_offset = gi_source
            .find("if gpixel_is_gpu_vegetation(temporal_gpixel) != current_is_gpu_vegetation")
            .expect("GI temporal保险门必须按上一帧原始GBuffer分类"); // 即使当前全屏短路，未来恢复temporal也不能绕过类别门。
        let temporal_resolve_offset = gi_source
            .find("let temporal_surface = gpixel_resolve(temporal_gpixel")
            .expect("官方GI temporal表面解析主体必须保留"); // 类别门必须发生在上一帧材质解析之前。
        let temporal_reservoir_offset = gi_source
            .find("let temporal_reservoir = gi_reservoirs_a")
            .expect("官方上一帧GI reservoir读取主体必须保留"); // 未来恢复temporal时仍从A缓冲读取。
        assert!(
            temporal_class_offset < temporal_resolve_offset
                && temporal_resolve_offset < temporal_reservoir_offset
        ); // previous类别不兼容时不得解析或读取其历史reservoir。
        assert!(
            gi_source
                .contains("gpixel_is_gpu_vegetation(spatial_gpixel) != current_is_gpu_vegetation")
        ); // 同帧30像素空间复用不得跨植被类别。
        let spatial_class_offset = gi_source
            .find("if gpixel_is_gpu_vegetation(spatial_gpixel) != current_is_gpu_vegetation")
            .expect("GI spatial必须按原始GBuffer类别过滤"); // 类别判断不能等到解析邻居材质后才执行。
        let spatial_class_block = &gi_source[spatial_class_offset..]; // 只检查该类别分支的首段，避免误命中后续循环控制。
        let spatial_continue_offset = spatial_class_block
            .find("continue;")
            .expect("GI spatial跨类候选必须跳过"); // 不能return empty清掉当前canonical样本。
        let spatial_block_end = spatial_class_block
            .find("}")
            .expect("GI spatial类别分支必须闭合"); // continue必须属于mismatch分支自身。
        assert!(spatial_continue_offset < spatial_block_end); // 跨类分支只能跳过候选，不能提前终止整个spatial阶段。
        let spatial_resolve_offset = gi_source
            .find("let spatial_surface = gpixel_resolve(spatial_gpixel")
            .expect("官方GI spatial表面解析必须保留"); // 只改变类别兼容性，不改材质解析。
        let spatial_reservoir_offset = gi_source
            .find("let spatial_reservoir = gi_reservoirs_b")
            .expect("官方当前帧GI reservoir读取必须保留"); // spatial仍读取本帧B缓冲，不退化为禁用。
        assert!(
            spatial_class_offset < spatial_resolve_offset
                && spatial_resolve_offset < spatial_reservoir_offset
        ); // 跨类候选必须在解析表面与读取reservoir前直接continue。
        assert!(gi_source.contains("for (var i = 0u; i < 5u; i++)")); // 保持Bevy 0.19每像素五次spatial候选预算。
        assert!(gi_source.contains("const SPATIAL_REUSE_RADIUS_PIXELS = 30.0;")); // 保持已实机验证的30像素搜索半径。
        assert!(gi_source.contains("query_world_cache(")); // world cache查询与正常混合路径不得因副本缩减而丢失。
        assert!(gi_source.contains("generate_initial_reservoir(")); // 每帧GI initial仍保留，不能把整段GI照明误删。
        assert!(gi_source.contains("load_spatial_reservoir(")); // 同帧spatial仍保留，确保只翻转temporal一项。
    }
}
