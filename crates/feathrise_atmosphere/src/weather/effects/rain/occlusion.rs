//! 自动屋顶遮雨消费者：矩阵与深度从同一纹理读取，逐片元裁切雨丝而不阻塞GPU模拟。
use super::super::rain_occlusion::RainOcclusionMap; // 生产者提供完整原子纹理身份。
use super::WeatherRainEmitter; // 只更新天气自有的唯一发射体。
use bevy::prelude::*; // 资源、图像与ECS查询。
use bevy_hanabi::*; // 正式材质纹理与渲染修饰器接口。

pub(crate) fn sync_weather_rain_occlusion(
    map: Option<Res<RainOcclusionMap>>, // 独立天气宿主允许缺少生产者。
    mut emitters: Query<&mut EffectMaterial, With<WeatherRainEmitter>>, // 不读取或遍历单个雨粒子。
) {
    // 暂停仍执行材质身份发布。
    let fallback = RainOcclusionMap::default(); // 缺失生产者立即回到无覆盖纹理。
    let map = map.as_deref().unwrap_or(&fallback); // 完整图像包含其自身投影元数据。
    for mut material in &mut emitters {
        // 正式运行只有一个天气发射体。
        if material.images.len() == 2 && material.images[1] != map.image {
            // 稳定帧不标记材质变化。
            material.images[1] = map.image.clone(); // 只替换屋顶槽，保持水面槽身份。
        } // 首帧两个槽由正式发射体构造一次性登记。
    } // 完成固定大小身份同步。
} // 不将矩阵作为独立属性发布，避免与深度纹理跨帧错配。

#[derive(Clone, Copy, Debug, Reflect)] // 修饰器没有独立GPU资源。
pub(super) struct RainOcclusionModifier; // 自动几何与原有作者遮雨代理并存。

impl Modifier for RainOcclusionModifier {
    // 只在渲染阶段读取片元可见材质。
    fn context(&self) -> ModifierContext {
        ModifierContext::Render
    } // 不修改模拟寿命。
    fn as_render(&self) -> Option<&dyn RenderModifier> {
        Some(self)
    } // 标准渲染接口。
    fn as_render_mut(&mut self) -> Option<&mut dyn RenderModifier> {
        Some(self)
    } // 标准可变接口。
    fn into_boxed_render(self: Box<Self>) -> Option<Box<dyn RenderModifier>> {
        Some(self)
    } // 标准对象转换。
    fn attributes(&self) -> std::borrow::Cow<'_, [Attribute]> {
        std::borrow::Cow::Borrowed(&[])
    } // 世界位置由片元深度恢复，不读取仅顶点可见粒子缓冲。
    fn boxed_clone(&self) -> BoxedModifier {
        Box::new(*self)
    } // 缓存效果克隆。
    fn apply(&self, _module: &mut Module, context: &mut ShaderWriter) -> Result<(), ExprError> {
        // 禁止模拟阶段误用纹理。
        Err(ExprError::InvalidModifierContext(
            context.modifier_context(),
            ModifierContext::Render,
            "rain_occlusion",
        )) // 明确拒绝错误阶段。
    } // 完成上下文验证。
} // 完成正式修饰器接口。

impl RenderModifier for RainOcclusionModifier {
    // 片元裁切保留屋顶上方的雨丝部分。
    fn apply_render(
        &self,
        _module: &mut Module,
        context: &mut RenderContext,
    ) -> Result<(), ExprError> {
        // 安装正式模板代码。
        context
            .render_extra
            .push_str(include_str!("occlusion/decode.wgsl")); // 生产端同源位编码解码，无半精度深度误差。
        context
            .fragment_code
            .push_str(&include_str!("occlusion/fragment.wgsl").replace(
                "256.0",
                &format!("{:.1}", super::super::rain_occlusion::RAIN_OCCLUSION_DEPTH),
            )); // 偏移由生产端唯一深度跨度生成。
        Ok(()) // 材质槽由EffectMaterial提供。
    } // 完成片元代码安装。
    fn boxed_render_clone(&self) -> Box<dyn RenderModifier> {
        Box::new(*self)
    } // 廉价克隆。
    fn as_modifier(&self) -> &dyn Modifier {
        self
    } // 标准向上转换。
} // 完成渲染修饰器实现。

#[cfg(test)] // 无窗口回归身份、暂停与原子纹理契约。
mod tests; // 完整Hanabi模板另由shelter/template_tests验证。
