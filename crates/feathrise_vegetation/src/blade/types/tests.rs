use super::*; // 读取内部紧凑实例镜像。

const DRAW_SHADER_SOURCE: &str = include_str!("../shaders/gpu_grass_minimal.wesl"); // 测试下沉一级后仍读取blade共享绘制源码，锁定随机种子不能退回草根浮点位模式。
const COMPUTE_SHADER_SOURCE: &str = include_str!("../shaders/gpu_grass_compute.wgsl"); // 测试下沉一级后仍读取blade共享Compute源码和粗瓦片安全边界。

#[test] // 公开profile槽构造器必须在CPU侧阻止任何超出固定两位ABI的编号。
fn profile_slot_rejects_out_of_bounds_indices() {
    assert_eq!(
        (0..GPU_GRASS_PROFILE_CAPACITY as u32)
            .map(GpuBladeProfileSlot::new)
            .collect::<Vec<_>>(),
        vec![
            Some(GpuBladeProfileSlot::SLOT_0),
            Some(GpuBladeProfileSlot::SLOT_1),
            Some(GpuBladeProfileSlot::SLOT_2),
            Some(GpuBladeProfileSlot::SLOT_3),
        ]
    ); // 四个公开槽必须与稳定整数索引一一对应。
    assert_eq!(GpuBladeProfileSlot::new(4), None); // 第一个越界值不得静默截断到两位。
    assert_eq!(GpuBladeProfileSlot::new(u32::MAX), None); // 极端外部输入同样必须明确拒绝。
    assert_eq!(
        GpuBladeProfileSlot::from_gpu_index(u32::MAX),
        GpuBladeProfileSlot::SLOT_0
    ); // 内部损坏位段使用确定零号回退，禁止数组越界。
}

#[test] // 顶点缓冲步长必须与WGSL存储数组完全一致。
fn compact_instance_includes_four_byte_volume_normal() {
    assert_eq!(std::mem::size_of::<GpuBladeCompactInstance>(), 20); // 三个f32与一个u32不得产生额外尾部填充。
    assert_eq!(std::mem::align_of::<GpuBladeCompactInstance>(), 4); // 成员只要求四字节CPU对齐，GPU步长由显式二十字节声明控制。
}

#[test] // Compute没有Bevy预处理指令，必须直接通过当前wgpu同版本Naga的WGSL解析与语义验证。
fn compute_shader_is_valid_wgsl() {
    let module = wgpu::naga::front::wgsl::parse_str(&crate::shaders::compute_shader_source())
        .unwrap_or_else(|error| panic!("GPU草地Compute WGSL语法错误：{error}")); // 解析失败时保留Naga具体行列信息。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(), // 覆盖类型、控制流、绑定和表达式合法性。
        wgpu::naga::valid::Capabilities::all(), // 验证完整WebGPU能力集合而不误拒绝后端支持的标准语法。
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("GPU草地Compute WGSL语义错误：{error}")); // 防止只通过文本断言却在运行时创建管线失败。
    for (name, expected_size, metadata_offset) in
        [("GrassPatch", 416, 272), ("GrassViewCulling", 304, 288)]
    {
        let ty = module
            .types
            .iter()
            .map(|(_, ty)| ty)
            .find(|ty| ty.name.as_deref() == Some(name))
            .expect("生产 uniform 类型必须存在");
        let wgpu::naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("uniform 必须是结构体")
        };
        assert_eq!(
            *span, expected_size,
            "{name} 的 WGSL 大小必须与 Rust ShaderType 相同"
        );
        assert_eq!(
            members.iter().find(|member| member.name.as_deref().is_some_and(|name| name.ends_with("lod_metadata")))
                .expect("档数元数据字段必须存在").offset,
            metadata_offset,
            "{name} 的档数元数据必须对齐"
        );
    }
}

#[test] // 绘制文件包含Bevy预处理指令，抽取新增profile辅助函数及其真实结构声明做独立Naga解析验证。
fn draw_profile_proxy_helper_is_valid_wgsl() {
    let profile_start = DRAW_SHADER_SOURCE // 从生产绘制shader读取真实profile结构，避免测试维护另一份字段镜像。
        .find("struct BladeVegetationProfile {")
        .expect("绘制shader必须声明BladeVegetationProfile"); // 类型声明被重命名时要求同步更新辅助函数契约。
    let profile_tail_marker = "} // 完成单种叶片植被固定内容参数。"; // 使用结构末尾设计注释精确隔离单个WGSL声明。
    let profile_end = DRAW_SHADER_SOURCE[profile_start..] // 在结构起点之后查找结束标记，避免命中其他右花括号。
        .find(profile_tail_marker)
        .map(|offset| profile_start + offset + profile_tail_marker.len())
        .expect("绘制shader必须保留完整BladeVegetationProfile声明"); // 结构截取失败时提供明确回归信息。
    let helper_start = DRAW_SHADER_SOURCE // 从生产绘制shader直接抽取待验证函数，确保拼错类型会进入Naga。
        .find("fn profile_proxy_horizontal_scale(")
        .expect("绘制shader必须声明四级水平覆盖辅助函数"); // 辅助函数意外删除时不得让测试静默通过。
    let helper_tail_marker = "} // 结束四级水平覆盖换算。"; // 使用函数唯一末尾注释隔离完整函数体。
    let helper_end = DRAW_SHADER_SOURCE[helper_start..] // 从函数起点搜索其唯一结束标记。
        .find(helper_tail_marker)
        .map(|offset| helper_start + offset + helper_tail_marker.len())
        .expect("绘制shader必须保留完整四级水平覆盖辅助函数"); // 截断函数会在这里给出可读错误。
    let validation_source = format!(
        // 补齐该函数依赖的两个生产常量后构造不含Bevy预处理指令的标准WGSL。
        "{}\nconst GEOMETRY_LOD_SHIFT: u32 = 29u;\nconst GEOMETRY_LOD_MASK: u32 = 0x7u;\n{}\n",
        &DRAW_SHADER_SOURCE[profile_start..profile_end], // 复用生产profile声明，让函数参数和字段访问接受真实类型检查。
        &DRAW_SHADER_SOURCE[helper_start..helper_end], // 复用生产辅助函数，让类型名拼写错误必然触发解析失败。
    ); // 完成最小可独立验证的绘制WGSL片段。
    let module = wgpu::naga::front::wgsl::parse_str(&validation_source)
        .unwrap_or_else(|error| panic!("GPU草地绘制profile辅助函数WGSL语法错误：{error}")); // 未声明类型等错误在单元测试阶段直接暴露。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(), // 覆盖结构字段、位运算、内建函数和返回类型合法性。
        wgpu::naga::valid::Capabilities::all(), // 使用与Compute测试一致的完整WebGPU标准能力集合。
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("GPU草地绘制profile辅助函数WGSL语义错误：{error}")); // 防止错误延迟到游戏运行时管线专用化阶段。
}

#[test] // 同一世界草格跨越窗口吸附边界后仍必须保持颜色、宽度、倾斜和风相位不变。
fn draw_randomness_uses_stable_packed_appearance_bits() {
    assert!(
        DRAW_SHADER_SOURCE.contains("packed_appearance & STABLE_APPEARANCE_MASK"), // 低二十四位不含材质LOD并由稳定世界草格生成。
        "绘制shader必须从紧凑实例稳定低二十四位派生外观随机种子",
    ); // 完成稳定实例位段存在性检查。
    assert!(
        !DRAW_SHADER_SOURCE.contains("bitcast<u32>(vertex.root_world"), // 浮点加法顺序会在22.4米窗口重定位时改变末位，禁止作为持久随机种子。
        "绘制shader不得重新使用草根f32位模式生成外观随机种子",
    ); // 完成旧不稳定随机路径回归保护。
}

#[test] // DLSS RR等时间重建必须看到程序风摆与玩家压草的真实逐帧位移，不能只报告相机运动。
fn draw_motion_vectors_reconstruct_previous_dynamic_blade_pose() {
    assert!(
        DRAW_SHADER_SOURCE.contains(
            "@group(3) @binding(4) var<uniform> previous_grass_interaction: GrassInteraction;",
        ),
        "绘制shader必须绑定上一完整渲染帧的风场与足迹",
    ); // 锁定RenderApp历史快照确实进入预通道可见的GPU布局。
    assert!(
        DRAW_SHADER_SOURCE.contains("prepass_globals.time - prepass_globals.delta_time")
            && DRAW_SHADER_SOURCE.contains("previous_wind_offset")
            && DRAW_SHADER_SOURCE.contains("previous_interaction_offset")
            && DRAW_SHADER_SOURCE.contains(
                "output.previous_world_position = vec4<f32>(grass_surface_position(vertex.root_world, previous_world_position, surface_basis), 1.0);",
            ),
        "运动矢量必须以上一帧时间、风参数和局部交互重建旧草叶世界位置",
    ); // 静止相机下仍持续运动的密集叶片不能再向RR提交零速度历史。
    assert!(
        !DRAW_SHADER_SOURCE.contains("output.previous_world_position = output.world_position"),
        "不得恢复把动态草叶上一帧位置直接等同当前帧位置的旧实现",
    ); // 防止白色历史能量在相机附近高像素速度叶片上重新累计。
}

#[test] // Deferred首表面与Forward最终颜色属于同一草叶时，两个着色器变体必须生成逐位一致的裁剪位置。
fn draw_vertex_position_is_invariant_across_deferred_and_forward_variants() {
    const INVARIANT_POSITION_DECLARATION: &str =
        // 单一字面量同时锁定WGSL属性顺序和共享输出字段名称。
        "@builtin(position) @invariant clip_position: vec4<f32>"; // Vulkan后端据此为位置输出保留跨管线不变量约束。
    assert_eq!(
        DRAW_SHADER_SOURCE
            .matches(INVARIANT_POSITION_DECLARATION) // 只检查生产shader的共享GrassVertexOutput，阴影独立输出不属于本契约。
            .count(), // 统计完整声明的唯一出现次数，防止只给某个宏分支补标记。
        1, // 主颜色和Deferred预通道必须共用同一个带不变量的位置字段。
        "GPU草叶Deferred输入与Forward覆盖必须共享唯一的@invariant裁剪位置输出",
    ); // 时间抖动不能再放大两个管线变体的浮点优化差异并造成孤立黑叶像素。
    let validation_source = format!(
        // 使用当前项目锁定的Naga验证真实属性语法及其顶点输出位置限制。
        "struct InvariantVertexOutput {{\n    {},\n}}\n@vertex\nfn invariant_vertex() -> InvariantVertexOutput {{\n    var output: InvariantVertexOutput;\n    output.clip_position = vec4<f32>(0.0, 0.0, 0.0, 1.0);\n    return output;\n}}\n",
        INVARIANT_POSITION_DECLARATION, // 验证的字段声明直接复用上方生产源码契约字面量。
    ); // 构造不依赖Bevy预处理器的最小合法顶点着色器。
    let module = wgpu::naga::front::wgsl::parse_str(&validation_source)
        .unwrap_or_else(|error| panic!("GPU草地裁剪位置不变量WGSL语法错误：{error}")); // 属性顺序或语法不受当前Naga支持时立即给出明确错误。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(), // 覆盖顶点阶段输出接口与不变量属性的语义限制。
        wgpu::naga::valid::Capabilities::all(), // 使用项目当前WebGPU完整能力集合，不借助后端私有扩展。
    )
    .validate(&module)
    .unwrap_or_else(|error| panic!("GPU草地裁剪位置不变量WGSL语义错误：{error}")); // 防止文本存在但无法进入真实着色器模块。
}

#[test] // RR材质引导必须描述最终可见草叶，不能继续从同像素地下地形GBuffer解析。
fn draw_deferred_surface_matches_visible_grass_material() {
    assert!(
        DRAW_SHADER_SOURCE.contains("GPU_GRASS_DEFERRED_SURFACE")
            && DRAW_SHADER_SOURCE.contains("fn vegetation_surface_color(")
            && DRAW_SHADER_SOURCE.contains("fn vegetation_deferred_gbuffer("),
        "Deferred草叶必须启用独立表面变体并与主颜色共享综合色函数",
    ); // 锁定主颜色与GBuffer基础色只有一个生产来源，避免后续调色重新产生RR引导偏差。
    assert!(
        DRAW_SHADER_SOURCE.contains("output.deferred = vegetation_deferred_gbuffer(")
            && DRAW_SHADER_SOURCE.contains("output.deferred_lighting_pass_id = 1u;"),
        "Deferred草叶必须实际覆盖Rgba32Uint GBuffer与lighting pass id",
    ); // 只声明四附件格式但使用空写掩码会把地下地形材质继续交给RR，必须永久阻止。
    assert!(
        DRAW_SHADER_SOURCE.contains("const DEFERRED_FLAGS_GPU_VEGETATION_BIT: u32 = 1u << 7u;")
            && DRAW_SHADER_SOURCE.contains("4u | DEFERRED_FLAGS_GPU_VEGETATION_BIT"),
        "Deferred草叶必须同时保留shadow receiver并用bit7标记GPU植被类别",
    ); // Solari GI依赖该私有高位阻止草叶与地下地面跨帧、跨空间错误复用；升级Bevy时必须重新审计官方flags。
    assert!(
        DRAW_SHADER_SOURCE.contains("vec4<f32>(0.0, 0.0, 1.0, 0.0)")
            && DRAW_SHADER_SOURCE.contains("vec4<f32>(base_color_srgb, 1.0)"),
        "自定义纯漫反射草光照必须提交零镜面、非金属和完全粗糙的匹配材质引导",
    ); // RR不得用不存在于最终草叶颜色中的镜面分量反向放大亮度历史。
    assert!(
        DRAW_SHADER_SOURCE.contains("@builtin(front_facing) is_front: bool")
            && DRAW_SHADER_SOURCE.contains("let visible_world_normal = select(")
            && DRAW_SHADER_SOURCE.contains("-normalized_world_normal")
            && DRAW_SHADER_SOURCE.contains("is_front,"),
        "双面草叶的Deferred法线必须按真实正反面翻转到可见半球",
    ); // 关闭背面剔除时不能把同一资产法线直接用于两面，否则Solari与RR会收到错误半球法线。
}

#[test] // TAA与SSAO组合时必须让草叶NormalPrepass和Deferred继续共用同一动态可见面法线。
fn draw_normal_prepass_writes_bevy_encoded_visible_grass_normal() {
    // Windows 检出可能把 WGSL 统一为 CRLF；契约检查只关心预处理器相邻关系，先归一化换行避免把工作树格式误判为 Shader 回归。
    let normalized_draw_shader = DRAW_SHADER_SOURCE.replace("\r\n", "\n");
    assert!(
        normalized_draw_shader.contains("@if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)")
            && normalized_draw_shader.contains(
                "output.world_normal = surface_basis * reconstructed_grass_world_normal("
            ),
        "Normal或Deferred预通道必须从真实风摆、足迹和实例姿态重建草叶世界法线",
    ); // 禁止只修pipeline附件格式却继续把地下地形法线留在草叶深度像素。
    assert!(
        normalized_draw_shader.contains("@if(NORMAL_PREPASS)")
            && normalized_draw_shader.contains("@location(0) normal: vec4<f32>")
            && normalized_draw_shader.contains(
                "output.normal = vec4<f32>(visible_world_normal * 0.5 + vec3<f32>(0.5), 1.0);",
            ),
        "草叶NormalPrepass必须按Bevy 0.19 ABI写location0的[-1,1]到[0,1]世界法线编码",
    ); // 锁定Rgb10a2Unorm附件不仅被声明，而且确实由草叶片元覆盖。
    assert!(
        normalized_draw_shader.contains("let visible_world_normal = select(")
            && normalized_draw_shader.contains("-normalized_world_normal")
            && normalized_draw_shader.contains("is_front,"),
        "NormalPrepass与Deferred必须继续按front_facing共用双面草叶可见法线",
    ); // SSAO法线、Solari GBuffer与RR normal guide不得在背面采用不同半球。
    let motion_import = normalized_draw_shader
        .find("@if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) import bevy_pbr::render::pbr_prepass_functions::calculate_motion_vector")
        .expect("运动函数导入必须受Motion宏保护");
    let motion_output = normalized_draw_shader
        .find("@if(MOTION_VECTOR_PREPASS) @location(1) motion_vector: vec2<f32>")
        .expect("运动附件输出必须受Motion宏保护");
    let motion_write = normalized_draw_shader
        .find("@if(MOTION_VECTOR_PREPASS) output.motion_vector = calculate_motion_vector(")
        .expect("运动矢量计算必须受Motion宏保护");
    assert!(motion_import < motion_output && motion_output < motion_write); // Normal-only变体不能要求PreviousView绑定或不存在的location1附件。
}

#[test] // 粗剔除瓦片必须覆盖一格邻格重映射与百分之九十八格内抖动，禁止在LOD边缘误删草簇。
fn tile_culling_covers_maximum_clustered_root_displacement() {
    const ROOT_JITTER_SPAN: f32 = 0.98; // 与Compute着色器的稳定格内抖动跨度保持一致。
    const NEIGHBOR_REMAP_DISPLACEMENT: f32 = 1.0; // 四个对角邻格重映射会让每个轴最多偏离源格一格。
    const MAXIMUM_JITTER_DISPLACEMENT: f32 = ROOT_JITTER_SPAN * 0.5; // 零到一随机值减半后让每个轴最多再偏移跨度的一半。
    const REQUIRED_GRID_MARGIN: f32 = // 粗瓦片单轴最小安全余量由两个独立位移相加得到。
        NEIGHBOR_REMAP_DISPLACEMENT + MAXIMUM_JITTER_DISPLACEMENT; // 一格重映射加零点四九格抖动等于一点四九格。

    assert!(
        (REQUIRED_GRID_MARGIN - 1.49).abs() <= f32::EPSILON, // 数学契约明确锁定当前一点四九格最大单轴位移。
        "粗瓦片安全余量必须覆盖邻格重映射与完整格内抖动",
    ); // 完成最大位移数学契约检查。
    assert!(
        COMPUTE_SHADER_SOURCE
            .contains("const MAX_ROOT_GRID_DISPLACEMENT: f32 = 1.0 + ROOT_JITTER_SPAN * 0.5;",), // 命名常量必须直接从抖动跨度推导，禁止重新写入容易过期的裸数值。
        "Compute shader必须由ROOT_JITTER_SPAN推导草根最大格位移",
    ); // 完成着色器常量来源契约检查。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("+ vec2<f32>(MAX_ROOT_GRID_DISPLACEMENT);",), // 粗瓦片两个轴必须实际消费上述最大位移，而不只声明未使用常量。
        "粗瓦片两个轴必须使用最大草根格位移余量",
    ); // 完成粗剔除调用点契约检查。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("max(half_extent_grid.x, half_extent_grid.y) * spacing;"), // 右侧不足十六格的边缘瓦片仍可能在另一轴覆盖完整十六格，必须取较长轴。
        "粗瓦片标量半边长必须覆盖边缘瓦片两个轴的较大跨度",
    ); // 完成非正方形边缘瓦片保守范围契约检查。
    assert!(
        !COMPUTE_SHADER_SOURCE.contains("+ 0.325) * spacing"), // 旧百分之三十二点五格余量无法覆盖草簇重映射，必须永久移除。
        "Compute shader不得恢复不足的旧粗剔除余量",
    ); // 完成旧错误裸数值回归保护。
}

#[test] // LOD过渡必须在Compute选择完整父簇，绘制阶段不得再次恢复片元打孔或三级明暗倍率。
fn lod_transition_uses_complete_hierarchical_clusters() {
    assert!(
        COMPUTE_SHADER_SOURCE.contains("fn screen_space_lod_selects(")
            && COMPUTE_SHADER_SOURCE.contains("floor_parent_cell(fine_cell, side)")
            && COMPUTE_SHADER_SOURCE.contains("1u << min(lod_level, 7u)")
            && COMPUTE_SHADER_SOURCE.contains("ultra_budget_selects(fine_cell)"),
        "Compute shader必须按实际档数的二倍世界父格与最粗档固定预算选择唯一LOD",
    ); // 八档使用同一层级入口，前三次交接仍保留旧世界格比例。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("return !selects_child &&")
            && COMPUTE_SHADER_SOURCE.contains("if (!selects_child) { return false; }"),
        "每个父簇的祖先保留与子树进入必须保持严格互补",
    ); // 防止未来重新引入两档同时消失或完整重叠。
    assert!(
        COMPUTE_SHADER_SOURCE.contains(
            "density_sample_world = parent_center_world(occupancy_parent, occupancy_side, fine_spacing)"
        ) && COMPUTE_SHADER_SOURCE.contains(
            "let selected_count = density_selected_count(density_probability, 16u, parent_seed)"
        ) && COMPUTE_SHADER_SOURCE.contains(
            "let child_rank = density_child_rank(local_child, parent_seed)"
        ) && COMPUTE_SHADER_SOURCE
            .contains("root_world = canonical_lod_root(fine_cell, lod_level, fine_spacing)",),
        "三级代理必须共享远父格密度，把概率转换为父格内稳定均匀的前N个子格，并把每级根限制在自身层级父格内",
    ); // 防止重新引入整簇一次抽签、各窗口独立密度或跨父格跳根。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("density_selection.proxy_coverage")
            && COMPUTE_SHADER_SOURCE.contains("packed_coverage << PROXY_COVERAGE_SHIFT")
            && DRAW_SHADER_SOURCE.contains("* proxy_coverage"),
        "中远宽代理必须携带实际最细叶片覆盖份额，维持近中远单位面积总叶宽守恒",
    ); // 防止密度减少后中远代理仍按满父格宽度绘制。
    assert!(
        DRAW_SHADER_SOURCE.contains("fn reused_crop_ultra_horizontal_scale(")
            && DRAW_SHADER_SOURCE.contains("const REUSED_CROP_ULTRA_HORIZONTAL_SCALE: f32 = 4.0;")
            && DRAW_SHADER_SOURCE.contains(
                "* reused_crop_ultra_horizontal_scale(profile, vertex.packed_appearance)",
            )
            && DRAW_SHADER_SOURCE.contains("profile.warm_tint.w >= 0.5"),
        "复用远景麦簇的作物超远档必须补偿八乘八父格的四倍水平覆盖",
    ); // 麦田和杂粮LOD3不得把远景资产直接稀释到六十四格代理，复用三号槽的沙洲草也不得误命中补偿。
    assert!(
        DRAW_SHADER_SOURCE.contains("fn profile_proxy_horizontal_scale(") // 需要覆盖守恒的内容使用profile开关共享四级水平补偿。
            && DRAW_SHADER_SOURCE.contains("profile.cool_tint.w >= 0.5") // 只有地图显式打开标志的profile才扩大粗级代理。
            && DRAW_SHADER_SOURCE.contains("1u << min(geometry_lod, 7u)") // 水平跨度跟随全部八档的二倍格距。
            && DRAW_SHADER_SOURCE
                .contains("* profile_proxy_horizontal_scale(profile, vertex.packed_appearance);"), // 绘制顶点必须真实消费当前profile覆盖比例。
        "启用覆盖标志的profile必须按LOD格距扩展XZ代理，并保持未启用profile不变",
    ); // 通用运行时只锁定profile开关语义，具体内容包围由消费者适配层验收。
    assert!(
        !DRAW_SHADER_SOURCE.contains("stable_lod_dither")
            && !DRAW_SHADER_SOURCE.contains("lod_coverage")
            && !DRAW_SHADER_SOURCE.contains("lod_height_scale")
            && !DRAW_SHADER_SOURCE.contains("lod_color_factor"),
        "绘制shader不得切碎草叶或按LOD改变基础高度和颜色",
    ); // 俯视或近裁面下每个进入Draw的草叶都必须保持完整。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("closest_axis_distance - max_horizontal_sweep")
            && COMPUTE_SHADER_SOURCE.contains("max(grass_patch.vegetation_bounds.y, 0.01)"),
        "近镜头整株避让必须读取当前普通草或麦田的最大风摆与原型宽度",
    ); // 禁止继续硬编码普通草扫掠半径，让高秆或宽代理穿进镜头后被近裁面截成残片。
}

#[test] // 4K与窄FOV可以请求窗外细级，但GPU必须保留仍驻留的祖先代理而不是制造空带。
fn screen_space_lod_falls_back_when_requested_child_is_not_resident() {
    let viewport_height = 2160.0_f32; // 使用用户4090常见的4K物理视口高度。
    let vertical_fov = std::f32::consts::FRAC_PI_4; // 使用常见四十五度垂直视场验证投影阈值。
    let pixel_scale = viewport_height * 0.5 / (vertical_fov * 0.5).tan(); // 与culling.rs上传公式严格一致。
    let far_to_middle_start_depth = 0.70 * pixel_scale / 12.0; // 十二像素开始从远代理请求更细子树。
    let middle_safe_radius = 104.0 - 2.0; // 中窗在两米外沿安全边距之前才允许接管。
    assert!(far_to_middle_start_depth > middle_safe_radius); // 证明固定阈值在4K下确实会请求超出中窗的细级。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("let residency = lod_child_residency_weight(center,")
            && COMPUTE_SHADER_SOURCE.contains("content_lod_residency_radius(child_level)")
            && COMPUTE_SHADER_SOURCE.contains("projected_reference_pixels(center)) * residency;")
            && COMPUTE_SHADER_SOURCE.contains("lod_transition_salt(parent_level)) < weight;"),
        "近中子级接近常驻边界时，屏幕LOD必须按固定父簇稳定交还仍驻留的祖先",
    ); // 锁定高分辨率与窄FOV下既不会出现真实空草带，也不会重新形成布尔硬环。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("lod_selected = lod_level + 1u == content_lod_count()"),
        "相机数据暂不可用时必须只保留远祖先，禁止三个零内圈窗口重叠",
    ); // 覆盖窗口最小化、首帧或多玩法视图配置错误的保守路径。
    assert!(
        COMPUTE_SHADER_SOURCE
            .contains("(!content_uses_ultra_budget() || ultra_budget_selects(fine_cell))")
            && COMPUTE_SHADER_SOURCE.contains("lod_ultra_pixel_thresholds"),
        "带第四级标志的窗口在视图失效时必须回退固定预算LOD3，并消费独立超远像素阈值",
    ); // 三级兼容回退和四级隔离原型回退必须根据显式标志分流。
}

#[test] // 八个紧凑绘制桶按玩家等面积环预留固定容量，超远相机不得改变这个预算口径。
fn depth_buckets_keep_their_equal_area_capacity_contract() {
    assert!(
        COMPUTE_SHADER_SOURCE.contains("let bucket_distance_squared = root_distance_squared;")
            && COMPUTE_SHADER_SOURCE.contains("let bucket_inner_radius = inner_radius;"),
        "绘制桶必须继续使用窗口自己的玩家径向面积口径",
    ); // 均匀草场下八桶负载才会与Rust为每桶预留的八分之一容量匹配。
    assert!(
        !COMPUTE_SHADER_SOURCE
            .contains("bucket_distance_squared = dot(camera_delta, camera_delta)"),
        "不得按超距相机距离把全部候选压进同一个固定容量桶",
    ); // 防止桶溢出后的竞争顺序把静止草麦随机丢成大块频闪。
}

#[test] // 穷举正负世界父格，证明远父簇或每个中父簇始终有且仅有一套完整代理。
fn hierarchical_proxy_reference_has_no_overlap_or_hole() {
    let floor_parent = |cell: i32, side: i32| cell.div_euclid(side); // 与WGSL对负坐标执行floor除法的结果一致。
    for far_parent_x in -3..=3 {
        for far_parent_z in -3..=3 {
            for middle_resident in [false, true] {
                for far_selects_fine in [false, true] {
                    let fine_subtree = middle_resident && far_selects_fine; // 未驻留中级强制保留远祖先。
                    let far_instances = usize::from(!fine_subtree); // 每个四乘四父格至多一个远簇。
                    let mut middle_instances = 0usize; // 细子树最多包含四个完整中簇。
                    let mut near_instances = 0usize; // 每个被近级接管的中父格恰含四片近叶。
                    for middle_offset_x in 0..2 {
                        for middle_offset_z in 0..2 {
                            let middle_parent_x = far_parent_x * 2 + middle_offset_x;
                            let middle_parent_z = far_parent_z * 2 + middle_offset_z;
                            for near_resident in [false, true] {
                                for middle_selects_near in [false, true] {
                                    let near_selected =
                                        fine_subtree && near_resident && middle_selects_near;
                                    let middle_selected = fine_subtree && !near_selected;
                                    let local_middle_instances = usize::from(middle_selected);
                                    let local_near_instances = if near_selected { 4 } else { 0 };
                                    assert_eq!(
                                        usize::from(!fine_subtree)
                                            + local_middle_instances
                                            + usize::from(local_near_instances > 0),
                                        1,
                                        "每个中父格必须由远祖先、中簇或四片近叶中的唯一一档承担",
                                    ); // 远级按整个四乘四父格托底，细子树内每个二乘二父格再独立完整交接。
                                    if near_resident && middle_selects_near {
                                        assert_eq!(
                                            local_near_instances,
                                            usize::from(fine_subtree) * 4
                                        );
                                    } else {
                                        assert_eq!(local_near_instances, 0);
                                    } // 近级一旦接管必须四片齐全，不能只出现半个父簇。
                                }
                            }
                            let representative_fine_x = middle_parent_x * 2;
                            let representative_fine_z = middle_parent_z * 2;
                            assert_eq!(floor_parent(representative_fine_x, 4), far_parent_x);
                            assert_eq!(floor_parent(representative_fine_z, 4), far_parent_z); // 正负坐标的四个中父格都必须稳定归属当前远父格。
                            if fine_subtree {
                                let choose_near = (middle_parent_x ^ middle_parent_z) & 1 == 0;
                                if choose_near {
                                    near_instances += 4;
                                } else {
                                    middle_instances += 1;
                                }
                            }
                        }
                    }
                    if fine_subtree {
                        assert_eq!(far_instances, 0); // 进入细子树后远祖先严格退出。
                        assert_eq!(middle_instances * 4 + near_instances, 16); // 四个中簇或等价十六近叶始终完整覆盖父簇。
                    } else {
                        assert_eq!((far_instances, middle_instances, near_instances), (1, 0, 0)); // 子级不可用或未被选择时远祖先独占。
                    }
                }
            }
        }
    }
    let child_is_resident = |distance: f32, outer: f32| distance <= (outer - 2.0).max(0.0); // 复现WGSL两米安全域边界。
    assert!(child_is_resident(44.0, 46.0)); // 近级完整安全边界仍允许接管。
    assert!(!child_is_resident(44.001, 46.0)); // 一旦进入近窗外沿就必须回退中级。
    assert!(child_is_resident(102.0, 104.0)); // 中级完整安全边界仍允许接管。
    assert!(!child_is_resident(102.001, 104.0)); // 一旦进入中窗外沿就必须回退远级。
}

#[test] // 负世界坐标的父格必须向负无穷取整，不能因整数向零截断在世界原点两侧产生接缝。
fn signed_world_cells_map_to_euclidean_lod_parents() {
    let floor_parent = |value: i32, side: f32| (value as f32 / side).floor() as i32; // 复现WGSL floor_parent_cell的逐轴公式。
    assert_eq!(
        (-5..=4)
            .map(|cell| floor_parent(cell, 2.0))
            .collect::<Vec<_>>(),
        vec![-3, -2, -2, -1, -1, 0, 0, 1, 1, 2]
    ); // 每两个连续最细格只映射到一个中父格，负坐标同样成立。
    assert_eq!(
        (-5..=4)
            .map(|cell| floor_parent(cell, 4.0))
            .collect::<Vec<_>>(),
        vec![-2, -1, -1, -1, -1, 0, 0, 0, 0, 1]
    ); // 每四个连续最细格只映射到一个远父格，原点两侧没有不等宽父格。
}

#[test] // Vulkan 兼容展开必须逐项保留原 Bayer 密度排列，不能改变稳定候选集合。
fn packed_density_ranks_preserve_all_original_entries() {
    // 从生产 WGSL 读取两个常量，避免测试仅验证自己的复制值。
    let read = |name: &str| {
        // 解码源码中稳定命名的十六进制常量。
        let prefix = format!("const {name}: u32 = 0x"); // 与生产声明格式契约一致。
        let line = COMPUTE_SHADER_SOURCE
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .expect("生产 Shader 必须声明打包密度名次"); // 不能静默使用测试默认值。
        u32::from_str_radix(line.split('u').next().unwrap(), 16)
            .expect("密度名次必须是合法 u32 十六进制") // 读取真正生产数据。
    }; // 完成两个固定标量的解析器。
    let words = [read("DENSITY_RANK_LOW"), read("DENSITY_RANK_HIGH")]; // 索引零至七、八至十五各占一个 u32。
    let expected = [0u32, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5]; // 原 Shader 四乘四 Bayer 排列。
    for (index, expected) in expected.into_iter().enumerate() {
        // 穷举全部输入而非抽样。
        assert_eq!(
            (words[index / 8] >> ((index % 8) * 4)) & 15,
            expected,
            "原密度名次索引 {index} 必须不变"
        ); // 保留相同前 N 项选草集合。
    } // 完成十六输入的精确回归。
    assert!(
        COMPUTE_SHADER_SOURCE.contains("return (packed_ranks >> ((rank_index & 7u) * 4u)) & 15u;")
    ); // GPU 必须使用同一四位取槽规则。
    assert!(!COMPUTE_SHADER_SOURCE.contains("var corners = array<")); // 不恢复会触发 Naga 29 非法 ArrayStride 的函数数组。
    assert!(
        !COMPUTE_SHADER_SOURCE
            .lines()
            .any(|line| line.starts_with("var<workgroup>") && line.contains("array<"))
    ); // 工作组标量展开也需保留到依赖修复。
} // 完成 Vulkan 数组兼容语义回归。
