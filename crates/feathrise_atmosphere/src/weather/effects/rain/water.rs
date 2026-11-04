//! 湖面雨丝裁切：读取局部真实水面高度图，按片元世界位置截断长雨丝。
use super::WeatherRainEmitter;
use bevy::prelude::*; // 属性发布、图像句柄与单发射体查询。
use bevy_hanabi::*; // 只使用Hanabi正式片元材质纹理接口。
use feathrise_environment::rain_water::{RAIN_WATER_SPAN, RainWaterMask}; // 不直接依赖场景，窗口跨度与生产端唯一同源。 // 只更新唯一天气雨幕，不影响技能粒子。

pub(super) const BOUNDS: &str = "rain_water_bounds"; // 世界XZ范围，避免把图边缘延伸成无限水面。
pub(super) const STATE: &str = "rain_water_state"; // X启用，其余分量保留。

pub(super) fn register_properties(writer: &ExprWriter) {
    // 与既有遮雨属性使用同一安全Vec4布局。
    writer.add_property(BOUNDS, Vec4::ZERO.into()); // 未准备高度图时范围为空。
    writer.add_property(STATE, Vec4::ZERO.into()); // 没有场景生产者时雨幕保持正常。
} // 完成两个固定属性注册。

fn property_values(mask: &RainWaterMask) -> (Vec4, Vec4) {
    // 先纯读取目标值，稳定帧不能借用Mut触发变化标记。
    let valid = mask.active
        && mask.bounds.is_finite()
        && mask.bounds.x.abs() < 1e19
        && mask.bounds.y.abs() < 1e19
        && mask.bounds.z == RAIN_WATER_SPAN
        && mask.bounds.w == RAIN_WATER_SPAN; // 固定跨度与片元契约一致，极端原点不得占用禁用哨兵范围。
    let state = Vec4::new(if valid { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0); // 同帧完整资格。
    let bounds = if valid { mask.bounds } else { Vec4::ZERO }; // 非有限数不上传GPU。
    (bounds, state) // 以固定两个Vec4返回完整规范化快照。
} // 完成安全属性值计算。

pub(super) fn set_properties(properties: &mut EffectProperties, mask: &RainWaterMask) {
    // 新建及暂停帧共用相同发布规则。
    let (bounds, state) = property_values(mask); // 首帧也不能上传非法范围。
    if properties.get_stored(BOUNDS) != Some(bounds.into()) {
        properties.set(BOUNDS, bounds.into());
    } // 稳定相机范围不重复触发Hanabi属性提取。
    if properties.get_stored(STATE) != Some(state.into()) {
        properties.set(STATE, state.into());
    } // 停雨/切图/资格失效立即停止读旧图。
} // 完成有界属性同步。

pub(crate) fn sync_weather_rain_water(
    mask: Option<Res<RainWaterMask>>,
    mut emitters: Query<(&mut EffectProperties, &mut EffectMaterial), With<WeatherRainEmitter>>,
) {
    // 安排在RainWaterMaskUpdate之后，模拟暂停也执行。
    let fallback = RainWaterMask::default(); // 独立天气宿主没有地图时安全禁用裁切。
    let mask = mask.as_deref().unwrap_or(&fallback); // 同一完成快照控制范围和图像身份。
    let (bounds, state) = property_values(mask); // 唯一快照只规范化一次。
    for (mut properties, mut material) in &mut emitters {
        // 正式天气始终只有一个发射体。
        if properties.get_stored(BOUNDS) != Some(bounds.into())
            || properties.get_stored(STATE) != Some(state.into())
        {
            set_properties(&mut properties, mask);
        } // 变化判断先于可变解引用，稳定帧零属性重传。
        if material.images.len() == 2 && material.images[0] != mask.image {
            material.images[0] = mask.image.clone(); // 只改槽0，不能覆盖槽1自动屋顶纹理。
        } // 图像身份改变才标记材质，两个消费系统互不覆盖。
    } // 完成单发射体发布。
} // 结束独立水面数据消费。

#[derive(Clone, Copy, Debug, Reflect)] // 修饰器无独立运行时资源。
pub(super) struct RainWaterModifier; // 纹理只在fragment采样，符合Hanabi材质绑定可见性。

impl Modifier for RainWaterModifier {
    // 正式RenderModifier接口。
    fn context(&self) -> ModifierContext {
        ModifierContext::Render
    } // 不为每滴雨执行CPU碰撞。
    fn as_render(&self) -> Option<&dyn RenderModifier> {
        Some(self)
    } // 登记片元代码生成。
    fn as_render_mut(&mut self) -> Option<&mut dyn RenderModifier> {
        Some(self)
    } // 保持效果编辑接口。
    fn into_boxed_render(self: Box<Self>) -> Option<Box<dyn RenderModifier>> {
        Some(self)
    } // 标准trait转换。
    fn attributes(&self) -> &[Attribute] {
        &[Attribute::F32_0]
    } // 沿用已修复的真实属性行号桥。
    fn boxed_clone(&self) -> BoxedModifier {
        Box::new(*self)
    } // 缓存效果廉价复制。
    fn apply(&self, _module: &mut Module, context: &mut ShaderWriter) -> Result<(), ExprError> {
        Err(ExprError::InvalidModifierContext(
            context.modifier_context(),
            ModifierContext::Render,
        ))
    } // 禁止向模拟阶段注入材质纹理读取。
} // 完成通用修饰器接口。

impl RenderModifier for RainWaterModifier {
    // 水面仅遮挡水下雨丝，不改主水面透明度。
    fn apply_render(
        &self,
        _module: &mut Module,
        context: &mut RenderContext,
    ) -> Result<(), ExprError> {
        // Hanabi正式模板负责传递片元所属粒子。
        context.set_needs_uv(); // 雨丝没有贴图UV用途，借正式vec2插值接口传递同一窗口原点，不访问片元不可见的属性缓冲。
        context.vertex_code.push_str("let rain_water_property_index = bitcast<u32>(particle.f32_0);\nlet rain_water_state = properties[rain_water_property_index].rain_water_state;\nout.uv = select(vec2<f32>(1e20), properties[rain_water_property_index].rain_water_bounds.xy, rain_water_state.x > 0.5);\n"); // 所有顶点同值，禁用哨兵也同帧发布，暂停无需模拟写回粒子。
        context.fragment_code.push_str(
            &include_str!("water/fragment.wgsl")
                .replace("RAIN_WATER_SPAN", &format!("{RAIN_WATER_SPAN:.1}")),
        ); // 常量来自生产端同一契约，实际片元不再读取group2属性。
        Ok(()) // 所需唯一纹理由EffectMaterial提供。
    } // 完成片元裁切安装。
    fn boxed_render_clone(&self) -> Box<dyn RenderModifier> {
        Box::new(*self)
    } // 无额外所有权。
    fn as_modifier(&self) -> &dyn Modifier {
        self
    } // 标准trait向上转换。
} // 完成渲染修饰器接口。

#[cfg(test)] // 无窗口验证范围、绑定、暂停与真实模板。
mod tests; // 针对水面裁切的独立回归。
