//! 游戏内空气墙半透明调试显示：单在途、固定网格批次、显隐不触碰碰撞。
mod geometry; // 同源盒体规格转为固定预算网格。
mod outline; // 编辑器档走原始作者点贴地轮廓，与完整墙面分离。
use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes}; // 每个子地图借用独立状态，父图保留原资源。
use super::{
    derivation::{Chunk, ChunkKey},
    resources::DynamicMapBoundaryState,
};
use bevy::{
    light::{NotShadowCaster, NotShadowReceiver},
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
}; // 后台只计算纯网格，ECS由主线程采用。
use feathrise_settings::{AirWallDisplayMode, SceneDisplaySettings}; // F1持久化的独立显示开关。
use feathrise_scene_core::state::ScenePhase; // 可视根与地图一起销毁。
pub(in crate::maps::dynamic_map) use outline::show_air_wall_outlines;
use std::{collections::BTreeMap, sync::Arc}; // 只共享已采用碰撞缓存，不复制整图规格。
#[derive(Component)]
pub(in crate::maps::dynamic_map) struct WallVisualRoot; // 全部调试墙面显隐的唯一根。
#[derive(Default)]
pub(in crate::maps::dynamic_map) struct VisualState {
    source: Arc<BTreeMap<ChunkKey, Arc<Chunk>>>, // 所见范围严格对应实际已安装碰撞。
    generation: u64,                             // 切图或碰撞变化后拒收旧网格。
    cursor: Option<geometry::Cursor>,            // 下一固定批次。
    task: Option<Task<(u64, geometry::Batch)>>,  // 旧任务真正完成前不释放槽位。
    root: Option<Entity>,                        // 显隐只写根，稳定帧不扫描所有墙面。
    entities: Vec<Entity>,                       // 换代时每帧最多回收32个旧批次。
    retiring: bool,                              // 旧几何完全回收后再显示新碰撞版本。
    material: Option<Handle<StandardMaterial>>,  // 所有批次共用一份无光照材质。
}
#[allow(clippy::too_many_arguments)]
pub(in crate::maps::dynamic_map) fn show_air_walls(
    mut commands: Commands,
    settings: Res<SceneDisplaySettings>,
    boundary: InstanceRes<DynamicMapBoundaryState>,
    mut state: Local<VisualState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut roots: InstanceQuery<&mut Visibility, With<WallVisualRoot>>,
) {
    let enabled = settings.air_wall_display_mode == AirWallDisplayMode::Full; // 游戏显隐与作者视口独立。
    if !Arc::ptr_eq(&state.source, &boundary.derivation.installed) {
        state.source = Arc::clone(&boundary.derivation.installed); // 已成功采用的碰撞才会改变来源。
        state.generation = state.generation.wrapping_add(1);
        state.cursor = Some(default());
        state.retiring = true; // 拒绝半套新旧几何混合显示。
    }
    if let Some(root) = state.root
        && let Ok(mut visibility) = roots.get_mut(root)
    {
        let desired = if enabled && !state.retiring {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }; // 开关只影响可见面。
        if *visibility != desired {
            *visibility = desired;
        }
    }
    if state.retiring {
        for _ in 0..32 {
            let Some(entity) = state.entities.pop() else {
                break;
            };
            commands.entity(entity).try_despawn();
        } // 有界释放旧批次和Mesh句柄。
        if state.entities.is_empty() {
            if let Some(root) = state.root.take() {
                commands.entity(root).try_despawn();
            }
            state.retiring = false;
        } // 不堆积旧代次。
    }
    if let Some((generation, batch)) = state.task.as_mut().and_then(check_ready) {
        state.task = None; // 真正完成后释放唯一槽。
        if generation == state.generation && !state.retiring {
            state.cursor = batch.next; // 成功采用才推进游标。
            if let Some(mesh) = batch.mesh {
                let material = state
                    .material
                    .get_or_insert_with(|| {
                        materials.add(StandardMaterial {
                            base_color: Color::srgba(0.10, 0.85, 0.95, 0.20), // 半透明青色，便于同时看清墙后场景。
                            alpha_mode: AlphaMode::Blend,                     // 真实半透明面。
                            unlit: true,        // 不受昼夜亮度影响，避免把墙误认为灯光。
                            cull_mode: None,    // 从墙内外两侧均能看见。
                            double_sided: true, // 双面法线用于稳定呈现。
                            ..default()
                        })
                    })
                    .clone(); // 不逐段创建材质。
                let root = *state.root.get_or_insert_with(|| {
                    commands
                        .spawn((
                            WallVisualRoot,
                            Name::new("空气墙调试显示"),
                            Transform::IDENTITY,
                            if enabled {
                                Visibility::Inherited
                            } else {
                                Visibility::Hidden
                            },
                            DespawnOnExit(ScenePhase::Active),
                        ))
                        .id()
                }); // 地图退出统一清理。
                let entity = commands
                    .spawn((
                        Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(material),
                        Transform::IDENTITY,
                        NotShadowCaster,
                        NotShadowReceiver,
                        feathrise_scene_core::view::CameraRaycastIgnore, // 调试墙不把第三人称镜头推近。
                        Pickable::IGNORE,                             // 辅助面不接管鼠标选择。
                        bevy::camera::visibility::RenderLayers::layer(
                            feathrise_scene_core::constants::render_layers::AIR_WALL_GAMEPLAY_LAYER,
                        ), // 专用玩法层，F12相机不接收。
                        ChildOf(root),
                    ))
                    .id(); // 显示实体没有物理组件，不投影或挡光。
                state.entities.push(entity); // 只登记固定大小新批次。
            }
        } // 过期网格最多一个固定批次，可直接释放。
    }
    if enabled
        && !state.retiring
        && state.task.is_none()
        && let Some(cursor) = state.cursor
    {
        let source = Arc::clone(&state.source);
        let generation = state.generation; // 后台持有不可变物理规格。
        state.task = Some(
            AsyncComputeTaskPool::get()
                .spawn(async move { (generation, geometry::build(&source, cursor)) }),
        ); // 每帧最多启动一个256段批次，关闭后不再计算。
    }
}

#[cfg(test)]
mod tests {
    use super::super::helpers::BoundaryColliderSpec;
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn air_wall_toggle_reuses_geometry_and_never_creates_colliders() {
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new); // 真实非阻塞任务池。
        let mut app = App::new();
        app.init_resource::<SceneDisplaySettings>()
            .init_resource::<DynamicMapBoundaryState>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .add_systems(Update, show_air_walls);
        let chunk = Chunk {
            specs: vec![BoundaryColliderSpec {
                name: "wall".into(),
                size: Vec3::splat(2.0),
                transform: Transform::IDENTITY,
            }],
            collider: avian3d::prelude::Collider::cuboid(2.0, 2.0, 2.0),
        }; // 模拟一块已安装的碰撞规格。
        app.world_mut()
            .resource_mut::<DynamicMapBoundaryState>()
            .derivation
            .installed = Arc::new(BTreeMap::from([((0, 0), Arc::new(chunk))]));
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            0
        ); // 默认关闭不构造网格。
        app.world_mut()
            .resource_mut::<SceneDisplaySettings>()
            .air_wall_display_mode = AirWallDisplayMode::Full;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            app.update();
            if app.world_mut().query::<&Mesh3d>().iter(app.world()).count() == 1 {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        } // 真正后台批次可完成。
        let mesh = app
            .world_mut()
            .query::<&Mesh3d>()
            .single(app.world())
            .unwrap()
            .0
            .id();
        assert_eq!(
            app.world_mut()
                .query::<&avian3d::prelude::Collider>()
                .iter(app.world())
                .count(),
            0
        ); // 辅助实体无碰撞组件。
        app.world_mut()
            .resource_mut::<SceneDisplaySettings>()
            .air_wall_display_mode = AirWallDisplayMode::Hidden;
        app.update();
        assert_eq!(
            *app.world_mut()
                .query_filtered::<&Visibility, With<WallVisualRoot>>()
                .single(app.world())
                .unwrap(),
            Visibility::Hidden
        ); // 只隐藏根。
        app.world_mut()
            .resource_mut::<SceneDisplaySettings>()
            .air_wall_display_mode = AirWallDisplayMode::Full;
        app.update();
        assert_eq!(
            app.world_mut()
                .query::<&Mesh3d>()
                .single(app.world())
                .unwrap()
                .0
                .id(),
            mesh
        ); // 再开直接复用GPU几何。
        app.world_mut()
            .resource_mut::<SceneDisplaySettings>()
            .air_wall_display_mode = AirWallDisplayMode::Editor;
        app.update();
        assert_eq!(
            *app.world_mut()
                .query_filtered::<&Visibility, With<WallVisualRoot>>()
                .single(app.world())
                .unwrap(),
            Visibility::Hidden
        ); // Full切Editor当帧隐藏所有整面墙，不能等待轮廓后台完成。
        app.world_mut()
            .resource_mut::<DynamicMapBoundaryState>()
            .derivation
            .installed = Arc::default();
        app.update();
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            0
        ); // 删除实际墙后不会残留旧显示。
    }
}
