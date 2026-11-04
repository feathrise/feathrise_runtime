//! 在Bevy Solari加载pipeline前覆盖DI shader，固化已验spatial策略、视线Fresnel修正与动态地形主表面镜面隔离。

use std::{path::Path, path::PathBuf}; // 内嵌注册同时需要稳定虚拟路径与watcher源路径。

use bevy::asset::io::embedded::EmbeddedAssetRegistry; // 在Solari注册后、RenderStartup建pipeline前覆盖同名资源。
use bevy::prelude::App; // 稳定策略只在插件build期安装，不增加任何运行时ECS系统。

const RESTIR_DI_EMBEDDED_PATH: &str = "bevy_solari/realtime/restir_di.wgsl"; // 精确替换官方0.19.1直接光root。
const RESTIR_DI_SOURCE_PATH: &str = "src/denoising/shaders/restir_di_stability.wgsl"; // watcher身份与include_bytes来源保持同一正式稳定策略文件。

/// 安装DI spatial稳定策略、漫反射视线Fresnel兼容修正与已验收的动态地形主表面镜面隔离；不写历史reset或增加逐帧CPU工作。
pub(crate) fn install_di_stability_shader(app: &mut App) {
    let registry = app.world().resource::<EmbeddedAssetRegistry>(); // 此时Solari官方资源已注册且pipeline尚未创建。
    registry.insert_asset(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(RESTIR_DI_SOURCE_PATH), // watcher跟踪正式DI稳定策略源码。
        Path::new(RESTIR_DI_EMBEDDED_PATH), // 覆盖DI root，不触碰sampling、BRDF或GI虚拟路径。
        &include_bytes!("shaders/restir_di_stability.wgsl")[..], // spatial入口短路邻居复用，DI漫反射保留已验相机侧修正并隔离动态地形主表面镜面。
    ); // 冷启动固定使用已验策略，不产生reset或稳定帧CPU写入。
}

#[cfg(test)]
mod tests {
    use super::*; // 直接检查实际编入二进制的虚拟路径与WGSL稳定契约。

    #[test]
    fn di_stability_shader_disables_spatial_reuse_while_preserving_initial_and_temporal() {
        let source = include_str!("shaders/restir_di_stability.wgsl"); // 测试真实覆盖源码而非另造fixture。
        assert_eq!(
            RESTIR_DI_EMBEDDED_PATH,
            "bevy_solari/realtime/restir_di.wgsl"
        ); // Bevy升级导致虚拟路径漂移时必须显式失败。
        assert!(source.contains("const DISABLE_DI_SPATIAL_REUSE: bool = true;")); // 正式稳定基线必须从首个有效帧停用DI同帧spatial复用。
        let spatial_function = source
            .find("fn load_spatial_reservoir(")
            .expect("官方DI spatial入口必须保留"); // 从函数边界内检查稳定策略分支，避免误命中常量声明。
        let spatial_policy = source[spatial_function..]
            .find("if DISABLE_DI_SPATIAL_REUSE")
            .map(|offset| spatial_function + offset)
            .expect("DI spatial入口必须检查稳定策略常量"); // 精确定位正式稳定行为分支。
        let neighbor_pixel_load = source[spatial_function..]
            .find("let spatial_pixel_id = get_neighbor_pixel_id")
            .map(|offset| spatial_function + offset)
            .expect("官方DI邻居采样主体必须保留"); // 保留上游主体，便于版本升级时逐项重审。
        let neighbor_gbuffer_load = source[neighbor_pixel_load..]
            .find("textureLoad(gbuffer, spatial_pixel_id")
            .map(|offset| neighbor_pixel_load + offset)
            .expect("官方DI邻居GBuffer读取必须保留"); // 锁定spatial实际消费的是当前帧邻居表面。
        let neighbor_reservoir_load = source[neighbor_gbuffer_load..]
            .find("load_reservoir_b(spatial_pixel_id)")
            .map(|offset| neighbor_gbuffer_load + offset)
            .expect("官方DI邻居reservoir读取必须保留"); // 锁定spatial实际消费的是同帧B reservoir。
        assert!(spatial_policy < neighbor_pixel_load); // 稳定策略必须先于邻居坐标生成。
        assert!(spatial_policy < neighbor_gbuffer_load); // 稳定策略必须先于邻居深度与GBuffer解析。
        assert!(spatial_policy < neighbor_reservoir_load); // 稳定策略必须先于邻居reservoir读取。
        assert!(source[spatial_policy..neighbor_pixel_load].contains(
            "return NeighborInfo(empty_reservoir(), world_position, world_normal, vec3(0.0));"
        )); // 禁用态只返回空邻居候选，不能清空当前像素的initial/temporal reservoir。
        assert_eq!(source.matches("trace_light_visibility(").count(), 2); // 上轮无效visibility旁路已撤销，两次官方遮挡射线恢复。
        assert!(!source.contains("BYPASS_PRIMARY_DI_VISIBILITY")); // 不允许叠加上一轮已经证伪的实验变量。
        assert!(!source.contains("SYNCHRONIZED_DARK_AB")); // 已证伪的实验常量不得混入正式稳定源码。
        assert!(source.contains("let motion_vector = textureLoad(motion_vectors")); // DI motion重投影恢复官方路径。
        assert!(source.contains("textureLoad(previous_gbuffer")); // 上一帧GBuffer读取恢复官方路径。
        assert!(source.contains("load_reservoir_a(temporal_pixel_id)")); // 上一帧DI reservoir恢复官方路径。
        assert!(source.contains("generate_initial_reservoir(")); // 每帧DI initial继续生成当前样本。
        assert!(source.contains("load_temporal_reservoir(")); // DI temporal复用恢复，保持spatial为唯一变量。
        assert!(source.contains("evaluate_di_diffuse_brdf_without_view_fresnel(")); // DI漫反射使用本项目已验掠射视角兼容路径。
        assert!(source.contains("textureStore(view_output")); // DI仍是Solari不透明HDR基础写色阶段。
    }

    #[test] // 正式DI移除已验视线Fresnel、恢复第七轮相机侧基线，并按第十轮实机结论隔离动态地形主表面镜面。
    fn di_ground_darkening_fix_keeps_smooth_diffuse_and_isolates_dynamic_specular() {
        let source = include_str!("shaders/restir_di_stability.wgsl"); // 检查实际覆盖Solari DI root的同一份WGSL。
        assert!(!source.contains("SHOW_DI_FRESNEL_SPLIT_FOR_GROUND_DARKENING_DEBUG")); // 实机定位完成后不得把六分屏留在正常画面。
        assert!(!source.contains("panel_width")); // 正式写色不得继续按屏幕坐标分区。
        let helper_offset = source
            .find("fn evaluate_di_diffuse_brdf_without_view_fresnel(")
            .expect("DI必须保留独立且可审计的漫反射兼容函数"); // 与共享GI/path tracing BRDF隔离，避免扩大修正范围。
        let next_function_offset = source[helper_offset..]
            .find("fn generate_initial_reservoir(")
            .map(|offset| helper_offset + offset)
            .expect("DI initial主体必须紧随兼容函数保留"); // 只截取兼容函数检查具体能量项。
        let helper = &source[helper_offset..next_function_offset]; // 后续官方reservoir源码不参与本项字符串判定。
        assert!(helper.contains("let NdotL = dot(world_normal, wi)")); // 入射余弦必须继续来自正式法线与选中光线。
        assert!(helper.contains("let view_NdotV = dot(view_side_normal, wo)")); // 动态地形相机侧必须继续读取真实三角guide。
        assert!(helper.contains("NdotL < 0.0001 || view_NdotV < 0.0001")); // 第七轮已验基线只组合平滑光源侧与几何相机侧。
        assert!(!helper.contains("reflection_NdotL")); // 第九轮实机无效的几何光源侧门必须完整撤回。
        assert!(!helper.contains("let NdotV = dot(world_normal, wo)")); // 动态地形不能再因平滑法线越过相机半球而整段归零。
        assert!(helper.contains("calculate_diffuse_color(")); // base color、metallic与传输材质语义保持Bevy PBR公式。
        assert!(helper.contains("calculate_F0(")); // reflectance到F0的正式映射不得被常量近似替代。
        assert!(helper.contains("let incident_layering = 1.0 - fresnel(F0, NdotL)")); // 入射侧Fresnel必须保留。
        assert!(!helper.contains("fresnel(F0, NdotV)")); // 已证实制造泛黑的视线侧Fresnel不得重新乘回漫反射。
        assert!(helper.contains("return diffuse_color * incident_layering * NdotL")); // 余弦与Lambert归一化后的最终关系必须固定。
        assert!(source.contains("let ray_surface = primary_ray_surface(surface)")); // 动态地形最终DI必须只惰性解析一次三角数据。
        assert!(source.contains("ray_surface.view_side_normal")); // 同次解析的真实几何法线必须服务相机侧硬门。
        assert!(!source.contains("di_diffuse_view_side_normal(surface)")); // 旧A槽guide捷径会破坏大小地图标准平滑法线，禁止回归。
        assert!(!source.contains("di_diffuse_shading_normal(")); // 第八轮实机无效的连续视线法线不得继续叠加。
        let dynamic_specular_gate = // 保存第十轮已验收的普通材质镜面分流条件源码片段。
            "if surface.dynamic_terrain == 0u && surface.material.roughness >"; // 锁定动态地形必须跳过的直接镜面门。
        assert!(source.contains(dynamic_specular_gate)); // 第十轮实机确认后正式阻断动态地形直接镜面，普通材质继续按官方阈值分流。
        assert!(source.contains("brdf += evaluate_specular_brdf(")); // 普通材质DI镜面瓣继续使用Bevy Solari 0.19.1官方实现。
        assert_eq!(source.matches("trace_light_visibility(").count(), 2); // 两次visibility不得因BRDF修正被旁路。
    } // 完成掠射视角漫反射兼容修正源码契约。
}
