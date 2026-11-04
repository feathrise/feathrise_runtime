//! 逐雨丝渲染遮挡：固定193项向量属性与地表同源盒射线函数，暂停也能随建筑变动。
use bevy::prelude::*; // 反射、矩阵与向量属性值。
use bevy_hanabi::*; // 自定义渲染修饰器和效果属性接口。
use feathrise_environment::rain_shelter::{RAIN_SHELTER_RAY_WGSL, RainShelterGpu}; // 唯一完整遮雨快照与纯GPU几何算法。

const HEADER: &str = "weather_shelter_header"; // 数量和全体最高世界Y。
const CAPACITY: usize = 32; // 与共享GPU固定数组容量一致，着色器不会无界检查。

pub(super) fn register_properties(writer: &ExprWriter) {
    // 仅构建一次Hanabi效果时注册完整属性布局。
    writer.add_property(HEADER, Vec4::ZERO.into()); // 空场景默认没有遮挡。
    for index in 0..CAPACITY {
        // 固定32个矩阵、半径和世界水平范围组。
        for (column, value) in Mat4::IDENTITY.to_cols_array_2d().into_iter().enumerate() {
            // Hanabi0.19属性布局仅按16字节步进，64字节矩阵必须拆为四列。
            writer.add_property(
                format!("weather_shelter_matrix_{index}_col_{column}"),
                Vec4::from_array(value).into(),
            ); // 每列独立占16字节，避免上传越界和属性重叠。
        } // WGSL再按同一列顺序恢复矩阵，不改变遮雨几何。
        writer.add_property(format!("weather_shelter_extent_{index}"), Vec4::ZERO.into()); // XYZ局部半径，W世界最高点。
        writer.add_property(format!("weather_shelter_bounds_{index}"), Vec4::ZERO.into()); // 世界minX/minZ/maxX/maxZ让屋外雨丝在矩阵前快速退出。
    } // 完成固定193项布局，总遮雨有效载荷仍为3088字节。
} // 结束效果属性注册。

pub(super) fn set_properties(properties: &mut EffectProperties, gpu: &RainShelterGpu) {
    // 调用方仅在快照变化或新建实体时执行。
    properties.set(HEADER, gpu.header.into()); // 先复制同一完成快照的头部。
    for (index, volume) in gpu.shelters.iter().enumerate() {
        // 每批固定32组，无几何搜索或粒子CPU循环。
        for (column, value) in volume
            .world_to_local
            .to_cols_array_2d()
            .into_iter()
            .enumerate()
        {
            // 与注册时保持相同列主序，支持任意旋转缩放和平移。
            properties.set(
                &format!("weather_shelter_matrix_{index}_col_{column}"),
                Vec4::from_array(value).into(),
            ); // 只上传Hanabi支持的16字节向量属性。
        } // 四列来自同一完整遮雨快照。
        properties.set(
            &format!("weather_shelter_extent_{index}"),
            volume.half_extents.into(),
        ); // 上传同源体积与最高点快路径。
        properties.set(
            &format!("weather_shelter_bounds_{index}"),
            volume.world_bounds.into(),
        ); // 共享已完成水平包围盒，不在雨系统重算角点。
    } // Hanabi随后整体提取该属性组件，不向GPU暴露半批结果。
} // 结束有界变化发布。

#[derive(Clone, Copy, Debug, Reflect)] // 粒子携带实际属性行号，避免渲染入口依赖不存在的模拟变量。
pub(super) struct RainPropertyIndexModifier; // 只占一个通用32位粒子槽，不保存会过期的遮雨快照。

impl Modifier for RainPropertyIndexModifier {
    // Init保证暂停新生雨滴也有索引，Update刷新当前分配身份。
    fn context(&self) -> ModifierContext {
        ModifierContext::Init | ModifierContext::Update
    } // 两个模拟模板均提供真实properties_array_index。
    fn attributes(&self) -> &[Attribute] {
        &[Attribute::F32_0]
    } // 雨效果独占此槽，按位保存u32不做浮点数值转换。
    fn boxed_clone(&self) -> BoxedModifier {
        Box::new(*self)
    } // 无独立CPU/GPU资源。
    fn apply(&self, _module: &mut Module, context: &mut ShaderWriter) -> Result<(), ExprError> {
        // 当前模拟效果已解析属性行号。
        if !self.context().contains(context.modifier_context()) {
            return Err(ExprError::InvalidModifierContext(
                context.modifier_context(),
                self.context(),
            ));
        } // 不向渲染入口错误注入模拟变量。
        context
            .main_code
            .push_str("particle.f32_0 = bitcast<f32>(properties_array_index);\n"); // 保留全部32位身份，不能假设缓存属性永远位于第0行。
        Ok(()) // 粒子写回由Hanabi正常模拟链完成。
    } // 完成当前属性索引记录。
} // 结束跨模拟/渲染阶段的属性身份桥。

#[derive(Clone, Copy, Debug, Reflect)] // 无运行态的小型修饰器可随缓存效果克隆。
pub(super) struct RainShelterModifier; // 只在渲染阶段隐藏受遮挡雨丝，不修改粒子寿命。

impl Modifier for RainShelterModifier {
    // 显式实现公开trait，避免依赖Hanabi私有宏。
    fn context(&self) -> ModifierContext {
        ModifierContext::Render
    } // 本功能每帧渲染生效，暂停仍可刷新。
    fn as_render(&self) -> Option<&dyn RenderModifier> {
        Some(self)
    } // 允许效果资产收集渲染修饰器。
    fn as_render_mut(&mut self) -> Option<&mut dyn RenderModifier> {
        Some(self)
    } // 支持Hanabi资产编辑接口。
    fn into_boxed_render(self: Box<Self>) -> Option<Box<dyn RenderModifier>> {
        Some(self)
    } // 保持渲染trait对象转换。
    fn attributes(&self) -> &[Attribute] {
        &[Attribute::POSITION, Attribute::F32_0]
    } // 读取粒子位置和模拟阶段保存的真实属性行号。
    fn boxed_clone(&self) -> BoxedModifier {
        Box::new(*self)
    } // 修饰器没有独立缓冲或资源。
    fn apply(&self, _module: &mut Module, context: &mut ShaderWriter) -> Result<(), ExprError> {
        // 错误应用到模拟阶段应明确失败。
        Err(ExprError::InvalidModifierContext(
            context.modifier_context(),
            ModifierContext::Render,
        )) // 防止暂停零delta屏蔽遮雨更新。
    } // 结束模拟上下文拒绝。
} // 结束通用修饰器接口。

impl RenderModifier for RainShelterModifier {
    // 在颜色、大小和朝向修饰器之后调用。
    fn apply_render(
        &self,
        _module: &mut Module,
        context: &mut RenderContext,
    ) -> Result<(), ExprError> {
        // 注入正式雨丝顶点阶段。
        context.render_extra.push_str(RAIN_SHELTER_RAY_WGSL); // 与地表、水花同源的纯盒射线公式。
        context.vertex_code.push_str(&vertex_code()); // 属性数组身份使用Hanabi既有instance下标。
        Ok(()) // 无新纹理或绑定需要渲染器另外准备。
    } // 结束渲染代码生成。
    fn boxed_render_clone(&self) -> Box<dyn RenderModifier> {
        Box::new(*self)
    } // 效果缓存可以廉价复制修饰器。
    fn as_modifier(&self) -> &dyn Modifier {
        self
    } // 向上转成通用修饰器接口。
} // 结束渲染修饰器接口。

fn vertex_code() -> String {
    // 只在Hanabi编译效果时构造固定上限WGSL代码。
    let mut code = format!(
        "let rain_property_index = bitcast<u32>(particle.f32_0);\nlet shelter_header = properties[rain_property_index].{HEADER};\nvar shelter_blocked = false;\nif shelter_header.x > 0.0 && particle.position.y + 0.03 < shelter_header.y {{\n"
    ); // 渲染模板没有properties_array_index；从当前粒子身份访问同帧属性，暂停仍读取新遮雨数据。
    for index in 0..CAPACITY {
        // 展开属性访问以兼容Hanabi独立命名布局，无临时大型矩阵数组。
        let matrix = format!(
            "mat4x4<f32>(properties[rain_property_index].weather_shelter_matrix_{index}_col_0, properties[rain_property_index].weather_shelter_matrix_{index}_col_1, properties[rain_property_index].weather_shelter_matrix_{index}_col_2, properties[rain_property_index].weather_shelter_matrix_{index}_col_3)"
        ); // 四个16字节属性在GPU按列还原原矩阵，绕过Hanabi矩阵属性布局缺陷。
        code.push_str(&format!("if !shelter_blocked && shelter_header.x > {index}.0 {{ shelter_blocked = rain_shelter_volume_blocks(particle.position, {matrix}, properties[rain_property_index].weather_shelter_extent_{index}, properties[rain_property_index].weather_shelter_bounds_{index}); }}\n")); // 屋外先用XZ和Y范围退出，真实候选才做矩阵；首次命中后其余体不再求slab。
    } // 完成固定32体判定。
    code.push_str("}\nif shelter_blocked { color.a = 0.0; size = vec3<f32>(0.0); }\n"); // 仅隐藏该雨丝，屋外背景继续绘制且暂停无需kill模拟。
    code // 返回注入Hanabi现有顶点main的完整语句。
} // 结束逐雨丝代码构造。

#[cfg(test)] // 完整Hanabi模板必须额外验证跨模拟/渲染的属性身份。
mod template_tests;
#[cfg(test)] // 无GPU测试属性类型与实际渲染代码的Naga合法性。
mod tests; // 生产效果构造与遮挡数据链回归独立保存。 // 不伪造渲染阶段缺失的properties_array_index。
