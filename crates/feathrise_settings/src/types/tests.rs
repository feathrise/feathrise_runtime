use super::*; // 复用公开画质类型与默认设置。

#[test]
fn default_graphics_quality_matches_existing_high_baseline() {
    let graphics = GraphicsSettings::default(); // 功能加入前的正式画面必须映射为高档。
    assert_eq!(graphics.preset(), Some(GraphicsQualityPreset::High)); // 七个联动字段应完整匹配高档。
    assert_eq!(graphics.anti_aliasing, AntiAliasingMode::Msaa4); // 高档保持四倍 MSAA。
    assert_eq!(graphics.shadow_quality.shadow_map_sizes(), (2048, 1024)); // 保持阴影贴图基线。
    assert_eq!(graphics.shadow_distance.cascade_parameters(), (4, 150.0)); // 保持四级联与一百五十米距离。
    assert_eq!(graphics.shadow_filtering, ShadowFilteringQuality::Soft); // 保持 Gaussian 柔和滤波。
    assert_eq!(
        graphics.vegetation_quality.runtime_multipliers(),
        (1.0, 1.0, 1.0)
    ); // 植被保持作者值。
    assert_eq!(
        graphics.scene_detail_quality.runtime_multipliers(),
        (1.0, 1.0)
    ); // 场景物件保持作者值。
    assert_eq!(graphics.effects_quality, EffectsQuality::High); // 技能和天气粒子保持作者数量。
    assert_eq!(graphics.effects_quality.particle_multiplier(), 1.0); // 高档倍率必须精确为一。
    assert_eq!(graphics.texture_filtering, TextureFilteringQuality::Off); // 默认不覆盖纹理作者采样器和现有三线性 1× 基线。
    assert_eq!(graphics.render_scale, RenderScaleQuality::Native); // 保持窗口主场景原生分辨率基线。
    assert_eq!(graphics.fsr_mode, FsrMode::Off); // 默认构建与旧配置不自动启用候选超分。
    assert_eq!(graphics.fsr_sharpening_percent, 0); // FSR 内建 RCAS 默认不改变作者锐度。
    assert_eq!(graphics.bloom_quality, BloomQuality::Off); // 保持无 Bloom 基线。
    assert_eq!(
        graphics.ambient_occlusion_quality,
        AmbientOcclusionQuality::Off
    ); // 保持无 SSAO 的画面与 GPU 成本基线。
    assert_eq!(graphics.contact_shadow_quality, ContactShadowQuality::Off); // 保持无接触阴影基线。
    assert_eq!(graphics.sharpening_quality, SharpeningQuality::Off); // 保持无全局 CAS 基线。
    assert_eq!(graphics.volumetric_fog_quality, VolumetricFogQuality::Off); // 保持只有距离雾、无体积光基线。
    assert_eq!(graphics.frame_generation_mode, FrameGenerationMode::Off); // 默认不改变呈现倍数或输入延迟。
    assert!(!graphics.vsync); // 保持关闭垂直同步。
}

#[test]
fn volumetric_fog_quality_has_bounded_steps_and_safe_storage_fallback() {
    assert_eq!(VolumetricFogQuality::Off.raymarch_steps(), None); // 关闭档不创建体积雾视图。
    assert_eq!(VolumetricFogQuality::Low.raymarch_steps(), Some(32)); // 低档固定三十二步预算。
    assert_eq!(VolumetricFogQuality::High.raymarch_steps(), Some(64)); // 高档使用 Bevy 默认六十四步预算。
    assert_eq!(VolumetricFogQuality::High.storage_name(), "High"); // JSON 使用稳定 PascalCase 名称。
    assert_eq!(
        VolumetricFogQuality::from_storage_name("Cinematic"),
        VolumetricFogQuality::Off
    ); // 未知枚举安全关闭，旧配置不会增加全屏渲染成本。
}

#[test]
fn experimental_taa_has_a_stable_storage_name_without_changing_defaults() {
    assert!(AntiAliasingMode::ALL.contains(&AntiAliasingMode::Taa)); // 设置页必须公开手动 TAA 入口。
    assert_eq!(AntiAliasingMode::Taa.display_name(), "TAA"); // 玩家界面使用通行缩写。
    assert_eq!(AntiAliasingMode::Taa.storage_name(), "Taa"); // JSON 使用稳定 PascalCase 名称。
    assert_eq!(
        AntiAliasingMode::from_storage_name("Taa"),
        AntiAliasingMode::Taa
    ); // 合法名称必须无损恢复。
    assert_eq!(
        GraphicsSettings::default().anti_aliasing,
        AntiAliasingMode::Msaa4
    ); // 新选项不得改变默认画面。
}

#[test]
fn sharpening_quality_has_documented_strengths_and_safe_storage_fallback() {
    assert_eq!(SharpeningQuality::Off.sharpening_strength(), None); // 关闭档不构造全局 CAS 覆盖。
    assert_eq!(SharpeningQuality::Low.sharpening_strength(), Some(0.25)); // 低档固定轻度锐化。
    assert_eq!(SharpeningQuality::High.sharpening_strength(), Some(0.60)); // 高档采用 Bevy 标准强度。
    assert_eq!(SharpeningQuality::Low.storage_name(), "Low"); // JSON 使用稳定 PascalCase 名称。
    assert_eq!(
        SharpeningQuality::from_storage_name("High"),
        SharpeningQuality::High
    ); // 合法高档无损恢复。
    assert_eq!(
        SharpeningQuality::from_storage_name("Oversharp"),
        SharpeningQuality::Off
    ); // 未知枚举安全关闭。
}

#[test]
fn texture_filtering_has_bounded_anisotropy_and_safe_storage_fallback() {
    let expected = [
        (TextureFilteringQuality::Off, "关闭", None),
        (TextureFilteringQuality::Anisotropic4, "4×", Some(4)),
        (TextureFilteringQuality::Anisotropic8, "8×", Some(8)),
        (TextureFilteringQuality::Anisotropic16, "16×", Some(16)),
    ]; // 档位仅使用 WebGPU 保证范围内的 4/8/16 倍采样预算。
    for (quality, display_name, clamp) in expected {
        assert_eq!(quality.display_name(), display_name);
        assert_eq!(quality.anisotropy_clamp(), clamp);
        assert_eq!(
            TextureFilteringQuality::from_storage_name(quality.storage_name()),
            quality,
        ); // 每个稳定 JSON 名称必须无损往返。
    }
    assert_eq!(
        TextureFilteringQuality::from_storage_name("Anisotropic32"),
        TextureFilteringQuality::Off,
    ); // 未知值不得意外覆盖作者采样器或增加 GPU 成本。
}

#[test]
fn render_scale_has_fixed_tiers_and_full_prepass_compatibility() {
    let expected = [
        (RenderScaleQuality::Performance, "67%", 0.67),
        (RenderScaleQuality::Balanced, "75%", 0.75),
        (RenderScaleQuality::Quality, "85%", 0.85),
        (RenderScaleQuality::Native, "100%", 1.0),
    ];
    for (quality, display_name, scale_factor) in expected {
        assert_eq!(quality.display_name(), display_name); // 玩家看到的百分比必须与运行时系数一致。
        assert_eq!(quality.scale_factor(), scale_factor);
        assert_eq!(
            RenderScaleQuality::from_storage_name(quality.storage_name()),
            quality,
        ); // 四个稳定枚举名均应无损往返。
    }
    assert_eq!(
        RenderScaleQuality::from_storage_name("Dynamic"),
        RenderScaleQuality::Native,
    ); // 未知值不得意外降低旧玩家画面清晰度。

    let mut graphics = GraphicsSettings::default();
    graphics.set_anti_aliasing(AntiAliasingMode::Taa); // 先模拟玩家选择完整预通道 TAA。
    graphics.set_render_scale(RenderScaleQuality::Balanced); // 后选择低比例时最后选择生效。
    assert_eq!(graphics.anti_aliasing, AntiAliasingMode::Fxaa); // 空间放大后改用兼容的单帧 FXAA。
    assert_eq!(graphics.render_scale, RenderScaleQuality::Balanced);
    graphics.set_ambient_occlusion_quality(AmbientOcclusionQuality::High); // 随后开启 SSAO 应恢复完整预通道尺寸。
    assert_eq!(graphics.render_scale, RenderScaleQuality::Native);
    assert_eq!(
        graphics.ambient_occlusion_quality,
        AmbientOcclusionQuality::High
    );
    graphics.set_render_scale(RenderScaleQuality::Quality); // 再选择 85% 时空间放大优先并关闭完整深度效果。
    assert_eq!(
        graphics.ambient_occlusion_quality,
        AmbientOcclusionQuality::Off
    ); // 低比例必须关闭 SSAO。
    graphics.set_contact_shadow_quality(ContactShadowQuality::High); // 随后开启接触阴影时完整深度预通道优先。
    assert_eq!(graphics.render_scale, RenderScaleQuality::Native); // 接触阴影必须恢复 100% 深度有效区域。
    graphics.set_render_scale(RenderScaleQuality::Performance); // 最后再选择 67% 空间比例。
    assert_eq!(graphics.contact_shadow_quality, ContactShadowQuality::Off); // 空间缩放必须关闭会按完整纹理寻址的接触阴影。
}

#[test]
fn ambient_occlusion_quality_has_safe_storage_and_msaa_compatibility_rules() {
    assert_eq!(AmbientOcclusionQuality::ALL.len(), 3); // 设置页只公开关、低、高三档。
    assert_eq!(AmbientOcclusionQuality::Low.storage_name(), "Low"); // JSON 使用稳定 PascalCase 名称。
    assert_eq!(
        AmbientOcclusionQuality::from_storage_name("Cinematic"),
        AmbientOcclusionQuality::Off
    ); // 未知枚举安全关闭，保持旧画面。
    let mut graphics = GraphicsSettings::default(); // 默认从不兼容 SSAO 的 MSAA 4× 开始。
    graphics.set_ambient_occlusion_quality(AmbientOcclusionQuality::High); // 玩家最后选择高 SSAO。
    assert_eq!(graphics.anti_aliasing, AntiAliasingMode::Taa); // 开启时自动使用官方推荐的单采样 TAA。
    assert_eq!(
        graphics.ambient_occlusion_quality,
        AmbientOcclusionQuality::High
    ); // SSAO 档位保持玩家选择。
    graphics.set_anti_aliasing(AntiAliasingMode::Msaa8); // 玩家随后明确选择极高 MSAA。
    assert_eq!(
        graphics.ambient_occlusion_quality,
        AmbientOcclusionQuality::Off
    ); // 最后选择的 MSAA 生效并关闭不兼容 SSAO。
}

#[test]
fn contact_shadow_quality_keeps_antialiasing_but_requires_native_scale() {
    assert_eq!(ContactShadowQuality::ALL.len(), 3); // 设置页只公开关、低、高三档。
    assert_eq!(ContactShadowQuality::Low.display_name(), "低"); // 玩家界面使用简短中文档名。
    assert_eq!(ContactShadowQuality::High.storage_name(), "High"); // JSON 使用稳定 PascalCase 名称。
    assert_eq!(
        ContactShadowQuality::from_storage_name("Cinematic"),
        ContactShadowQuality::Off
    ); // 未知枚举安全关闭，保持旧画面。
    let mut graphics = GraphicsSettings::default(); // 默认保持 MSAA 4×。
    graphics.set_render_scale(RenderScaleQuality::Quality); // 先模拟玩家使用 85% 空间缩放。
    graphics.set_contact_shadow_quality(ContactShadowQuality::High); // 接触阴影允许复用多采样深度，但必须恢复完整 viewport。
    assert_eq!(graphics.anti_aliasing, AntiAliasingMode::Msaa4); // 独立设置不得改写抗锯齿。
    assert_eq!(graphics.render_scale, RenderScaleQuality::Native); // 最后选择的接触阴影恢复完整深度有效区域。
    assert_eq!(graphics.preset(), Some(GraphicsQualityPreset::High)); // 独立后处理不改变综合预设。
}

#[test]
fn every_preset_maps_to_the_documented_seven_linked_fields() {
    let expected = [
        (
            GraphicsQualityPreset::Low,   // 低档综合预设。
            AntiAliasingMode::Fxaa,       // 低档抗锯齿。
            ShadowQuality::Low,           // 低档阴影贴图。
            ShadowDistanceQuality::Low,   // 低档阴影距离。
            ShadowFilteringQuality::Fast, // 低档快速滤波。
            VegetationQuality::Low,       // 低档植被。
            SceneDetailQuality::Low,      // 低档场景细节。
            EffectsQuality::Low,          // 低档 GPU 粒子。
        ),
        (
            GraphicsQualityPreset::Medium, // 中档综合预设。
            AntiAliasingMode::Msaa2,       // 中档抗锯齿。
            ShadowQuality::Medium,         // 中档阴影贴图。
            ShadowDistanceQuality::Medium, // 中档阴影距离。
            ShadowFilteringQuality::Soft,  // 中档柔和滤波。
            VegetationQuality::Medium,     // 中档植被。
            SceneDetailQuality::Medium,    // 中档场景细节。
            EffectsQuality::Medium,        // 中档 GPU 粒子。
        ),
        (
            GraphicsQualityPreset::High,  // 正式高档预设。
            AntiAliasingMode::Msaa4,      // 高档抗锯齿基线。
            ShadowQuality::High,          // 高档阴影贴图基线。
            ShadowDistanceQuality::High,  // 高档阴影距离基线。
            ShadowFilteringQuality::Soft, // 高档柔和滤波基线。
            VegetationQuality::High,      // 高档植被基线。
            SceneDetailQuality::High,     // 高档场景细节基线。
            EffectsQuality::High,         // 高档 GPU 粒子基线。
        ),
        (
            GraphicsQualityPreset::Ultra, // 极高综合预设。
            AntiAliasingMode::Msaa8,      // 极高抗锯齿。
            ShadowQuality::Ultra,         // 极高阴影贴图。
            ShadowDistanceQuality::Ultra, // 极高阴影距离。
            ShadowFilteringQuality::Soft, // 极高仍使用柔和滤波。
            VegetationQuality::Ultra,     // 极高植被。
            SceneDetailQuality::Ultra,    // 极高场景细节。
            EffectsQuality::Ultra,        // 极高 GPU 粒子。
        ),
    ]; // 固定数组作为综合画质映射的机器可读契约。
    for (preset, aa, shadow, distance, filtering, vegetation, detail, effects) in expected {
        assert_eq!(preset.values(), (aa, shadow)); // 保留既有二元映射接口。
        assert_eq!(preset.shadow_details(), (distance, filtering)); // 验证阴影细项映射。
        assert_eq!(preset.vegetation_quality(), vegetation); // 验证植被映射。
        assert_eq!(preset.scene_detail_quality(), detail); // 验证场景细节映射。
        assert_eq!(preset.effects_quality(), effects); // 验证 GPU 粒子映射。
    }
}

#[test]
fn content_quality_levels_map_to_bounded_runtime_multipliers() {
    let vegetation = [
        (VegetationQuality::Low, (0.55, 1.35, 0.60)), // 低档显著压低密度与范围。
        (VegetationQuality::Medium, (0.75, 1.15, 0.80)), // 中档保留主要轮廓。
        (VegetationQuality::High, (1.0, 1.0, 1.0)),   // 高档严格保持作者基线。
        (VegetationQuality::Ultra, (1.0, 0.8, 1.0)),  // 极高只延伸精细 LOD。
    ]; // 固定植被倍率映射。
    for (quality, multipliers) in vegetation {
        assert_eq!(quality.runtime_multipliers(), multipliers); // 每档必须精确命中设计值。
    }
    let detail = [
        (SceneDetailQuality::Low, (0.60, 0.60)), // 低档成比例降低视觉范围。
        (SceneDetailQuality::Medium, (0.80, 0.80)), // 中档保留主要远景轮廓。
        (SceneDetailQuality::High, (1.0, 1.0)),  // 高档严格保持作者基线。
        (SceneDetailQuality::Ultra, (1.10, 1.0)), // 极高不扩大最远范围。
    ]; // 固定场景细节倍率映射。
    for (quality, multipliers) in detail {
        assert_eq!(quality.runtime_multipliers(), multipliers); // 每档必须精确命中设计值。
    }
}

#[test]
fn effects_quality_maps_to_documented_particle_multipliers_and_storage() {
    let expected = [
        (EffectsQuality::Low, 0.50),    // 低档粒子数量减半。
        (EffectsQuality::Medium, 0.75), // 中档保留四分之三。
        (EffectsQuality::High, 1.0),    // 高档保持当前基线。
        (EffectsQuality::Ultra, 1.25),  // 极高增加四分之一。
    ]; // 固定 GPU 粒子倍率映射。
    for (quality, multiplier) in expected {
        assert_eq!(quality.particle_multiplier(), multiplier); // 运行时必须使用精确设计倍率。
        assert_eq!(
            EffectsQuality::from_storage_name(quality.storage_name()),
            quality
        ); // 稳定名称应无损往返。
    }
    assert_eq!(
        EffectsQuality::from_storage_name("Unlimited"),
        EffectsQuality::High
    ); // 非法值只回退当前基线。
}

#[test]
fn linked_manual_change_is_custom_but_post_processing_and_vsync_are_independent() {
    let mut graphics = GraphicsSettings {
        bloom_quality: BloomQuality::High, // 从完整高档组合开始，并让 Bloom 独立变化。
        ..Default::default()
    };
    graphics.set_render_scale(RenderScaleQuality::Quality); // 固定内部渲染比例独立变化。
    assert_eq!(graphics.preset(), Some(GraphicsQualityPreset::High)); // 空间比例不参与综合预设判定。
    graphics.set_contact_shadow_quality(ContactShadowQuality::High); // 接触阴影独立变化，并按最后选择规则恢复 100%。
    graphics.sharpening_quality = SharpeningQuality::High; // CAS 独立变化。
    graphics.volumetric_fog_quality = VolumetricFogQuality::High; // 体积雾与光束独立变化。
    graphics.texture_filtering = TextureFilteringQuality::Anisotropic16; // 纹理过滤独立变化。
    graphics.vsync = true; // 垂直同步独立变化。
    assert_eq!(graphics.preset(), Some(GraphicsQualityPreset::High)); // 七个独立字段不得改变综合状态。
    graphics.apply_preset(GraphicsQualityPreset::Low); // 低预设使用可与 SSAO 共存的 FXAA。
    graphics.set_ambient_occlusion_quality(AmbientOcclusionQuality::High); // 合法单采样组合中启用 SSAO。
    assert_eq!(graphics.preset(), Some(GraphicsQualityPreset::Low)); // SSAO 本身不参与综合预设判定。
    graphics.effects_quality = EffectsQuality::Low; // 手动降低 GPU 粒子数量。
    assert_eq!(graphics.preset(), Some(GraphicsQualityPreset::Low)); // 低档特效与当前低预设仍保持匹配。
    graphics.effects_quality = EffectsQuality::High; // 制造不匹配的手动特效组合。
    assert_eq!(graphics.preset(), None); // 特效质量属于综合联动字段。
    graphics.apply_preset(GraphicsQualityPreset::High); // 恢复高档七字段并选择 MSAA 4×。
    assert_eq!(
        graphics.ambient_occlusion_quality,
        AmbientOcclusionQuality::Off
    ); // 含 MSAA 的综合预设必须消除 SSAO 冲突。
    graphics.shadow_distance = ShadowDistanceQuality::Ultra; // 再制造阴影距离非标准组合。
    assert_eq!(graphics.preset(), None); // 任一联动字段不匹配都应显示自定义。
}

#[test]
fn reset_restores_complete_graphics_defaults() {
    let mut graphics = GraphicsSettings {
        anti_aliasing: AntiAliasingMode::Off, // 模拟关闭抗锯齿。
        shadow_quality: ShadowQuality::Ultra, // 模拟极高阴影。
        shadow_distance: ShadowDistanceQuality::Ultra, // 模拟极远阴影。
        shadow_filtering: ShadowFilteringQuality::Fast, // 模拟快速滤波。
        vegetation_quality: VegetationQuality::Low, // 模拟低植被。
        scene_detail_quality: SceneDetailQuality::Low, // 模拟低场景细节。
        effects_quality: EffectsQuality::Low, // 模拟低特效粒子。
        texture_filtering: TextureFilteringQuality::Anisotropic16, // 模拟最高各向异性过滤。
        render_scale: RenderScaleQuality::Performance, // 模拟 67% 性能档内部渲染比例。
        bloom_quality: BloomQuality::High,    // 模拟高 Bloom。
        ambient_occlusion_quality: AmbientOcclusionQuality::High, // 模拟高 SSAO。
        contact_shadow_quality: ContactShadowQuality::High, // 模拟高接触阴影。
        sharpening_quality: SharpeningQuality::High, // 模拟高 CAS。
        volumetric_fog_quality: VolumetricFogQuality::High, // 模拟高档体积雾与光束。
        dlss_mode: DlssMode::Performance,     // 模拟独立 DLSS 性能档。
        raytracing_enabled: true,             // 模拟开启 Solari。
        ray_reconstruction_enabled: true,     // 模拟开启 RR 偏好。
        frame_generation_mode: FrameGenerationMode::Fsr3_2x, // 模拟开启跨厂商固定2x帧生成偏好。
        dlss_sharpening_percent: 73,          // 模拟连续后处理锐化。
        fsr_mode: FsrMode::Quality,           // 模拟开启 AMD 质量档。
        fsr_sharpening_percent: 64,           // 模拟 FSR 内建 RCAS 强度。
        vsync: true,                          // 模拟开启垂直同步。
    }; // 构造覆盖全部画质字段的非默认样本。
    assert_eq!(graphics.preset(), None); // 混合组合不应命中综合预设。
    graphics.reset(); // 执行画质页恢复默认。
    assert_eq!(graphics, GraphicsSettings::default()); // 全部字段应恢复完整高档基线。
}

#[test]
fn hardware_aware_reset_only_enables_auto_and_rr_on_supported_devices() {
    let mut graphics = GraphicsSettings::default();
    graphics.reset_for_dlss_support(true);
    assert_eq!(graphics.dlss_mode, DlssMode::Auto);
    assert!(!graphics.raytracing_enabled); // 即使支持，首次默认也不自动开启 Solari。
    assert!(graphics.ray_reconstruction_enabled); // RR 只预选，等待玩家以后开启光追。
    assert_eq!(graphics.dlss_sharpening_percent, 0);

    graphics.reset_for_dlss_support(false);
    assert_eq!(graphics, GraphicsSettings::default()); // 不支持设备四项全部保持关闭基线。
}

#[test]
fn vendor_upscaling_sanitizer_clamps_external_values() {
    let mut graphics = GraphicsSettings {
        dlss_sharpening_percent: u8::MAX,
        fsr_sharpening_percent: u8::MAX,
        ..GraphicsSettings::default()
    };
    graphics.sanitize_vendor_upscaling_values();
    assert_eq!(graphics.dlss_sharpening_percent, 100);
    assert_eq!(graphics.fsr_sharpening_percent, 100);
}

#[test]
fn vendor_super_resolution_last_selection_wins() {
    let mut graphics = GraphicsSettings::default();
    graphics.set_dlss_mode(DlssMode::Quality);
    graphics.ray_reconstruction_enabled = true;
    graphics.frame_generation_mode = FrameGenerationMode::Fsr3_2x;
    graphics.set_fsr_mode(FsrMode::Balanced);
    assert_eq!(graphics.fsr_mode, FsrMode::Balanced);
    assert_eq!(graphics.dlss_mode, DlssMode::Off);
    assert!(!graphics.ray_reconstruction_enabled);
    assert_eq!(graphics.frame_generation_mode, FrameGenerationMode::Fsr3_2x); // FSR超分与FSR帧生成相互独立，不能被厂商超分互斥误关。

    graphics.frame_generation_mode = FrameGenerationMode::Dlss2x;
    graphics.set_fsr_mode(FsrMode::Quality);
    assert_eq!(graphics.frame_generation_mode, FrameGenerationMode::Dlss2x); // DLSS-G具有独立输入，切换FSR超分不得取消玩家帧生成请求。

    graphics.set_dlss_mode(DlssMode::Performance);
    assert_eq!(graphics.dlss_mode, DlssMode::Performance);
    assert_eq!(graphics.fsr_mode, FsrMode::Off);
}

#[test]
fn frame_generation_survives_native_and_vendor_upscaling_transitions() {
    for backend in [FrameGenerationMode::Dlss2x, FrameGenerationMode::Fsr3_2x] {
        // 两种帧生成都必须跨厂商切换和关闭超分保留，不能在设置层制造单向依赖。
        let mut graphics = GraphicsSettings::default(); // 从原生渲染默认值开始。
        graphics.set_frame_generation_mode(backend); // 记录独立后端选择。
        graphics.set_dlss_mode(DlssMode::Quality); // 原生切入DLSS超分。
        assert_eq!(graphics.frame_generation_mode, backend); // 开启DLSS超分保留帧生成。
        graphics.set_dlss_mode(DlssMode::Off); // 关闭DLSS超分恢复原生路径。
        assert_eq!(graphics.frame_generation_mode, backend); // 原生路径仍允许所选帧生成。
        graphics.set_dlss_mode(DlssMode::Dlaa); // DLAA也属于DLSS互斥域。
        graphics.set_fsr_mode(FsrMode::Quality); // 切入FSR必须释放DLSS，不能释放帧生成。
        assert_eq!(graphics.dlss_mode, DlssMode::Off); // 两种时间域超分继续互斥。
        assert_eq!(graphics.frame_generation_mode, backend); // FSR超分保留任一帧生成后端。
        graphics.set_fsr_mode(FsrMode::Off); // 关闭FSR再次恢复原生路径。
        graphics.sanitize_vendor_upscaling_values(); // 外部配置归一化也不能清除帧生成。
        assert_eq!(graphics.frame_generation_mode, backend); // 关闭超分及归一化均保持用户选择。
    }
}

#[test]
fn switching_frame_generation_backend_preserves_upscaling_and_vsync_contract() {
    let mut graphics = GraphicsSettings::default(); // 以普通画质基线构造设置。
    graphics.set_fsr_mode(FsrMode::Balanced); // 选择FSR超分，验证DLSS-G不会强制切换超分。
    graphics.vsync = true; // 显式覆盖基线，验证厂商后端的垂直同步约束。
    graphics.set_frame_generation_mode(FrameGenerationMode::Dlss2x); // 选择独立DLSS帧生成。
    assert_eq!(graphics.frame_generation_mode, FrameGenerationMode::Dlss2x); // 单一枚举仅保留当前后端。
    assert_eq!(graphics.fsr_mode, FsrMode::Balanced); // DLSS-G保留FSR质量选择。
    assert_eq!(graphics.dlss_mode, DlssMode::Off); // 不偷偷开启DLSS超分。
    assert!(!graphics.vsync); // 继续遵守现有Streamline Vulkan禁用VSync契约。
    graphics.set_frame_generation_mode(FrameGenerationMode::Fsr3_2x); // 替换请求为另一个帧生成后端。
    assert_eq!(graphics.frame_generation_mode, FrameGenerationMode::Fsr3_2x); // 两种帧生成无法同时开启。
    assert_eq!(graphics.fsr_mode, FsrMode::Balanced); // 后端切换不修改超分。
    graphics.set_frame_generation_mode(FrameGenerationMode::Off); // 关闭帧生成也应独立。
    assert_eq!(graphics.frame_generation_mode, FrameGenerationMode::Off); // 关闭仅撤销帧生成请求。
    assert_eq!(graphics.fsr_mode, FsrMode::Balanced); // 超分继续保持原有质量。
}
