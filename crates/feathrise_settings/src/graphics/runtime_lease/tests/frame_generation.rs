//! 两种帧生成共用相机契约及与DLSS/FSR超分租约的抗锯齿优先级、独立释放回归。

use super::*; // 复用父测试域的真实画质系统和租约恢复夹具类型。

#[test]
fn frame_generation_requires_only_its_six_independent_inputs() {
    let camera = Entity::from_raw_u32(10).expect("测试相机索引有效"); // 使用固定目标，无需创建渲染设备。
    let requirements = GraphicsRuntimeRequirements::frame_generation(camera); // 两后端必须调用同一工厂。
    assert_eq!(requirements.target_camera, camera); // 不得扩大至其他窗口或纹理相机。
    assert!(requirements.force_single_sample); // 输入纹理必须单采样。
    assert!(requirements.force_fxaa); // 无时间超分时提供FXAA基础抗锯齿。
    assert!(requirements.require_hdr); // 保留公共HUDless颜色捕获契约。
    assert!(requirements.require_main_texture_copy_source); // 允许公共颜色捕获复制主纹理。
    assert!(requirements.require_depth_prepass); // 独立提供深度。
    assert!(requirements.require_motion_vector_prepass); // 独立提供运动矢量。
    assert!(!requirements.force_native_render_scale); // 原生和固定空间放大均能使用帧生成。
    assert!(!requirements.suspend_fxaa); // 暂停FXAA由时间超分租约独立声明。
    assert!(!requirements.suspend_dlss_super_resolution); // 帧生成不排斥任一超分。
    assert!(!requirements.require_deferred_prepass); // 不引入光追GBuffer。
    assert!(!requirements.require_deferred_double_buffer); // 不引入光追历史附件。
    assert!(!requirements.require_depth_double_buffer); // 不引入光追深度双缓冲。
    assert!(!requirements.require_main_texture_storage_binding); // 帧生成自身不要求计算写回主纹理。
    assert!(!requirements.suspend_ssao); // 保留原生路径普通环境遮蔽。
    assert!(!requirements.suspend_contact_shadows); // 保留普通接触阴影。
    assert!(!requirements.suspend_standard_shadows); // 保留光栅阴影。
}

#[test]
fn temporal_upscaling_suppresses_fg_fxaa_and_each_lease_releases_independently() {
    for fsr in [false, true] {
        // 两个超分提供者遵守同一合并和恢复契约。
        let mut app = App::new(); // 使用真实ECS系统验证优先级，不复制实现布尔表达式。
        app.init_resource::<RenderSettings>() // 默认玩家请求MSAA 4×。
            .init_resource::<GraphicsRuntimeLeaseRegistry>() // 每轮建立独立所有权注册表。
            .add_systems(Update, (apply_antialiasing, ApplyDeferred).chain()); // 将普通画质对合并要求的决定真正写入组件。
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),                      // 提供真实三维相机要求组件。
                RenderTarget::Window(WindowRef::Primary), // 只对窗口相机运行普通抗锯齿系统。
                Msaa::Sample4,                            // 无租约时应恢复的玩家档位。
            ))
            .id(); // 固定单相机，无GPU初始化。
        let fg_requirements = GraphicsRuntimeRequirements::frame_generation(camera); // 使用正式公共工厂。
        let sr_requirements = if fsr {
            // 分别覆盖DLSS与FSR原有超分声明。
            GraphicsRuntimeRequirements::fsr3_super_resolution(camera) // FSR同样暂停FXAA。
        } else {
            GraphicsRuntimeRequirements::dlss_super_resolution(camera) // DLSS同样暂停FXAA。
        };
        let fg = app
            .world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .acquire(fg_requirements); // 独立登记帧生成。
        app.update(); // 原生帧生成路径采用FXAA。
        assert!(app.world().get::<Fxaa>(camera).is_some()); // 无超分时强制FXAA生效。
        let sr = app
            .world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .acquire(sr_requirements); // 叠加时间超分。
        app.update(); // 两份声明合并后重新收敛实际抗锯齿。
        assert!(app.world().get::<Fxaa>(camera).is_none()); // suspend_fxaa必须压过FG的force_fxaa。
        assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Off)); // 两者均要求单采样。
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(sr); // 先释放超分，帧生成继续运行。
        app.update(); // 恢复原生帧生成的抗锯齿。
        assert!(app.world().get::<Fxaa>(camera).is_some()); // 超分退出后同帧恢复FG的FXAA。
        assert_eq!(
            app.world()
                .resource::<GraphicsRuntimeLeaseRegistry>()
                .requirements_for_camera(camera),
            Some(fg_requirements)
        ); // 所有SR附加要求完整释放。
        let sr = app
            .world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .acquire(sr_requirements); // 重新组合以覆盖相反释放顺序。
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(fg); // 关闭FG不能撤销超分自身的输入要求。
        app.update(); // 仅剩超分时保持其抗锯齿所有权。
        assert!(app.world().get::<Fxaa>(camera).is_none()); // 没有残留FG FXAA覆盖。
        assert_eq!(
            app.world()
                .resource::<GraphicsRuntimeLeaseRegistry>()
                .requirements_for_camera(camera),
            Some(sr_requirements)
        ); // SR全部要求保持独立有效。
        app.world_mut()
            .resource_mut::<GraphicsRuntimeLeaseRegistry>()
            .release(sr); // 最后一份租约释放。
        app.update(); // 最新玩家设置重新接管。
        assert_eq!(app.world().get::<Msaa>(camera), Some(&Msaa::Sample4)); // 恢复玩家MSAA档位。
        assert_eq!(
            app.world()
                .resource::<GraphicsRuntimeLeaseRegistry>()
                .active_lease_count(),
            0
        ); // 两个释放顺序均无残留租约。
    }
}
