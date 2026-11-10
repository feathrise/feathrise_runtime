//! 显式小工程验收的真实可视与碰撞快照；正常游戏不执行，也不增加作者依赖。

#[path = "smoke_render_mesh_counts.rs"]
mod render_counts;

use bevy::{ecs::system::SystemState, mesh::VertexAttributeValues, prelude::*};
use feathrise_scene::{
    DynamicWaterStatus, ScenePlacementInstance, TerrainObjectQueries, TerrainObjectResource,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) fn enabled() -> bool {
    std::env::var("NEW_EDITOR_NATIVE_SMOKE_AUTHOR_TOOLS").as_deref() == Ok("1")
}

pub(super) fn install(app: &mut App) -> Result<(), String> {
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
    render_counts::ready(world, &meshes)
}

/// 专用模板只有1089地形顶点及少量三角模型；超过上限明确拒绝，不在玩法帧做全场景摘要。
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
    // 正式玩法可合法显示简化LOD；作者内容指纹读取同显示节点保留的原始来源，另记实际显示规模。
    let display_vertices = world
        .resource::<Assets<Mesh>>()
        .get(&mesh)
        .and_then(|mesh| mesh.try_attribute(Mesh::ATTRIBUTE_POSITION).ok())
        .map(VertexAttributeValues::len);
    let source_mesh = world
        .query::<(Entity, &Mesh3d, Option<&feathrise_scene::StaticMeshLodOriginal>)>()
        .iter(world)
        .find(|(child, handle, _)| handle.id() == mesh.id() && belongs_to(world, *child, entity))
        .map(|(_, handle, original)| {
            original.map_or_else(|| handle.0.clone(), |original| original.mesh.clone())
        })
        .ok_or("活动地形没有对应的显示节点/原始网格来源")?;
    let mesh = world
        .resource::<Assets<Mesh>>()
        .get(&source_mesh)
        .ok_or("地形可视网格尚未加载")?;
    let VertexAttributeValues::Float32x3(positions) = mesh
        .try_attribute(Mesh::ATTRIBUTE_POSITION)
        .map_err(|error| format!("地形原始源顶点不可读：{error}"))?
    else {
        return Err("验收地形顶点格式不符".into());
    };
    let indices = mesh
        .try_indices_option()
        .map_err(|error| format!("地形原始源索引不可读：{error}"))?;
    let index_count = indices.map_or(0, |indices| indices.len());
    let vertex_count = positions.len();
    if positions.len() > 2048 || index_count > 8192 {
        return Err("专业验收地形超出有限快照预算".into());
    }
    let mut digest = Sha256::new();
    for vertex in positions {
        for value in vertex {
            digest.update(value.to_le_bytes());
        }
    }
    if let Some(indices) = indices {
        for index in indices.iter() {
            digest.update((index as u32).to_le_bytes());
        }
    }
    let mesh_sha256 = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
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
    let mut rendered = Vec::new();
    for (entity, id) in roots {
        let building = records.iter().find(|record| record.id == id).map(|record| json!({
            "record":record,
            "matches_saved_record":world.get::<ScenePlacementInstance>(entity).is_some_and(|instance| instance.matches_record(&map, record, &[])),
            "position":world.get::<GlobalTransform>(entity).map(|transform| transform.translation().to_array()),
        }));
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
        json!({"terrain_id":terrain.as_str(),"mesh_sha256":mesh_sha256,"vertices":vertex_count,
        "indices":index_count,"display_vertices":display_vertices,"collision_height_at_sculpt_center":collision_height,
        "water":water,"river_surfaces":rivers,"rendered_objects":rendered}),
    )
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
