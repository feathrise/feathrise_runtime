//! 作者动态天空/天空盒请求、公开阶段和相机原状态所有权。

use bevy::core_pipeline::Skybox; // 天空盒模式保存并安装可见 Cube，动态模式只保留 IBL。
use bevy::prelude::*; // Component、Handle、Image与Quat。

use crate::{WorldSkyProfile, time::WorldEnvironmentMapProfile}; // 保存可见天空与环境补光/反射各自的日间基准。

/// 运行时可见天空的互斥来源；场景层显式把场景数据枚举转换到此领域类型。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthoredSkyMode {
    /// Bevy 原生大气与独立动态云生成背景，离线资源只提供 IBL。
    #[default]
    Dynamic, // 保持旧构造器及旧场景的兼容默认。
    /// 主 Cube 生成可见背景并关闭动态大气与云，但继续显示世界时间模拟的天体圆盘。
    Skybox, // 三张 Cube 必须完整就绪后才原子采用。
}

/// 一台相机当前请求的场景天空资源；相同组件同时服务玩法窗口和F12离屏视口。
#[derive(Component, Clone, Debug, PartialEq)]
pub struct AuthoredSkyboxRequest {
    /// 本请求选择的互斥可见天空来源。
    pub mode: AuthoredSkyMode,
    /// 相对assets根的天空主路径；动态模式只派生 IBL，天空盒模式还加载主 Cube。
    pub asset_path: Option<String>,
    /// 天空盒在完整日照下的作者亮度；动态模式仅保留该值供模式切换。
    pub base_brightness: f32,
    /// 完整日照下的环境漫反射补光强度，单位为cd/m²。
    pub base_environment_diffuse_intensity: f32,
    /// 完整日照下的环境镜面反射强度，单位为cd/m²。
    pub base_environment_specular_intensity: f32,
    /// IBL 绕世界Y轴旋转角，允许负角并在构造四元数前归一化。
    pub yaw_degrees: f32,
    /// 场景环境作者修订；迟到资产事件只能作用于仍匹配的请求。
    pub revision: u64,
}

impl AuthoredSkyboxRequest {
    /// 构造兼容旧调用方的动态天空请求；主路径只加载 IBL 两件套。
    pub fn new(
        asset_path: Option<String>, // 调用方直接传场景清单字段，不在此访问文件系统。
        base_brightness: f32,       // 动态模式仅保留并验证天空盒切换基准。
        base_environment_diffuse_intensity: f32, // 人物与地形天空补光拥有独立作者基准。
        base_environment_specular_intensity: f32, // 水面与光滑材质天空反射拥有独立作者基准。
        yaw_degrees: f32,           // 编辑器与玩法使用完全相同的方向。
        revision: u64,              // 每次环境作者变化或场景换代递增。
    ) -> Self {
        Self::new_with_mode(
            AuthoredSkyMode::Dynamic, // 缺省入口保持动态天空，不让旧调用方突然显示烘焙背景。
            asset_path,               // 原路径继续作为 IBL 派生身份。
            base_brightness,          // 天空盒基准仍完整保留。
            base_environment_diffuse_intensity, // 漫反射基准原样转发。
            base_environment_specular_intensity, // 镜面基准原样转发。
            yaw_degrees,              // IBL 方向原样转发。
            revision,                 // 作者修订原样转发。
        )
    }

    /// 构造带显式可见天空模式的统一异步请求。
    #[allow(clippy::too_many_arguments)] // 六个作者字段与一个模式共同构成不可拆分的场景环境快照。
    pub fn new_with_mode(
        mode: AuthoredSkyMode,      // 场景或 F12 已从场景持久枚举完成显式转换。
        asset_path: Option<String>, // 主 Cube 与两张 IBL 共用稳定路径前缀。
        base_brightness: f32,       // 仅天空盒模式写入可见背景。
        base_environment_diffuse_intensity: f32, // 两种模式都使用的漫反射基准。
        base_environment_specular_intensity: f32, // 两种模式都使用的镜面基准。
        yaw_degrees: f32,           // 可见 Cube 与 IBL 使用同一 Y 轴方向。
        revision: u64,              // 迟到资产事件必须匹配当前作者修订。
    ) -> Self {
        Self {
            mode,                                // 模式决定是否请求与采用主 Cube。
            asset_path,                          // 路径验证由加载系统统一执行。
            base_brightness,                     // 天空盒亮度在入口统一检查。
            base_environment_diffuse_intensity,  // 有效性在发起两张派生 Image 请求前统一检查。
            base_environment_specular_intensity, // 有效性在发起两张派生 Image 请求前统一检查。
            yaw_degrees,                         // 非有限值不得进入Quat。
            revision,                            // 后续状态与日志携带同一身份。
        }
    }

    /// 返回运行时实际写入 EnvironmentMapLight 的Y轴旋转。
    pub(crate) fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.yaw_degrees.rem_euclid(360.0).to_radians()) // 等价负角折回稳定范围。
    }

    /// 在发起 AssetServer 请求前拒绝明显不安全或不可能成为环境 IBL 的字段。
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !self.base_brightness.is_finite() || self.base_brightness < 0.0 {
            return Err("天空盒亮度必须是有限非负数".to_owned()); // 动态模式保留该作者值，切换天空盒后才写入可见背景。
        }
        if !self.base_environment_diffuse_intensity.is_finite()
            || self.base_environment_diffuse_intensity < 0.0
        {
            return Err("天空补光强度必须是有限非负数".to_owned()); // 无效漫反射比例不能进入派生Cube。
        }
        if !self.base_environment_specular_intensity.is_finite()
            || self.base_environment_specular_intensity < 0.0
        {
            return Err("天空反射强度必须是有限非负数".to_owned()); // 无效镜面基准不能进入PBR环境Uniform。
        }
        if !self.yaw_degrees.is_finite() {
            return Err("环境 IBL 旋转角必须是有限数".to_owned()); // 非有限 Quat 会污染相机 Uniform。
        }
        if self.mode == AuthoredSkyMode::Skybox && self.asset_path.is_none() {
            return Err("天空盒模式必须指定主KTX2资源".to_owned()); // 不发起必然无法生成可见背景的半套请求。
        }
        if let Some(path) = self.asset_path.as_deref() {
            let lower = path.to_ascii_lowercase(); // 只对扩展名做不区分大小写判断。
            if path.is_empty()
                || path.starts_with('/')
                || path.contains("..")
                || path.contains('\\')
                || !lower.ends_with(".ktx2")
                || lower.ends_with("_diffuse.ktx2")
                || lower.ends_with("_specular.ktx2")
            {
                return Err("天空资源必须是assets内使用正斜杠的KTX2主路径，不能直接选择_diffuse或_specular派生图".to_owned()); // 主路径可显示为背景并始终派生两张 IBL。
            }
        }
        Ok(()) // KTX2内部face与视图类型只能在Image资产解码后验证。
    }

    /// 按固定同目录后缀协议派生漫反射与镜面预过滤资源路径。
    pub(crate) fn asset_paths(&self) -> Option<AuthoredSkyboxAssetPaths> {
        let source = self.asset_path.clone()?; // None请求不产生任何Image依赖。
        let stem = &source[..source.len() - ".ktx2".len()]; // validate保证末尾是ASCII KTX2扩展名。
        Some(AuthoredSkyboxAssetPaths {
            diffuse: format!("{stem}_diffuse.ktx2"), // 低频辐照图由离线工具生成在历史主路径同目录。
            specular: format!("{stem}_specular.ktx2"), // 完整GGX Mip链使用独立派生文件。
            source, // 动态模式只作状态与派生身份，天空盒模式还以该路径加载可见主 Cube。
        })
    }
}

/// 一组离线天空资源的主身份与两张 IBL 派生路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthoredSkyboxAssetPaths {
    pub(crate) source: String, // 动态模式只作派生身份，天空盒模式还加载为可见 Cube。
    pub(crate) diffuse: String, // 余弦卷积Cube用于材质漫反射天空补光。
    pub(crate) specular: String, // 完整GGX Mip链按材质粗糙度提供镜面反射。
}

/// 一台相机的作者天空资源加载阶段。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthoredSkyboxPhase {
    /// 场景未指定环境 IBL，相机保持原环境贴图；动态天空仍可独立显示。
    #[default]
    Disabled,
    /// 路径已受检，正在等待Image解码或热更新重新就绪。
    Loading,
    /// 当前模式要求的 IBL 两件套或天空盒三件套已经原子应用到相机。
    Ready,
    /// 当前请求确认失败，相机已恢复原环境贴图或无 IBL 状态。
    Failed,
}

impl AuthoredSkyboxPhase {
    /// 返回F12和诊断面板使用的稳定中文阶段名。
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Disabled => "未设置",
            Self::Loading => "加载中",
            Self::Ready => "已就绪",
            Self::Failed => "失败",
        }
    }
}

/// F12和诊断面板可读取的相机级天空资源结果。
#[derive(Component, Clone, Debug, Default)]
pub struct AuthoredSkyboxStatus {
    /// 当前加载、活动或失败阶段。
    pub phase: AuthoredSkyboxPhase,
    /// 当前跟踪的相对资产路径；禁用时为None。
    pub asset_path: Option<String>,
    /// 与请求一致的作者修订，便于识别迟到状态。
    pub revision: u64,
    /// 用户可见等待项或失败原因；稳定Ready状态不逐帧重写。
    pub detail: String,
    /// 当前相机是否仍有一张已完整采用的作者可见 Cube；热换等待时可保持 true。
    pub visible_skybox_active: bool,
}

impl AuthoredSkyboxStatus {
    /// 判断当前请求是否真正由一张已采用作者 Cube 提供可见背景。
    pub fn uses_visible_skybox(&self, request: &AuthoredSkyboxRequest) -> bool {
        request.mode == AuthoredSkyMode::Skybox && self.visible_skybox_active // 模式声明与运行时采用证据必须同时成立。
    }
}

/// 当前跟踪句柄在原子天空资源组中的职责。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrackedSkyboxAssetKind {
    Visible,  // 仅天空盒模式存在的可见主 Cube。
    Diffuse,  // 两种模式都存在的余弦卷积辐照 Cube。
    Specular, // 两种模式都存在的 GGX 预过滤 Cube。
}

/// 当前请求跟踪的可选主 Cube 与必需 IBL 两件套强句柄。
#[derive(Clone, Debug)]
pub(crate) struct TrackedAuthoredSkybox {
    pub(crate) mode: AuthoredSkyMode, // 冻结模式，后续采用不再读取可能变化的请求组件。
    pub(crate) paths: AuthoredSkyboxAssetPaths, // 失败日志与事件诊断保留主身份及两张相对路径。
    pub(crate) visible_handle: Option<Handle<Image>>, // 动态模式为 None，天空盒模式强持有主 Cube。
    pub(crate) diffuse_handle: Handle<Image>, // 天空补光等待、活动和热修复期间保持强句柄。
    pub(crate) specular_handle: Handle<Image>, // 水面与材质反射等待、活动和热修复期间保持强句柄。
    pub(crate) revision: u64,         // 新请求覆盖后旧资产事件不得改变当前状态。
    pub(crate) base_brightness: f32,  // 天空盒采用时构造不漂移的昼夜亮度基准。
    pub(crate) base_environment_diffuse_intensity: f32, // 采用时编码漫反射补光比例并保存昼夜基准。
    pub(crate) base_environment_specular_intensity: f32, // 采用时绑定GGX镜面强度并保存昼夜基准。
    pub(crate) rotation: Quat,        // 请求进入时冻结有限归一化方向。
}

impl TrackedAuthoredSkybox {
    /// 返回当前模式的两张或三张明确资产，供有界事件匹配和加载状态复验共用。
    pub(crate) fn assets(
        &self,
    ) -> impl Iterator<Item = (&str, &Handle<Image>, TrackedSkyboxAssetKind)> {
        [
            self.visible_handle.as_ref().map(|handle| {
                (
                    self.paths.source.as_str(),      // 主路径就是可见 Cube 的资源路径。
                    handle,                          // 强句柄只在天空盒模式存在。
                    TrackedSkyboxAssetKind::Visible, // 检查通用 Cube 采样契约。
                )
            }),
            Some((
                self.paths.diffuse.as_str(),     // 固定漫反射派生路径。
                &self.diffuse_handle,            // 补光强句柄。
                TrackedSkyboxAssetKind::Diffuse, // 检查单 Mip 与小尺寸 CPU 像素契约。
            )),
            Some((
                self.paths.specular.as_str(),     // 固定 GGX 派生路径。
                &self.specular_handle,            // 反射强句柄。
                TrackedSkyboxAssetKind::Specular, // 检查完整粗糙度 Mip 链。
            )),
        ]
        .into_iter()
        .flatten() // 固定最多三项，不分配临时 Vec，也不扫描全资产表。
    }
}

/// 天空运行时拥有的相机原状态、当前已采用资源与唯一候选。
#[derive(Component, Clone)]
pub(crate) struct AuthoredSkyboxRuntime {
    pub(crate) original_skybox: Option<Skybox>, // 首次接管前的可见背景在清除资源时精确归还。
    pub(crate) original_sky_profile: Option<WorldSkyProfile>, // 外部天空昼夜基准与其 Cube 同步恢复。
    pub(crate) original_environment_map: Option<EnvironmentMapLight>, // 保存外部环境贴图及其强度、旋转和句柄。
    pub(crate) original_generated_environment_map: Option<GeneratedEnvironmentMapLight>, // 若外部相机本来使用实时生成器也按值恢复。
    pub(crate) original_environment_profile: Option<WorldEnvironmentMapProfile>, // 外部昼夜强度基准不能被作者天空请求覆盖。
    pub(crate) active_source: Option<String>, // 已原子采用的 IBL 身份；候选失败时可保留上一套有效环境。
    pub(crate) active_mode: Option<AuthoredSkyMode>, // 已采用可见来源与路径共同判断热换失败能否保留旧结果。
    pub(crate) active_visible_handle: Option<Handle<Image>>, // 上一有效可见 Cube；动态模式或完整回退时为 None。
    pub(crate) tracked: Option<TrackedAuthoredSkybox>,       // 同一相机最多一个待加载或活动候选。
}
