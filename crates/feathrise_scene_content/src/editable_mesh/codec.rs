//! 旧64槽填充小网格与大型紧凑网格读写同一作者字段，读入时限量并完成一次完整验证。

use super::{
    EditableMeshGeometry, LEGACY_CAPACITY, MAX_EDITABLE_MESH_FACES, MAX_EDITABLE_MESH_VERTICES,
    MeshFace,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, SeqAccess, Visitor},
    ser::{SerializeSeq, SerializeStruct},
};
use std::marker::PhantomData;

impl Serialize for EditableMeshGeometry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let legacy = self.vertex_count() <= LEGACY_CAPACITY && self.face_count() <= LEGACY_CAPACITY;
        let mut state = serializer.serialize_struct("EditableMeshGeometry", 4)?;
        state.serialize_field("vertices_half_mm", &Sequence(self.vertices(), legacy))?;
        state.serialize_field("faces", &Sequence(self.faces(), legacy))?;
        state.serialize_field("vertex_count", &self.vertex_count())?;
        state.serialize_field("face_count", &self.face_count())?;
        state.end()
    }
}

struct Sequence<'a, T>(&'a [T], bool);

impl<T: Serialize + Default> Serialize for Sequence<'_, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let len = if self.1 { LEGACY_CAPACITY } else { self.0.len() };
        let mut seq = serializer.serialize_seq(Some(len))?;
        for value in self.0 {
            seq.serialize_element(value)?;
        }
        if self.1 {
            let empty = T::default();
            for _ in self.0.len()..LEGACY_CAPACITY {
                seq.serialize_element(&empty)?;
            }
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for EditableMeshGeometry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Encoded {
            vertices_half_mm: Bounded<[i32; 3], MAX_EDITABLE_MESH_VERTICES>,
            faces: Bounded<MeshFace, MAX_EDITABLE_MESH_FACES>,
            vertex_count: usize,
            face_count: usize,
        }
        let decoded = Encoded::deserialize(deserializer)?;
        let mut vertices = decoded.vertices_half_mm.0;
        let mut faces = decoded.faces.0;
        if decoded.vertex_count > vertices.len() || decoded.face_count > faces.len() {
            return Err(de::Error::custom("可编辑网格实际点面数量少于声明数量"));
        }
        let padded = vertices.len() == LEGACY_CAPACITY && faces.len() == LEGACY_CAPACITY;
        if padded {
            if vertices[decoded.vertex_count..].iter().any(|&vertex| vertex != [0; 3])
                || faces[decoded.face_count..].iter().any(|&face| face != MeshFace::default())
            {
                return Err(de::Error::custom("可编辑网格未使用的旧格式槽必须清零"));
            }
            vertices.truncate(decoded.vertex_count);
            faces.truncate(decoded.face_count);
        } else if vertices.len() != decoded.vertex_count || faces.len() != decoded.face_count {
            return Err(de::Error::custom("紧凑网格的点面数组必须和声明数量相同"));
        }
        Self::from_owned_parts(vertices, faces).map_err(de::Error::custom)
    }
}

struct Bounded<T, const MAX: usize>(Vec<T>);

impl<'de, T: Deserialize<'de>, const MAX: usize> Deserialize<'de> for Bounded<T, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoundedVisitor<T, const MAX: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for BoundedVisitor<T, MAX> {
            type Value = Bounded<T, MAX>;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "最多{MAX}项作者网格数据")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX));
                while let Some(value) = seq.next_element()? {
                    if values.len() == MAX {
                        return Err(de::Error::custom("可编辑网格数组超过安全内存预算"));
                    }
                    values.push(value);
                }
                Ok(Bounded(values))
            }
        }
        deserializer.deserialize_seq(BoundedVisitor::<T, MAX>(PhantomData))
    }
}
