//! 可编辑网格是经过一次完整验证的不可变来源。存档、历史、显示、碰撞、拾取共享同一拓扑。

use crate::{EDITABLE_CUBOID_EDGES, EDITABLE_CUBOID_FACES, ScenePlacementAssetTarget};
use bevy::math::DVec3;
use serde::{Deserialize, Serialize};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

mod codec;
mod spatial;
mod triangulation;
mod validation;
#[cfg(test)]
mod tests;

/// 单个来源及其构造、派生数据的保守字节预算。编辑操作以空间代价判断，按实际点面规模扩容。
pub const MAX_EDITABLE_MESH_BYTES: usize = 96 * 1024 * 1024;
pub const MAX_EDITABLE_MESH_FACE_VERTICES: usize = 16;
// 持有坐标、二维邻接、闭壳约2V三角/3V边、包围层级，并为建造过渡数组与哈希表预留空间。
const VERTEX_BYTES: usize = 384;
const FACE_BYTES: usize = 160;
/// 单项解码与输入循环的防御上界；实际容量以整体内存预算为准。
pub const MAX_EDITABLE_MESH_VERTICES: usize = MAX_EDITABLE_MESH_BYTES / VERTEX_BYTES;
pub const MAX_EDITABLE_MESH_FACES: usize = MAX_EDITABLE_MESH_BYTES / FACE_BYTES;
pub const MAX_EDITABLE_MESH_EDGES: usize = 3 * MAX_EDITABLE_MESH_VERTICES - 6;
pub const MAX_EDITABLE_MESH_TRIANGLES: usize = 2 * MAX_EDITABLE_MESH_VERTICES - 4;
const LEGACY_CAPACITY: usize = 64;

/// 保守估计权威几何、派生空间索引与构造期内存，None表示超过usize范围。
pub fn editable_mesh_required_bytes(vertices: usize, faces: usize) -> Option<usize> {
    vertices.checked_mul(VERTEX_BYTES)?.checked_add(faces.checked_mul(FACE_BYTES)?)
}

/// 一面只存作者有序边界。未用槽清零，保留旧小网格的每面序列化形态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MeshFace {
    vertices: [u32; MAX_EDITABLE_MESH_FACE_VERTICES],
    vertex_count: u8,
}

impl MeshFace {
    pub fn new(vertices: &[u32]) -> Result<Self, String> {
        let mut face = Self::default();
        if !(3..=MAX_EDITABLE_MESH_FACE_VERTICES).contains(&vertices.len()) {
            return Err("单个网格面需要3～16个边界顶点".into());
        }
        face.vertex_count = vertices.len() as u8;
        face.vertices[..vertices.len()].copy_from_slice(vertices);
        face.validate(usize::MAX)?;
        Ok(face)
    }

    pub fn indices(&self) -> &[u32] {
        &self.vertices[..usize::from(self.vertex_count).min(MAX_EDITABLE_MESH_FACE_VERTICES)]
    }

    fn validate(&self, vertices: usize) -> Result<(), String> {
        let count = usize::from(self.vertex_count);
        if !(3..=MAX_EDITABLE_MESH_FACE_VERTICES).contains(&count)
            || self.vertices[count..].iter().any(|&vertex| vertex != 0)
        {
            return Err("网格面需要3～16个索引，未使用索引必须清零".into());
        }
        if self.indices().iter().enumerate().any(|(index, &vertex)| {
            vertex as usize >= vertices || self.indices()[..index].contains(&vertex)
        }) {
            return Err("网格面索引无效或重复".into());
        }
        Ok(())
    }
}

/// 草稿和历史取样只复制引用；正式采用共享一次构造的拓扑与空间索引。
#[derive(Clone)]
pub struct EditableMeshGeometry(Arc<MeshData>);

struct MeshData {
    open_surface: bool,
    vertices: Vec<[i32; 3]>,
    faces: Vec<MeshFace>,
    edges: Vec<[usize; 2]>,
    edge_faces: Vec<[usize; 2]>,
    triangles: Vec<[usize; 3]>,
    surface: Vec<(usize, [usize; 3])>,
    face_starts: Vec<usize>,
    points: Vec<DVec3>,
    spatial: spatial::TriangleIndex,
    bounds: ([i32; 3], [i32; 3]),
    fingerprint: u64,
}

impl std::fmt::Debug for EditableMeshGeometry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("EditableMeshGeometry")
            .field("open_surface", &self.is_open())
            .field("vertices", &self.vertex_count())
            .field("faces", &self.face_count())
            .field("fingerprint", &self.0.fingerprint)
            .finish()
    }
}

impl PartialEq for EditableMeshGeometry {
    fn eq(&self, other: &Self) -> bool {
        self.same_storage(other) || (self.0.fingerprint == other.0.fingerprint
            && self.is_open() == other.is_open()
            && self.vertices() == other.vertices() && self.faces() == other.faces())
    }
}

impl Eq for EditableMeshGeometry {}

impl Hash for EditableMeshGeometry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.fingerprint.hash(state);
    }
}

impl EditableMeshGeometry {
    /// 创建朝 +Y 的单个 XZ 四边面。毫米尺寸正好对应本地半毫米坐标的半边长。
    pub fn from_plane_size_mm(size_mm: [u32; 2]) -> Result<Self, String> {
        if size_mm.iter().any(|size| !(1..=10_000_000).contains(size)) {
            return Err("平面尺寸必须在 0.001 至 10000 米之间".into());
        }
        let [x, z] = size_mm.map(|size| size as i32);
        let vertices = vec![[-x, 0, -z], [-x, 0, z], [x, 0, z], [x, 0, -z]];
        Self::construct(vertices, vec![MeshFace::new(&[0, 1, 2, 3])?], true)
    }

    pub fn is_open(&self) -> bool { self.0.open_surface }

    /// 保留已校验的旧八点调用入口；公开作者入口通过受检构造返回错误。
    pub fn from_cuboid_vertices(vertices: [[i32; 3]; 8]) -> Self {
        Self::checked_cuboid(vertices).expect("构造内置八点网格前必须校验来源")
    }

    fn checked_cuboid(vertices: [[i32; 3]; 8]) -> Result<Self, String> {
        let faces = EDITABLE_CUBOID_FACES.map(|face| MeshFace::new(&face.map(|vertex| vertex as u32)).expect("内置面固定四点"));
        Self::from_owned_parts(vertices.to_vec(), faces.to_vec())
    }

    pub fn from_parts(vertices: &[[i32; 3]], faces: &[MeshFace]) -> Result<Self, String> {
        Self::check_budget(vertices.len(), faces.len(), false)?;
        Self::from_owned_parts(vertices.to_vec(), faces.to_vec())
    }

    pub fn from_owned_parts(vertices: Vec<[i32; 3]>, faces: Vec<MeshFace>) -> Result<Self, String> {
        Self::construct(vertices, faces, false)
    }

    /// 拓扑操作继承作者来源的开口或闭合约定，不能让坏闭壳悄悄降级为开口面。
    pub fn with_parts(&self, vertices: Vec<[i32; 3]>, faces: Vec<MeshFace>) -> Result<Self, String> {
        Self::construct(vertices, faces, self.is_open())
    }

    fn construct(vertices: Vec<[i32; 3]>, faces: Vec<MeshFace>, open_surface: bool) -> Result<Self, String> {
        Self::check_budget(vertices.len(), faces.len(), open_surface)?;
        let derived = validation::validate(&vertices, &faces, open_surface)?;
        let mut hasher = DefaultHasher::new();
        open_surface.hash(&mut hasher);
        vertices.hash(&mut hasher);
        faces.hash(&mut hasher);
        Ok(Self(Arc::new(MeshData {
            open_surface,
            vertices,
            faces,
            edges: derived.edges,
            edge_faces: derived.edge_faces,
            triangles: derived.triangles,
            surface: derived.surface,
            face_starts: derived.face_starts,
            points: derived.points,
            spatial: derived.spatial,
            bounds: derived.bounds,
            fingerprint: hasher.finish(),
        })))
    }

    fn check_budget(vertices: usize, faces: usize, open_surface: bool) -> Result<(), String> {
        if open_surface && (vertices < 3 || faces == 0) {
            return Err("开口网格至少需要3个共享顶点和1个面".into());
        }
        if !open_surface && (vertices < 4 || faces < 4) {
            return Err("可编辑闭壳至少需要4个共享顶点和4个面".into());
        }
        if editable_mesh_required_bytes(vertices, faces).is_none_or(|bytes| bytes > MAX_EDITABLE_MESH_BYTES) {
            return Err("可编辑网格的原始几何与派生数据超过96 MiB安全内存预算".into());
        }
        Ok(())
    }

    /// 独立变换不回写共享来源；采用前用新坐标构造一次完整候选。
    pub fn with_vertices(&self, vertices: Vec<[i32; 3]>) -> Result<Self, String> {
        if vertices == self.vertices() {
            return Ok(self.clone());
        }
        Self::check_budget(vertices.len(), self.face_count(), self.is_open())?;
        self.with_parts(vertices, self.faces().to_vec())
    }

    pub fn same_storage(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// 已采用共享来源实际保留的数组容量；历史可在常数时间计费，无需再扫描点面。
    pub fn allocation_bytes(&self) -> usize {
        let data = &self.0;
        size_of::<MeshData>()
            + data.vertices.capacity() * size_of::<[i32; 3]>()
            + data.faces.capacity() * size_of::<MeshFace>()
            + data.edges.capacity() * size_of::<[usize; 2]>()
            + data.edge_faces.capacity() * size_of::<[usize; 2]>()
            + data.triangles.capacity() * size_of::<[usize; 3]>()
            + data.surface.capacity() * size_of::<(usize, [usize; 3])>()
            + data.face_starts.capacity() * size_of::<usize>()
            + data.points.capacity() * size_of::<DVec3>()
            + data.spatial.allocation_bytes()
    }

    pub fn vertices(&self) -> &[[i32; 3]] { &self.0.vertices }

    pub fn faces(&self) -> &[MeshFace] { &self.0.faces }

    pub fn vertex_count(&self) -> usize { self.0.vertices.len() }

    pub fn face_count(&self) -> usize { self.0.faces.len() }

    pub fn bounds_half_mm(&self) -> ([i32; 3], [i32; 3]) { self.0.bounds }

    /// 真实作者边固定顺序；不把渲染三角对角线混入选区。
    pub fn edges(&self) -> &[[usize; 2]] { &self.0.edges }

    /// 与作者边索引对齐的相邻面。开口网格没有面的边侧使用 usize::MAX。
    pub fn edge_faces(&self) -> &[[usize; 2]] { &self.0.edge_faces }

    /// 遍历指定边真实相邻的一个或两个面，编辑工具无需处理开口边的空侧。
    pub fn adjacent_faces(&self, edge: usize) -> impl Iterator<Item = usize> {
        self.edge_faces().get(edge).into_iter().flatten().copied().filter(|&face| face != usize::MAX)
    }

    pub fn triangles(&self) -> &[[usize; 3]] { &self.0.triangles }

    pub fn triangles_with_faces(&self) -> &[(usize, [usize; 3])] { &self.0.surface }

    pub fn face_triangles(&self, face: usize) -> Option<&[[usize; 3]]> {
        let start = *self.0.face_starts.get(face)?;
        let end = *self.0.face_starts.get(face.checked_add(1)?)?;
        Some(&self.0.triangles[start..end])
    }

    /// 局部半毫米射线保持调用者t单位，方向无需归一。返回正式三角索引和t。
    pub fn nearest_triangle(&self, origin: DVec3, direction: DVec3, max_t: f64) -> Option<(usize, f64)> {
        self.0.spatial.nearest_triangle(origin, direction, max_t, self.triangles(), &self.0.points)
    }

    /// 兼容原显式校验入口。受检构造是唯一写入边界，共享引用已经具备完整校验结果。
    pub fn validate(&self) -> Result<(), String> { Ok(()) }
}

impl ScenePlacementAssetTarget {
    /// 元素变换只改变坐标时保留旧八点来源；已拓扑编辑的对象继续使用完整网格来源。
    pub fn with_editable_geometry(&self, geometry: EditableMeshGeometry) -> Result<Self, String> {
        let (color_srgb, roughness_milli, collision) = match *self {
            Self::Cuboid { color_srgb, roughness_milli, collision, .. }
            | Self::EditableCuboid { color_srgb, roughness_milli, collision, .. }
            | Self::EditableMesh { color_srgb, roughness_milli, collision, .. } => (color_srgb, roughness_milli, collision),
            _ => return Err("当前对象不支持网格编辑".into()),
        };
        let legacy_faces = EDITABLE_CUBOID_FACES.map(|face| MeshFace::new(&face.map(|vertex| vertex as u32)).expect("内置面固定四点"));
        let target = if !matches!(self, Self::EditableMesh { .. })
            && geometry.vertex_count() == 8 && geometry.faces() == legacy_faces
        {
            Self::EditableCuboid {
                vertices_half_mm: geometry.vertices().try_into().expect("已检查八点数量"),
                color_srgb,
                roughness_milli,
                collision,
            }
        } else {
            Self::EditableMesh { geometry, color_srgb, roughness_milli, collision }
        };
        target.validate()?;
        Ok(target)
    }

    pub fn editable_geometry(&self) -> Option<EditableMeshGeometry> {
        if let Self::EditableMesh { geometry, .. } = self {
            return Some(geometry.clone());
        }
        self.validate().ok()?;
        EditableMeshGeometry::checked_cuboid(self.editable_vertices_half_mm()?).ok()
    }
}
