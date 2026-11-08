//! 遮雨回归覆盖无碰撞空间证据、非均匀父变换以及正式GPU上传空闲行为。

use super::*;
use bevy::prelude::*; // 无窗口App和数学输入。
use bevy::render::storage::ShaderBuffer; // 正式编码后的字节包。 // 使用生产接口而不是测试复制品。

fn volume(transform: Transform) -> RainShelterVolume {
    // 小测试屋檐代理。
    RainShelterVolume::from_box(
        RainShelterBox {
            center: Vec3::ZERO,
            half_extents: Vec3::new(3.0, 0.2, 2.0),
        },
        &GlobalTransform::from(transform),
    )
    .unwrap() // 无Mesh与Collider依赖。
} // 完成夹具。

#[test] // 屋顶上面仍受雨，屋檐正下方被挡。
fn box_raycast_obeys_above_below_side_and_height_limit() {
    // 同一个盒覆盖空间边界。
    let roof = volume(Transform::from_xyz(0.0, 4.0, 0.0)); // 四米高屋顶。
    assert!(roof.blocks_rain(Vec3::ZERO)); // 室内下方遮雨。
    assert!(!roof.blocks_rain(Vec3::new(0.0, 4.3, 0.0))); // 屋顶上方受雨。
    assert!(!roof.blocks_rain(Vec3::new(3.1, 0.0, 0.0))); // 檐外不能被粗包围判定误遮。
    assert!(!roof.blocks_rain(Vec3::new(0.0, -130.0, 0.0))); // 有限世界射线不无限遮雨。
    assert!(!roof.blocks_rain(Vec3::NAN)); // 非法候选不进入矩阵计算。
} // 完成基本空间回归。

#[test] // 旋转和非均匀缩放仍使用准确OBB而非世界AABB。
fn transformed_roof_does_not_cover_aabb_empty_corner() {
    // 构造对角窄桥验证空角。
    let source = RainShelterBox {
        center: Vec3::ZERO,
        half_extents: Vec3::new(4.0, 0.1, 0.25),
    }; // 长条桥面。
    let transform = GlobalTransform::from(
        Transform::from_xyz(0.0, 5.0, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4))
            .with_scale(Vec3::new(2.0, 1.0, 1.0)),
    ); // 父级旋转缩放组合。
    let bridge = RainShelterVolume::from_box(source, &transform).unwrap(); // 生产逆矩阵构造。
    assert!(bridge.blocks_rain(Vec3::new(2.0, 0.0, -2.0))); // 真正桥下被遮挡。
    assert!(!bridge.blocks_rain(Vec3::new(2.0, 0.0, 2.0))); // 世界包围盒空角仍下雨。
    assert!(
        RainShelterVolume::from_box(
            source,
            &GlobalTransform::from(Transform::from_scale(Vec3::ZERO))
        )
        .is_none()
    ); // 退化变换不可发布。
} // 完成变换回归。

#[test] // 实际缓冲编码验证固定ABI与稳定帧无重复上传。
fn fixed_buffer_uploads_complete_batch_only_on_change() {
    // 不建立渲染设备或窗口。
    assert_eq!(std::mem::size_of::<RainShelterVolume>(), 96); // 单体与WGSL一致。
    assert_eq!(std::mem::size_of::<RainShelterGpu>(), 3088); // 头16字节加32体。
    let mut app = App::new(); // 最小生产主世界。
    systems::install_rain_shelter(&mut app); // 真正安装固定UUID缓冲。
    app.update(); // 消费初始资源变更。
    <bevy::render::storage::GpuShaderBuffer as bevy::render::render_asset::RenderAsset>::take_gpu_data(
        &mut app.world_mut()
        .resource_mut::<Assets<ShaderBuffer>>()
        .get_mut(rain_shelter_buffer_handle().id())
        .unwrap(),
        None,
    ).unwrap();
    app.update(); // 暂停稳定帧不重新生成包。
    assert!(
        app.world()
            .resource::<Assets<ShaderBuffer>>()
            .get(rain_shelter_buffer_handle().id())
            .unwrap()
            .cast_slice::<u8>()
            .is_none()
    ); // 正式上传系统保持空闲。
    let roof = volume(Transform::from_xyz(0.0, 4.0, 0.0)); // 创建完整新代理。
    let mut state = app.world_mut().resource_mut::<RainShelterState>(); // 宿主一次发布完整批次。
    state.gpu.header = Vec4::new(1.0, roof.half_extents.w, 0.0, 0.0); // 有效数和全局早退高度。
    state.gpu.shelters[0] = roof; // 唯一完整OBB。
    assert_eq!(state.exposure(Vec3::ZERO), 0.0); // CPU同帧使用正式批次。
    app.update(); // 正式上传编码完整布局。
    assert_eq!(
        app.world()
            .resource::<Assets<ShaderBuffer>>()
            .get(rain_shelter_buffer_handle().id())
            .unwrap()
            .cast_slice::<u8>()
            .unwrap()
            .len(),
        3088
    ); // 不上传可变长半成品。
} // 完成固定预算上传回归。
