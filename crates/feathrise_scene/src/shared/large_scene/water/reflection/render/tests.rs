//! 水面捕获资源租约回归，防止删除标准折射和反复分配稳定目标。
use super::*; // 测试生产所有权决策。

#[test] // 退出反射仅能撤销自己仍然拥有的纹理。
fn removal_respects_standard_transmission_takeover() {
    // 热切画质或相机后不能破坏玻璃折射。
    assert_eq!(
        action(false, true, true, false, true),
        ReflectionTextureAction::RemoveOwned
    ); // 自有目标可释放。
    assert_eq!(
        action(false, true, false, true, true),
        ReflectionTextureAction::ForgetOwnership
    ); // 标准系统已经替换对象则仅忘记旧租约。
    assert_eq!(
        action(false, false, false, true, true),
        ReflectionTextureAction::Ignore
    ); // 从未拥有的标准目标不修改。
} // 完成所有权撤销回归。

#[test] // 新视图、规格变化和稳定帧使用不同资源动作。
fn current_view_reuses_matching_texture_and_rebuilds_resize() {
    // 每视图纹理独立，不依赖窗口全局尺寸。
    assert_eq!(
        action(true, false, false, false, false),
        ReflectionTextureAction::Allocate
    ); // 没有标准折射物也能创建SSR副本。
    assert_eq!(
        action(true, true, true, false, true),
        ReflectionTextureAction::Keep
    ); // 稳定自有纹理不反复分配。
    assert_eq!(
        action(true, false, false, true, true),
        ReflectionTextureAction::Keep
    ); // 活动标准折射纹理直接复用。
    assert_eq!(
        action(true, true, false, true, true),
        ReflectionTextureAction::ForgetOwnership
    ); // 活动标准纹理接管后保留数据但撤销旧所有权。
    assert_eq!(
        action(true, true, true, false, false),
        ReflectionTextureAction::Allocate
    ); // Resize或HDR变化用完整新目标替换。
    assert_eq!(
        action(true, false, false, false, true),
        ReflectionTextureAction::Allocate
    ); // 无活动标准折射时不借用遗留旧纹理。
} // 完成标准共存与尺寸变化回归。

#[test] // GPU复制必须使用同格式同尺寸单采样源，不能误复制MSAA附件或上个窗口尺寸。
fn copy_rejects_multisampling_resize_and_format_changes() {
    // 独立验证复制守卫，避免GPU错误延迟到进图。
    let source = ReflectionTextureSpec {
        size: Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
        format: TextureFormat::Rgba16Float,
        sample_count: 1,
    }; // 实际主视图HDR单采样布局。
    assert!(copy_specs_match(source, source)); // 完整相同规格允许一次颜色copy。
    assert!(!copy_specs_match(
        source,
        ReflectionTextureSpec {
            sample_count: 4,
            ..source
        }
    )); // 不复制或解析多采样目标。
    assert!(!copy_specs_match(
        ReflectionTextureSpec {
            sample_count: 4,
            ..source
        },
        ReflectionTextureSpec {
            sample_count: 4,
            ..source
        }
    )); // 即使两边同为MSAA仍不冒充已解析颜色。
    assert!(!copy_specs_match(
        source,
        ReflectionTextureSpec {
            format: TextureFormat::Rgba8UnormSrgb,
            ..source
        }
    )); // HDR切换期间旧图拒绝。
    assert!(!copy_specs_match(
        source,
        ReflectionTextureSpec {
            size: Extent3d {
                width: 1920,
                ..source.size
            },
            ..source
        }
    )); // 窗口与渲染比例目标变化不能越界复制。
} // 完成实际GPU契约回归。

#[test] // 用户自定义CameraMainTextureUsages缺少COPY_SRC时必须准备侧退回IBL。
fn missing_copy_source_usage_revokes_owned_capture() {
    // 不把非法GPU命令留到驱动验证时才失败。
    let source = ReflectionTextureSpec {
        size: Extent3d {
            width: 1280,
            height: 720,
            depth_or_array_layers: 1,
        },
        format: TextureFormat::Rgba16Float,
        sample_count: 1,
    }; // 其余规格均合法。
    let usage = TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING; // 模拟用户删除默认COPY_SRC。
    assert!(!source_is_eligible(source, usage)); // 缺必要usage立即拒绝。
    assert_eq!(
        action(source_is_eligible(source, usage), true, true, false, true),
        ReflectionTextureAction::RemoveOwned
    ); // 已有本功能图片也撤销，标准绑定恢复零图。
    assert!(source_is_eligible(source, usage | TextureUsages::COPY_SRC)); // 恢复能力后正常允许重新捕获。
    let non_hdr = ReflectionTextureSpec {
        format: TextureFormat::Rgba8UnormSrgb,
        ..source
    }; // 非HDR视图已经在材质Shader里完成色调映射。
    assert!(!source_is_eligible(
        non_hdr,
        usage | TextureUsages::COPY_SRC
    )); // 不能把显示颜色当线性HDR再次做Fresnel混合。
    assert_eq!(
        action(
            source_is_eligible(non_hdr, usage | TextureUsages::COPY_SRC),
            true,
            true,
            false,
            false
        ),
        ReflectionTextureAction::RemoveOwned
    ); // HDR切换过渡撤销旧自有图片，不能保留旧场景反射。
} // 完成自定义相机usage回归。

#[test] // 可见透明对象只有真实水材质才授权完整视图颜色copy。
fn material_identity_excludes_unrelated_transparent_objects() {
    // 透明玻璃、粒子等不能被误当成可见水。
    let water = Handle::<super::super::super::material::DynamicWaterMaterial>::default()
        .id()
        .untyped(); // 项目水类型的真实资产身份。
    let standard = Handle::<StandardMaterial>::default().id().untyped(); // 普通透明StandardMaterial即使外观相似也不授权。
    assert!(is_water_material(water)); // 动态水实际队列项可以授权。
    assert!(!is_water_material(standard)); // 普通材质不触发水面全屏copy。
} // 完成现有材质索引类型回归。
