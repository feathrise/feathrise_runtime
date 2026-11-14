//! 场景摆放 GLB 的读写。节点变换和真实网格是 Blender 与 Feathrise 共用的作者源。

use std::collections::{HashMap, HashSet};

use bevy::prelude::{Mat4, Quat, Vec3};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    MAX_SCENE_PLACEMENTS_PER_SCENE, SCENE_SCHEMA_VERSION, SceneId, ScenePlacementAssetTarget,
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
    let mut stack = roots.iter().rev().map(|node| Ok((index(node)?, Mat4::IDENTITY)))
        .collect::<Result<Vec<_>, String>>()?;
    let mut visited = HashSet::new();
    let mut ids = HashSet::new();
    let mut placements = Vec::new();
    while let Some((node_index, parent)) = stack.pop() {
        if !visited.insert(node_index) {
            return Err("场景 GLB 的节点重复或形成环".into());
        }
        let node = nodes.get(node_index).ok_or("场景 GLB 的节点索引越界")?;
        let world = parent * node_transform(node)?;
        if let Some(children) = node.get("children") {
            for child in children.as_array().ok_or("GLB 节点 children 无效")?.iter().rev() {
                stack.push((index(child)?, world));
            }
        }
        if let Some(record) = read_node(&document, node, world, asset_path, preserve_recipes)? {
            if !ids.insert(record.id.clone()) {
                return Err(format!("场景 {scene_id} 的摆放 ID {} 重复", record.id));
            }
            placements.push(record);
            if placements.len() > MAX_SCENE_PLACEMENTS_PER_SCENE {
                return Err(format!("场景 {scene_id} 超过摆放对象上限"));
            }
        }
    }
    let result = ScenePlacementsAsset { schema_version: SCENE_SCHEMA_VERSION, scene_id, placements };
    result.validate_courtyards()?;
    Ok(result)
}

fn read_node(
    document: &Value,
    node: &Value,
    transform: Mat4,
    asset_path: &str,
    preserve_recipes: bool,
) -> Result<Option<ScenePlacementRecord>, String> {
    let extras = &node["extras"];
    let owned_mesh = node.get("mesh").map(index).transpose()?;
    let custom_id = extras[PLACEMENT_ID].as_str();
    if owned_mesh.is_none() && custom_id.is_none() {
        return Ok(None); // 场景标识及 Blender 中的普通分组空节点不是可视摆放物。
    }
    let name = node["name"].as_str().filter(|name| !name.is_empty());
    let id = custom_id.map(str::to_owned).or_else(|| {
        // 新建 Blender 对象由名称派生身份，首次 Feathrise 保存后固定为节点属性。
        name.map(|name| format!("glb-{}", crate::authoring_io::bytes_hash(name.as_bytes())))
    }).ok_or("新增 GLB 网格节点需要名称")?;
    let mut fields: Value = match extras[PLACEMENT_FIELDS].as_str() {
        Some(fields) => serde_json::from_str(fields).map_err(|error| format!("节点 {id} 的摆放属性无效：{error}"))?,
        None => json!({}),
    };
    if !fields.is_object() {
        return Err(format!("节点 {id} 的摆放属性必须为对象"));
    }
    let (translation, yaw, scale) = placement_transform(transform)
        .map_err(|error| format!("节点 {id}：{error}"))?;
    fields["id"] = json!(id);
    fields["translation"] = json!(translation);
    fields["yaw_degrees"] = json!(yaw);
    fields["scale"] = json!(scale);
    if let Some(name) = name.filter(|name| Some(*name) != custom_id) {
        if fields["editor"].is_null() {
            fields["editor"] = json!({});
        }
        fields["editor"].as_object_mut().ok_or_else(|| format!("节点 {id} 的编辑属性无效"))?
            .insert("name".to_owned(), json!(name));
    }
    if let Some(mesh) = owned_mesh {
        let collision = extras[COLLISION].as_bool()
            .or_else(|| extras[COLLISION].as_u64().map(|value| value != 0))
            .unwrap_or(false);
        let material = mesh_material(document, mesh)?;
        let mesh_matches = extras[BUILTIN_MESH].as_u64() == Some(mesh as u64);
        let recipe = (preserve_recipes && mesh_matches).then(|| extras[BUILTIN_RECIPE].as_str()).flatten()
            .map(serde_json::from_str::<ScenePlacementAssetTarget>).transpose()
            .map_err(|error| format!("节点 {id} 的内置网格参数无效：{error}"))?;
        let target = if let Some(recipe) = recipe.filter(|target| target.is_builtin() && builtin_material(*target).2 == collision) {
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
        fields["asset_target"] = serde_json::to_value(target).map_err(|error| error.to_string())?;
    } else if fields["asset_path"].as_str().is_none_or(str::is_empty) {
        return Err(format!("节点 {id} 没有网格或共享资产引用"));
    }
    let record: ScenePlacementRecord = serde_json::from_value(fields)
        .map_err(|error| format!("节点 {id} 的摆放无效：{error}"))?;
    record.asset_target.validate()?;
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

fn placement_transform(transform: Mat4) -> Result<([f32; 3], f32, [f32; 3]), String> {
    if !transform.is_finite() || transform.determinant() <= 0.0 {
        return Err("摆放变换必须有限且为正缩放".into());
    }
    let (scale, rotation, translation) = transform.to_scale_rotation_translation();
    let forward = rotation * Vec3::Z;
    let yaw = forward.x.atan2(forward.z);
    let reconstructed = Mat4::from_scale_rotation_translation(scale, Quat::from_rotation_y(yaw), translation);
    if !transform.abs_diff_eq(reconstructed, 0.0002 * scale.max_element().max(1.0)) {
        return Err("场景摆放使用 Y 轴旋转，请在 Blender 应用倾斜、镜像或剪切变换后导出".into());
    }
    let yaw = yaw.to_degrees();
    Ok((translation.to_array(), yaw, scale.to_array()))
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
    for record in &placements.placements {
        let name = if record.editor.name.is_empty() { &record.id } else { &record.editor.name };
        let rotation = Quat::from_rotation_y(record.yaw_degrees.to_radians()).to_array();
        let mut node = json!({"name": name, "translation": record.translation, "scale": record.scale, "rotation": rotation});
        let mut fields = serde_json::to_value(record).map_err(|error| error.to_string())?;
        let field_map = fields.as_object_mut().expect("摆放记录总是对象");
        for key in ["id", "translation", "yaw_degrees", "scale"] {
            field_map.remove(key);
        }
        let mut extras = json!({PLACEMENT_ID: record.id});
        let mesh = match record.asset_target {
            target if target.is_builtin() => {
                target.validate()?;
                let recipe = serde_json::to_string(&target).map_err(|error| error.to_string())?;
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
                if mesh_material(&document, mesh as usize)? != material {
                    return Err(format!("场景 GLB 网格 {} 的材质已换代，请重新打开场景", record.id));
                }
                extras[COLLISION] = json!(collision);
                Some(mesh as usize)
            }
            _ if record.asset_path == asset_path => return Err(format!("场景 GLB 内部引用 {} 无效", record.id)),
            _ => None, // 共享资产在 GLB 内保存为带引用的空节点，由原摆放加载器实例化。
        };
        if let Some(mesh) = mesh {
            node["mesh"] = json!(mesh);
            let field_map = fields.as_object_mut().expect("摆放记录总是对象");
            field_map.remove("asset_path");
            field_map.remove("asset_target");
        }
        extras[PLACEMENT_FIELDS] = json!(serde_json::to_string(&fields).map_err(|error| error.to_string())?);
        node["extras"] = extras;
        nodes.push(node);
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
    document["nodes"] = json!(nodes);
    document["scenes"] = json!([{"nodes": (0..nodes.len()).collect::<Vec<_>>() }]);
    document["scene"] = json!(0);
    write_chunks(&document, &bin)
}

fn builtin_material(target: ScenePlacementAssetTarget) -> ([u8; 3], u16, bool) {
    match target {
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
    target: ScenePlacementAssetTarget,
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
    let material = push(document, "materials", json!({"pbrMetallicRoughness": {
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
}
