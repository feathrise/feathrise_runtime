//! 区域边界作者数据、校验规则与静态碰撞线段派生。
mod footprint; // 游戏与编辑器共用地表辅助线，避免两种显示越改越不同。
pub use footprint::{boundary_display_point, boundary_footprint_lines}; // 保持跨crate唯一轮廓及贴地算法。

use std::collections::BTreeSet; // 稳定校验边界路径ID，错误顺序不会受哈希随机种子影响。

use bevy::prelude::{Asset, Rect, Vec2}; // 公共资产、世界矩形与二维X/Z坐标。
use bevy::reflect::TypePath; // 自定义JSON资产通过TypePath登记到Bevy资产系统。
use serde::{Deserialize, Serialize}; // F12保存与正式运行时读取共享同一数据格式。

use crate::{SCENE_SCHEMA_VERSION, SceneId}; // 边界文件沿用场景公共格式版本和稳定场景身份。

/// 跨帧捕获空气墙不可变输入；调用方负责场景/修订身份变化时重置。
#[derive(Clone, Debug, Default)]
pub struct SceneBoundaryCapture {
    document: Option<SceneBoundariesAsset>, // 尚未完成的结果不得提前发布。
    path_index: usize,                    // 下一条源路径。
    point_index: usize,                   // 当前路径已复制点数。
    complete: bool,                       // 完整输入才能被take采用。
}

impl SceneBoundaryCapture {
    /// 每次最多复制指定点数及路径数；空墙也消耗路径预算。
    pub fn step(
        &mut self,
        source: &SceneBoundariesAsset,
        point_budget: usize,
        path_budget: usize,
    ) -> bool {
        if self.complete {
            return true;
        } // 完成后不重复复制。
        let document = self.document.get_or_insert_with(|| SceneBoundariesAsset {
            schema_version: source.schema_version, // 冻结调用方已经核验的版本。
            scene_id: source.scene_id.clone(),         // 冻结场景身份。
            paths: Vec::new(),                     // 不在启动帧预分配全部墙对象。
        });
        let mut points_left = point_budget; // 点和路径两个预算独立收口。
        let mut paths_left = path_budget;
        while self.path_index < source.paths.len() && paths_left > 0 {
            let path = &source.paths[self.path_index]; // 同一源身份由调用方保证。
            if document.paths.len() == self.path_index {
                document.paths.push(SceneBoundaryPath {
                    id: path.id.clone(),
                    label: path.label.clone(), // 只复制本条元数据。
                    role: path.role,
                    enabled: path.enabled,
                    visible: path.visible, // 独立对象状态。
                    points: Vec::new(),
                    closed: path.closed, // 点数组分帧累积。
                    bottom_y: path.bottom_y,
                    top_y: path.top_y,
                    thickness: path.thickness, // 物理规格。
                });
            }
            let count = (path.points.len() - self.point_index).min(points_left); // 当前帧剩余切片。
            document.paths[self.path_index]
                .points
                .extend_from_slice(&path.points[self.point_index..self.point_index + count]);
            self.point_index += count;
            points_left -= count;
            if self.point_index < path.points.len() {
                break;
            } // 下一帧继续同一面墙。
            self.path_index += 1;
            self.point_index = 0;
            paths_left -= 1;
        }
        self.complete = self.path_index == source.paths.len(); // 零墙也能立即完整完成。
        self.complete
    }

    /// 只有整张输入捕获完成才交付所有权，调用方随后重置以开始下一代。
    pub fn take(&mut self) -> Option<SceneBoundariesAsset> {
        if !self.complete {
            return None;
        } // 拒绝半成品。
        self.document.take()
    }
}

/// 单个场景允许的边界路径上限，避免损坏作者文件制造无界实体数量。
pub const MAX_SCENE_BOUNDARY_PATHS: usize = 16_384; // 仅作损坏文件保护；运行时必须使用空间索引和后台派生，不能视为同步帧预算。

/// 单条边界允许的控制点上限。
pub const MAX_SCENE_BOUNDARY_POINTS_PER_PATH: usize = 65_536; // 单对象文件安全预算，不限制为少量控制点。

/// 单个场景允许的边界控制点总上限。
pub const MAX_SCENE_BOUNDARY_POINTS_TOTAL: usize = 1_048_576; // 文件级内存保护，碰撞生成不得在交互帧同步扫描整个预算。

/// 相邻控制点必须保持的最小世界距离。
pub const MIN_SCENE_BOUNDARY_SEGMENT_METERS: f32 = 0.25; // 退化薄片不进入Avian碰撞构造。

/// 新建场景的安全墙从地形几何边缘向内退让的距离。
pub const DEFAULT_SCENE_BOUNDARY_INSET_METERS: f32 = 2.0; // 保证墙体中心仍位于白模地形覆盖范围内。

/// 新建场景区域空气墙的默认厚度。
pub const DEFAULT_SCENE_BOUNDARY_THICKNESS_METERS: f32 = 2.0; // 足以阻止高速角色穿越，同时不会明显侵占可玩区域。

/// 单个场景全部可热加载区域边界。
#[derive(Asset, TypePath, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneBoundariesAsset {
    /// 数据格式版本。
    #[serde(deserialize_with = "crate::types::deserialize_scene_schema_version")]
    pub schema_version: u32,
    /// 文件所属场景。
    pub scene_id: SceneId,
    /// 独立空气墙对象；允许为空，也允许只有内部开放挡墙。
    pub paths: Vec<SceneBoundaryPath>,
}

/// 一条由世界X/Z控制点连接而成的空气墙路径。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneBoundaryPath {
    /// 场景内稳定路径ID。
    pub id: String,
    /// F12界面显示名称。
    pub label: String,
    /// 显式区分场景外框和内部挡墙，不依赖列表顺序。
    pub role: SceneBoundaryRole,
    /// 是否启用物理碰撞，与作者辅助线显隐独立。
    pub enabled: bool,
    /// 是否显示作者辅助线，不影响正式碰撞。
    pub visible: bool,
    /// 世界X/Z控制点；闭合路径会额外连接最后一点与第一点。
    pub points: Vec<[f32; 2]>,
    /// true表示闭合多边形，false表示仅连接相邻点的补充挡墙。
    pub closed: bool,
    /// 碰撞墙最低世界Y。
    pub bottom_y: f32,
    /// 碰撞墙最高世界Y。
    pub top_y: f32,
    /// 碰撞墙沿路径法线方向的完整厚度。
    pub thickness: f32,
}

/// 空气墙语义与是否闭合分离，内部挡墙也可以闭合成围挡。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneBoundaryRole {
    Outer, // 场景外框：正式保存必须闭合，并参与出口方向计算。
    #[default] // 未指定角色的新挡墙使用内部语义。
    Internal, // 内部挡墙：支持开放折线或闭合围挡，不影响出口方向。
}

impl SceneBoundaryPath {
    /// 插点只检查受影响的两条邻边；场景范围由编辑器落点约束及正式文档校验负责。
    pub fn insert_point(&mut self, index: usize, point: [f32; 2]) -> Result<(), String> {
        if index > self.points.len() || self.points.len() >= MAX_SCENE_BOUNDARY_POINTS_PER_PATH {
            return Err("插点位置无效或超过单墙文件安全预算".to_owned()); // 失败不修改作者数据。
        }
        let point_vec = Vec2::from_array(point); // 只计算本次插入点的局部约束。
        if !point_vec.is_finite() {
            return Err("空气墙控制点必须是有限坐标".to_owned()); // 不让NaN进入草稿。
        }
        let previous = index
            .checked_sub(1)
            .and_then(|i| self.points.get(i))
            .or_else(|| self.closed.then(|| self.points.last()).flatten()); // 闭合首点的前驱是末点。
        let next = self
            .points
            .get(index)
            .or_else(|| self.closed.then(|| self.points.first()).flatten()); // 闭合末尾的后继是首点。
        if [previous, next].into_iter().flatten().any(|neighbor| {
            point_vec.distance(Vec2::from_array(*neighbor)) < MIN_SCENE_BOUNDARY_SEGMENT_METERS
        }) {
            return Err("新控制点距离相邻点过近".to_owned()); // 拒绝退化碰撞段。
        }
        self.points.insert(index, point); // 仅在全部局部校验通过后提交插点。
        Ok(()) // 调用方可把成功修改纳入一次撤销事务。
    }

    /// 删除控制点时保持合法开放/闭合最小点数，并检查新连接的邻边。
    pub fn remove_point(&mut self, index: usize) -> Result<[f32; 2], String> {
        let minimum = if self.closed { 3 } else { 2 }; // 围挡至少三点，开放墙至少两点。
        if index >= self.points.len() || self.points.len() <= minimum {
            return Err(format!(
                "当前空气墙至少保留{minimum}个控制点；可直接删除整墙"
            )); // 不制造不可保存的半墙。
        }
        let previous = index
            .checked_sub(1)
            .and_then(|i| self.points.get(i))
            .or_else(|| self.closed.then(|| self.points.last()).flatten()); // 首点删除时连接末点。
        let next = self
            .points
            .get(index + 1)
            .or_else(|| self.closed.then(|| self.points.first()).flatten()); // 末点删除时连接首点。
        if let (Some(previous), Some(next)) = (previous, next) {
            if Vec2::from_array(*previous).distance(Vec2::from_array(*next))
                < MIN_SCENE_BOUNDARY_SEGMENT_METERS
            {
                return Err("删除后相邻点过近，请先移动控制点".to_owned()); // 保持原草稿可用。
            }
        }
        Ok(self.points.remove(index)) // 成功后返回被删除点，供调用方记录撤销。
    }
}

/// 运行时从边界路径派生的一段固定预算静态墙。
#[derive(Clone, Debug, PartialEq)]
pub struct SceneBoundarySegment {
    /// 所属稳定路径ID。
    pub path_id: String,
    /// 路径内从零开始的线段编号。
    pub segment_index: usize,
    /// 世界X/Z起点。
    pub start: Vec2,
    /// 世界X/Z终点。
    pub end: Vec2,
    /// 碰撞墙最低世界Y。
    pub bottom_y: f32,
    /// 碰撞墙最高世界Y。
    pub top_y: f32,
    /// 碰撞墙完整厚度。
    pub thickness: f32,
}

impl SceneBoundariesAsset {
    /// 为新建或旧版动态场景构造覆盖完整世界范围的默认闭合安全边界。
    pub fn rectangular_default(
        scene_id: &SceneId,      // 默认文件必须绑定当前稳定场景身份。
        world_min: [f32; 2], // 地形世界X/Z最小角。
        world_max: [f32; 2], // 地形世界X/Z最大角。
        base_height: f32,    // 高度编码的最低可表示世界Y。
        height_scale: f32,   // 高度编码可表示的完整垂直跨度。
    ) -> Result<Self, String> {
        let min = Vec2::from_array(world_min); // 数组清单统一转换为Bevy二维坐标。
        let max = Vec2::from_array(world_max); // 最大角与最小角共同校验有效范围。
        if !min.is_finite() || !max.is_finite() || !min.cmplt(max).all() {
            return Err("场景世界范围无效，无法生成默认区域边界".to_owned()); // 损坏清单不得生成NaN碰撞。
        }
        if !base_height.is_finite() || !height_scale.is_finite() || height_scale <= 0.0 {
            return Err("场景高度量程无效，无法生成默认区域边界".to_owned()); // 垂直墙必须覆盖有限正高度区间。
        }
        let half_small_axis = (max - min).min_element() * 0.5; // 取较短边的一半，避免极小场景内缩后翻转。
        let inset = DEFAULT_SCENE_BOUNDARY_INSET_METERS.min((half_small_axis - 0.5).max(0.0)); // 至少保留一米可用直径。
        let path = SceneBoundaryPath {
            id: "playable_boundary".to_owned(), // 主可玩区使用稳定英文身份。
            label: "主区域边界".to_owned(),     // F12向作者显示直白中文名称。
            role: SceneBoundaryRole::Outer,       // 新默认外框显式登记角色。
            enabled: true,                      // 默认安全保护有效。
            visible: true,                      // 作者可以直接检查默认外框。
            points: vec![
                [min.x + inset, min.y + inset], // 西北角向内退让默认安全距离。
                [max.x - inset, min.y + inset], // 东北角保持世界X/Z方向一致。
                [max.x - inset, max.y - inset], // 东南角位于最大X/Z附近。
                [min.x + inset, max.y - inset], // 西南角完成矩形顺序。
            ],
            closed: true,                // 新场景从第一次进入玩法起就拥有完整闭合保护。
            bottom_y: base_height - 8.0, // 低于最低可雕刻地形八米，防止从河床边缘钻出。
            top_y: base_height + height_scale + 32.0, // 高于最高可雕刻地形三十二米，覆盖跳跃与坡顶。
            thickness: DEFAULT_SCENE_BOUNDARY_THICKNESS_METERS, // 统一使用受检默认厚度。
        };
        let asset = Self {
            schema_version: SCENE_SCHEMA_VERSION, // 新文件始终使用当前公共格式版本。
            scene_id: scene_id.clone(),             // 复制轻量稳定身份，不引用编辑器会话。
            paths: vec![path],                  // 默认只生成一条主闭合边界。
        };
        asset.validate(scene_id, Rect::from_corners(min, max))?; // 构造器自身也必须通过正式文件校验。
        Ok(asset) // 返回可直接保存和运行时采用的完整边界文档。
    }

    /// 校验正式空气墙文件；外框必须闭合，内部开放墙和零墙都是合法作者选择。
    pub fn validate(&self, expected_scene_id: &SceneId, _world_bounds: Rect) -> Result<(), String> {
        if self.schema_version != SCENE_SCHEMA_VERSION {
            return Err(format!(
                "区域边界版本{}与运行时版本{}不一致",
                self.schema_version, SCENE_SCHEMA_VERSION
            )); // 不兼容格式不得静默猜测字段含义。
        }
        if &self.scene_id != expected_scene_id {
            return Err(format!(
                "区域边界所属场景{}与当前场景{}不一致",
                self.scene_id, expected_scene_id
            )); // 跨场景文件误配会直接阻止采用。
        }
        // 空气墙是显式玩法边界，不依赖旧主地形矩形；只校验墙自身的有限几何和预算。
        if self.paths.len() > MAX_SCENE_BOUNDARY_PATHS {
            return Err(format!(
                "空气墙数量超过文件安全预算{MAX_SCENE_BOUNDARY_PATHS}"
            )); // 零墙可用于无需人工碰撞约束的场景。
        }
        let mut ids = BTreeSet::new(); // 确保稳定路径身份不会在热更时产生歧义。
        let mut total_points = 0usize; // 汇总所有路径点数以执行场景级硬预算。
        for path in &self.paths {
            if path.role == SceneBoundaryRole::Outer && !path.closed {
                return Err(format!(
                    "场景外框{}必须闭合；开放挡墙请使用内部类型",
                    path.id
                )); // 防止把未完成外框误当完整保护。
            }
            if path.id.is_empty()
                || path.id.starts_with('/')
                || path.id.contains("..")
                || path.id.contains('\\')
                || !ids.insert(path.id.clone())
            {
                return Err(format!("区域边界路径ID无效或重复：{}", path.id)); // 路径ID同时服务诊断和实体稳定命名。
            }
            let minimum_points = if path.closed { 3 } else { 2 }; // 闭合多边形比开放挡墙多需要一个非共线控制点。
            if path.points.len() < minimum_points
                || path.points.len() > MAX_SCENE_BOUNDARY_POINTS_PER_PATH
            {
                return Err(format!(
                    "区域边界{}控制点数量必须为{}..={}",
                    path.id, minimum_points, MAX_SCENE_BOUNDARY_POINTS_PER_PATH
                )); // 不完整作者笔划不能保存或解除F12暂停。
            }
            if !path.bottom_y.is_finite()
                || !path.top_y.is_finite()
                || path.top_y <= path.bottom_y
                || !(path.top_y - path.bottom_y).is_finite()
            {
                return Err(format!("区域边界{}垂直范围无效", path.id)); // 负高或非有限墙体不能进入物理世界。
            }
            if !path.thickness.is_finite() || !(0.25..=32.0).contains(&path.thickness) {
                return Err(format!("区域边界{}厚度必须为0.25..=32米", path.id)); // 保持足够阻挡能力并限制异常宽墙。
            }
            total_points = total_points.saturating_add(path.points.len()); // 饱和累加避免损坏大输入溢出。
            if total_points > MAX_SCENE_BOUNDARY_POINTS_TOTAL {
                return Err(format!(
                    "空气墙控制点总数超过文件安全预算{MAX_SCENE_BOUNDARY_POINTS_TOTAL}"
                )); // 超预算立即失败，不继续遍历损坏文件剩余点集。
            }
            for (index, point) in path.points.iter().enumerate() {
                let point = Vec2::from_array(*point); // 每个控制点都转换到统一世界X/Z表示。
                if !point.is_finite() {
                    return Err(format!("区域边界{}第{}点不是有限坐标", path.id, index));
                }
            }
            for segment in path.points.windows(2) {
                let start = Vec2::from_array(segment[0]); // 相邻点构成一段开放或闭合路径主体。
                let end = Vec2::from_array(segment[1]); // 终点与起点距离必须足以生成稳定盒体。
                if start.distance(end) < MIN_SCENE_BOUNDARY_SEGMENT_METERS {
                    return Err(format!("区域边界{}包含过短或重复线段", path.id)); // 零长度旋转会产生NaN。
                }
            }
            if path.closed {
                let start = Vec2::from_array(*path.points.last().expect("闭合路径至少三个点")); // 正式校验已保证非空。
                let end = Vec2::from_array(path.points[0]); // 最后一段回到首点形成完整安全圈。
                if start.distance(end) < MIN_SCENE_BOUNDARY_SEGMENT_METERS {
                    return Err(format!("区域边界{}闭合线段过短", path.id)); // 首尾重复点不应再生成额外薄片。
                }
            }
        }
        if total_points > MAX_SCENE_BOUNDARY_POINTS_TOTAL {
            return Err(format!(
                "区域边界控制点总数超过{MAX_SCENE_BOUNDARY_POINTS_TOTAL}"
            )); // 多路径总预算不能通过拆分绕过。
        }
        Ok(()) // 完整文件可以原子进入正式运行时。
    }

    /// 派生全部非退化碰撞线段；编辑器未闭合草稿也可用于即时开放路径预览。
    pub fn collision_segments(&self) -> Vec<SceneBoundarySegment> {
        let mut segments = Vec::new(); // 容量随受检控制点上限固定，不进入稳定帧重复分配。
        for path in &self.paths {
            if !path.enabled {
                continue; // 仅停用碰撞会移除线段，隐藏辅助线不改变碰撞。
            }
            for (segment_index, pair) in path.points.windows(2).enumerate() {
                push_segment(&mut segments, path, segment_index, pair[0], pair[1]); // 相邻点始终形成主体线段。
            }
            if path.closed && path.points.len() >= 3 {
                let segment_index = path.points.len() - 1; // 闭合段使用最后控制点编号，诊断稳定。
                push_segment(
                    &mut segments, // 把最后一点到第一点加入同一完整结果。
                    path,          // 沿用当前路径厚度和垂直范围。
                    segment_index, // 保存闭合段稳定编号。
                    *path.points.last().expect("闭合草稿至少三个点"), // 经过长度门后安全读取最后一点。
                    path.points[0],                                   // 首点完成闭环。
                );
            }
        }
        segments // 调用方只在作者变更或资产事件时消费完整列表。
    }

    /// 返回从给定世界X/Z位置指向主边界最近线段点的顺时针角度。
    ///
    /// 角度沿用传送点视觉约定：`0°`指向北方`-Z`、`90°`指向东方`+X`。
    /// 这里只读取启用的外框；内部挡墙不得改变场景出口箭头。调用方应只在
    /// 传送点或边界修订变化时计算，并在没有可用主边界时保留旧作者角度作为兼容回退。
    pub fn nearest_main_boundary_direction_degrees(&self, world_xz: Vec2) -> Option<f32> {
        let path = self.main_outer_path()?; // 显式角色优先，不让列表首个内部挡墙改变出口方向。
        if !world_xz.is_finite() || path.points.len() < 2 {
            return None; // 不完整作者笔划或损坏坐标不能生成NaN旋转。
        }

        let mut nearest = None; // 固定上限内单次扫描，不分配碰撞线段临时Vec。
        for pair in path.points.windows(2) {
            consider_nearest_boundary_point(
                &mut nearest,
                world_xz,
                Vec2::from_array(pair[0]),
                Vec2::from_array(pair[1]),
            ); // 相邻控制点贡献一段真实可达边界。
        }
        if path.closed && path.points.len() >= 3 {
            consider_nearest_boundary_point(
                &mut nearest,
                world_xz,
                Vec2::from_array(*path.points.last().expect("闭合主边界至少三个点")),
                Vec2::from_array(path.points[0]),
            ); // 闭合段必须参与最近点判断，不能只比较控制点。
        }

        let nearest = nearest?;
        let toward_boundary = nearest.point - world_xz; // 普通点直接朝向几何上最近的边界位置。
        if toward_boundary.length_squared() > f32::EPSILON {
            return portal_direction_degrees(toward_boundary);
        }

        // 点恰好落在线上时“最近点方向”为零；闭合主边界改用该段外法线，仍给出稳定出口方向。
        let signed_area_twice = closed_path_signed_area_twice(path)?;
        let outward = if signed_area_twice > 0.0 {
            Vec2::new(nearest.segment_direction.y, -nearest.segment_direction.x) // 逆时针路径的外侧位于线段右方。
        } else {
            Vec2::new(-nearest.segment_direction.y, nearest.segment_direction.x) // 顺时针路径的外侧位于线段左方。
        };
        portal_direction_degrees(outward)
    }

    /// 按作者顺序返回首个启用外框，列表重排后仍遵循显式角色。
    pub fn main_outer_path(&self) -> Option<&SceneBoundaryPath> {
        self.paths.iter().find_map(|path| {
            (path.enabled && path.role == SceneBoundaryRole::Outer).then_some(path) // 没有外框时调用方继续使用已有出口角度。
        })
    }
}

/// 主边界最近点计算保留命中的线段方向，供“点恰好在线上”时派生稳定外法线。
#[derive(Clone, Copy, Debug)]
struct NearestBoundaryPoint {
    point: Vec2,             // 线段上距离传送点最近的世界X/Z位置。
    distance_squared: f32,   // 比较平方距离避免每段开方。
    segment_direction: Vec2, // 从作者起点指向终点的非退化方向。
}

/// 把一段有限非退化线段纳入当前最近点；相同距离保留作者顺序中更早的一段以稳定角点结果。
fn consider_nearest_boundary_point(
    nearest: &mut Option<NearestBoundaryPoint>,
    world_xz: Vec2,
    start: Vec2,
    end: Vec2,
) {
    let segment_direction = end - start;
    let segment_length_squared = segment_direction.length_squared();
    if !start.is_finite()
        || !end.is_finite()
        || !segment_length_squared.is_finite()
        || segment_length_squared < MIN_SCENE_BOUNDARY_SEGMENT_METERS.powi(2)
    {
        return; // 绘制中的重复点和损坏正式数据都不参与方向推导。
    }
    let progress =
        ((world_xz - start).dot(segment_direction) / segment_length_squared).clamp(0.0, 1.0); // 最近点可以落在线段内部，也可以落在真实端点。
    let point = start + segment_direction * progress;
    let distance_squared = world_xz.distance_squared(point);
    if !distance_squared.is_finite()
        || nearest
            .as_ref()
            .is_some_and(|current| distance_squared >= current.distance_squared)
    {
        return; // 非有限结果或不更近的候选不改写稳定完成值。
    }
    *nearest = Some(NearestBoundaryPoint {
        point,
        distance_squared,
        segment_direction,
    });
}

/// 计算闭合主路径的两倍有符号面积；零面积或非有限路径没有可判定的外侧。
fn closed_path_signed_area_twice(path: &SceneBoundaryPath) -> Option<f32> {
    if !path.closed || path.points.len() < 3 {
        return None; // 开放补墙没有统一内外语义。
    }
    let mut area_twice = 0.0;
    for pair in path.points.windows(2) {
        let start = Vec2::from_array(pair[0]);
        let end = Vec2::from_array(pair[1]);
        if !start.is_finite() || !end.is_finite() {
            return None; // 损坏点不得扩散为NaN法线。
        }
        area_twice += start.perp_dot(end);
    }
    let last = Vec2::from_array(*path.points.last()?);
    let first = Vec2::from_array(path.points[0]);
    area_twice += last.perp_dot(first); // 补上末点到首点的闭合面积项。
    (area_twice.is_finite() && area_twice.abs() > f32::EPSILON).then_some(area_twice)
}

/// 把世界X/Z方向转换为传送箭头使用的“北零度、顺时针增加”角度。
fn portal_direction_degrees(direction: Vec2) -> Option<f32> {
    if !direction.is_finite() || direction.length_squared() <= f32::EPSILON {
        return None; // 零向量没有方向，调用方继续使用兼容回退。
    }
    Some(
        direction
            .x
            .atan2(-direction.y) // Vec2.y对应世界Z，负Z是北方零度。
            .to_degrees()
            .rem_euclid(360.0), // 统一输出0..360，便于序列化和测试。
    )
}

/// 把一段有限且非退化的作者线加入碰撞结果；不完整草稿中的坏点安全跳过。
fn push_segment(
    segments: &mut Vec<SceneBoundarySegment>, // 当前场景完整碰撞段输出。
    path: &SceneBoundaryPath,                 // 线段继承的作者路径参数。
    segment_index: usize,                   // 路径内稳定线段编号。
    start: [f32; 2],                        // 世界X/Z起点数组。
    end: [f32; 2],                          // 世界X/Z终点数组。
) {
    let start = Vec2::from_array(start); // 转换为便于距离与旋转计算的二维向量。
    let end = Vec2::from_array(end); // 终点使用同一世界X/Z约定。
    if !start.is_finite()
        || !end.is_finite()
        || start.distance(end) < MIN_SCENE_BOUNDARY_SEGMENT_METERS
        || !path.bottom_y.is_finite()
        || !path.top_y.is_finite()
        || path.top_y <= path.bottom_y
        || !(path.top_y - path.bottom_y).is_finite()
        || !path.thickness.is_finite()
        || path.thickness <= 0.0
    {
        return; // F12正在绘制的中间状态不得向物理世界注入无效盒体。
    }
    segments.push(SceneBoundarySegment {
        path_id: path.id.clone(),  // 诊断实体保留稳定路径身份。
        segment_index,             // 保存调用方给出的稳定顺序。
        start,                     // 复制二维起点。
        end,                       // 复制二维终点。
        bottom_y: path.bottom_y,   // 每条路径可以拥有独立最低高度。
        top_y: path.top_y,         // 每条路径可以拥有独立最高高度。
        thickness: path.thickness, // 使用作者指定完整厚度。
    });
}

#[cfg(test)]
mod tests {
    use super::*; // 回归只验证纯数据构造、校验和线段数量。

    #[test]
    fn independent_internal_walls_and_empty_documents_are_valid() {
        let map = SceneId::parse("MAP-wall-contract").unwrap(); // 真实稳定场景身份。
        let bounds = Rect::from_corners(Vec2::splat(-50.0), Vec2::splat(50.0)); // 合法场景范围。
        let mut document =
            SceneBoundariesAsset::rectangular_default(&map, [-50.0; 2], [50.0; 2], 0.0, 20.0)
                .unwrap();
        document.paths[0].role = SceneBoundaryRole::Internal; // 首条内部墙不能隐式成为外框。
        document.paths[0].closed = false; // 开放挡墙独立有效。
        document.validate(&map, bounds).unwrap(); // 不要求补画外框。
        assert_eq!(
            document.nearest_main_boundary_direction_degrees(Vec2::ZERO),
            None
        ); // 内部墙不改变出口。
        let expected = document.collision_segments(); // 记录有效碰撞。
        document.paths[0].visible = false; // 隐藏辅助线不能停用碰撞。
        assert_eq!(document.collision_segments(), expected);
        document.paths[0].enabled = false; // 显式停用才撤销碰撞。
        assert!(document.collision_segments().is_empty());
        let json = serde_json::to_string(&document).unwrap(); // 保存角色和两个独立开关。
        assert_eq!(
            serde_json::from_str::<SceneBoundariesAsset>(&json).unwrap(),
            document
        );
        for required in ["role", "enabled", "visible"] {
            let mut value = serde_json::to_value(&document).unwrap(); // 新契约不猜测缺失对象语义。
            value["paths"][0].as_object_mut().unwrap().remove(required);
            assert!(serde_json::from_value::<SceneBoundariesAsset>(value).is_err());
        }
        document.paths.clear(); // 作者可以删除全部空气墙。
        document.validate(&map, bounds).unwrap();
    }

    #[test]
    fn capture_has_bounded_progress_and_never_exposes_partial_documents() {
        let map = SceneId::parse("MAP-wall-capture").unwrap(); // 有限点集验证多帧交付。
        let source =
            SceneBoundariesAsset::rectangular_default(&map, [-50.0; 2], [50.0; 2], 0.0, 20.0)
                .unwrap();
        let mut capture = SceneBoundaryCapture::default();
        assert!(!capture.step(&source, 1, 1)); // 每次只复制一个点。
        assert_eq!(capture.document.as_ref().unwrap().paths[0].points.len(), 1);
        assert!(capture.take().is_none()); // 半成品不允许发布。
        assert!(!capture.step(&source, 1, 1));
        assert!(!capture.step(&source, 1, 1));
        assert!(capture.step(&source, 1, 1));
        assert_eq!(capture.take().unwrap(), source); // 元数据、角色和点全部一致。
    }

    #[test]
    fn point_insert_and_delete_reject_degenerate_edges_atomically() {
        let map = SceneId::parse("MAP-wall-points").unwrap();
        let mut document =
            SceneBoundariesAsset::rectangular_default(&map, [-50.0; 2], [50.0; 2], 0.0, 20.0)
                .unwrap();
        let path = &mut document.paths[0]; // 闭合四边形回归插入和删除。
        let before = path.clone(); // 失败必须保持原点集。
        assert!(path.insert_point(1, path.points[0]).is_err());
        assert_eq!(*path, before);
        path.insert_point(1, [0.0, -48.0]).unwrap(); // 合法边中点。
        assert_eq!(path.remove_point(1).unwrap(), [0.0, -48.0]);
        assert_eq!(*path, before); // 成对操作恢复原始几何。
        path.remove_point(1).unwrap(); // 四点可以变成三点。
        assert!(path.remove_point(1).is_err()); // 闭合围挡不能删成两点。
    }

    #[test] // 新场景默认边界必须闭合并在地形范围内生成四段墙。
    fn rectangular_default_is_a_valid_closed_safety_boundary() {
        let scene_id = SceneId::parse("MAP-BOUNDARY-TEST").expect("测试场景ID应有效"); // 使用正式身份校验。
        let boundaries = SceneBoundariesAsset::rectangular_default(
            &scene_id,        // 文件所属测试场景。
            [-50.0, -30.0], // 一百米东西宽度。
            [50.0, 30.0],   // 六十米南北长度。
            -80.0,          // 默认最低高度。
            160.0,          // 默认完整高度跨度。
        )
        .expect("合法场景范围应生成默认边界");
        boundaries
            .validate(
                &scene_id,
                Rect::from_corners(Vec2::new(-50.0, -30.0), Vec2::new(50.0, 30.0)),
            )
            .expect("默认边界必须通过正式文件校验");
        assert_eq!(boundaries.collision_segments().len(), 4); // 四控制点闭环必须产生四段碰撞墙。
        assert!(boundaries.paths[0].closed); // 默认主边界不能处于未闭合作者状态。
    }

    #[test] // 只有开放折线不能冒充完整场景安全圈。
    fn formal_boundary_requires_at_least_one_closed_path() {
        let scene_id = SceneId::parse("MAP-BOUNDARY-OPEN").expect("测试场景ID应有效"); // 创建稳定测试身份。
        let mut boundaries = SceneBoundariesAsset::rectangular_default(
            &scene_id,        // 使用同一场景身份。
            [-32.0, -32.0], // 合法最小角。
            [32.0, 32.0],   // 合法最大角。
            -80.0,          // 合法最低高度。
            160.0,          // 合法高度跨度。
        )
        .expect("默认边界应构造成功");
        boundaries.paths[0].closed = false; // 模拟作者尚未点击“闭合边界”。
        assert!(
            boundaries
                .validate(
                    &scene_id,
                    Rect::from_corners(Vec2::splat(-32.0), Vec2::splat(32.0))
                )
                .is_err()
        ); // 保存与退出门必须拒绝开放主路径。
        assert_eq!(boundaries.collision_segments().len(), 3); // 未闭合草稿仍即时预览已有三段墙。
    }

    #[test] // 矩形主边界应按传送点所在位置稳定得到四个基本方向，而不是固定零度。
    fn portal_direction_points_to_nearest_main_boundary_segment() {
        let scene_id = SceneId::parse("MAP-BOUNDARY-DIRECTION").expect("测试场景ID应有效");
        let boundaries = SceneBoundariesAsset::rectangular_default(
            &scene_id,
            [-50.0, -30.0],
            [50.0, 30.0],
            -80.0,
            160.0,
        )
        .expect("矩形测试边界应构造成功");
        assert_eq!(
            boundaries.nearest_main_boundary_direction_degrees(Vec2::new(0.0, -20.0)),
            Some(0.0)
        ); // 靠近北边时指向-Z。
        assert_eq!(
            boundaries.nearest_main_boundary_direction_degrees(Vec2::new(40.0, 0.0)),
            Some(90.0)
        ); // 靠近东边时指向+X。
        assert_eq!(
            boundaries.nearest_main_boundary_direction_degrees(Vec2::new(0.0, 20.0)),
            Some(180.0)
        ); // 靠近南边时指向+Z。
        assert_eq!(
            boundaries.nearest_main_boundary_direction_degrees(Vec2::new(-40.0, 0.0)),
            Some(270.0)
        ); // 靠近西边时指向-X。
    }

    #[test] // 不规则边界使用线段最近点，因此箭头可以连续指向斜向出口而不是吸附控制点。
    fn portal_direction_uses_closest_point_on_diagonal_segment() {
        let scene_id = SceneId::parse("MAP-DIAGONAL-DIRECTION").expect("测试场景ID应有效");
        let boundaries = SceneBoundariesAsset {
            schema_version: SCENE_SCHEMA_VERSION,
            scene_id,
            paths: vec![SceneBoundaryPath {
                id: "playable_boundary".to_owned(),
                label: "斜向主边界".to_owned(),
                role: SceneBoundaryRole::Outer, // 明确场景外框。
                enabled: true,                // 默认参与碰撞。
                visible: true,                // 默认显示辅助线。
                points: vec![[0.0, -10.0], [10.0, 0.0], [-10.0, 0.0]],
                closed: true,
                bottom_y: -10.0,
                top_y: 20.0,
                thickness: 2.0,
            }],
        };
        let angle = boundaries
            .nearest_main_boundary_direction_degrees(Vec2::new(5.0, -3.0))
            .expect("三角形内点应找到最近斜边");
        assert!((angle - 45.0).abs() < 0.001); // 最近点方向是东北，连续角度应为45°。
    }

    #[test] // 传送点恰好在线上时使用闭合路径外法线，不能退化成零向量或NaN。
    fn portal_on_boundary_uses_stable_outward_normal() {
        let scene_id = SceneId::parse("MAP-BOUNDARY-NORMAL").expect("测试场景ID应有效");
        let boundaries = SceneBoundariesAsset::rectangular_default(
            &scene_id,
            [-20.0, -20.0],
            [20.0, 20.0],
            -20.0,
            40.0,
        )
        .expect("矩形测试边界应构造成功");
        assert_eq!(
            boundaries.nearest_main_boundary_direction_degrees(Vec2::new(0.0, -18.0)),
            Some(0.0)
        ); // 默认内缩后的北边界位于Z=-18，外法线仍指向北。
    }

    #[test] // 不完整主路径没有方向信用，调用方必须保留旧作者角度。
    fn incomplete_main_boundary_has_no_portal_direction() {
        let boundaries = SceneBoundariesAsset {
            schema_version: SCENE_SCHEMA_VERSION,
            scene_id: SceneId::parse("MAP-NO-DIRECTION").expect("测试场景ID应有效"),
            paths: vec![SceneBoundaryPath {
                id: "playable_boundary".to_owned(),
                label: "未完成主边界".to_owned(),
                role: SceneBoundaryRole::Outer, // 明确场景外框。
                enabled: true,                // 默认参与碰撞。
                visible: true,                // 默认显示辅助线。
                points: vec![[0.0, 0.0]],
                closed: false,
                bottom_y: -10.0,
                top_y: 20.0,
                thickness: 2.0,
            }],
        };
        assert_eq!(
            boundaries.nearest_main_boundary_direction_degrees(Vec2::ZERO),
            None
        ); // 无线段时明确交给兼容回退，不伪造北向。
    }
}
