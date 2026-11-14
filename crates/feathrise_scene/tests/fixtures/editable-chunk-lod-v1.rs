//! 56共享点、54四边面的合法闭壳夹具；同一来源用于真实分块、碰撞与显示验证。
use feathrise_scene_content::{EditableMeshGeometry, MeshFace};
use std::collections::BTreeMap;

pub fn geometry() -> EditableMeshGeometry {
    let mut points = Vec::new();
    let mut lookup = BTreeMap::new();
    let mut faces = Vec::new();
    // 三轴每面分为3×3格；按外法线指定两个切向，保证正式有向流形校验通过。
    for (axis, side, u, v) in [(0, 0, 2, 1), (0, 3, 1, 2), (1, 0, 0, 2),
        (1, 3, 2, 0), (2, 0, 1, 0), (2, 3, 0, 1)] {
        for row in 0..3 { for col in 0..3 {
            let mut face = Vec::new();
            for (x, y) in [(col, row), (col + 1, row), (col + 1, row + 1), (col, row + 1)] {
                let mut point = [0; 3];
                point[axis] = side * 2000 - 3000;
                point[u] = x * 2000 - 3000;
                point[v] = y * 2000 - 3000;
                let next = points.len() as u8;
                face.push(*lookup.entry(point).or_insert_with(|| { points.push(point); next }));
            }
            faces.push(MeshFace::new(&face).unwrap());
        } }
    }
    EditableMeshGeometry::from_parts(&points, &faces).unwrap()
}
