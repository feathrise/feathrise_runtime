//! 屋顶纹理消费者回归：暂停时切换身份、缺失生产者退回，以及与水面槽互不覆盖。
use super::super::{sync_weather_rain, sync_weather_rain_water}; // 首帧由真实天气发射体构造。
use super::*; // 正式同步系统与共享数据契约。
use feathrise_environment::rain_water::RainWaterMask; // 同时验证既有水面消费。

#[test] // 后置生产者更新不能依赖粒子模拟时间推进。
fn paused_roof_image_changes_preserve_water_and_clear_removed_producer() {
    // 独立无GPU回归。
    let mut app = App::new(); // 最小ECS宿主。
    app.insert_resource(Assets::<EffectAsset>::default()); // 构造真实Hanabi效果。
    app.insert_resource(crate::WeatherOutput {
        precipitation: 1.0,
        ..default()
    }); // 保持单一天气雨实体。
    let mut time = Time::<Virtual>::default(); // 暂停不应冻结材质发布。
    time.pause(); // 明确测试零模拟delta。
    app.insert_resource(time); // 安装暂停时钟。
    app.insert_resource(RainWaterMask::default()); // 保留水面槽生产身份。
    app.insert_resource(RainOcclusionMap::default()); // 首帧没有完整屋顶图。
    app.add_systems(Update, sync_weather_rain); // 正式发射体构造路径。
    app.add_systems(
        PostUpdate,
        (sync_weather_rain_water, sync_weather_rain_occlusion).chain(),
    ); // 两个消费系统依次保留对方的槽。
    app.world_mut()
        .spawn((feathrise_scene_core::view::PrimarySceneView, Transform::default())); // 唯一玩法相机。
    app.update(); // 正式构造双槽材质。
    let emitter = app
        .world_mut()
        .query_filtered::<Entity, With<WeatherRainEmitter>>()
        .single(app.world())
        .unwrap(); // 后续必须保留实体身份。
    let roof_image = bevy::asset::uuid_handle!("a70b405e-f2fd-48e5-bcdc-e48b6d54b516"); // 新地图深度图身份。
    let water_image = bevy::asset::uuid_handle!("97643dfb-cfd8-4d99-81ae-3647ef27303e"); // 同帧变化的独立水面身份。
    app.world_mut().resource_mut::<RainOcclusionMap>().image = roof_image.clone(); // 模拟完整屋顶图发布。
    app.world_mut().resource_mut::<RainWaterMask>().image = water_image.clone(); // 模拟水面同时更新。
    app.update(); // 暂停帧更新两个槽。
    assert_eq!(
        app.world().get::<EffectMaterial>(emitter).unwrap().images,
        vec![water_image.clone(), roof_image]
    ); // 两个独立消费者不能互相覆盖。
    app.world_mut().clear_trackers(); // 隔离下一稳定帧的变更标记。
    app.update(); // 图像身份和天气输入完全稳定。
    assert!(
        !app.world()
            .entity(emitter)
            .get_ref::<EffectMaterial>()
            .unwrap()
            .is_changed()
    ); // 稳定帧禁止重复材质提取。
    app.world_mut().remove_resource::<RainOcclusionMap>(); // 模拟地图卸载或生产者撤销。
    app.update(); // 缺失生产者恢复无覆盖fallback。
    assert_eq!(
        app.world().get::<EffectMaterial>(emitter).unwrap().images,
        vec![water_image, RainOcclusionMap::default().image]
    ); // 屋顶清理不改变水面身份。
} // 完成生命周期、暂停与槽隔离验证。
