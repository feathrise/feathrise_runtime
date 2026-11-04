//! 动态地图单 primitive 白模 GLB 构建器。

use bevy::math::{Mat4, Quat, Vec3}; // 读取Blender节点层级时按glTF列主序组合完整世界变换。
use serde_json::{Value, json}; // glTF JSON块使用结构化值生成，避免中文名称转义错误。

use crate::{
    // GLB保存与内存派生共享同一网格校验、法线和采样规格。
    TerrainMeshInput,
    TerrainSampleSpec,
    derive::{derive_vertex_normals, mesh_bounds, validate_world_height_bounds},
}; // 完成共享算法导入。

const GLB_MAGIC: u32 = 0x4654_6C67; // ASCII `glTF`小端魔数。
const GLB_VERSION: u32 = 2; // 输出固定为glTF 2.0。
const JSON_CHUNK_TYPE: u32 = 0x4E4F_534A; // ASCII `JSON`块类型。
const BIN_CHUNK_TYPE: u32 = 0x004E_4942; // ASCII `BIN\0`块类型。

/// 动态地图唯一地形Mesh名称；场景运行时、F12加载和保存共用。
pub const DYNAMIC_TERRAIN_MESH_NAME: &str = "动态地图地形_网格";
/// 动态地图唯一白模材质名称；额外材质属性不属于F12保存契约。
pub const DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME: &str = "地表_白模";

/// 规则平地GLB的精确数量和BIN布局；只保存整数，不分配与地图面积相关的内存。
struct FlatGlbLayout {
    vertex_count: usize,   // POSITION与NORMAL各有一个元素对应每个顶点。
    index_count: usize,    // 每格两个三角形，共六个U32索引。
    position_bytes: usize, // POSITION在BIN中的字节数。
    normal_bytes: usize,   // NORMAL在BIN中的字节数。
    index_bytes: usize,    // 索引在BIN中的字节数。
    binary_bytes: usize,   // 唯一BIN块的完整字节数。
}

/// 只按GLB/u32格式容量预检新建平地分辨率，不把地图尺寸绑到固定单轴格数。
pub fn validate_flat_terrain_glb_resolution(resolution: [u32; 2]) -> Result<(), String> {
    flat_glb_layout(resolution).map(|_| ()) // 编辑器预览与后台创建使用同一无大分配容量门。
}

/// 计算单primitive平地的格式大小，避免为了验证精细地图而先分配巨大网格。
fn flat_glb_layout(resolution: [u32; 2]) -> Result<FlatGlbLayout, String> {
    let [width, height] = resolution; // 两轴均为包含边界端点的顶点数。
    if width < 2 || height < 2 {
        return Err("新地图地形GLB网格每轴至少需要2个顶点".to_owned()); // 防止减一和空拓扑。
    }
    let vertex_count = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| "地形GLB顶点数量溢出".to_owned())?; // 先在宽整数中计算真实顶点数。
    if vertex_count > u64::from(u32::MAX) {
        return Err("单个地形GLB的顶点数超过U32索引容量；需要分块地形".to_owned()); // 单primitive索引必须能访问全部顶点。
    }
    let index_count = u64::from(width - 1)
        .checked_mul(u64::from(height - 1))
        .and_then(|cells| cells.checked_mul(6))
        .ok_or_else(|| "地形GLB索引数量溢出".to_owned())?; // 每格固定两个朝上三角形。
    let position_bytes = vertex_count
        .checked_mul(12)
        .ok_or_else(|| "地形GLB位置字节数溢出".to_owned())?; // 每顶点三个F32。
    let normal_bytes = position_bytes; // 每顶点另存三个F32法线。
    let index_bytes = index_count
        .checked_mul(4)
        .ok_or_else(|| "地形GLB索引字节数溢出".to_owned())?; // 索引固定为U32。
    let binary_bytes = position_bytes
        .checked_add(normal_bytes)
        .and_then(|bytes| bytes.checked_add(index_bytes))
        .ok_or_else(|| "地形GLB BIN块长度溢出".to_owned())?; // 三段数据连续写入唯一BIN块。
    if binary_bytes > u64::from(u32::MAX) {
        return Err("单个地形GLB的BIN块超过4GiB格式长度；需要分块地形".to_owned()); // GLB块长字段为U32。
    }
    let vertex_count =
        usize::try_from(vertex_count).map_err(|_| "平台无法表示地形GLB顶点数".to_owned())?; // 最终Vec长度必须可表示。
    let index_count =
        usize::try_from(index_count).map_err(|_| "平台无法表示地形GLB索引数".to_owned())?; // 索引循环使用同一计数。
    let position_bytes =
        usize::try_from(position_bytes).map_err(|_| "平台无法表示地形GLB位置字节数".to_owned())?; // JSON bufferView偏移需与写入一致。
    let normal_bytes =
        usize::try_from(normal_bytes).map_err(|_| "平台无法表示地形GLB法线字节数".to_owned())?; // NORMAL长度与POSITION相同。
    let index_bytes =
        usize::try_from(index_bytes).map_err(|_| "平台无法表示地形GLB索引字节数".to_owned())?; // U32索引段长度。
    let binary_bytes =
        usize::try_from(binary_bytes).map_err(|_| "平台无法表示地形GLB BIN块长度".to_owned())?; // BIN块长度上限已验证。
    // 预览使用最宽的F32边界文字核对容器容量；正式构建还会按真实JSON字节再精确检查。
    let worst_document = white_terrain_document(
        vertex_count,
        index_count,
        position_bytes,
        normal_bytes,
        index_bytes,
        binary_bytes,
        [f32::MIN; 3],
        [f32::MAX; 3],
    );
    let worst_json_len = serde_json::to_vec(&worst_document)
        .map_err(|error| format!("序列化地形GLB预检文档失败：{error}"))?
        .len(); // 元数据大小只与数字位数有关，不生成顶点或索引。
    let padded_json_len = worst_json_len
        .checked_add(3)
        .ok_or_else(|| "地形GLB JSON长度溢出".to_owned())?
        & !3; // GLB JSON块按四字节对齐。
    let total_length = 28usize
        .checked_add(padded_json_len)
        .and_then(|bytes| bytes.checked_add(binary_bytes))
        .ok_or_else(|| "地形GLB文件长度溢出".to_owned())?; // 12字节头和两个8字节块头。
    if total_length > u32::MAX as usize {
        return Err("单个地形GLB超过4GiB格式长度；需要分块地形".to_owned()); // 文件头长度字段为U32。
    }
    Ok(FlatGlbLayout {
        vertex_count,
        index_count,
        position_bytes,
        normal_bytes,
        index_bytes,
        binary_bytes,
    }) // 返回供构建器复用的真实布局。
}

/// 从完整GLB提取唯一动态地形primitive并烘焙其场景节点世界变换。
pub fn parse_dynamic_terrain_glb(bytes: &[u8]) -> Result<TerrainMeshInput, String> {
    let (document, binary) = parse_glb_chunks(bytes)?; // 文件头和两个必需块先完整验证。
    let meshes = json_array(&document, "meshes")?; // 动态地形必须来自显式mesh表。
    let matching_meshes = meshes
        .iter()
        .enumerate()
        .filter(|(_, mesh)| {
            mesh.get("name").and_then(Value::as_str) == Some(DYNAMIC_TERRAIN_MESH_NAME)
        })
        .collect::<Vec<_>>(); // 名称契约允许GLB存在其他非地形对象，但地形本身必须唯一。
    let [(mesh_index, mesh)] = matching_meshes.as_slice() else {
        return Err(format!(
            "GLB必须且只能包含一个名为{DYNAMIC_TERRAIN_MESH_NAME}的地形Mesh，当前为{}个",
            matching_meshes.len()
        ));
    }; // 完成唯一地形Mesh门。
    let primitives = mesh
        .get("primitives")
        .and_then(Value::as_array)
        .ok_or_else(|| "动态地形Mesh缺少primitives".to_owned())?; // primitive列表必须显式存在。
    let [primitive] = primitives.as_slice() else {
        return Err(format!(
            "动态地形Mesh必须只有一个primitive，当前为{}个",
            primitives.len()
        ));
    }; // F12无法保存多材质或多primitive扩展属性。
    if primitive.get("mode").and_then(Value::as_u64).unwrap_or(4) != 4 {
        return Err("动态地形primitive必须使用TriangleList模式".to_owned()); // 不猜测strip/fan连接关系。
    } // 完成primitive拓扑门。
    validate_white_material(&document, primitive)?; // 防止误接管同名但非白模材质的场景对象。
    let transform = unique_terrain_node_transform(&document, *mesh_index)?; // 场景层级只允许实例化一次该地形Mesh。
    let position_accessor = primitive
        .get("attributes")
        .and_then(Value::as_object)
        .and_then(|attributes| attributes.get("POSITION"))
        .and_then(Value::as_u64)
        .ok_or_else(|| "动态地形primitive缺少POSITION accessor".to_owned())?
        as usize; // POSITION是唯一必需顶点属性。
    let accessors = json_array(&document, "accessors")?; // 位置与索引都引用同一受检表。
    let positions = read_positions(
        accessors
            .get(position_accessor)
            .ok_or_else(|| "POSITION accessor越界".to_owned())?,
        &document,
        binary,
        transform,
    )?; // 读取并烘焙完整节点变换。
    let indices = if let Some(index_accessor) = primitive.get("indices").and_then(Value::as_u64) {
        read_indices(
            accessors
                .get(index_accessor as usize)
                .ok_or_else(|| "索引accessor越界".to_owned())?,
            &document,
            binary,
        )? // U8/U16/U32统一提升为U32并保持顺序。
    } else {
        (0..positions.len())
            .map(|index| u32::try_from(index).map_err(|_| "无索引地形顶点数超过U32".to_owned()))
            .collect::<Result<Vec<_>, _>>()? // 无索引TriangleList规范化为显式连续索引。
    }; // 完成索引读取。
    Ok(TerrainMeshInput { positions, indices }) // 详细三维面积、高度量程与顶层采样门由共享派生算法继续验证。
} // 结束GLB地形提取。

/// 按独立于高度采样的 GLB 网格分辨率创建新地图平面。
pub fn build_flat_terrain_mesh(
    spec: TerrainSampleSpec, // 新地图清单的范围、分辨率和高度量程。
    world_y: f32,            // 初始平面的世界Y，通常为零。
) -> Result<TerrainMeshInput, String> {
    let [width, height] = spec.resolution; // 此处的分辨率只描述GLB顶点网格，不描述高度采样。
    let layout = flat_glb_layout(spec.resolution)?; // 只受实际U32索引和GLB文件长度约束，不再按单轴1024格截断。
    validate_flat_terrain_world(spec, world_y)?; // 平面位置和可编辑Y量程与运行时共享契约。
    let mut positions = Vec::new(); // 先创建空向量，内存不足时返回错误而非直接申请失败。
    positions
        .try_reserve_exact(layout.vertex_count)
        .map_err(|error| format!("申请地形GLB顶点内存失败：{error}"))?; // 旧通用Mesh入口仍须可诊断内存失败。
    for z in 0..height {
        let world_z = sample_axis(spec.world_min[1], spec.world_max[1], z, height); // Z端点严格对应清单边界。
        for x in 0..width {
            let world_x = sample_axis(spec.world_min[0], spec.world_max[0], x, width); // X端点严格对应清单边界。
            positions.push([world_x, world_y, world_z]); // 新图只保存白模世界空间位置。
        } // 完成当前Z行顶点。
    } // 完成规则顶点生成。
    let mut indices = Vec::new(); // 索引保留原项目绕序，容量由纯预检结果确定。
    indices
        .try_reserve_exact(layout.index_count)
        .map_err(|error| format!("申请地形GLB索引内存失败：{error}"))?; // 容量不足时不创建半份Mesh。
    for z in 0..height - 1 {
        for x in 0..width - 1 {
            let a = z * width + x; // 单元北西顶点。
            let b = a + 1; // 单元北东顶点。
            let c = a + width; // 单元南西顶点。
            let d = c + 1; // 单元南东顶点。
            indices.extend_from_slice(&[a, c, b, b, c, d]); // 保持项目正式朝上NE-SW对角绕序。
        } // 完成当前Z行单元。
    } // 完成全部规则索引。
    Ok(TerrainMeshInput { positions, indices }) // 返回可直接编码GLB或后台派生的唯一源。
} // 结束新地图平面构建。

/// 新图平面与派生地形共享同一世界范围和可编辑高度约束。
fn validate_flat_terrain_world(spec: TerrainSampleSpec, world_y: f32) -> Result<(), String> {
    validate_world_height_bounds(spec)?; // 分辨率由独立GLB格式预检负责，世界范围仍复用共享规则。
    let maximum = spec.base_height + spec.height_scale; // 计算完整可编辑上界。
    if !world_y.is_finite() || !(spec.base_height..=maximum).contains(&world_y) {
        return Err(format!(
            "新地图初始高度{world_y}超出可编辑范围{}..{maximum}",
            spec.base_height
        )); // 初始平面必须立即能派生高度。
    }
    Ok(()) // 世界和初始Y均可供平面生成。
}

/// 新建规则平地直接写入最终GLB缓冲，避免同时驻留Mesh、法线、BIN和GLB四份大数据。
pub fn build_flat_terrain_glb(spec: TerrainSampleSpec, world_y: f32) -> Result<Vec<u8>, String> {
    let layout = flat_glb_layout(spec.resolution)?; // 在申请内存前按真实格式验证顶点、索引和文件字节数。
    validate_flat_terrain_world(spec, world_y)?; // 不允许越界或非有限坐标写入GLB。
    let [width, height] = spec.resolution; // 两轴顶点数量包含世界四条边。
    let min = [spec.world_min[0], world_y, spec.world_min[1]]; // 平面包围盒下界直接来自清单。
    let max = [spec.world_max[0], world_y, spec.world_max[1]]; // 平面包围盒上界直接来自清单。
    let document = white_terrain_document(
        layout.vertex_count,
        layout.index_count,
        layout.position_bytes,
        layout.normal_bytes,
        layout.index_bytes,
        layout.binary_bytes,
        min,
        max,
    ); // 与通用保存入口使用同一primitive与材质协议。
    let mut json_bytes =
        serde_json::to_vec(&document).map_err(|error| format!("序列化GLB JSON块失败：{error}"))?; // 元数据只有常数规模。
    pad_to_four(&mut json_bytes, b' '); // JSON块必须四字节对齐。
    let total_length = 28usize
        .checked_add(json_bytes.len())
        .and_then(|bytes| bytes.checked_add(layout.binary_bytes))
        .ok_or_else(|| "地形GLB文件长度溢出".to_owned())?; // 精确核对真实JSON后的完整文件长度。
    let total_length_u32 = u32::try_from(total_length)
        .map_err(|_| "单个地形GLB超过4GiB格式长度；需要分块地形".to_owned())?; // GLB头长度字段为U32。
    let json_length_u32 =
        u32::try_from(json_bytes.len()).map_err(|_| "地形GLB JSON块超过U32长度".to_owned())?; // 块头使用U32长度。
    let binary_length_u32 =
        u32::try_from(layout.binary_bytes).map_err(|_| "地形GLB BIN块超过U32长度".to_owned())?; // 由预检保证仍明确转换。
    let mut glb = Vec::new(); // 只申请最终文件缓冲，不构建临时Mesh和第二份BIN。
    glb.try_reserve_exact(total_length)
        .map_err(|error| format!("申请地形GLB文件内存失败（需要{total_length}字节）：{error}"))?; // 内存失败返回可见错误。
    glb.extend_from_slice(&GLB_MAGIC.to_le_bytes()); // GLB文件魔数。
    glb.extend_from_slice(&GLB_VERSION.to_le_bytes()); // glTF 2.0版本。
    glb.extend_from_slice(&total_length_u32.to_le_bytes()); // 完整文件长度。
    glb.extend_from_slice(&json_length_u32.to_le_bytes()); // JSON块长度。
    glb.extend_from_slice(&JSON_CHUNK_TYPE.to_le_bytes()); // JSON块类型。
    glb.extend_from_slice(&json_bytes); // 常数规模GLB文档。
    glb.extend_from_slice(&binary_length_u32.to_le_bytes()); // BIN块长度。
    glb.extend_from_slice(&BIN_CHUNK_TYPE.to_le_bytes()); // BIN块类型。
    for z in 0..height {
        let world_z = sample_axis(spec.world_min[1], spec.world_max[1], z, height); // 当前Z行使用真实等距坐标。
        for x in 0..width {
            let world_x = sample_axis(spec.world_min[0], spec.world_max[0], x, width); // 最后X点精确取世界上界。
            for value in [world_x, world_y, world_z] {
                glb.extend_from_slice(&value.to_le_bytes()); // POSITION直接写入最终BIN段。
            }
        }
    }
    const UP_NORMAL_BYTES: [u8; 12] = [0, 0, 0, 0, 0, 0, 128, 63, 0, 0, 0, 0]; // 平面朝上绕序的法线恒为(0,1,0)。
    for _ in 0..layout.vertex_count {
        glb.extend_from_slice(&UP_NORMAL_BYTES); // NORMAL无需计算或持有逐顶点临时数组。
    }
    for z in 0..height - 1 {
        for x in 0..width - 1 {
            let a = z * width + x; // 单元北西顶点。
            let b = a + 1; // 单元北东顶点。
            let c = a + width; // 单元南西顶点。
            let d = c + 1; // 单元南东顶点。
            for index in [a, c, b, b, c, d] {
                glb.extend_from_slice(&index.to_le_bytes()); // 与旧平面构建器保持完全相同的朝上拓扑。
            }
        }
    }
    debug_assert_eq!(glb.len(), total_length); // 保护布局计算与写入顺序的一致性。
    Ok(glb) // 返回唯一大缓冲供地图创建事务写盘。
}

/// 保留顶点顺序和三角连接关系，重算法线后编码最小白模 GLB。
pub fn build_white_terrain_glb(mesh: &TerrainMeshInput) -> Result<Vec<u8>, String> {
    let position_bytes = mesh
        .positions
        .len()
        .checked_mul(12)
        .ok_or_else(|| "地形GLB位置字节数溢出".to_owned())?; // 每顶点三个F32。
    let normal_bytes = position_bytes; // NORMAL与POSITION按同一顶点数存三个F32。
    let index_bytes = mesh
        .indices
        .len()
        .checked_mul(4)
        .ok_or_else(|| "地形GLB索引字节数溢出".to_owned())?; // 索引固定使用U32。
    let binary_bytes = position_bytes
        .checked_add(normal_bytes)
        .and_then(|bytes| bytes.checked_add(index_bytes))
        .ok_or_else(|| "地形GLB BIN块长度溢出".to_owned())?; // 分配前核对完整BIN载荷。
    let binary_length_u32 = u32::try_from(binary_bytes)
        .map_err(|_| "单个地形GLB的BIN块超过4GiB格式长度；需要分块地形".to_owned())?; // GLB块长使用U32。
    let normals = derive_vertex_normals(mesh)?; // 保存前与运行时使用完全相同的三维拓扑和作者绕序法线。
    let (min, max) = mesh_bounds(&mesh.positions); // GLB accessor写入真实世界包围盒。
    debug_assert_eq!(normals.len(), mesh.positions.len()); // 共享法线算法须一顶点对应一法线。
    let document = white_terrain_document(
        mesh.positions.len(),
        mesh.indices.len(),
        position_bytes,
        normal_bytes,
        index_bytes,
        binary_bytes,
        min,
        max,
    ); // 与新图平面直写入口共享唯一白模GLB文档结构。
    let mut json_bytes =
        serde_json::to_vec(&document).map_err(|error| format!("序列化GLB JSON块失败：{error}"))?; // JSON错误不产生半份文件。
    pad_to_four(&mut json_bytes, b' '); // JSON块使用空格满足四字节对齐。
    let total_length = 28usize
        .checked_add(json_bytes.len())
        .and_then(|bytes| bytes.checked_add(binary_bytes))
        .ok_or_else(|| "地形GLB文件长度溢出".to_owned())?; // 头、两个块头和全部载荷。
    let total_length_u32 = u32::try_from(total_length)
        .map_err(|_| "单个地形GLB超过4GiB格式长度；需要分块地形".to_owned())?; // glTF 2.0长度字段为U32。
    let json_length_u32 =
        u32::try_from(json_bytes.len()).map_err(|_| "地形GLB JSON块超过U32长度".to_owned())?; // GLB块头必须可表示JSON长度。
    let mut glb = Vec::new(); // 直接申请最终文件缓冲，不再复制一整份临时BIN块。
    glb.try_reserve_exact(total_length)
        .map_err(|error| format!("申请地形GLB文件内存失败（需要{total_length}字节）：{error}"))?; // 保存失败返回明确诊断。
    glb.extend_from_slice(&GLB_MAGIC.to_le_bytes()); // 写入GLB魔数。
    glb.extend_from_slice(&GLB_VERSION.to_le_bytes()); // 写入格式版本。
    glb.extend_from_slice(&total_length_u32.to_le_bytes()); // 写入完整文件长度。
    glb.extend_from_slice(&json_length_u32.to_le_bytes()); // 写入JSON块长度。
    glb.extend_from_slice(&JSON_CHUNK_TYPE.to_le_bytes()); // 写入JSON块类型。
    glb.extend_from_slice(&json_bytes); // 写入完整JSON文档。
    glb.extend_from_slice(&binary_length_u32.to_le_bytes()); // 写入BIN块长度。
    glb.extend_from_slice(&BIN_CHUNK_TYPE.to_le_bytes()); // 写入BIN块类型。
    push_vec3_slice(&mut glb, &mesh.positions); // 按原顶点顺序直接写入最终GLB的POSITION段。
    push_vec3_slice(&mut glb, &normals); // 写入重算的每顶点NORMAL段。
    push_u32_slice(&mut glb, &mesh.indices); // 写入原三角顺序和连接关系。
    debug_assert_eq!(glb.len(), total_length); // 三段载荷均为四字节整数倍，不需要BIN末尾补齐。
    Ok(glb) // 返回完整内存GLB供原子文件事务发布。
} // 结束白模GLB编码。

/// 平地直写与通用网格保存共用同一GLB文档，避免两条生成路径产生不同格式。
fn white_terrain_document(
    vertex_count: usize,   // POSITION与NORMAL accessor的元素数。
    index_count: usize,    // 索引accessor的元素数。
    position_bytes: usize, // POSITION bufferView的字节数。
    normal_bytes: usize,   // NORMAL bufferView的字节数。
    index_bytes: usize,    // 索引bufferView的字节数。
    binary_bytes: usize,   // 唯一内嵌BIN块的总字节数。
    min: [f32; 3],         // POSITION accessor的世界包围盒下界。
    max: [f32; 3],         // POSITION accessor的世界包围盒上界。
) -> Value {
    let normals_offset = position_bytes; // NORMAL紧随POSITION数据。
    let indices_offset = position_bytes + normal_bytes; // 索引紧随NORMAL数据。
    json!({
        "asset": { "version": "2.0", "generator": "rpg_editor_map" },
        "scene": 0,
        "scenes": [{ "name": "动态地图白模场景", "nodes": [0] }],
        "nodes": [{ "name": "动态地图地形", "mesh": 0 }],
        "meshes": [{
            "name": "动态地图地形_网格",
            "primitives": [{
                "attributes": { "POSITION": 0, "NORMAL": 1 },
                "indices": 2,
                "material": 0,
                "mode": 4
            }]
        }],
        "materials": [{
            "name": "地表_白模",
            "pbrMetallicRoughness": {
                "baseColorFactor": [1.0, 1.0, 1.0, 1.0],
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0
            },
            "doubleSided": false
        }],
        "buffers": [{ "byteLength": binary_bytes }],
        "bufferViews": [
            { "buffer": 0, "byteOffset": 0, "byteLength": position_bytes, "target": 34962 },
            { "buffer": 0, "byteOffset": normals_offset, "byteLength": normal_bytes, "target": 34962 },
            { "buffer": 0, "byteOffset": indices_offset, "byteLength": index_bytes, "target": 34963 }
        ],
        "accessors": [
            {
                "bufferView": 0, "componentType": 5126, "count": vertex_count,
                "type": "VEC3", "min": min, "max": max
            },
            { "bufferView": 1, "componentType": 5126, "count": vertex_count, "type": "VEC3" },
            { "bufferView": 2, "componentType": 5125, "count": index_count, "type": "SCALAR" }
        ]
    }) // 完成单场景、单网格、单primitive、单白材质文档。
} // 结束共享GLB文档构造。

/// 验证GLB头并返回JSON文档与唯一内嵌BIN块。
pub(super) fn parse_glb_chunks(bytes: &[u8]) -> Result<(Value, &[u8]), String> {
    if bytes.len() < 20 || read_u32(bytes, 0)? != GLB_MAGIC || read_u32(bytes, 4)? != GLB_VERSION {
        return Err("terrain.glb不是有效的glTF 2.0二进制文件".to_owned()); // 头部不合法时不继续按任意偏移读取。
    } // 完成魔数和版本门。
    let declared_length = read_u32(bytes, 8)? as usize; // GLB头声明完整文件长度。
    if declared_length != bytes.len() {
        return Err(format!(
            "terrain.glb声明长度{declared_length}与实际{}不一致",
            bytes.len()
        ));
    } // 禁止截断或尾随未知数据。
    let mut offset = 12usize; // 第一个块紧随十二字节头。
    let mut json_bytes = None; // GLB必须恰有可解析JSON块。
    let mut binary = None; // 动态地形只接受内嵌BIN数据。
    while offset < bytes.len() {
        let chunk_length = read_u32(bytes, offset)? as usize; // 先读块载荷长度。
        let chunk_type = read_u32(bytes, offset + 4)?; // 再读四字节块类型。
        let start = offset
            .checked_add(8)
            .ok_or_else(|| "GLB块偏移溢出".to_owned())?; // 防止损坏长度回绕。
        let end = start
            .checked_add(chunk_length)
            .ok_or_else(|| "GLB块长度溢出".to_owned())?; // 防止载荷尾回绕。
        let chunk = bytes
            .get(start..end)
            .ok_or_else(|| "GLB块超出文件边界".to_owned())?; // 完整载荷必须存在。
        match chunk_type {
            JSON_CHUNK_TYPE if json_bytes.is_none() => json_bytes = Some(chunk), // 第一JSON块成为文档。
            BIN_CHUNK_TYPE if binary.is_none() => binary = Some(chunk), // 第一BIN块成为buffer 0。
            JSON_CHUNK_TYPE | BIN_CHUNK_TYPE => return Err("GLB包含重复JSON或BIN块".to_owned()), // 动态地形不接受模糊多块映射。
            _ => {} // 合法扩展块不影响标准地形访问器。
        } // 完成块分类。
        offset = end; // 进入下一块头。
    } // 完成全部块遍历。
    if offset != bytes.len() {
        return Err("GLB块没有精确结束于文件边界".to_owned()); // 防御非整块尾数据。
    } // 完成块终点门。
    let json_bytes = json_bytes.ok_or_else(|| "GLB缺少JSON块".to_owned())?; // JSON文档是glTF结构权威。
    let json_end = json_bytes
        .iter()
        .rposition(|byte| !matches!(*byte, b' ' | 0))
        .map_or(0, |index| index + 1); // 去掉GLB允许的空格或NUL对齐。
    let document = serde_json::from_slice(&json_bytes[..json_end])
        .map_err(|error| format!("解析GLB JSON块失败：{error}"))?; // 文档必须完整可解析。
    let binary = binary.ok_or_else(|| "GLB缺少内嵌BIN块".to_owned())?; // F12不追踪外部bin文件。
    validate_embedded_buffer(&document, binary.len())?; // buffer 0必须对应当前BIN块且没有外部URI。
    Ok((document, binary)) // 返回借用文件载荷，不复制整份GLB。
} // 结束GLB块解析。

/// 验证动态地形只使用GLB内嵌buffer 0。
fn validate_embedded_buffer(document: &Value, binary_length: usize) -> Result<(), String> {
    let buffers = json_array(document, "buffers")?; // 所有accessor最终必须映射到buffer表。
    let [buffer] = buffers else {
        return Err(format!(
            "动态地形GLB必须只有一个内嵌buffer，当前为{}个",
            buffers.len()
        ));
    }; // 外部或多buffer资产不属于单文件保存契约。
    if buffer.get("uri").is_some() {
        return Err("动态地形GLB不得引用外部buffer URI".to_owned()); // terrain.glb必须是唯一持久化地形文件。
    } // 完成外部URI门。
    let declared = buffer
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or_else(|| "GLB buffer缺少byteLength".to_owned())? as usize; // 标准buffer长度必需。
    if declared > binary_length || binary_length.saturating_sub(declared) > 3 {
        return Err(format!(
            "GLB BIN长度{binary_length}与buffer声明{declared}不一致"
        ));
    } // BIN最多只允许四字节对齐产生的三个尾字节。
    Ok(()) // 完成内嵌buffer验证。
} // 结束buffer门。

/// 验证唯一primitive引用稳定白模材质名称。
fn validate_white_material(document: &Value, primitive: &Value) -> Result<(), String> {
    let material_index = primitive
        .get("material")
        .and_then(Value::as_u64)
        .ok_or_else(|| "动态地形primitive缺少白模材质".to_owned())?
        as usize; // 材质身份是防误接管门。
    let material = json_array(document, "materials")?
        .get(material_index)
        .ok_or_else(|| "动态地形材质索引越界".to_owned())?; // 索引必须存在。
    if material.get("name").and_then(Value::as_str) != Some(DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME) {
        return Err(format!(
            "动态地形primitive材质必须命名为{DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME}"
        ));
    } // 完成白模名称门。
    Ok(()) // 其他材质参数不属于F12读取契约。
} // 结束材质验证。

/// 在默认场景层级中查找唯一地形节点并组合其世界变换。
fn unique_terrain_node_transform(document: &Value, mesh_index: usize) -> Result<Mat4, String> {
    let scenes = json_array(document, "scenes")?; // 只遍历默认场景可达节点。
    let scene_index = document.get("scene").and_then(Value::as_u64).unwrap_or(0) as usize; // 未显式指定时按glTF惯例取首场景。
    let scene = scenes
        .get(scene_index)
        .ok_or_else(|| "GLB默认scene索引越界".to_owned())?; // 场景必须存在。
    let roots = scene
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| "GLB默认scene缺少根nodes".to_owned())?; // 空场景不能覆盖地图范围。
    let nodes = json_array(document, "nodes")?; // 场景层级引用统一节点表。
    let mut matches = Vec::new(); // 只保留地形Mesh实例的完整世界矩阵。
    let mut path = Vec::new(); // 当前递归路径检测非法节点环。
    for root in roots {
        let root = root
            .as_u64()
            .ok_or_else(|| "scene根节点索引不是整数".to_owned())? as usize; // 根引用必须是索引。
        visit_node(
            nodes,
            root,
            Mat4::IDENTITY,
            mesh_index,
            &mut path,
            &mut matches,
        )?; // 递归组合父子变换。
    } // 完成全部场景根遍历。
    let [transform] = matches.as_slice() else {
        return Err(format!(
            "默认场景必须且只能实例化一次动态地形Mesh，当前为{}次",
            matches.len()
        ));
    }; // 同一作者Mesh多实例会重复整份地形身份，仍明确拒绝；洞穴应直接建在这一个primitive内。
    Ok(*transform) // 返回地形节点完整世界矩阵。
} // 结束地形节点查找。

/// 深度优先遍历一个glTF节点并收集目标Mesh世界矩阵。
fn visit_node(
    nodes: &[Value],
    node_index: usize,
    parent: Mat4,
    target_mesh: usize,
    path: &mut Vec<usize>,
    matches: &mut Vec<Mat4>,
) -> Result<(), String> {
    if path.contains(&node_index) {
        return Err("GLB节点层级包含循环引用".to_owned()); // 非树结构不能定义稳定世界变换。
    } // 完成环检测。
    let node = nodes
        .get(node_index)
        .ok_or_else(|| "GLB节点索引越界".to_owned())?; // 每个引用必须存在。
    path.push(node_index); // 进入当前递归路径。
    let world = parent * node_local_transform(node)?; // glTF列向量语义为父矩阵乘局部矩阵。
    if node.get("mesh").and_then(Value::as_u64) == Some(target_mesh as u64) {
        matches.push(world); // 目标实例数量稍后统一验证为一。
    } // 完成当前节点Mesh匹配。
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            let child = child
                .as_u64()
                .ok_or_else(|| "GLB子节点索引不是整数".to_owned())?
                as usize; // 子引用必须是索引。
            visit_node(nodes, child, world, target_mesh, path, matches)?; // 继续累积完整层级变换。
        } // 完成全部子节点遍历。
    } // 完成可选children处理。
    path.pop(); // 离开当前递归路径。
    Ok(()) // 完成一个节点子树。
} // 结束节点遍历。

/// 读取glTF节点matrix或TRS并构造局部矩阵。
fn node_local_transform(node: &Value) -> Result<Mat4, String> {
    if let Some(matrix) = node.get("matrix").and_then(Value::as_array) {
        if matrix.len() != 16 {
            return Err("GLB节点matrix必须包含16个数值".to_owned()); // Mat4长度必须精确。
        } // 完成矩阵长度门。
        let mut columns = [0.0f32; 16]; // glTF matrix按列主序存储。
        for (target, value) in columns.iter_mut().zip(matrix) {
            *target = json_f32(value, "节点matrix")?; // 非有限或超出f32范围明确拒绝。
        } // 完成十六分量读取。
        return Ok(Mat4::from_cols_array(&columns)); // 直接沿用glTF列主序。
    } // matrix存在时按规范忽略TRS。
    let translation = json_vec(
        node.get("translation"),
        3,
        &[0.0, 0.0, 0.0],
        "节点translation",
    )?; // 默认零平移。
    let rotation = json_vec(
        node.get("rotation"),
        4,
        &[0.0, 0.0, 0.0, 1.0],
        "节点rotation",
    )?; // glTF四元数顺序为xyzw。
    let scale = json_vec(node.get("scale"), 3, &[1.0, 1.0, 1.0], "节点scale")?; // 默认单位缩放。
    let rotation = Quat::from_xyzw(rotation[0], rotation[1], rotation[2], rotation[3]); // 构造glam四元数。
    if !rotation.is_finite() || rotation.length_squared() <= f32::EPSILON {
        return Err("GLB节点rotation不是有限非零四元数".to_owned()); // 无效旋转不得传播NaN。
    } // 完成四元数门。
    Ok(Mat4::from_scale_rotation_translation(
        Vec3::new(scale[0], scale[1], scale[2]), // 缩放先作用于局部位置。
        rotation.normalize(),                    // glTF作者误差允许归一化，但不允许零四元数。
        Vec3::new(translation[0], translation[1], translation[2]), // 平移最后进入矩阵。
    )) // 返回标准TRS局部矩阵。
} // 结束节点变换读取。

/// 读取Float32 VEC3 POSITION并应用唯一节点世界矩阵。
fn read_positions(
    accessor: &Value,
    document: &Value,
    binary: &[u8],
    transform: Mat4,
) -> Result<Vec<[f32; 3]>, String> {
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || accessor.get("type").and_then(Value::as_str) != Some("VEC3")
        || accessor.get("sparse").is_some()
    {
        return Err("动态地形POSITION必须是非稀疏Float32 VEC3".to_owned()); // 不隐式扩展稀疏或量化位置。
    } // 完成POSITION格式门。
    let count = accessor_count(accessor)?; // 顶点数后续仍经过共享非空、引用关系与有限值校验。
    let (start, stride, element_size) = accessor_layout(accessor, document, binary, 12)?; // 三个f32固定十二字节。
    debug_assert_eq!(element_size, 12); // 调用方传入的元素宽度固定。
    let mut positions = Vec::with_capacity(count); // 一次分配目标世界位置数组。
    for index in 0..count {
        let offset = start + index * stride; // byteStride允许读取Blender交错顶点buffer。
        let x = read_f32(binary, offset)?; // 读取局部X。
        let y = read_f32(binary, offset + 4)?; // 读取局部Y。
        let z = read_f32(binary, offset + 8)?; // 读取局部Z。
        let world = transform.transform_point3(Vec3::new(x, y, z)); // 烘焙完整节点层级变换。
        positions.push(world.to_array()); // 保持accessor原顶点顺序。
    } // 完成全部位置读取。
    Ok(positions) // 返回世界空间位置。
} // 结束POSITION读取。

/// 读取SCALAR索引并统一为U32。
fn read_indices(accessor: &Value, document: &Value, binary: &[u8]) -> Result<Vec<u32>, String> {
    if accessor.get("type").and_then(Value::as_str) != Some("SCALAR")
        || accessor.get("sparse").is_some()
    {
        return Err("动态地形索引必须是非稀疏SCALAR".to_owned()); // 不接受稀疏索引。
    } // 完成索引类型门。
    let component = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| "索引accessor缺少componentType".to_owned())?; // 索引分量类型必需。
    let element_size = match component {
        5121 => 1usize, // UNSIGNED_BYTE。
        5123 => 2usize, // UNSIGNED_SHORT。
        5125 => 4usize, // UNSIGNED_INT。
        _ => return Err(format!("动态地形索引componentType {component}不受支持")),
    }; // 完成索引宽度选择。
    let count = accessor_count(accessor)?; // 逻辑索引数。
    let (start, stride, _) = accessor_layout(accessor, document, binary, element_size)?; // 支持规范允许的stride。
    let mut indices = Vec::with_capacity(count); // 一次分配统一U32表。
    for index in 0..count {
        let offset = start + index * stride; // 进入当前逻辑标量。
        let value = match component {
            5121 => *binary.get(offset).ok_or_else(|| "U8索引越界".to_owned())? as u32,
            5123 => read_u16(binary, offset)? as u32,
            5125 => read_u32(binary, offset)?,
            _ => unreachable!("索引分量已在上方验证"),
        }; // 完成当前索引读取。
        indices.push(value); // 保持原三角顺序。
    } // 完成全部索引读取。
    Ok(indices) // 返回统一索引表。
} // 结束索引读取。

/// 解析accessor、bufferView偏移和步长，并验证最后一个元素完整落在BIN内。
fn accessor_layout(
    accessor: &Value,
    document: &Value,
    binary: &[u8],
    element_size: usize,
) -> Result<(usize, usize, usize), String> {
    let view_index = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| "动态地形accessor缺少bufferView".to_owned())? as usize; // GLB地形不接受隐式零填充accessor。
    let view = json_array(document, "bufferViews")?
        .get(view_index)
        .ok_or_else(|| "动态地形bufferView索引越界".to_owned())?; // 视图必须存在。
    if view.get("buffer").and_then(Value::as_u64) != Some(0) {
        return Err("动态地形bufferView必须引用内嵌buffer 0".to_owned()); // 单文件契约只允许buffer 0。
    } // 完成buffer引用门。
    let view_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize; // 视图默认从buffer首字节开始。
    let accessor_offset = accessor
        .get("byteOffset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize; // accessor默认从视图首字节开始。
    let start = view_offset
        .checked_add(accessor_offset)
        .ok_or_else(|| "accessor字节偏移溢出".to_owned())?; // 合并两层偏移。
    let stride = view
        .get("byteStride")
        .and_then(Value::as_u64)
        .map_or(element_size, |value| value as usize); // 非交错视图使用紧密元素宽度。
    if stride < element_size {
        return Err("bufferView byteStride小于accessor元素宽度".to_owned()); // 交错步长不能覆盖当前元素。
    } // 完成步长门。
    let count = accessor_count(accessor)?; // 验证完整范围需要逻辑元素数。
    let required_end = if count == 0 {
        start // 空accessor不读取任何元素，后续算法会拒绝空地形。
    } else {
        start
            .checked_add(
                (count - 1)
                    .checked_mul(stride)
                    .ok_or_else(|| "accessor步长乘法溢出".to_owned())?,
            )
            .and_then(|offset| offset.checked_add(element_size))
            .ok_or_else(|| "accessor末端偏移溢出".to_owned())? // 最后元素末端必须完整存在。
    }; // 完成所需末端计算。
    let view_length = view
        .get("byteLength")
        .and_then(Value::as_u64)
        .ok_or_else(|| "bufferView缺少byteLength".to_owned())? as usize; // 视图长度必需。
    let view_end = view_offset
        .checked_add(view_length)
        .ok_or_else(|| "bufferView末端溢出".to_owned())?; // 计算视图边界。
    if required_end > view_end || required_end > binary.len() {
        return Err("动态地形accessor数据超出bufferView或BIN边界".to_owned()); // 禁止跨视图读取邻接数据。
    } // 完成范围门。
    Ok((start, stride, element_size)) // 返回紧凑读取布局。
} // 结束accessor布局解析。

/// 读取accessor count并转换为主机usize。
fn accessor_count(accessor: &Value) -> Result<usize, String> {
    usize::try_from(
        accessor
            .get("count")
            .and_then(Value::as_u64)
            .ok_or_else(|| "accessor缺少count".to_owned())?,
    )
    .map_err(|_| "accessor count超过主机usize".to_owned()) // 极端损坏值不截断。
} // 结束accessor数量读取。

/// 取得文档中的必需JSON数组。
fn json_array<'a>(document: &'a Value, key: &str) -> Result<&'a [Value], String> {
    document
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("GLB JSON缺少数组{key}")) // 调用方取得统一切片接口。
} // 结束JSON数组读取。

/// 读取可选固定长度浮点数组，缺席时复制默认值。
fn json_vec(
    value: Option<&Value>,
    length: usize,
    default: &[f32],
    label: &str,
) -> Result<Vec<f32>, String> {
    let Some(values) = value else {
        return Ok(default.to_vec()); // 缺席TRS属性使用规范默认值。
    }; // 完成缺席分支。
    let values = values
        .as_array()
        .ok_or_else(|| format!("{label}必须是数组"))?; // 属性必须为JSON数组。
    if values.len() != length {
        return Err(format!("{label}必须包含{length}个数值")); // 长度精确匹配Vec3或Quat。
    } // 完成长度门。
    values.iter().map(|value| json_f32(value, label)).collect() // 逐项读取有限f32。
} // 结束浮点数组读取。

/// 读取一个有限且可表示为f32的JSON数值。
fn json_f32(value: &Value, label: &str) -> Result<f32, String> {
    let value = value.as_f64().ok_or_else(|| format!("{label}包含非数值"))? as f32; // serde_json数值先转f64再收窄。
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| format!("{label}包含非有限或超范围数值")) // 防止无穷进入矩阵。
} // 结束JSON浮点读取。

/// 按小端读取U16并检查边界。
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "读取U16超出GLB边界".to_owned())?; // 两字节必须完整。
    Ok(u16::from_le_bytes([value[0], value[1]])) // 返回小端值。
} // 结束U16读取。

/// 按小端读取U32并检查边界。
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "读取U32超出GLB边界".to_owned())?; // 四字节必须完整。
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]])) // 返回小端值。
} // 结束U32读取。

/// 按小端读取Float32并检查边界。
fn read_f32(bytes: &[u8], offset: usize) -> Result<f32, String> {
    let value = read_u32(bytes, offset)?; // Float32与U32共享四字节边界读取。
    Ok(f32::from_bits(value)) // 有限值由共享地形验证统一报告具体顶点。
} // 结束Float32读取。

/// 按包含端点的规则网格取得一个轴的世界采样值。
fn sample_axis(min: f32, max: f32, index: u32, count: u32) -> f32 {
    if index + 1 == count {
        max // 最后一个顶点显式使用清单原上界。
    } else {
        min + (max - min) * index as f32 / (count - 1) as f32 // 其余顶点等距插值。
    } // 完成规则轴采样。
} // 结束规则轴采样。

/// 按原数组顺序写入小端Float32x3。
fn push_vec3_slice(target: &mut Vec<u8>, values: &[[f32; 3]]) {
    for vector in values {
        for value in vector {
            target.extend_from_slice(&value.to_le_bytes()); // 每个浮点使用glTF小端字节序。
        } // 完成当前向量三分量写入。
    } // 完成全部向量写入。
} // 结束Vec3编码。

/// 按原数组顺序写入小端U32索引。
fn push_u32_slice(target: &mut Vec<u8>, values: &[u32]) {
    for value in values {
        target.extend_from_slice(&value.to_le_bytes()); // 保持三角顺序和连接关系完全不变。
    } // 完成全部索引写入。
} // 结束U32编码。

/// 把GLB块补齐到四字节边界。
fn pad_to_four(bytes: &mut Vec<u8>, fill: u8) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(fill); // 每次最多补三个字节。
    } // 完成四字节对齐。
} // 结束块对齐。

#[cfg(test)]
mod tests {
    use super::*; // 直接验证新图和最小GLB协议。

    #[test]
    fn flat_grid_has_no_artificial_axis_limit() {
        let mut spec = TerrainSampleSpec {
            resolution: [2049, 2],    // X轴2048格超过旧限制，但完整GLB远低于格式容量。
            world_min: [0.0, 0.0],    // 有限且严格递增的地图范围。
            world_max: [2048.0, 1.0], // X轴每格一米，便于核对末端顶点。
            base_height: -1.0,        // 零高度位于可编辑量程内。
            height_scale: 2.0,        // 量程上界为正一米。
        }; // 狭长网格验证单轴上限解除，避免测试分配百万顶点。
        let mesh = build_flat_terrain_mesh(spec, 0.0).expect("2048格单轴应可创建GLB网格"); // 验证旧1024门槛已移除。
        assert_eq!(mesh.positions.len(), 2049 * 2); // 两行均包含地图边界端点。
        assert_eq!(mesh.indices.len(), 2048 * 6); // 每格仍含两个朝上三角形。
        assert_eq!(mesh.positions[2048], [2048.0, 0.0, 0.0]); // 最末X端点不因高细分丢失。
        spec.resolution = [0, 2]; // 零宽也必须由独立规格门拒绝。
        assert!(build_flat_terrain_mesh(spec, 0.0).is_err()); // 防止减一计算下溢。
        spec.resolution = [2, 2]; // 恢复最小网格，单独检验世界范围。
        spec.world_max[0] = spec.world_min[0]; // 倒置或零跨度不能参与顶点插值。
        assert!(build_flat_terrain_mesh(spec, 0.0).is_err()); // 平面仍遵守共享世界范围契约。
    }

    #[test]
    fn flat_glb_preflight_uses_format_capacity_without_allocating_the_grid() {
        assert!(validate_flat_terrain_glb_resolution([4569, 4737]).is_ok()); // 截图571×592米、0.125米格距约10亿字节GLB可以预检通过。
        let layout = flat_glb_layout([4569, 4737]).expect("截图参数应小于GLB文件格式上限"); // 只计算整数和小型JSON，不分配大网格。
        assert_eq!(layout.vertex_count, 21_643_353); // 格点包含两轴末端。
        assert_eq!(layout.binary_bytes, 1_038_657_624); // 位置、法线和索引合计约991MiB。
        assert!(
            validate_flat_terrain_glb_resolution([65_537, 65_537])
                .unwrap_err()
                .contains("U32索引")
        ); // 极端整图超出单primitive索引容量。
        assert!(
            validate_flat_terrain_glb_resolution([20_000, 20_000])
                .unwrap_err()
                .contains("4GiB")
        ); // 顶点索引尚可表示时由真实文件字节长度拒绝。
    }

    #[test]
    fn direct_flat_glb_matches_generic_mesh_encoding() {
        let spec = TerrainSampleSpec {
            resolution: [7, 5],      // 用小非方形网格比较完整文件，不消耗大地图内存。
            world_min: [-3.0, -2.0], // 两轴居中且步长为一米。
            world_max: [3.0, 2.0],   // 两轴最后顶点与清单边界精确一致。
            base_height: -80.0,      // 与新地图默认高度量程一致。
            height_scale: 160.0,     // 零平面位于合法区间。
        };
        let direct = build_flat_terrain_glb(spec, 0.0).expect("直写平地GLB应成功"); // 只使用最终文件缓冲。
        let generic_mesh = build_flat_terrain_mesh(spec, 0.0).expect("通用网格应成功"); // 原路径作为字节级基线。
        let generic = build_white_terrain_glb(&generic_mesh).expect("通用网格保存应成功"); // 法线和索引须相同。
        assert_eq!(direct, generic); // 两条入口产生完全相同的JSON、位置、法线和拓扑。
        assert_eq!(parse_dynamic_terrain_glb(&direct).unwrap(), generic_mesh); // 正式解析器能够读取直写文件。
    }

    fn rewrite_glb_document(bytes: &[u8], edit: impl FnOnce(&mut Value)) -> Vec<u8> {
        let (mut document, binary) = parse_glb_chunks(bytes).expect("测试GLB必须可读取块");
        edit(&mut document); // 只改结构JSON，BIN保持逐字节不变。
        let mut json_bytes = serde_json::to_vec(&document).expect("测试GLB JSON必须可序列化");
        pad_to_four(&mut json_bytes, b' ');
        let total_length = 12 + 8 + json_bytes.len() + 8 + binary.len();
        let mut rebuilt = Vec::with_capacity(total_length);
        rebuilt.extend_from_slice(&GLB_MAGIC.to_le_bytes());
        rebuilt.extend_from_slice(&GLB_VERSION.to_le_bytes());
        rebuilt.extend_from_slice(&(total_length as u32).to_le_bytes());
        rebuilt.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        rebuilt.extend_from_slice(&JSON_CHUNK_TYPE.to_le_bytes());
        rebuilt.extend_from_slice(&json_bytes);
        rebuilt.extend_from_slice(&(binary.len() as u32).to_le_bytes());
        rebuilt.extend_from_slice(&BIN_CHUNK_TYPE.to_le_bytes());
        rebuilt.extend_from_slice(binary);
        rebuilt
    }

    #[test]
    fn glb_preserves_arbitrary_vertex_and_index_order() {
        let mesh = TerrainMeshInput {
            positions: vec![
                [-2.0, 0.0, -1.0],
                [2.0, 0.0, -1.0],
                [-2.0, 1.0, 1.0],
                [2.0, 1.0, 1.0],
            ], // 非规则高度四点。
            indices: vec![0, 2, 1, 1, 2, 3], // 明确连接关系。
        }; // 完成任意拓扑夹具。
        let glb = build_white_terrain_glb(&mesh).expect("合法任意拓扑必须可编码"); // 执行正式保存编码。
        assert_eq!(&glb[0..4], b"glTF"); // 文件必须拥有GLB魔数。
        let text = String::from_utf8_lossy(&glb); // JSON块足以验证公开名称和属性契约。
        assert!(text.contains("动态地图地形_网格")); // 保留运行时唯一网格身份。
        assert!(text.contains("地表_白模")); // 保留运行时唯一材质身份。
        assert!(!text.contains("TEXCOORD_0")); // F12不承诺保存UV或额外属性。
        let index_bytes = mesh
            .indices
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>(); // 构造原索引小端序列。
        assert!(
            glb.windows(index_bytes.len())
                .any(|window| window == index_bytes)
        ); // GLB中必须原样保留索引顺序。
    } // 结束GLB拓扑回归。

    #[test]
    fn parser_applies_complete_parent_and_child_node_transform() {
        let source = TerrainMeshInput {
            positions: vec![[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [-1.0, 0.0, 1.0]],
            indices: vec![0, 2, 1],
        };
        let glb = build_white_terrain_glb(&source).expect("测试GLB必须可编码");
        let transformed = rewrite_glb_document(&glb, |document| {
            document["scenes"][0]["nodes"] = json!([0]);
            document["nodes"] = json!([
                {
                    "name": "父节点",
                    "translation": [10.0, 2.0, -3.0],
                    "children": [1]
                },
                {
                    "name": "动态地图地形",
                    "mesh": 0,
                    "scale": [2.0, 1.0, 3.0]
                }
            ]);
        });
        let parsed = parse_dynamic_terrain_glb(&transformed).expect("合法父子TRS必须被烘焙");
        assert_eq!(parsed.positions[0], [8.0, 2.0, -6.0]); // 先子缩放，再父平移。
        assert_eq!(parsed.positions[1], [12.0, 2.0, -6.0]);
        assert_eq!(parsed.positions[2], [8.0, 2.0, 0.0]);
        assert_eq!(parsed.indices, source.indices); // 节点变换绝不能重排拓扑。
    }

}
