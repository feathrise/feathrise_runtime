//! Hanabi遮雨正式属性布局、生成WGSL及暂停ECS数据发布回归。
use super::super::{
    WeatherRainEmitter, build_rain_effect, sync_weather_rain, sync_weather_rain_shelter,
}; // 不经测试替身建立生产效果和系统。
use super::*; // 使用正式修饰器和属性桥。
use feathrise_environment::rain_shelter::RainShelterState; // 与运行时完全相同的共享资源。

#[test] // 正式EffectAsset携带所有矩阵并实际安装逐滴修饰器。
fn generated_hanabi_rain_shelter_shader_validates() {
    // 不启动GPU或游戏窗口。
    let effect = build_rain_effect(); // 创建真实生产效果，检查注册链而非手工Module。
    assert_eq!(effect.properties().len(), 196); // 193项遮雨属性、2项水面属性加1项既有风速；矩阵必须拆成四个vec4。
    let properties = effect.property_layout(); // 由Hanabi按真实属性类型计算布局。
    assert_eq!(properties.cpu_size(), 3132); // 195个vec4加最后12字节风速，不计GPU尾部对齐。
    assert_eq!(properties.min_binding_size().get(), 3136); // GPU Properties数组步长必须补齐16字节对齐。
    assert_eq!(properties.align(), 16); // 与WGSL vec4/vec3最大对齐一致。
    let mut serialized = vec![0_u8; properties.cpu_size() as usize]; // 精确复现真实CPU上传目标长度，不能扩大缓冲掩盖布局错误。
    let mut ranges = Vec::new(); // 保存真实每项范围，用于检测重叠而非只检查末项。
    for (offset, property) in properties.properties() {
        // 直接检查EffectAsset.property_layout而非手写替代布局。
        let start = offset as usize; // 与Hanabi写入使用同一字节偏移。
        let end = start
            .checked_add(property.size())
            .expect("属性结束偏移不可溢出"); // 检查所有矩阵列和其余属性。
        assert!(
            end <= serialized.len(),
            "属性{}写入{}..{}超过CPU缓冲{}",
            property.name(),
            start,
            end,
            serialized.len()
        ); // 原Mat4布局会在此明确暴露1564字节越界。
        assert_eq!(
            start % property.value_type().align(),
            0,
            "属性{}未按WGSL类型对齐",
            property.name()
        ); // 同时检查对齐契约。
        assert_eq!(property.default_value().as_bytes().len(), property.size()); // 实际待写入字节必须等于布局宣称类型大小。
        serialized[start..end].copy_from_slice(property.default_value().as_bytes()); // 按真实属性范围进行CPU拷贝，不使用额外容错空间。
        ranges.push((start, end, property.name())); // 保存名称便于失败定位。
    } // 完成196项完整CPU写界限验证。
    ranges.sort_unstable_by_key(|range| range.0); // 排序与Hanabi类型重排独立，不能假设注册顺序就是布局顺序。
    for pair in ranges.windows(2) {
        assert!(
            pair[0].1 <= pair[1].0,
            "属性{}和{}的CPU写入范围重叠",
            pair[0].2,
            pair[1].2
        );
    } // 原Mat4步进16错误同样会造成重叠。
    let particles = effect.particle_layout(); // 使用所有生产修饰器要求的实际粒子布局。
    let textures = effect.texture_layout(); // 使用生产效果纹理布局。
    let mut context = RenderContext::new(&properties, &particles, &textures); // 标准Hanabi代码生成上下文。
    let mut module = effect.module().clone(); // 测试拥有独立编译模块。
    let modifier = RainShelterModifier; // 本用例单独验证遮雨顶点段与完整属性布局；全修饰器真实接线由template_tests覆盖。
    modifier.apply_render(&mut module, &mut context).unwrap(); // 后续水面片元裁切不应改变这里的顶点段测试边界。
    assert!(context.vertex_code.contains("shelter_blocked")); // 错误漏接线或顺序变动会明确失败。
    let source = format!(
        "{}\n@group(0) @binding(0) var<storage, read> properties: array<Properties>;\n{}\nstruct Particle {{ position: vec3<f32>, f32_0: f32 }}\n@vertex fn main() -> @builtin(position) vec4<f32> {{ var particle: Particle; particle.position = vec3<f32>(0.0); var color = vec4<f32>(1.0); var size = vec3<f32>(1.0); {} return vec4<f32>(particle.position + size * color.a, 1.0); }}",
        properties.generate_property_struct_code().unwrap(),
        context.render_extra,
        context.vertex_code
    ); // 用Hanabi真实属性声明和实际modifier代码组成合法顶点入口。
    let parsed = wgpu::naga::front::wgsl::parse_str(&source)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source))); // 解析实际矩阵属性访问及同源slab函数。
    wgpu::naga::valid::Validator::new(
        wgpu::naga::valid::ValidationFlags::all(),
        wgpu::naga::valid::Capabilities::all(),
    )
    .validate(&parsed)
    .unwrap(); // 验证顶点存储读取、矩阵布局与分支类型。
    let (properties_type, declaration) = parsed
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("Properties"))
        .expect("实际Hanabi源码必须生成Properties结构"); // 使用Naga真实计算出的成员布局。
    let wgpu::naga::TypeInner::Struct { members, span } = &declaration.inner else {
        panic!("Properties必须是WGSL结构");
    }; // 不接受伪造同名标量。
    assert_eq!(*span, 3136); // WGSL结构尾部对齐决定真正GPU步长。
    assert_eq!(members.len(), 196); // 不允许源码偷偷遗漏CPU中的任一属性。
    for (offset, property) in properties.properties() {
        // 按名字逐项对照，避免Hanabi排序后偶然对上索引。
        let member = members
            .iter()
            .find(|member| member.name.as_deref() == Some(property.name()))
            .expect("每项CPU属性必须存在对应WGSL成员"); // 对照实际生成结构。
        assert_eq!(
            member.offset,
            offset,
            "CPU/GPU属性{}偏移不一致",
            property.name()
        ); // 发现CPU矩阵步进16但GPU实际64的原始根因。
        let wgpu::naga::TypeInner::Vector { size, scalar } = &parsed.types[member.ty].inner else {
            panic!("遮雨和风属性只能使用小向量，不能重新引入Mat4");
        }; // 允许vec3风速和vec4遮雨数据。
        assert_eq!(
            *size as usize * scalar.width as usize,
            property.size(),
            "属性{}CPU/GPU大小不一致",
            property.name()
        ); // 字节尺寸逐项严格相同。
    } // 完成全部成员GPU偏移与类型大小对照。
    let stride = parsed
        .types
        .iter()
        .find_map(|(_, ty)| match &ty.inner {
            wgpu::naga::TypeInner::Array { base, stride, .. } if *base == properties_type => {
                Some(*stride)
            }
            _ => None,
        })
        .expect("存储绑定必须使用真实Properties数组"); // 检查实际声明的数组stride而非只看CPU向上取整。
    assert_eq!(stride as u64, properties.min_binding_size().get()); // GPU读取第二个效果实例时也不能错位。
} // 完成生产着色器自动验证。

#[test] // 动态建筑更新必须在零模拟时间里改变存活雨丝，稳定帧不重传。
fn paused_rain_publishes_changed_shelter_and_clears_removed_resource() {
    // 验证真实sync_weather_rain链。
    let mut app = App::new(); // 无渲染最小ECS应用。
    app.insert_resource(Assets::<EffectAsset>::default()); // 允许真实生产雨效果资产创建。
    app.insert_resource(crate::WeatherOutput {
        precipitation: 1.0,
        ..default()
    }); // 连续大雨保持唯一雨实体。
    let mut time = Time::<Virtual>::default(); // 零时间模拟暂停。
    time.pause(); // 遮挡变更不能依赖模拟delta。
    app.insert_resource(time); // 发布暂停时钟。
    app.insert_resource(RainShelterState::default()); // 首帧没有建筑代理。
    app.add_systems(Update, sync_weather_rain); // 运行真实发布系统。
    app.add_systems(PostUpdate, sync_weather_rain_shelter); // 同帧后置发布与正式调度一致。
    app.world_mut()
        .spawn((feathrise_scene_core::view::PrimarySceneView, Transform::default())); // 唯一玩法相机。
    app.update(); // 创建首帧属性完整的雨实体。
    let emitter = app
        .world_mut()
        .query_filtered::<Entity, With<WeatherRainEmitter>>()
        .single(app.world())
        .unwrap(); // 记录实体身份确保没有重建整幕雨。
    app.world_mut()
        .resource_mut::<RainShelterState>()
        .gpu
        .header = Vec4::new(1.0, 8.0, 0.0, 0.0); // 模拟建筑新增后的完整共享快照。
    app.world_mut()
        .resource_mut::<RainShelterState>()
        .gpu
        .shelters[0]
        .world_bounds = Vec4::new(-3.0, -4.0, 3.0, 4.0); // 世界水平包围盒必须与同批矩阵一起交给雨丝早退。
    app.update(); // 暂停帧仍应发布属性。
    assert_eq!(
        app.world()
            .get::<EffectProperties>(emitter)
            .unwrap()
            .get_stored(HEADER),
        Some(Vec4::new(1.0, 8.0, 0.0, 0.0).into())
    ); // 存活同一实体收到新遮雨数量。
    assert_eq!(
        app.world()
            .get::<EffectProperties>(emitter)
            .unwrap()
            .get_stored("weather_shelter_bounds_0"),
        Some(Vec4::new(-3.0, -4.0, 3.0, 4.0).into())
    ); // 实际ECS链完整转交minX/minZ/maxX/maxZ。
    app.world_mut().clear_trackers(); // 隔离后续稳定帧写入。
    app.update(); // 没有建筑或风变化。
    assert!(
        !app.world()
            .entity(emitter)
            .get_ref::<EffectProperties>()
            .unwrap()
            .is_changed()
    ); // 稳定帧不重新提取193项遮雨属性。
    app.world_mut().remove_resource::<RainShelterState>(); // 场景清理或独立插件卸除不留旧遮挡。
    app.update(); // 缺失共享资源时采用完整空快照。
    assert_eq!(
        app.world()
            .get::<EffectProperties>(emitter)
            .unwrap()
            .get_stored(HEADER),
        Some(Vec4::ZERO.into())
    ); // 原雨实体清除旧建筑遮挡。
} // 完成暂停、稳定与清理回归。

#[test] // 非单位矩阵必须保持列主序，不能仅用单位矩阵掩盖列交换或平移丢失。
fn published_shelter_matrix_columns_reconstruct_rotated_scaled_transform() {
    // 正式属性桥与真实EffectAsset布局一起验证。
    let effect = build_rain_effect(); // 使用真实196项属性声明。
    let layout = effect.property_layout(); // 与运行时序列化一致。
    let transform = Transform::from_xyz(7.0, 5.0, -3.0)
        .with_rotation(Quat::from_euler(EulerRot::XYZ, 0.31, 0.62, -0.18))
        .with_scale(Vec3::new(2.0, 0.75, 1.25)); // 同时包含非均匀缩放、三轴旋转和平移。
    let source = feathrise_environment::rain_shelter::RainShelterBox {
        center: Vec3::new(0.3, -0.2, 0.8),
        half_extents: Vec3::new(3.0, 0.2, 2.0),
    }; // 非原点局部盒继续测试作者中心偏移。
    let volume = feathrise_environment::rain_shelter::RainShelterVolume::from_box(
        source,
        &GlobalTransform::from(transform),
    )
    .unwrap(); // 使用生产CPU逆矩阵构造。
    let mut gpu = RainShelterGpu::default(); // 单体完整快照。
    gpu.header = Vec4::new(1.0, volume.half_extents.w, 0.0, 0.0); // 真实体数和最高点。
    gpu.shelters[0] = volume; // 交给实际属性发布函数。
    let mut published = EffectProperties::default(); // 与生产新雨实体使用相同属性容器。
    set_properties(&mut published, &gpu); // 不复制或重写生产矩阵拆列逻辑。
    let mut bytes = vec![0_u8; layout.cpu_size() as usize]; // 上传工作区保持严格3132字节。
    for (offset, property) in layout.properties() {
        // 按真实布局拷贝发布值，风速缺覆盖时用正式默认值。
        let value = published
            .get_stored(property.name())
            .unwrap_or(*property.default_value()); // CPU写值来源与Hanabi实际语义一致。
        let start = offset as usize; // 正式布局偏移。
        bytes[start..start + property.size()].copy_from_slice(value.as_bytes()); // 重现导致用户崩溃的实际切片写入边界。
    } // 完成完整属性包验证。
    let columns: [[f32; 4]; 4] = std::array::from_fn(|column| {
        // 从实际上传字节恢复每一列，不能直接拿原矩阵证明自己。
        let name = format!("weather_shelter_matrix_0_col_{column}"); // 与GPU源码访问一致的正式列名。
        let offset = layout
            .properties()
            .find_map(|(offset, property)| (property.name() == name).then_some(offset))
            .expect("生产布局必须注册四列") as usize; // 通过公开迭代器定位Hanabi重排后真实偏移，不调用crate私有offset函数。
        let expected = Vec4::from_array(volume.world_to_local.to_cols_array_2d()[column]); // 仅用于独立逐列发布断言。
        assert_eq!(published.get_stored(&name), Some(expected.into())); // 检查实际set_properties没有转置或漏列。
        std::array::from_fn(|row| {
            f32::from_ne_bytes(
                bytes[offset + row * 4..offset + row * 4 + 4]
                    .try_into()
                    .unwrap(),
            )
        }) // 从CPU包解码四个浮点分量。
    }); // 完成列主序读取。
    let restored = Mat4::from_cols_array_2d(&columns); // 等价于shader mat4x4(col0,col1,col2,col3)构造。
    assert!(restored.abs_diff_eq(volume.world_to_local, 0.000_001)); // 完整逆变换必须保持原位值。
    let local_point = Vec3::new(1.1, -0.1, -0.4); // 任意非对称盒内点。
    let world_point = transform
        .to_matrix()
        .transform_point3(source.center + local_point); // 独立使用正向变换产生世界点。
    assert!(
        restored
            .transform_point3(world_point)
            .abs_diff_eq(local_point, 0.000_01)
    ); // 证明缩放、旋转、中心与平移真实还原。
} // 完成矩阵列发布及CPU上传回归。
