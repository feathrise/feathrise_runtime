//! 无窗口真实覆盖系统回归：不同对象、作者票据与高度框架必须共同匹配且只产生一次绘制。

use super::*;
use crate::maps::dynamic_map::{
    clipmap::{
        ClipmapBounds, ClipmapFlatHeightPage, ClipmapHeightExtension, DynamicClipmapGroundSurface,
        test_ground_snapshot,
    },
    instances::{SubMapActive, SubMapRuntime},
    sparse_runtime::SparseClipmapRuntime,
};
use feathrise_map_content::{MapInstancePath, TerrainId};
use feathrise_scene_core::residency::MapSourceFrame;
use std::collections::BTreeMap;

struct Fixture {
    app: App,
    map: MapId,
    ids: [TerrainId; 2],
    sources: [Entity; 2],
    displays: [Entity; 2],
    height_handles: [Handle<ClipmapHeightMaterial>; 2],
    green: Handle<Image>,
    blue: Handle<Image>,
    vegetation: Handle<Image>,
}

impl Fixture {
    fn new() -> Self {
        let mut app = App::new();
        app.init_resource::<Assets<Image>>()
            .init_resource::<Assets<ClipmapHeightMaterial>>()
            .init_resource::<Assets<SparseClipmapSurfaceMaterial>>()
            .init_resource::<Assets<OverlayMaterial>>()
            .init_resource::<ClipmapFlatHeightPage>()
            .init_resource::<DynamicClipmapVegetationOverlay>()
            .init_resource::<DynamicClipmapSurfaceOverlay>()
            .init_resource::<OverlayState>()
            .init_resource::<SparseClipmapRuntime>()
            .init_resource::<DynamicClipmapGroundSurface>()
            .init_resource::<InstanceResourceTable<SparseClipmapRuntime>>()
            .init_resource::<InstanceResourceTable<DynamicClipmapGroundSurface>>()
            .init_resource::<crate::TerrainObjectAuthoringTarget>()
            .add_systems(Update, sync_overlay);
        let map = MapId::parse("MAP-surface-feedback").unwrap();
        app.insert_resource(ActiveDynamicMap {
            map_id: Some(map.clone()),
        });
        let ids = ["feedback-a", "feedback-b"].map(|id| TerrainId::parse(id).unwrap());
        let batch = app
            .world_mut()
            .spawn((SubMapActive, Transform::IDENTITY))
            .id();
        let poses = [
            Transform::from_xyz(20.0, 6.0, -30.0)
                .with_rotation(Quat::from_rotation_y(0.4))
                .with_scale(Vec3::new(2.0, 3.0, 0.5)),
            Transform::from_xyz(-50.0, 9.0, 12.0)
                .with_rotation(Quat::from_rotation_y(0.7))
                .with_scale(Vec3::new(0.5, 2.0, 4.0)),
        ];
        let sources = poses.map(|pose| {
            app.world_mut()
                .spawn((pose, MapSourceFrame::from_transform(pose), ChildOf(batch)))
                .id()
        });
        let fallback = app.world().resource::<ClipmapFlatHeightPage>().clone();
        let height_handles = std::array::from_fn(|index| {
            let mut height = ClipmapHeightExtension::new_flat(
                ClipmapBounds::new(Vec2::ZERO, Vec2::new(96.0, 64.0)).unwrap(),
                UVec2::new(96, 64),
                &fallback,
            )
            .unwrap();
            height.set_source_frame(poses[index].to_matrix());
            app.world_mut()
                .resource_mut::<Assets<ClipmapHeightMaterial>>()
                .add(ExtendedMaterial {
                    base: StandardMaterial::default(),
                    extension: height,
                })
        });
        let displays = std::array::from_fn(|index| {
            app.world_mut()
                .spawn((
                    DynamicSparseClipmapPrimitive,
                    Mesh3d(Handle::default()),
                    Transform::from_xyz(index as f32 * 4.0, 0.0, index as f32 * 8.0),
                    MeshMaterial3d(height_handles[index].clone()),
                    ChildOf(sources[index]),
                ))
                .id()
        });
        for index in 0..2 {
            let mut runtime = SparseClipmapRuntime::default();
            runtime.display = Some(displays[index]);
            app.world_mut()
                .resource_mut::<InstanceResourceTable<SparseClipmapRuntime>>()
                .values
                .insert(sources[index], runtime);
            let mut snapshot = test_ground_snapshot();
            snapshot.map_id = map.clone();
            app.world_mut()
                .resource_mut::<InstanceResourceTable<DynamicClipmapGroundSurface>>()
                .values
                .insert(
                    sources[index],
                    DynamicClipmapGroundSurface {
                        snapshot: Some(snapshot),
                        ..default()
                    },
                );
        }
        app.insert_resource(SubMapRuntime::test_active(
            batch,
            BTreeMap::from([
                (
                    MapInstancePath::root(map.clone()).terrain(ids[0].clone()),
                    sources[0],
                ),
                (
                    MapInstancePath::root(map.clone()).terrain(ids[1].clone()),
                    sources[1],
                ),
            ]),
        ));
        let [green, blue, vegetation] = std::array::from_fn(|_| {
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::default())
        });
        let mut fixture = Self {
            app,
            map,
            ids,
            sources,
            displays,
            height_handles,
            green,
            blue,
            vegetation,
        };
        fixture.select(0);
        fixture
    }

    fn select(&mut self, index: usize) {
        {
            let mut target = self
                .app
                .world_mut()
                .resource_mut::<crate::TerrainObjectAuthoringTarget>();
            target.map = Some(self.map.clone());
            target.terrain = Some(self.ids[index].clone());
        }
        crate::maps::dynamic_map::instances::test_apply_authoring_target(self.app.world_mut());
        assert_eq!(
            self.app
                .world()
                .resource::<crate::TerrainObjectAuthoringTarget>()
                .instance(),
            Some(self.sources[index])
        );
    }

    fn identity(&self, index: usize) -> ClipmapSurfaceOverlayIdentity {
        ClipmapSurfaceOverlayIdentity {
            map_id: self.map.clone(),
            terrain_id: Some(self.ids[index].clone()),
            author_revision: 9,
            terrain_revision: 0,
            surface_revision: 7,
            source_to_world: self
                .app
                .world()
                .get::<MapSourceFrame>(self.sources[index])
                .unwrap()
                .source_to_world,
        }
    }

    fn publish_selection(&mut self, index: usize) {
        let identity = self.identity(index);
        let mut channel = self
            .app
            .world_mut()
            .resource_mut::<DynamicClipmapSurfaceOverlay>();
        channel.current = Some(identity.clone());
        channel.selection = Some(ClipmapSurfaceCoverage {
            identity,
            image: self.blue.clone(),
        });
    }

    fn material(&self) -> &OverlayMaterial {
        let state = self.app.world().resource::<OverlayState>();
        self.app
            .world()
            .resource::<Assets<OverlayMaterial>>()
            .get(state.material.as_ref().unwrap())
            .unwrap()
    }
}

#[test]
fn surface_channels_share_actual_height_binding_and_one_entity_with_trail_priority() {
    let mut fixture = Fixture::new();
    fixture.publish_selection(0);
    fixture.app.update();
    let entity = fixture
        .app
        .world()
        .resource::<OverlayState>()
        .entity
        .unwrap();
    let handle = fixture
        .app
        .world()
        .resource::<OverlayState>()
        .material
        .clone()
        .unwrap();
    assert_eq!(
        fixture.material().extension.density,
        Some(fixture.blue.clone())
    );
    assert_eq!(fixture.material().extension.params.options.x, 1.0);
    let original = fixture
        .app
        .world()
        .resource::<Assets<ClipmapHeightMaterial>>()
        .get(&fixture.height_handles[0])
        .unwrap();
    assert!(
        fixture
            .material()
            .base
            .extension
            .same_height_binding(&original.extension),
        "反馈必须复制实际地形高度/页表/源框架，不能回到平面材质"
    );
    assert_eq!(
        *fixture.app.world().get::<Transform>(entity).unwrap(),
        *fixture
            .app
            .world()
            .get::<Transform>(fixture.displays[0])
            .unwrap()
    );
    assert_eq!(
        fixture.app.world().get::<ChildOf>(entity).unwrap().parent(),
        fixture.sources[0]
    );
    assert_eq!(
        *fixture.app.world().get::<RenderLayers>(entity).unwrap(),
        RenderLayers::layer(31)
    );
    assert!(
        fixture
            .app
            .world()
            .get::<NoFrustumCulling>(entity)
            .is_some()
    );
    let identity = fixture.identity(0);
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicClipmapSurfaceOverlay>()
        .trail = Some(ClipmapSurfaceCoverage {
        identity,
        image: fixture.green.clone(),
    });
    fixture.app.update();
    assert_eq!(
        fixture.material().extension.density,
        Some(fixture.green.clone())
    );
    assert_eq!(
        fixture.app.world().resource::<OverlayState>().entity,
        Some(entity)
    );
    assert_eq!(
        fixture.app.world().resource::<OverlayState>().material,
        Some(handle)
    );
    assert_eq!(
        fixture
            .app
            .world_mut()
            .query::<&ClipmapOverlayEntity>()
            .iter(fixture.app.world())
            .count(),
        1
    );
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicClipmapSurfaceOverlay>()
        .trail = None;
    fixture.app.update();
    assert_eq!(
        fixture.material().extension.density,
        Some(fixture.blue.clone())
    );
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<Assets<OverlayMaterial>>()
            .len(),
        1
    );
}

#[test]
fn same_map_target_pose_and_author_changes_reject_old_surface_channels_then_vegetation_recovers() {
    let mut fixture = Fixture::new();
    fixture.publish_selection(0);
    fixture.app.update();
    fixture.select(1);
    fixture.app.update();
    assert!(
        fixture
            .app
            .world()
            .resource::<OverlayState>()
            .entity
            .is_none(),
        "同图换网格必须拒绝旧选中层"
    );
    fixture.publish_selection(1);
    fixture.app.update();
    assert_eq!(
        fixture
            .app
            .world()
            .get::<ChildOf>(
                fixture
                    .app
                    .world()
                    .resource::<OverlayState>()
                    .entity
                    .unwrap()
            )
            .unwrap()
            .parent(),
        fixture.sources[1]
    );
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicClipmapSurfaceOverlay>()
        .current
        .as_mut()
        .unwrap()
        .author_revision += 1;
    fixture.app.update();
    assert!(
        fixture
            .app
            .world()
            .resource::<OverlayState>()
            .entity
            .is_none(),
        "迟到同层图不获得新作者信用"
    );
    fixture.publish_selection(1);
    let mut frame = *fixture
        .app
        .world()
        .get::<MapSourceFrame>(fixture.sources[1])
        .unwrap();
    frame.source_to_world.w_axis.x += 5.0;
    frame.world_to_source = frame.source_to_world.inverse();
    fixture
        .app
        .world_mut()
        .entity_mut(fixture.sources[1])
        .insert(frame);
    fixture.app.update();
    assert!(
        fixture
            .app
            .world()
            .resource::<OverlayState>()
            .entity
            .is_none(),
        "对象姿态变化必须撤销旧反馈"
    );
    fixture.publish_selection(1);
    fixture.app.update();
    assert!(
        fixture
            .app
            .world()
            .resource::<OverlayState>()
            .entity
            .is_none(),
        "新源框架不能借尚未更新的旧高度绑定"
    );
    fixture
        .app
        .world_mut()
        .resource_mut::<Assets<ClipmapHeightMaterial>>()
        .get_mut(&fixture.height_handles[1])
        .unwrap()
        .extension
        .set_source_frame(frame.source_to_world);
    fixture.app.update();
    assert!(
        fixture
            .app
            .world()
            .resource::<OverlayState>()
            .entity
            .is_some()
    );

    // 地表通道撤回后原植被输入仍能使用相同材质路径；没有清空其图片或改写作者像素。
    *fixture
        .app
        .world_mut()
        .resource_mut::<DynamicClipmapSurfaceOverlay>() = Default::default();
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicClipmapVegetationOverlay>()
        .density = Some(ClipmapVegetationDensity {
        map_id: fixture.map.clone(),
        image: fixture.vegetation.clone(),
    });
    fixture.app.update();
    assert_eq!(
        fixture.material().extension.density,
        Some(fixture.vegetation.clone())
    );
    fixture
        .app
        .world_mut()
        .resource_mut::<DynamicClipmapVegetationOverlay>()
        .density = None;
    fixture.app.update();
    assert!(
        fixture
            .app
            .world()
            .resource::<OverlayState>()
            .entity
            .is_none()
    );
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<Assets<OverlayMaterial>>()
            .len(),
        0
    );
}

#[test]
fn overlay_evidence_reads_created_material_and_rejects_stale_bindings() {
    let mut fixture = Fixture::new();
    let (revision, missing_pages) = {
        let snapshot = fixture.app.world()
            .resource::<InstanceResourceTable<DynamicClipmapGroundSurface>>()
            .values.get(&fixture.sources[0]).unwrap().snapshot.as_ref().unwrap();
        (snapshot.revision, snapshot.missing_pages)
    };
    fixture.app.world_mut().resource_mut::<DynamicClipmapVegetationOverlay>().brush = Some(ClipmapVegetationBrush {
        map_id: fixture.map.clone(), revision, missing_pages,
        center: Vec2::new(7.0, 11.0), radius: 3.5, strength: 0.6,
        color: LinearRgba::new(0.2, 0.8, 0.4, 1.0),
    });
    fixture.app.update(); // 原生产系统真实创建唯一覆盖实体和材质。
    let before = clipmap_brush_overlay_evidence(fixture.app.world()).unwrap();
    assert!(before.bound && before.render_layer);
    assert!(!before.visible, "无窗口夹具没有真实视图可见性回执，不能捏造GPU成功");
    assert_eq!(before.source_entity, Some(fixture.displays[0].to_bits()));
    assert_eq!(before.parent, Some(fixture.sources[0].to_bits()));
    assert_eq!(before.brush, [7.0, 11.0, 3.5, 0.6]);
    let handle = fixture.app.world().resource::<OverlayState>().material.clone().unwrap();
    fixture.app.world_mut().resource_mut::<Assets<OverlayMaterial>>()
        .get_mut(&handle).unwrap().extension.params.brush.z = 9.0;
    let changed = clipmap_brush_overlay_evidence(fixture.app.world()).unwrap();
    assert_eq!(changed.brush[2], 9.0, "证据必须读取材质，不能复述仍为3.5的输入半径");
    assert_ne!(serde_json::to_value(&before).unwrap(), serde_json::to_value(&changed).unwrap());
    fixture.app.world_mut().resource_mut::<Assets<ClipmapHeightMaterial>>()
        .get_mut(&fixture.height_handles[0]).unwrap().extension.set_source_frame(Mat4::IDENTITY);
    assert!(!clipmap_brush_overlay_evidence(fixture.app.world()).unwrap().bound,
        "实际高度绑定改变而覆盖尚未同步时，旧材质不能获得bound信用");
}
