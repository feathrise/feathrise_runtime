//! SSR颜色复制的完整规格与租约动作值；纯决策测试不依赖GPU。
use bevy::render::render_resource::{Extent3d, TextureFormat}; // 源和目标复制必须逐项相同。

#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 规格按值比较，无图像或CPU像素副本。
pub(super) struct ReflectionTextureSpec {
    // 仅支持当前视图解析后的二维单采样颜色。
    pub(super) size: Extent3d, // 实际ViewTarget内部纹理尺寸，不读取窗口逻辑分辨率。
    pub(super) format: TextureFormat, // 保留HDR和非HDR原格式，避免复制时颜色空间变换。
    pub(super) sample_count: u32, // 必须为一，不能直接复制MSAA附件。
} // 结束完整颜色规格。

#[derive(Clone, Copy, Debug, PartialEq, Eq)] // 所有权决策与具体GPU对象分离。
pub(super) enum ReflectionTextureAction {
    // 每个视图一帧只采取一种资源动作。
    RemoveOwned,     // 资格撤销且当前纹理仍属于水面反射。
    ForgetOwnership, // 标准折射已替换纹理，只移除过期所有权记录。
    Keep,            // 本帧已有完整同规格纹理，无额外GPU分配。
    Allocate,        // 新视图或尺寸/格式变化时创建一次完整目标。
    Ignore,          // 非目标且没有所有权时完全不干预。
} // 结束资源动作枚举。
