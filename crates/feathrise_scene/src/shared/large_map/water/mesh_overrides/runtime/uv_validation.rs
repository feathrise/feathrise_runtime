//! POSITION/UV0有限值和米制三角证据的逐primitive固定预算增量校验。

use super::super::*; // 复用UV语义常量、索引读取helper与Bevy Mesh属性类型。

/// 单primitive UV校验一次推进后的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UvValidationProgress {
    Pending,                     // 仍有顶点或抽样三角留到下一帧。
    Complete { samples: usize }, // 全部语义证据通过并返回实际根预算消费量。
} // 结束UV校验进度。

/// 不借用Mesh资产的跨帧数值游标；每次推进重新短借用当前只读属性切片。
pub(super) struct IncrementalUvValidation {
    expected_vertices: usize,     // Mesh内容换代或属性长度变化时失败关闭。
    requires_normals: bool,       // 斜坡/瀑布必须逐顶点验证Blender法线，水平面允许旧资产缺省。
    world_from_mesh: Mat3A,       // 启动时已由增量父链完整验证。
    vertex_cursor: usize,         // 下一待扫描POSITION/UV0顶点。
    min_u: f32,                   // 已扫描前缀U下界。
    max_u: f32,                   // 已扫描前缀U上界。
    min_v: f32,                   // 已扫描前缀V下界。
    max_v: f32,                   // 已扫描前缀V上界。
    triangle_count: usize,        // 当前TriangleList完整逻辑三角数。
    sampled_triangles: usize,     // 均匀抽样目标，已受单primitive和全根剩余额度限制。
    triangle_cursor: usize,       // 下一待分析抽样三角。
    along_edges: usize,           // 已观察沿程候选边数。
    meter_scale_edges: usize,     // 其中U/世界米比例有效的边数。
    vertex_range_validated: bool, // 顶点前缀完成后只计算一次轴跨度。
} // 结束增量UV校验游标。

impl IncrementalUvValidation {
    pub(super) fn new(
        positions_len: usize,
        uv0_len: usize,
        normals_len: Option<usize>,
        logical_index_count: usize,
        world_from_mesh: Mat3A,
        triangle_sample_budget: usize,
    ) -> Result<Self, String> {
        if !world_from_mesh.is_finite() {
            return Err("Mesh局部到世界的线性变换包含非有限值".to_owned());
        }
        if positions_len == 0 || uv0_len == 0 {
            return Err("POSITION/UV0不能为空".to_owned());
        }
        if positions_len != uv0_len {
            return Err("POSITION与UV0顶点数不一致".to_owned());
        }
        if normals_len.is_some_and(|normal_len| normal_len != positions_len) {
            return Err("NORMAL与POSITION顶点数不一致".to_owned());
        }
        if logical_index_count < 3 || logical_index_count % 3 != 0 {
            return Err("TriangleList索引/顶点数必须是非零的3的倍数".to_owned());
        }
        if triangle_sample_budget == 0 {
            return Err("当前placement的UV几何抽样超出单根固定预算".to_owned());
        }
        let triangle_count = logical_index_count / 3; // TriangleList每三个逻辑索引是一个面。
        Ok(Self {
            expected_vertices: positions_len,
            requires_normals: normals_len.is_some(),
            world_from_mesh,
            vertex_cursor: 0,
            min_u: f32::INFINITY,
            max_u: f32::NEG_INFINITY,
            min_v: f32::INFINITY,
            max_v: f32::NEG_INFINITY,
            triangle_count,
            sampled_triangles: triangle_count.min(triangle_sample_budget),
            triangle_cursor: 0,
            along_edges: 0,
            meter_scale_edges: 0,
            vertex_range_validated: false,
        })
    }

    /// 分别按顶点数和抽样三角数预算推进，两个预算互不借用。
    pub(super) fn advance(
        &mut self,
        positions: &[[f32; 3]],
        uv0: &[[f32; 2]],
        normals: Option<&[[f32; 3]]>,
        indices: Option<&Indices>,
        vertex_budget: usize,
        triangle_budget: usize,
    ) -> Result<UvValidationProgress, String> {
        if positions.len() != self.expected_vertices || uv0.len() != self.expected_vertices {
            return Err("POSITION/UV0在增量校验期间发生长度变化".to_owned());
        }
        if self.requires_normals
            && normals.is_none_or(|values| values.len() != self.expected_vertices)
        {
            return Err("NORMAL在增量校验期间缺失或长度变化".to_owned());
        }
        let vertex_end = self
            .vertex_cursor
            .saturating_add(vertex_budget)
            .min(self.expected_vertices); // 每帧只读取明确数量顶点。
        for vertex_index in self.vertex_cursor..vertex_end {
            let position = positions[vertex_index];
            let uv = uv0[vertex_index];
            if !Vec3::from_array(position).is_finite() {
                return Err("POSITION包含NaN或无穷值，无法建立UV米制证据".to_owned());
            }
            if let Some(normals) = normals {
                let normal = Vec3::from_array(normals[vertex_index]);
                if !normal.is_finite() || normal.length_squared() <= 1.0e-12 {
                    return Err("NORMAL包含NaN、无穷值或零向量".to_owned());
                }
            }
            let [u, v] = uv;
            if !u.is_finite() || !v.is_finite() {
                return Err("UV0包含NaN或无穷值".to_owned());
            }
            if !(-MESH_WATER_UV_V_RANGE_TOLERANCE..=1.0 + MESH_WATER_UV_V_RANGE_TOLERANCE)
                .contains(&v)
            {
                return Err(format!(
                    "UV0.V={v}明显超出横向0..1约定（容差±{MESH_WATER_UV_V_RANGE_TOLERANCE}）"
                ));
            }
            self.min_u = self.min_u.min(u);
            self.max_u = self.max_u.max(u);
            self.min_v = self.min_v.min(v);
            self.max_v = self.max_v.max(v);
        }
        self.vertex_cursor = vertex_end; // 精确保存已扫描前缀。
        if self.vertex_cursor < self.expected_vertices {
            return Ok(UvValidationProgress::Pending); // 顶点未完成前绝不提前读取三角证据。
        }
        if !self.vertex_range_validated {
            self.validate_axis_spans()?; // 全顶点有限后只验证一次全局U/V跨度。
            self.vertex_range_validated = true;
        }

        let triangle_end = self
            .triangle_cursor
            .saturating_add(triangle_budget)
            .min(self.sampled_triangles); // 每帧均匀样本数独立受限。
        for sample_index in self.triangle_cursor..triangle_end {
            let triangle_index = sample_index * self.triangle_count / self.sampled_triangles;
            let logical_base = triangle_index * 3;
            let triangle = [
                mesh_water_vertex_index(indices, logical_base, positions.len())?,
                mesh_water_vertex_index(indices, logical_base + 1, positions.len())?,
                mesh_water_vertex_index(indices, logical_base + 2, positions.len())?,
            ];
            for [from, to] in [
                [triangle[0], triangle[1]],
                [triangle[1], triangle[2]],
                [triangle[2], triangle[0]],
            ] {
                let du = (uv0[to][0] - uv0[from][0]).abs();
                let dv = (uv0[to][1] - uv0[from][1]).abs();
                let local_edge =
                    Vec3::from_array(positions[to]) - Vec3::from_array(positions[from]);
                let edge_length = (self.world_from_mesh * local_edge).length();
                if edge_length < MIN_MESH_WATER_GEOMETRY_EDGE_LENGTH
                    || du < MIN_MESH_WATER_ALONG_EDGE_U_DELTA
                    || du < dv * 2.0
                {
                    continue;
                }
                self.along_edges = self.along_edges.saturating_add(1);
                let u_per_meter = du / edge_length;
                if (MIN_MESH_WATER_U_PER_METER_RATIO..=MAX_MESH_WATER_U_PER_METER_RATIO)
                    .contains(&u_per_meter)
                {
                    self.meter_scale_edges = self.meter_scale_edges.saturating_add(1);
                }
            }
        }
        self.triangle_cursor = triangle_end; // 精确保存均匀样本前缀。
        if self.triangle_cursor < self.sampled_triangles {
            return Ok(UvValidationProgress::Pending);
        }
        let minimum_evidence_edges = if self.triangle_count == 1 { 1 } else { 2 };
        if self.along_edges < minimum_evidence_edges
            || self.meter_scale_edges < minimum_evidence_edges
            || self.meter_scale_edges.saturating_mul(2) < self.along_edges
        {
            return Err(format!(
                "UV0.U缺少局部米制沿程证据：沿程候选边{}条，米制比例边{}条；长河不能使用归一化U=0..1",
                self.along_edges, self.meter_scale_edges
            ));
        }
        Ok(UvValidationProgress::Complete {
            samples: self.sampled_triangles,
        })
    }

    fn validate_axis_spans(&self) -> Result<(), String> {
        let u_span = self.max_u - self.min_u;
        if !u_span.is_finite() {
            return Err("UV0.U的有限端点差值溢出".to_owned());
        }
        if u_span < MIN_MESH_WATER_UV_AXIS_SPAN {
            return Err(format!(
                "UV0.U没有有效沿程跨度（至少{MIN_MESH_WATER_UV_AXIS_SPAN}）"
            ));
        }
        let v_span = self.max_v - self.min_v;
        if !v_span.is_finite() {
            return Err("UV0.V的有限端点差值溢出".to_owned());
        }
        if v_span < MIN_MESH_WATER_UV_AXIS_SPAN {
            return Err(format!(
                "UV0.V没有有效横向跨度（至少{MIN_MESH_WATER_UV_AXIS_SPAN}）"
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn vertex_cursor(&self) -> usize {
        self.vertex_cursor // 极值回归锁定一次推进只消费规定顶点数。
    }
} // 结束增量UV校验实现。
