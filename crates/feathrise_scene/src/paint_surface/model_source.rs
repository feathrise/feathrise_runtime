//! 后台读取原始静态 GLB 与内置作者几何；不用显示 LOD 或碰撞开关决定模型能否种植。

use super::{MeshSurfacePrimitive, MeshSurfaceSnapshot};
use bevy::prelude::*;
use feathrise_scene_content::{
    EditableMeshGeometry, ScenePlacementAssetTarget, ScenePlacementRecord,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    path::{Component, Path},
    sync::Arc,
};

const MAX_GLB_BYTES: u64 = 128 * 1024 * 1024;

pub fn load_model_surface(
    asset_root: &Path,
    record: &ScenePlacementRecord,
) -> Result<MeshSurfaceSnapshot, String> {
    load_model_surface_source(asset_root, &record.asset_path, record.asset_target)
}

/// 运行时读取实例冻结的不可变资产身份；不为查询几何复制整份作者记录或读取碰撞组件。
pub fn load_model_surface_source(
    asset_root: &Path,
    asset_path: &str,
    target: ScenePlacementAssetTarget,
) -> Result<MeshSurfaceSnapshot, String> {
    read_model_surface_source(asset_root, asset_path, target)?.build()
}

/// IO池只冻结原始字节；JSON解析、三角拓扑、邻接关系和BVH在计算池调用build。
pub struct ModelSurfaceSourceInput {
    target: ScenePlacementAssetTarget,
    bytes: Option<Vec<u8>>,
}
impl ModelSurfaceSourceInput {
    pub fn build(self) -> Result<MeshSurfaceSnapshot, String> {
        let target = self.target;
        if let Some(bytes) = self.bytes {
            return build_glb_surface(&bytes, target);
        }
        build_builtin_surface(target)
    }
}

pub fn read_model_surface_source(
    asset_root: &Path,
    asset_path: &str,
    target: ScenePlacementAssetTarget,
) -> Result<ModelSurfaceSourceInput, String> {
    target.validate()?;
    if target.is_builtin() {
        return Ok(ModelSurfaceSourceInput {
            target,
            bytes: None,
        });
    }
    let bytes = read_glb_bytes(asset_root, asset_path)?;
    Ok(ModelSurfaceSourceInput {
        target,
        bytes: Some(bytes),
    })
}
fn build_builtin_surface(target: ScenePlacementAssetTarget) -> Result<MeshSurfaceSnapshot, String> {
    let geometry = match target {
        ScenePlacementAssetTarget::EditableMesh { geometry, .. } => geometry,
        target => EditableMeshGeometry::from_cuboid_vertices(
            target
                .editable_vertices_half_mm()
                .ok_or("模型内置顶点无效")?,
        ),
    };
    geometry.validate()?;
    let positions = geometry
        .vertices()
        .iter()
        .map(|point| point.map(|v| v as f32 * 0.0005))
        .collect();
    let indices = geometry
        .triangles_with_faces()
        .into_iter()
        .flat_map(|(_, triangle)| triangle.map(|i| i as u32))
        .collect();
    MeshSurfaceSnapshot::build(vec![MeshSurfacePrimitive {
        primitive: 0,
        positions: Arc::new(positions),
        indices: Arc::new(indices),
        primitive_to_object: Mat4::IDENTITY,
    }])
}
fn read_glb_bytes(asset_root: &Path, asset_path: &str) -> Result<Vec<u8>, String> {
    let relative = asset_path.split('#').next().unwrap_or_default();
    let path = Path::new(relative);
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("模型种植面来源必须位于登记资产根内".into());
    }
    if path
        .extension()
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("glb"))
    {
        return Err("当前静态模型种植面需要内嵌缓冲的 GLB 来源".into());
    }
    let root = asset_root.canonicalize().map_err(|e| e.to_string())?;
    let path = root.join(path).canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(&root) {
        return Err("模型种植面来源越出资产根".into());
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > MAX_GLB_BYTES {
        return Err("模型 GLB 超过128MiB读取预算".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_GLB_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_GLB_BYTES {
        return Err("模型 GLB 在读取期间超过预算".into());
    }
    Ok(bytes)
}

pub(super) fn build_glb_surface(
    bytes: &[u8],
    target: ScenePlacementAssetTarget,
) -> Result<MeshSurfaceSnapshot, String> {
    if bytes.len() < 12
        || &bytes[..4] != b"glTF"
        || number(bytes, 4)? != 2
        || number(bytes, 8)? as usize != bytes.len()
    {
        return Err("模型 GLB 文件头或长度无效".into());
    }
    let mut cursor = 12usize;
    let mut document = None;
    let mut bin = None;
    while cursor < bytes.len() {
        let len = number(bytes, cursor)? as usize;
        let tag = bytes.get(cursor + 4..cursor + 8).ok_or("GLB 块头越界")?;
        let start = cursor.checked_add(8).ok_or("GLB 块地址溢出")?;
        let end = start.checked_add(len).ok_or("GLB 块长度溢出")?;
        let data = bytes.get(start..end).ok_or("GLB 块数据越界")?;
        if tag == b"JSON" {
            if document.is_some() {
                return Err("GLB JSON 块重复".into());
            }
            document = Some(serde_json::from_slice::<Value>(data).map_err(|e| e.to_string())?);
        } else if tag == b"BIN\0" {
            if bin.is_some() {
                return Err("GLB BIN 块重复".into());
            }
            bin = Some(data);
        }
        cursor = end;
    }
    let document = document.ok_or("GLB 缺少 JSON 块")?;
    let bin = bin.ok_or("GLB 缺少内嵌 BIN 缓冲")?;
    if let Some(buffers) = document.get("buffers").and_then(Value::as_array)
        && (buffers.len() != 1
            || buffers[0].get("uri").is_some()
            || buffers[0]["byteLength"]
                .as_u64()
                .is_none_or(|len| len > bin.len() as u64))
    {
        return Err("模型种植面只读取 GLB 内嵌的单个受检缓冲".into());
    }
    let mut primitives = Vec::new();
    match target {
        ScenePlacementAssetTarget::Primitive {
            mesh, primitive, ..
        } => {
            primitives.push(read_primitive(
                &document,
                bin,
                mesh as usize,
                primitive as usize,
                0,
                Mat4::IDENTITY,
            )?);
        }
        ScenePlacementAssetTarget::Scene0 => {
            let scene = document["scenes"]
                .as_array()
                .and_then(|scenes| scenes.first())
                .ok_or("GLB 缺少 Scene0")?;
            let roots = scene["nodes"].as_array().ok_or("GLB Scene0 缺少节点")?;
            let mut pending = roots
                .iter()
                .rev()
                .map(|node| Ok((index(node)?, Mat4::IDENTITY)))
                .collect::<Result<Vec<_>, String>>()?;
            let mut visited = BTreeSet::new();
            while let Some((node_index, parent)) = pending.pop() {
                if !visited.insert(node_index) {
                    return Err("GLB 模型层级存在重复节点或环".into());
                }
                let node = document["nodes"]
                    .as_array()
                    .and_then(|nodes| nodes.get(node_index))
                    .ok_or("GLB 节点越界")?;
                let transform = parent * node_transform(node)?;
                if node.get("skin").is_some() {
                    return Err("首版种植面仅支持无蒙皮静态模型".into());
                }
                if let Some(mesh) = node.get("mesh") {
                    let mesh = index(mesh)?;
                    let count = document["meshes"]
                        .as_array()
                        .and_then(|meshes| meshes.get(mesh))
                        .and_then(|mesh| mesh["primitives"].as_array())
                        .ok_or("GLB 网格来源无效")?
                        .len();
                    for primitive in 0..count {
                        primitives.push(read_primitive(
                            &document,
                            bin,
                            mesh,
                            primitive,
                            u32::try_from(primitives.len())
                                .map_err(|_| "GLB primitive 数量溢出")?,
                            transform,
                        )?);
                    }
                }
                if let Some(children) = node.get("children") {
                    for child in children
                        .as_array()
                        .ok_or("GLB 子节点列表无效")?
                        .iter()
                        .rev()
                    {
                        pending.push((index(child)?, transform));
                    }
                }
            }
        }
        _ => return Err("模型 GLB 目标类型无效".into()),
    }
    MeshSurfaceSnapshot::build(primitives)
}

fn read_primitive(
    document: &Value,
    bin: &[u8],
    mesh: usize,
    primitive: usize,
    id: u32,
    frame: Mat4,
) -> Result<MeshSurfacePrimitive, String> {
    let primitive = document["meshes"]
        .as_array()
        .and_then(|meshes| meshes.get(mesh))
        .and_then(|mesh| mesh["primitives"].as_array())
        .and_then(|primitives| primitives.get(primitive))
        .ok_or("GLB primitive 越界")?;
    if primitive["mode"].as_u64().unwrap_or(4) != 4
        || primitive.get("targets").is_some()
        || primitive["attributes"].get("JOINTS_0").is_some()
        || primitive["attributes"].get("WEIGHTS_0").is_some()
    {
        return Err("模型种植面需要无蒙皮、无形变的静态三角列表".into());
    }
    let position_accessor = accessor(
        document,
        bin,
        index(&primitive["attributes"]["POSITION"])?,
        "VEC3",
        12,
    )?;
    if position_accessor.component != 5126 {
        return Err("模型位置属性需要 Float32 三维坐标".into());
    }
    let mut positions = Vec::with_capacity(position_accessor.count);
    for row in 0..position_accessor.count {
        let values = position_accessor.row(row, 12)?;
        positions.push(std::array::from_fn(|i| {
            f32::from_le_bytes(values[i * 4..i * 4 + 4].try_into().unwrap())
        }));
    }
    let indices = if let Some(source) = primitive.get("indices") {
        let accessor = accessor(document, bin, index(source)?, "SCALAR", 0)?;
        let width = match accessor.component {
            5121 => 1,
            5123 => 2,
            5125 => 4,
            _ => return Err("模型三角索引类型无效".into()),
        };
        (0..accessor.count)
            .map(|row| {
                let bytes = accessor.row(row, width)?;
                Ok(match width {
                    1 => u32::from(bytes[0]),
                    2 => u32::from(u16::from_le_bytes(bytes.try_into().unwrap())),
                    _ => u32::from_le_bytes(bytes.try_into().unwrap()),
                })
            })
            .collect::<Result<Vec<_>, String>>()?
    } else {
        (0..u32::try_from(positions.len()).map_err(|_| "模型顶点数量溢出")?).collect()
    };
    Ok(MeshSurfacePrimitive {
        primitive: id,
        positions: Arc::new(positions),
        indices: Arc::new(indices),
        primitive_to_object: frame,
    })
}

struct Accessor<'a> {
    bytes: &'a [u8],
    offset: usize,
    stride: usize,
    count: usize,
    component: u32,
}
impl Accessor<'_> {
    fn row(&self, index: usize, width: usize) -> Result<&[u8], String> {
        let stride = if self.stride == 0 { width } else { self.stride };
        if stride < width {
            return Err("模型顶点步长小于属性宽度".into());
        }
        let start = index
            .checked_mul(stride)
            .and_then(|value| value.checked_add(self.offset))
            .ok_or("模型属性地址溢出")?;
        self.bytes
            .get(start..start.checked_add(width).ok_or("模型属性宽度溢出")?)
            .ok_or_else(|| "模型属性读取越界".into())
    }
}
fn accessor<'a>(
    document: &Value,
    bin: &'a [u8],
    id: usize,
    kind: &str,
    width: usize,
) -> Result<Accessor<'a>, String> {
    let accessor = document["accessors"]
        .as_array()
        .and_then(|values| values.get(id))
        .ok_or("GLB accessor 越界")?;
    if accessor["type"].as_str() != Some(kind) || accessor.get("sparse").is_some() {
        return Err("GLB accessor 形状或稀疏布局不受支持".into());
    }
    let view = index(&accessor["bufferView"])?;
    let view = document["bufferViews"]
        .as_array()
        .and_then(|values| values.get(view))
        .ok_or("GLB bufferView 越界")?;
    if view["buffer"].as_u64() != Some(0) {
        return Err("GLB bufferView 不是内嵌缓冲".into());
    }
    let start = optional_index(view, "byteOffset")?;
    let length = index(&view["byteLength"])?;
    let bytes = bin
        .get(start..start.checked_add(length).ok_or("GLB bufferView 溢出")?)
        .ok_or("GLB bufferView 超出 BIN")?;
    let count = index(&accessor["count"])?;
    if count.saturating_mul(width.max(4)) > 128 * 1024 * 1024 {
        return Err("模型属性超过128MiB读取预算".into());
    }
    Ok(Accessor {
        bytes,
        offset: optional_index(accessor, "byteOffset")?,
        stride: optional_index(view, "byteStride")?,
        count,
        component: u32::try_from(index(&accessor["componentType"])?)
            .map_err(|_| "模型属性类型越界")?,
    })
}
fn optional_index(value: &Value, key: &str) -> Result<usize, String> {
    value
        .get(key)
        .map(index)
        .transpose()
        .map(|v| v.unwrap_or(0))
}
fn index(value: &Value) -> Result<usize, String> {
    usize::try_from(value.as_u64().ok_or("GLB 整数无效")?).map_err(|_| "GLB 整数溢出".into())
}
fn number(bytes: &[u8], offset: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("GLB 头部越界")?
            .try_into()
            .unwrap(),
    ))
}
fn node_transform(node: &Value) -> Result<Mat4, String> {
    let matrix = if let Some(matrix) = node.get("matrix") {
        let values: [f32; 16] =
            serde_json::from_value(matrix.clone()).map_err(|e| e.to_string())?;
        Mat4::from_cols_array(&values)
    } else {
        let translation: [f32; 3] = node
            .get("translation")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or([0.; 3]);
        let rotation: [f32; 4] = node
            .get("rotation")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or([0., 0., 0., 1.]);
        let scale: [f32; 3] = node
            .get("scale")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or([1.; 3]);
        let rotation = Quat::from_array(rotation);
        if !rotation.is_finite() || (rotation.length_squared() - 1.0).abs() > 0.001 {
            return Err("GLB 节点旋转无效".into());
        }
        Mat4::from_scale_rotation_translation(
            Vec3::from_array(scale),
            rotation,
            Vec3::from_array(translation),
        )
    };
    if !super::geometry::valid_matrix(matrix) {
        return Err("GLB 节点姿态无效".into());
    }
    Ok(matrix)
}
