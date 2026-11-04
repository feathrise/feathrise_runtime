//! 使用Hanabi公开完整模板生成器验证正式雨丝入口，禁止测试伪造属性行号全局变量。
//! 外部View/Spawner仅使用类型边界桩；Particle、Properties、vertex/fragment及全部雨修饰器均来自真实EffectAsset模板。

use super::super::build_rain_effect;
use bevy_hanabi::{Attribute, EffectShaderSources, LayoutFlags}; // 公开生成器包含当前依赖版本的真实vfx_render.wgsl。 // 与生产雨实体完全相同的效果构造。

const EXTERNAL_RENDER_TYPES: &str = r#"
struct View { world_from_view: mat4x4<f32>, clip_from_world: mat4x4<f32>, world_from_clip: mat4x4<f32>, main_pass_viewport: vec4<f32> }
struct SimParams { times: vec4<f32> }
struct Spawner { transform: mat3x4<f32>, inverse_transform: mat3x4<f32>, effect_metadata_index: u32, render_indirect_read_index: u32 }
struct BatchInfo { base_effect: u32, base_particle: u32, prefix_sum_offset: u32, prefix_sum_count: u32 }
struct IndirectRow { particle_index: vec2<u32> }
struct IndirectBuffer { rows: array<IndirectRow> }
"#; // 仅补外部引擎数据类型；没有properties_array_index、rain_property_index或Particle定义。

fn shader_define_enabled(name: &str, flags: LayoutFlags) -> bool {
    // 按公开生成结果选择真实条件分支，不人工固定雨模式。
    let flag = match name {
        // 当前Hanabi渲染模板使用的全部宏；升级出现新宏必须显式审查。
        "LOCAL_SPACE_SIMULATION" => LayoutFlags::LOCAL_SPACE_SIMULATION, // 保留生产全局/局部空间选择。
        "USE_ALPHA_MASK" => LayoutFlags::USE_ALPHA_MASK,                 // 透明模式来自生产资产。
        "FLIPBOOK" => LayoutFlags::FLIPBOOK, // 翻页纹理由实际修饰器决定。
        "NEEDS_UV" => LayoutFlags::NEEDS_UV, // 纹理入口字段。
        "RIBBONS" => LayoutFlags::RIBBONS,   // 保留带状粒子分支资格。
        "NEEDS_NORMAL" => LayoutFlags::NEEDS_NORMAL, // 法线入口字段。
        "NEEDS_PARTICLE_FRAGMENT" => LayoutFlags::NEEDS_PARTICLE_FRAGMENT, // 粒子片元读取资格。
        other => panic!("Hanabi模板出现未审查的Shader宏：{other}"), // 不将未知分支悄悄丢弃。
    }; // 完成真实宏映射。
    flags.contains(flag) // 与Hanabi实际管线标志一致。
} // 完成宏条件选择。

fn preprocess_render_template(source: &str, flags: LayoutFlags) -> String {
    // 仅处理导入和条件语法，绝不重写vertex函数或补变量。
    let mut output = String::from(EXTERNAL_RENDER_TYPES); // 外部依赖类型与项目代码边界保持清晰。
    let mut branches = Vec::new(); // 保存嵌套宏的父资格和当前条件。
    let mut enabled = true; // 顶层代码始终保留。
    let mut multiline_import = false; // Hanabi公共类型导入分成多行。
    for line in source.lines() {
        // 完整遍历生成后的真实模板。
        let trimmed = line.trim(); // 只为识别预处理指令去除首尾空白。
        if multiline_import {
            if trimmed.contains('}') {
                multiline_import = false;
            }
            continue;
        } // 外部导入由上方类型边界提供。
        if trimmed.starts_with("#import ") {
            multiline_import = trimmed.contains('{') && !trimmed.contains('}');
            continue;
        } // 不修改其余WGSL源码。
        if let Some(name) = trimmed.strip_prefix("#ifdef ") {
            // 与生产flag一致选择条件。
            let condition = shader_define_enabled(name.split_whitespace().next().unwrap(), flags); // 宏后可有注释。
            branches.push((enabled, condition)); // 保存父分支，支持嵌套FLIPBOOK。
            enabled &= condition; // 只有父子均启用才保留。
            continue; // 指令不送入Naga。
        } // 完成条件开始。
        if trimmed.starts_with("#else") {
            // 只切换当前分支。
            let (parent, condition) = branches.last_mut().expect("模板#else必须有对应#ifdef"); // 损坏模板明确失败。
            *condition = !*condition; // 进入互补分支。
            enabled = *parent && *condition; // 保留外层禁用状态。
            continue; // 指令不送入Naga。
        } // 完成else。
        if trimmed.starts_with("#endif") {
            enabled = branches.pop().expect("模板#endif必须有对应#ifdef").0;
            continue;
        } // 恢复上一层资格。
        assert!(
            !trimmed.starts_with('#'),
            "未处理的Hanabi模板指令：{trimmed}"
        ); // 不忽略引擎升级后的新语法。
        if enabled {
            output.push_str(line);
            output.push('\n');
        } // 原样保留真实入口、声明、修饰器与模板注释。
    } // 完成模板预处理。
    assert!(
        branches.is_empty() && !multiline_import,
        "Hanabi模板预处理结构未闭合"
    ); // 不能截断模板使错误消失。
    output // 返回完整vertex与fragment源码。
} // 完成最小引擎边界适配。

fn forbidden_fragment_bindings(
    module: &wgpu::naga::Module,
    info: &wgpu::naga::valid::ModuleInfo,
) -> Vec<(u32, u32)> {
    // WGSL合法不代表符合Hanabi实际管线的阶段可见性。
    let mut forbidden = Vec::new(); // 保存实际被片元入口使用但未向片元开放的资源。
    for (index, entry) in module.entry_points.iter().enumerate() {
        // Naga入口分析包含调用函数的全局资源使用。
        if entry.stage != wgpu::naga::ShaderStage::Fragment {
            continue;
        } // 属性仍允许在顶点阶段读取。
        let uses = info.get_entry_point(index); // 读取正式入口的GlobalUse而非字符串猜测。
        for (handle, global) in module.global_variables.iter() {
            // 检查所有真实生成绑定。
            if uses[handle].is_empty() {
                continue;
            } // 声明存在但片元未读取不构成阶段越界。
            let Some(binding) = &global.binding else {
                continue;
            }; // 普通私有全局没有绑定权限。
            let allowed = matches!(
                (binding.group, binding.binding),
                (0, 0 | 1) | (1, 0) | (3, _)
            ); // Hanabi视图/模拟参数/Spawner及片元材质；group2粒子与属性、group1间接表仅供顶点。
            if !allowed {
                forbidden.push((binding.group, binding.binding));
            } // 明确暴露group2 binding3的原运行时错误。
        } // 完成当前片元入口绑定审计。
    } // 完成所有片元入口检查。
    forbidden // 空结果才满足真实阶段可见性契约。
} // 完成引擎绑定权限回归辅助函数。

#[test] // 真正模板不提供properties_array_index，必须由粒子携带的实际身份恢复。
fn actual_hanabi_render_template_compiles_without_invented_property_index() {
    // 不运行游戏或初始化GPU。
    let effect = build_rain_effect(); // 所有Init/Update/Render修饰器都来自正式资产。
    assert!(effect.particle_layout().contains(Attribute::F32_0)); // 同时证明通用32位槽进入真实粒子缓冲布局。
    let sources = EffectShaderSources::generate(&effect, None, 0)
        .expect("正式雨资产必须能生成三个真实Hanabi模板"); // 不通过自造main或RenderContext替身绕过模板。
    assert!(sources.layout_flags.contains(LayoutFlags::NEEDS_UV)); // 水面窗口经正式UV插值通道由顶点传给片元。
    assert!(
        !sources
            .layout_flags
            .contains(LayoutFlags::NEEDS_PARTICLE_FRAGMENT)
    ); // Hanabi粒子缓冲不向片元开放，禁止重新引入片元粒子读取。
    assert_eq!(effect.texture_layout().layout.len(), 2); // 水面与屋顶独立纹理槽，不能改变槽0既有语义。
    for (name, source) in [
        ("init", &sources.init_shader_source),
        ("update", &sources.update_shader_source),
    ] {
        // 新生与已存活粒子都必须发布真实属性身份。
        assert!(
            source.contains("particle.f32_0 = bitcast<f32>(properties_array_index);"),
            "{name}模板必须安装生产身份写入修饰器"
        ); // 缺少任一阶段会造成暂停首帧或重分配后错误。
        assert!(
            source.contains("properties_array_index ="),
            "{name}模板必须提供实际属性行号"
        ); // 不能回退固定索引0。
    } // 完成两个模拟入口接线验证。
    assert!(!EXTERNAL_RENDER_TYPES.contains("properties_array_index")); // 边界桩不能偷偷提供原本缺失的全局。
    let source = preprocess_render_template(&sources.render_shader_source, sources.layout_flags); // 完整真实vfx_render入口与生产修饰器保持不变。
    assert!(source.contains("fn find_location_from_particle(")); // 证明保留实际批次/粒子定位模板。
    assert!(source.contains("@vertex") && source.contains("fn vertex(")); // 没有换成测试自造main。
    assert!(
        source.contains("material_texture_0")
            && source.contains("world_from_clip")
            && source.contains("main_pass_viewport")
    ); // 真正片元段必须绑定水面纹理并按当前视口重建世界坐标。
    assert!(source.contains("rain_water_bounds") && source.contains("rain_water_state")); // 同一粒子属性行同时携带局部窗口与启用状态。
    assert!(source.contains("material_texture_1") && source.contains("rain_occlusion_decode")); // 自动屋顶消费者进入真实完整Hanabi模板。
    let parsed = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source))); // 未定义行号将直接被真实入口解析拒绝。
    let info = wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&parsed)
    .unwrap(); // WGSL语义验证后还需检查运行时绑定阶段。
    assert!(
        forbidden_fragment_bindings(&parsed, &info).is_empty(),
        "片元读取了Hanabi仅向顶点开放的绑定"
    ); // 独立覆盖Naga语法验证无法发现的管线布局错误。
    assert!(
        parsed
            .entry_points
            .iter()
            .any(|entry| entry.name == "vertex" && entry.stage == wgpu::naga::ShaderStage::Vertex)
    ); // 验证真正Hanabi顶点入口被编译。
    assert!(
        parsed
            .entry_points
            .iter()
            .any(|entry| entry.name == "fragment"
                && entry.stage == wgpu::naga::ShaderStage::Fragment)
    ); // 同时保留实际片元入口。
    assert!(
        !parsed
            .global_variables
            .iter()
            .any(|(_, variable)| variable.name.as_deref() == Some("properties_array_index"))
    ); // 渲染不能依赖不存在的模拟全局。
    let particle = parsed
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("Particle"))
        .unwrap()
        .1; // 检查真正模板生成的粒子结构。
    let wgpu::naga::TypeInner::Struct { members, .. } = &particle.inner else {
        panic!("真实粒子布局必须生成结构");
    }; // 防止类型同名替代。
    assert!(
        members
            .iter()
            .any(|member| member.name.as_deref() == Some("f32_0"))
    ); // 32位身份字段真正进入Shader读取结构。
    let declaration = "let rain_property_index = bitcast<u32>(particle.f32_0);"; // 正式跨阶段身份恢复声明。
    assert_eq!(source.matches(declaration).count(), 1); // 不允许其他默认声明掩盖缺陷。
    let broken = source.replacen(declaration, "", 1); // 故意删除唯一身份恢复，模拟原用户崩溃。
    let error = wgpu::naga::front::wgsl::parse_str(&broken)
        .expect_err("删除身份恢复后完整模板必须因未定义索引失败"); // 反证本测试没有用自造全局掩盖问题。
    assert!(
        error
            .emit_to_string(&broken)
            .contains("rain_property_index")
    ); // 错误必须指向真正缺失的属性索引。
    let fragment_start = source.find("fn fragment(").expect("必须保留真实片元入口"); // 只在正式片元函数中注入负向读取。
    let body_start = fragment_start
        + source[fragment_start..]
            .find('{')
            .expect("片元函数体必须存在")
        + 1; // 不依赖模板函数参数换行格式。
    let mut broken_binding = source.clone(); // 保留完整真实模板，单独模拟旧版错误。
    broken_binding.insert_str(
        body_start,
        "\nif properties[0u].rain_water_state.x > 0.5 { discard; }\n",
    ); // 固定索引在WGSL语义上合法，但片元读取属性违反Hanabi绑定权限。
    let broken_module =
        wgpu::naga::front::wgsl::parse_str(&broken_binding).expect("权限负向样本应保持语法合法"); // 避免用语法错误掩盖实际阶段权限检查。
    let broken_info = wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&broken_module)
    .expect("Naga语义验证本身无法知道Hanabi绑定阶段"); // 明确证明原来的验证盲区。
    assert!(
        forbidden_fragment_bindings(&broken_module, &broken_info).contains(&(2, 3)),
        "必须拒绝片元读取仅VERTEX可见的properties绑定"
    ); // 负向控制必须命中用户遇到的实际group2 binding3。
} // 完成完整引擎模板回归。
