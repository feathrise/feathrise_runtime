//! Blender导入复杂网格的placement级动态水体接管。
//!
//! 【功能设计】本模块只给显式携带`DynamicWaterMeshOverrides`的单个
//! `WorldAssetRoot`工作。有覆盖的根只进入固定容量去重队列，每帧最多处理一个；
//! 新快照完整预检成功后才恢复上轮并一起换成既有`DynamicWaterMaterial`；原`Mesh3d`
//! 始终只读保留，整网格流动模式由小型材质uniform编码，任何错误都保留上轮成功画面。

use bevy::{
    camera::visibility::NoFrustumCulling, // GPU波峰可能离开Blender静态AABB，接管期间禁用视锥裁剪。
    gltf::GltfMeshName,                   // 最终身份使用Blender Mesh datablock名与对象节点路径。
    light::{NotShadowCaster, NotShadowReceiver}, // 接管期只禁投影；接影标记仍需快照并在撤销时精确恢复作者状态。
    math::Mat3A,                                 // UV局部边长通过完整层级线性变换成世界米。
    mesh::Indices, // 逻辑索引快照供跨帧三角证据验证，原Mesh资产始终只读。
    prelude::*,    // ECS、资产、变换与层级基础类型。
    world_serialization::{
        InstanceId, WorldAssetRoot, WorldInstance, WorldInstanceReady, WorldInstanceSpawner,
    }, // ready observer与Changed热改共用当前World实例身份。
}; // 完成Bevy依赖导入。
use feathrise_scene_core::view::CameraRaycastIgnore; // 被接管水网格不得挡住相机防穿墙射线。
#[cfg(test)]
use feathrise_map_content::MapPlacementWaterSurface;
use feathrise_map_content::{DEFAULT_MAP_WATER_PERCEPTUAL_ROUGHNESS, MapPlacementWaterKind}; // GPU整网格流动样式复用作者水体类别，并与数据水域共享清晰反射基准。 // 纯身份匹配oracle只在测试构建需要完整作者类型。

use super::components::{
    DynamicWaterMeshOverrideNumericPatch, DynamicWaterMeshOverrides, DynamicWaterSurface,
    WaterJumpCycle,
}; // 公开根输入、受控O(1)数值补丁与既有GPU水面更新组件。
use super::constants::{RIBBON_STYLE_SENTINEL, WATER_TRANSPARENT_SORT_DEPTH_BIAS}; // 整网格uniform复用样条样式哨兵，并统一透明水排序。
use super::footprint::WaterInteractionFootprint; // 复杂导入网格本阶段明确禁用Legacy玩家足迹。
use super::material::{DynamicWaterMaterial, create_dynamic_water_material}; // 仅复用现有GPU水材质，不创建第二套Shader。

#[cfg(test)]
const MAX_MESH_WATER_PRIMITIVES_PER_ROOT: usize = 64; // 旧单根数量阈值仅用于测试夹具，运行时不限制primitive总数。
#[cfg(test)]
const MAX_MESH_WATER_UV_VERTICES_PER_ROOT: usize = 65_536; // 旧单根顶点阈值仅用于回归，实际扫描按每帧游标推进。
const MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE: usize = 128; // 长索引网格均匀抽样而不全量遍历三角形。
#[cfg(test)]
const MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_ROOT: usize =
    MAX_MESH_WATER_PRIMITIVES_PER_ROOT * MAX_MESH_WATER_UV_TRIANGLE_SAMPLES_PER_PRIMITIVE; // 旧根级三角抽样阈值仅用于回归，不限制primitive数量。
const MAX_MESH_WATER_ANCESTORS_TO_WORLD: usize = 128; // 世界Y偏移最多追溯六十四层Blender路径加六十四层placement外部层级，异常深树失败关闭。
#[cfg(test)]
const MAX_MESH_WATER_PENDING_ROOTS: usize = 4_096; // 旧去重队列阈值仅用于回归；合法根总量按实际分配性增长。
const MIN_MESH_WATER_UV_AXIS_SPAN: f32 = 1.0e-3; // 小于一毫米数值跨度无法稳定驱动沿程或横向流场。
const MESH_WATER_UV_V_RANGE_TOLERANCE: f32 = 1.0e-3; // 允许glTF f32量化在0..1边界外千分之一的舍入误差。
const MIN_MESH_WATER_GEOMETRY_EDGE_LENGTH: f32 = 1.0e-4; // 过短退化边不参与UV米制比例证据。
const MIN_MESH_WATER_ALONG_EDGE_U_DELTA: f32 = 1.0e-4; // 无可见沿程U变化的边不能证明流向。
const MIN_MESH_WATER_U_PER_METER_RATIO: f32 = 0.5; // `|dU|/局部三维边长`在二倍容差内才认为米制证据。
const MAX_MESH_WATER_U_PER_METER_RATIO: f32 = 2.0; // 容差允许弯道内外岸、斜水面与跨宽对角边差异。
const STYLE_HORIZONTAL_OR_SLOPED: f32 = 0.0; // 水平面与斜水面都进入既有普通河流逐像素分支。
const STYLE_WATERFALL: f32 = 2.0; // 瀑布使用既有样条瀑布分支，波形沿Blender顶点法线鼓动。

/// 公开根覆盖的内部就绪结果，placement候选门只读取而不修改。
#[derive(Component, Clone, Debug, PartialEq, Eq)] // 失败详情保留精确节点路径供候选诊断，F12只读回显可比较状态变化。
pub enum DynamicWaterMeshOverrideStatus {
    Pending,        // 根已进入有界队列，旧成功水面或Blender原材质继续显示。
    Ready,          // 空覆盖无需恢复，或最新覆盖已经成功原子接管。
    Failed(String), // 新根保持Blender原材质，已有根保留上一轮成功水面并发布精确错误。
} // 结束根接管状态。

impl DynamicWaterMeshOverrideStatus {
    /// 候选事务只在明确Ready时允许推进计数。
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready) // 失败必须由另一分支携带详情拒绝整份修订。
    } // 完成就绪状态读取。

    /// 返回失败具体原因，Pending或Ready时返回None。
    pub fn failure_detail(&self) -> Option<&str> {
        match self {
            Self::Pending | Self::Ready => None, // 等待或成功状态都没有失败文本。
            Self::Failed(detail) => Some(detail), // 借用原诊断，不在每帧结算时复制字符串。
        } // 完成状态分支读取。
    } // 完成失败详情读取。
} // 结束根接管状态读取接口。

/// 根级水体状态所证明的World实例身份；GLB换代后旧Ready不能继续作为候选或保存证据。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::shared::large_map::water) struct DynamicWaterMeshOverrideInstanceIdentity {
    instance_id: InstanceId, // 根组件替换时仍拒绝旧WorldInstanceReady。
    ready_epoch: u64,        // 原地热重载复用InstanceId时以WorldAsset epoch拒绝旧Ready。
} // 只保存O(1)票据，不持有场景或资产快照。

/// 记录本轮已接管primitive的有界实体表，热改恢复不再扫描全世界。
#[derive(Component, Clone, Debug, Default)] // 数量在成功接管前已被根总额常量封顶。
pub(super) struct DynamicWaterMeshOverrideApplied {
    entities: Vec<Entity>, // 仅保存本根自己接管的primitive，不会命中旧GLB白名单水面。
} // 结束已接管根索引。

/// 每个导入水primitive的完整恢复记录。
#[derive(Component, Clone)] // 原句柄是强句柄，接管期间glTF共享资产不会被意外卸载。
pub(super) struct DynamicWaterMeshOverridePrimitive {
    owner_root: Entity, // 恢复时再核对所有者，禁止跨placement根操作。
    original_material: Handle<StandardMaterial>, // 原Blender PBR材质句柄用于清空或失败恢复。
    original_transform: Transform, // 高度偏移永远从原primitive局部位姿重算，不累加。
    dynamic_material: Handle<DynamicWaterMaterial>, // 本次独占扩展材质在热改、撤销或despawn时精确释放。
    had_no_frustum_culling: bool, // 记住原primitive是否已有该标记，恢复不误删作者状态。
    had_camera_raycast_ignore: bool, // 相机射线标记同样按原状态精确恢复。
    had_not_shadow_caster: bool,  // 原本不投影的网格在撤销赋水后仍保留设定。
    had_not_shadow_receiver: bool, // 接管期会强制接影，撤销时据此精确恢复作者原标记。
} // 结束单primitive恢复记录。

/// 只读导入primitive查询；已接管项会缺少StandardMaterial，由内部恢复记录提供原值。
pub(super) type ImportedPrimitiveData = (
    Entity,
    &'static GltfMeshName,
    Option<&'static Mesh3d>,
    Option<&'static MeshMaterial3d<StandardMaterial>>,
    Option<&'static Transform>,
    &'static ChildOf,
    Has<NoFrustumCulling>,
    Has<CameraRaycastIgnore>,
    Has<NotShadowCaster>,
    Has<NotShadowReceiver>,
); // 结束只读primitive查询布局。

/// 匹配前从当前层级或上轮恢复记录得到的原始primitive快照。
#[derive(Clone)] // 小型快照仅保持至本次预检完成。
struct PrimitiveCandidate {
    entity: Entity,                                      // 最终接管的primitive实体。
    object_node: Entity,        // 区分同路径重名Blender对象，歧义时失败关闭。
    node_path: Vec<String>,     // 从当前placement根到Blender对象节点的全路径。
    mesh_name: String,          // `GltfMeshName`与路径共同构成最终身份。
    mesh: Option<Handle<Mesh>>, // 原`Mesh3d`只用于属性预检，应用与恢复都不替换其句柄。
    original_material: Option<Handle<StandardMaterial>>, // 已接管项从内部记录恢复该值。
    original_transform: Option<Transform>, // 局部位姿缺失时不能安全应用高度偏移。
    had_no_frustum_culling: bool, // 以原始状态而非当前接管状态建立新恢复记录。
    had_camera_raycast_ignore: bool, // 只记录原标记是否存在。
    had_not_shadow_caster: bool, // 只记录原标记是否存在。
    had_not_shadow_receiver: bool, // 只记录原标记是否存在。
} // 结束原始primitive快照。

/// 全量预检与分帧材质登记都通过后才持有的小型替换完成值。
struct PreparedPrimitive {
    candidate: PrimitiveCandidate, // 应用与恢复都使用同一原始快照。
    dynamic_material: Handle<DynamicWaterMaterial>, // 尚未挂到实体的独占动态材质，取消时按固定预算释放。
    flow_speed: f32, // 只写入独占材质uniform，原Mesh不克隆也不增加顶点属性。
    flow_style_encoding: Vec2, // surface_settings.y/z中的强制整网格样式，不依赖或改写原UV1。
    local_translation_offset: Vec3, // 世界Y米制高度已在预检期反算为primitive父坐标中的局部位移。
} // 结束单primitive预检完成值。

/// 将作者类别映射到现有WGSL的流场样式数值。
fn water_style(kind: MapPlacementWaterKind) -> f32 {
    match kind {
        MapPlacementWaterKind::Horizontal | MapPlacementWaterKind::Sloped => {
            STYLE_HORIZONTAL_OR_SLOPED // 两者都保留Blender几何，只使用普通河流流纹。
        }
        MapPlacementWaterKind::Waterfall => STYLE_WATERFALL, // 瀑布切换幕布法线波动与瀑布流纹。
    } // 完成三种作者类别映射。
} // 结束样式编码。

/// 在原本只接受单位流向的uniform槽编码强制整网格模式，绝不改写共享Mesh属性。
fn encoded_whole_mesh_flow_style(kind: MapPlacementWaterKind) -> Vec2 {
    Vec2::new(RIBBON_STYLE_SENTINEL + water_style(kind), 0.0) // x高于任何单位方向分量，y保持有限占位。
} // 结束uniform整网格模式编码。

/// 读取一个TriangleList逻辑索引，不复制U16/U32索引资产。
fn mesh_water_vertex_index(
    indices: Option<&Indices>, // glTF primitive可以有索引或直接以连续顶点成三角形。
    logical_index: usize,      // 三角列表中的逻辑顶点位置。
    vertex_count: usize,       // POSITION与UV0已核对的共同长度。
) -> Result<usize, &'static str> {
    let vertex_index = match indices {
        Some(Indices::U16(values)) => values
            .get(logical_index)
            .copied()
            .map(usize::from)
            .ok_or("三角索引超出U16索引表")?, // 只按抽样位置O(1)读取。
        Some(Indices::U32(values)) => values
            .get(logical_index)
            .copied()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or("三角索引超出U32索引表")?, // 不分配临时u32数组。
        None => logical_index, // 无索引TriangleList每三个连续顶点构成一个面。
    }; // 完成逻辑索引读取。
    if vertex_index >= vertex_count {
        return Err("三角索引引用了不存在的POSITION/UV0顶点"); // 抽样命中损坏索引时失败关闭。
    } // 完成顶点边界检查。
    Ok(vertex_index) // 返回可安全读取两类属性的索引。
} // 结束三角逻辑索引读取。

/// 有界验证整网格UV0的有限值、轴跨度、V归一范围与U局部米制证据。
#[cfg(test)] // 旧全量纯函数只保留为与增量校验结果对照的测试oracle，不进入帧关键生产代码。
fn validate_whole_mesh_water_uv0(
    positions: &[[f32; 3]], // 原Mesh局部POSITION，仅读且已受全根顶点数硬门保护。
    uv0: &[[f32; 2]],       // 作者约定U为沿程米、V为横向0..1。
    indices: Option<&Indices>, // 只均匀抽样三角边，不克隆或遍历超长索引表。
    world_from_mesh: Mat3A, // 包含primitive、Blender节点、placement和外层缩放的世界线性变换。
    triangle_sample_budget: usize, // 调用方传入当前根剩余固定预算。
) -> Result<usize, String> {
    if !world_from_mesh.is_finite() {
        return Err("Mesh局部到世界的线性变换包含非有限值".to_owned()); // 不用损坏层级猜测世界米制边长。
    } // 完成几何层级变换有限值门。
    if positions.is_empty() || uv0.is_empty() {
        return Err("POSITION/UV0不能为空".to_owned()); // 空属性无法生成任何流场证据。
    } // 完成空属性失败门。
    if positions.len() != uv0.len() {
        return Err("POSITION与UV0顶点数不一致".to_owned()); // 每个渲染顶点必须有一一对应沿程坐标。
    } // 完成属性长度门。
    let mut min_u = f32::INFINITY; // 累计全网格U数值范围。
    let mut max_u = f32::NEG_INFINITY; // 累计全网格U数值范围。
    let mut min_v = f32::INFINITY; // 累计全网格V数值范围。
    let mut max_v = f32::NEG_INFINITY; // 累计全网格V数值范围。
    for (&position, &uv) in positions.iter().zip(uv0) {
        if !Vec3::from_array(position).is_finite() {
            return Err("POSITION包含NaN或无穷值，无法建立UV米制证据".to_owned()); // 几何边长必须是有限值。
        } // 完成单顶点位置有限值检查。
        let [u, v] = uv; // 拆分沿程和横向作者坐标。
        if !u.is_finite() || !v.is_finite() {
            return Err("UV0包含NaN或无穷值".to_owned()); // 任一非有限值都会污染GPU流场。
        } // 完成单顶点UV有限值检查。
        if !(-MESH_WATER_UV_V_RANGE_TOLERANCE..=1.0 + MESH_WATER_UV_V_RANGE_TOLERANCE).contains(&v)
        {
            return Err(format!(
                "UV0.V={v}明显超出横向0..1约定（容差±{MESH_WATER_UV_V_RANGE_TOLERANCE}）"
            )); // 不把常规纹理平铺V冒充河面横向权重。
        } // 完成V范围门。
        min_u = min_u.min(u); // 更新U下界。
        max_u = max_u.max(u); // 更新U上界。
        min_v = min_v.min(v); // 更新V下界。
        max_v = max_v.max(v); // 更新V上界。
    } // 完成受全根顶点预算限制的属性扫描。
    let u_span = max_u - min_u; // 先显式计算沿程数值跨度，两个极大有限值的差也可能溢出。
    if !u_span.is_finite() {
        return Err("UV0.U的有限端点差值溢出".to_owned()); // 不让无穷跨度进入边比例计算。
    } // 完成U跨度有限值门。
    if u_span < MIN_MESH_WATER_UV_AXIS_SPAN {
        return Err(format!(
            "UV0.U没有有效沿程跨度（至少{MIN_MESH_WATER_UV_AXIS_SPAN}）"
        )); // 全零或近似常数U无法显示流动。
    } // 完成U跨度门。
    let v_span = max_v - min_v; // V已受边界门保护，仍显式保持与U相同的有限语义。
    if !v_span.is_finite() {
        return Err("UV0.V的有限端点差值溢出".to_owned()); // 防御未来调整V范围时引入静默溢出。
    } // 完成V跨度有限值门。
    if v_span < MIN_MESH_WATER_UV_AXIS_SPAN {
        return Err(format!(
            "UV0.V没有有效横向跨度（至少{MIN_MESH_WATER_UV_AXIS_SPAN}）"
        )); // 常数V无法产生岸边至中心的横向语义。
    } // 完成V跨度门。

    let logical_index_count = indices.map_or(positions.len(), Indices::len); // 只读长度，不遍历完整索引数组。
    if logical_index_count < 3 || logical_index_count % 3 != 0 {
        return Err("TriangleList索引/顶点数必须是非零的3的倍数".to_owned()); // 非完整三角形无法建立边证据。
    } // 完成三角列表数量门。
    if triangle_sample_budget == 0 {
        return Err("当前placement的UV几何抽样超出单根固定预算".to_owned()); // 不为超额primitive开启无界工作。
    } // 完成抽样预算门。
    let triangle_count = logical_index_count / 3; // TriangleList每三个逻辑索引是一个面。
    let sampled_triangles = triangle_count.min(triangle_sample_budget); // 小网格全检，大网格只做确定性均匀抽样。
    let mut along_edges = 0usize; // 记录U变化显著大于V变化的沿程候选边。
    let mut meter_scale_edges = 0usize; // 记录U增量与局部三维边长在二倍容差内的边。
    for sample_index in 0..sampled_triangles {
        let triangle_index = sample_index * triangle_count / sampled_triangles; // 从整个索引域均匀取样，不只检查网格开头。
        let logical_base = triangle_index * 3; // 定位当前抽样三角的首索引。
        let triangle = [
            mesh_water_vertex_index(indices, logical_base, positions.len())?,
            mesh_water_vertex_index(indices, logical_base + 1, positions.len())?,
            mesh_water_vertex_index(indices, logical_base + 2, positions.len())?,
        ]; // 只保存三个usize，不构建索引副本。
        for [from, to] in [
            [triangle[0], triangle[1]],
            [triangle[1], triangle[2]],
            [triangle[2], triangle[0]],
        ] {
            let du = (uv0[to][0] - uv0[from][0]).abs(); // 边的沿程数值增量不依赖三角绕序。
            let dv = (uv0[to][1] - uv0[from][1]).abs(); // 边的横向增量用于排除纯横截面与明显跨宽边。
            let local_edge = Vec3::from_array(positions[to]) - Vec3::from_array(positions[from]); // 先在原Mesh局部空间取无平移边向量。
            let edge_length = (world_from_mesh * local_edge).length(); // 组合全层级旋转/缩放后比较真正世界米，placement缩放不能伪装米制U。
            if edge_length < MIN_MESH_WATER_GEOMETRY_EDGE_LENGTH
                || du < MIN_MESH_WATER_ALONG_EDGE_U_DELTA
                || du < dv * 2.0
            {
                continue; // 退化边、无U变化边与V主导跨宽边不纳入沿程尺度统计。
            } // 完成沿程候选边过滤。
            along_edges = along_edges.saturating_add(1); // 最大只是固定抽样三角数的三倍。
            let u_per_meter = du / edge_length; // 真正米制U在局部沿程边上应接近1。
            if (MIN_MESH_WATER_U_PER_METER_RATIO..=MAX_MESH_WATER_U_PER_METER_RATIO)
                .contains(&u_per_meter)
            {
                meter_scale_edges = meter_scale_edges.saturating_add(1); // 弯道和对角边允许二倍有限容差。
            } // 完成单边米制证据分类。
        } // 完成当前抽样三角的三边分析。
    } // 完成固定上限均匀三角抽样。
    let minimum_evidence_edges = if triangle_count == 1 { 1 } else { 2 }; // 单三角小水面允许一条纯沿程边，更大网格至少要两条。
    if along_edges < minimum_evidence_edges
        || meter_scale_edges < minimum_evidence_edges
        || meter_scale_edges.saturating_mul(2) < along_edges
    {
        return Err(format!(
            "UV0.U缺少局部米制沿程证据：沿程候选边{along_edges}条，米制比例边{meter_scale_edges}条；长河不能使用归一化U=0..1"
        )); // 多数边尺度明显不符时失败，不让静态纹理UV冒充沿程米。
    } // 完成局部米制U证据门。
    Ok(sampled_triangles) // 返回实际消费的全根三角预算。
} // 结束整网格UV0语义预检。

/// 判断当前根是否必须进入逐帧队列；普通空覆盖且无旧接管可立即Ready。
fn requires_pending_processing(
    overrides: &DynamicWaterMeshOverrides, // 当前根最新完整作者快照。
    has_applied_override: bool,            // 上轮成功水材质是否仍需恢复。
) -> bool {
    !overrides.surfaces().is_empty() || has_applied_override // 新覆盖要预检，空覆盖只有存在旧接管时才需要恢复。
} // 结束待处理判定。

/// 从`Mesh{n}/Primitive{m}`标签解析仅供诊断的glTF mesh索引。
fn parse_gltf_mesh_index(label: &str) -> Option<u32> {
    let (mesh, primitive) = label.strip_prefix("Mesh")?.split_once("/Primitive")?; // 严格接受Bevy 0.19 `GltfAssetLabel::Primitive`字面格式。
    if primitive.is_empty() || !primitive.bytes().all(|byte| byte.is_ascii_digit()) {
        return None; // primitive段也必须完整为十进制数，避免部分匹配。
    } // 完成primitive标签尾部校验。
    mesh.parse().ok() // mesh索引仅诊断，溢出或非数字都安全返回None。
} // 结束glTF mesh索引解析。

/// 精确比较节点全路径与`GltfMeshName`，绝不使用mesh_index决定接管。
#[cfg(test)] // 生产路径已用SurfaceIdentity HashMap完成同一精确语义，纯helper仅服务回归断言。
fn candidate_matches(
    surface: &MapPlacementWaterSurface, // 作者最终身份。
    candidate: &PrimitiveCandidate,     // 当前placement根内的某一primitive快照。
) -> bool {
    candidate.node_path == surface.node_path && candidate.mesh_name == surface.mesh_name // 两部分必须同时完全相等。
} // 结束最终身份比较。

/// 将WorldAsset实例层级路径归一为与GLB浏览器增量Scene0遍历一致的作者路径。
#[cfg(test)] // 纯回归直接验证浏览器路径归一规则，运行时在父链追溯时就跳过该节点。
fn without_world_scene_root(mut path: Vec<String>) -> Vec<String> {
    if !path.is_empty() {
        path.remove(0); // `WorldAsset.world`的顶层scene root是Bevy实例化容器，不是Blender作者节点。
    } // 空路径保持空，后续公共校验会对无Blender节点值失败关闭。
    path // 返回从scene root的下一层Blender节点开始的路径。
} // 结束World scene root排除。

mod queue; // 固定容量FIFO只保存去重根身份，作者快照继续以根组件最新值为权威。
mod runtime; // ECS observer、热改恢复、全量预检与原子应用保持在独立运行时模块。

pub(super) use queue::DynamicWaterMeshOverrideQueue; // 水插件初始化唯一固定容量队列资源。
use queue::MeshWaterEnqueueResult; // observer与Changed收集按入队结果发布根状态。
pub(super) use runtime::DynamicWaterMeshOverrideWorkState; // 水插件初始化唯一single-flight增量工作资源。

/// 整网格赋水收集与单根处理的公开crate内调度边界。
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)] // placement结算显式等待ProcessPending集合之后再读取状态。
pub(crate) enum DynamicWaterMeshOverrideSystems {
    ProcessPending, // 每帧最多弹出一个根并执行现有单根硬预算预检与提交。
} // 结束整网格赋水调度集合。

pub(super) use runtime::{
    RetiredMeshWaterMaterials, apply_dynamic_water_mesh_numeric_patch,
    cancel_removed_dynamic_water_mesh_world_asset, cleanup_removed_mesh_override_assets,
    initialize_dynamic_water_mesh_overrides, invalidate_discarded_dynamic_water_mesh_world_asset,
    invalidate_dynamic_water_mesh_asset_epoch, process_pending_dynamic_water_mesh_override,
    queue_inserted_dynamic_water_mesh_overrides, restore_removed_dynamic_water_mesh_overrides,
    retire_removed_mesh_override_materials,
}; // 水体插件只注册轻量入队、单根处理与恢复入口，不暴露内部资产记录。

#[cfg(test)] // 身份匹配、索引诊断和uniform模式都是无IO纯回归。
mod tests; // 测试与运行时资产接管分离，主模块保持可维护行数。
