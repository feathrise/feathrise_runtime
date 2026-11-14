//! GPU草地玩法主相机视锥统一参数的准备与上传。

use bevy::camera::{MainPassResolutionOverride, primitives::Frustum}; // 同时读取Bevy视锥与画质系统写入的实际主通道像素尺寸。
use bevy::prelude::*; // Commands、Query、Res与Vec4等基础类型。
use bevy::render::render_resource::{BindingResource, ShaderType, UniformBuffer}; // 使用std140安全布局上传六平面共享uniform。
use bevy::render::renderer::{RenderDevice, RenderQueue}; // 启动时创建缓冲并在每帧写入当前玩法相机平面。
use bevy::render::view::ExtractedView; // 只读取已经同步到渲染世界的本帧玩法视图。

use crate::components::GpuBladeView; // 排除小场景、阴影和辅助相机，草地Compute只跟随唯一玩法视图。

const INACTIVE_HALF_SPACE_DISTANCE: f32 = 1.0e30; // 非法或无穷远平面替换为永远通过的有限正距离，避免GPU传播Infinity或NaN。
const GRASS_VIEW_MODE_DISABLED: f32 = 0.0; // 无视图或多视图时连六平面、投影LOD与近镜头避让都保守禁用。
const GRASS_VIEW_MODE_LEGACY: f32 = 1.0; // 唯一兼容视图只启用现有六平面剔除并继续使用玩家径向LOD。
const GRASS_VIEW_MODE_SCREEN_SPACE: f32 = 2.0; // 唯一有效透视视图同时启用六平面、投影LOD与近镜头整株避让。
const GRASS_VIEW_MODE_ORTHOGRAPHIC: f32 = 3.0; // 正交视图使用同一像素LOD，但像素比例不再除以相机深度；沿用既有uniform布局。

/// 与WGSL `GrassViewCulling`严格一致的304字节共享相机数据。
#[repr(C)] // 向量与扩展数组在Rust、ShaderType和WGSL中都保持声明顺序和十六字节字段边界。
#[derive(Clone, Copy, Debug, ShaderType)]
pub(super) struct GpuBladeViewCullingUniform {
    pub(super) frustum_planes: [Vec4; 6], // 左、右、上、下、近、远六平面法线及有符号距离。
    pub(super) camera_position_enabled: Vec4, // xyz保留玩法相机世界位置，w为禁用、兼容或屏幕空间明确模式值。
    pub(super) camera_forward_pixel_scale: Vec4, // xyz为世界前向；w为透视单位深度或正交恒定的一米物理像素比例。
    pub(super) lod_pixel_thresholds: Vec4,       // xy为远转中开始/结束，zw为中转近开始/结束。
    pub(super) lod_ultra_pixel_thresholds: Vec4, // xy为超远转入既有远级子树开始/结束，zw保留。
    pub(super) camera_fade_lod_params: Vec4, // xy为近镜头整株避让范围，z为参考草高，w为统一最细草格。
    pub(super) lod_residency_radii: Vec4,    // xyzw为近中远超远常驻外半径。
    pub(super) lod_residency_transition_widths: Vec4, // xyz为近/中/远子级过渡宽度，w为统一安全边距。
    pub(super) quality_multipliers: Vec4, // xyz为最终密度、LOD阈值和作者范围倍率，w保留。
    pub(super) lod_pixel_thresholds_ext: [Vec4; 2],
    pub(super) lod_residency_radii_ext: Vec4,
    pub(super) lod_residency_transitions_ext: Vec4,
    pub(super) lod_metadata: Vec4, // 扩展区和基础区同帧发布，x记录实际档数。
}

impl GpuBladeViewCullingUniform {
    /// 没有唯一玩法视图时返回完全保守的禁用值，不允许误删全部草地。
    fn disabled() -> Self {
        Self {
            frustum_planes: [Vec4::new(0.0, 0.0, 0.0, INACTIVE_HALF_SPACE_DISTANCE); 6], // 六个平面都永远通过。
            camera_position_enabled: Vec4::new(0.0, 0.0, 0.0, GRASS_VIEW_MODE_DISABLED), // 禁用模式不得保留任意旧相机位置。
            camera_forward_pixel_scale: Vec4::ZERO, // 零前向与零比例让后续WGSL即使误读也不会产生有效投影尺寸。
            lod_pixel_thresholds: Vec4::ZERO, // 多视图或无视图时不得随意选择某一台相机的视觉LOD。
            lod_ultra_pixel_thresholds: Vec4::ZERO, // 禁用模式不得触发超远代理交接。
            camera_fade_lod_params: Vec4::ZERO, // 多视图或无视图时不得在世界中错误挖出镜头避让孔。
            lod_residency_radii: Vec4::ZERO,  // 禁用模式不声明任何跨窗口子级可用性。
            lod_residency_transition_widths: Vec4::ZERO, // 禁用模式不得伪造常驻边界过渡。
            quality_multipliers: Vec4::ONE, // 没有唯一合法视图时保持作者基线，禁止异常状态清空草地。
            lod_pixel_thresholds_ext: [Vec4::ZERO; 2],
            lod_residency_radii_ext: Vec4::ZERO,
            lod_residency_transitions_ext: Vec4::ZERO,
            lod_metadata: Vec4::ZERO,
        } // 完成安全禁用数据构造。
    }

    /// 从同一渲染帧的玩法视图和Bevy视锥构造GPU数据。
    fn from_view(
        view: &ExtractedView,
        frustum: &Frustum,
        grass_view: &GpuBladeView,
        resolution_override: Option<&MainPassResolutionOverride>,
    ) -> Self {
        let viewport_height = main_pass_viewport_height(view.viewport.w, resolution_override); // 固定渲染比例启用时LOD必须跟随真实着色像素而非窗口高度。
        let screen_projection = if grass_view.uses_screen_space_lod() {
            perspective_pixel_scale(view.clip_from_view, viewport_height)
                .map(|scale| (GRASS_VIEW_MODE_SCREEN_SPACE, scale))
                .or_else(|| orthographic_pixel_scale(view.clip_from_view, viewport_height)
                    .map(|scale| (GRASS_VIEW_MODE_ORTHOGRAPHIC, scale)))
        } else {
            None
        }; // 作者正交视图不能永久停在首帧祖先托底，否则小范围种植会被最远档稀疏预算清空。
        let screen_space_enabled = screen_projection.is_some();
        let mode = screen_projection.map_or(GRASS_VIEW_MODE_LEGACY, |(mode, _)| mode);
        let camera_forward_pixel_scale = match screen_projection {
            Some((_, pixel_scale)) => {
                view.world_from_view.forward().as_vec3().extend(pixel_scale)
            } // 世界前向与同一视图的物理像素比例必须成对生效。
            _ => Vec4::ZERO, // 兼容模式不向尚未接入的投影LOD暴露半套相机参数。
        }; // 完成相机前向与投影比例打包。
        Self {
            frustum_planes: frustum.half_spaces.map(|half_space| {
                conservative_half_space(half_space.normal_d()) // 保留有效单位平面并把无穷远平面转为有限禁用平面。
            }),
            camera_position_enabled: view.world_from_view.translation().extend(mode), // xyz与明确模式共享一个十六字节槽。
            camera_forward_pixel_scale, // 屏幕空间模式提供相机前向与物理像素比例，兼容模式保持全零。
            lod_pixel_thresholds: if screen_space_enabled {
                grass_view.lod_pixel_thresholds() // 只有完整有效的唯一投影视图才能启用配置阈值。
            } else {
                Vec4::ZERO // 保守回退禁止错误阈值被后续着色器误用。
            },
            lod_ultra_pixel_thresholds: if screen_space_enabled {
                grass_view.lod_ultra_pixel_thresholds() // 普通草和双作物四级窗口共同读取超远转远阈值。
            } else {
                Vec4::ZERO // 兼容路径不允许生成半套超远层级状态。
            },
            camera_fade_lod_params: if screen_space_enabled {
                grass_view.camera_fade_lod_params() // 近镜头整株避让与视觉LOD使用同一模式门。
            } else {
                Vec4::ZERO // 兼容路径绝不改变当前草片可见性。
            },
            lod_residency_radii: if screen_space_enabled {
                grass_view.lod_residency_radii() // 目标细级越过自身常驻安全域时由GPU强制回退祖先代理。
            } else {
                Vec4::ZERO // 旧径向环带不使用跨窗口回退参数。
            },
            lod_residency_transition_widths: if screen_space_enabled {
                grass_view.lod_residency_transition_widths() // 用独立宽带把高秆麦田等内容稳定交还祖先LOD。
            } else {
                Vec4::ZERO // 兼容模式没有父簇常驻边界过渡。
            },
            quality_multipliers: grass_view.quality_multipliers().extend(1.0), // 兼容与屏幕空间视图都应用同一植被质量且不改作者配置。
            lod_pixel_thresholds_ext: if screen_space_enabled {
                grass_view.lod_pixel_thresholds_ext()
            } else {
                [Vec4::ZERO; 2]
            },
            lod_residency_radii_ext: if screen_space_enabled {
                grass_view.lod_residency_radii_ext()
            } else {
                Vec4::ZERO
            },
            lod_residency_transitions_ext: if screen_space_enabled {
                grass_view.lod_residency_transitions_ext()
            } else {
                Vec4::ZERO
            },
            lod_metadata: if screen_space_enabled {
                grass_view.lod_metadata()
            } else {
                Vec4::ZERO
            },
        } // 完成玩法相机剔除数据构造。
    }
}

/// 返回本帧 3D 主通道真实物理高度；没有画质覆盖时保持完整提取视口。
fn main_pass_viewport_height(
    extracted_viewport_height: u32,
    resolution_override: Option<&MainPassResolutionOverride>,
) -> u32 {
    resolution_override.map_or(extracted_viewport_height, |size| size.0.y)
}

/// RenderApp跨帧复用的一份玩法相机视锥uniform缓冲。
#[derive(Resource)]
pub(super) struct GpuBladeViewCulling {
    uniform: UniformBuffer<GpuBladeViewCullingUniform>, // 每帧仅更新304字节，不为同镜头下的全部植被窗口重复上传。
}

impl GpuBladeViewCulling {
    /// 返回准备计算绑定组所需的真实GPU uniform绑定。
    pub(super) fn binding(&self) -> Option<BindingResource<'_>> {
        self.uniform.binding() // 启动系统已经创建缓冲，Option只保留设备初始化失败时的安全出口。
    }
}

/// RenderDevice建立后创建一次共享视锥uniform，首帧相机尚未提取时保持禁用。
pub(super) fn init_gpu_grass_view_culling(
    mut commands: Commands,           // 把持久uniform包装器插入渲染世界。
    render_device: Res<RenderDevice>, // 创建符合当前设备对齐要求的uniform缓冲。
    render_queue: Res<RenderQueue>,   // 写入首帧安全禁用值。
) {
    let mut uniform = UniformBuffer::from(GpuBladeViewCullingUniform::disabled()); // 初始化为绝不误剔除的六平面。
    uniform.set_label(Some("gpu_grass_view_culling_uniform")); // 为RenderDoc与WGPU诊断提供稳定资源名。
    uniform.write_buffer(&render_device, &render_queue); // RenderStartup立即建立真实GPU缓冲供后续绑定组使用。
    commands.insert_resource(GpuBladeViewCulling { uniform }); // 全部草窗共享同一份本帧玩法视锥。
}

/// 每帧把唯一`GpuBladeView`的Bevy视锥上传给草地Compute。
pub(super) fn prepare_gpu_grass_view_culling(
    mut culling: ResMut<GpuBladeViewCulling>, // 原地更新跨帧复用的304字节uniform。
    render_device: Res<RenderDevice>, // `UniformBuffer`首次变化或设备重建时可能重新分配底层缓冲。
    render_queue: Res<RenderQueue>,   // 正常帧只排队一次304字节写入。
    views: Query<(
        &ExtractedView,
        &Frustum,
        &GpuBladeView,
        Option<&MainPassResolutionOverride>,
    )>, // 只接受明确带配置组件的玩法相机，并读取本帧真实主通道像素高度。
) {
    let mut matching_views = views.iter(); // 显式检查唯一性，不能静默选择多个玩法视图中的任意一个。
    let first_view = matching_views.next(); // 读取零个或第一个匹配视图。
    let has_second_view = matching_views.next().is_some(); // 多个目标视图时宁可禁用剔除也不让另一相机看到缺草。
    let uniform = match (first_view, has_second_view) {
        (Some((view, frustum, grass_view, resolution_override)), false) => {
            GpuBladeViewCullingUniform::from_view(view, frustum, grass_view, resolution_override) // 唯一视图同时准备六平面与按真实主通道尺寸计算的投影配置。
        }
        _ => GpuBladeViewCullingUniform::disabled(), // 无视图或配置错误的多视图都回退为完整保留。
    }; // 完成当前帧视锥选择。
    culling.uniform.set(uniform); // 更新CPU侧std140数据而不更换GPU资源句柄。
    culling.uniform.write_buffer(&render_device, &render_queue); // 在Compute RenderGraph节点执行前提交同帧六平面。
}

/// 从标准透视矩阵和物理视口高度计算单位深度下一米对应的像素数。
fn perspective_pixel_scale(clip_from_view: Mat4, viewport_height: u32) -> Option<f32> {
    if viewport_height == 0 {
        return None; // 最小化窗口或目标尚未建立时禁止产生零除或伪造投影LOD。
    } // 完成视口高度快速保护。
    if !clip_from_view.is_finite() || clip_from_view.w_axis.w.abs() > 0.0001 {
        return None; // Bevy标准透视矩阵w轴w为零；正交留给独立分支，其他自定义投影回退兼容模式。
    } // 完成投影类型与有限性检查。
    let vertical_projection_scale = clip_from_view.y_axis.y.abs(); // 标准透视矩阵该项等于一除以二分之一垂直视场角正切。
    let pixel_scale = vertical_projection_scale * viewport_height as f32 * 0.5; // NDC负一到正一对应完整物理视口高度。
    if pixel_scale.is_finite() && pixel_scale > 0.0 {
        Some(pixel_scale) // 返回可直接乘草高并除以视深的正像素比例。
    } else {
        None // 退化投影不得进入屏幕空间LOD或近镜头避让模式。
    } // 完成有限正比例检查。
}

/// 标准正交投影的一米像素数只取决于缩放和真实视口，不依赖为容纳场景而后移的相机。
fn orthographic_pixel_scale(clip_from_view: Mat4, viewport_height: u32) -> Option<f32> {
    if viewport_height == 0 || !clip_from_view.is_finite()
        || (clip_from_view.w_axis.w - 1.0).abs() > 0.0001
        || clip_from_view.x_axis.w.abs() > 0.0001
        || clip_from_view.y_axis.w.abs() > 0.0001
        || clip_from_view.z_axis.w.abs() > 0.0001
    {
        return None; // 未识别的自定义投影继续保守回退，不能把透视参数当作正交比例。
    }
    let pixel_scale = clip_from_view.y_axis.y.abs() * viewport_height as f32 * 0.5;
    (pixel_scale.is_finite() && pixel_scale > 0.0).then_some(pixel_scale)
}

/// 把Bevy无限远平面或任何非有限值转换为不会错误剔除的有限平面。
fn conservative_half_space(normal_d: Vec4) -> Vec4 {
    let normal_length_squared = normal_d.truncate().length_squared(); // 有效Bevy半空间的法线已经归一化且长度约为一。
    if !normal_length_squared.is_finite() || normal_length_squared < 0.25 || !normal_d.w.is_finite()
    {
        Vec4::new(0.0, 0.0, 0.0, INACTIVE_HALF_SPACE_DISTANCE) // 禁用异常单平面而不是禁用整帧草地。
    } else {
        normal_d // 正常左、右、上、下、近、有限远平面保持Bevy原始世界空间契约。
    }
}

#[cfg(test)]
mod tests {
    use super::*; // 验证禁用回退与无限远平面不会产生错误全剔除数据。
    use crate::types::GpuBladeViewLodConfig; // 公共投影LOD测试显式构造纯数值配置。
    use std::mem::{offset_of, size_of}; // ABI测试必须同时验证Rust物理布局与ShaderType声明大小。

    /// 纯逻辑复现后续WGSL的远中近连续权重，提前锁定阈值顺序与总覆盖率契约。
    fn screen_space_lod_weights(projected_height_pixels: f32, thresholds: Vec4) -> Vec3 {
        let far_to_middle = smoothstep(thresholds.x, thresholds.y, projected_height_pixels); // 第一段只在远景与中景之间交叉。
        let middle_to_near = smoothstep(thresholds.z, thresholds.w, projected_height_pixels); // 第二段只在中景与近景之间交叉。
        Vec3::new(
            middle_to_near,                 // x为近景覆盖率。
            far_to_middle - middle_to_near, // y为两段互补后剩余的中景覆盖率。
            1.0 - far_to_middle,            // z为远景覆盖率。
        ) // 两段阈值不重叠时三档始终非负且和为一。
    }

    /// WGSL兼容的有限区间smoothstep，供投影LOD纯逻辑测试使用。
    fn smoothstep(edge_start: f32, edge_end: f32, value: f32) -> f32 {
        let t = ((value - edge_start) / (edge_end - edge_start)).clamp(0.0, 1.0); // 测试配置保证结束阈值严格大于开始阈值。
        t * t * (3.0 - 2.0 * t) // 使用与着色器相同的三次Hermite曲线。
    }

    #[test]
    fn disabled_culling_keeps_all_planes_inactive() {
        let disabled = GpuBladeViewCullingUniform::disabled(); // 构造没有唯一玩法视图时的回退数据。
        assert_eq!(GpuBladeViewCullingUniform::min_size().get(), 304); // 基础区224字节，八档扩展区80字节。
        assert_eq!(size_of::<GpuBladeViewCullingUniform>(), 304); // `repr(C)`物理布局不得额外插入尾部空洞。
        assert_eq!(
            offset_of!(GpuBladeViewCullingUniform, camera_forward_pixel_scale),
            112
        ); // 投影与LOD前三项从偏移112开始，并在偏移176前结束。
        assert_eq!(
            offset_of!(GpuBladeViewCullingUniform, lod_pixel_thresholds),
            128
        ); // 第一组视觉LOD参数占用第九个vec4。
        assert_eq!(
            offset_of!(GpuBladeViewCullingUniform, lod_ultra_pixel_thresholds),
            144
        ); // 第十个vec4保存超远到远级子树的屏幕阈值。
        assert_eq!(
            offset_of!(GpuBladeViewCullingUniform, camera_fade_lod_params),
            160
        ); // 近镜头与母格参数移动到第十一个vec4。
        assert_eq!(
            offset_of!(GpuBladeViewCullingUniform, lod_residency_radii),
            176
        ); // 四级常驻半径参数占用第十二个vec4。
        assert_eq!(
            offset_of!(GpuBladeViewCullingUniform, lod_residency_transition_widths),
            192
        ); // 三段过渡和安全边距占用第十三个vec4。
        assert_eq!(
            offset_of!(GpuBladeViewCullingUniform, quality_multipliers),
            208
        ); // 三个植被画质倍率占用基础区最后一个vec4并结束于224字节。
        assert_eq!(disabled.camera_position_enabled.w, GRASS_VIEW_MODE_DISABLED); // WGSL必须直接跳过视锥与所有相机驱动逻辑。
        assert!(disabled.frustum_planes.iter().all(|plane| plane.w > 0.0)); // 每个回退平面都位于无限远正侧。
        assert_eq!(disabled.camera_forward_pixel_scale, Vec4::ZERO); // 禁用时不泄漏旧前向或投影比例。
        assert_eq!(disabled.lod_pixel_thresholds, Vec4::ZERO); // 禁用时没有可误用的像素阈值。
        assert_eq!(disabled.lod_ultra_pixel_thresholds, Vec4::ZERO); // 禁用时没有超远交接阈值。
        assert_eq!(disabled.camera_fade_lod_params, Vec4::ZERO); // 禁用时不允许产生镜头附近空洞。
        assert_eq!(disabled.lod_residency_radii, Vec4::ZERO); // 禁用时不允许伪造任何常驻子级。
        assert_eq!(disabled.lod_residency_transition_widths, Vec4::ZERO); // 禁用时不允许伪造边界过渡。
        assert_eq!(disabled.quality_multipliers, Vec4::ONE); // 无视图回退不得意外降低或清空植被。
        assert_eq!(offset_of!(GpuBladeViewCullingUniform, lod_metadata), 288);
        assert_eq!(disabled.lod_metadata, Vec4::ZERO);
        assert_eq!(disabled.lod_pixel_thresholds_ext, [Vec4::ZERO; 2]);
    }

    #[test]
    fn infinite_far_plane_becomes_conservative_finite_plane() {
        let plane = conservative_half_space(Vec4::new(0.0, 0.0, 0.0, f32::INFINITY)); // 模拟Bevy无限反向Z的禁用远平面。
        assert!(plane.is_finite()); // GPU uniform中不传播Infinity或NaN。
        assert_eq!(plane.truncate(), Vec3::ZERO); // 零法线使所有世界位置得到同一正距离。
        assert!(plane.w > 0.0); // 球体测试永远不会把候选判到该平面外侧。
    }

    #[test]
    fn perspective_pixel_scale_matches_vertical_fov_and_physical_viewport() {
        let vertical_fov = std::f32::consts::FRAC_PI_4; // 使用四十五度垂直视场便于独立计算期望比例。
        let viewport_height = 1152; // 明确使用物理像素高度，避免窗口缩放后LOD阈值漂移。
        let projection = Mat4::perspective_infinite_reverse_rh(vertical_fov, 16.0 / 9.0, 0.1); // 匹配Bevy常用无限反向Z透视矩阵。
        let actual = perspective_pixel_scale(projection, viewport_height)
            .expect("标准透视投影应产生有限正像素比例");
        let expected = viewport_height as f32 * 0.5 / (vertical_fov * 0.5).tan(); // 一米高物体在一米视深处的理论投影像素。
        assert!((actual - expected).abs() < 0.001); // CPU打包值必须与纯FOV公式一致。
        assert_eq!(perspective_pixel_scale(projection, 0), None); // 零高视口必须禁用屏幕空间路径。
        assert_eq!(
            perspective_pixel_scale(
                Mat4::orthographic_rh(-10.0, 10.0, -10.0, 10.0, 0.1, 1000.0),
                viewport_height,
            ),
            None
        ); // 当前投影公式不适用于正交相机，必须保守回退兼容模式。
    }

    #[test]
    fn render_scale_override_drives_grass_screen_space_lod_pixels() {
        let override_size = MainPassResolutionOverride(UVec2::new(1286, 724)); // 模拟 1080p 的 67% 固定比例。
        assert_eq!(main_pass_viewport_height(1080, Some(&override_size)), 724); // 草地必须按真实低分辨率主通道选择 LOD。
        assert_eq!(main_pass_viewport_height(1080, None), 1080); // 原生路径继续使用完整窗口物理高度。
    }

    #[test]
    fn orthographic_pixels_follow_zoom_and_physical_resolution_without_depth() {
        let projection = Mat4::orthographic_rh(-10.0, 10.0, -4.0, 4.0, 0.1, 133.0);
        assert_eq!(orthographic_pixel_scale(projection, 1000), Some(125.0));
        let wider = Mat4::orthographic_rh(-20.0, 20.0, -8.0, 8.0, 0.1, 10_000.0);
        assert_eq!(orthographic_pixel_scale(wider, 1000), Some(62.5));
        assert_eq!(orthographic_pixel_scale(projection, 500), Some(62.5));
        assert_eq!(orthographic_pixel_scale(projection, 0), None);
        assert_eq!(orthographic_pixel_scale(Mat4::perspective_infinite_reverse_rh(
            std::f32::consts::FRAC_PI_4, 1.0, 0.1), 1000), None);
        let mut invalid = projection;
        invalid.z_axis.w = -0.1;
        assert_eq!(orthographic_pixel_scale(invalid, 1000), None);
    }

    #[test]
    fn screen_space_thresholds_form_continuous_normalized_lod_weights() {
        let legacy = GpuBladeView::legacy(); // 未升级相机必须保持旧行为且不暴露半套参数。
        assert!(!legacy.uses_screen_space_lod()); // 默认兼容配置不会改变当前草窗选择。
        assert_eq!(legacy.lod_pixel_thresholds(), Vec4::ZERO); // 禁用配置没有伪阈值。
        assert_eq!(legacy.camera_fade_lod_params(), Vec4::ZERO); // 禁用配置没有镜头避让。

        let screen_space = GpuBladeView::screen_space_lod_with(GpuBladeViewLodConfig::new(
            Vec2::new(12.0, 20.0),         // 远转中使用正宽像素过渡。
            Vec2::new(40.0, 56.0),         // 中转近与前一段保持有序。
            Vec2::new(0.18, 0.55),         // 近镜头整株避让范围保持正宽。
            0.70,                          // 测试参考叶片高度。
            0.0875,                        // 测试最细世界格距。
            Vec3::new(46.0, 104.0, 220.0), // 三级常驻半径严格递增。
            2.0,                           // 子级外沿安全余量。
            Vec2::new(8.0, 16.0),          // 近中两段稳定过渡宽度。
        )); // 公共测试显式提供纯数值配置，不依赖任何项目场景预设。
        let thresholds = screen_space.lod_pixel_thresholds(); // 四项分别描述远中与中近交叉段。
        let residency = screen_space.lod_residency_radii(); // 近中远窗口与外沿安全边距必须严格有序。
        let residency_transitions = screen_space.lod_residency_transition_widths(); // 近中两个子级必须使用正宽过渡。
        assert!(screen_space.uses_screen_space_lod()); // 构造器必须显式请求新路径。
        assert!(
            thresholds.x < thresholds.y
                && thresholds.y <= thresholds.z
                && thresholds.z < thresholds.w
        ); // 两段必须有正宽度且不得交叠，否则中景权重可能变负。
        assert!(residency.x > 0.0 && residency.y > residency.x && residency.z > residency.y); // 三级兼容配置仍必须保持近中远逐级扩大。
        assert!(
            residency_transitions.x > 0.0
                && residency_transitions.y > residency_transitions.x
                && residency_transitions.y < residency.y
        ); // 中级使用更宽过渡吸收高秆远代理硬切，且任何过渡都不能吞掉完整常驻半径。
        for projected_height in [0.0, 12.0, 16.0, 20.0, 30.0, 40.0, 48.0, 56.0, 96.0] {
            let weights = screen_space_lod_weights(projected_height, thresholds); // 覆盖两端、两段内部和纯中景区间。
            assert!(weights.cmpge(Vec3::ZERO).all()); // 任意档位都不能得到负概率。
            assert!(weights.cmple(Vec3::ONE).all()); // 任意档位都不能超过完整覆盖率。
            assert!((weights.element_sum() - 1.0).abs() < 0.000_001); // 三档逐株概率始终严格互补。
        }
        assert_eq!(
            screen_space.camera_fade_lod_params(),
            Vec4::new(0.18, 0.55, 0.70, 0.0875)
        ); // 锁定整株避让、参考草高与统一母格的视图ABI顺序。
    }
}
