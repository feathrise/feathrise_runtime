//! 通用地表WGSL资产的前向/Deferred固定句柄与注册入口。

use bevy::asset::{load_internal_asset, uuid_handle}; // 内置资产无需宿主项目维护额外Shader路径。
use bevy::prelude::{App, Handle, Shader}; // 插件构建阶段注册固定Shader资产。
use bevy::shader::ShaderDefVal; // Deferred资产私有define防止视图级DEFERRED_PREPASS误展开Forward参数。

/// 通用地形权重PBR扩展着色器的稳定内部句柄。
pub(super) const TERRAIN_SURFACE_SHADER_HANDLE: Handle<Shader> =
    // 固定UUID供MaterialExtension静态返回。
    uuid_handle!("679519ec-d870-4200-90f3-85b5d08e2a9c"); // 沿用已验证草土地表句柄，旧实现删除后不会发生冲突。

/// Deferred专用地表片元着色器句柄；只该资产声明片元三角编号扩展。
pub(super) const TERRAIN_SURFACE_DEFERRED_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("812ca1ac-88d5-4a6a-bc71-ff87c4d1587e"); // 独立UUID防止前向管线继承PRIMITIVE_INDEX设备要求。

/// WGSL扩展指令必须位于全部预处理与全局声明之前，因此只能在Deferred资产构造时置于源码首字节。
const DEFERRED_PRIMITIVE_INDEX_DIRECTIVE: &str = "enable primitive_index;\n"; // 普通地表源码保持无扩展，非光追路径不扩大设备能力边界。
const DEFERRED_PRIMITIVE_INDEX_SHADER_DEF: &str = "TERRAIN_SURFACE_PRIMITIVE_INDEX"; // 资产级身份只随Deferred句柄进入组合，不能复用视图级define。

/// 注册共用同一正式源码、但仅Deferred入口启用三角编号扩展的两个WGSL资产。
pub(super) fn register_terrain_surface_shader(app: &mut App) {
    load_internal_asset!(
        app,
        uuid_handle!("922885c8-de6f-44ab-8868-a2f4ddcaa1fe"),
        "shaders/rain_shelter.wgsl",
        |source, path| Shader::from_wgsl(
            format!("#define_import_path feathrise_environment::rain_shelter\n{source}"),
            path
        )
    ); // 纯盒相交正文也供Hanabi复用。
    load_internal_asset!(
        app,
        uuid_handle!("01cbac0b-d7ce-4bcd-8d75-c72586414d7e"),
        "shaders/surface_weather.wgsl",
        Shader::from_wgsl
    ); // 地表和河湖共用固定天气布局。
    load_internal_asset!(
        app,
        uuid_handle!("07618896-9324-4646-9d83-8873c2f4d165"),
        "shaders/rain.wgsl",
        Shader::from_wgsl
    ); // 共用固定九候选雨滴细纹，避免两套尺度和时钟。
    load_internal_asset!(
        app,
        uuid_handle!("b9e4e90b-20ec-45b6-9cb7-19b03620c7f4"),
        "shaders/wet_surface.wgsl",
        Shader::from_wgsl
    ); // 湿润与浅积水PBR纯函数。
    // 只在TerrainSurfacePlugin启用时执行一次。
    load_internal_asset!(
        // 使用Bevy官方宏把源码编译进环境crate。
        app,                            // 向当前应用Shader资产表注册源码。
        TERRAIN_SURFACE_SHADER_HANDLE,  // 材质扩展引用的稳定句柄。
        "shaders/terrain_surface.wgsl", // 相对当前模块定位通用地表Shader。
        Shader::from_wgsl               // 保留Bevy的#import、条件编译和运行时管线缓存。
    ); // 完成内置Shader注册。
    load_internal_asset!(
        app,                                    // 向同一Shader资产表注册Deferred专用入口。
        TERRAIN_SURFACE_DEFERRED_SHADER_HANDLE, // MaterialExtension只在Deferred管线返回该句柄。
        "shaders/terrain_surface.wgsl", // 权重混合、PBR输入与Forward入口仍只有一份受测源码。
        |source, path| Shader::from_wgsl_with_defs(
            format!("{DEFERRED_PRIMITIVE_INDEX_DIRECTIVE}{source}"), // 扩展指令必须成为组合前root源码的首个token。
            path,
            vec![ShaderDefVal::Bool(
                DEFERRED_PRIMITIVE_INDEX_SHADER_DEF.into(), // 只有该资产展开片元primitive_index参数与GBuffer写入。
                true,
            )],
        )
    ); // 完成Deferred专用Shader注册。
} // 完成Shader注册入口。

#[cfg(test)] // Shader条件编译与大小地图兼容GBuffer元数据由源码契约锁定。
mod tests {
    #[test] // 空作者层显示白模，存在层但未绘制的区域保留中性裸地。
    fn empty_surface_returns_white_before_any_layer_sampling() {
        let blend = TERRAIN_SURFACE_SHADER_SOURCE
            .split("fn blend_indexed_surface(")
            .nth(1)
            .unwrap(); // 限定正式索引混合入口。
        let white = blend.find("if terrain_surface.counts.x < 0.5 && source_total <= 0.000001 {").unwrap(); // 空表优先于任何数组采样。
        let neutral = blend.find("if source_total <= 0.000001 {").unwrap(); // 有层但未绘制仍显示原有中性裸地。
        let sampling = blend
            .find("sample_indexed_layer(source.indices[slot]")
            .unwrap(); // 首次实际层表与全局数组访问。
        assert!(white < neutral && neutral < sampling); // 必须先返回，不能先越界访问零层表。
        let weights = TERRAIN_SURFACE_SHADER_SOURCE
            .split("fn sample_indexed_surface_weights(")
            .nth(1)
            .unwrap()
            .split("fn decode_indexed_normal")
            .next()
            .unwrap(); // 检查正式采样入口没有默认材质回填。
        assert!(!weights.contains("output_weights[0] = 1.0")); // 零纹素与地图外区域都不得自动变成第一作者层。
        assert!(blend[white..neutral].contains("vec4<f32>(1.0)")); // 空作者层分支必须明确返回无贴图白模。
        assert!(blend[neutral..sampling].contains("vec4<f32>(0.5, 0.5, 0.5, 1.0)")); // 有层但局部未绘制保持中性灰。
    } // 完成空地表着色入口契约。
    use super::{DEFERRED_PRIMITIVE_INDEX_DIRECTIVE, DEFERRED_PRIMITIVE_INDEX_SHADER_DEF}; // 验证生产注册入口真正注入的首行扩展与资产私有define。

    // 测试只读取随crate编译的同一份正式WGSL源码。
    const TERRAIN_SURFACE_SHADER_SOURCE: &str = include_str!("shaders/terrain_surface.wgsl"); // 不复制第二份Shader文本。

    #[test] // 动态地形必须保留标准A/B槽，仅在指数安全的G槽发布三角身份。
    fn dynamic_terrain_metadata_preserves_auxiliary_deferred_pbr_fields() {
        let grass_branch_position = TERRAIN_SURFACE_SHADER_SOURCE // 从正式Shader取得草土平滑法线分支位置。
            .find("if blended.grass_weight > 0.0001") // 定位微法线进入PBR输入的唯一分支。
            .expect("地表Shader必须保留草土数据相关分支"); // 平滑网格法线仍可叠加草土微法线。
        let deferred_position = TERRAIN_SURFACE_SHADER_SOURCE // 从同一源码取得标准Deferred编码位置。
            .find("var out = deferred_output(in, pbr_input);") // 定位官方字段完整写入点。
            .expect("地表Shader必须先编码平滑Deferred输出"); // 自定义数据不得改变基础色、粗糙度、AO或清漆。
        let mode_position = TERRAIN_SURFACE_SHADER_SOURCE // 取得动态会话身份门位置。
            .find("if terrain_surface.appearance.z > 0.5") // 定位只允许光追副本进入的分支。
            .expect("Deferred会话标记必须守卫动态地形元数据"); // HN-01和作者源默认不会进入。
        let normal_position = TERRAIN_SURFACE_SHADER_SOURCE // 取得当前地表法线重打包位置。
            .find("out.deferred.a = pack_24bit_normal_and_flags(octahedral_encode(surface_normal), deferred_flags);") // A槽保存平滑或F12平直模式的最终法线。
            .expect("动态地形必须保留当前地表法线并只附加身份位"); // 大小地图与RR guide均消费该槽。
        let primitive_position = TERRAIN_SURFACE_SHADER_SOURCE // 取得指数安全三角身份写入位置。
            .find("out.deferred.g = primitive_index & DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK;") // G槽只保存低十九位编号。
            .expect("动态地形必须在G槽发布完整三角编号"); // 唯一TLAS代理按同号三角惰性解析。
        let forward_position = TERRAIN_SURFACE_SHADER_SOURCE // 取得Forward正式输出位置。
            .find("apply_pbr_lighting(pbr_input)") // Forward路径不得读取动态元数据；允许SSA输出而不绑定局部赋值写法。
            .expect("地表Shader必须保留Forward输出"); // 非Deferred视图继续使用标准PBR。
        assert!(!TERRAIN_SURFACE_SHADER_SOURCE.contains("enable primitive_index;")); // 共用正文不得让Forward管线继承Deferred设备能力。
        assert_eq!(
            DEFERRED_PRIMITIVE_INDEX_DIRECTIVE,
            "enable primitive_index;\n"
        ); // Deferred构造必须在任何#import前注入扩展指令。
        assert_eq!(
            DEFERRED_PRIMITIVE_INDEX_SHADER_DEF,
            "TERRAIN_SURFACE_PRIMITIVE_INDEX"
        ); // 不能退回会泄漏到同视图Forward变体的DEFERRED_PREPASS。
        assert!(TERRAIN_SURFACE_SHADER_SOURCE.contains("#ifdef TERRAIN_SURFACE_PRIMITIVE_INDEX")); // 参数与GBuffer元数据只由Deferred资产私有define展开。
        assert!(
            TERRAIN_SURFACE_SHADER_SOURCE
                .contains("const DEFERRED_DYNAMIC_TERRAIN_PRIMITIVE_MASK: u32 = 0x7FFFFu;")
        ); // 十九位编号必须让RGB9E5指数五位恒零。
        assert!(
            TERRAIN_SURFACE_SHADER_SOURCE
                .contains("@builtin(primitive_index) primitive_index: u32")
        ); // 编号必须来自硬件片元内建值而非世界坐标猜测。
        assert!(!TERRAIN_SURFACE_SHADER_SOURCE.contains("out.deferred.b =")); // B槽AO与清漆不得再被三角编号覆盖。
        assert!(!TERRAIN_SURFACE_SHADER_SOURCE.contains("guide_normal")); // A槽不得再发布会让俯视地图出现棱角的逐面guide。
        assert!(grass_branch_position < deferred_position); // 草土微法线必须先进入正式平滑GBuffer。
        assert!(deferred_position < mode_position); // 元数据只能在标准Deferred编码完成后写入。
        assert!(mode_position < normal_position); // 动态身份门必须包围A槽标记写入。
        assert!(normal_position < primitive_position); // 当前地表法线必须先于三角身份发布。
        assert!(primitive_position < forward_position); // 自定义写入保持在Deferred条件编译分支内。
    } // 完成大小地图标准PBR字段兼容测试。

    #[test] // F12平直预览必须只替换片元受光法线，不得要求复制或拆点大地图网格。
    fn flat_shading_uses_world_position_derivatives_before_surface_detail() {
        let mode_position = TERRAIN_SURFACE_SHADER_SOURCE // 从生产源码定位统一着色模式门。
            .find("if terrain_surface.shading.x > 0.5") // 设置仅通过既有材质uniform切换。
            .expect("地表Shader必须保留F12平直着色模式门"); // 缺少门会让正式画面永久变成平直着色。
        let derivative_position =
            TERRAIN_SURFACE_SHADER_SOURCE // 定位无需修改网格拓扑的几何面法线算法。
                .find("cross(dpdx(world_position), dpdy(world_position))") // 世界位置导数在同一光栅三角形内恒定。
                .expect("平直着色必须从片元世界位置导数取得三角形法线"); // 禁止回退为整图CPU拆点或网格重建。
        let pbr_position = TERRAIN_SURFACE_SHADER_SOURCE // 定位平直法线正式写入PBR输入的位置。
            .find("pbr_input.world_normal = face_normal") // 标准阴影与受光读取一致的几何朝向。
            .expect("平直着色必须更新标准PBR世界法线"); // 仅计算不用会造成开关无视觉效果。
        let surface_detail_position = TERRAIN_SURFACE_SHADER_SOURCE // 定位后续作者地表层采样入口。
            .find("if terrain_surface.appearance.w < 1.5") // 平直基础法线仍允许材质微法线叠加。
            .expect("地表Shader必须保留正式作者层入口"); // 着色切换不得绕开地表权重。
        assert!(mode_position < derivative_position); // 导数只能在用户开启开关时成为受光基础。
        assert!(derivative_position < pbr_position); // 必须先求面法线再写入PBR输入。
        assert!(pbr_position < surface_detail_position); // 作者微法线继续基于新的几何面法线工作。
    } // 完成F12平直着色Shader契约回归。

    #[test] // 十九位三角编号必须无损覆盖预算，并让标准RGB9E5视图只看到近零伪发光。
    fn dynamic_terrain_primitive_index_keeps_rgb9e5_exponent_zero() {
        const PRIMITIVE_MASK: u32 = 0x7FFFFu32; // 与生产WGSL十九位三角字段保持一致。
        const RGB9E5_EXPONENT_SHIFT: u32 = 27u32; // 标准RGB9E5共享指数位从位二十七开始。
        let samples = [0u32, 65_535u32, 65_536u32, 524_287u32]; // 覆盖零、低位边界、首次高位和预算末项。
        for primitive_index in samples {
            // 逐项模拟生产Shader的G槽写入与项目Solari解码。
            let packed = primitive_index & PRIMITIVE_MASK; // 只保留固定预算允许的十九位身份。
            let decoded = packed & PRIMITIVE_MASK; // 项目Solari从同一低十九位恢复身份。
            assert_eq!(decoded, primitive_index); // 每个代表值都必须无损往返。
            assert_eq!(packed >> RGB9E5_EXPONENT_SHIFT, 0u32); // 大小地图标准Deferred不得把编号解释成高指数发光。
        } // 完成全部三角预算边界检查。
    } // 完成辅助地图RGB9E5兼容身份测试。
} // 完成通用地表Shader测试模块。
