//! 真实窗口相机通过正式camera_system初始化及禁用恢复后，Clipmap按当前机位使用作者距离档。
//! 窗口与FOV仍更新真实投影，但不能擅自增加作者LOD档位。

use super::*;
use bevy::{
    asset::{AssetEvent, RenderAssetUsages},
    camera::RenderTarget,
    ecs::{entity_disabling::Disabled, system::SystemState},
    render::{
        camera::camera_system,
        render_resource::{Extent3d, TextureDimension, TextureFormat},
        texture::ManualTextureViews,
    },
    window::{
        PrimaryWindow, WindowCreated, WindowResized, WindowResolution, WindowScaleFactorChanged,
    },
};
use feathrise_map_content::{compile_display_lod_plan, default_display_lods_for_spec};

struct Fixture {
    app: App,
    window: Entity,
    game: Entity,
    editor: Entity,
}

fn pose(height: f32) -> GlobalTransform {
    GlobalTransform::from(Transform::from_xyz(0.0, height, 0.0).looking_at(Vec3::ZERO, Vec3::Z))
}

fn fixture(disabled: bool) -> Fixture {
    let mut app = App::new();
    app.add_message::<WindowCreated>()
        .add_message::<WindowResized>()
        .add_message::<WindowScaleFactorChanged>()
        .add_message::<AssetEvent<Image>>()
        .init_resource::<Assets<Image>>()
        .init_resource::<ManualTextureViews>()
        .add_systems(PostUpdate, camera_system); // 使用引擎实际生产系统；无GPU和手填computed尺寸。
    let window = app
        .world_mut()
        .spawn((
            Window {
                resolution: WindowResolution::new(1024, 1024),
                ..default()
            },
            PrimaryWindow,
        ))
        .id();
    let game = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: !disabled,
                ..default()
            }, // gameplay真实默认窗口目标，没有显式Viewport。
            Projection::Perspective(PerspectiveProjection {
                fov: 1.0,
                aspect_ratio: 3.0,
                ..default()
            }),
            pose(10.0),
        ))
        .id();
    if disabled {
        app.world_mut().entity_mut(game).insert(Disabled);
    }
    let image = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_fill(
            Extent3d {
                width: 512,
                height: 512,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0; 4],
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::MAIN_WORLD,
        ));
    let editor = app
        .world_mut()
        .spawn((
            Camera3d::default(),
            Camera {
                is_active: disabled,
                ..default()
            },
            RenderTarget::from(image), // 编辑器使用独立RTT，尺寸同样由正式camera_system取得。
            Projection::Perspective(PerspectiveProjection {
                fov: 1.0,
                ..default()
            }),
            DynamicClipmapAuthoringView::default(),
            pose(640.0),
        ))
        .id();
    // 无关活动Camera3d不能成为游戏fallback，必须继续使用明确的父世界登记Entity。
    app.world_mut().spawn((Camera3d::default(), pose(999.0)));
    app.world_mut().write_message(WindowCreated { window });
    Fixture {
        app,
        window,
        game,
        editor,
    }
}

fn resolve(world: &mut World, game: Entity) -> Option<ClipmapView> {
    let mut state = SystemState::<ClipmapViews>::new(world);
    state.get(world).unwrap().resolve(Some(game))
}

fn plan(view: ClipmapView) -> (ViewLodKey, CompiledDisplayLodPlan) {
    let spec = SparseHeightSpec::new([-64.0; 2], [64.0; 2], 0.25, -64.0, 128.0).unwrap();
    let authored =
        compile_display_lod_plan(&spec, &default_display_lods_for_spec(&spec).unwrap()).unwrap();
    let (_, key) = view
        .choice(&spec, MapSourceFrame::default(), [0.0; 2], None, &authored)
        .unwrap();
    let plan = compile_view_plan(&spec, &authored, key).unwrap();
    assert_eq!(authored.authored_lods.len(), 4);
    assert_eq!(
        plan.authored_lods, authored.authored_lods,
        "相机不能改写作者四档设置"
    );
    assert!(
        plan.bands
            .iter()
            .all(|band| [1, 2, 4, 8].contains(&band.spacing_multiplier)),
        "默认四档的连续倍率没有额外过渡档，禁止FOV或像素大小新增倍率"
    );
    (key, plan)
}

#[test]
fn window_camera_lod_recovers_after_disabled_camera_misses_added_and_window_events() {
    let Fixture {
        mut app,
        game,
        editor,
        ..
    } = fixture(true);
    for _ in 0..4 {
        app.update();
    } // camera_system实际运行并消费创建事件，Disabled相机不在查询中。
    assert!(app.world().get::<Camera>(game).unwrap().viewport.is_none());
    assert!(
        app.world()
            .get::<Camera>(game)
            .unwrap()
            .computed
            .target_info
            .is_none()
    );
    let editor_view = resolve(app.world_mut(), game).unwrap();
    assert_eq!(editor_view.position, pose(640.0).translation());
    let (editor_key, editor_plan) = plan(editor_view);
    app.world_mut().get_mut::<Camera>(editor).unwrap().is_active = false;
    app.world_mut().entity_mut(game).remove::<Disabled>();
    app.world_mut().get_mut::<Camera>(game).unwrap().is_active = true;
    assert!(
        resolve(app.world_mut(), game).is_none(),
        "恢复前尚无尺寸，LOD只能暂时保留上一编辑方案"
    );
    app.update(); // 只恢复启停，没有改Projection/Viewport或补造Window事件。
    let camera = app.world().get::<Camera>(game).unwrap();
    assert_eq!(
        camera.physical_viewport_size(),
        Some(UVec2::splat(1024)),
        "旧实现错过Added后永久没有target_info；恢复必须在当前正式目标上重新初始化"
    );
    assert!(
        camera.viewport.is_none(),
        "恢复不能伪造固定Viewport掩盖窗口尺寸来源"
    );
    let Projection::Perspective(projection) = app.world().get::<Projection>(game).unwrap() else {
        panic!("玩法使用透视相机");
    };
    assert_eq!(
        projection.aspect_ratio, 1.0,
        "尺寸与真实投影必须在同次初始化中采用"
    );
    let game_view = resolve(app.world_mut(), game).unwrap();
    assert_eq!(game_view.position, pose(10.0).translation());
    let (game_key, game_plan) = plan(game_view);
    assert!(game_key.minimum_multiplier < editor_key.minimum_multiplier);
    assert!(
        game_plan.index_count > editor_plan.index_count,
        "关闭F12后真实游戏相机必须恢复细档拓扑，不能继续使用编辑器最后粗档"
    );
}

#[test]
fn window_camera_lod_tracks_authored_distance_and_updates_real_projection_without_extra_lods() {
    let Fixture {
        mut app,
        window,
        game,
        ..
    } = fixture(false);
    app.update();
    let (near, near_plan) = plan(resolve(app.world_mut(), game).unwrap());
    app.world_mut().entity_mut(game).insert(pose(640.0));
    app.update();
    let (far, far_plan) = plan(resolve(app.world_mut(), game).unwrap());
    assert!(far.minimum_multiplier > near.minimum_multiplier);
    assert!(
        far_plan.index_count < near_plan.index_count,
        "实际游戏相机移动必须改变LOD拓扑"
    );
    let original_projection = app.world().get::<Camera>(game).unwrap().clip_from_view();
    app.world_mut()
        .get_mut::<Window>(window)
        .unwrap()
        .resolution
        .set_physical_resolution(4096, 2304);
    app.world_mut().write_message(WindowResized {
        window,
        width: 4096.0,
        height: 2304.0,
    });
    app.update();
    let camera = app.world().get::<Camera>(game).unwrap();
    assert_eq!(
        camera.physical_viewport_size(),
        Some(UVec2::new(4096, 2304))
    );
    assert!(camera.viewport.is_none());
    let resized_projection = camera.clip_from_view();
    assert_ne!(
        resized_projection, original_projection,
        "真实窗口长宽比必须更新引擎投影矩阵"
    );
    let Projection::Perspective(projection) = app.world().get::<Projection>(game).unwrap() else {
        panic!("玩法使用透视相机");
    };
    assert!((projection.aspect_ratio - 16.0 / 9.0).abs() < 0.00001);
    let (resized, resized_plan) = plan(resolve(app.world_mut(), game).unwrap());
    assert_eq!(resized, far, "窗口像素更新不得修改作者世界距离档位");
    assert_eq!(resized_plan, far_plan);
    if let Projection::Perspective(projection) =
        &mut *app.world_mut().get_mut::<Projection>(game).unwrap()
    {
        projection.fov = 0.25;
    }
    app.update();
    let zoomed_projection = app.world().get::<Camera>(game).unwrap().clip_from_view();
    assert_ne!(
        zoomed_projection, resized_projection,
        "FOV修改仍须由正式camera_system采用到投影矩阵"
    );
    let Projection::Perspective(projection) = app.world().get::<Projection>(game).unwrap() else {
        panic!("玩法使用透视相机");
    };
    assert_eq!(projection.fov, 0.25);
    assert!((projection.aspect_ratio - 16.0 / 9.0).abs() < 0.00001);
    let (zoomed, zoomed_plan) = plan(resolve(app.world_mut(), game).unwrap());
    assert_eq!(zoomed, resized, "窄FOV不能发明作者四档之外的精度");
    assert_eq!(zoomed_plan, resized_plan);
}
