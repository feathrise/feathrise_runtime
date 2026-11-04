//! 所有顶点属性按固定字节碎片冻结；完整重组与减面只在后台进行。
const SOURCE_BYTES: usize = 32 * 1024 * 1024;

use bevy::{
    mesh::{Indices, MeshVertexAttribute, PrimitiveTopology, VertexAttributeValues},
    prelude::*,
};
use super::{copy_static_mesh_attribute_range, empty_static_mesh_attribute};

struct AttributeCapture {
    attribute: MeshVertexAttribute,
    count: usize,
    cursor: usize,
    chunks: Vec<VertexAttributeValues>,
}

pub struct StaticMeshCapture {
    mesh: Mesh,
    attributes: Vec<AttributeCapture>,
    attribute_cursor: usize,
    index_count: Option<usize>,
    index_cursor: usize,
    index_chunks: Vec<Vec<u32>>,
}

impl StaticMeshCapture {
    pub fn new(source: &Mesh) -> Result<Self, String> {
        drop(source.try_attributes().map_err(|error| error.to_string())?);
        if source.primitive_topology() != PrimitiveTopology::TriangleList
            || source
                .try_attribute_option(Mesh::ATTRIBUTE_JOINT_INDEX)
                .ok()
                .flatten()
                .is_some()
            || source
                .try_attribute_option(Mesh::ATTRIBUTE_JOINT_WEIGHT)
                .ok()
                .flatten()
                .is_some()
            || source.try_has_morph_targets().unwrap_or(true)
        {
            return Err("通用网格LOD仅接受无骨骼、无形变的静态三角网格".into());
        }
        let mut mesh = Mesh::new(source.primitive_topology(), source.asset_usage);
        mesh.enable_raytracing = source.enable_raytracing;
        mesh.final_aabb = source.final_aabb;
        let attributes = source
            .try_attributes()
            .map_err(|error| error.to_string())?
            .take(65)
            .map(|(attribute, values)| AttributeCapture {
                attribute: attribute.clone(),
                count: values.len(),
                cursor: 0,
                chunks: Vec::new(),
            })
            .collect::<Vec<_>>();
        if attributes.len() > 64 {
            return Err("静态网格属性布局超过当前捕获规格，保留原始网格".into());
        }
        let bytes = static_mesh_bytes(source);
        if bytes > SOURCE_BYTES {
            return Err("静态网格超过32MiB后台冻结工作信用，保留完整作者源与原始显示".into());
        }
        let index_count = source
            .try_indices_option()
            .map_err(|error| error.to_string())?
            .map(Indices::len);
        Ok(Self {
            mesh,
            attributes,
            attribute_cursor: 0,
            index_count,
            index_cursor: 0,
            index_chunks: Vec::new(),
        })
    }

    pub fn step(&mut self, source: &Mesh, mut bytes: usize) -> Result<bool, String> {
        while bytes >= 64 && self.attribute_cursor < self.attributes.len() {
            let capture = &mut self.attributes[self.attribute_cursor];
            let values = source
                .try_attribute_option(capture.attribute.id)
                .map_err(|error| error.to_string())?
                .ok_or("静态网格属性在捕获期间被移除")?;
            if values.len() != capture.count {
                return Err("静态网格属性在捕获期间换代".into());
            }
            if capture.cursor == capture.count {
                self.attribute_cursor += 1;
                continue;
            }
            let stride = capture.attribute.format.size() as usize;
            let count = (bytes.min(64 * 1024) / stride).min(capture.count - capture.cursor);
            if count == 0 {
                return Ok(false);
            }
            let mut chunk = empty_static_mesh_attribute(values);
            copy_static_mesh_attribute_range(
                values,
                &mut chunk,
                capture.cursor..capture.cursor + count,
            )?;
            capture.chunks.push(chunk); // 每次只分配本片，不在主线程扩容整张网格的顶点数组。
            capture.cursor += count;
            bytes -= count * stride;
        }
        if self.attribute_cursor != self.attributes.len() {
            return Ok(false);
        }
        if let Some(count) = self.index_count {
            let indices = source
                .try_indices_option()
                .map_err(|error| error.to_string())?
                .ok_or("静态网格索引在捕获期间被移除")?;
            if indices.len() != count {
                return Err("静态网格索引在捕获期间换代".into());
            }
            let end = (self.index_cursor + bytes.min(64 * 1024) / 4).min(count);
            if end != self.index_cursor {
                let values = match indices {
                    Indices::U32(values) => values[self.index_cursor..end].to_vec(),
                    Indices::U16(values) => values[self.index_cursor..end]
                        .iter()
                        .map(|value| u32::from(*value))
                        .collect(),
                };
                self.index_chunks.push(values);
                self.index_cursor = end;
            }
            Ok(self.index_cursor == count)
        } else {
            Ok(true)
        }
    }

    pub fn finish(mut self) -> Result<Mesh, String> {
        for attribute in self.attributes {
            let first = attribute.chunks.first().ok_or("静态网格存在空顶点属性")?;
            let mut values = empty_static_mesh_attribute(first);
            for chunk in attribute.chunks {
                copy_static_mesh_attribute_range(&chunk, &mut values, 0..chunk.len())?;
            }
            self.mesh.insert_attribute(attribute.attribute, values);
        }
        if self.index_count.is_some() {
            self.mesh.insert_indices(Indices::U32(
                self.index_chunks.into_iter().flatten().collect(),
            ));
        }
        Ok(self.mesh)
    }
}

pub fn static_mesh_bytes(mesh: &Mesh) -> usize {
    let attributes = mesh
        .try_attributes()
        .ok()
        .map(|values| {
            values.take(65).fold(0usize, |total, (attribute, values)| {
                total.saturating_add(
                    values
                        .len()
                        .saturating_mul(attribute.format.size() as usize),
                )
            })
        })
        .unwrap_or(usize::MAX);
    attributes.saturating_add(
        mesh.try_indices_option()
            .ok()
            .flatten()
            .map_or(0, |indices| indices.len().saturating_mul(4)),
    )
}
