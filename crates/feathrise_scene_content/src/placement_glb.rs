//! 场景摆放 GLB 的读写。节点变换和真实网格是 Blender 与 Feathrise 共用的作者源。

use std::collections::{HashMap, HashSet};

use bevy::prelude::{Mat4, Quat, Vec3};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    SCENE_SCHEMA_VERSION, SceneId, ScenePlacementAssetTarget,
    ScenePlacementRecord, ScenePlacementsAsset,
};

const JSON_CHUNK: [u8; 4] = *b"JSON";
const BIN_CHUNK: [u8; 4] = *b"BIN\0";
const SCENE_ID: &str = "feathrise_scene_id";
const SCENE_VERSION: &str = "feathrise_scene_version";
const GEOMETRY_HASH: &str = "feathrise_geometry_hash";
const PLACEMENT_ID: &str = "feathrise_id";
const COLLISION: &str = "feathrise_collision";
const PLACEMENT_FIELDS: &str = "feathrise_placement";
const BUILTIN_RECIPE: &str = "feathrise_builtin";
const BUILTIN_MESH: &str = "feathrise_builtin_mesh";
const LIGHTS: &str = "KHR_lights_punctual";

/// 运行时、作者保存与打包检查共享同一读法，旧工程仍可读取原摆放文档。
pub fn read_scene_placements(bytes: &[u8], asset_path: &str) -> Result<ScenePlacementsAsset, String> {
    if !asset_path.ends_with(".glb") {
        return serde_json::from_slice(bytes).map_err(|error| error.to_string());
    }
    let (document, bin) = read_chunks(bytes)?;
    let nodes = array(&document, "nodes")?;
    let scenes = array(&document, "scenes")?;
    let scene_index = document.get("scene").map(index).transpose()?.unwrap_or(0);
    let scene = scenes.get(scene_index).ok_or("场景 GLB 缺少默认场景")?;
    let roots = array(scene, "nodes")?;
    let marker = roots
        .iter()
        .map(index)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|node| nodes.get(node)?.get("extras"))
        .filter(|extras| extras.get(SCENE_ID).is_some())
        .collect::<Vec<_>>();
    let [marker] = marker.as_slice() else {
        return Err("场景 GLB 必须有一个带场景 ID 的根空节点".into());
    };
    let scene_id = SceneId::parse(marker[SCENE_ID].as_str().ok_or("GLB 场景 ID 无效")?)?;
    if marker[SCENE_VERSION].as_u64() != Some(u64::from(SCENE_SCHEMA_VERSION)) {
        return Err(format!("场景 {scene_id} 的 GLB 格式版本无效"));
    }
    let preserve_recipes = marker[GEOMETRY_HASH]
        .as_str()
        .is_some_and(|expected| expected == geometry_hash(&document, bin));
    let mut ids = HashSet::new();
    let mut placements = Vec::new();
    let mut recipes = HashMap::new();
    visit_nodes(nodes, roots, |node, world| {
        if let Some(record) = read_node(&document, node, world, asset_path, preserve_recipes, &mut recipes)? {
            if !ids.insert(record.id.clone()) {
                return Err(format!("场景 {scene_id} 的摆放 ID {} 重复", record.id));
            }
            placements.push(record);
        }
        Ok(())
    })?;
    let result = ScenePlacementsAsset { schema_version: SCENE_SCHEMA_VERSION, scene_id, placements };
    result.validate_courtyards()?;
    Ok(result)
}

/// 主场景会把层级摊平，保留 Blender 灯光与读取作者摆放必须使用同一组世界矩阵。
fn visit_nodes<'a>(nodes: &'a [Value], roots: &[Value], mut visit: impl FnMut(&'a Value, Mat4) -> Result<(), String>) -> Result<(), String> {
    let mut stack = roots.iter().rev().map(|node| Ok((index(node)?, Mat4::IDENTITY))).collect::<Result<Vec<_>, String>>()?;
    let mut visited = HashSet::new();
    while let Some((i, parent)) = stack.pop() {
        if !visited.insert(i) { return Err("场景 GLB 的节点重复或形成环".into()); }
        let node = nodes.get(i).ok_or("场景 GLB 的节点索引越界")?;
        let world = parent * node_transform(node)?;
        if let Some(children) = node.get("children") {
            for child in children.as_array().ok_or("GLB 节点 children 无效")?.iter().rev() {
                stack.push((index(child)?, world));
            }
        }
        visit(node, world)?;
    }
    Ok(())
}

#[derive(Default)]
struct ExistingLights {
    attached: HashMap<String, Value>,
    standalone: Vec<(Value, Value)>,
    authored: HashMap<String, Value>,
}

impl ExistingLights {
    fn read(document: &Value, nodes: &[Value]) -> Result<Self, String> {
        let mut result = Self::default();
        let Some(definitions) = document["extensions"][LIGHTS]["lights"].as_array() else { return Ok(result); };
        let scene = document.get("scene").map(index).transpose()?.unwrap_or(0);
        let roots = array(document["scenes"].get(scene).ok_or("场景 GLB 缺少默认场景")?, "nodes")?;
        visit_nodes(nodes, roots, |node, world| {
            let Some(light) = node["extensions"][LIGHTS].get("light") else { return Ok(()); };
            let definition = definitions.get(index(light)?).ok_or("GLB 灯光索引越界")?.clone();
            let id = placement_node_id(node["extras"][PLACEMENT_ID].as_str(), node["name"].as_str(), true);
            if node.get("mesh").is_some() {
                if let Some(id) = id { result.attached.insert(id, definition); }
            } else if matches!(definition["type"].as_str(), Some("point" | "spot")) {
                if let Some(id) = id { result.authored.insert(id, definition); }
            } else {
                // 非作者灯光也要重新绑定到扩展表；保留名称和其他节点字段，移除已展开的层级和旧局部姿态。
                let mut passthrough = node.clone();
                let fields = passthrough.as_object_mut().ok_or("GLB 灯光节点无效")?;
                for key in ["children", "translation", "rotation", "scale"] { fields.remove(key); }
                passthrough["matrix"] = json!(world.to_cols_array());
                result.standalone.push((passthrough, definition));
            }
            Ok(())
        })?;
        Ok(result)
    }
}

/// Blender 新增网格按名称派生身份，首次保存后沿用稳定节点属性。
pub(crate) fn placement_node_id(custom_id: Option<&str>, name: Option<&str>, mesh: bool) -> Option<String> {
    custom_id.map(str::to_owned).or_else(|| {
        mesh.then_some(name).flatten().filter(|name| !name.is_empty())
            .map(|name| format!("glb-{}", crate::authoring_io::bytes_hash(name.as_bytes())))
    })
}

fn read_node<'a>(
    document: &'a Value,
    node: &'a Value,
    transform: Mat4,
    asset_path: &str,
    preserve_recipes: bool,
    recipes: &mut HashMap<&'a str, ScenePlacementAssetTarget>,
) -> Result<Option<ScenePlacementRecord>, String> {
    let extras = &node["extras"];
    let owned_mesh = node.get("mesh").map(index).transpose()?;
    let custom_id = extras[PLACEMENT_ID].as_str();
    let light = node["extensions"][LIGHTS].get("light").map(index).transpose()?
        .map(|i| document["extensions"][LIGHTS]["lights"].get(i).ok_or("GLB 灯光索引越界")).transpose()?
        .filter(|light| matches!(light["type"].as_str(), Some("point" | "spot")));
    if owned_mesh.is_none() && custom_id.is_none() && light.is_none() {
        return Ok(None); // 场景标识及 Blender 中的普通分组空节点不是可视摆放物。
    }
    let name = node["name"].as_str().filter(|name| !name.is_empty());
    let id = placement_node_id(custom_id, name, owned_mesh.is_some() || light.is_some())
        .ok_or("新增的 GLB 网格或灯光节点需要名称")?;
    let mut fields: Value = match extras[PLACEMENT_FIELDS].as_str() {
        Some(fields) => serde_json::from_str(fields).map_err(|error| format!("节点 {id} 的摆放属性无效：{error}"))?,
        None => json!({}),
    };
    if !fields.is_object() {
        return Err(format!("节点 {id} 的摆放属性必须为对象"));
    }
    let (translation, rotation, scale) = placement_transform(transform)
        .map_err(|error| format!("节点 {id}：{error}"))?;
    fields["id"] = json!(id);
    fields["translation"] = json!(translation);
    fields["yaw_degrees"] = json!(0.0);
    fields["scale"] = json!(scale);
    if let Some(name) = name.filter(|name| Some(*name) != custom_id) {
        if fields["editor"].is_null() {
            fields["editor"] = json!({});
        }
        fields["editor"].as_object_mut().ok_or_else(|| format!("节点 {id} 的编辑属性无效"))?
            .insert("name".to_owned(), json!(name));
    }
    let target = if let Some(mesh) = owned_mesh {
        let collision = extras[COLLISION].as_bool()
            .or_else(|| extras[COLLISION].as_u64().map(|value| value != 0))
            .unwrap_or(false);
        let material = mesh_material(document, mesh)?;
        let mesh_matches = extras[BUILTIN_MESH].as_u64() == Some(mesh as u64);
        let recipe = if let Some(encoded) = extras[BUILTIN_RECIPE].as_str().filter(|_| preserve_recipes && mesh_matches) {
            // 相同配方跨摆放复用已验证 Arc 几何，不重复解码、三角化或构建空间索引。
            let target = match recipes.entry(encoded) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let decoded = serde_json::from_str::<ScenePlacementAssetTarget>(encoded)
                        .map_err(|error| format!("节点 {id} 的内置网格参数无效：{error}"))?;
                    entry.insert(decoded).clone()
                }
            };
            Some(target)
        } else {
            None
        };
        let target = if let Some(recipe) = recipe.filter(|target| target.is_builtin_mesh() && builtin_material(target).2 == collision) {
            fields["asset_path"] = json!("");
            recipe.validate()?;
            recipe
        } else {
            fields["asset_path"] = json!(asset_path);
            ScenePlacementAssetTarget::Primitive {
                mesh: u32::try_from(mesh).map_err(|_| "GLB 网格索引过大")?,
                primitive: 0,
                material,
                collision,
            }
        };
        fields.as_object_mut().expect("已核对摆放字段对象").remove("asset_target");
        Some(target)
    } else if let Some(light) = light {
        fields["asset_path"] = json!("");
        Some(ScenePlacementAssetTarget::Light {
            light: serde_json::from_value(light.clone()).map_err(|error| format!("节点 {id} 的灯光无效：{error}"))?,
        })
    } else {
        if fields["asset_path"].as_str().is_none_or(str::is_empty) {
            return Err(format!("节点 {id} 没有网格或共享资产引用"));
        }
        None
    };
    let mut record: ScenePlacementRecord = serde_json::from_value(fields)
        .map_err(|error| format!("节点 {id} 的摆放无效：{error}"))?;
    if let Some(target) = target {
        record.asset_target = target;
    }
    record.asset_target.validate()?;
    record.set_rotation(rotation).map_err(|error| format!("节点 {id}：{error}"))?;
    Ok(Some(record))
}

fn mesh_material(document: &Value, mesh: usize) -> Result<Option<u32>, String> {
    let mesh = array(document, "meshes")?.get(mesh).ok_or("GLB 网格索引越界")?;
    let [primitive] = array(mesh, "primitives")?.as_slice() else {
        return Err("场景 GLB 的每个摆放节点必须使用单材质网格，可在 Blender 按材质拆分".into());
    };
    if primitive["mode"].as_u64().unwrap_or(4) != 4 || primitive["attributes"].get("POSITION").is_none() {
        return Err("场景 GLB 的摆放网格必须是三角面".into());
    }
    primitive.get("material").map(|material| {
        let material = index(material)?;
        array(document, "materials")?.get(material).ok_or("GLB 材质索引越界")?;
        u32::try_from(material).map_err(|_| "GLB 材质索引过大".into())
    }).transpose()
}

fn node_transform(node: &Value) -> Result<Mat4, String> {
    if let Some(matrix) = node.get("matrix") {
        let matrix = serde_json::from_value::<[f32; 16]>(matrix.clone())
            .map_err(|_| "GLB 节点矩阵无效")?;
        return Ok(Mat4::from_cols_array(&matrix));
    }
    let translation = vec_field(node, "translation", [0.0; 3])?;
    let scale = vec_field(node, "scale", [1.0; 3])?;
    let rotation = node.get("rotation").map(|value| serde_json::from_value::<[f32; 4]>(value.clone()))
        .transpose().map_err(|_| "GLB 节点旋转无效")?.unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let rotation = Quat::from_array(rotation);
    if !rotation.is_finite() || (rotation.length_squared() - 1.0).abs() > 0.001 {
        return Err("GLB 节点旋转必须为单位四元数".into());
    }
    Ok(Mat4::from_scale_rotation_translation(scale, rotation.normalize(), translation))
}

fn vec_field(node: &Value, field: &str, default: [f32; 3]) -> Result<Vec3, String> {
    node.get(field).map(|value| serde_json::from_value::<[f32; 3]>(value.clone()))
        .transpose().map_err(|_| format!("GLB 节点 {field} 无效"))
        .map(|value| Vec3::from_array(value.unwrap_or(default)))
}

fn placement_transform(transform: Mat4) -> Result<([f32; 3], Quat, [f32; 3]), String> {
    if !transform.is_finite() || transform.determinant() <= 0.0 {
        return Err("摆放变换必须有限且为正缩放".into());
    }
    let (scale, rotation, translation) = transform.to_scale_rotation_translation();
    let reconstructed = Mat4::from_scale_rotation_translation(scale, rotation, translation);
    if !transform.abs_diff_eq(reconstructed, 0.0002 * scale.max_element().max(1.0)) {
        return Err("场景摆放不接受剪切，请先在 Blender 应用剪切变换再导出".into());
    }
    Ok((translation.to_array(), rotation, scale.to_array()))
}

/// 编辑器写入同一个 GLB。已有模型的缓冲和材质沿用原文件，变换修改只更新节点。
pub fn write_scene_placements_glb(
    placements: &ScenePlacementsAsset,
    asset_path: &str,
    previous: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let (mut document, previous_bin) = if let Some(bytes) = previous {
        let (document, bin) = read_chunks(bytes)?;
        (document, bin)
    } else {
        (json!({"asset": {"version": "2.0", "generator": "Feathrise"}}), &[][..])
    };
    let previous_hash = geometry_hash(&document, previous_bin);
    let mut bin = previous_bin.to_vec();
    let mut recipes = HashMap::new();
    let existing_nodes = document["nodes"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut existing_lights = ExistingLights::read(&document, existing_nodes)?;
    let previous_animation_targets = (!document["animations"].is_null()).then(|| {
        existing_nodes.iter().map(|node| placement_node_id(
            node["extras"][PLACEMENT_ID].as_str(), node["name"].as_str(), node.get("mesh").is_some() || node["extensions"][LIGHTS].get("light").is_some(),
        )).collect::<Vec<_>>()
    });
    let authoritative_animation = existing_nodes.iter().any(|node| {
        node["extras"][SCENE_ID].as_str() == Some(placements.scene_id.as_str())
            && node["extras"]["feathrise_scene_animation"].as_bool() == Some(true)
    });
    let motion_colliders: HashMap<_, _> = existing_nodes.iter().filter_map(|node| {
        Some((node["extras"][PLACEMENT_ID].as_str()?.to_owned(),
            node["extras"].get("feathrise_motion_collider")?.clone()))
    }).collect();
    if existing_nodes.iter().any(|node| node["extras"][SCENE_ID].as_str() == Some(placements.scene_id.as_str())
        && node["extras"][GEOMETRY_HASH].as_str() == Some(previous_hash.as_str())) {
        for node in existing_nodes {
            if let (Some(recipe), Some(mesh)) = (node["extras"][BUILTIN_RECIPE].as_str(), node.get("mesh")) {
                recipes.insert(recipe.to_owned(), index(mesh)?);
            }
        }
    }
    let mut nodes = vec![json!({"name": "Feathrise", "extras": {
        SCENE_ID: placements.scene_id, SCENE_VERSION: placements.schema_version
    }})];
    if authoritative_animation {
        nodes[0]["extras"]["feathrise_scene_animation"] = json!(true);
    }
    let mut lights = Vec::new();
    for record in &placements.placements {
        let name = if record.editor.name.is_empty() { &record.id } else { &record.editor.name };
        let rotation = record.rotation().to_array();
        let mut node = json!({"name": name, "translation": record.translation, "scale": record.scale, "rotation": rotation});
        // 内置编辑源可能有数十万面。几何配方在BUILTIN_RECIPE只序列化一次，节点字段不再复制同一整片数据。
        let builtin_fields;
        let fields_source = if record.asset_target.is_builtin() {
            builtin_fields = ScenePlacementRecord {
                asset_target: ScenePlacementAssetTarget::Scene0,
                ..record.clone()
            };
            &builtin_fields
        } else {
            record
        };
        let mut fields = serde_json::to_value(fields_source).map_err(|error| error.to_string())?;
        let field_map = fields.as_object_mut().expect("摆放记录总是对象");
        for key in ["id", "translation", "yaw_degrees", "scale"] {
            field_map.remove(key);
        }
        // GLB 中旋转以节点为准。移除只在旧 JSON 记录中使用的余量。
        if let Some(editor) = field_map.get_mut("editor").and_then(Value::as_object_mut) {
            editor.remove("rotation_offset");
            if editor.is_empty() {
                field_map.remove("editor");
            }
        }
        let mut extras = json!({PLACEMENT_ID: record.id});
        let mesh = match &record.asset_target {
            ScenePlacementAssetTarget::Light { light } => {
                node["extensions"][LIGHTS] = json!({ "light": lights.len() });
                let mut definition = existing_lights.authored.remove(&record.id).unwrap_or_else(|| json!({}));
                let fields = definition.as_object_mut().ok_or("GLB 灯光定义无效")?;
                for key in ["range", "spot"] { fields.remove(key); }
                fields.extend(json!(light).as_object().unwrap().clone());
                lights.push(definition);
                None
            }
            target if target.is_builtin_mesh() => {
                target.validate()?;
                let recipe = serde_json::to_string(target).map_err(|error| error.to_string())?;
                let mesh = if let Some(mesh) = recipes.get(&recipe) { *mesh } else {
                    let mesh = append_builtin(&mut document, &mut bin, target)?;
                    recipes.insert(recipe.clone(), mesh);
                    mesh
                };
                extras[BUILTIN_RECIPE] = json!(recipe);
                extras[BUILTIN_MESH] = json!(mesh);
                extras[COLLISION] = json!(builtin_material(target).2);
                Some(mesh)
            }
            ScenePlacementAssetTarget::Primitive { mesh, primitive: 0, material, collision }
                if record.asset_path == asset_path => {
                if mesh_material(&document, *mesh as usize)? != *material {
                    return Err(format!("场景 GLB 网格 {} 的材质已换代，请重新打开场景", record.id));
                }
                extras[COLLISION] = json!(collision);
                Some(*mesh as usize)
            }
            _ if record.asset_path == asset_path => return Err(format!("场景 GLB 内部引用 {} 无效", record.id)),
            _ => None, // 共享资产在 GLB 内保存为带引用的空节点，由原摆放加载器实例化。
        };
        if let Some(mesh) = mesh {
            node["mesh"] = json!(mesh);
            if let Some(light) = existing_lights.attached.remove(&record.id) {
                node["extensions"][LIGHTS] = json!({"light":lights.len()});
                lights.push(light);
            }
            let field_map = fields.as_object_mut().expect("摆放记录总是对象");
            field_map.remove("asset_path");
            field_map.remove("asset_target");
        }
        extras[PLACEMENT_FIELDS] = json!(serde_json::to_string(&fields).map_err(|error| error.to_string())?);
        if let Some(collider) = motion_colliders.get(record.id.as_str()) {
            extras["feathrise_motion_collider"] = (*collider).clone();
        }
        node["extras"] = extras;
        nodes.push(node);
    }
    for (mut node, light) in existing_lights.standalone {
        node["extensions"][LIGHTS] = json!({"light":lights.len()});
        lights.push(light);
        nodes.push(node);
    }
    // 点光与聚光的光学参数只写进标准扩展。主场景重新编排节点时，一并重建所有光源索引。
    if !lights.is_empty() {
        document["extensions"][LIGHTS]["lights"] = json!(lights);
        if document["extensionsUsed"].is_null() { document["extensionsUsed"] = json!([]); }
        let used = document["extensionsUsed"].as_array_mut().ok_or("GLB extensionsUsed 无效")?;
        if !used.iter().any(|extension| extension.as_str() == Some(LIGHTS)) { used.push(json!(LIGHTS)); }
    } else if document["extensions"].get(LIGHTS).is_some() {
        if let Some(extensions) = document["extensions"].as_object_mut() { extensions.remove(LIGHTS); }
        for key in ["extensionsUsed", "extensionsRequired"] {
            if let Some(used) = document[key].as_array_mut() {
                used.retain(|extension| extension.as_str() != Some(LIGHTS));
                if used.is_empty() { document.as_object_mut().unwrap().remove(key); }
            }
        }
    }
    if !bin.is_empty() {
        if document["buffers"].is_null() {
            document["buffers"] = json!([{"byteLength": bin.len()}]);
        } else if document["buffers"][0]["uri"].is_null() {
            document["buffers"][0]["byteLength"] = json!(bin.len());
        } else {
            return Err("场景 GLB 的第一个缓冲必须内嵌".into());
        }
    }
    nodes[0]["extras"][GEOMETRY_HASH] = json!(geometry_hash(&document, &bin));
    if let Some(targets) = previous_animation_targets {
        rebind_animation_nodes(&mut document, &targets, &nodes)?;
    }
    document["nodes"] = json!(nodes);
    document["scenes"] = json!([{"nodes": (0..nodes.len()).collect::<Vec<_>>() }]);
    document["scene"] = json!(0);
    write_chunks(&document, &bin)
}

/// 交换或删除场景对象时按稳定 ID 重绑标准动画通道，保留采样器、时间、片段属性和 BIN。
fn rebind_animation_nodes(
    document: &mut Value, old: &[Option<String>], nodes: &[Value],
) -> Result<(), String> {
    let new: HashMap<_, _> = nodes.iter().enumerate().filter_map(|(index, node)| {
        Some((node["extras"][PLACEMENT_ID].as_str()?, index))
    }).collect();
    let animations = document["animations"].as_array_mut().ok_or("场景 GLB 的动画列表无效")?;
    for animation in animations.iter_mut() {
        let channels = animation["channels"].as_array_mut().ok_or("场景 GLB 的动画通道无效")?;
        for channel in channels.iter_mut() {
            let Some(target) = channel["target"].get_mut("node") else { continue; };
            let id = old.get(index(target)?).ok_or("场景 GLB 动画节点超出范围")?.as_deref();
            *target = id.and_then(|id| new.get(id)).map_or(Value::Null, |index| json!(index));
        }
        channels.retain(|channel| channel["target"].get("node") != Some(&Value::Null));
    }
    animations.retain(|animation| animation["channels"].as_array().is_some_and(|channels| !channels.is_empty()));
    if animations.is_empty() {
        document.as_object_mut().expect("已读取的 GLB JSON 根是对象").remove("animations");
    }
    Ok(())
}

fn builtin_material(target: &ScenePlacementAssetTarget) -> ([u8; 3], u16, bool) {
    match *target {
        ScenePlacementAssetTarget::Cuboid { color_srgb, roughness_milli, collision, .. }
        | ScenePlacementAssetTarget::EditableCuboid { color_srgb, roughness_milli, collision, .. }
        | ScenePlacementAssetTarget::EditableMesh { color_srgb, roughness_milli, collision, .. } => {
            (color_srgb, roughness_milli, collision)
        }
        _ => unreachable!("只编码内置几何"),
    }
}

fn append_builtin(
    document: &mut Value,
    bin: &mut Vec<u8>,
    target: &ScenePlacementAssetTarget,
) -> Result<usize, String> {
    let geometry = target.editable_geometry().ok_or("内置几何无效")?;
    let positions: Vec<_> = geometry.vertices().iter()
        .map(|point| Vec3::from_array(point.map(|value| value as f32 * 0.0005))).collect();
    let mut triangles = Vec::new();
    let mut normals = Vec::new();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for triangle in geometry.triangles() {
        let [a, b, c] = triangle.map(|i| positions[i]);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        for position in [a, b, c] {
            triangles.push(position.to_array());
            normals.push(normal.to_array());
            min = min.min(position);
            max = max.max(position);
        }
    }
    let position = append_accessor(document, bin, triangles.len(), "VEC3", 5126, 34962,
        triangles.iter().flatten().flat_map(|value| value.to_le_bytes()))?;
    document["accessors"][position]["min"] = json!(min.to_array());
    document["accessors"][position]["max"] = json!(max.to_array());
    let normal = append_accessor(document, bin, normals.len(), "VEC3", 5126, 34962,
        normals.iter().flatten().flat_map(|value| value.to_le_bytes()))?;
    let indices = append_accessor(document, bin, triangles.len(), "SCALAR", 5125, 34963,
        (0..triangles.len() as u32).flat_map(u32::to_le_bytes))?;
    let (color, roughness, _) = builtin_material(target);
    let [r, g, b] = color;
    let color = bevy::color::Color::srgb_u8(r, g, b).to_linear();
    let open_surface = matches!(target, ScenePlacementAssetTarget::EditableMesh { geometry, .. } if geometry.is_open());
    let material = push(document, "materials", json!({"doubleSided": open_surface, "pbrMetallicRoughness": {
        "baseColorFactor": [color.red, color.green, color.blue, color.alpha],
        "metallicFactor": 0.0, "roughnessFactor": f32::from(roughness) / 1000.0
    }}))?;
    push(document, "meshes", json!({"primitives": [{
        "attributes": {"POSITION": position, "NORMAL": normal}, "indices": indices,
        "material": material, "mode": 4
    }]}))
}

fn append_accessor(
    document: &mut Value,
    bin: &mut Vec<u8>,
    count: usize,
    shape: &str,
    component: u32,
    target: u32,
    bytes: impl Iterator<Item = u8>,
) -> Result<usize, String> {
    let offset = bin.len();
    bin.extend(bytes);
    let view = push(document, "bufferViews", json!({
        "buffer": 0, "byteOffset": offset, "byteLength": bin.len() - offset, "target": target
    }))?;
    push(document, "accessors", json!({"bufferView": view, "componentType": component, "count": count, "type": shape}))
}

fn push(document: &mut Value, key: &str, value: Value) -> Result<usize, String> {
    if document[key].is_null() {
        document[key] = json!([]);
    }
    let array = document[key].as_array_mut().ok_or_else(|| format!("GLB 的 {key} 必须是数组"))?;
    let index = array.len();
    array.push(value);
    Ok(index)
}

fn index(value: &Value) -> Result<usize, String> {
    value.as_u64().and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| "GLB 索引必须为非负整数".into())
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value[key].as_array().ok_or_else(|| format!("GLB 缺少 {key} 数组"))
}

fn geometry_hash(document: &Value, bin: &[u8]) -> String {
    let mut hash = Sha256::new();
    // 仅在真实网格和材质完整匹配时保留 F12 参数化编辑能力；Blender 改形后直接采用新网格。
    for key in ["buffers", "bufferViews", "accessors", "meshes", "materials", "textures", "images"] {
        hash.update(document[key].to_string());
    }
    hash.update(bin);
    hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn read_chunks(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
    fn word(bytes: &[u8], start: usize) -> Result<u32, String> {
        let word = bytes.get(start..start + 4).ok_or("场景 GLB 文件不完整")?;
        Ok(u32::from_le_bytes(word.try_into().expect("固定四字节")))
    }
    if !bytes.starts_with(b"glTF") || word(bytes, 4)? != 2 || word(bytes, 8)? as usize != bytes.len() {
        return Err("场景 GLB 文件头、版本或长度无效".into());
    }
    let mut offset = 12;
    let mut document = None;
    let mut bin = None;
    while offset < bytes.len() {
        let length = word(bytes, offset)? as usize;
        let kind = word(bytes, offset + 4)?.to_le_bytes();
        let start = offset + 8;
        let end = start.checked_add(length).ok_or("场景 GLB 数据块长度溢出")?;
        let chunk = bytes.get(start..end).ok_or("场景 GLB 数据块越界")?;
        if length % 4 != 0 {
            return Err("场景 GLB 数据块未四字节对齐".into());
        }
        match kind {
            JSON_CHUNK if offset == 12 => {
                document = Some(serde_json::from_slice(chunk).map_err(|error| error.to_string())?);
            }
            BIN_CHUNK if document.is_some() && bin.is_none() => bin = Some(chunk),
            _ => return Err("场景 GLB 包含重复或不支持的数据块".into()),
        }
        offset = end;
    }
    Ok((document.ok_or("场景 GLB 缺少结构数据块")?, bin.unwrap_or_default()))
}

fn write_chunks(document: &Value, bin: &[u8]) -> Result<Vec<u8>, String> {
    let mut json = serde_json::to_vec(document).map_err(|error| error.to_string())?;
    json.resize(json.len().next_multiple_of(4), b' ');
    let json_length = u32::try_from(json.len()).map_err(|_| "场景 GLB 结构超过格式容量")?;
    let bin_length = u32::try_from(bin.len()).map_err(|_| "场景 GLB 网格超过格式容量")?;
    let length = 20_u32.checked_add(json_length)
        .and_then(|length| if bin.is_empty() { Some(length) } else { length.checked_add(8)?.checked_add(bin_length) })
        .ok_or("场景 GLB 超过格式容量")?;
    let mut bytes = Vec::with_capacity(length as usize);
    bytes.extend_from_slice(b"glTF");
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&json_length.to_le_bytes());
    bytes.extend_from_slice(&JSON_CHUNK);
    bytes.extend_from_slice(&json);
    if !bin.is_empty() {
        bytes.extend_from_slice(&bin_length.to_le_bytes());
        bytes.extend_from_slice(&BIN_CHUNK);
        bytes.extend_from_slice(bin);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "scenes/SCENE-glb/scene.glb";

    fn dock() -> ScenePlacementsAsset {
        serde_json::from_value(json!({
            "schema_version": SCENE_SCHEMA_VERSION, "scene_id": "SCENE-glb",
            "placements": [{
                "id": "bank-floor", "editor": {"name":"夯土河岸", "group":"河岸和栈道"},
                "asset_target": {"kind":"cuboid","size_mm":[26500,1200,46000],
                    "color_srgb":[164,132,88],"roughness_milli":850,"collision":true},
                "translation":[0.25,-0.6,-15], "yaw_degrees":0, "scale":[1,1,1]
            }]
        })).unwrap()
    }

    #[test]
    fn native_glb_bakes_mesh_and_restores_editable_cuboid_after_save() {
        let draft = dock();
        let bytes = write_scene_placements_glb(&draft, PATH, None).unwrap();
        let (gltf, bin) = read_chunks(&bytes).unwrap();
        assert_eq!(gltf["nodes"][1]["extras"][PLACEMENT_ID], "bank-floor");
        assert_eq!(gltf["nodes"][1]["extras"][COLLISION], true);
        let positions = gltf["meshes"][0]["primitives"][0]["attributes"]["POSITION"].as_u64().unwrap() as usize;
        let max = gltf["accessors"][positions]["max"].as_array().unwrap();
        for (actual, expected) in max.iter().zip([13.25, 0.6, 23.0]) {
            assert!((actual.as_f64().unwrap() - expected).abs() < 0.000_01);
        }
        assert!(!bin.is_empty());
        assert_eq!(read_scene_placements(&bytes, PATH).unwrap().placements, draft.placements);
    }

    #[test]
    fn local_lights_round_trip_standard_nodes_and_reindex_after_deletion() {
        let mut draft = dock();
        let mut point = draft.placements[0].clone();
        point.id = "courtyard-point".into();
        point.asset_target = ScenePlacementAssetTarget::from_local_light([1.0, 0.5, 0.25], 700.0, Some(9.0), None).unwrap();
        let mut spot = point.clone();
        spot.id = "door-spot".into();
        spot.asset_target = ScenePlacementAssetTarget::from_local_light([0.1, 0.5, 1.0], 450.0, None, Some([0.2, 0.7])).unwrap();
        spot.set_rotation(Quat::from_rotation_x(-0.75)).unwrap();
        draft.placements.extend([point, spot]);

        let mut bytes = write_scene_placements_glb(&draft, PATH, None).unwrap();
        for count in [2, 1, 0] {
            let (gltf, _) = read_chunks(&bytes).unwrap();
            let lights = &gltf["extensions"][LIGHTS]["lights"];
            assert_eq!(lights.as_array().map_or(0, Vec::len), count);
            assert_eq!(gltf["extensionsUsed"].as_array().is_some_and(|used| used.contains(&json!(LIGHTS))), count > 0);
            if count > 0 {
                assert_eq!(gltf["nodes"][2]["extensions"][LIGHTS]["light"], 0);
                assert_eq!(lights[count-1]["type"], "spot");
                assert_eq!(lights[count-1]["intensity"], 450.0);
                assert!(lights[count-1].get("range").is_none());
            }
            let restored = read_scene_placements(&bytes, PATH).unwrap();
            assert_eq!(restored.placements.len(), draft.placements.len());
            for (record, expected) in restored.placements.iter().zip(&draft.placements) {
                assert_eq!(record.asset_target, expected.asset_target);
                assert!(record.rotation().dot(expected.rotation()).abs() > 0.999999);
            }
            if count > 0 {
                draft.placements.remove(1);
                bytes = write_scene_placements_glb(&draft, PATH, Some(&bytes)).unwrap();
            }
        }
        for cone in [[0.5, 0.5], [-0.1, 0.5], [0.0, 2.0]] {
            assert!(ScenePlacementAssetTarget::from_local_light([1.0; 3], 1.0, None, Some(cone)).is_err());
        }
    }

    #[test]
    fn saving_local_lights_preserves_blender_light_metadata_and_unmanaged_sun_or_mesh_lights() {
        let mut draft = dock();
        let mut lamp = draft.placements[0].clone();
        lamp.id = "ceiling".into();
        lamp.asset_target = ScenePlacementAssetTarget::from_local_light([1.0;3], 500.0, Some(5.0), None).unwrap();
        draft.placements.push(lamp);
        let bytes = write_scene_placements_glb(&draft, PATH, None).unwrap();
        let (mut incoming, bin) = read_chunks(&bytes).unwrap();
        let lights = incoming["extensions"][LIGHTS]["lights"].as_array_mut().unwrap();
        lights[0]["extras"] = json!({"blender_link":"ceiling"});
        lights.push(json!({"type":"spot","extras":{"mesh_emitter":true}}));
        lights.push(json!({"type":"directional","name":"Sun"}));
        incoming["nodes"][1]["extensions"][LIGHTS] = json!({"light":1});
        incoming["nodes"][1]["children"] = json!([3]);
        incoming["nodes"].as_array_mut().unwrap().push(json!({"name":"Sun","translation":[0,2,0],"extensions":{LIGHTS:{"light":2}}}));
        let bytes = write_chunks(&incoming, bin).unwrap();
        let resaved = write_scene_placements_glb(&read_scene_placements(&bytes, PATH).unwrap(), PATH, Some(&bytes)).unwrap();
        let (document, _) = read_chunks(&resaved).unwrap();
        let definition = |node: &Value| &document["extensions"][LIGHTS]["lights"][node["extensions"][LIGHTS]["light"].as_u64().unwrap() as usize];
        assert_eq!(definition(&document["nodes"][1])["extras"]["mesh_emitter"], true);
        assert_eq!(definition(&document["nodes"][2])["extras"]["blender_link"], "ceiling");
        assert_eq!(definition(&document["nodes"][3])["type"], "directional");
        let sun = node_transform(&document["nodes"][3]).unwrap().transform_point3(Vec3::ZERO);
        assert!(sun.abs_diff_eq(Vec3::new(0.25,1.4,-15.0), 0.0001));
    }

    #[test]
    fn scene_placement_rotation_round_trips_standard_glb_and_legacy_json() {
        let mut draft = dock();
        let expected = Quat::from_rotation_x(0.6) * Quat::from_rotation_y(-0.7) * Quat::from_rotation_z(0.4);
        draft.placements[0].set_rotation(expected).unwrap();
        draft.placements[0].scale = [1.5, 0.75, 2.0];
        let legacy = serde_json::to_vec(&draft.placements[0]).unwrap();
        let reread: ScenePlacementRecord = serde_json::from_slice(&legacy).unwrap();
        assert_eq!(reread, draft.placements[0]);
        assert!(reread.rotation().dot(expected).abs() > 0.999999);

        let bytes = write_scene_placements_glb(&draft, PATH, None).unwrap();
        let (document, _) = read_chunks(&bytes).unwrap();
        let node = &document["nodes"][1];
        let rotation: [f32; 4] = serde_json::from_value(node["rotation"].clone()).unwrap();
        assert!(Quat::from_array(rotation).dot(expected).abs() > 0.999999);
        let extras: Value = serde_json::from_str(node["extras"][PLACEMENT_FIELDS].as_str().unwrap()).unwrap();
        assert!(extras["editor"].get("rotation_offset").is_none(), "GLB 只用标准节点四元数");
        let loaded = read_scene_placements(&bytes, PATH).unwrap().placements.remove(0);
        assert!(loaded.rotation().dot(expected).abs() > 0.999999);
        assert!(Vec3::from_array(loaded.scale).abs_diff_eq(Vec3::new(1.5, 0.75, 2.0), 0.00001));

        let mut old = dock().placements.remove(0);
        old.yaw_degrees = 60.0;
        assert_eq!(old.rotation(), Quat::from_rotation_y(60_f32.to_radians()));
        assert!(serde_json::to_value(&old).unwrap()["editor"].get("rotation_offset").is_none());
    }

    #[test]
    fn referenced_scene0_saves_collision_per_instance() {
        let mut draft = dock();
        draft.placements[0].asset_path = "scenes/SCENE-glb/models/floor.glb".to_owned();
        draft.placements[0].asset_target = ScenePlacementAssetTarget::Scene { collision: true };
        let mut old = draft.placements[0].clone();
        old.id = "other-floor".to_owned();
        old.asset_target = serde_json::from_value(json!({"kind": "scene0"})).unwrap();
        assert_eq!(old.asset_target, ScenePlacementAssetTarget::Scene0);
        draft.placements.push(old);

        let bytes = write_scene_placements_glb(&draft, PATH, None).unwrap();
        let (document, bin) = read_chunks(&bytes).unwrap();
        assert!(bin.is_empty()); // 两项只保存模型引用和各自碰撞状态。
        let fields = |index: usize| -> Value {
            serde_json::from_str(document["nodes"][index]["extras"][PLACEMENT_FIELDS].as_str().unwrap()).unwrap()
        };
        assert_eq!(fields(1)["asset_target"], json!({"kind": "scene0", "collision": true}));
        assert!(fields(2).get("asset_target").is_none());
        assert_eq!(read_scene_placements(&bytes, PATH).unwrap().placements, draft.placements);
    }

    #[test]
    fn scene_motion_uses_standard_glb_after_placement_reordering() {
        use bevy::{
            asset::{AssetApp, AssetPlugin, AssetServer, Assets, Handle, io::{
                AssetSourceBuilder, AssetSourceId, memory::{Dir, MemoryAssetReader},
            }},
            gltf::GltfPlugin,
            prelude::{AnimationClip, App, Mesh, StandardMaterial, TaskPoolPlugin, Transform},
            world_serialization::WorldAsset,
        };
        use std::{path::Path, time::{Duration, Instant}};
        use crate::{SceneMotionAsset, loaders::ScenePlacementsLoader};

        let mut draft = dock();
        let mut other = draft.placements[0].clone();
        other.id = "other".into();
        draft.placements.push(other);
        let bytes = write_scene_placements_glb(&draft, PATH, None).unwrap();
        let (mut document, bin) = read_chunks(&bytes).unwrap();
        let mut bin = bin.to_vec();
        let mut append = |shape: &str, values: &[f32]| {
            let accessor = append_accessor(&mut document, &mut bin, 2, shape, 5126, 34962,
                values.iter().flat_map(|value| value.to_le_bytes())).unwrap();
            let view = document["accessors"][accessor]["bufferView"].as_u64().unwrap() as usize;
            document["bufferViews"][view].as_object_mut().unwrap().remove("target");
            accessor
        };
        let time = append("SCALAR", &[0.0, 2.0]);
        let position = append("VEC3", &[0.0, 1.0, 0.0, 0.0, 3.0, 0.0]);
        let quarter = std::f32::consts::FRAC_1_SQRT_2;
        let rotation = append("VEC4", &[0.0, 0.0, 0.0, 1.0, 0.0, quarter, 0.0, quarter]);
        document["accessors"][time]["min"] = json!([0.0]);
        document["accessors"][time]["max"] = json!([2.0]);
        document["nodes"][1]["extras"]["feathrise_motion_collider"] = json!({
            "center":[0.25,0.0,0.0],"half_extents":[1.5,0.2,1.5]
        });
        document["buffers"][0]["byteLength"] = json!(bin.len());
        document["nodes"][0]["extras"][GEOMETRY_HASH] = json!(geometry_hash(&document, &bin));
        document["animations"] = json!([
            {"name":"其他动作","samplers":[{"input":time,"output":position}],
             "channels":[{"sampler":0,"target":{"node":2,"path":"translation"}}]},
            {"name":"场景动画","extras":{"feathrise_scene_animation":true,"feathrise_autoplay":true,
                "feathrise_duration":2.0,"feathrise_playback":"ping_pong"},
             "samplers":[{"input":time,"output":position},{"input":time,"output":rotation}],
             "channels":[{"sampler":0,"target":{"node":1,"path":"translation"}},
                         {"sampler":1,"target":{"node":1,"path":"rotation"}}]},
            {"name":"开门","extras":{"feathrise_scene_animation":true,"feathrise_clip_id":"door-open"},
             "samplers":[{"input":time,"output":position}],
             "channels":[{"sampler":0,"target":{"node":2,"path":"translation"}}]}
        ]);
        let animated = write_chunks(&document, &bin).unwrap();
        draft.placements.swap(0, 1);
        let saved = write_scene_placements_glb(&draft, PATH, Some(&animated)).unwrap();
        let (mut rebound, contents) = read_chunks(&saved).unwrap();
        assert_eq!(rebound["animations"][0]["channels"][0]["target"]["node"], 1);
        assert_eq!(rebound["animations"][1]["channels"][1]["target"]["node"], 2);
        assert_eq!(rebound["animations"][2]["channels"][0]["target"]["node"], 1);
        assert_eq!(rebound["animations"][1]["samplers"], document["animations"][1]["samplers"]);
        assert_eq!(rebound["nodes"][2]["extras"]["feathrise_motion_collider"],
            document["nodes"][1]["extras"]["feathrise_motion_collider"]);
        assert_eq!(contents, bin);

        // 真实 GltfLoader 和原摆放读取器同时访问数据库登记的同一个主 GLB。
        let files = Dir::default();
        files.insert_asset(Path::new(PATH), saved.clone());
        let mut manual = rebound.clone();
        manual["animations"][1]["extras"].as_object_mut().unwrap().remove("feathrise_autoplay");
        let manual_path = "manual/scene.glb";
        files.insert_asset(Path::new(manual_path), write_chunks(&manual, contents).unwrap());
        manual["animations"][1].as_object_mut().unwrap().remove("extras");
        manual["animations"].as_array_mut().unwrap().remove(2);
        let blender_path = "blender/scene.glb";
        files.insert_asset(Path::new(blender_path), write_chunks(&manual, contents).unwrap());
        let source = files.clone();
        let mut app = App::new();
        app.register_asset_source(AssetSourceId::Default, AssetSourceBuilder::new(move || {
            Box::new(MemoryAssetReader { root: source.clone() })
        }))
        .add_plugins((TaskPoolPlugin::default(), AssetPlugin {
            watch_for_changes_override: Some(false), ..Default::default()
        }, GltfPlugin::default()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<WorldAsset>()
        .init_asset::<AnimationClip>()
        .init_asset::<ScenePlacementsAsset>()
        .init_asset::<SceneMotionAsset>()
        .init_asset_loader::<ScenePlacementsLoader>();
        crate::scene_motion::install_gltf_motion(&mut app);
        app.finish();
        app.cleanup();
        let server = app.world().resource::<AssetServer>().clone();
        let placements: Handle<ScenePlacementsAsset> = server.load(PATH);
        let motion: Handle<SceneMotionAsset> = server.load(SceneMotionAsset::placement_path(&placements).unwrap());
        let manual: Handle<SceneMotionAsset> = server.load(format!("{manual_path}#SceneMotion"));
        let blender: Handle<SceneMotionAsset> = server.load(format!("{blender_path}#SceneMotion"));
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            let data = app.world().resource::<Assets<SceneMotionAsset>>();
            if let (Some(asset), Some(manual), Some(blender)) = (data.get(&motion), data.get(&manual), data.get(&blender)) {
                for source in [manual, blender] {
                    assert!(source.authoritative && source.tracks.as_ref().unwrap().is_empty(),
                        "省略自动播放时主 GLB 继续覆盖旧运动旁档且沿用普通摆放");
                }
                assert_eq!(asset.clips.clips().len(), 2, "只登记带场景动画标记的两个片段");
                let legacy = &asset.clips.clips()[asset.clips.clip_index("scene-clip-1").unwrap()];
                assert!(legacy.auto_play, "已有片段使用 glTF 的绝对动画序号");
                let door_index = asset.clips.clip_index("door-open").unwrap();
                assert_eq!(asset.clips.clips()[door_index].name, "开门");
                assert!(!asset.clips.clips()[door_index].auto_play);
                assert_eq!(asset.clips.placement("other").unwrap()[0].clip_index, door_index);
                assert_eq!(manual.clips.clips().len(), 2, "关闭自动播放仍保留两个可触发片段");
                assert_eq!(blender.clips.clips().len(), 1, "未标记的 Blender 文件沿用第一个相容片段");
                assert_eq!(blender.clips.clip_index("scene-clip-0"), Some(0));
                let tracks = asset.tracks.as_ref().unwrap();
                assert_eq!(tracks.len(), 1, "优先播放已标记的场景片段");
                let track = &tracks["bank-floor"];
                let (position, rotation) = track.sample(3.0, Transform::IDENTITY);
                assert_eq!(position, Vec3::Y * 2.0);
                assert!(rotation.dot(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)).abs() > 0.9999);
                assert_eq!(track.collider_for(&draft.placements[1]).unwrap().center.x, 0.25);
                break;
            }
            assert!(Instant::now() < deadline, "共同 GLB 加载状态：{:?}", server.get_load_state(motion.id()));
            std::thread::sleep(Duration::from_millis(1));
        }

        // 删除管理片段后，场景根标记保留，其他标准片段不能被误认成当前场景动画。
        rebound["nodes"][0]["extras"]["feathrise_scene_animation"] = json!(true);
        rebound["animations"].as_array_mut().unwrap().truncate(1);
        let empty = write_chunks(&rebound, contents).unwrap();
        let empty = write_scene_placements_glb(&draft, PATH, Some(&empty)).unwrap();
        files.insert_asset(Path::new(PATH), empty);
        server.reload(PATH);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            app.update();
            let data = app.world().resource::<Assets<SceneMotionAsset>>();
            if let Some(asset) = data.get(&motion).filter(|asset| asset.authoritative && asset.clips.clips().is_empty()) {
                assert!(asset.tracks.as_ref().unwrap().is_empty());
                break;
            }
            assert!(Instant::now() < deadline, "主 GLB 动画清空后未热更新");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn plane_glb_keeps_open_author_geometry_double_sided_material_and_collision() {
        let mut draft = dock();
        draft.placements[0].asset_target = ScenePlacementAssetTarget::EditableMesh {
            geometry: crate::EditableMeshGeometry::from_plane_size_mm([3000, 5000]).unwrap(),
            color_srgb: [100, 150, 200], roughness_milli: 250, collision: true,
        };
        let bytes = write_scene_placements_glb(&draft, PATH, None).unwrap();
        let (document, _) = read_chunks(&bytes).unwrap();
        assert_eq!(document["materials"][0]["doubleSided"], true);
        assert_eq!(document["nodes"][1]["extras"][COLLISION], true);
        let reopened = read_scene_placements(&bytes, PATH).unwrap();
        assert_eq!(reopened.placements, draft.placements);
        assert!(reopened.placements[0].asset_target.editable_geometry().unwrap().is_open());
    }

    #[test]
    fn changed_glb_geometry_supersedes_recipe_and_resaving_preserves_buffers() {
        let bytes = write_scene_placements_glb(&dock(), PATH, None).unwrap();
        let (gltf, bin) = read_chunks(&bytes).unwrap();
        // 模拟建模软件直接修改顶点；旧的可参数化配方不得覆盖新数据。
        let mut edited = bin.to_vec();
        edited[4..8].copy_from_slice(&1.25_f32.to_le_bytes());
        let exported = write_chunks(&gltf, &edited).unwrap();
        let mut draft = read_scene_placements(&exported, PATH).unwrap();
        assert_eq!(draft.placements[0].asset_path, PATH);
        assert!(draft.placements[0].asset_target.has_primitive_collision());
        draft.placements[0].translation[0] = 2.0;
        let saved = write_scene_placements_glb(&draft, PATH, Some(&exported)).unwrap();
        assert_eq!(read_chunks(&saved).unwrap().1, edited);
        assert_eq!(read_scene_placements(&saved, PATH).unwrap().placements, draft.placements);
    }

    #[test]
    fn glb_persisted_dense_builtin_copies_reuse_the_validated_recipe() {
        let mut draft = dock();
        let mut record = draft.placements[0].clone();
        for _ in 0..15 {
            record = crate::extruded_placement_record(&record, 3, 0.1).unwrap();
        }
        let original = record.asset_target.editable_geometry().unwrap();
        assert_eq!(original.vertex_count(), 68);
        draft.placements[0] = record.clone();
        record.id = "bank-copy".into();
        record.translation[0] += 30.0;
        draft.placements.push(record);
        let saved = write_scene_placements_glb(&draft, PATH, None).unwrap();
        let loaded = read_scene_placements(&saved, PATH).unwrap();
        assert_eq!(loaded.placements, draft.placements);
        let restored = loaded.placements[0].asset_target.editable_geometry().unwrap();
        let copied = loaded.placements[1].asset_target.editable_geometry().unwrap();
        assert_eq!(restored, original);
        assert!(restored.same_storage(&copied));
    }
}
