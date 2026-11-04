//! 静水层逐帧准备，以共享发布状态和隐藏父根一次切换整批渲染与互动。
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
mod gpu_tests; // 真实PBR回执必须覆盖隐藏独立实例，纯ECS立即放行不能证明预热完成。

pub(super) struct StaticBatch {
    pub root: Entity,
    pub published: Arc<AtomicBool>,
    pub layers: VecDeque<InstalledStaticLayer>,
    pub warmups: VecDeque<Entity>,
    pub height_revision: [u64; 3],
    pub ticket: Option<WaterGpuUploadTicket>,
}

impl StaticBatch {
    pub fn new(commands: &mut Commands, height_revision: [u64; 3]) -> Self {
        Self {
            root: commands
                .spawn((
                    Name::new("Clipmap静水完整批次"),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    DespawnOnExit(ScenePhase::Active),
                ))
                .id(),
            published: Arc::new(AtomicBool::new(false)),
            layers: VecDeque::new(),
            warmups: VecDeque::new(),
            height_revision,
            ticket: None,
        }
    }

    pub fn publish(&self, commands: &mut Commands) {
        self.published.store(true, Ordering::Release);
        commands.entity(self.root).insert(Visibility::Inherited);
    }

    pub fn retire(
        self,
        commands: &mut Commands,
        runtime: &mut ClipmapWaterRuntime,
        uploads: &mut WaterGpuUploadQueue,
    ) {
        self.published.store(false, Ordering::Release);
        commands.entity(self.root).try_insert(Visibility::Hidden);
        if let Some(ticket) = self.ticket {
            uploads.retire(ticket);
        }
        runtime.static_retired.push_back(self); // 移交所有权，不能在主帧遍历任意数量的旧层。
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::maps::dynamic_map::clipmap::{
        ClipmapBounds, ClipmapFlatHeightPage, test_ground_snapshot,
    };
    use bevy::ecs::world::CommandQueue;

    #[test]
    fn static_water_batch_stages_beyond_eight_layers_and_publishes_all_at_once() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        crate::shared::large_map::install_water_retirement(&mut app);
        let ground = test_ground_snapshot();
        let bounds = (Vec2::ZERO, Vec2::new(96., 64.));
        let height = ClipmapHeightExtension::new_flat(
            ClipmapBounds::new(bounds.0, bounds.1).unwrap(),
            UVec2::new(96, 64),
            &ClipmapFlatHeightPage {
                heights: default(),
                page_table: default(),
            },
        )
        .unwrap();
        let mut meshes = Assets::<Mesh>::default();
        let mesh = Mesh3d(meshes.add(Cuboid::new(1., 1., 1.)));
        let mut materials = Assets::<ClipmapWaterMaterial>::default();
        let mut queue = CommandQueue::default();
        let mut batch =
            StaticBatch::new(&mut Commands::new(&mut queue, app.world()), ground.revision);
        for i in 0..12 {
            let layer: MapWaterLayer = serde_json::from_value(serde_json::json!({
                "id":format!("layer-{i}"), "label":"测试静水", "mask_source_path":"test.r8",
                "mask_texture_path":"test.ktx2", "resolution":[2,2], "water_level":20.
            }))
            .unwrap();
            let pending = PendingLayer {
                image: Some(default()),
                prepared: PreparedLayer {
                    descriptor: WaterSurfaceDescriptor::from_layer(&layer),
                    layer,
                    pixels: Arc::from(vec![255; 4]),
                    coverage_bounds: Some([0, 0, 2, 2]),
                    draft_image: None,
                },
            };
            stage_layer(
                &mut Commands::new(&mut queue, app.world()),
                &mut materials,
                &mut batch,
                &pending,
                &ground.map_id,
                bounds,
                &ground,
                &mesh,
                Transform::IDENTITY,
                &height,
                0.,
            );
            queue.apply(app.world_mut());
            assert_eq!(batch.layers.len(), i + 1);
            for layer in &batch.layers {
                assert_eq!(
                    app.world()
                        .get::<ClipmapWaterSurface>(layer.entity)
                        .unwrap()
                        .coverage_at(Vec2::ONE),
                    0.
                );
            }
        }
        batch.publish(&mut Commands::new(&mut queue, app.world()));
        queue.apply(app.world_mut());
        for layer in &batch.layers {
            assert_eq!(
                app.world()
                    .get::<ClipmapWaterSurface>(layer.entity)
                    .unwrap()
                    .coverage_at(Vec2::ONE),
                1.
            );
        }
        let root = batch.root;
        let old_surface = app
            .world()
            .get::<ClipmapWaterSurface>(batch.layers[0].entity)
            .unwrap()
            .clone();
        let mut runtime = ClipmapWaterRuntime::default();
        batch.retire(
            &mut Commands::new(&mut queue, app.world()),
            &mut runtime,
            &mut WaterGpuUploadQueue::default(),
        );
        queue.apply(app.world_mut());
        assert_eq!(old_surface.coverage_at(Vec2::ONE), 0.); // 缓存中的同批足迹也立即失去授权。
        assert_eq!(
            *app.world().get::<Visibility>(root).unwrap(),
            Visibility::Hidden
        );
        app.insert_resource(runtime)
            .insert_resource(meshes)
            .insert_resource(materials)
            .add_systems(First, reap);
        for remaining in (0..12).rev() {
            app.update();
            assert_eq!(
                app.world().resource::<Assets<ClipmapWaterMaterial>>().len(),
                remaining
            );
        }
        app.update();
        assert!(app.world().get_entity(root).is_err());
        assert!(
            app.world()
                .resource::<Assets<Mesh>>()
                .get(&mesh.0)
                .is_some()
        ); // 共享地形环带绝不被水体回收。
    }
}

pub(super) fn stage_layer(
    commands: &mut Commands,
    materials: &mut Assets<ClipmapWaterMaterial>,
    batch: &mut StaticBatch,
    layer: &PendingLayer,
    map_id: &MapId,
    world_bounds: (Vec2, Vec2),
    snapshot: &crate::maps::dynamic_map::ClipmapGroundSnapshot,
    mesh: &Mesh3d,
    transform: Transform,
    height: &ClipmapHeightExtension,
    elapsed: f32,
) {
    let prepared = &layer.prepared;
    let minimum_depth = prepared.descriptor.minimum_stable_depth();
    let material = materials.add(ClipmapWaterMaterial {
        base: create_water_surface_material(&prepared.descriptor, elapsed),
        extension: ClipmapWaterExtension::new(
            height,
            layer.image.clone().expect("完整静水候选持有遮罩句柄"),
            prepared.layer.water_level,
            minimum_depth,
            snapshot.max_mip(),
            snapshot.missing_pages,
        ),
    });
    let surface = ClipmapWaterSurface {
        map_id: map_id.clone(),
        layer_id: prepared.layer.id.clone(),
        resolution: prepared.layer.resolution,
        world_min: world_bounds.0,
        world_max: world_bounds.1,
        water_level: prepared.layer.water_level,
        minimum_depth,
        height_revision: snapshot.revision,
        coverage_bounds: prepared.coverage_bounds.expect("后台已过滤空层"),
        mask: prepared.pixels.clone(),
        volume: false,
        published: Some(batch.published.clone()),
    };
    let interaction = DynamicWaterSurface::from_clipmap(
        &surface,
        snapshot,
        prepared.descriptor.flow_direction,
        prepared.descriptor.flow_speed,
    );
    let entity = commands
        .spawn((
            Name::new(format!("Clipmap静水_{}", prepared.layer.id)),
            PaintedWaterSurface {
                map_id: map_id.clone(),
                layer_id: prepared.layer.id.clone(),
                kind: WaterSurfaceKind::Still,
                purpose: PaintedWaterPurpose::Runtime,
            },
            surface,
            interaction,
            DynamicWaterCpuInteraction,
            mesh.clone(),
            transform,
            MeshMaterial3d(material.clone()),
            Visibility::Inherited,
            ChildOf(batch.root),
            NoFrustumCulling,
            NotShadowCaster,
            CameraRaycastIgnore,
        ))
        .id();
    let warmup = commands
        .spawn((
            feathrise_scene_core::residency::MapInstanceDetached, // 保留实例归属与退役登记，但避开隐藏候选父链；否则候选就绪与材质预热互相等待。
            mesh.clone(),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(0., -100_000., 0.).with_scale(Vec3::splat(0.000_001)),
            Visibility::Inherited,
            NoFrustumCulling,
            NotShadowCaster,
            CameraRaycastIgnore,
            DespawnOnExit(ScenePhase::Active),
        ))
        .id();
    batch
        .layers
        .push_back(InstalledStaticLayer { entity, material });
    batch.warmups.push_back(warmup);
}

/// 隐藏作者层只供应同代R8和PBR；三维水面运行时按实际BVH构建、预热后发布几何。
pub(super) fn stage_density_layer(commands:&mut Commands,materials:&mut Assets<ClipmapWaterMaterial>,batch:&mut StaticBatch,
    layer:&PendingLayer,map_id:&MapId,bounds:(Vec2,Vec2),snapshot:&crate::ClipmapGroundSnapshot,
    fallback:&feathrise_environment::terrain_surface::TerrainSurfaceFallbackImages,elapsed:f32) {
    let prepared=&layer.prepared;
    let minimum_depth=prepared.descriptor.minimum_stable_depth();
    let material=materials.add(ClipmapWaterMaterial {base:create_water_surface_material(&prepared.descriptor,elapsed),
        extension:ClipmapWaterExtension::for_density(layer.image.clone().expect("完整静水遮罩"),bounds,prepared.layer.water_level,minimum_depth,fallback)});
    let surface=ClipmapWaterSurface {map_id:map_id.clone(),layer_id:prepared.layer.id.clone(),resolution:prepared.layer.resolution,
        world_min:bounds.0,world_max:bounds.1,water_level:prepared.layer.water_level,minimum_depth,height_revision:snapshot.revision,
        coverage_bounds:prepared.coverage_bounds.expect("非空静水层"),mask:prepared.pixels.clone(),volume:false,published:Some(batch.published.clone())};
    let interaction=DynamicWaterSurface::from_clipmap(&surface,snapshot,prepared.descriptor.flow_direction,prepared.descriptor.flow_speed);
    let entity=commands.spawn((Name::new(format!("原生静水作者层_{}",prepared.layer.id)),surface,interaction,MeshMaterial3d(material.clone()),
        Transform::IDENTITY,Visibility::Hidden,ChildOf(batch.root))).id();
    batch.layers.push_back(InstalledStaticLayer {entity,material});
}

/// 与地图状态无关地推进旧批次回收；共享地形环带的Mesh绝不属于水层所有权。
pub(crate) fn reap(
    mut commands: Commands,
    mut runtime: ResMut<ClipmapWaterRuntime>,
    mut materials: ResMut<Assets<ClipmapWaterMaterial>>,
) {
    let Some(batch) = runtime.static_retired.front_mut() else {
        return;
    };
    if let Some(warmup) = batch.warmups.pop_front() {
        commands.entity(warmup).try_despawn();
    }
    if let Some(layer) = batch.layers.pop_front() {
        commands.queue(move |world: &mut World| {
            if let Ok(mut entity) = world.get_entity_mut(layer.entity) {
                let old = (
                    entity.take::<ClipmapWaterSurface>(),
                    entity.take::<DynamicWaterSurface>(),
                );
                entity.despawn();
                crate::shared::large_map::retire_water_cpu_in_world(world, old);
            }
        });
        materials.remove(layer.material.id());
    } else if batch.warmups.is_empty() {
        commands.entity(batch.root).try_despawn();
        runtime.static_retired.pop_front();
    }
}
