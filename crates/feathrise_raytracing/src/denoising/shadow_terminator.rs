//! 在Solari创建管线前安装动态低模地形的标准GBuffer兼容解析、DI几何相机侧、主表面镜面隔离与虚拟平滑面起点修正。

use std::{path::Path, path::PathBuf}; // 光泽root覆盖需要稳定虚拟路径和可热更源路径。

use bevy::asset::io::embedded::EmbeddedAssetRegistry; // 在Solari注册官方资源后精确覆盖光泽root。
use bevy::prelude::App; // 安装只发生在插件build阶段，不引入任何逐帧ECS系统。
use bevy::shader::load_shader_library; // 项目GBuffer扩展必须作为独立永久库载入，不能只登记未请求的内嵌字节。

const SPECULAR_GI_EMBEDDED_PATH: &str = "bevy_solari/realtime/specular_gi.wgsl"; // 只把光泽路径首条射线接到新起点函数。
const SPECULAR_GI_SOURCE_PATH: &str = "src/denoising/shaders/specular_gi_shadow_terminator.wgsl"; // watcher跟踪0.19.1兼容副本。

#[cfg(test)]
#[path = "shadow_terminator_tests.rs"]
mod projection_tests;

/// 安装GBuffer兼容元数据独立库与光泽主射线覆盖；DI/GI root随后从同一虚拟库导入安全解析、DI几何相机侧与起点函数。
pub(crate) fn install_dynamic_terrain_shadow_terminator_shaders(app: &mut App) {
    load_shader_library!(app, "shaders/gbuffer_shadow_terminator.wgsl"); // 永久加载独立#import模块，避免Solari官方gbuffer_utils已进入Shader资产缓存后遮蔽扩展函数。
    let registry = app.world().resource::<EmbeddedAssetRegistry>(); // SolariPlugins已在上一行build完成官方内嵌登记。
    registry.insert_asset(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SPECULAR_GI_SOURCE_PATH), // 热更身份指向光泽兼容副本。
        Path::new(SPECULAR_GI_EMBEDDED_PATH), // 替换官方光泽GI root并继续导出其两个粗糙度常量。
        &include_bytes!("shaders/specular_gi_shadow_terminator.wgsl")[..], // 只改变首条光泽射线起点的一份完整0.19.1副本。
    ); // 完成光泽GI root覆盖。
} // 完成插件期Shader安装。

#[cfg(test)] // 路径、标准GBuffer字段、DI几何相机侧、镜面隔离、惰性顶点读取和三条实时主射线链由源码契约共同锁定。
mod tests {
    use super::*; // 复用生产虚拟路径常量，避免测试复制第二份模块身份。

    // 测试直接读取编入二进制的四份正式WGSL，不维护脱离运行时的夹具。
    const GBUFFER_SOURCE: &str = include_str!("shaders/gbuffer_shadow_terminator.wgsl"); // 公共GBuffer兼容解析与虚拟平滑面算法。
    const SPECULAR_GI_SOURCE: &str = include_str!("shaders/specular_gi_shadow_terminator.wgsl"); // 光泽主路径覆盖。
    const RESTIR_DI_SOURCE: &str = include_str!("shaders/restir_di_stability.wgsl"); // 已验DI稳定root叠加新起点。
    const RESTIR_GI_SOURCE: &str = include_str!("shaders/restir_gi_stability.wgsl"); // 已验GI稳定root叠加新起点。

    #[test] // Bevy升级或虚拟资产路径漂移时必须在CPU测试中明确失败。
    fn shadow_terminator_uses_unique_library_and_overrides_exact_solari_root() {
        assert_eq!(
            SPECULAR_GI_EMBEDDED_PATH,
            "bevy_solari/realtime/specular_gi.wgsl"
        ); // 锁定光泽root身份。
    } // 完成虚拟路径契约测试。

    #[test] // 动态地形必须从标准A槽读取平滑受光法线，并从指数安全G槽恢复完整三角身份。
    fn gbuffer_preserves_smooth_normal_and_lazily_recovers_geometric_guide() {
        assert!(
            GBUFFER_SOURCE.contains("const DEFERRED_FLAGS_DYNAMIC_TERRAIN_BIT: u32 = 1u << 3u;")
        ); // 与地表输出共用第四flag位。
        assert!(
            GBUFFER_SOURCE
                .contains("#define_import_path feathrise_raytracing::gbuffer_shadow_terminator")
        ); // 三个root必须能绕开Solari官方模块缓存显式导入扩展函数。
        assert!(
            GBUFFER_SOURCE
                .contains("let world_normal = octahedral_decode(unpack_24bit_normal(gpixel.a));")
        ); // A槽平滑/草土微法线必须同时服务大小地图、RR guide和项目Solari。
        assert!(
            GBUFFER_SOURCE
                .contains("const DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK: u32 = 0x7FFFFu;")
        ); // 解码端必须只消费指数安全的G槽低十九位。
        assert!(
            GBUFFER_SOURCE.contains(
                "let primitive_index = gpixel.g & DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK;"
            )
        ); // 十九位三角身份必须从单一槽无损恢复。
        assert!(!GBUFFER_SOURCE.contains("unpack_dynamic_terrain_smooth_normal")); // 旧G槽法线编码会被大小地图误解为发光，禁止回归。
        assert!(!GBUFFER_SOURCE.contains("gpixel.b >> 16u")); // 标准AO与清漆字段不得再被三角编号占用。
        assert!(GBUFFER_SOURCE.contains("if surface.dynamic_terrain == 0u")); // 普通材质必须在任何TLAS缓冲读取前返回。
        let early_return = GBUFFER_SOURCE
            .find("if surface.dynamic_terrain == 0u")
            .expect("普通像素早退必须存在"); // 取得惰性分支位置。
        let vertex_load = GBUFFER_SOURCE
            .find("let vertices = load_vertices(instance_geometry, surface.primitive_index);")
            .expect("动态地形必须按三角编号读取三个顶点"); // 取得实际储存缓冲读取位置。
        assert!(early_return < vertex_load); // 普通像素不能为本修正承担三角读取成本。
        assert!(
            GBUFFER_SOURCE
                .contains("return ResolvedPrimaryRaySurface(ray_origin, geometric_normal);")
        ); // DI必须复用同次三角解析得到的真实相机侧法线。
        assert!(GBUFFER_SOURCE.contains("-min(0.0, dot(corner_delta_0, normal_0)) * normal_0")); // 首角只产生朝外修正。
        assert!(
            GBUFFER_SOURCE.contains(
                "let ray_origin = smooth_surface_origin + (geometric_normal * RAY_T_MIN);"
            )
        ); // 虚拟平滑面后仍保留数值自交小偏移。
        assert!(!GBUFFER_SOURCE.contains("fn shadow_terminator_factor(")); // 实机已否定的逐面能量因子不得再次制造棱角。
    } // 完成公共标准GBuffer与惰性几何guide算法契约测试。

    #[test] // 漫反射保持平滑；动态地形DI只以几何guide判断相机侧，并按第十轮实机结论隔离两条主表面镜面。
    fn dynamic_terrain_keeps_smooth_diffuse_and_isolates_primary_specular() {
        assert_eq!(
            RESTIR_DI_SOURCE
                .matches("primary_ray_origin(surface)")
                .count(),
            1
        ); // DI initial只读取一次起点，最终阶段改用可同时返回几何相机侧的公共结构。
        assert_eq!(
            RESTIR_DI_SOURCE
                .matches("primary_ray_surface(surface)")
                .count(),
            1
        ); // DI最终visibility与相机侧必须共享一次三角读取。
        assert_eq!(
            RESTIR_GI_SOURCE
                .matches("primary_ray_origin(surface)")
                .count(),
            2
        ); // GI initial和最终可见性各使用公共虚拟起点接口。
        assert!(!RESTIR_DI_SOURCE.contains("shadow_terminator_factor(")); // DI不能再把逐面几何法线乘入最终能量。
        assert!(!RESTIR_GI_SOURCE.contains("shadow_terminator_factor(")); // GI不能再把逐面几何法线乘入最终能量。
        assert!(
            SPECULAR_GI_SOURCE.contains("var ray_origin = primary_ray_origin(primary_surface);")
        ); // 光泽路径只修正首条射线。
        let dynamic_specular_return = // 保存动态地形镜面隔离分支的源码位置。
            SPECULAR_GI_SOURCE // 扫描实际覆盖Solari光泽root的正式源码。
                .find("if surface.dynamic_terrain != 0u") // 定位第十轮已验收的动态地形早退分支。
                .expect("已验收策略必须在主表面镜面评估前识别动态地形"); // 取得动态地形镜面隔离分支位置。
        let specular_brdf = // 保存普通材质正式镜面评估的源码位置。
            SPECULAR_GI_SOURCE // 继续扫描同一份正式光泽root源码。
                .find("let brdf = evaluate_specular_brdf(wo, wi, surface.world_normal, surface.material);") // 定位普通材质保留的官方BRDF调用。
                .expect("普通材质必须保留官方specular GI BRDF"); // 取得普通材质正式镜面评估位置。
        assert!(dynamic_specular_return < specular_brdf); // 动态地形必须在读取GI reservoir或评估Ns·V前返回。
        assert!(RESTIR_DI_SOURCE.contains("surface.world_normal, surface.material")); // DI BRDF继续读取平滑法线。
        assert!(GBUFFER_SOURCE.contains("view_side_normal: vec3<f32>")); // 惰性三角解析必须显式返回DI相机侧法线。
        assert!(RESTIR_DI_SOURCE.contains("ray_surface.view_side_normal")); // 只有DI最终漫反射消费真实几何相机侧。
        assert!(!RESTIR_DI_SOURCE.contains("di_diffuse_view_side_normal(surface)")); // 旧A槽guide捷径会破坏大小地图标准法线，禁止回归。
        assert!(!RESTIR_DI_SOURCE.contains("reflection_NdotL")); // 第九轮实机无效的几何光源侧门必须完整撤回。
        let dynamic_specular_gate = // 保存DI普通材质镜面分流条件的源码片段。
            "if surface.dynamic_terrain == 0u && surface.material.roughness >"; // 锁定动态地形必须跳过的直接镜面门。
        assert!(RESTIR_DI_SOURCE.contains(dynamic_specular_gate)); // DI必须只为普通材质保留主表面镜面。
        assert!(!GBUFFER_SOURCE.contains("fn di_diffuse_shading_normal(")); // 第八轮无效的连续视线修正必须完整撤回。
        assert!(RESTIR_GI_SOURCE.contains("sample_uniform_hemisphere(surface.world_normal, rng)")); // 第六轮失败后GI采样必须恢复平滑法线基线。
        assert!(!RESTIR_GI_SOURCE.contains("view_side_normal")); // 漫反射GI不属于已验收的几何相机侧兼容范围。
        assert!(!SPECULAR_GI_SOURCE.contains("view_side_normal")); // specular GI只按动态地形身份早退，不复用DI相机侧字段。
    } // 完成三条主射线与标准GBuffer职责分离测试。
} // 完成动态地形shadow terminator覆盖测试模块。
