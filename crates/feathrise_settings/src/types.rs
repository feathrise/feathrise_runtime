//! 渲染请求的公开值类型；不承载游戏或编辑器的应用偏好。

mod ambient_occlusion; // 屏幕空间遮蔽档位与 SSAO/MSAA 互斥规则独立维护。
mod detail_quality; // 内容细节档位独立维护，避免核心画质类型文件继续膨胀。
mod dlss; // NVIDIA DLSS 正式模式与 Auto 输出分辨率解析规则。
mod mesh_lod; // 普通静态网格显示与碰撞的独立全局简化策略。
mod frame_generation; // DLSS-G 与 FSR-G 共用的启动期交换链所有权请求。
mod fsr; // AMD FSR 3时间域超分固定模式与持久化规则。
mod post_processing; // 独立后处理和体积光档位集中维护，避免核心类型文件超过模块上限。
mod texture_filtering; // StandardMaterial 纹理过滤档位独立维护，默认不覆盖作者采样器。
pub use ambient_occlusion::{AmbientOcclusionQuality, ContactShadowQuality}; // 导出独立屏幕空间遮蔽档位。
pub use detail_quality::{EffectsQuality, SceneDetailQuality, VegetationQuality}; // 保持原公开路径并导出新增特效档。
pub use dlss::DlssMode; // 设置、UI、Solari 与持久化共用同一正式 DLSS 请求枚举。
pub use mesh_lod::MeshLodSettings;
pub use frame_generation::FrameGenerationMode; // 设置、启动引导与UI共用同一后端枚举。
pub use fsr::FsrMode; // 设置、UI与候选运行时共用同一FSR请求枚举。
pub use post_processing::{
    BloomQuality, RenderScaleQuality, SharpeningQuality, VolumetricFogQuality,
}; // 保持既有公开路径并导出渲染比例档位。
pub use texture_filtering::TextureFilteringQuality; // 导出持久化、UI 与 RenderApp 共用的纹理过滤档位。

/// 输出到窗口的 3D 相机可选抗锯齿模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum AntiAliasingMode {
    Off,   // 关闭全部抗锯齿。
    Fxaa,  // 使用单次后处理 FXAA。
    Msaa2, // 使用 2 倍多重采样。
    #[default]
    Msaa4, // 使用与旧版本一致的 4 倍多重采样。
    Msaa8, // 使用 8 倍多重采样。
    Taa,   // 使用跨帧累积 TAA；当前作为手动实验选项，不进入综合预设。
}

impl AntiAliasingMode {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 6] = [
        Self::Off,   // 关闭全部抗锯齿。
        Self::Fxaa,  // 低成本单帧后处理。
        Self::Msaa2, // 两倍多重采样。
        Self::Msaa4, // 当前高档画面基线。
        Self::Msaa8, // 当前极高档画面基线。
        Self::Taa,   // 手动实验性跨帧抗锯齿。
    ];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关闭",
            Self::Fxaa => "FXAA",
            Self::Msaa2 => "2×",
            Self::Msaa4 => "4×",
            Self::Msaa8 => "8×",
            Self::Taa => "TAA",
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Fxaa => "Fxaa",
            Self::Msaa2 => "Msaa2",
            Self::Msaa4 => "Msaa4",
            Self::Msaa8 => "Msaa8",
            Self::Taa => "Taa",
        }
    }

    /// 从持久化枚举名读取模式，未知值安全回退到旧版画面基线。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Off" => Self::Off,
            "Fxaa" => Self::Fxaa,
            "Msaa2" => Self::Msaa2,
            "Msaa8" => Self::Msaa8,
            "Taa" => Self::Taa,
            _ => Self::Msaa4,
        }
    }
}
/// 全局实时阴影质量；关闭只停用阴影资格，不改变灯光作者配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ShadowQuality {
    Off,    // 暂停全部实时阴影。
    Low,    // 方向光/聚光灯 512，点光 256。
    Medium, // 方向光/聚光灯 1024，点光 512。
    #[default]
    High, // 保持旧版 2048/1024 基线。
    Ultra,  // 方向光/聚光灯 4096，点光 2048。
}

impl ShadowQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Off => "关闭",
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
            Self::Ultra => "极高",
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }

    /// 从持久化枚举名读取质量，未知值安全回退到旧版阴影基线。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Off" => Self::Off,
            "Low" => Self::Low,
            "Medium" => Self::Medium,
            "Ultra" => Self::Ultra,
            _ => Self::High,
        }
    }

    /// 返回方向光/聚光灯与点光阴影贴图边长。
    pub const fn shadow_map_sizes(self) -> (usize, usize) {
        match self {
            Self::Off | Self::Low => (512, 256), // 关闭时保留最低合法资源尺寸，灯光本身不会投影。
            Self::Medium => (1024, 512),
            Self::High => (2048, 1024),
            Self::Ultra => (4096, 2048),
        }
    }
}
/// 方向光阴影可见距离与级联数量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ShadowDistanceQuality {
    Low,    // 50 米、2 级联，优先降低低档阴影绘制成本。
    Medium, // 100 米、3 级联。
    #[default]
    High, // 150 米、4 级联，保持 Bevy 0.19.1 当前基线。
    Ultra,  // 300 米、4 级联，扩大远景阴影范围。
}

impl ShadowDistanceQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
            Self::Ultra => "极高",
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Ultra => "Ultra",
        }
    }

    /// 从持久化枚举名读取档位，未知值回退到 150 米四级联基线。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Low" => Self::Low,
            "Medium" => Self::Medium,
            "Ultra" => Self::Ultra,
            _ => Self::High,
        }
    }

    /// 返回方向光级联数量与最远阴影距离（米）。
    pub const fn cascade_parameters(self) -> (usize, f32) {
        match self {
            Self::Low => (2, 50.0),
            Self::Medium => (3, 100.0),
            Self::High => (4, 150.0),
            Self::Ultra => (4, 300.0),
        }
    }
}
/// 窗口目标 3D 相机使用的实时阴影滤波档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ShadowFilteringQuality {
    Fast, // Hardware2x2，优先速度。
    #[default]
    Soft, // Gaussian，保持 Bevy 默认柔和阴影基线。
}

impl ShadowFilteringQuality {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 2] = [Self::Fast, Self::Soft];

    /// 返回面向玩家的简短名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Fast => "快速",
            Self::Soft => "柔和",
        }
    }

    /// 返回 settings.json 使用的稳定枚举名。
    pub const fn storage_name(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Soft => "Soft",
        }
    }

    /// 从持久化枚举名读取档位，未知值回退到 Gaussian 柔和滤波。
    pub fn from_storage_name(value: &str) -> Self {
        match value {
            "Fast" => Self::Fast,
            _ => Self::Soft,
        }
    }
}

/// 同时联动抗锯齿、阴影贴图、距离、滤波、植被、场景细节和特效的综合画质预设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum GraphicsQualityPreset {
    Low,    // 低画质。
    Medium, // 中画质。
    #[default]
    High, // 与旧版本一致的默认高画质。
    Ultra,  // 极高画质。
}

impl GraphicsQualityPreset {
    /// 设置界面的固定排列顺序。
    pub const ALL: [Self; 4] = [Self::Low, Self::Medium, Self::High, Self::Ultra];

    /// 返回面向玩家的预设名称。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Low => "低",
            Self::Medium => "中",
            Self::High => "高",
            Self::Ultra => "极高",
        }
    }

    /// 返回预设对应的抗锯齿与阴影组合。
    pub const fn values(self) -> (AntiAliasingMode, ShadowQuality) {
        match self {
            Self::Low => (AntiAliasingMode::Fxaa, ShadowQuality::Low),
            Self::Medium => (AntiAliasingMode::Msaa2, ShadowQuality::Medium),
            Self::High => (AntiAliasingMode::Msaa4, ShadowQuality::High),
            Self::Ultra => (AntiAliasingMode::Msaa8, ShadowQuality::Ultra),
        }
    }

    /// 返回预设对应的阴影距离与滤波组合，不改变既有 `values()` 签名。
    pub const fn shadow_details(self) -> (ShadowDistanceQuality, ShadowFilteringQuality) {
        match self {
            Self::Low => (ShadowDistanceQuality::Low, ShadowFilteringQuality::Fast),
            Self::Medium => (ShadowDistanceQuality::Medium, ShadowFilteringQuality::Soft),
            Self::High => (ShadowDistanceQuality::High, ShadowFilteringQuality::Soft),
            Self::Ultra => (ShadowDistanceQuality::Ultra, ShadowFilteringQuality::Soft),
        }
    }

    /// 返回预设对应的 GPU 叶片植被质量。
    pub const fn vegetation_quality(self) -> VegetationQuality {
        match self {
            Self::Low => VegetationQuality::Low,
            Self::Medium => VegetationQuality::Medium,
            Self::High => VegetationQuality::High,
            Self::Ultra => VegetationQuality::Ultra,
        }
    }

    /// 返回预设对应的树木、灌木与岩石场景细节质量。
    pub const fn scene_detail_quality(self) -> SceneDetailQuality {
        match self {
            Self::Low => SceneDetailQuality::Low,
            Self::Medium => SceneDetailQuality::Medium,
            Self::High => SceneDetailQuality::High,
            Self::Ultra => SceneDetailQuality::Ultra,
        }
    }

    /// 返回预设对应的技能与天气 GPU 粒子质量。
    pub const fn effects_quality(self) -> EffectsQuality {
        match self {
            Self::Low => EffectsQuality::Low,
            Self::Medium => EffectsQuality::Medium,
            Self::High => EffectsQuality::High,
            Self::Ultra => EffectsQuality::Ultra,
        }
    }
}
/// 可持久化的画质设置；综合预设由七个联动单项实时推导，独立纹理过滤、后处理和体积雾不参与判定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    pub anti_aliasing: AntiAliasingMode, // 窗口目标 3D 相机的抗锯齿。
    pub shadow_quality: ShadowQuality,   // 全局实时阴影贴图质量。
    pub shadow_distance: ShadowDistanceQuality, // 方向光阴影距离与级联数。
    pub shadow_filtering: ShadowFilteringQuality, // 窗口相机的阴影滤波。
    pub vegetation_quality: VegetationQuality, // GPU 叶片植被密度、范围与 LOD 档位。
    pub scene_detail_quality: SceneDetailQuality, // 树木、灌木与岩石视觉 LOD 和最远范围档位。
    pub effects_quality: EffectsQuality, // 技能与天气 GPU 粒子数量档位。
    pub texture_filtering: TextureFilteringQuality, // 独立 StandardMaterial 线性纹理各向异性过滤档位。
    pub render_scale: RenderScaleQuality,           // 独立窗口 3D 主场景内部渲染比例。
    pub bloom_quality: BloomQuality,                // 独立 Bloom 后处理档位。
    pub ambient_occlusion_quality: AmbientOcclusionQuality, // 独立玩法相机 SSAO 档位。
    pub contact_shadow_quality: ContactShadowQuality, // 独立玩法相机接触阴影档位。
    pub sharpening_quality: SharpeningQuality,      // 独立 CAS 自适应锐化档位。
    pub volumetric_fog_quality: VolumetricFogQuality, // 独立玩法体积雾与天体光束档位。
    pub fsr_mode: FsrMode,                          // 独立 AMD FSR 3时间域超分请求模式。
    pub fsr_sharpening_percent: u8,                 // FSR内部RCAS锐化百分比，运行时钳制到0..=100。
    pub dlss_mode: DlssMode,                        // 独立 DLSS SR/RR 请求模式。
    pub raytracing_enabled: bool,                   // 是否请求当前受支持场景启用 Solari。
    pub ray_reconstruction_enabled: bool,           // RR 偏好；依赖不满足时保留选择但不生效。
    pub frame_generation_mode: FrameGenerationMode, // 下次启动取得交换链所有权的固定2x后端。
    pub dlss_sharpening_percent: u8, // DLSS 后处理 CAS 锐化百分比，运行时钳制到 0..=100。
    pub vsync: bool,                 // 独立主窗口垂直同步开关。
}

impl GraphicsSettings {
    /// 应用综合预设，联动抗锯齿、三个阴影细项、植被、场景细节和特效质量。
    pub fn apply_preset(&mut self, preset: GraphicsQualityPreset) {
        let (anti_aliasing, shadow_quality) = preset.values(); // 先读取兼容既有签名的抗锯齿与阴影组合。
        self.set_anti_aliasing(anti_aliasing); // 含 MSAA 的预设必须按互斥规则关闭 SSAO。
        self.shadow_quality = shadow_quality; // 阴影贴图仍严格跟随综合档位。
        (self.shadow_distance, self.shadow_filtering) = preset.shadow_details();
        self.vegetation_quality = preset.vegetation_quality(); // 综合档必须真实降低主要 GPU 叶片负载。
        self.scene_detail_quality = preset.scene_detail_quality(); // 综合档同步降低固定景观物的视觉 LOD 成本。
        self.effects_quality = preset.effects_quality(); // 综合档同步调整技能与天气透明粒子数量。
    }

    /// 根据七个联动单项推导综合预设，纹理过滤、渲染比例、后处理、体积雾与垂直同步不参与判定。
    pub fn preset(self) -> Option<GraphicsQualityPreset> {
        GraphicsQualityPreset::ALL.into_iter().find(|preset| {
            preset.values() == (self.anti_aliasing, self.shadow_quality)
                && preset.shadow_details() == (self.shadow_distance, self.shadow_filtering)
                && preset.vegetation_quality() == self.vegetation_quality
                && preset.scene_detail_quality() == self.scene_detail_quality
                && preset.effects_quality() == self.effects_quality
        })
    }

    /// 仅恢复画质页默认值，不影响显示、控制或快捷键设置。
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 按当前硬件已确认的 DLSS SR 支持状态恢复画质页正式默认值。
    pub fn reset_for_dlss_support(&mut self, dlss_supported: bool) {
        *self = Self::default(); // 先恢复与旧玩家一致的全部普通画质基线。
        if dlss_supported {
            self.dlss_mode = DlssMode::Auto; // 仅支持设备的首次安装或手动恢复默认采用 Auto。
            self.ray_reconstruction_enabled = true; // 预选 RR，但光追默认关闭且 UI 会保留依赖选择。
        }
    }

    /// 设置 FSR 超分模式；非关闭档仅关闭 DLSS SR/RR，独立深度/运动输入使两种帧生成均不依赖超分。
    pub fn set_fsr_mode(&mut self, mode: FsrMode) {
        self.fsr_mode = mode; // 首先保存玩家明确选择。
        if mode.is_enabled() {
            self.dlss_mode = DlssMode::Off; // 同一相机不得同时运行两种时间域超分。
            self.ray_reconstruction_enabled = false; // RR 依赖DLSS并会替换独立SR。
        }
    }

    /// 设置 DLSS 模式；非关闭档按“最后选择生效”关闭 FSR 请求，切档和关闭均保留独立帧生成选择。
    pub fn set_dlss_mode(&mut self, mode: DlssMode) {
        self.dlss_mode = mode; // 保留完整DLSS模式，包括Auto和DLAA。
        if mode.is_enabled() {
            self.fsr_mode = FsrMode::Off; // 防止两个时间重建节点同帧接管。
        }
    }

    /// 设置下次启动取得交换链所有权的固定2x后端。
    pub fn set_frame_generation_mode(&mut self, mode: FrameGenerationMode) {
        self.frame_generation_mode = mode; // DLSS-G与FSR-G互斥由单一枚举结构保证。
        if mode == FrameGenerationMode::Dlss2x {
            self.vsync = false; // 当前Streamline Vulkan路径不支持VSync；FSR 3不擅自改写该偏好。
        }
    }

    /// 钳制可能由外部代码或手改配置写入的两种厂商超分锐化百分比。
    pub fn sanitize_vendor_upscaling_values(&mut self) {
        self.fsr_sharpening_percent = self.fsr_sharpening_percent.min(100); // FSR内部RCAS同样只接受百分比范围。
        self.dlss_sharpening_percent = self.dlss_sharpening_percent.min(100); // 百分比上限固定为一百。
    }
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            anti_aliasing: AntiAliasingMode::Msaa4, // 保持 Bevy 当前 4× MSAA 基线。
            shadow_quality: ShadowQuality::High,    // 保持 2048/1024 阴影贴图基线。
            shadow_distance: ShadowDistanceQuality::High, // 保持 150 米四级联基线。
            shadow_filtering: ShadowFilteringQuality::Soft, // 保持 Gaussian 滤波基线。
            vegetation_quality: VegetationQuality::High, // 保持全部 GPU 叶片作者密度、范围与 LOD 基线。
            scene_detail_quality: SceneDetailQuality::High, // 保持树木、灌木与岩石作者 LOD 和最远范围。
            effects_quality: EffectsQuality::High,          // 保持技能与天气 GPU 粒子作者数量基线。
            texture_filtering: TextureFilteringQuality::Off, // 保持纹理作者采样器与三线性 1× 基线。
            render_scale: RenderScaleQuality::Native,       // 保持窗口 3D 主场景原生分辨率基线。
            bloom_quality: BloomQuality::Off,               // 保持当前无 Bloom 的 LDR 基线。
            ambient_occlusion_quality: AmbientOcclusionQuality::Off, // 保持当前无 SSAO 的性能与画面基线。
            contact_shadow_quality: ContactShadowQuality::Off, // 保持当前无接触阴影的性能与画面基线。
            sharpening_quality: SharpeningQuality::Off,        // 保持当前无全局 CAS 的锐度基线。
            volumetric_fog_quality: VolumetricFogQuality::Off, // 保持当前只有距离雾、无体积光的基线。
            fsr_mode: FsrMode::Off, // 候选构建和旧配置均不得自动启用FSR超分。
            fsr_sharpening_percent: 0, // 默认不执行FSR内部RCAS。
            dlss_mode: DlssMode::Off, // 旧配置和便携构建不得自动改变画面。
            raytracing_enabled: false, // 首次支持设备也默认保持光栅路径。
            ray_reconstruction_enabled: false, // 旧配置缺少字段时不得悄然预选厂商降噪。
            frame_generation_mode: FrameGenerationMode::Off, // 旧配置与普通构建不得自动改变呈现节奏。
            dlss_sharpening_percent: 0,                      // 默认不执行 DLSS 后处理 CAS。
            vsync: false,                                    // 保持项目当前关闭垂直同步的基线。
        }
    }
}
mod air_wall_display;
#[cfg(test)]
mod tests; // 空气墙辅助显示与碰撞开关独立。
pub use air_wall_display::AirWallDisplayMode;
