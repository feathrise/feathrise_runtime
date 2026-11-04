//! 正式大地图动态水面的纯波纹值类型与固定 GPU ABI。
//! 【功能设计】CPU 只维护八个统一玩家波源，顶点位移、波谷乘色与动态法线全部由 WGSL 消费固定参数完成。

use bevy::prelude::{Vec2, Vec4}; // 玩家波源使用局部XZ坐标，GPU数据统一按vec4对齐。
use bevy::render::render_resource::ShaderType; // 让材质Uniform严格按WGSL布局编码。

use super::constants::PLAYER_RIPPLE_CAPACITY; // GPU数组长度与CPU固定波源池容量保持唯一口径。

/// 一圈由玩家入水、脚步、起跳或落水触发、会随时间扩散的局部水波。
#[derive(Clone, Copy)] // 小型值类型可安全保存在固定容量波源池中并高效遍历。
pub(super) struct PlayerWaterRipple {
    // 波源只保存局部平面坐标、年龄和初始强度。
    pub(super) center_local_xz: Vec2, // 水面子网格局部XZ坐标，确保波圈跟随GLB变换正确定位。
    pub(super) age: f32,              // 自波纹发射后的秒数，用于计算半径与寿命衰减。
    pub(super) amplitude: f32,        // 当前波源的初始高度，慢走与疾跑可产生不同强度。
} // 结束单个玩家波源定义。

/// 同时返回真实几何高度和用于同一水面顶点颜色的归一化波形信号。
#[cfg(test)] // GPU迁移后该值只保留为Shader参考公式的纯逻辑回归载体。
#[derive(Clone, Copy, Default)] // 小型采样值只在单帧顶点循环中传递，不持有资产或实体引用。
pub(super) struct PlayerRippleSample {
    // 高度控制真实曲面，信号只辅助呈现波谷圆环而不生成额外平面。
    pub(super) height: f32,        // 叠加并限幅后的局部几何波高，单位为米。
    pub(super) visual_signal: f32, // 归一化峰谷信号，负值用于在原生材质上压暗波谷。
} // 结束玩家波纹采样值定义。

/// 单帧水面 GPU 参数固定占用二十个 `vec4`。
#[repr(C)] // Rust物理布局与WGSL连续二十个vec4保持一致。
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)] // 支持材质Uniform编码、历史推进和布局测试。
pub(crate) struct DynamicWaterGpuFrame {
    pub(super) time_and_visual: Vec4, // x为时间，y为波谷压暗，z为岸边波幅指数，w保留。
    pub(super) ripple_shape: Vec4,    // x/y/z/w依次为寿命、速度、环带半宽和波长。
    pub(super) ripple_limits: Vec4, // x/y/z/w依次为建立时间、高度上限、有效波源数和信号归一化振幅。
    pub(super) surface_settings: Vec4, // x为岸边UV宽度，y/z为局部XZ流向或整网格样式哨兵，w为基础流速。
    pub(super) wave_directions_amplitudes: [Vec4; 4], // xy方向、z振幅、w波数，固定四组。
    pub(super) wave_speeds: Vec4,      // 四组波峰米制速度，与水流速度完全分离。
    pub(super) wave_steepness: Vec4,   // 四组水平Gerstner位移比例。
    pub(super) micro_normal_a: Vec4,   // x平铺米、yz滚动速度、w有效强度。
    pub(super) micro_normal_b: Vec4,   // 第二组滚动法线使用相同布局。
    pub(super) ripples: [Vec4; PLAYER_RIPPLE_CAPACITY], // 每槽xy为局部中心、z为年龄、w为初始振幅。
} // 结束单帧GPU水面参数定义。

/// 当前帧和上一帧参数组成固定六百四十字节水面材质Uniform。
#[repr(C)] // 两个二十vec4帧连续排列，供主通道与运动向量预通道分别读取。
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)] // AsBindGroup直接写入材质binding 100。
pub(crate) struct DynamicWaterUniform {
    pub(super) current: DynamicWaterGpuFrame, // 主渲染、阴影、深度和延迟通道读取当前波形。
    pub(super) previous: DynamicWaterGpuFrame, // 运动向量预通道重建上一帧波形位置。
} // 结束完整GPU水面Uniform定义。
