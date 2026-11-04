//! 用真实文字生成、槽位分配与camera_system验证同帧退休合同；不启动窗口或GPU。
use crate::{
    BillboardBakePool, BillboardColorMaterial, BillboardDecoration, BillboardTemporalGuideMaterial,
    BillboardText, BillboardTextHandle, BillboardTextLink, BillboardTextOutline, bake,
    configure_bake_target_retirement, configure_billboard_pose_update, spawn_billboard_texts,
    systems::update_billboard_texts,
};
use bevy::{
    camera::{CameraUpdateSystems, RenderTarget},
    ecs::{
        error::BevyError,
        schedule::SingleThreadedExecutor,
        system::{IntoSystem, RunSystemOnce},
    },
    prelude::*,
    render::{camera::camera_system, texture::ManualTextureViews},
    window::{WindowCreated, WindowResized, WindowScaleFactorChanged},
};

struct BakingTask {
    owner: Entity,
    billboard: Entity,
    camera: Entity,
    image: Handle<Image>,
    mesh: Handle<Mesh>,
    material: Handle<BillboardColorMaterial>,
    guide_material: Handle<BillboardTemporalGuideMaterial>,
    source_entities: Vec<Entity>,
    decoration_meshes: Vec<Handle<Mesh>>,
    decoration_materials: Vec<Handle<ColorMaterial>>,
}

fn fixture() -> App {
    let mut app = App::new();
    app.init_resource::<Assets<Image>>()
        .init_resource::<Assets<Mesh>>()
        .init_resource::<Assets<ColorMaterial>>()
        .init_resource::<Assets<BillboardColorMaterial>>()
        .init_resource::<Assets<BillboardTemporalGuideMaterial>>()
        .init_resource::<Time>()
        .init_resource::<BillboardBakePool>()
        .init_resource::<ManualTextureViews>()
        .add_message::<WindowCreated>()
        .add_message::<WindowResized>()
        .add_message::<WindowScaleFactorChanged>()
        .add_message::<AssetEvent<Image>>();
    app.world_mut()
        .run_system_once(bake::setup_billboard_bake_pool)
        .unwrap(); // 直接使用生产固定池；仅创建四个轻量相机和一张占位图。
    app
}

fn start_task(app: &mut App) -> BakingTask {
    let owner = app
        .world_mut()
        .spawn((
            Transform::default(),
            BillboardText {
                text: "正在烘焙的名牌".into(),
                texture_size: UVec2::new(64, 16),
                crop_to_text: false,
                shadow: None,
                outline: Some(BillboardTextOutline {
                    color: Color::BLACK,
                    width_px: 1.0,
                }),
                decorations: vec![BillboardDecoration {
                    radius_px: 3.0,
                    color: Color::WHITE,
                    z: 1.0,
                }],
                ..default()
            },
        ))
        .id();
    app.world_mut()
        .run_system_once(spawn_billboard_texts)
        .unwrap(); // 不伪造Link，真实生成最终图像、网格和两种材质。
    let billboard = app.world().get::<BillboardTextHandle>(owner).unwrap().0;
    app.world_mut()
        .run_system_once(bake::assign_billboard_bake_jobs)
        .unwrap(); // 真实切换Projection/RenderTarget并创建文字、描边和装饰源。
    let camera = app
        .world()
        .resource::<BillboardBakePool>()
        .slots
        .iter()
        .find(|slot| slot.billboard == Some(billboard))
        .unwrap()
        .camera_entity;
    let link = app.world().get::<BillboardTextLink>(billboard).unwrap();
    assert!(!link.bake_ready); // 复现宿主在活跃烘焙期退出，不用已经归还槽位的TTL标签替代。
    let mut source_entities = vec![link.temporal_guide_entity, link.text_entity.unwrap()];
    source_entities.extend(link.outline_entities.iter().copied());
    source_entities.extend(link.decoration_entities.iter().copied());
    BakingTask {
        owner,
        billboard,
        camera,
        image: link.image.clone(),
        mesh: link.mesh.clone(),
        material: link.material.clone(),
        guide_material: link.temporal_guide_material.clone(),
        source_entities,
        decoration_meshes: link.decoration_meshes.clone(),
        decoration_materials: link.decoration_materials.clone(),
    }
}

fn assert_released(app: &App, task: &BakingTask) {
    let world = app.world();
    assert!(world.get_entity(task.billboard).is_err());
    for entity in &task.source_entities {
        assert!(world.get_entity(*entity).is_err());
    }
    assert!(world.resource::<Assets<Image>>().get(&task.image).is_none());
    assert!(world.resource::<Assets<Mesh>>().get(&task.mesh).is_none());
    assert!(
        world
            .resource::<Assets<BillboardColorMaterial>>()
            .get(&task.material)
            .is_none()
    );
    assert!(
        world
            .resource::<Assets<BillboardTemporalGuideMaterial>>()
            .get(&task.guide_material)
            .is_none()
    );
    for mesh in &task.decoration_meshes {
        assert!(world.resource::<Assets<Mesh>>().get(mesh).is_none());
    }
    for material in &task.decoration_materials {
        assert!(
            world
                .resource::<Assets<ColorMaterial>>()
                .get(material)
                .is_none()
        );
    } // 夹具仍强持有旧句柄，断言证明生产路径显式回收，不是靠句柄自动失效。
}

#[test]
fn old_retirement_then_owner_cleanup_returns_real_missing_image_error() {
    for active in [false, true] {
        let mut app = fixture();
        let task = start_task(&mut app);
        app.world_mut()
            .get_mut::<Camera>(task.camera)
            .unwrap()
            .is_active = active;
        app.world_mut().despawn(task.owner); // 模拟Update已经移除宿主，最终Link仍等PostUpdate清理。

        // 固定执行旧图允许的危险顺序，负控不依赖并行执行器恰好选择某种先后关系。
        app.world_mut()
            .run_system_once(bake::release_orphaned_bake_slots)
            .unwrap();
        assert_eq!(
            app.world().resource::<BillboardBakePool>().slots[0].billboard,
            Some(task.billboard)
        ); // 第一遍退休检查时Link仍存在，故相机仍指向任务纹理。
        app.world_mut()
            .run_system_once(update_billboard_texts)
            .unwrap();
        assert_released(&app, &task);
        let camera_result: Result<(), BevyError> =
            app.world_mut().run_system_once(camera_system).unwrap(); // 明确保留Result输出，不能交给调度器默认错误处理器。
        let error = camera_result
            .expect_err("旧顺序必须使真实camera_system返回悬空Image错误")
            .to_string();
        assert!(error.contains("RenderTarget::Image missing"), "{error}");
        assert!(error.contains(&format!("{:?}", task.image.id())), "{error}");
        // 捕获返回值而非调用默认错误处理器，不把预期负控变成真实进程崩溃。
        app.world_mut()
            .run_system_once(bake::release_orphaned_bake_slots)
            .unwrap();
        let camera_result: Result<(), BevyError> =
            app.world_mut().run_system_once(camera_system).unwrap();
        camera_result.expect("本帧退休门在清理后运行即可修复真实相机目标");
    }
}

#[derive(Resource, Default)]
struct RemoveOwner(Option<Entity>);

fn remove_owner_in_update(mut commands: Commands, mut owner: ResMut<RemoveOwner>) {
    if let Some(owner) = owner.0.take() {
        commands.entity(owner).try_despawn();
    }
}

#[derive(Resource, Default)]
struct CameraValidation {
    frames: usize,
    error: Option<String>,
}

fn capture_camera_result(
    In(result): In<Result<(), BevyError>>,
    mut validation: ResMut<CameraValidation>,
) {
    validation.frames += 1;
    validation.error = result.err().map(|error| error.to_string());
}

#[test]
fn production_postupdate_retires_unfinished_owner_before_real_camera_and_reuses_same_slot() {
    for active in [false, true] {
        let mut app = fixture();
        let task = start_task(&mut app);
        app.world_mut()
            .get_mut::<Camera>(task.camera)
            .unwrap()
            .is_active = active;
        let cameras: Vec<_> = app
            .world()
            .resource::<BillboardBakePool>()
            .slots
            .iter()
            .map(|slot| slot.camera_entity)
            .collect();
        let placeholder = app
            .world()
            .resource::<BillboardBakePool>()
            .placeholder_image
            .clone();
        app.insert_resource(RemoveOwner(Some(task.owner)))
            .init_resource::<CameraValidation>()
            .add_systems(Update, remove_owner_in_update);
        configure_bake_target_retirement(&mut app);
        configure_billboard_pose_update(&mut app); // 两个生产入口共同安装，不在夹具里替退休门补after。
        app.add_systems(
            PostUpdate,
            camera_system
                .pipe(capture_camera_result)
                .in_set(CameraUpdateSystems)
                .after(update_billboard_texts), // 真实校验读取本帧回收结果；这是旧注册图也允许的先后关系。
        );
        app.edit_schedule(PostUpdate, |schedule| {
            schedule.set_executor(SingleThreadedExecutor::new());
        }); // 先登记退休门并固定串行；正确先后必须由生产依赖覆盖登记顺序。

        app.update(); // 宿主先在Update退出，再执行真实PostUpdate文字回收与相机校验。
        let validation = app.world().resource::<CameraValidation>();
        assert_eq!(validation.frames, 1);
        assert!(validation.error.is_none(), "{:?}", validation.error);
        assert_released(&app, &task);
        assert_eq!(
            app.world()
                .get::<RenderTarget>(task.camera)
                .and_then(RenderTarget::as_image),
            Some(&placeholder)
        );
        assert!(!app.world().get::<Camera>(task.camera).unwrap().is_active);
        assert!(
            app.world()
                .resource::<BillboardBakePool>()
                .slots
                .iter()
                .all(|slot| slot.billboard.is_none())
        );
        assert_eq!(app.world().resource::<Assets<Image>>().len(), 1);

        let next = start_task(&mut app); // 用真实FIFO分配器验证刚归还的第一槽立即可再次接纳任务。
        assert_eq!(next.camera, task.camera);
        assert_ne!(next.image.id(), task.image.id());
        app.update();
        let validation = app.world().resource::<CameraValidation>();
        assert_eq!(validation.frames, 2);
        assert!(validation.error.is_none(), "{:?}", validation.error);
        assert_eq!(
            app.world()
                .get::<RenderTarget>(next.camera)
                .and_then(RenderTarget::as_image),
            Some(&next.image)
        );
        assert!(app.world().get::<Camera>(next.camera).unwrap().is_active);
        let pool = app.world().resource::<BillboardBakePool>();
        assert_eq!(pool.slots[0].billboard, Some(next.billboard));
        assert_eq!(
            pool.slots
                .iter()
                .map(|slot| slot.camera_entity)
                .collect::<Vec<_>>(),
            cameras
        );
        let images = app.world().resource::<Assets<Image>>();
        assert_eq!(images.len(), 2); // 仅长期占位图和新任务图存活，旧任务不能因固定相机继续占用而滞留。
        assert!(images.get(&placeholder).is_some());
        assert!(images.get(&next.image).is_some());

        app.world_mut().resource_mut::<RemoveOwner>().0 = Some(next.owner);
        app.update(); // 第二笔也能完整回收，证明槽位复用没有带入上一笔的退休身份。
        assert_released(&app, &next);
        assert!(app.world().resource::<CameraValidation>().error.is_none());
        assert_eq!(app.world().resource::<Assets<Image>>().len(), 1);
        assert_eq!(
            app.world()
                .get::<RenderTarget>(next.camera)
                .and_then(RenderTarget::as_image),
            Some(&placeholder)
        );
    }
}

#[test]
fn replacing_active_request_releases_all_sources_and_retires_camera_before_validation() {
    let mut app = fixture();
    let previous = start_task(&mut app); // 含真实主文字、八向描边、装饰和时间引导，当前仍占固定烘焙槽。
    let placeholder = app
        .world()
        .resource::<BillboardBakePool>()
        .placeholder_image
        .clone();
    app.init_resource::<CameraValidation>()
        .add_systems(Update, spawn_billboard_texts);
    configure_bake_target_retirement(&mut app);
    app.add_systems(
        PostUpdate,
        camera_system
            .pipe(capture_camera_result)
            .in_set(CameraUpdateSystems),
    );
    app.world_mut()
        .entity_mut(previous.owner)
        .insert(BillboardText {
            text: "更新后的源名称".into(),
            texture_size: UVec2::new(96, 32),
            color: Color::srgb(0.2, 0.4, 0.6),
            crop_to_text: false,
            ..default()
        });

    app.update(); // 与游戏重发持久文本相同：原锚点不销毁，只替换显示请求。
    assert_released(&app, &previous); // 强持有旧句柄也必须回收全部动态资产，不能依赖自动卸载。
    assert!(app.world().resource::<CameraValidation>().error.is_none());
    assert_eq!(
        app.world()
            .get::<RenderTarget>(previous.camera)
            .and_then(RenderTarget::as_image),
        Some(&placeholder),
    ); // 退休门必须在真实camera_system之前撤掉旧图像目标。
    assert!(
        !app.world()
            .get::<Camera>(previous.camera)
            .unwrap()
            .is_active
    );
    let replacement = app
        .world()
        .get::<BillboardTextHandle>(previous.owner)
        .unwrap()
        .0;
    assert_ne!(replacement, previous.billboard);
    let spec = app
        .world()
        .get::<BillboardTextLink>(replacement)
        .unwrap()
        .bake_spec
        .as_ref()
        .unwrap();
    assert_eq!(spec.text, "更新后的源名称");
    assert_eq!(spec.texture_size, UVec2::new(96, 32));
    assert_eq!(spec.color, Color::srgb(0.2, 0.4, 0.6)); // 重发完整新规格，不能沿用旧尺寸或外观。
    assert_eq!(app.world().resource::<Assets<Image>>().len(), 2);

    app.world_mut()
        .run_system_once(bake::assign_billboard_bake_jobs)
        .unwrap();
    let pool = app.world().resource::<BillboardBakePool>();
    assert_eq!(pool.slots[0].billboard, Some(replacement));
    assert_eq!(pool.slots[0].camera_entity, previous.camera); // 被替换任务归还原固定槽，不新增相机。
}

#[test]
fn replacing_queued_request_skips_dead_entry_and_does_not_repeat_without_request() {
    use bevy::ecs::entity_disabling::Disabled;

    let mut app = fixture();
    let owner = app
        .world_mut()
        .spawn((Transform::default(), BillboardText::default()))
        .id();
    app.world_mut()
        .run_system_once(spawn_billboard_texts)
        .unwrap();
    let previous = app.world().get::<BillboardTextHandle>(owner).unwrap().0;
    let (old_image, old_mesh, old_material, old_guide_material) = {
        let link = app.world().get::<BillboardTextLink>(previous).unwrap();
        (
            link.image.clone(),
            link.mesh.clone(),
            link.material.clone(),
            link.temporal_guide_material.clone(),
        )
    };
    app.world_mut().entity_mut(previous).insert(Disabled); // 旧实例仍在停驻状态时也必须按句柄回收其资产。
    app.world_mut().entity_mut(owner).insert(BillboardText {
        text: "队列中的新名称".into(),
        ..default()
    });
    app.world_mut()
        .run_system_once(spawn_billboard_texts)
        .unwrap();
    let replacement = app.world().get::<BillboardTextHandle>(owner).unwrap().0;
    assert_ne!(replacement, previous);
    assert!(app.world().get_entity(previous).is_err());
    assert!(
        app.world()
            .resource::<Assets<Image>>()
            .get(&old_image)
            .is_none()
    );
    assert!(
        app.world()
            .resource::<Assets<Mesh>>()
            .get(&old_mesh)
            .is_none()
    );
    assert!(
        app.world()
            .resource::<Assets<BillboardColorMaterial>>()
            .get(&old_material)
            .is_none()
    );
    assert!(
        app.world()
            .resource::<Assets<BillboardTemporalGuideMaterial>>()
            .get(&old_guide_material)
            .is_none()
    );

    app.world_mut()
        .run_system_once(bake::assign_billboard_bake_jobs)
        .unwrap();
    let pool = app.world().resource::<BillboardBakePool>();
    assert!(pool.waiting.is_empty()); // FIFO跳过被替换的旧任务，不把失效实体卡在队首。
    assert_eq!(pool.slots[0].billboard, Some(replacement));
    app.world_mut()
        .run_system_once(spawn_billboard_texts)
        .unwrap();
    assert_eq!(
        app.world().get::<BillboardTextHandle>(owner).unwrap().0,
        replacement
    );
    assert_eq!(app.world().resource::<Assets<Image>>().len(), 2); // 没有新请求时不再次分配或烘焙。
}

#[test]
fn parked_request_is_consumed_after_reactivation_even_after_added_tick_expires() {
    use bevy::ecs::entity_disabling::Disabled;

    let mut app = fixture();
    app.add_systems(Update, spawn_billboard_texts);
    let owner = app
        .world_mut()
        .spawn((
            Transform::default(),
            Disabled,
            BillboardText {
                text: "停驻期间更新的名称".into(),
                ..default()
            },
        ))
        .id();
    app.update();
    app.update(); // 消费系统照常运行，Disabled对象的请求仍保留，但Added过滤的时间窗口已经过去。
    assert!(app.world().get::<BillboardTextHandle>(owner).is_none());
    assert!(app.world().get::<BillboardText>(owner).is_some());

    app.world_mut().entity_mut(owner).remove::<Disabled>();
    app.update();
    let billboard = app
        .world()
        .get::<BillboardTextHandle>(owner)
        .expect("激活后不能漏掉仍挂在锚点上的请求")
        .0;
    assert!(app.world().get::<BillboardText>(owner).is_none());
    assert_eq!(
        app.world()
            .get::<BillboardTextLink>(billboard)
            .unwrap()
            .bake_spec
            .as_ref()
            .unwrap()
            .text,
        "停驻期间更新的名称",
    );
    app.update();
    assert_eq!(
        app.world().get::<BillboardTextHandle>(owner).unwrap().0,
        billboard
    );
    assert_eq!(app.world().resource::<Assets<Image>>().len(), 2);
}
