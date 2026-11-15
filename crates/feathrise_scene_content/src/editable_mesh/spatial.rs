//! 作者网格的局部三角包围层级。构造期复核整壳相交，之后共享给面拾取与遮挡。

use bevy::math::DVec3;
use std::ops::Range;

const LEAF_TRIANGLES: usize = 8;

#[derive(Clone, Copy, Debug)]
struct Bounds {
    min: DVec3,
    max: DVec3,
}

impl Bounds {
    fn triangle([a, b, c]: [DVec3; 3]) -> Self {
        Self { min: a.min(b).min(c), max: a.max(b).max(c) }
    }

    fn union(self, other: Self) -> Self {
        Self { min: self.min.min(other.min), max: self.max.max(other.max) }
    }

    fn center(self) -> DVec3 {
        (self.min + self.max) * 0.5
    }

    fn overlaps(self, other: Self) -> bool {
        !self.min.cmpgt(other.max).any() && !other.min.cmpgt(self.max).any()
    }

    fn ray_entry(self, origin: DVec3, direction: DVec3, mut far: f64) -> Option<f64> {
        let mut near: f64 = 0.0;
        for axis in 0..3 {
            if direction[axis] == 0.0 {
                if origin[axis] < self.min[axis] || origin[axis] > self.max[axis] {
                    return None;
                }
                continue;
            }
            let a = (self.min[axis] - origin[axis]) / direction[axis];
            let b = (self.max[axis] - origin[axis]) / direction[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return None;
            }
        }
        Some(near)
    }
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Bounds,
    first: u32,
    second: u32,
    leaf: bool,
}

impl Node {
    fn range(self) -> Range<usize> {
        self.first as usize..self.second as usize
    }
}

#[derive(Debug, Default)]
pub(super) struct TriangleIndex {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

impl TriangleIndex {
    pub(super) fn allocation_bytes(&self) -> usize {
        self.nodes.capacity() * size_of::<Node>() + self.order.capacity() * size_of::<u32>()
    }

    pub(super) fn build(
        triangles: &[[usize; 3]],
        points: &[DVec3],
        validate_mesh: bool,
    ) -> Result<Self, String> {
        let bounds: Vec<_> = triangles.iter()
            .map(|triangle| Bounds::triangle(triangle.map(|vertex| points[vertex])))
            .collect();
        let mut order: Vec<_> = (0..bounds.len() as u32).collect();
        let mut nodes = Vec::new();
        if !order.is_empty() {
            Self::build_node(&mut nodes, &mut order, 0, &bounds);
        }
        let index = Self { nodes, order };
        if validate_mesh && !index.nodes.is_empty() {
            index.check_overlaps(0, 0, &bounds, triangles, points)?;
        }
        Ok(index)
    }

    fn build_node(nodes: &mut Vec<Node>, order: &mut [u32], start: usize, triangles: &[Bounds]) -> u32 {
        let first = triangles[order[0] as usize];
        let (bounds, centers) = order[1..].iter().fold(
            (first, Bounds { min: first.center(), max: first.center() }),
            |(bounds, centers), &triangle| {
                let next = triangles[triangle as usize];
                let center = next.center();
                (bounds.union(next), centers.union(Bounds { min: center, max: center }))
            },
        );
        let id = nodes.len() as u32;
        nodes.push(Node { bounds, first: start as u32, second: (start + order.len()) as u32, leaf: true });
        if order.len() <= LEAF_TRIANGLES {
            return id;
        }
        let span = centers.max - centers.min;
        let axis = if span.x >= span.y && span.x >= span.z { 0 } else if span.y >= span.z { 1 } else { 2 };
        let middle = order.len() / 2;
        // 中位选择让树深保持对数。相同中心按真实三角序号排序，避免构造随哈希遍历改变。
        order.select_nth_unstable_by(middle, |a, b| {
            triangles[*a as usize].center()[axis]
                .total_cmp(&triangles[*b as usize].center()[axis])
                .then_with(|| a.cmp(b))
        });
        let (left, right) = order.split_at_mut(middle);
        let first = Self::build_node(nodes, left, start, triangles);
        let second = Self::build_node(nodes, right, start + middle, triangles);
        nodes[id as usize] = Node { bounds, first, second, leaf: false };
        id
    }

    fn check_overlaps(
        &self,
        a: u32,
        b: u32,
        triangles: &[Bounds],
        indices: &[[usize; 3]],
        points: &[DVec3],
    ) -> Result<(), String> {
        let first = self.nodes[a as usize];
        let second = self.nodes[b as usize];
        if !first.bounds.overlaps(second.bounds) {
            return Ok(());
        }
        if first.leaf && second.leaf {
            for i in first.range() {
                let ai = self.order[i] as usize;
                for j in second.range() {
                    if a == b && i >= j {
                        continue;
                    }
                    let bi = self.order[j] as usize;
                    if triangles[ai].overlaps(triangles[bi])
                        && crate::editable_cuboid::triangles_intersect_illegally(indices[ai], indices[bi], points)
                    {
                        return Err("可编辑网格表面不能相互穿透或重叠".into());
                    }
                }
            }
        } else if a == b {
            // 同一子树只比较各自内部和一次两侧交叉 pair，跳过与自身重复比较的整片候选。
            self.check_overlaps(first.first, first.first, triangles, indices, points)?;
            self.check_overlaps(first.first, first.second, triangles, indices, points)?;
            self.check_overlaps(first.second, first.second, triangles, indices, points)?;
        } else if first.leaf {
            self.check_overlaps(a, second.first, triangles, indices, points)?;
            self.check_overlaps(a, second.second, triangles, indices, points)?;
        } else {
            self.check_overlaps(first.first, b, triangles, indices, points)?;
            self.check_overlaps(first.second, b, triangles, indices, points)?;
        }
        Ok(())
    }

    pub(super) fn nearest_triangle(
        &self,
        origin: DVec3,
        direction: DVec3,
        max_t: f64,
        triangles: &[[usize; 3]],
        points: &[DVec3],
    ) -> Option<(usize, f64)> {
        if self.nodes.is_empty() || !origin.is_finite() || !direction.is_finite()
            || direction == DVec3::ZERO || max_t.is_nan() || max_t < 0.0
        {
            return None;
        }
        let mut hit = None;
        let mut ray = RayQuery { origin, direction, max_t, triangles, points };
        self.query_node(0, &mut ray, &mut hit);
        hit
    }

    fn query_node(&self, node: u32, ray: &mut RayQuery<'_>, hit: &mut Option<(usize, f64)>) {
        let node = self.nodes[node as usize];
        if node.bounds.ray_entry(ray.origin, ray.direction, ray.max_t).is_none() {
            return;
        }
        if node.leaf {
            for ordered in node.range() {
                let triangle = self.order[ordered] as usize;
                if let Some(distance) = ray.triangle(triangle)
                    && distance.is_finite()
                    && distance <= ray.max_t
                    && hit.is_none_or(|(previous, last)| distance < last || triangle < previous)
                {
                    ray.max_t = distance;
                    *hit = Some((triangle, distance));
                }
            }
            return;
        }
        let entry = |child: u32| self.nodes[child as usize].bounds.ray_entry(ray.origin, ray.direction, ray.max_t);
        match (entry(node.first), entry(node.second)) {
            (Some(first), Some(second)) if second < first => {
                self.query_node(node.second, ray, hit);
                self.query_node(node.first, ray, hit);
            }
            (Some(_), Some(_)) => {
                self.query_node(node.first, ray, hit);
                self.query_node(node.second, ray, hit);
            }
            (Some(_), None) => self.query_node(node.first, ray, hit),
            (None, Some(_)) => self.query_node(node.second, ray, hit),
            (None, None) => {}
        }
    }
}

struct RayQuery<'a> {
    origin: DVec3,
    direction: DVec3,
    max_t: f64,
    triangles: &'a [[usize; 3]],
    points: &'a [DVec3],
}

impl RayQuery<'_> {
    fn triangle(&self, index: usize) -> Option<f64> {
        let [a, b, c] = self.triangles[index].map(|vertex| self.points[vertex]);
        let ab = b - a;
        let ac = c - a;
        let cross = self.direction.cross(ac);
        let determinant = ab.dot(cross);
        if determinant == 0.0 {
            return None;
        }
        let offset = self.origin - a;
        let u = offset.dot(cross) / determinant;
        let normal = offset.cross(ab);
        let v = self.direction.dot(normal) / determinant;
        let distance = ac.dot(normal) / determinant;
        (u >= -1e-12 && v >= -1e-12 && u + v <= 1.0 + 1e-12 && distance >= 0.0).then_some(distance)
    }
}
