//! 显式小工程验收的真实可视与碰撞快照；正常游戏不执行，也不增加作者依赖。

#[path = "smoke_render_mesh_counts.rs"]
mod render_counts;
#[path = "smoke_author_height/mod.rs"]
mod height_source;

use bevy::{ecs::system::SystemState, mesh::VertexAttributeValues, prelude::*};
use feathrise_scene::{
    DynamicWaterStatus, ScenePlacementInstance, TerrainObjectQueries, TerrainObjectResource,
};
use serde_json::{Value, json};

pub(super) fn enabled() -> bool {
    std::env::var("NEW_EDITOR_NATIVE_SMOKE_AUTHOR_TOOLS").as_deref() == Ok("1")
}

pub(super) fn install(app: &mut App) -> Result<(), String> {
    height_source::install(app);
    render_counts::install(app)
}

/// 水网格允许上传后释放CPU属性；先等待真实RenderMesh的有界元数据回执。
pub(super) fn render_ready(world: &mut World) -> bool {
    let map = world.resource::<super::SavedGameStartup>().scene_id.clone();
    let meshes = world
        .query::<(&feathrise_scene::PaintedWaterSurface, &Mesh3d)>()
        .iter(world)
        .filter(|(surface, _)| {
            surface.scene_id == map
                && surface.purpose == feathrise_scene::PaintedWaterPurpose::Runtime
                && surface.kind == feathrise_scene::WaterSurfaceKind::River
        })
        .take(33)
        .map(|(_, mesh)| mesh.id())
        .collect::<Vec<_>>();
    let water_ready = render_counts::ready(world, &meshes);
    height_source::ready(world) && water_ready
}

/// 几何只读取正式显示规模；完整作者指纹来自此前后台完成的真实分页源。
pub(super) fn snapshot(world: &mut World) -> Result<Value, String> {
    let map = world.resource::<super::SavedGameStartup>().scene_id.clone();
    let terrain = feathrise_scene_content::TerrainId::parse("starter-ground")?;
    let mut queries = SystemState::<TerrainObjectQueries>::new(world);
    let query = queries.get(world).map_err(|error| error.to_string())?;
    let entity = query
        .instance(&map, &terrain)
        .ok_or("专业验收地形没有活动实例")?;
    let (mesh, _, _) = query
        .render_mesh(&map, &terrain)
        .ok_or("专业验收地形没有正式可视网格")?;
    let (_, collider) = query
        .snapshot(entity, &map)
        .ok_or("专业验收地形没有真实碰撞表面")?;
    let collision_height = collider
        .support_below(Vec3::new(-8., 100., -8.), 200.)
        .map(|point| point.y);
    // Clipmap的显示模板通过GPU位移，平面顶点Y不代表作者高度，不能继续按旧Heightfield散列Mesh。
    let display_vertices = world
        .resource::<Assets<Mesh>>()
        .get(&mesh)
        .and_then(|mesh| mesh.try_attribute(Mesh::ATTRIBUTE_POSITION).ok())
        .map(VertexAttributeValues::len);
    let (height_source_path, height_source_sha256) = height_source::fingerprint(world)?;
    let mut water_access = SystemState::<TerrainObjectResource<DynamicWaterStatus>>::new(world);
    let waters = water_access.get(world).map_err(|error| error.to_string())?;
    let water = waters
        .for_saved_object(&map, &terrain)
        .filter(|(active, _)| *active == entity)
        .map(|(_, state)| {
            json!({"phase":format!("{:?}",state.phase),"layers":state.layer_count,
            "triangles":state.triangle_count,"desired_generation":state.desired_generation,
            "applied_generation":state.applied_generation,"detail":state.detail})
        });
    let roots = world
        .query::<(Entity, &ScenePlacementInstance)>()
        .iter(world)
        .filter(|(_, instance)| {
            instance.scene_id() == &map
                && instance.is_active()
                && !instance.asset_target().is_builtin()
        })
        .take(32)
        .map(|(entity, instance)| (entity, instance.id().to_owned()))
        .collect::<Vec<_>>();
    let records = world
        .get_resource::<feathrise_scene_content::SceneContentRegistry>()
        .and_then(|registry| registry.handles(&map))
        .and_then(|handles| {
            world
                .resource::<Assets<feathrise_scene_content::ScenePlacementsAsset>>()
                .get(&handles.placements)
        })
        .map(|asset| {
            asset
                .placements
                .iter()
                .filter(|record| record.building_preset_id.is_some())
                .take(16)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // 上方World可变查询已经完成，再取得正式只读视图；不能让早期碰撞视图跨越这些借用。
    let query = queries.get(world).map_err(|error| error.to_string())?;
    let mut rendered = Vec::new();
    for (entity, id) in roots {
        let building = records.iter().find(|record| record.id == id).map(|record| {
            // 正式根冻结的是承托关联解析后的姿态；GLB节点的缓存世界TRS不能冒充同一比较空间。
            let pose = record.terrain_binding.as_ref().filter(|_| record.terrain_patch.is_none())
                .map(|binding| query.binding_pose(&map, None, binding)).transpose()?;
            let resolved = resolved_building_record(record, pose);
            Ok::<_, String>(building_snapshot(world, entity, &map, record, &resolved))
        }).transpose()?;
        rendered.push(json!({"id":id,"building":building,"meshes":object_meshes(world, entity)?}));
    }
    let rivers = world.query::<(Entity, &feathrise_scene::PaintedWaterSurface, &Mesh3d)>().iter(world)
        .filter(|(_, surface, _)| surface.scene_id == map && surface.purpose == feathrise_scene::PaintedWaterPurpose::Runtime
            && surface.kind == feathrise_scene::WaterSurfaceKind::River)
        .filter(|(surface_entity, _, _)| belongs_to(world, *surface_entity, entity))
        .take(32)
        .filter_map(|(_, surface, mesh)| render_counts::get(world, mesh.id())
            .map(|counts| json!({"id":surface.layer_id,"vertices":counts.vertices,"triangles":counts.indices/3,"geometry_evidence":"RenderAssets<RenderMesh>"})))
        .collect::<Vec<_>>();
    Ok(
        json!({"terrain_id":terrain.as_str(),"height_source_sha256":height_source_sha256,
        "height_source_path":height_source_path,"geometry_evidence":"SparseHeightDocument spec, tile keys and height codes",
        "display_vertices":display_vertices,"collision_height_at_sculpt_center":collision_height,
        "water":water,"river_surfaces":rivers,"rendered_objects":rendered}),
    )
}

fn resolved_building_record(
    record: &feathrise_scene_content::ScenePlacementRecord,
    pose: Option<feathrise_scene_content::TerrainObjectTransform>,
) -> feathrise_scene_content::ScenePlacementRecord {
    let mut resolved = record.clone();
    if let Some(pose) = pose {
        resolved.translation = pose.translation;
        resolved.yaw_degrees = pose.yaw_degrees;
        resolved.scale = pose.scale;
    }
    resolved
}

fn building_snapshot(world: &World, entity: Entity, map: &feathrise_scene_content::SceneId,
    record: &feathrise_scene_content::ScenePlacementRecord,
    resolved: &feathrise_scene_content::ScenePlacementRecord) -> Value {
    let expected = feathrise_scene_content::TerrainObjectTransform {
        translation: resolved.translation, yaw_degrees: resolved.yaw_degrees, scale: resolved.scale, ..default()
    }.as_transform();
    let expected_world = world.get::<ChildOf>(entity)
        .and_then(|parent| world.get::<GlobalTransform>(parent.parent()))
        .map_or_else(|| GlobalTransform::from(expected), |parent| parent.mul_transform(expected));
    let local = world.get::<Transform>(entity);
    let global = world.get::<GlobalTransform>(entity);
    json!({
        "record":record,"resolved_record":resolved,
        // 保留生产精确门，不能用容差把旧根或错误资产认作当前保存记录。
        "matches_saved_record":world.get::<ScenePlacementInstance>(entity)
            .is_some_and(|instance| instance.matches_record(map, resolved, &[])),
        "pose_matches_resolved_record":local.is_some_and(|local| building_pose_matrix_matches(local.to_matrix(), expected.to_matrix()))
            && global.is_some_and(|global| building_pose_matrix_matches(global.to_matrix(), expected_world.to_matrix())),
        "position":global.map(|transform| transform.translation().to_array()),
        "local_matrix":local.map(|transform| transform.to_matrix().to_cols_array()),
        "world_matrix":global.map(|transform| transform.to_matrix().to_cols_array()),
    })
}

/// 只接受Avian有限次TRS分解带来的基向量舍入，平移与仿射项仍须精确相同。
fn building_pose_matrix_matches(actual: Mat4, expected: Mat4) -> bool {
    if !actual.is_finite() || !expected.is_finite() || actual.w_axis != expected.w_axis {
        return false;
    }
    (0..3).all(|column| {
        let actual_axis = actual.col(column);
        let expected_axis = expected.col(column);
        if actual_axis.w != expected_axis.w {
            return false;
        }
        // 正式静态建筑经init_physics_transform分解，再经position_to_transform重新归父；
        // 四元数不能逐位等于作者值。8*EPSILON按每根期望轴长计，不借用Avian的0.1度同步阈值，
        // 也不把小模型的预算抬到单位尺寸；f64只避免误差长度计算本身溢出或下溢。
        let expected_basis = expected_axis.truncate().as_dvec3();
        let error = (actual_axis.truncate().as_dvec3() - expected_basis).length();
        error <= f64::from(8.0 * f32::EPSILON) * expected_basis.length()
    })
}

fn belongs_to(world: &World, mut entity: Entity, target: Entity) -> bool {
    for _ in 0..32 {
        if entity == target {
            return true;
        }
        let Some(parent) = world.get::<ChildOf>(entity) else {
            return false;
        };
        entity = parent.parent();
    }
    false
}

fn object_meshes(world: &World, root: Entity) -> Result<Vec<Value>, String> {
    let mut stack = vec![root];
    let mut visited = 0;
    let mut result = Vec::new();
    while let Some(entity) = stack.pop() {
        visited += 1;
        if visited > 256 {
            return Err("验收模型子树超出256节点预算".into());
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
        let Some(mesh) = world
            .get::<Mesh3d>(entity)
            .and_then(|handle| world.resource::<Assets<Mesh>>().get(handle.id()))
        else {
            continue;
        };
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.try_attribute(Mesh::ATTRIBUTE_POSITION).ok()
        else {
            continue;
        };
        // 小模型只记录原始顶点；大模型不复制全量，避免把骨骼附件或其他工程网格带进有限证据。
        if positions.len() > 64 {
            continue;
        }
        let material = world
            .get::<MeshMaterial3d<StandardMaterial>>(entity)
            .and_then(|handle| {
                world
                    .resource::<Assets<StandardMaterial>>()
                    .get(handle.id())
            });
        let image = material
            .and_then(|material| material.base_color_texture.as_ref())
            .and_then(|handle| world.resource::<Assets<Image>>().get(handle.id()));
        result.push(json!({"positions":positions,
            "base_color_linear":material.map(|material| material.base_color.to_linear().to_f32_array()),
            "roughness":material.map(|material| material.perceptual_roughness),
            "image_size":image.map(|image| [image.width(),image.height()]),
            "first_pixel":image.and_then(|image| image.data.as_ref()).map(|data| data.iter().take(4).copied().collect::<Vec<_>>())}));
        if result.len() > 32 {
            return Err("验收模型可视网格超出32项预算".into());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_building_snapshot_accepts_real_physics_rounding_and_rejects_changed_pose_or_source() {
        use avian3d::prelude::{PhysicsPlugins, Position, Rotation};

        let map = feathrise_scene_content::SceneId::parse("M1-building").unwrap();
        let mut source: feathrise_scene_content::ScenePlacementRecord = serde_json::from_value(json!({
            "id":"building", "asset_path":"scene/probe.glb", "building_preset_id":"sample-building",
            "translation":[-16.,0.,-16.], "yaw_degrees":90., "scale":[1.,1.,1.],
            "terrain_binding":{"terrain_id":"starter-ground", "local_pose":{
                "translation":[-16.,0.,-16.], "yaw_degrees":90., "scale":[1.,1.,1.]
            }}
        })).unwrap();
        source.scale[0] = f32::from_bits(1.0f32.to_bits() + 1); // 保存节点的缓存世界TRS与关联局部姿态分属两个空间。
        let pose = source.terrain_binding.as_ref().unwrap().resolve(Default::default()).unwrap();
        let resolved = resolved_building_record(&source, Some(pose));
        let instance = ScenePlacementInstance::new(&map, &resolved);
        assert!(!instance.matches_record(&map, &source, &[]), "复现旧验收比较了错误空间");
        let local = pose.as_transform();
        // 与正式建筑相同地插入静态碰撞组合，必须经过真实物理钩子、固定更新与父变换传播，
        // 不能用手写近似四元数或直接复制作者GlobalTransform替代这次回归。
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default(), TransformPlugin,
            bevy::mesh::MeshPlugin, PhysicsPlugins::default()))
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f64(1.0 / 60.0)));
        app.finish();
        app.cleanup();
        let parent_pose = Transform::from_xyz(3., 2., 1.);
        let parent = app.world_mut().spawn(parent_pose).id();
        let root = app.world_mut().spawn((instance, local, ChildOf(parent))).id();
        app.world_mut().entity_mut(root).insert(feathrise_scene::building_placement_collision(&resolved).unwrap());
        for _ in 0..4 { app.update(); }
        let world = app.world_mut();
        assert!(world.get::<Position>(root).is_some() && world.get::<Rotation>(root).is_some());
        assert_ne!(world.get::<Transform>(root).unwrap().rotation, local.rotation,
            "真实Avian父建筑路径必须复现使旧逐位姿态比较失败的舍入");
        let proof = building_snapshot(&world, root, &map, &source, &resolved);
        assert_eq!(proof["matches_saved_record"], true);
        assert_eq!(proof["pose_matches_resolved_record"], true);
        assert_eq!(proof["record"], serde_json::to_value(&source).unwrap());
        assert_eq!(proof["resolved_record"], serde_json::to_value(&resolved).unwrap());

        let actual_local = *world.get::<Transform>(root).unwrap();
        let parent_global = *world.get::<GlobalTransform>(parent).unwrap();
        let assert_changed_pose_fails = |world: &mut World, changed: Transform, reason: &str| {
            world.entity_mut(root).insert((changed, parent_global.mul_transform(changed)));
            let proof = building_snapshot(world, root, &map, &source, &resolved);
            assert_eq!(proof["matches_saved_record"], true, "姿态证据不得改写实例身份");
            assert_eq!(proof["pose_matches_resolved_record"], false, "{reason}");
        };
        let mut changed = actual_local;
        changed.translation.x = f32::from_bits(changed.translation.x.to_bits() + 1);
        assert_changed_pose_fails(world, changed, "平移变化一个f32步长也必须失败");
        let mut changed = actual_local;
        changed.rotation = Quat::from_rotation_y(0.001f32.to_radians()) * changed.rotation;
        assert_changed_pose_fails(world, changed, "0.001度的真实转动不能当作分解舍入");
        let mut changed = actual_local;
        changed.scale.x += 1e-4;
        assert_changed_pose_fails(world, changed, "真实缩放变化不能当作分解舍入");
        world.entity_mut(root).insert((actual_local, parent_global.mul_transform(actual_local)));
        let mut wrong_record = resolved.clone();
        wrong_record.asset_path = "scene/other.glb".into();
        assert_eq!(building_snapshot(&world, root, &map, &source, &wrong_record)["matches_saved_record"], false,
            "姿态解析不能放宽稳定资产身份门");
        let mut wrong_record = resolved.clone();
        wrong_record.building_preset_id = Some("other-building".into());
        assert_eq!(building_snapshot(&world, root, &map, &source, &wrong_record)["matches_saved_record"], false,
            "建筑metadata仍精确比较");
        let mut wrong_source = source.clone();
        wrong_source.terrain_binding.as_mut().unwrap().local_pose.translation[0] =
            f32::from_bits((-16.0f32).to_bits() + 1);
        // 生产实例冻结的是关联解析后的TRS，不保存原始binding；反例须先走同一正式解析，
        // 不能只改尚未消费的metadata后要求实例门拒绝。原source完整性另由宿主精确比较。
        let wrong_pose = wrong_source.terrain_binding.as_ref().unwrap().resolve(Default::default()).unwrap();
        let wrong_record = resolved_building_record(&wrong_source, Some(wrong_pose));
        assert_ne!(wrong_record.translation, resolved.translation);
        let proof = building_snapshot(&world, root, &map, &wrong_source, &wrong_record);
        assert_eq!(proof["matches_saved_record"], false,
            "承托来源解析出一个f32步长的实际变化时不能复用原实例");
        assert_eq!(proof["pose_matches_resolved_record"], false,
            "实际根仍位于旧承托位置，平移必须精确拒绝");
    }

    #[test]
    fn building_pose_matrix_rounding_is_relative_and_affine_entries_stay_exact() {
        let expected = Mat4::from_scale(Vec3::splat(0.001));
        let mut rounded = expected;
        rounded.x_axis.x *= 1.0 + 4.0 * f32::EPSILON;
        assert!(building_pose_matrix_matches(rounded, expected));
        rounded.x_axis.x = expected.x_axis.x * (1.0 + 32.0 * f32::EPSILON);
        assert!(!building_pose_matrix_matches(rounded, expected), "小模型不得沿用单位尺寸误差预算");
        let mut changed = expected;
        changed.w_axis.x = f32::from_bits(1);
        assert!(!building_pose_matrix_matches(changed, expected), "平移没有舍入预算");
        changed = expected;
        changed.x_axis.w = f32::from_bits(1);
        assert!(!building_pose_matrix_matches(changed, expected), "仿射末行没有舍入预算");
        changed = expected;
        changed.z_axis.z = f32::NAN;
        assert!(!building_pose_matrix_matches(changed, expected));
    }
}
