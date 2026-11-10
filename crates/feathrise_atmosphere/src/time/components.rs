// 世界时间组件：声明由玩法场景显式标记的天体方向光与天空盒原始参数。

use bevy::prelude::*; // Component 与 Color 用于声明可查询的场景光照数据。

/// 运行时生成的可见天体图层；只表达画面核心或柔光，不承担实际照明职责。
#[derive(Clone, Copy, Debug, Eq, PartialEq)] // 系统按值去重太阳核心、太阳柔光和月亮，测试直接断言昼夜显隐。
pub(crate) enum CelestialVisualKind {
    Sun,     // 六至十八点沿世界本地日间弧线显示物理角直径的白热核心。
    SunHalo, // 与太阳核心同方向显示大范围渐隐柔光，但不参与光源角直径和阴影。
    Moon,    // 十八至次日六点沿独立月亮轨迹显示。
}

/// 标记动态场景运行时生成的可见太阳核心、太阳柔光或月亮圆盘。
#[derive(Component, Clone, Copy, Debug)] // 三个轻量实体逐帧跟随当前玩法或F12透视相机。
pub(crate) struct CelestialVisual {
    pub(crate) kind: CelestialVisualKind, // 三层共享网格，但使用独立材质、缩放、轨迹和显隐时段。
}

/// 区分固定场景兼容灯、动态场景太阳灯与动态场景月亮灯。
#[derive(Clone, Copy, Debug, Eq, PartialEq)] // 稳定枚举让光照系统和 Solari 历史使用同一身份，不依赖实体遍历顺序。
pub enum WorldCelestialLightKind {
    LegacyCombined, // HN-01/HN-02 继续由一盏灯在日夜间复用，完整保留旧画面。
    Sun,            // 动态场景太阳灯只沿真实日间太阳轨迹照明。
    Moon,           // 动态场景月亮灯只沿真实夜间月亮轨迹照明。
}

/// 标记世界时间插件可以接管的玩法方向光，并保存场景作者配置的日间基准。
#[derive(Component, Clone, Debug, PartialEq)] // 组件随场景实体生命周期存在，动态场景只在解析结果变化时写入。
pub struct WorldCelestialLight {
    /// 当前实体承担的固定兼容、太阳或月亮职责。
    pub(crate) kind: WorldCelestialLightKind, // 显式身份禁止再次把相距近半圈的两个天体方向插值到同一盏灯。
    /// 场景作者配置的日间方向光照度。
    pub base_illuminance: f32, // 昼夜和季节系数始终以该值为基准，避免逐帧累乘漂移。
    /// 场景作者配置的高角度日光颜色。
    pub base_color: Color, // 太阳升高时恢复该颜色，低角度时向暖色过渡。
    /// 固定场景的旧月光峰值，或动态场景独立月亮灯的午夜峰值。
    pub night_peak_illuminance: f32, // 保留字段名兼容既有 JSON，并恢复与月亮高度曲线一致的峰值语义。
    /// 夜间月光的作者基准颜色。
    pub night_color: Color, // 固定场景继续使用旧冷蓝色，动态场景允许独立配置。
    /// 绕世界 Y 轴同时旋转太阳和月亮的偏移弧度。
    pub azimuth_offset_radians: f32, // 只改变水平朝向，不改变天体高度曲线。
    /// 光源角直径弧度，与同实体 `SunDisk` 以及 Solari 历史身份保持一致。
    pub angular_diameter_radians: f32, // 动态作者值直接影响光源范围和软阴影。
    /// 当前场景的日间清屏颜色。
    pub day_clear_color: Color, // 夜晚从该颜色平滑过渡到统一深蓝黑色。
    /// 动态场景使用独立太阳灯与月亮灯，环境通道继续使用连续二十四小时曲线。
    pub(crate) uses_authored_day_night_curve: bool, // 固定 HN 场景保持旧单灯分段曲线，避免迁移改变画面。
}

/// 天空盒模式的可见背景亮度基准；动态模式不会安装或消费。
#[derive(Component, Clone, Copy, Debug)] // 仅包含一个数值，可直接复制用于测试和场景构建。
pub struct WorldSkyProfile {
    /// 天空盒在完整日照下的作者原始亮度。
    pub base_brightness: f32, // 最终光照每帧从该不变量重算昼夜与天气后的可见 Cube 亮度。
}

/// 标记需要随昼夜与天气变化强度的相机IBL补光与反射，并保存两个作者日间基准。
#[derive(Component, Clone, Copy, Debug)] // 独立于天空盒配置，恢复外部相机状态时不会误改原环境贴图强度。
pub(crate) struct WorldEnvironmentMapProfile {
    /// 天空漫反射补光在完整日照下的作者原始强度，单位同样为cd/m²。
    pub(crate) base_diffuse_intensity: f32, // 像素比例已在采用时编码，字段保留完整作者语义和恢复证据。
    /// 天空镜面反射在完整日照下的作者原始强度，单位同样为cd/m²。
    pub(crate) base_specular_intensity: f32, // 正常由Bevy公共Uniform承载，零值时改由补光基准承载。
}
