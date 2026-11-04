//! 水面遮雨消费者回归：真实雨实体的纹理身份、暂停发布、无效范围与稳定帧预算。
use super::super::sync_weather_rain;
use super::*; // 直接验证正式属性桥与同步系统。 // 首帧材质必须由正式雨实体构造提供。

#[test] // 非法窗口不能把旧湖面高度扩散到岸上。
fn invalid_water_ranges_clear_previously_active_properties() {
    // 覆盖失效、空范围和非有限值。
    let valid = RainWaterMask {
        bounds: Vec4::new(-256.0, -128.0, 512.0, 512.0),
        active: true,
        ..default()
    }; // 合法局部窗口。
    let mut cases = vec![RainWaterMask {
        active: false,
        ..valid.clone()
    }]; // 停雨或切图必须禁用。
    cases.push(RainWaterMask {
        bounds: Vec4::new(0.0, 0.0, 256.0, 512.0),
        ..valid.clone()
    }); // 与共享跨度不符必须禁用，不能把水域压缩到错误位置。
    cases.push(RainWaterMask {
        bounds: Vec4::new(1e20, 0.0, 512.0, 512.0),
        ..valid.clone()
    }); // 禁用哨兵范围不能作为有效原点发布。
    for bounds in [
        Vec4::new(0.0, 0.0, 0.0, 512.0),
        Vec4::new(0.0, 0.0, 512.0, -1.0),
        Vec4::new(f32::NAN, 0.0, 512.0, 512.0),
        Vec4::new(0.0, f32::INFINITY, 512.0, 512.0),
    ] {
        // 检查零尺寸、负尺寸和坐标污染。
        cases.push(RainWaterMask {
            bounds,
            ..valid.clone()
        }); // 保持active以单独验证范围校验。
    } // 完成坏快照集合。
    for invalid in cases {
        // 每个案例都必须清除一个此前有效的快照。
        let mut properties = EffectProperties::default(); // 与新雨实体相同的属性容器。
        set_properties(&mut properties, &valid); // 先建立真实有效状态。
        assert_eq!(properties.get_stored(STATE), Some(Vec4::X.into())); // 正常窗口应启用。
        assert_eq!(properties.get_stored(BOUNDS), Some(valid.bounds.into())); // 范围保持世界米制。
        set_properties(&mut properties, &invalid); // 正式消费者执行失效处理。
        assert_eq!(properties.get_stored(STATE), Some(Vec4::ZERO.into())); // 禁止继续读取旧水面。
        assert_eq!(properties.get_stored(BOUNDS), Some(Vec4::ZERO.into())); // NaN与无穷数不进入GPU属性。
    } // 完成全部失效路径。
} // 完成范围资格回归。

#[test] // 暂停不能冻结水面窗口或把稳定帧变成属性重传。
fn paused_water_mask_updates_binding_and_clears_removed_resource() {
    // 覆盖真实发射体创建及后置同步。
    let mut app = App::new(); // 无GPU最小ECS宿主。
    app.insert_resource(Assets::<EffectAsset>::default()); // 使用正式效果资产构造。
    app.insert_resource(crate::WeatherOutput {
        precipitation: 1.0,
        ..default()
    }); // 持续降雨保留同一雨实体。
    let mut time = Time::<Virtual>::default(); // 不依赖真实时钟推进。
    time.pause(); // 明确保持模拟暂停。
    app.insert_resource(time); // 发布暂停时间。
    let initial = RainWaterMask {
        bounds: Vec4::new(-256.0, -256.0, 512.0, 512.0),
        active: true,
        ..default()
    }; // 首帧就存在已发布水面。
    app.insert_resource(initial.clone()); // 首帧构造与后置消费者读取相同快照。
    app.add_systems(Update, sync_weather_rain); // 正式天气雨幕创建系统。
    app.add_systems(PostUpdate, sync_weather_rain_water); // 即使暂停仍发布最新水面。
    app.world_mut()
        .spawn((feathrise_scene_core::view::PrimarySceneView, Transform::default())); // 唯一游戏相机。
    app.update(); // 创建正式雨实体及材质。
    let emitter = app
        .world_mut()
        .query_filtered::<Entity, With<WeatherRainEmitter>>()
        .single(app.world())
        .unwrap(); // 后续必须继续使用同一实体。
    assert_eq!(
        app.world().get::<EffectMaterial>(emitter).unwrap().images,
        vec![
            initial.image,
            super::super::super::rain_occlusion::RainOcclusionMap::default().image
        ]
    ); // 正式构造绑定唯一实际水面图像。
    assert_eq!(
        app.world()
            .get::<EffectProperties>(emitter)
            .unwrap()
            .get_stored(BOUNDS),
        Some(initial.bounds.into())
    ); // 首帧范围完整发布。
    let replacement = bevy::asset::uuid_handle!("37cf50c6-b699-4c73-a131-f51dfca52a93"); // 独立图像身份验证材质确实更新。
    let changed = RainWaterMask {
        image: replacement,
        bounds: Vec4::new(0.0, 64.0, 512.0, 512.0),
        active: true,
    }; // 移动窗口与替换目标必须同批采用。
    app.insert_resource(changed.clone()); // 模拟生产者发布完整新快照。
    app.update(); // 暂停帧消费完成状态。
    assert_eq!(
        app.world().get::<EffectMaterial>(emitter).unwrap().images,
        vec![
            changed.image,
            super::super::super::rain_occlusion::RainOcclusionMap::default().image
        ]
    ); // 未重建实体也更新纹理身份。
    assert_eq!(
        app.world()
            .get::<EffectProperties>(emitter)
            .unwrap()
            .get_stored(BOUNDS),
        Some(changed.bounds.into())
    ); // 相同雨实体收到移动范围。
    assert_eq!(
        app.world()
            .get::<EffectProperties>(emitter)
            .unwrap()
            .get_stored(STATE),
        Some(Vec4::X.into())
    ); // 合法水图保持启用。
    app.world_mut().clear_trackers(); // 隔离稳定帧的变更标记。
    app.update(); // 没有范围、材质或天气变化。
    assert!(
        !app.world()
            .entity(emitter)
            .get_ref::<EffectProperties>()
            .unwrap()
            .is_changed()
    ); // 稳定帧不能重传整包196项属性。
    assert!(
        !app.world()
            .entity(emitter)
            .get_ref::<EffectMaterial>()
            .unwrap()
            .is_changed()
    ); // 稳定帧不能重新提取材质。
    app.world_mut().remove_resource::<RainWaterMask>(); // 场景卸载后不可继续裁切旧湖。
    app.update(); // 缺失共享资源采用正式默认快照。
    assert_eq!(
        app.world()
            .get::<EffectProperties>(emitter)
            .unwrap()
            .get_stored(STATE),
        Some(Vec4::ZERO.into())
    ); // 立即关闭旧水面采样。
    assert_eq!(
        app.world()
            .get::<EffectProperties>(emitter)
            .unwrap()
            .get_stored(BOUNDS),
        Some(Vec4::ZERO.into())
    ); // 清除旧范围。
    assert_eq!(
        app.world().get::<EffectMaterial>(emitter).unwrap().images,
        vec![
            RainWaterMask::default().image,
            super::super::super::rain_occlusion::RainOcclusionMap::default().image
        ]
    ); // 回到固定默认图像身份。
} // 完成暂停、纹理身份、稳定帧与清理回归。
