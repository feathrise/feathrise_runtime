// 已采用 Clipmap 高度采样；只共享地形驻留页，禁止整图高度或逐株 CPU 数据。

@group(0) @binding(10) var blade_clipmap_atlas: texture_2d_array<f32>; // 原地复用 R16 独立页数组。
@group(0) @binding(11) var blade_clipmap_table: texture_2d<u32>; // 紧凑整数页表。

fn clipmap_center(level: u32) -> vec2<f32> { // 与地形 stable_world_xz 使用相同的全局观察中心吸附。
    let step = blade_clipmap.grid.xy * exp2(f32(level)); // 每级按自身格距吸附。
    let lo = blade_clipmap.map_bounds.xy; // 世界网格原点。
    return lo + floor((blade_clipmap.view.xy - lo) / step + vec2<f32>(0.5)) * step; // 粗级固定格点，中心可在源外。
} // 完成固定时间的层中心计算。

fn clipmap_vertex_height(vertex: vec3<f32>) -> vec2<f32> { // xy 为真实世界 X/Z，z 为该顶点的几何采样级。
    let p = vertex.xy; // 只对已选中的三角面三个顶点取高。
    if any(p < blade_clipmap.map_bounds.xy) || any(p > blade_clipmap.map_bounds.zw) { return vec2<f32>(0.0, 1.0); } // 保持地形地图外顶点的零高度规则。
    let boundary=stitch_boundary_height(p,blade_clipmap.map_bounds);if boundary.y>0.5{return boundary;}
    let level = u32(min(vertex.z, blade_clipmap.options.x)); // 父页不足时与地形同样钳到最高页级。
    let scale = exp2(f32(level)); // 逻辑父页格距。
    let logical = clamp((p - blade_clipmap.map_bounds.xy) / blade_clipmap.grid.xy, vec2<f32>(0.0), blade_clipmap.grid.zw) / scale; // 作者格坐标。
    let counts = vec2<u32>(ceil(blade_clipmap.grid.zw / (256.0 * scale))); // 每级实际页数。
    var offset = 0u; // 各级按 Z 页数紧凑纵向排列。
    for (var i = 0u; i < level; i += 1u) { offset += u32(ceil(blade_clipmap.grid.w / (256.0 * exp2(f32(i))))); } // 与地形页表偏移相同。
    let tile = min(vec2<u32>(floor(logical / 256.0)), counts - vec2<u32>(1u)); // 最远端点归最后一页。
    let key = (tile.x | (tile.y << 16u)) + 1u; // 零保留为空槽，实际tile坐标不受图集layer重排影响。
    if any(blade_clipmap.missing_pages[level] == vec4<u32>(key)) { return vec2<f32>(0.0); } // 仅拒绝依赖缺失页的三角，不让新页请求清空整片草。
    let entry = textureLoad(blade_clipmap_table, vec2<i32>(i32(tile.x), i32(tile.y + offset)), 0); // 同一完整页表。
    if entry.x == 0u { return vec2<f32>(stitch_height(p,0.0,blade_clipmap.map_bounds), 1.0); }
    let size = vec2<i32>(textureDimensions(blade_clipmap_atlas, 0)); // 当前正式页一般为 257²。
    let local = (logical - vec2<f32>(tile) * 256.0) * vec2<f32>(size - vec2<i32>(1)) / 256.0; // 与地形线性采样的 texel 中心规则相同。
    let a = clamp(vec2<i32>(floor(local)), vec2<i32>(0), size - vec2<i32>(1)); // 左下整数 texel。
    let b = min(a + vec2<i32>(1), size - vec2<i32>(1)); // 同页 clamp-to-edge，不跨页过滤。
    let f = fract(local); // 中点等非整数顶点需要页内双线性。
    let layer = i32(entry.x - 1u); // 零值已在上面处理。
    let h00 = textureLoad(blade_clipmap_atlas, a, layer, 0).r; // 普通整格顶点只需一次读取。
    var raw = h00; // 整格快速路径避免多余纹理操作。
    if any(f > vec2<f32>(0.000001)) { // 扇形中心和不同父页格距需要精确双线性。
        let h10 = textureLoad(blade_clipmap_atlas, vec2<i32>(b.x, a.y), layer, 0).r; // 右下。
        let h01 = textureLoad(blade_clipmap_atlas, vec2<i32>(a.x, b.y), layer, 0).r; // 左上。
        let h11 = textureLoad(blade_clipmap_atlas, b, layer, 0).r; // 右上。
        raw = mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y); // 精确恢复地形顶点的线性纹理高度。
    } // 完成页内线性采样。
    return vec2<f32>(stitch_height(p,bitcast<f32>(entry.y) + raw * bitcast<f32>(entry.z),blade_clipmap.map_bounds), 1.0);
} // 完成单个地形顶点高度读取。

fn clipmap_vertex(local: vec2<f32>, level: u32, fine: bool) -> vec3<f32> { // 重建地形模板的实际 X/Z 和采样级。
    let sample_level = level - select(0u, 1u, fine && level > 0u); // 内缘顶点使用上一细层中心和 Mip。
    let step = blade_clipmap.grid.xy * exp2(f32(level)); // 局部位置仍使用当前模板缩放。
    return vec3<f32>(stitch_position(local * step + clipmap_center(sample_level),blade_clipmap.map_bounds), f32(sample_level));
} // 完成无高度读取的顶点重建。

fn clipmap_corner(local: vec2<f32>, level: u32, inner: vec2<i32>) -> vec3<f32> { // 常规整点和矩形洞口角点的 UV_1 标记。
    let inner_f = vec2<f32>(inner); // 当前级两轴独立内半径。
    let has_inner = all(inner > vec2<i32>(0)); // 首层没有接缝。
    let edge = (abs(local.x) == inner_f.x && abs(local.y) <= inner_f.y) || (abs(local.y) == inner_f.y && abs(local.x) <= inner_f.x); // 含内缘四个角。
    return clipmap_vertex(local, level, has_inner && edge); // 只有环内缘整点使用细级。
} // 完成模板角点标记。

fn clipmap_triangle(p: vec2<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> vec2<f32> { // 先判断 X/Z，选中后才读取三顶点高度。
    let ab = b.xy - a.xy; // 第一条边。
    let ac = c.xy - a.xy; // 第二条边。
    let ap = p - a.xy; // 查询点相对首顶点。
    let determinant = ab.x * ac.y - ab.y * ac.x; // 三角形有向面积。
    if abs(determinant) < 0.00000001 { return vec2<f32>(0.0); } // 独立吸附可能压扁内缘扇形，零面积面不参与取高。
    if stitches_active()&&determinant>=0.0{return vec2<f32>(0.0);} // 与正式地形背面剔除一致，边界收敛的翻面不能承托草根。
    let u = (ap.x * ac.y - ap.y * ac.x) / determinant; // b 的重心权重。
    let v = (ab.x * ap.y - ab.y * ap.x) / determinant; // c 的重心权重。
    if u < -0.000001 || v < -0.000001 || u + v > 1.000001 { return vec2<f32>(0.0); } // 未选中的三角不读取任何高度页。
    let ah = clipmap_vertex_height(a); // 命中面后才检查并采样真实三个顶点。
    let bh = clipmap_vertex_height(b); // 每个顶点按自身实际采样Mip查询固定四槽缺页掩码。
    let ch = clipmap_vertex_height(c); // 接缝顶点可使用不同细级Mip，不按草叶LOD寻址。
    if min(ah.y, min(bh.y, ch.y)) < 0.5 { return vec2<f32>(0.0); } // 三角依赖页不完整时只抑制该处草根，不回退平地。
    return vec2<f32>(ah.x * (1.0 - u - v) + bh.x * u + ch.x * v, 1.0); // 草根位于全部已驻留的真实三角面。
} // 完成三角面高度插值。

// Naga 29 为函数数组生成 Vulkan 不允许的 ArrayStride；显式四角选择仅规避后端编码，不改变扇形拓扑。
fn clipmap_polygon_corner(index: u32, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>, d: vec3<f32>) -> vec3<f32> { // 依赖修复后可恢复局部数组。
    switch index { // 索引固定为零至三。
        case 0u: { return a; } // 左下角。
        case 1u: { return b; } // 左上角。
        case 2u: { return c; } // 右上角。
        default: { return d; } // 右下角。
    } // 无函数地址空间数组，也不增加高度采样。
} // 完成四角选择兼容入口。
fn clipmap_highest(a:vec2<f32>,b:vec2<f32>)->vec2<f32> {
    if b.y>0.5&&(a.y<0.5||b.x>a.x){return b;}return a;
}
fn clipmap_cell(p: vec2<f32>, cell: vec2<i32>, band_index: u32) -> vec2<f32> { // 最多一个常规格或内缘五角扇形。
    var nearest=vec2<f32>(0.0);
    let limits = blade_clipmap.bands[band_index]; // xy内半径、zw外半径。
    let inner = vec2<i32>(limits.xy); // 环带中央矩形洞口。
    let outer = vec2<i32>(limits.zw); // 当前矩形有限边界。
    let level = u32(blade_clipmap.options.w) + band_index; // 编译计划保证相邻倍率严格2:1。
    if any(cell < -outer) || any(cell >= outer) { return vec2<f32>(0.0); } // 不存在的模板格。
    let in_x = cell.x >= -inner.x && cell.x < inner.x; // 洞口 X 范围。
    let in_z = cell.y >= -inner.y && cell.y < inner.y; // 洞口 Z 范围。
    let has_inner = all(inner > vec2<i32>(0)); // 首层是完整矩形。
    if has_inner && in_x && in_z { return vec2<f32>(0.0); } // 内洞由前一层覆盖。
    var side = -1; // 常规格默认两三角。
    if has_inner { // 仅环内缘使用五角扇形。
        if cell.x == inner.x && in_z { side = 0; } // 左边拆分。
        else if cell.y + 1 == -inner.y && in_x { side = 1; } // 上边拆分。
        else if cell.x + 1 == -inner.x && in_z { side = 2; } // 右边拆分。
        else if cell.y == inner.y && in_x { side = 3; } // 下边拆分。
    } // 完成与静态拓扑相同的内缘规则。
    let q = vec2<f32>(cell); // 单元左下局部坐标。
    let a = clipmap_corner(q, level, inner); // 左下角。
    let b = clipmap_corner(q + vec2<f32>(0.0, 1.0), level, inner); // 左上角。
    let c = clipmap_corner(q + vec2<f32>(1.0, 1.0), level, inner); // 右上角。
    let d = clipmap_corner(q + vec2<f32>(1.0, 0.0), level, inner); // 右下角。
    if side < 0 { // 大部分候选仅走两三角和三次高度采样。
        let first = clipmap_triangle(p, a, b, d); // 与模板 [a,c,b] 实际坐标绕序相同。
        if first.y > 0.5&&!stitches_active() { return first; } // 普通网格仍走首次命中的快路径。
        return clipmap_highest(first,clipmap_triangle(p, d, b, c));
    } // 完成常规格快速路径。
    var midpoint = vec2<f32>(0.0, 0.5); // Naga 29 Vulkan ArrayStride 兼容：避免只需四项的函数局部数组。
    switch side { // 四条可拆内边与原静态拓扑完全一致。
        case 1: { midpoint = vec2<f32>(0.5, 1.0); } // 上内边。
        case 2: { midpoint = vec2<f32>(1.0, 0.5); } // 右内边。
        case 3: { midpoint = vec2<f32>(0.5, 0.0); } // 下内边。
        default: {} // 默认左内边。
    } // 依赖修复后可撤销兼容展开，不改变任何候选散布规则。
    let seam = clipmap_vertex(q + midpoint, level, true); // 内边细格中点和细层同位同高。
    let center = clipmap_vertex(q + vec2<f32>(0.5), level, false); // 粗中心固定在当前层中心，不随细边滑动。
    for (var edge = 0u; edge < 4u; edge += 1u) { // 四条边中一条拆成两段，共五个真实三角。
        let next = (edge + 1u) % 4u; // 多边形闭合。
        if edge == u32(side) { // 内边包含额外细格中点。
            let first = clipmap_triangle(p, center, clipmap_polygon_corner(edge, a, b, c, d), seam); // 内缘第一扇形面。
            if first.y > 0.5 {if !stitches_active(){return first;}nearest=clipmap_highest(nearest,first);}
            let second = clipmap_triangle(p, center, seam, clipmap_polygon_corner(next, a, b, c, d)); // 内缘第二扇形面。
            if second.y > 0.5 {if !stitches_active(){return second;}nearest=clipmap_highest(nearest,second);}
        } else { // 其余三条完整边。
            let result = clipmap_triangle(p, center, clipmap_polygon_corner(edge, a, b, c, d), clipmap_polygon_corner(next, a, b, c, d)); // 普通扇形面。
            if result.y > 0.5 {if !stitches_active(){return result;}nearest=clipmap_highest(nearest,result);}
        } // 完成单边检查。
    } // 完成固定五角拓扑检查。
    return nearest; // 没有有效正面时保持禁草。
} // 完成有限单元查询。

fn inside_volume_footprint(p:vec2<f32>)->bool {
    for (var i=0u; i<min(u32(blade_clipmap.hole_info.x),32u); i+=1u) {
        let hole=blade_clipmap.holes[i];
        if all(p>hole.xy) && all(p<hole.zw) { return true; }
    }
    return false;
}
fn sample_clipmap_surface(p: vec2<f32>) -> vec2<f32> { // 根按实际位置交接，边界属于室外，避免两边同时退让。
    if inside_volume_footprint(p) {return vec2<f32>(0.0);}
    return sample_clipmap_envelope(p);
}
// 隐藏包络只确定跨界密度对应的表面层，绝不能替代洞内草根的真实三维承托。
fn sample_clipmap_envelope(p:vec2<f32>)->vec2<f32> {
    var nearest=vec2<f32>(0.0); // 收敛角落可有多个投影面，与地形深度缓冲同取最高有效正面。
    if any(p < blade_clipmap.map_bounds.xy) || any(p > blade_clipmap.map_bounds.zw) { return vec2<f32>(0.0); } // 地图外不生成草。
    let band_count = min(u32(blade_clipmap.options.z), 16u); // 作者编译硬门之外再防御损坏uniform。
    for (var band_index = 0u; band_index < band_count; band_index += 1u) { // 可变边界不能再由固定64半格公式反推级别。
        let level = u32(blade_clipmap.options.w) + band_index; // 相邻环带倍率逐级翻倍。
        let limits = blade_clipmap.bands[band_index]; // 当前两轴内外半径。
        let step = blade_clipmap.grid.xy * exp2(f32(level)); // 当前几何格距。
        let local = (p - clipmap_center(level)) / step; // 当前层普通网格局部坐标。
        if any(abs(local) > vec2<f32>(limits.zw)) && !stitches_active() { continue; }
        let cell = vec2<i32>(floor(local)); // 普通区域唯一可能单元。
        let direct = clipmap_cell(p, cell, band_index); // 非接缝只检查该格。
        if direct.y > 0.5 {if !stitches_active(){return direct;}if nearest.y<0.5||direct.x>nearest.x{nearest=direct;}}
        for (var z = -1; z <= 1; z += 1) { // 内缘中心偏移不超过半个粗格，最多九个邻格。
            for (var x = -1; x <= 1; x += 1) { // 不扫描完整环带或整图。
                if x == 0 && z == 0 { continue; } // 已检查中心格。
                let result = clipmap_cell(p, cell + vec2<i32>(x, z), band_index); // 包含角点变形及五角扇形。
                if result.y > 0.5 {if !stitches_active(){return result;}if nearest.y<0.5||result.x>nearest.x{nearest=result;}}
            } // 完成固定局部邻格 X 检查。
        } // 完成固定局部邻格 Z 检查。
        if stitches_active() {
            for(var candidate=1u;candidate<16u;candidate+=1u) {
                let source=stitch_candidate(p,blade_clipmap.map_bounds,step,clipmap_center(level),candidate);
                if (candidate%4u>0u&&abs(source.x-p.x)<0.000001)||(candidate/4u>0u&&abs(source.y-p.y)<0.000001){continue;}
                if all(abs(source-p)<vec2<f32>(0.000001)){continue;}
                let inverse_cell=vec2<i32>(floor((source-clipmap_center(level))/step));
                let inverse_direct=clipmap_cell(p,inverse_cell,band_index);
                if inverse_direct.y>0.5&&(nearest.y<0.5||inverse_direct.x>nearest.x){nearest=inverse_direct;}
                for(var dz=-1;dz<=1;dz+=1){for(var dx=-1;dx<=1;dx+=1){
                    if dx==0&&dz==0{continue;}
                    let result=clipmap_cell(p,inverse_cell+vec2<i32>(dx,dz),band_index);
                    if result.y>0.5&&(nearest.y<0.5||result.x>nearest.x){nearest=result;}
                }}
            }
        }
    } // 完成几何级定位。
    return nearest; // 静态 Clipmap 覆盖外仍不显示草。
} // 完成实际显示地形高度查询。

// 通用 GPU 草地候选分类与紧凑化计算着色器。
// 【功能设计】每个候选只执行一次距离、密度和高度判定，最终存活草按平方距离进入八个等面积径向桶。
// 【功能设计】每个工作组每个非空桶只执行一次全局预留，实例缓冲总容量不因分桶增加，绘制阶段按近到远消费八条间接命令。
// 【功能设计】同一文件提供分类压缩与间接参数收尾两个入口，Rust必须先清零动态计数、再分类、最后收尾。

struct GrassPatch { // 保持与主世界GpuBladePatch相同的416字节动态uniform布局。
    origin_extent: vec4<f32>, // xyz为候选窗口世界中心，w为正方形窗口边长。
    grid: vec4<u32>, // x为单边格数，y为候选总数，z为稳定种子，w最低位启用密度图，其余位保存几何LOD和行为标志。
    focus_radius: vec4<f32>, // xyz为连续玩家观察中心，w为本环完全消失半径。
    height_world: vec4<f32>, // xy/zw为高度单元纹理覆盖的世界X/Z最小角与最大角。
    height_decode: vec4<f32>, // x为最低高度，y为解码跨度，z非零时启用真实高度，w为全局激活覆盖率。
    density_world: vec4<f32>, // xy/zw为密度纹理覆盖的世界X/Z最小角与最大角。
    lod_density: vec4<f32>, // x为内半径，y/z为内外过渡宽度，w为非零密度概率增益。
    vegetation_bounds: vec4<f32>, // x为最高叶尖，y为水平扫掠半径，z为投影参考高度，w为根部下扎深度。
    density_control_world: vec4<f32>, // xy/zw为可选受控种植区最小角与最大角；零范围表示不追加裁剪。
    view_lod_pixel_thresholds: vec4<f32>, // 可选内容级远中与中近像素阈值；全零表示继承玩法视图。
    view_lod_ultra_fade: vec4<f32>, // xy为内容级超远转远阈值，zw为近镜头整株避让距离。
    view_lod_residency_radii: vec4<f32>, // xyzw为内容级前四档常驻半径，局部配置由档数元数据启用。
    view_lod_residency_transitions: vec4<f32>, // xyz为内容级子级回退宽度，w为共用安全边距。
    view_lod_pixel_thresholds_ext: array<vec4<f32>, 2>,
    view_lod_residency_radii_ext: vec4<f32>,
    view_lod_residency_transitions_ext: vec4<f32>,
    view_lod_metadata: vec4<f32>, // x为实际档数；零继承旧地图三/四档标志。
    source_to_world: mat4x4<f32>,
    world_to_source: mat4x4<f32>,
} // 完成与Rust ShaderType一致的候选窗口输入布局。

struct CompactGrassInstance { // 同时作为storage输出和实例顶点缓冲，五个32位字段的字节步长严格为20。
    root_world_x: f32, // 字节0：已经包含稳定抖动的草根世界X。
    surface_y: f32, // 字节4：按真实地形三角面插值得到的草根世界Y。
    root_world_z: f32, // 字节8：已经包含稳定抖动的草根世界Z。
    packed_appearance: u32, // 字节12：旋转、缩放、内容profile、覆盖份额和几何LOD。
    surface_normal: u32, // 字节16：snorm8法线XYZ及有效位；零值兼容室外默认朝上姿态。
} // 完成Float32x3加两个Uint32实例顶点格式。

struct GrassDrawState { // 每个近中远环各持有一份80字节聚合诊断状态。
    index_count: u32, // 字节0：原始草网格索引数量，由Rust在网格准备后写入。
    instance_count: u32, // 字节4：finalize_indirect汇总八桶后的安全紧凑实例数量。
    first_index: u32, // 字节8：MeshAllocator索引切片起点。
    base_vertex: i32, // 字节12：MeshAllocator顶点切片起点。
    first_instance: u32, // 字节16：聚合诊断保留位，当前固定为零。
    reserved_instances: atomic<u32>, // 字节20：收尾阶段汇总八桶原始预留总数。
    overflow_instances: atomic<u32>, // 字节24：收尾阶段汇总八桶超出分区容量的实例数。
    distance_survivors: atomic<u32>, // 字节28：通过玩法视锥与距离环带判定的候选数。
    density_survivors: atomic<u32>, // 字节32：继续通过密度图概率判定的候选数。
    surface_survivors: atomic<u32>, // 字节36：最终取得合法地表高度的候选数。
    capacity: u32, // 字节40：当前紧凑实例缓冲最多容纳的20字节记录数。
    candidate_count: u32, // 字节44：本次允许分类的候选上限，必须与窗口配置一致。
    dispatch_group_count: u32, // 字节48：GPU粗剔除后实际细分类工作组总数。
    lod_index: u32, // 字节52：零一二分别表示近中远，供回读和图形调试器识别。
    buffer_generation: u32, // 字节56：扩容或重建代次，防止旧回读误认成当前资源。
    total_tiles: u32, // 字节60：当前候选正方形切分后的全部十六乘十六瓦片数。
    active_tiles: u32, // 字节64：粗距离环带与保守视锥判定后进入细分类的瓦片数。
    scanned_candidates: u32, // 字节68：细分类实际启动的候选线程数，包含边缘瓦片的安全尾线程。
    vegetation_profile: u32, // 字节72：零普通草、一麦田等稳定内容编号供异步回读拆分。
    vertex_count: u32, // 字节76：当前GLB primitive真实顶点数供F3计算几何预算。
} // 完成计算与异步诊断共用状态布局。

struct GrassDepthBucketState { // 每条三十二字节记录以前二十字节直接兼容DrawIndexedIndirectArgs。
    index_count: u32, // 字节0：当前LOD原型索引数量，由Rust写入。
    instance_count: u32, // 字节4：收尾阶段写入当前桶安全实例数。
    first_index: u32, // 字节8：MeshAllocator共享索引切片起点。
    base_vertex: i32, // 字节12：MeshAllocator共享顶点切片起点。
    first_instance: u32, // 字节16：当前桶在完整紧凑实例缓冲中的固定分区起点。
    reserved_instances: atomic<u32>, // 字节20：分类工作组为当前桶预留的实例数。
    capacity: u32, // 字节24：当前桶实例分区容量，八桶总和等于原实例容量。
    overflow_instances: atomic<u32>, // 字节28：收尾阶段记录当前桶超出容量的实例数。
} // 完成深度桶Compute写入与索引间接绘制共用布局。

struct GrassTileDispatch { // 二十字节状态的前三项直接兼容DispatchIndirectArgs。
    dispatch_x: u32, // 字节0：细分类派发X，最多六万五千五百三十五组。
    dispatch_y: u32, // 字节4：活跃瓦片超过单维限制时继续展开到Y。
    dispatch_z: u32, // 字节8：当前二维瓦片列表始终固定为一。
    active_count: atomic<u32>, // 字节12：粗剔除工作线程向活跃列表追加编号的全局计数。
} // 完成GPU生成且GPU直接消费的瓦片间接派发布局。

struct GrassViewCulling { // 全部植被窗口共享同一渲染帧的304字节视锥、八档LOD和画质数据。
    frustum_planes: array<vec4<f32>, 6>, // 左、右、上、下、近、远六个世界半空间的单位法线与有符号距离。
    camera_position_enabled: vec4<f32>, // xyz为相机世界位置；w为零禁用、一仅视锥、二启用屏幕空间LOD。
    camera_forward_pixel_scale: vec4<f32>, // xyz为相机世界前向，w为半视口高度乘投影矩阵Y缩放。
    lod_pixel_thresholds: vec4<f32>, // xy为远转中像素范围，zw为中转近像素范围。
    lod_ultra_pixel_thresholds: vec4<f32>, // xy为超远代理转入既有远级子树的像素范围，zw保留。
    camera_fade_lod_params: vec4<f32>, // xy为近镜头整株避让范围，z为参考草高，w为统一最细世界格距。
    lod_residency_radii: vec4<f32>, // xyzw为近中远超远相机驱动常驻外半径。
    lod_residency_transition_widths: vec4<f32>, // xyz为近/中/远子级过渡宽度，w为统一安全边距。
    quality_multipliers: vec4<f32>, // xyz为最终密度、屏幕LOD阈值和作者范围倍率，w保留。
    lod_pixel_thresholds_ext: array<vec4<f32>, 2>,
    lod_residency_radii_ext: vec4<f32>,
    lod_residency_transitions_ext: vec4<f32>,
    lod_metadata: vec4<f32>,
} // 完成与Rust GpuBladeViewCullingUniform一致的基础区与八档扩展布局。

@group(0) @binding(0) var<uniform> grass_patch: GrassPatch; // 三圈共享动态uniform缓冲并由动态offset选择当前窗口。
@group(0) @binding(1) var grass_height_cells: texture_2d<f32>; // 复用现有RGBA16Unorm四角高度单元纹理。
@group(0) @binding(2) var grass_density_map: texture_2d<f32>; // 复用现有线性RGBA8Unorm密度概率纹理。
@group(0) @binding(3) var<storage, read_write> compact_grass: array<CompactGrassInstance>; // 当前环带的连续紧凑实例输出。
@group(0) @binding(4) var<storage, read_write> grass_state: GrassDrawState; // 当前环带的预留计数、阶段统计和间接参数。
@group(0) @binding(5) var<uniform> grass_view_culling: GrassViewCulling; // 全部草圈绑定同一份本帧玩法相机六平面。
@group(0) @binding(6) var<storage, read_write> active_tiles: array<u32>; // 粗剔除写入、候选细分类读取的短瓦片工作列表。
@group(0) @binding(7) var<storage, read_write> tile_dispatch: GrassTileDispatch; // GPU生成的活跃计数与间接派发参数。
@group(0) @binding(8) var<storage, read_write> depth_buckets: array<GrassDepthBucketState>; // 八个由近到远的径向桶分区与间接参数。

const CONSERVATIVE_HORIZONTAL_RADIUS: f32 = 2.0; // 包含原型宽度、视锥数值误差和旋转边缘安全带，避免原地旋转缺草。
const DENSITY_FIELD_ENABLED_FLAG: u32 = 1u; // 保持grid.w最低位启用密度概率采样的契约。
const LOD_LEVEL_SHIFT: u32 = 1u; // grid.w第1位开始保存几何档号。
const LOD_LEVEL_MASK: u32 = 0x7u; // 三位LOD字段支持 0..=7 共八档。
const HIERARCHICAL_LOD_FLAG: u32 = 1u << 5u; // 正式层级窗口使用同源父簇，并在相机数据无效时只保留最远祖先代理。
const VEGETATION_PROFILE_SHIFT: u32 = 6u; // grid.w第6位开始保存两位普通草、麦田或后续叶片内容编号。
const VEGETATION_PROFILE_MASK: u32 = 0x3u; // 两位内容编号与绘制端固定四槽profile数组严格一致。
const ULTRA_LOD_CHAIN_FLAG: u32 = 1u << 8u; // 正式层级内容显式进入顶层父簇，兼容短链内容保持原父树。
const ULTRA_LOD_BUDGET_SHIFT: u32 = 9u; // 两位预算编码从第九位开始保存高、中、低固定档。
const ULTRA_LOD_BUDGET_MASK: u32 = 0x3u; // 预算编码三作为低档安全回退。
const PROXY_COVERAGE_SHIFT: u32 = 2u; // 最高外观字节中间三位开始保存当前LOD代理的面积覆盖比例。
const PROXY_COVERAGE_MASK: u32 = 0x7u; // 三位编码一至八份覆盖，零编码表示八分之一而不是无实例。
const COMPACT_WORKGROUP_SIZE: u32 = 256u; // 256线程兼顾当前设备通用上限和前缀扫描效率。
const TILE_SIDE: u32 = 16u; // 十六乘十六候选恰好由一个二百五十六线程细分类工作组覆盖。
const TILE_CULL_WORKGROUP_SIZE: u32 = 64u; // 每个低成本粗剔除工作组检查六十四个瓦片。
const MAX_DISPATCH_DIMENSION: u32 = 65535u; // WebGPU单维工作组上限，活跃列表可安全展开到Y维。
const DEPTH_BUCKET_COUNT: u32 = 8u; // 固定小桶数最多生成二十四个草地draw，避免逐草全局排序成本。
const TWO_PI: f32 = 6.28318530718; // 保持旧路径完整一圈旋转常量并在Compute中只计算一次三角函数。
const APPEARANCE_CHANNEL_MASK: u32 = 0x000000ffu; // 四个外观通道都使用零到255的八位量化值。
const ROOT_JITTER_SPAN: f32 = 0.98; // 草根可使用自身格子百分之九十八的宽度，消除旧百分之六十五抖动留下的隐约行列。
const MAX_ROOT_GRID_DISPLACEMENT: f32 = 1.0 + ROOT_JITTER_SPAN * 0.5; // 粗瓦片每轴扩一格邻格重映射加半个抖动跨度，严格覆盖草根最大一点四九格位移。
const APPEARANCE_CLUSTER_METERS: f32 = 3.4; // 三点四米连续低频簇让相邻草高略相关，同时不形成可见硬块边界。
const CLUSTER_REMAP_SALT: u32 = 0x632be5abu; // 独立盐决定候选向四个对角邻格之一重映射，禁止与朝向、密度或高度随机相关。
const SCREEN_SPACE_LOD_MODE: f32 = 1.5; // 相机状态w大于该值时由投影尺寸选择三级草簇，玩家距离只管理常驻边界。
const FAR_TO_MIDDLE_LOD_SALT: u32 = 0x2c9277b5u; // 四乘四最细父格使用固定盐在远簇和更细子树间整簇切换。
const MIDDLE_TO_NEAR_LOD_SALT: u32 = 0x7f4a7c15u; // 二乘二最细父格使用另一固定盐在中簇和四个近叶间整簇切换。
const ACTIVATION_SALT: u32 = 0x91e10da5u; // 首载覆盖率只控制完整实例出现，禁止与LOD或密度随机源相关。
const RESIDENCY_FADE_SALT: u32 = 0xd1b54a35u; // 常驻窗口外沿使用世界稳定整株稀释，把三维草柔和交给全图地表材质。
const CAMERA_PROXIMITY_SALT: u32 = 0x94d049bbu; // 镜头穿入草叶时只在极小范围整株避让，不再展示被近裁面截断的残片。
const CANONICAL_ROOT_SALT: u32 = 0x51ed270bu; // 三级父簇根位置只由绝对层级格决定，不再读取各窗口独立seed。
const CANONICAL_OCCUPANCY_SALT: u32 = 0xa511e9b3u; // 四乘四最细父格共享同一密度占用随机源，LOD替换不改变有草或无草。
const CANONICAL_LEVEL_STRIDE: u32 = 0x9e3779b9u; // 不同层级根外观使用独立但确定的父格随机序列。
const CANONICAL_ROOT_JITTER_SPAN: f32 = 0.84; // 草根只在自身层级父格内部抖动，禁止跨父格重映射破坏LOD对应关系。

// Naga 29 的 SPIR-V 后端为 Workgroup 数组无条件写 ArrayStride；标量槽展开规避 Vulkan 验证错误，依赖修复后可撤销。
var<workgroup> bucket_count_0: atomic<u32>; // 原桶 0 的同一工作组原子计数，不改变预留或散布语义。
var<workgroup> bucket_base_0: u32; // 原桶 0 的独立共享基址，仍由原屏障保证跨线程可见。
var<workgroup> bucket_count_1: atomic<u32>; // 原桶 1 的同一工作组原子计数，不改变预留或散布语义。
var<workgroup> bucket_base_1: u32; // 原桶 1 的独立共享基址，仍由原屏障保证跨线程可见。
var<workgroup> bucket_count_2: atomic<u32>; // 原桶 2 的同一工作组原子计数，不改变预留或散布语义。
var<workgroup> bucket_base_2: u32; // 原桶 2 的独立共享基址，仍由原屏障保证跨线程可见。
var<workgroup> bucket_count_3: atomic<u32>; // 原桶 3 的同一工作组原子计数，不改变预留或散布语义。
var<workgroup> bucket_base_3: u32; // 原桶 3 的独立共享基址，仍由原屏障保证跨线程可见。
var<workgroup> bucket_count_4: atomic<u32>; // 原桶 4 的同一工作组原子计数，不改变预留或散布语义。
var<workgroup> bucket_base_4: u32; // 原桶 4 的独立共享基址，仍由原屏障保证跨线程可见。
var<workgroup> bucket_count_5: atomic<u32>; // 原桶 5 的同一工作组原子计数，不改变预留或散布语义。
var<workgroup> bucket_base_5: u32; // 原桶 5 的独立共享基址，仍由原屏障保证跨线程可见。
var<workgroup> bucket_count_6: atomic<u32>; // 原桶 6 的同一工作组原子计数，不改变预留或散布语义。
var<workgroup> bucket_base_6: u32; // 原桶 6 的独立共享基址，仍由原屏障保证跨线程可见。
var<workgroup> bucket_count_7: atomic<u32>; // 原桶 7 的同一工作组原子计数，不改变预留或散布语义。
var<workgroup> bucket_base_7: u32; // 原桶 7 的独立共享基址，仍由原屏障保证跨线程可见。
var<workgroup> stage_count_0: atomic<u32>; // 原阶段 0 的同一工作组原子统计。
var<workgroup> stage_count_1: atomic<u32>; // 原阶段 1 的同一工作组原子统计。
var<workgroup> stage_count_2: atomic<u32>; // 原阶段 2 的同一工作组原子统计。

fn clear_bucket_group(index: u32) { // 与原前八线程逐槽清零完全等价。
    switch index { // 无 Workgroup 数组装饰。
        case 0u: { atomicStore(&bucket_count_0, 0u); bucket_base_0 = 0u; } // 清零原桶 0。
        case 1u: { atomicStore(&bucket_count_1, 0u); bucket_base_1 = 0u; } // 清零原桶 1。
        case 2u: { atomicStore(&bucket_count_2, 0u); bucket_base_2 = 0u; } // 清零原桶 2。
        case 3u: { atomicStore(&bucket_count_3, 0u); bucket_base_3 = 0u; } // 清零原桶 3。
        case 4u: { atomicStore(&bucket_count_4, 0u); bucket_base_4 = 0u; } // 清零原桶 4。
        case 5u: { atomicStore(&bucket_count_5, 0u); bucket_base_5 = 0u; } // 清零原桶 5。
        case 6u: { atomicStore(&bucket_count_6, 0u); bucket_base_6 = 0u; } // 清零原桶 6。
        case 7u: { atomicStore(&bucket_count_7, 0u); bucket_base_7 = 0u; } // 清零原桶 7。
        default: {} // 调用方已限制八槽范围。
    } // 完成显式槽选择。
} // 保留外部原工作组屏障。
fn add_bucket_group(index: u32) -> u32 { // 每个存活线程仍只执行一次相同槽位原子加。
    switch index { // 显式标量槽保持同一竞争域。
        case 0u: { return atomicAdd(&bucket_count_0, 1u); } // 原桶 0 连续槽位。
        case 1u: { return atomicAdd(&bucket_count_1, 1u); } // 原桶 1 连续槽位。
        case 2u: { return atomicAdd(&bucket_count_2, 1u); } // 原桶 2 连续槽位。
        case 3u: { return atomicAdd(&bucket_count_3, 1u); } // 原桶 3 连续槽位。
        case 4u: { return atomicAdd(&bucket_count_4, 1u); } // 原桶 4 连续槽位。
        case 5u: { return atomicAdd(&bucket_count_5, 1u); } // 原桶 5 连续槽位。
        case 6u: { return atomicAdd(&bucket_count_6, 1u); } // 原桶 6 连续槽位。
        default: { return atomicAdd(&bucket_count_7, 1u); } // 合法最后一桶。
    } // 完成原子槽选择。
} // 不增加全局原子或更改容量。
fn load_bucket_group(index: u32) -> u32 { // 屏障后读取原工作组桶数量。
    switch index { // 与原动态下标相同的八选一。
        case 0u: { return atomicLoad(&bucket_count_0); } // 原桶 0 数量。
        case 1u: { return atomicLoad(&bucket_count_1); } // 原桶 1 数量。
        case 2u: { return atomicLoad(&bucket_count_2); } // 原桶 2 数量。
        case 3u: { return atomicLoad(&bucket_count_3); } // 原桶 3 数量。
        case 4u: { return atomicLoad(&bucket_count_4); } // 原桶 4 数量。
        case 5u: { return atomicLoad(&bucket_count_5); } // 原桶 5 数量。
        case 6u: { return atomicLoad(&bucket_count_6); } // 原桶 6 数量。
        default: { return atomicLoad(&bucket_count_7); } // 合法最后一桶。
    } // 完成原子读取。
} // 不改变聚合屏障。
fn store_bucket_base(index: u32, value: u32) { // 前八线程各自写回原全局预留基址。
    switch index { // 不把独立标量写合并为可能竞争的向量写。
        case 0u: { bucket_base_0 = value; } // 发布原桶 0 基址。
        case 1u: { bucket_base_1 = value; } // 发布原桶 1 基址。
        case 2u: { bucket_base_2 = value; } // 发布原桶 2 基址。
        case 3u: { bucket_base_3 = value; } // 发布原桶 3 基址。
        case 4u: { bucket_base_4 = value; } // 发布原桶 4 基址。
        case 5u: { bucket_base_5 = value; } // 发布原桶 5 基址。
        case 6u: { bucket_base_6 = value; } // 发布原桶 6 基址。
        case 7u: { bucket_base_7 = value; } // 发布原桶 7 基址。
        default: {} // 调用方已限制八槽范围。
    } // 完成基址发布。
} // 仍由后续原屏障同步。
fn load_bucket_base(index: u32) -> u32 { // 存活线程在原屏障后读取对应基址。
    switch index { // 同一八桶布局不变。
        case 0u: { return bucket_base_0; } // 原桶 0 基址。
        case 1u: { return bucket_base_1; } // 原桶 1 基址。
        case 2u: { return bucket_base_2; } // 原桶 2 基址。
        case 3u: { return bucket_base_3; } // 原桶 3 基址。
        case 4u: { return bucket_base_4; } // 原桶 4 基址。
        case 5u: { return bucket_base_5; } // 原桶 5 基址。
        case 6u: { return bucket_base_6; } // 原桶 6 基址。
        default: { return bucket_base_7; } // 合法最后一桶。
    } // 完成共享基址读取。
} // 不改变实例索引计算。
fn clear_stage_group(index: u32) { // 原前三线程初始化相同三阶段统计。
    switch index { // 标量展开只规避 Naga 数组装饰。
        case 0u: { atomicStore(&stage_count_0, 0u); } // 原阶段 0 清零。
        case 1u: { atomicStore(&stage_count_1, 0u); } // 原阶段 1 清零。
        case 2u: { atomicStore(&stage_count_2, 0u); } // 原阶段 2 清零。
        default: {} // 调用方已限制三个阶段。
    } // 完成统计清零。
} // 保持原工作组屏障。
fn load_stage_group(index: u32) -> u32 { // 原前三线程发布相同统计数量。
    switch index { // 不改变设备级聚合次数。
        case 0u: { return atomicLoad(&stage_count_0); } // 距离阶段。
        case 1u: { return atomicLoad(&stage_count_1); } // 密度阶段。
        default: { return atomicLoad(&stage_count_2); } // 地表阶段。
    } // 完成统计读取。
} // 完成 Naga 29 兼容展开。

fn hash_u32(input: u32) -> u32 { // 完整复用旧顶点着色器的32位整数哈希。
    var value = input; // 使用局部可变值依次执行原有混合步骤。
    value = value ^ (value >> 16u); // 第一次高低位异或混合保持不变。
    value = value * 0x7feb352du; // 第一乘数保持所有既有世界草格布局不变。
    value = value ^ (value >> 15u); // 第二次异或混合保持不变。
    value = value * 0x846ca68bu; // 第二乘数保持既有随机序列不变。
    value = value ^ (value >> 16u); // 最终异或得到与旧路径相同的32位哈希。
    return value; // 返回稳定候选哈希。
} // 完成32位哈希。

fn hash_unit(input: u32) -> f32 { // 把同一整数哈希映射到旧路径使用的闭区间零到一。
    return f32(hash_u32(input) & 0x00ffffffu) / 16777215.0; // 保留低24位和原有除数以避免随机阈值漂移。
} // 完成稳定单位随机值。

fn probability_rejects(random_value: f32, probability: f32) -> bool { // 保持零必剔、一必留和中间严格小于的概率语义。
    if (probability <= 0.0) { // 黑色禁种区或环带外必须严格拒绝。
        return true; // 即使随机哈希恰为零也不允许漏出草株。
    } // 完成严格零概率分支。
    if (probability >= 1.0) { // 完整保留区必须无条件通过。
        return false; // 即使随机哈希恰为一也不丢失候选。
    } // 完成严格一概率分支。
    return random_value >= probability; // 中间概率继续实现random小于probability才保留。
} // 完成概率端点与中间值判定。

fn hash_world_cell(cell: vec2<i32>, seed: u32) -> u32 { // 世界整数草格与环带固定种子共同决定稳定外观。
    let cell_x = bitcast<u32>(cell.x); // 负世界X按位解释以保持旧路径哈希结果。
    let cell_z = bitcast<u32>(cell.y); // 负世界Z按位解释以保持旧路径哈希结果。
    return hash_u32((cell_x * 0x9e3779b9u) ^ (cell_z * 0x85ebca6bu) ^ seed); // 完整复用原有二维草格混合常量。
} // 完成世界草格哈希。

fn hash_world_cell_unit(cell: vec2<i32>, seed: u32) -> f32 { // 把任意低频世界格映射为连续噪声角点值。
    return f32(hash_world_cell(cell, seed) & 0x00ffffffu) / 16777215.0; // 直接复用已混合结果，避免对同一角点重复执行完整哈希。
} // 完成世界格单位随机值。

fn floor_parent_cell(fine_cell: vec2<i32>, divisor: f32) -> vec2<i32> { // 负世界坐标也必须映射到与正坐标同样大小的LOD父格。
    return vec2<i32>(floor(vec2<f32>(fine_cell) / divisor)); // 使用浮点floor避免WGSL有符号整数除法向零截断破坏窗口跨界稳定性。
} // 完成欧几里得意义的二维父格映射。

fn parent_center_world(parent_cell: vec2<i32>, parent_side: f32, fine_spacing: f32) -> vec2<f32> { // 用同一父格中心计算全部子叶的切换距离。
    return (vec2<f32>(parent_cell) * parent_side + vec2<f32>((parent_side - 1.0) * 0.5)) * fine_spacing; // 二级父格中心位于零点五细格，四级父格中心位于一点五细格。
} // 完成不会受各级草根抖动影响的世界父格中心。

struct HierarchicalDensitySelection { // 返回当前层级候选是否存活以及宽代理应承担的覆盖比例。
    survives: bool, // false表示该层级块没有任何由密度图选中的最细叶片。
    proxy_coverage: f32, // 一表示完整代理，四分之一表示中簇只代表一片近叶。
} // 完成仅在Compute局部使用的层级密度选择结果。

// Naga 29 Vulkan 兼容：动态常量数组可能降为带 ArrayStride 的函数数组，原 Bayer 名次无损打包为四位槽。
const DENSITY_RANK_LOW: u32 = 0x6e4ca280u; // 原前八项 0,8,2,10,12,4,14,6，最低四位对应索引零。
const DENSITY_RANK_HIGH: u32 = 0x5d7f91b3u; // 原后八项 3,11,1,9,15,7,13,5，空间分散排列不变。

fn density_child_rank(local_child: vec2<i32>, parent_seed: u32) -> u32 { // 为四乘四父格内每个最细子格生成唯一且空间分散的名次。
    var child_x = u32(local_child.x); // 欧几里得父格计算保证局部X严格位于零至三。
    var child_y = u32(local_child.y); // 欧几里得父格计算保证局部Y严格位于零至三。
    let transform = parent_seed & 0x7u; // 每个世界父格稳定选择八种旋转或镜像，打散重复排序方向。
    if ((transform & 0x1u) != 0u) { // 第一位控制水平镜像。
        child_x = 3u - child_x; // 四格索引镜像后仍保持零至三。
    } // 完成水平镜像。
    if ((transform & 0x2u) != 0u) { // 第二位控制垂直镜像。
        child_y = 3u - child_y; // 四格索引镜像后仍保持零至三。
    } // 完成垂直镜像。
    if ((transform & 0x4u) != 0u) { // 第三位控制转置并与两次镜像组合成八种方形对称。
        let swapped_x = child_x; // 临时保存转置前X。
        child_x = child_y; // 原Y成为转置后X。
        child_y = swapped_x; // 原X成为转置后Y。
    } // 完成稳定方形变换。
    let rank_index = child_y * 4u + child_x; // 保持原变换位置索引，不改变任何候选集合。
    let packed_ranks = select(DENSITY_RANK_LOW, DENSITY_RANK_HIGH, rank_index >= 8u); // 只替换固定表的存储表达。
    return (packed_ranks >> ((rank_index & 7u) * 4u)) & 15u; // 恢复与原数组逐项相同的唯一名次。
} // 完成父格内稳定均匀排序。

fn density_selected_count(density_probability: f32, slot_count: u32, parent_seed: u32) -> u32 { // 把连续密度转换为父格内确定数量而不是整簇一次伯努利。
    let scaled_count = clamp(density_probability, 0.0, 1.0) * f32(slot_count); // 概率乘父格最细叶片数得到期望数量。
    let whole_count = u32(floor(scaled_count)); // 整数部分在该父格内始终保留。
    let fractional_count = scaled_count - f32(whole_count); // 小数部分通过世界稳定随机做无偏取整。
    let rounded_extra = select( // 零或一的额外叶片保持跨窗口和跨帧稳定。
        0u, // 固定随机值未落入小数概率时不增加叶片。
        1u, // 固定随机值落入小数概率时增加一片。
        hash_unit(parent_seed ^ 0x165667b1u) < fractional_count, // 只使用父格身份，不读取帧号或当前LOD。
    ); // 完成父格级随机舍入。
    return min(whole_count + rounded_extra, slot_count); // 一概率严格得到全部叶片且永不越过槽数。
} // 完成连续密度到稳定整数覆盖的转换。

fn hierarchical_density_selection( // 让近、中、远代理守恒同一四乘四父格内的实际叶片数量。
    fine_cell: vec2<i32>, // 当前候选换算到共同最细格后的世界身份。
    lod_level: u32, // 零至三分别表示近叶、中簇、远簇和超远代理。
    density_probability: f32, // 父格中心读取并应用内容增益后的最终密度概率。
) -> HierarchicalDensitySelection {
    if (lod_level >= 3u) { // 超远级由一个八乘八代理和正式地表共同承接远距离轮廓。
        let parent_side = lod_parent_side(lod_level); // 扩展档按自身层级覆盖，不再全部别名到八乘八。
        let ultra_parent = floor_parent_cell(fine_cell, parent_side);
        let ultra_seed = hash_world_cell(ultra_parent, CANONICAL_OCCUPANCY_SALT); // 使用顶层世界父格生成稳定整数覆盖。
        let slots = u32(parent_side * parent_side);
        let selected_count = density_selected_count(density_probability, slots, ultra_seed);
        return HierarchicalDensitySelection( // 超远只生成一个宽代理并按真实占用缩窄。
            selected_count > 0u, // 黑区或稳定取整为零的父格不生成代理。
            f32(selected_count) / f32(slots), // 只做解析计数，最高档也不遍历16384个最细格。
        ); // 完成超远密度选择。
    } // 完成超远独立分支。

    let far_parent = floor_parent_cell(fine_cell, 4.0); // 近中远三级共同归属唯一四乘四密度父格。
    let far_origin = far_parent * 4; // 恢复该父格左下角最细世界格供局部排序。
    let parent_seed = hash_world_cell(far_parent, CANONICAL_OCCUPANCY_SALT); // 父格身份同时决定稳定取整和排序方向。
    let selected_count = density_selected_count(density_probability, 16u, parent_seed); // 每个父格按概率得到零至十六片实际近叶。
    if (lod_level == 0u) { // 近级每个候选直接对应一片最细叶片。
        let local_child = fine_cell - far_origin; // 局部坐标严格位于零至三。
        let child_rank = density_child_rank(local_child, parent_seed); // 读取空间分散且唯一的父格内名次。
        return HierarchicalDensitySelection( // 近叶要么完整保留要么完整剔除。
            child_rank < selected_count, // 只保留排序前N个子格，禁止十六片整簇一起抽签。
            1.0, // 存活近叶始终使用完整单叶宽度。
        ); // 完成近级稳定均匀选择。
    } // 完成近级分支。

    if (lod_level == 1u) { // 中级一个代理严格代表当前二乘二子块中的四片近叶。
        let middle_parent = floor_parent_cell(fine_cell, 2.0); // 找到当前中簇覆盖的二乘二最细父格。
        let middle_origin = middle_parent * 2; // 恢复该中簇左下角最细格。
        var middle_selected_count = 0u; // 累加当前四个子格中实际进入前N名的数量。
        for (var child_y = 0i; child_y < 2i; child_y = child_y + 1i) { // 遍历中簇的两行最细格。
            for (var child_x = 0i; child_x < 2i; child_x = child_x + 1i) { // 遍历每行两个最细格。
                let child_cell = middle_origin + vec2<i32>(child_x, child_y); // 恢复当前子格绝对世界身份。
                let local_child = child_cell - far_origin; // 转为共同四乘四父格内局部坐标。
                if (density_child_rank(local_child, parent_seed) < selected_count) { // 使用与近级完全相同的前N名集合。
                    middle_selected_count = middle_selected_count + 1u; // 当前中簇多承担一片近叶覆盖。
                } // 完成单个子格计数。
            } // 完成当前行计数。
        } // 完成二乘二子块计数。
        return HierarchicalDensitySelection( // 中簇宽度按实际承担的一至四片近叶缩放。
            middle_selected_count > 0u, // 当前子块没有选中叶片时不生成空代理。
            f32(middle_selected_count) / 4.0, // 一至四片分别使用四分之一至完整宽度。
        ); // 完成中级覆盖守恒选择。
    } // 完成中级分支。

    return HierarchicalDensitySelection( // 远级一个代理承担整个四乘四父格。
        selected_count > 0u, // 父格没有选中近叶时远级同样不生成代理。
        f32(selected_count) / 16.0, // 远代理按零至十六片中的真实份额缩窄。
    ); // 完成远级覆盖守恒选择。
} // 完成三级一致、父格内均匀的密度选择。

fn lod_parent_side(lod_level: u32) -> f32 { // 八档分别覆盖1、2、4、8、16、32、64、128个最细格。
    return f32(1u << min(lod_level, 7u));
}

fn ultra_budget_block_side() -> f32 { // 解码超远世界稳定代理池的块单边父格数。
    let encoded = (grass_patch.grid.w >> ULTRA_LOD_BUDGET_SHIFT) & ULTRA_LOD_BUDGET_MASK; // 只读取两位固定预算编码。
    if (encoded == 0u) {
        return 2.0; // 高档每二乘二个超远父格保留一个代理。
    }
    if (encoded == 1u) {
        return 3.0; // 增密档每三乘三个超远父格保留一个代理。
    }
    if (encoded == 2u) {
        return 4.0; // 中档每四乘四个超远父格保留一个代理。
    }
    return 8.0; // 低档每八乘八个超远父格保留一个代理。
}

fn ultra_budget_selects(fine_cell: vec2<i32>) -> bool { // 每个固定世界预算块只选择一个超远父格，避免原子截断造成时域闪烁。
    let ultra_parent = floor_parent_cell(fine_cell, lod_parent_side(content_lod_count() - 1u)); // 仅实际最粗档承接固定预算。
    let block_side = ultra_budget_block_side(); // 高、增密、中、低分别使用二、三、四、八父格单边。
    let budget_block = floor_parent_cell(ultra_parent, block_side); // 负世界坐标同样进入稳定欧几里得预算块。
    let block_side_u32 = u32(block_side); // 预算档都是精确整数，转换后用于取模和线性槽号。
    let slot_count = block_side_u32 * block_side_u32; // 每块最多六十四个候选槽。
    let chosen_slot = hash_world_cell(budget_block, 0x6a09e667u) % slot_count; // 固定世界哈希选择唯一代理槽。
    let local_parent = ultra_parent - budget_block * i32(block_side_u32); // 欧几里得父块保证局部坐标落在零至边长减一。
    let local_slot = u32(local_parent.y) * block_side_u32 + u32(local_parent.x); // 二维局部位置映射到唯一线性槽。
    return local_slot == chosen_slot; // 世界不动时选择永远不变，窗口平移只会补入新块。
}

fn canonical_lod_root(
    fine_cell: vec2<i32>,
    lod_level: u32,
    fine_spacing: f32,
) -> vec2<f32> { // 在当前层级父格内部生成世界稳定根位置，替代各LOD独立跨邻格重映射。
    let parent_side = lod_parent_side(lod_level); // 取得当前代理覆盖的一、二或四格边长。
    let parent_cell = floor_parent_cell(fine_cell, parent_side); // 负坐标同样映射到唯一层级父格。
    let parent_center = parent_center_world(parent_cell, parent_side, fine_spacing); // 每级代理从自身覆盖区域中心开始。
    let parent_seed = hash_world_cell(
        parent_cell,
        CANONICAL_ROOT_SALT ^ (lod_level * CANONICAL_LEVEL_STRIDE),
    ); // 层级与父格共同决定根位置和外观，不读取近中远窗口各自seed。
    let jitter = vec2<f32>(
        hash_unit(parent_seed ^ 0x68bc21ebu) - 0.5,
        hash_unit(parent_seed ^ 0x02e5be93u) - 0.5,
    ) * parent_side * fine_spacing * CANONICAL_ROOT_JITTER_SPAN; // 最大偏移小于父格半宽，根永远留在自己的覆盖父格内。
    return parent_center + jitter; // 代理切换只在同一个微型父簇内部重排，不会跨父格制造边界空洞。
}

fn canonical_lod_seed(fine_cell: vec2<i32>, lod_level: u32) -> u32 { // 为当前层级代理生成与窗口seed无关的稳定外观身份。
    let parent_cell = floor_parent_cell(fine_cell, lod_parent_side(lod_level)); // 近叶、中簇和远簇分别使用自身层级父格ID。
    return hash_world_cell(
        parent_cell,
        CANONICAL_ROOT_SALT ^ (lod_level * CANONICAL_LEVEL_STRIDE),
    ); // 与根位置共享父格身份，使窗口重定位和不同LOD实体都不会洗牌。
}

fn lod_child_residency_weight( // 计算目标细级在玩家常驻窗口安全边界内的稳定可用权重。
    parent_world_xz: vec2<f32>, // 当前近中共享父格或中远共享父格的世界中心。
    child_outer_radius: f32, // 目标细级窗口相对真实玩家的常驻外半径。
    transition_width: f32, // 从完整子级到完整祖先的玩家半径过渡宽度。
    configured_safety_margin: f32, // 当前窗口或全局视图提供的外沿安全边距。
) -> f32 { // 返回零至一；随后与固定世界父格哈希比较，不产生逐帧随机。
    return lod_distance_residency_weight(length(parent_world_xz - grass_patch.focus_radius.xz), child_outer_radius, transition_width, configured_safety_margin);
}

// 室外二维距离与洞内三维距离只在输入处区分，安全边距和过渡预算共用。
fn lod_distance_residency_weight(parent_distance:f32,child_outer_radius:f32,transition_width:f32,configured_safety_margin:f32)->f32 {
    let safety_margin = max(configured_safety_margin, 0.0); // 外沿淡出、父格抖动与浮点误差共用当前内容安全边距。
    let safe_radius = max(child_outer_radius - safety_margin, 0.0); // 子级只在完整驻留区内接管，不在自身外沿概率淡出区接管。
    if (child_outer_radius <= safety_margin || safe_radius <= 0.01) { // 损坏半径无法容纳有效的平滑安全域。
        return 0.0; // 完整回退祖先，避免反向smoothstep让三级同时出现。
    } // 完成非法安全域保护。
    let stable_transition_width = clamp(transition_width, 0.01, safe_radius); // 损坏配置也不能产生零宽硬切或越过玩家中心。
    let fully_resident_radius = max(safe_radius - stable_transition_width, 0.0); // 内端完整允许相机投影LOD选择子级。
    return 1.0 - smoothstep( // 从内侧完整子级连续过渡到外侧完整祖先。
        fully_resident_radius, // 该半径以内保持权重一。
        safe_radius, // 到达外沿安全边界时严格归零。
        parent_distance, // 玩家静止时同一世界父格始终得到同一权重。
    ); // 完成不依赖相机微抖的常驻过渡权重。
} // 完成目标细级稳定常驻权重。

fn projected_reference_pixels(parent_world_xz: vec2<f32>) -> f32 { // 把统一参考草高转换为当前玩法相机中的垂直像素尺寸。
    return projected_reference_pixels_at(vec3<f32>(parent_world_xz.x, grass_patch.focus_radius.y, parent_world_xz.y));
}

fn projected_reference_pixels_at(root:vec3<f32>)->f32 {
    let reference_height = max(grass_patch.vegetation_bounds.z, 0.01); // 每个内容窗口独立使用普通草或麦田典型株高。
    let reference_center = root + vec3<f32>(0.0, reference_height * 0.5, 0.0); // 室外沿用稳定参考高度，三维层使用实际层高度。
    let camera_to_parent = (grass_patch.source_to_world * vec4<f32>(reference_center, 1.0)).xyz - grass_view_culling.camera_position_enabled.xyz; // 取得世界相机到父格中心向量。
    let view_depth = dot(camera_to_parent, grass_view_culling.camera_forward_pixel_scale.xyz); // 沿真实相机前向计算透视深度而不是玩家平面距离。
    if (view_depth <= 0.01) { // 父格中心恰在相机近面或后方时优先选择高质量，边缘子叶仍由保守视锥决定是否绘制。
        return 1000000.0; // 足够大的有限像素值避免无穷数进入smoothstep。
    } // 完成近面保守分支。
    return reference_height
        * max(grass_view_culling.camera_forward_pixel_scale.w, 1.0)
        / view_depth; // 标准透视投影尺寸随视口、FOV和真实视深自动变化。
} // 完成屏幕空间草高估算。

fn uses_patch_view_lod_config() -> bool { // 允许同一视图内不同profile使用各自的LOD标定。
    return grass_patch.view_lod_metadata.x > 0.0; // 单档也能启用局部配置，不能用第四半径作为开关。
} // 完成不会影响既有地图窗口的内容级配置开关。

fn quality_density_multiplier() -> f32 { // 返回不会突破作者最终概率的全局植被密度倍率。
    return clamp(grass_view_culling.quality_multipliers.x, 0.0, 1.0); // 零允许未来完全关闭，一严格保持当前内容基线。
} // 完成最终概率质量倍率净化。

fn quality_lod_threshold_multiplier() -> f32 { // 返回屏幕LOD阈值倍率，越小越晚切换到粗模。
    return clamp(grass_view_culling.quality_multipliers.y, 0.5, 2.0); // 与主世界组件钳制范围一致，防止损坏uniform退化smoothstep。
} // 完成全局与内容级LOD共用倍率净化。

fn quality_view_distance_multiplier() -> f32 { // 返回作者草窗和常驻半径的统一收缩倍率。
    return clamp(grass_view_culling.quality_multipliers.z, 0.25, 1.0); // 只允许收缩，禁止越过已分配候选窗口和缓冲边界。
} // 完成不会触发GPU资源扩容的范围倍率净化。

fn content_lod_pixel_thresholds() -> vec4<f32> { // 返回当前内容远中与中近屏幕阈值。
    return select(
        grass_view_culling.lod_pixel_thresholds, // 旧窗口和主内容继续使用玩法视图集中规格。
        grass_patch.view_lod_pixel_thresholds,   // 混合草种地图可为当前窗口提供独立标定。
        uses_patch_view_lod_config(),
    ) * quality_lod_threshold_multiplier(); // 全局画质同时缩放继承值与内容局部标定。
} // 完成两段屏幕阈值读取。

fn content_lod_ultra_pixel_thresholds() -> vec2<f32> { // 返回当前内容超远代理转远级子树阈值。
    return select(
        grass_view_culling.lod_ultra_pixel_thresholds.xy, // 默认继承玩法视图主内容。
        grass_patch.view_lod_ultra_fade.xy,               // 局部配置前两项保存超远阈值。
        uses_patch_view_lod_config(),
    ) * quality_lod_threshold_multiplier(); // 极高档同样延伸超远到远级子树的精细轮廓距离。
} // 完成第四级交接阈值选择。

fn content_lod_residency_radii() -> vec4<f32> { // 返回当前内容近中远超远常驻域。
    return select(
        grass_view_culling.lod_residency_radii, // 旧窗口使用全局四档半径。
        grass_patch.view_lod_residency_radii,   // 局部配置避免低矮内容扩大其他草种预算。
        uses_patch_view_lod_config(),
    ) * quality_view_distance_multiplier(); // 低中档在所有内容中一致收缩子级可用范围。
} // 完成内容级常驻半径读取。

fn content_lod_residency_transitions() -> vec4<f32> { // 返回当前内容三段回退宽度与安全边距。
    let transitions = select(
        grass_view_culling.lod_residency_transition_widths, // 默认继承玩法视图。
        grass_patch.view_lod_residency_transitions,         // 局部内容使用自己的边界稳定带。
        uses_patch_view_lod_config(),
    ); // 完成回退参数基线选择。
    let distance_multiplier = quality_view_distance_multiplier(); // 与常驻半径使用同一比例保持边界形状。
    return transitions * distance_multiplier; // xyz过渡宽度和w安全边距共同缩放，避免低档吞掉有效范围。
} // 完成内容级过渡读取。

fn content_camera_fade_distances() -> vec2<f32> { // 返回镜头穿入当前内容运动包围体时的整株避让区间。
    return select(
        grass_view_culling.camera_fade_lod_params.xy, // 默认沿用玩法视图近镜头口径。
        grass_patch.view_lod_ultra_fade.zw,           // 局部配置后两项保存独立避让距离。
        uses_patch_view_lod_config(),
    ); // 完成当前内容近镜头避让选择。
} // 完成整株避让距离读取。

// 每候选最多检查七次父簇，不随地图或实例规模增加单候选工作量。
fn content_lod_count() -> u32 {
    if (uses_patch_view_lod_config()) {
        return clamp(u32(grass_patch.view_lod_metadata.x), 1u, 8u);
    }
    let global_count = u32(grass_view_culling.lod_metadata.x);
    if (global_count > 0u && global_count != 3u && global_count != 4u) {
        return clamp(global_count, 1u, 8u);
    }
    // 老调用方以超远标志区分三/四档；局部配置则始终明确给出实际档数。
    return select(3u, 4u, (grass_patch.grid.w & ULTRA_LOD_CHAIN_FLAG) != 0u);
}

fn content_uses_ultra_budget() -> bool {
    return content_lod_count() >= 4u && (grass_patch.grid.w & ULTRA_LOD_CHAIN_FLAG) != 0u;
}

fn content_lod_pixel_pair(child_level: u32) -> vec2<f32> {
    let base = content_lod_pixel_thresholds();
    if (child_level == 0u) { return base.zw; }
    if (child_level == 1u) { return base.xy; }
    if (child_level == 2u) { return content_lod_ultra_pixel_thresholds(); }
    let index = child_level - 3u;
    let packed = select(
        grass_view_culling.lod_pixel_thresholds_ext[index / 2u],
        grass_patch.view_lod_pixel_thresholds_ext[index / 2u],
        uses_patch_view_lod_config(),
    ) * quality_lod_threshold_multiplier();
    return select(packed.xy, packed.zw, (index & 1u) != 0u);
}

fn content_lod_residency_radius(level: u32) -> f32 {
    if (level < 4u) { return content_lod_residency_radii()[level]; }
    let extended = select(grass_view_culling.lod_residency_radii_ext,
        grass_patch.view_lod_residency_radii_ext, uses_patch_view_lod_config());
    return extended[level - 4u] * quality_view_distance_multiplier();
}

fn content_lod_transition_width(child_level: u32) -> f32 {
    if (child_level < 3u) { return content_lod_residency_transitions()[child_level]; }
    let extended = select(grass_view_culling.lod_residency_transitions_ext,
        grass_patch.view_lod_residency_transitions_ext, uses_patch_view_lod_config());
    return extended[child_level - 3u] * quality_view_distance_multiplier();
}

fn lod_transition_salt(parent_level: u32) -> u32 {
    if (parent_level == 1u) { return MIDDLE_TO_NEAR_LOD_SALT; }
    if (parent_level == 2u) { return FAR_TO_MIDDLE_LOD_SALT; }
    if (parent_level == 3u) { return 0xbb67ae85u; }
    return 0xbb67ae85u ^ (parent_level * CANONICAL_LEVEL_STRIDE);
}

fn lod_transition_weight(pair: vec2<f32>, pixels: f32) -> f32 {
    // 目录允许相等端点表示硬切，不能对相等边界调用未定义的 smoothstep。
    if (pair.y <= pair.x) { return select(0.0, 1.0, pixels >= pair.y); }
    return smoothstep(pair.x, pair.y, pixels);
}

fn screen_space_lod_selects(fine_cell: vec2<i32>, lod_level: u32, fine_spacing: f32) -> bool {
    let count = content_lod_count();
    if (lod_level >= count) { return false; }
    let coarsest = count - 1u;
    // 从真实最粗档逐级进入子树；同一父簇的所有子候选读取同一个世界哈希。
    for (var parent_level = coarsest; parent_level > 0u; parent_level = parent_level - 1u) {
        let side = lod_parent_side(parent_level);
        let parent = floor_parent_cell(fine_cell, side);
        let center = parent_center_world(parent, side, fine_spacing);
        let child_level = parent_level - 1u;
        let residency = lod_child_residency_weight(center,
            content_lod_residency_radius(child_level),
            content_lod_transition_width(child_level),
            content_lod_residency_transitions().w);
        let weight = lod_transition_weight(content_lod_pixel_pair(child_level),
            projected_reference_pixels(center)) * residency;
        let selects_child = hash_world_cell_unit(parent, lod_transition_salt(parent_level)) < weight;
        if (lod_level == parent_level) {
            return !selects_child && (parent_level != coarsest ||
                !content_uses_ultra_budget() || ultra_budget_selects(fine_cell));
        }
        if (!selects_child) { return false; }
    }
    return lod_level == 0u; // 单档没有父簇，直接保留；多档只有全部祖先交接后才到达最细档。
}

fn camera_proximity_keep_probability(root_world: vec2<f32>, surface_y: f32) -> f32 { // 按完整风摆草叶包围体处理镜头真正穿入草冠的三维邻域。
    if (grass_view_culling.camera_position_enabled.w < SCREEN_SPACE_LOD_MODE) { // 旧测试块未启用正式相机LOD时不改变既有实例数量。
        return 1.0; // 兼容无相机或只使用视锥的通用草块。
    } // 完成正式模式判断。
    let camera_position = (grass_patch.world_to_source * vec4<f32>(grass_view_culling.camera_position_enabled.xyz, 1.0)).xyz; // 读取唯一玩法相机真实世界位置。
    let root_penetration = max(grass_patch.vegetation_bounds.w, 0.0); // 普通草三厘米、麦秆四厘米根段独立参与包围。
    let max_blade_height = max(grass_patch.vegetation_bounds.x, 0.05); // 读取当前内容最高随机叶尖保守值。
    let max_horizontal_sweep = max(grass_patch.vegetation_bounds.y, 0.01); // 读取当前原型与弯曲共同形成的水平包围。
    let closest_blade_y = clamp( // 计算相机到草叶竖直根尖线段的最近点，避免俯视高空在地面挖洞。
        camera_position.y, // 相机位于草冠内部时最近点与相机同高。
        surface_y - root_penetration, // 当前内容草根或麦根下扎段是线段下界。
        surface_y + max_blade_height, // 当前内容最高随机叶尖是保守上界。
    ); // 完成三维线段最近点高度。
    let closest_delta = vec3<f32>( // 组合相机到当前草叶竖直线段的最短向量。
        root_world.x - camera_position.x, // 水平X距离。
        closest_blade_y - camera_position.y, // 相机高于草尖时保留真实垂直距离。
        root_world.y - camera_position.z, // 水平Z距离。
    ); // 完成最短向量。
    let closest_axis_distance = length(closest_delta); // 仅最终密度与地表均通过的可见候选执行一次平方根。
    let swept_surface_distance = max(
        closest_axis_distance - max_horizontal_sweep,
        0.0,
    ); // 从镜头到竖直根尖轴距离扣除最大风摆、叶宽和近裁面余量，得到到完整运动包围体的保守距离。
    let camera_fade_distances = content_camera_fade_distances(); // 矮草和高秆内容各自使用已标定近镜头避让区间。
    return lod_transition_weight(camera_fade_distances, swept_surface_distance); // 合法相等端点使用硬切，其余保持整株平滑避让。
} // 完成不产生半根草残片的近镜头避让。

fn clustered_cell_offset(cell_seed: u32) -> vec2<f32> { // 把每个源草格稳定投向四个对角邻格之一，形成有空隙也有多株的自然占用分布。
    let selector = hash_u32(cell_seed ^ CLUSTER_REMAP_SALT); // 两个低位独立选择世界X和Z的负一或正一格偏移。
    let offset_x = f32(i32((selector & 1u) * 2u) - 1); // 零位映射为负一格，一位映射为正一格。
    let offset_z = f32(i32(((selector >> 1u) & 1u) * 2u) - 1); // 次低位用同一公式选择另一轴。
    return vec2<f32>(offset_x, offset_z); // 每个目标格最多由四个稳定源候选命中，总候选量保持完全不变。
} // 完成不新增实例、不跨多格扭曲的局部有放回式草簇重映射。

fn smooth_world_noise(world_xz: vec2<f32>, cell_meters: f32, seed: u32) -> f32 { // 以世界米制坐标生成窗口重定位后不换样的双线性值噪声。
    let grid_position = world_xz / max(cell_meters, 0.001); // 把世界位置映射到低频草簇网格。
    let base_cell = vec2<i32>(floor(grid_position)); // 负坐标同样使用向下取整取得稳定左下角格。
    let fraction = fract(grid_position); // 保留当前点在低频单元内的零至一比例。
    let blend = fraction * fraction * (vec2<f32>(3.0) - fraction * 2.0); // 三次平滑插值消除低频格边界的一阶导数跳变。
    let value_00 = hash_world_cell_unit(base_cell, seed); // 读取左下角稳定随机值。
    let value_10 = hash_world_cell_unit(base_cell + vec2<i32>(1, 0), seed); // 读取右下角稳定随机值。
    let value_01 = hash_world_cell_unit(base_cell + vec2<i32>(0, 1), seed); // 读取左上角稳定随机值。
    let value_11 = hash_world_cell_unit(base_cell + vec2<i32>(1, 1), seed); // 读取右上角稳定随机值。
    let lower = mix(value_00, value_10, blend.x); // 沿世界X平滑插值低频单元下边。
    let upper = mix(value_01, value_11, blend.x); // 沿世界X平滑插值低频单元上边。
    return mix(lower, upper, blend.y); // 沿世界Z合并为没有方块硬边的草簇值。
} // 完成稳定低频草簇噪声。

fn clustered_scale_random(cell_seed: u32, root_world: vec2<f32>) -> f32 { // 合成单株随机和相邻草共享的连续高低簇。
    let individual = hash_unit(cell_seed ^ 0x368cc8b7u); // 单株分量继续保持同一世界草格稳定。
    let cluster = smooth_world_noise( // 草簇分量只在米制世界空间缓慢变化。
        root_world, // 使用最终草根世界位置保证视觉簇跟随地面而不是窗口原点。
        APPEARANCE_CLUSTER_METERS, // 三点四米尺度形成自然小草簇而不是大片同高区域。
        grass_patch.grid.z ^ 0xb5297a4du, // 当前LOD固定种子再加独立盐，避免与位置和密度随机相关。
    ); // 完成低频草簇采样。
    return clamp(individual * 0.62 + 0.19 + (cluster - 0.5) * 0.62, 0.0, 1.0); // 单株仍占主要变化，同时让相邻草高产生可见但连续的相关性。
} // 完成用于八位通道的草簇缩放随机量。

fn candidate_intersects_gameplay_frustum(root_world: vec2<f32>) -> bool { // 在任何地表纹理读取前用保守草株球测试六平面。
    if (grass_view_culling.camera_position_enabled.w < 0.5) { // 没有唯一玩法相机或检测到多玩法相机时关闭剔除。
        return true; // 安全回退为完整保留，不能因相机配置异常制造裸地。
    } // 完成共享视锥禁用分支。
    let root_penetration = max(grass_patch.vegetation_bounds.w, 0.0); // 当前内容独立声明根部下扎深度。
    let max_blade_height = max(grass_patch.vegetation_bounds.x, 0.05); // 当前内容独立声明最高叶尖。
    var minimum_y = grass_patch.origin_extent.y - root_penetration; // 固定平面从当前内容根段开始。
    var maximum_y = grass_patch.origin_extent.y + max_blade_height; // 固定平面覆盖当前内容最高随机叶尖。
    if (grass_patch.height_decode.z >= 0.5) { // 正式高度纹理尚未读取时只能使用整张图的解码包围范围。
        minimum_y = grass_patch.height_decode.x - root_penetration; // 最低地表仍包含当前内容下扎根段。
        maximum_y = grass_patch.height_decode.x // 从高度编码最小值开始。
            + max(grass_patch.height_decode.y, 0.0) // 覆盖整张高度图可能出现的最高地表。
            + max_blade_height; // 再覆盖当前内容最高叶尖，保证坡地与沟渠不会被误剔除。
    } // 完成无需高度纹理采样的保守竖向范围。
    let center_y = (minimum_y + maximum_y) * 0.5; // 用竖向包围段中点作为保守球心高度。
    let half_height = max((maximum_y - minimum_y) * 0.5, 0.0); // 竖向半径覆盖最低草根至最高草尖。
    let sphere_radius = sqrt( // 使用包住竖向段和水平安全带的外接球。
        half_height * half_height // 保留完整高度解码跨度。
            + CONSERVATIVE_HORIZONTAL_RADIUS * CONSERVATIVE_HORIZONTAL_RADIUS, // 保留草片宽度与视锥边缘滞后余量。
    ); // 完成不会把可见草叶误判到边界外的球半径。
    let sphere_center = grass_patch.source_to_world * vec4<f32>(root_world.x, center_y, root_world.y, 1.0); // 把已稳定抖动的草根X/Z与保守Y中点组合成世界球心。
    for (var plane_index = 0u; plane_index < 6u; plane_index = plane_index + 1u) { // 固定检查左、右、上、下、近、远六平面。
        let signed_distance = dot( // Bevy平面法线朝向视锥内部，正值表示球心位于可见侧。
            grass_view_culling.frustum_planes[plane_index], // 读取当前世界半空间法线和距离。
            sphere_center, // 齐次点积同时应用平面d分量。
        ); // 完成球心到当前平面的有符号距离。
        if (signed_distance + sphere_radius <= 0.0) { // 整个保守球都落到任意一个平面外才允许剔除。
            return false; // 立刻短路，避免该候选继续执行距离、密度与高度工作。
        } // 完成当前平面的保守球相交判定。
    } // 完成全部六平面相交检查。
    return true; // 与所有平面相交或位于其内侧的候选继续原有分类链路。
} // 完成旋转稳定的逐候选玩法视锥剔除。

fn tile_intersects_gameplay_frustum(center_xz: vec2<f32>, horizontal_half_extent: f32) -> bool { // 在逐草阶段前用覆盖整个瓦片和地形高度范围的球保守粗剔除。
    if (grass_view_culling.camera_position_enabled.w < 0.5) { // 多玩法相机或缺少唯一相机时沿用完整保留回退。
        return true; // 相机输入异常不能让整个瓦片错误消失。
    } // 完成粗视锥禁用分支。
    let root_penetration = max(grass_patch.vegetation_bounds.w, 0.0); // 当前内容独立声明根部下扎深度。
    let max_blade_height = max(grass_patch.vegetation_bounds.x, 0.05); // 当前内容独立声明最高叶尖。
    var minimum_y = grass_patch.origin_extent.y - root_penetration; // 固定平面从当前内容根段开始。
    var maximum_y = grass_patch.origin_extent.y + max_blade_height; // 固定平面覆盖当前内容最高随机叶尖。
    if (grass_patch.height_decode.z >= 0.5) { // 高度纹理尚未逐草读取，因此必须覆盖整张图的解码范围。
        minimum_y = grass_patch.height_decode.x - root_penetration; // 保守包含全图最低植被根段。
        maximum_y = grass_patch.height_decode.x // 从编码最低高度开始。
            + max(grass_patch.height_decode.y, 0.0) // 覆盖全图最高地形。
            + max_blade_height; // 再覆盖当前内容最高叶尖。
    } // 完成瓦片竖向包围范围。
    let center_y = (minimum_y + maximum_y) * 0.5; // 使用全高度段中点作为球心Y。
    let half_height = max((maximum_y - minimum_y) * 0.5, 0.0); // 竖向半径包含最低根段到最高草尖。
    let tile_root_radius = horizontal_half_extent * 1.41421356237; // 瓦片中心到水平正方形角点为半边长乘根号二。
    let single_grass_radius = sqrt( // 复用逐草视锥测试覆盖的竖向段与水平形变安全带。
        half_height * half_height // 单草完整地形高度范围。
            + CONSERVATIVE_HORIZONTAL_RADIUS * CONSERVATIVE_HORIZONTAL_RADIUS, // 单草原型、风摆和数值误差水平半径。
    ); // 完成候选自身的保守球半径。
    let sphere_radius = tile_root_radius + single_grass_radius; // 两个球半径相加才严格覆盖角点候选自身包围球，禁止边缘漏草。
    let sphere_center = grass_patch.source_to_world * vec4<f32>(center_xz.x, center_y, center_xz.y, 1.0); // 组合世界空间瓦片球心。
    for (var plane_index = 0u; plane_index < 6u; plane_index = plane_index + 1u) { // 固定测试六个玩法相机平面。
        let signed_distance = dot(grass_view_culling.frustum_planes[plane_index], sphere_center); // 计算球心到当前内向平面的有符号距离。
        if (signed_distance + sphere_radius <= 0.0) { // 整个球落在任意平面外才允许剔除该瓦片。
            return false; // 省去该瓦片后续二百五十六个逐草线程。
        } // 完成当前平面保守相交测试。
    } // 完成六平面粗判。
    return true; // 可能可见的瓦片进入活跃短列表，逐草阶段仍执行精确测试。
} // 完成不会因相机旋转制造空洞的瓦片视锥粗剔除。

fn tile_intersects_lod_ring(center_xz: vec2<f32>, half_extent: f32) -> bool { // 用二维AABB到焦点的最近和最远距离保守测试当前LOD环带。
    let absolute_delta = abs(center_xz - grass_patch.focus_radius.xz); // 利用轴对齐对称性取得焦点到瓦片中心的绝对差。
    let nearest_delta = max(absolute_delta - vec2<f32>(half_extent), vec2<f32>(0.0)); // 焦点位于瓦片投影内时最近距离为零。
    let farthest_delta = absolute_delta + vec2<f32>(half_extent); // 最远角在两个轴上都远离焦点。
    let view_distance_multiplier = quality_view_distance_multiplier(); // 低中档在瓦片阶段提前减少后续候选工作量。
    let outer_radius = max(grass_patch.focus_radius.w * view_distance_multiplier, 0.01); // 与逐草阶段使用同一缩放后安全外半径。
    if (dot(nearest_delta, nearest_delta) >= outer_radius * outer_radius) { // 瓦片最近点仍在外圈之外时必然全不可见。
        return false; // 完整剔除圆形候选区之外的大量方形窗口瓦片。
    } // 完成外半径粗剔除。
    let inner_radius = clamp(grass_patch.lod_density.x * view_distance_multiplier, 0.0, outer_radius - 0.01); // 兼容环带内外半径按同一比例收缩。
    if (inner_radius > 0.0 && dot(farthest_delta, farthest_delta) <= inner_radius * inner_radius) { // 瓦片最远角仍在内洞中时没有任何候选属于本圈。
        return false; // 完整剔除由更密一级LOD负责的中心瓦片。
    } // 完成内半径粗剔除。
    return true; // 与环带边界相交的瓦片保守进入精确逐草阶段。
} // 完成无平方根的瓦片环带粗判。

fn sample_surface_height(root_world: vec2<f32>) -> vec2<f32> {
    if blade_clipmap.options.y > 0.5 { return sample_clipmap_surface(root_world); } // 分页分支精确匹配当前地形真实三角面。 // 一次读取RGBA16四角并按GLB真实对角线恢复地形高度。
    if (grass_patch.height_decode.z < 0.5) { // 未启用正式高度字段时沿用窗口参考平面。
        return vec2<f32>(grass_patch.origin_extent.y, 1.0); // 返回固定高度和有效标记。
    } // 完成固定平面回退。
    let world_min = grass_patch.height_world.xy; // 读取高度纹理世界最小X/Z角。
    let world_max = grass_patch.height_world.zw; // 读取高度纹理世界最大X/Z角。
    let inside = all(root_world >= world_min) && all(root_world <= world_max); // 地图边界继续使用闭区间判定。
    if (!inside) { // 地图外候选不得生成草。
        return vec2<f32>(grass_patch.origin_extent.y, 0.0); // 返回回退高度和无效标记。
    } // 完成地图外严格禁种。
    let dimensions_u32 = textureDimensions(grass_height_cells, 0); // 读取实际高度单元纹理尺寸。
    let dimensions = vec2<f32>(dimensions_u32); // 转为浮点供世界到单元坐标换算。
    let cell_size = (world_max - world_min) / dimensions; // 每个RGBA纹素对应一个地形单元。
    let grid_position = (root_world - world_min) / cell_size; // 把草根映射到连续地形单元坐标。
    let cell_float = clamp( // 把边界端点稳定钳制在最后一个合法单元。
        floor(grid_position), // 向下取得草根所在地形单元。
        vec2<f32>(0.0), // 防止浮点误差落到负单元。
        dimensions - vec2<f32>(1.0), // 防止最大世界端点越过纹理尺寸。
    ); // 完成合法单元坐标计算。
    let fraction = clamp(grid_position - cell_float, vec2<f32>(0.0), vec2<f32>(1.0)); // 取得单元内X/Z比例。
    let encoded_corners = textureLoad(grass_height_cells, vec2<i32>(cell_float), 0); // 一次最近读取NW、NE、SW、SE四角。
    let corners = vec4<f32>(grass_patch.height_decode.x) // 从统一最低高度开始解码。
        + encoded_corners * grass_patch.height_decode.y; // 按统一米制跨度还原四角高度。
    var height = corners.x // 先按NW、NE、SW组成的下三角插值。
        + fraction.x * (corners.y - corners.x) // 沿X方向插值NW到NE。
        + fraction.y * (corners.z - corners.x); // 沿Z方向插值NW到SW。
    if (fraction.x + fraction.y > 1.0) { // GLB真实对角线另一侧必须改用SE三角形。
        height = corners.w // 以SE角作为上三角插值基点。
            + (1.0 - fraction.y) * (corners.y - corners.w) // 从SE向NE插值。
            + (1.0 - fraction.x) * (corners.z - corners.w); // 从SE向SW插值。
    } // 完成真实三角面对角线分支。
    return vec2<f32>(height, 1.0); // 返回最终地形高度与有效标记。
} // 完成一次候选一次高度采样。

fn sample_grass_density(root_world: vec2<f32>) -> f32 { // 一次最近读取现有红通道密度概率。
    if ((grass_patch.grid.w & DENSITY_FIELD_ENABLED_FLAG) == 0u) { // 未启用密度字段时完整保留候选。
        return 1.0; // 保持旧固定草块百分百密度回退。
    } // 完成无密度图分支。
    // 父格中心进入体积覆盖域时，从同一真实外部层的三维作者格读概率。
    // 草根仍只在室外生成；隐藏高度包络只用于排除同XZ的洞底、洞顶。
    if blade_clipmap.options.y>0.5 && inside_volume_footprint(root_world) && volume_texel(0u).w < -1.5 {
        let envelope=sample_clipmap_envelope(root_world);
        if envelope.y<0.5 {return 0.0;}
        let density=volume_density_on_envelope(vec3<f32>(root_world.x,envelope.x,root_world.y));
        if density.y>0.5 {return density_with_gain(density.x);}
        // 真正洞口或另一深度的种植不是室外密度；仍读原R8，实际根由覆盖域严格裁剪。
    }
    return density_with_gain(sample_r8_density(root_world,grass_patch.density_world));
}
fn density_with_gain(raw:f32)->f32 {
    return clamp(raw*max(grass_patch.lod_density.w,0.0),0.0,1.0)*quality_density_multiplier();
}
fn sample_r8_density(root_world:vec2<f32>,world_rect:vec4<f32>)->f32 {
    let control_min = grass_patch.density_control_world.xy; // 读取地图层可选受控种植区最小X/Z角。
    let control_max = grass_patch.density_control_world.zw; // 读取地图层可选受控种植区最大X/Z角。
    let control_enabled = all(control_max > control_min); // 默认零范围表示旧地图无需额外裁剪。
    if (control_enabled && !(all(root_world >= control_min) && all(root_world <= control_max))) {
        return 0.0; // 首轮受控栖息地之外严格禁种，但不修改原始密度图内容。
    } // 完成可选世界受控区快速拒绝。
    let world_min = world_rect.xy; // 普通路径使用density_world，三维交接使用保存的原R8映射。
    let world_max = world_rect.zw;
    if any(world_max<=world_min) {return 0.0;}
    let inside = all(root_world >= world_min) && all(root_world <= world_max); // 密度图边界继续使用闭区间判定。
    if (!inside) { // 密度世界矩形外严格禁种。
        return 0.0; // 返回严格零概率避免地图外漏草。
    } // 完成密度地图外分支。
    let dimensions_u32 = textureDimensions(grass_density_map, 0); // 读取实际密度图片尺寸。
    let maximum_pixel_u32 = dimensions_u32 - vec2<u32>(1u); // 最大世界端点对应最后一个合法纹素。
    let normalized_world = clamp( // 把世界草根映射到零到一纹理范围。
        (root_world - world_min) / (world_max - world_min), // 使用与旧路径完全相同的独立密度世界范围。
        vec2<f32>(0.0), // 钳制最小边界浮点误差。
        vec2<f32>(1.0), // 钳制最大边界浮点误差。
    ); // 完成密度归一化坐标。
    let density_pixel = vec2<i32>(round( // 最近纹素保持现有内容资产已制作的宏观过渡。
        normalized_world * vec2<f32>(maximum_pixel_u32), // 把归一化坐标缩放到零至尺寸减一。
    )); // 完成合法整数密度纹素坐标。
    return textureLoad(grass_density_map, density_pixel, 0).r; // 统一在返回后应用一次增益与画质倍率。
} // 完成一次候选一次密度采样。

fn quantize_unit(value: f32) -> u32 { // 把零到一随机值稳定量化进八位通道。
    return u32(round(clamp(value, 0.0, 1.0) * f32(APPEARANCE_CHANNEL_MASK))); // 最近整数量化把视觉误差控制在半个八位步长。
} // 完成单位值量化。

fn quantize_signed_unit(value: f32) -> u32 { // 把负一到正一的旋转分量映射进八位无符号通道。
    return quantize_unit(value * 0.5 + 0.5); // 负一映射为零、正一映射为255并沿用最近整数量化。
} // 完成有符号旋转分量量化。

fn pack_appearance( // 把旋转、缩放和材质档压进一个u32。
    cell_seed: u32, // 世界草格稳定种子继续决定朝向与缩放。
    root_world: vec2<f32>, // 最终世界草根供连续草簇高度计算，窗口重定位后保持不变。
    draw_flags: u32, // grid.w包含密度启用位和近中远材质级别。
    proxy_coverage: f32, // 当前代理承担的最细叶片份额，供绘制端只缩窄水平覆盖。
) -> u32 { // 返回绘制着色器逐字节解包的紧凑外观字段。
    let scale_random = clustered_scale_random(cell_seed, root_world); // 单株与连续草簇共同控制高度，打破整片等高复制感。
    let yaw_random = hash_unit(cell_seed ^ 0x4f1bbcdcu); // 完整复用零到二π旋转的随机源。
    let yaw = yaw_random * TWO_PI - atan2(grass_patch.source_to_world[0].z, grass_patch.source_to_world[0].x); // 严格复用旧路径朝向角公式并只在每候选Compute中求值一次。
    let packed_cos = quantize_signed_unit(cos(yaw)); // 低八位保存旧旋转公式得到的余弦。
    let packed_sin = quantize_signed_unit(sin(yaw)); // 次低八位保存旧旋转公式得到的正弦。
    let packed_scale = quantize_unit(scale_random); // 次高八位保存旧缩放公式的单位随机输入。
    let lod_level = (draw_flags >> LOD_LEVEL_SHIFT) & LOD_LEVEL_MASK; // 从共享标志位读取零至七几何身份。
    let vegetation_profile =
        (draw_flags >> VEGETATION_PROFILE_SHIFT) & VEGETATION_PROFILE_MASK; // 从草窗标志恢复普通草、麦田或后续内容编号。
    let coverage_slots = max(u32(round(clamp(proxy_coverage, 0.0, 1.0) * 8.0)), 1u); // 存活代理把覆盖量化到一至八份。
    let packed_coverage = (min(coverage_slots, 8u) - 1u) & PROXY_COVERAGE_MASK; // 零至七编码可无损表示八种非零份额。
    let packed_material = (packed_coverage << PROXY_COVERAGE_SHIFT)
        | vegetation_profile
        | (lod_level << 5u); // 高三位保存几何LOD，低两位保存内容profile，中间三位保存覆盖份额。
    return packed_cos | (packed_sin << 8u) | (packed_scale << 16u) | (packed_material << 24u); // 按cos、sin、scale、material顺序组成32位外观记录。
} // 完成紧凑外观打包。

// 普通高度场草根本来就在完整源框架中；叶片也要携带源上方向，不能倾斜地形后仍世界竖直。
// 复用三维种植既有的20字节实例ABI，水平来源继续使用零法线编码。
fn source_tilt_normal() -> u32 {
    let up=normalize(grass_patch.source_to_world[1].xyz);
    if distance(up,vec3<f32>(0.0,1.0,0.0))<0.000001 {return 0u;}
    return pack4x8snorm(vec4<f32>(up,1.0));
}

fn pack_tilted_source_appearance(cell_seed:u32,root:vec2<f32>,flags:u32,coverage:f32,packed_up:u32)->u32 {
    let original=pack_appearance(cell_seed,root,flags,coverage);
    if packed_up==0u {
        // 实际draw的局部X映射为(cos,0,sin)，故源X方位应相加；不能沿用旧打包的反向yaw。
        // 只修普通源包装器，单位源仍逐位相同，体积种植继续保留原pack_appearance合同。
        let angle=hash_unit(cell_seed^0x4f1bbcdcu)*TWO_PI+atan2(grass_patch.source_to_world[0].z,grass_patch.source_to_world[0].x);
        return (original&0xffff0000u)|quantize_signed_unit(cos(angle))|(quantize_signed_unit(sin(angle))<<8u);
    }
    // 与绘制端volume_basis使用同一量化法线及最短旋转，剩余朝向在该切平面中计算。
    let up=normalize(unpack4x8snorm(packed_up).xyz);
    var tangent=vec3<f32>(1.0,0.0,0.0);
    var bitangent=vec3<f32>(0.0,0.0,-1.0);
    if up.y>=-0.999999 {
        var factor=1.0/(1.0+up.y);
        if up.y<0.0 {factor=(1.0-up.y)/dot(up.xz,up.xz);}
        tangent=vec3<f32>(1.0-up.x*up.x*factor,-up.x,-up.x*up.z*factor);
        bitangent=vec3<f32>(-up.x*up.z*factor,-up.z,1.0-up.z*up.z*factor);
    }
    let source_x=normalize(grass_patch.source_to_world[0].xyz);
    let angle=hash_unit(cell_seed^0x4f1bbcdcu)*TWO_PI+atan2(dot(bitangent,source_x),dot(tangent,source_x));
    return (original&0xffff0000u)|quantize_signed_unit(cos(angle))|(quantize_signed_unit(sin(angle))<<8u);
}

@compute @workgroup_size(64) // 每个低成本工作组并行判定六十四个候选瓦片。
fn cull_tiles(
    @builtin(global_invocation_id) global_id: vec3<u32>, // X直接对应全部粗瓦片的线性编号。
) { // 开始无需纹理读取的环带AABB与保守视锥球测试。
    let tile_index = global_id.x; // 粗剔除只使用一维直接派发，尾线程由状态总数保护。
    if (tile_index >= grass_state.total_tiles) { // 最后一个六十四线程组可能包含无效尾线程。
        return; // 无效线程不得追加列表或修改活跃计数。
    } // 完成粗瓦片边界保护。
    let grid_side = max(grass_patch.grid.x, 1u); // 读取当前正方形候选单边格数。
    let tiles_per_side = (grid_side + TILE_SIDE - 1u) / TILE_SIDE; // 与Rust使用完全相同的十六格向上取整。
    let tile_x = tile_index % tiles_per_side; // 线性编号低维映射到瓦片X列。
    let tile_z = tile_index / tiles_per_side; // 线性编号高维映射到瓦片Z行。
    let first_grid = vec2<u32>(tile_x, tile_z) * TILE_SIDE; // 取得当前瓦片左上候选格坐标。
    let last_grid = min(first_grid + vec2<u32>(TILE_SIDE - 1u), vec2<u32>(grid_side - 1u)); // 边缘瓦片钳制到真实候选末格。
    let spacing = grass_patch.origin_extent.w / max(f32(grid_side - 1u), 1.0); // 复用端点式候选世界格距。
    let half_grid = (f32(grid_side) - 1.0) * 0.5; // 候选窗口中央格仍对应世界窗口中心。
    let center_grid = (vec2<f32>(first_grid) + vec2<f32>(last_grid)) * 0.5; // 当前实际边缘尺寸下的瓦片候选中心。
    let center_xz = grass_patch.origin_extent.xz // 从窗口世界中心开始恢复瓦片中心。
        + (center_grid - vec2<f32>(half_grid)) * spacing; // 映射到绝对世界X/Z。
    let half_extent_grid = vec2<f32>(last_grid - first_grid) * 0.5 // 分别保留边缘瓦片X与Z方向的真实候选半跨度。
        + vec2<f32>(MAX_ROOT_GRID_DISPLACEMENT); // 两个轴都扩展对角邻格重映射和完整格内抖动的最大位移。
    let half_extent = max(half_extent_grid.x, half_extent_grid.y) * spacing; // 标量粗判采用较长轴，避免不足十六格的最右列低估完整Z跨度。
    if ((grass_patch.grid.w & 0x40000000u) == 0u && (!tile_intersects_lod_ring(center_xz, half_extent) // 完全位于当前环带之外的瓦片不进入短列表。
        || !tile_intersects_gameplay_frustum(center_xz, half_extent))) { // 完全位于玩法视锥外的瓦片同样直接剔除。
        return; // 两种粗判都保证无假阴性，边界瓦片留给逐草精确测试。
    } // 完成瓦片粗剔除。
    let output_index = atomicAdd(&tile_dispatch.active_count, 1u); // 为通过粗判的瓦片预留唯一连续列表槽。
    if (output_index < grass_state.total_tiles) { // 列表容量严格等于总瓦片数，仍保留防御性越界保护。
        active_tiles[output_index] = tile_index; // 只写一个u32编号，后续细分类由GPU直接读取。
    } // 完成活跃瓦片列表追加。
} // 完成GPU粗瓦片工作列表生成。

@compute @workgroup_size(1) // 每草圈一个线程把原子计数转换为合法二维间接派发参数。
fn finalize_tiles(
    @builtin(global_invocation_id) global_id: vec3<u32>, // 防御错误派发尺寸时只允许首线程发布状态。
) { // 开始生成细分类DispatchIndirectArgs和F3瓦片统计。
    if (any(global_id != vec3<u32>(0u))) { // 非零线程禁止重复写同一派发状态。
        return; // 安全忽略未来错误配置产生的额外线程。
    } // 完成单线程入口保护。
    let active_count = min(atomicLoad(&tile_dispatch.active_count), grass_state.total_tiles); // 活跃数不得越过列表容量。
    let dispatch_x = min(active_count, MAX_DISPATCH_DIMENSION); // 优先填满X维且严格遵守WebGPU上限。
    let safe_dispatch_x = max(dispatch_x, 1u); // WGSL的select不会短路，因此先保证后续除数永远非零。
    let dispatch_y = select(1u, (active_count + safe_dispatch_x - 1u) / safe_dispatch_x, dispatch_x > 0u); // 空列表保持合法Y一，非空向上展开。
    tile_dispatch.dispatch_x = dispatch_x; // 字节零供后续dispatch_workgroups_indirect直接读取。
    tile_dispatch.dispatch_y = dispatch_y; // 字节四在超大活跃列表时展开第二维。
    tile_dispatch.dispatch_z = 1u; // 当前列表是一维线性数组，Z固定为一。
    grass_state.dispatch_group_count = active_count; // 发布真实细分类工作组数量供异步诊断。
    grass_state.active_tiles = active_count; // 发布通过粗判的瓦片总数。
    grass_state.scanned_candidates = min(active_count * COMPACT_WORKGROUP_SIZE, grass_state.candidate_count); // 显示真正进入细分类的候选规模并钳制边缘尾线程。
} // 完成无需CPU回读的GPU间接派发生成。

fn flattened_workgroup_index( // 把一维或二维dispatch统一展开为连续工作组编号。
    workgroup_id: vec3<u32>, // 当前工作组的三维内建坐标。
    workgroup_count: vec3<u32>, // 本次dispatch在三个维度的实际工作组数量。
) -> u32 { // 返回按X最快、Y次之、Z最慢展开的编号。
    return workgroup_id.x // 当前行内X偏移。
        + workgroup_id.y * workgroup_count.x // 加上前面完整Y行。
        + workgroup_id.z * workgroup_count.x * workgroup_count.y; // 加上前面完整Z平面。
} // 完成支持超过65535组候选的二维展开。

@compute @workgroup_size(256) // 每个工作组为最多256个最终候选执行一次连续区预留。
fn classify_and_compact( // 主分类入口必须在Rust清零状态后、AlphaMask主通道前执行。
    @builtin(workgroup_id) workgroup_id: vec3<u32>, // 用于支持一维和二维dispatch的工作组坐标。
    @builtin(num_workgroups) workgroup_count: vec3<u32>, // 用于把二维工作组坐标还原为连续候选区间。
    @builtin(local_invocation_index) local_index: u32, // 用作工作组前缀扫描槽位和组内输出偏移。
) { // 开始当前候选的唯一一次距离、密度和高度计算。
    if (local_index < DEPTH_BUCKET_COUNT) { // 前八线程并行清零八个径向桶的工作组计数与基址。
        clear_bucket_group(local_index); // 原计数和基址同次清零；标量展开规避 Naga 29 ArrayStride。
    } // 完成桶共享状态初始化。
    if (local_index < 3u) { // 前三线程并行清零距离、密度和地表三级计数。
        clear_stage_group(local_index); // 统计只在本工作组内竞争后再一次写入全局状态。
    } // 完成阶段统计初始化。
    workgroupBarrier(); // 确保全部线程看到已经清零的共享原子状态。

    let linear_workgroup = flattened_workgroup_index(workgroup_id, workgroup_count); // 把GPU生成的一维或二维间接派发展开为活跃列表下标。
    let workgroup_is_valid = linear_workgroup < grass_state.active_tiles; // 二维间接派发最后一行可能包含不对应活跃列表的尾工作组。
    var active_tile_index = 0u; // 无效尾工作组使用确定零值并仍参与全部组内屏障。
    if (workgroup_is_valid) { // 只有合法列表下标才允许读取storage数组。
        active_tile_index = active_tiles[linear_workgroup]; // 读取粗阶段已经判定为可能可见的原始瓦片编号。
    } // 完成二维派发尾工作组安全保护。
    let grid_side = max(grass_patch.grid.x, 1u); // 保持旧路径对零单边数量的安全保护。
    let tiles_per_side = (grid_side + TILE_SIDE - 1u) / TILE_SIDE; // 与粗阶段使用同一瓦片布局。
    let tile_x = active_tile_index % tiles_per_side; // 恢复当前活跃瓦片X列。
    let tile_z = active_tile_index / tiles_per_side; // 恢复当前活跃瓦片Z行。
    let local_x = local_index % TILE_SIDE; // 二百五十六线程低维映射到瓦片内X列。
    let local_z = local_index / TILE_SIDE; // 二百五十六线程高维映射到瓦片内Z行。
    let grid_x = tile_x * TILE_SIDE + local_x; // 恢复当前候选在完整正方形中的X格。
    let grid_z = tile_z * TILE_SIDE + local_z; // 恢复当前候选在完整正方形中的Z格。
    let candidate_index = grid_z * grid_side + grid_x; // 恢复与旧世界稳定哈希一致的线性候选编号。
    let safe_candidate_count = min(grass_patch.grid.y, grass_state.candidate_count); // 双重上限防止状态与uniform短暂不同步时越界。
    let candidate_is_valid = workgroup_is_valid && grid_x < grid_side && grid_z < grid_side && candidate_index < safe_candidate_count; // 派发尾组和边缘瓦片多余线程只参与同步而不读取候选。

    if (grass_patch.grid.w & 0x40000000u) != 0u { if candidate_is_valid { classify_volume_candidate(candidate_index); } return; }
    var distance_survives = false; // 默认无效尾线程和环带外候选不通过距离阶段。
    var density_survives = false; // 默认只有先通过距离后才允许读取密度纹理。
    var surface_survives = false; // 默认只有取得合法高度的候选才写紧凑实例。
    var depth_bucket_index = 0u; // 默认无效线程归零但不会参与桶计数或实例写入。
    var compact_root = vec3<f32>(0.0); // Naga 29 Vulkan 兼容：局部位置用向量，避免给 Function 结构变量继承 Storage 成员 Offset。
    var compact_appearance = 0u; // 独立 u32 保留原外观全部位，依赖修复后可恢复局部结构而不改变实例 ABI。
    var compact_surface_normal = 0u; // 倾斜普通网格复用三维草叶的表面基底，水平地图保持原零编码。

    if (candidate_is_valid) { // 只有合法候选编号才执行世界草格与纹理逻辑。
        let half_grid = (f32(grid_side) - 1.0) * 0.5; // 端点式奇数网格以中央候选为世界中心。
        let spacing = grass_patch.origin_extent.w / max(f32(grid_side - 1u), 1.0); // 完整复用窗口边长除间隔数公式。
        let root_base_world = grass_patch.origin_extent.xz // 从当前按完整草格吸附的窗口中心开始。
            + (vec2<f32>(f32(grid_x), f32(grid_z)) - vec2<f32>(half_grid)) * spacing; // 恢复候选未抖动的绝对世界位置。
        let world_cell = vec2<i32>( // 世界格而非窗口内编号决定所有稳定随机结果。
            i32(floor(root_base_world.x / spacing + 0.5)), // X按最近完整草格编号保持窗口移动后稳定。
            i32(floor(root_base_world.y / spacing + 0.5)), // Z按最近完整草格编号保持窗口移动后稳定。
        ); // 完成绝对世界草格坐标。
        let lod_level = grass_state.lod_index; // 状态中的零一二档与绘制原型及打包材质级严格一致。
        let uses_hierarchical_lod = (grass_patch.grid.w & HIERARCHICAL_LOD_FLAG) != 0u; // 正式三级窗口显式声明同源父簇契约。
        var fine_spacing = spacing; // 无唯一正式视图时回退当前草块格距，避免禁用uniform中的零值放大世界格编号。
        if (uses_hierarchical_lod) { // 同源父簇即使首帧相机uniform未就绪也必须恢复同一个最细世界格。
            fine_spacing = spacing / lod_parent_side(lod_level); // 近中远格距严格为母格的一、二、四倍，不依赖视图生命周期。
        } else if (grass_view_culling.camera_fade_lod_params.w >= 0.001) { // 非层级正式配置提供合法统一最细格时仍可读取视图参数。
            fine_spacing = grass_view_culling.camera_fade_lod_params.w; // 多级链共同读取消费者声明的世界基格。
        } // 完成通用草块安全回退。
        let fine_cell = vec2<i32>( // 无论当前候选属于哪个LOD，都换算到同一绝对最细世界格。
            i32(floor(root_base_world.x / fine_spacing + 0.5)), // X按最近最细格编号，窗口重定位后保持不变。
            i32(floor(root_base_world.y / fine_spacing + 0.5)), // Z使用同一规则并正确覆盖负世界坐标。
        ); // 完成近中远共同的层级身份。
        var cell_seed = hash_world_cell(world_cell, grass_patch.grid.z); // 旧单圈继续使用当前窗口固定种子生成稳定候选身份。
        let legacy_remapped_root_base_world = root_base_world + clustered_cell_offset(cell_seed) * spacing; // 旧单圈保留既有自然簇重映射。
        let legacy_jitter = vec2<f32>( // 旧单圈候选继续在重映射后的目标格内执行稳定分层抖动。
            hash_unit(cell_seed ^ 0x68bc21ebu) - 0.5, // X抖动继续使用既有独立哈希盐。
            hash_unit(cell_seed ^ 0x02e5be93u) - 0.5, // Z抖动继续使用既有独立哈希盐。
        ) * spacing * ROOT_JITTER_SPAN; // 兼容路径不改变既有最小样例与单圈压力结果。
        var root_world = legacy_remapped_root_base_world + legacy_jitter; // 默认保持旧窗口世界根位置。
        var density_sample_world = root_world; // 旧单圈继续按每根真实草根采样密度。
        var density_seed = cell_seed; // 旧单圈继续使用独立候选概率哈希。
        var density_proxy_coverage = 1.0; // 兼容单圈与完整近叶默认承担百分之百原型宽度。
        if (uses_hierarchical_lod) { // 正式三级代理必须共享父簇身份、占用和有界空间范围。
            cell_seed = canonical_lod_seed(fine_cell, lod_level); // 外观不再读取近中远窗口各自不同的seed。
            root_world = canonical_lod_root(fine_cell, lod_level, fine_spacing); // 每级根只在自己的父格内抖动，禁止跨父格。
            let occupancy_side = max(4.0, lod_parent_side(lod_level)); // 前三级共用原密度父格，扩展档读取自身父格。
            let occupancy_parent = floor_parent_cell(fine_cell, occupancy_side); // 当前层级树共享唯一稳定占用单元。
            density_sample_world = parent_center_world(occupancy_parent, occupancy_side, fine_spacing); // 超远由地表承接缺口，只执行一次代表纹素读取。
        } // 完成不会在LOD替换时改变密度采样父格或跨父格跳根的正式路径。

        let view_distance_multiplier = quality_view_distance_multiplier(); // 当前候选复用统一作者范围收缩倍率。
        let outer_radius = max(grass_patch.focus_radius.w * view_distance_multiplier, 0.01); // 保持缩放后外半径严格为正避免概率范围退化。
        let focus_delta = root_world - grass_patch.focus_radius.xz; // 先取得可复用的玩家中心到草根水平向量。
        let root_distance_squared = dot(focus_delta, focus_delta); // 平方距离足以淘汰候选正方形中约七成远圈线程。
        if (root_distance_squared < outer_radius * outer_radius // 最便宜的圆外硬剔除必须先于六平面球体测试。
            && candidate_intersects_gameplay_frustum(root_world)) { // 只有圆内候选才继续检查最多六个相机平面。
            let outer_fade_width = clamp(grass_patch.lod_density.z * view_distance_multiplier, 0.01, outer_radius); // 外淡出随范围等比缩放。
            let root_distance = sqrt(root_distance_squared); // 只有圆内候选需要为平滑交叉过渡执行一次平方根。
            let outer_keep_probability = 1.0 - smoothstep( // 外边缘继续按稳定逐株概率淡出。
                outer_radius - outer_fade_width, // 外淡出从完全保留半径开始。
                outer_radius, // 到配置外半径时严格完全消失。
                root_distance, // 使用草根真实抖动后距离避免规则圆列。
            ); // 完成外圈保留概率。
            let inner_radius = clamp(grass_patch.lod_density.x * view_distance_multiplier, 0.0, outer_radius - 0.01); // 兼容内半径按同一比例收缩。
            var residency_keep_probability = outer_keep_probability; // 正式层级模式内边界由父簇LOD负责，玩家距离只保留窗口外沿安全交接。
            let uses_screen_space_lod = grass_view_culling.camera_position_enabled.w >= SCREEN_SPACE_LOD_MODE; // 模式二同时启用相机投影LOD和近镜头整株避让。
            let uses_hierarchical_screen_lod = uses_hierarchical_lod && uses_screen_space_lod; // 只有声明同源父簇的三级窗口允许执行投影替换。
            if (!uses_hierarchical_lod && inner_radius > 0.0) { // 通用旧草块始终使用配置的内外环，不受玩法视图模式影响。
                let inner_fade_width = clamp( // 保持最小草块与旧径向配置的淡入范围合法。
                    grass_patch.lod_density.y * view_distance_multiplier, // 内淡入与缩放后的兼容环带保持相同比例。
                    0.01, // 避免零宽smoothstep。
                    outer_radius - inner_radius, // 防止内淡入越过完整环带。
                ); // 完成合法内淡入宽度。
                residency_keep_probability = residency_keep_probability * smoothstep( // 旧模式内外交接仍连续，但不再传给片元打孔。
                    inner_radius, // 内半径以内严格不属于本环带。
                    inner_radius + inner_fade_width, // 淡入结束后完整交给本环带。
                    root_distance, // 使用草根真实水平距离。
                ); // 完成旧模式整株保留概率。
            } // 完成兼容环带概率。
            var lod_selected = true; // 独立单圈和旧径向环不参与跨实体代理选择。
            if (uses_hierarchical_lod) { // 正式三级窗口任意时刻只允许一个祖先层级承担当前父簇。
                if (uses_hierarchical_screen_lod) {
                    lod_selected = screen_space_lod_selects(fine_cell, lod_level, fine_spacing); // 使用分类阶段同一世界母格，按投影选择并在子级窗外回退祖先。
                } else {
                    lod_selected = lod_level + 1u == content_lod_count()
                        && (!content_uses_ultra_budget() || ultra_budget_selects(fine_cell)); // 首帧仅选择确实存在的最终祖先。
                }
            } // 完成正式父簇唯一代理与安全回退。
            let residency_selected = !probability_rejects( // 外沿过渡按完整实例稀释，地表材质已经在下方持续承接。
                hash_world_cell_unit(fine_cell, RESIDENCY_FADE_SALT), // 固定世界盐与LOD seed、片元UV和帧号无关。
                residency_keep_probability, // 正式模式仅含外沿安全淡出，旧模式兼容内外环概率。
            ); // 完成常驻边界整株选择。
            let activation_selected = !probability_rejects( // 阶段5首载同样只增加完整实例。
                hash_world_cell_unit(fine_cell, ACTIVATION_SALT), // 独立盐避免首载与LOD或常驻外沿形成相关斑块。
                clamp(grass_patch.height_decode.w, 0.0, 1.0), // 零严格不显示，一严格完整保留。
            ); // 完成首载整株激活。
            distance_survives = lod_selected && residency_selected && activation_selected; // 未被唯一LOD选中的候选不会读取密度或高度纹理。
            let bucket_distance_squared = root_distance_squared; // 固定容量按玩家等面积环预留，超距相机不得把全部实例挤入最远桶。
            let bucket_inner_radius = inner_radius; // 正式三级内环为零；兼容旧环带继续从真实内半径计算面积。
            let radial_span_squared = max(outer_radius * outer_radius - bucket_inner_radius * bucket_inner_radius, 0.0001); // 平方半径差对应当前桶口径面积且避免退化除零。
            let normalized_ring_area = clamp((bucket_distance_squared - bucket_inner_radius * bucket_inner_radius) / radial_span_squared, 0.0, 0.999999); // 等面积分桶让八个实例分区在均匀草场中负载接近。
            depth_bucket_index = min(u32(normalized_ring_area * f32(DEPTH_BUCKET_COUNT)), DEPTH_BUCKET_COUNT - 1u); // 零号桶最近、七号桶最远，供绘制阶段严格顺序消费。
        } // 完成圆外硬剔除、玩法视锥、唯一整簇LOD与整株激活的无纹理分类。

        if (distance_survives) { // 环带外候选不再读取密度和高度纹理。
            let density_probability = sample_grass_density(density_sample_world); // 正式层级共享远父格密度，旧单圈仍按真实草根读取一次。
            if (uses_hierarchical_lod) { // 正式层级把连续概率转换为父格内均匀数量并跨LOD守恒覆盖。
                let density_selection = hierarchical_density_selection(fine_cell, lod_level, density_probability); // 近中远读取同一前N名子格集合。
                density_survives = density_selection.survives; // 没有承担任何最细叶片的代理直接淘汰。
                density_proxy_coverage = density_selection.proxy_coverage; // 宽代理按实际叶片份额缩窄而不改变株高。
            } else { // 兼容单圈继续保持每候选独立伯努利选择。
                density_survives = !probability_rejects( // 使用旧独立密度哈希决定是否生草。
                    hash_unit(density_seed ^ 0x165667b1u), // 每个真实草根拥有独立且世界稳定的随机值。
                    density_probability, // 使用最近纹素和地图级增益后的最终概率。
                ); // 完成兼容候选密度阶段。
            } // 完成正式与兼容密度选择分流。
        } // 完成按距离短路的密度采样。

        if (density_survives) { // 密度淘汰候选不再读取高度纹理。
            let surface = sample_surface_height(root_world); // 每个密度存活候选只读取一次真实地形四角。
            surface_survives = surface.y >= 0.5; // 地图内或固定平面候选才允许写入紧凑实例。
            if (surface_survives) { // 只有真实地表存在时才有意义计算相机到草叶线段的三维距离。
                let camera_keep_probability = camera_proximity_keep_probability(root_world, surface.x); // 高空俯视返回一，镜头穿入草冠才在半米内逐株避让。
                surface_survives = !probability_rejects( // 使用独立稳定随机值让避让边缘由完整草株稀疏恢复。
                    hash_world_cell_unit(fine_cell, CAMERA_PROXIMITY_SALT), // 世界格和固定盐保证相机静止时没有时域闪烁。
                    camera_keep_probability, // 十八厘米内严格零，五十五厘米外严格一。
                ); // 完成近镜头整株剔除。
            } // 完成近镜头条件分支。
            if (surface_survives) { // 只有最终可绘制候选才计算并保存外观。
                compact_root = (grass_patch.source_to_world * vec4<f32>(root_world.x, surface.x, root_world.y, 1.0)).xyz; // 三个 f32 仍逐字对应原草根 X/Y/Z，不生成可变局部结构。
                compact_surface_normal = source_tilt_normal();
                compact_appearance = pack_tilted_source_appearance(cell_seed, root_world, grass_patch.grid.w, density_proxy_coverage, compact_surface_normal);
            } // 完成最终记录构造。
        } // 完成按密度短路的高度采样。
    } // 完成合法候选唯一一次完整分类。

    if (distance_survives) { // 只有通过距离与视锥判定的候选计入第一级统计。
        atomicAdd(&stage_count_0, 1u); // 工作组原子避免逐候选争用设备级状态。
    } // 完成距离阶段组内计数。
    if (density_survives) { // 只有继续通过密度图的候选计入第二级统计。
        atomicAdd(&stage_count_1, 1u); // 同一工作组内聚合密度阶段数量。
    } // 完成密度阶段组内计数。
    var local_bucket_slot = 0u; // 非存活线程保持确定零值且不会使用该槽位。
    if (surface_survives) { // 最终存活线程才进入一个且仅一个径向桶。
        atomicAdd(&stage_count_2, 1u); // 地表阶段统计与桶分配共享同一存活判定。
        local_bucket_slot = add_bucket_group(depth_bucket_index); // 取得当前工作组当前桶内唯一连续槽位。
    } // 完成低延迟工作组级桶内分配。
    workgroupBarrier(); // 确保前八线程读取到所有候选完成后的桶数量。

    if (local_index < DEPTH_BUCKET_COUNT) { // 前八线程各自负责一个桶的设备级连续区域预留。
        let group_bucket_count = load_bucket_group(local_index); // 读取当前工作组落入该桶的最终存活数。
        if (group_bucket_count > 0u) { // 空桶无需执行全局原子操作。
            store_bucket_base(local_index, atomicAdd(&depth_buckets[local_index].reserved_instances, group_bucket_count)); // 每非空桶每工作组只竞争一次设备级计数器。
        } // 完成可选全局桶分区预留。
    } // 完成八桶并行预留。
    if (local_index < 3u) { // 前三线程各自把一个组内阶段计数发布到聚合诊断状态。
        let stage_count = load_stage_group(local_index); // 读取距离、密度或地表完整组内数量。
        if (stage_count > 0u) { // 空阶段无需执行全局原子操作。
            if (local_index == 0u) { // 零号统计线程负责距离阶段。
                atomicAdd(&grass_state.distance_survivors, stage_count); // 每工作组最多一次设备级距离计数。
            } else if (local_index == 1u) { // 一号统计线程负责密度阶段。
                atomicAdd(&grass_state.density_survivors, stage_count); // 每工作组最多一次设备级密度计数。
            } else { // 二号统计线程负责最终合法地表阶段。
                atomicAdd(&grass_state.surface_survivors, stage_count); // 每工作组最多一次设备级地表计数。
            } // 完成当前阶段聚合分支。
        } // 完成非零阶段计数发布。
    } // 完成三级聚合诊断更新。
    workgroupBarrier(); // 确保所有最终存活线程看到对应桶的全局预留基址。

    if (surface_survives) { // 只有最终存活候选需要写入紧凑实例分区。
        let bucket_output_index = load_bucket_base(depth_bucket_index) + local_bucket_slot; // 当前桶内全局预留起点加工作组内唯一槽位。
        if (bucket_output_index < depth_buckets[depth_bucket_index].capacity) { // 严格禁止写出当前桶固定分区。
            let output_index = depth_buckets[depth_bucket_index].first_instance + bucket_output_index; // 把桶内偏移转换为完整实例缓冲绝对槽位。
            compact_grass[output_index] = CompactGrassInstance(compact_root.x, compact_root.y, compact_root.z, compact_appearance, compact_surface_normal); // 草根与叶片共同继承当前完整源姿态，布局仍为20字节。
        } // 超出分区时不越界写，finalize会为该桶钳制draw并汇总精确溢出量。
    } // 完成当前候选深度分桶紧凑输出。
} // 完成每候选一次分类与每工作组每非空桶一次全局预留。

// 三维作者格由GPU按真实XYZ分档、筛选和生成；实例从不回读CPU。
@group(0) @binding(12) var volume_source: texture_2d<f32>;
struct VolumeAuthorCell { value:vec4<f32>, surface:vec4<f32>, author:vec4<f32>, }
fn volume_texel(pixel:u32)->vec4<f32> {
    let size=textureDimensions(volume_source);
    return textureLoad(volume_source,vec2<i32>(i32(pixel%size.x),i32(pixel/size.x)),0);
}
fn volume_cell(index:u32)->VolumeAuthorCell {
    let header=volume_texel(0u);
    let pixel=select(0u,u32(max(header.z,0.0)),header.w<0.0)+index*3u;
    return VolumeAuthorCell(volume_texel(pixel),volume_texel(pixel+1u),volume_texel(pixel+2u));
}
fn volume_uv(point:vec3<f32>,axis:u32)->vec2<f32> {
    if axis==0u {return point.yz;} if axis==2u {return point.xy;} return point.xz;
}
fn volume_surface_point(uv:vec2<f32>,cell:VolumeAuthorCell,axis:u32)->vec3<f32> {
    let n=normalize(cell.surface.xyz); let p=cell.value.xyz;
    if axis==0u {return vec3<f32>(p.x-dot(uv-p.yz,n.yz)/n.x,uv);}
    if axis==2u {return vec3<f32>(uv,p.z-dot(uv-p.xy,n.xy)/n.z);}
    return vec3<f32>(uv.x,p.y-dot(uv-p.xz,n.xz)/n.y,uv.y);
}
fn volume_density_hash(key:vec3<i32>,step:f32,axis:u32,salt:u32)->u32 {
    return hash_u32(bitcast<u32>(key.x)*73856093u ^ bitcast<u32>(key.y)*19349663u ^ bitcast<u32>(key.z)*83492791u
        ^ bitcast<u32>(step) ^ axis*0x9e3779b9u ^ salt);
}
fn volume_density_lookup(uv:vec2<f32>,cell:VolumeAuthorCell,axis:u32)->vec2<f32> {
    let header=volume_texel(0u);
    if header.w>=0.0 {return vec2<f32>(cell.value.w,1.0);} // 兼容外部宿主既有无头三texel源。
    if any(uv<volume_uv(volume_texel(2u).xyz,axis)) || any(uv>volume_uv(volume_texel(3u).xyz,axis)) {return vec2<f32>(0.0);}
    let step=cell.surface.w;
    let point=volume_surface_point(uv,cell,axis);
    let original_key=vec3<i32>(floor(cell.author.xyz/step+vec3<f32>(0.5)));
    var delta=vec3<i32>(floor((point-cell.author.xyz)/step+vec3<f32>(0.5)));
    // UV归属来自稳定作者中心；深度按原作者层相对当前投射点恢复，LOD小幅移位不重新抽签。
    delta[axis]=i32(floor((point[axis]-cell.value[axis])/step+0.5));
    let slots=u32(header.y);let salt=u32(volume_texel(1u).x);
    for(var neighbor=0u;neighbor<5u;neighbor=neighbor+1u){
        var key=original_key+delta;
        var depth_offset=0i;
        if neighbor>0u {depth_offset=i32((neighbor+1u)/2u)*select(-1i,1i,(neighbor&1u)!=0u);}
        key[axis]=key[axis]+depth_offset;
        var offset=delta;offset[axis]=offset[axis]+depth_offset;
        let expected_center=cell.author.xyz+vec3<f32>(offset)*step;
        let base=volume_density_hash(key,step,axis,salt);
        for(var probe=0u;probe<16u;probe=probe+1u){
            let slot=(base+probe)&(slots-1u);
            let entry=u32(volume_texel(4u+slot/4u)[slot&3u]);
            if entry==0u {break;}
            let candidate=volume_cell(entry-1u);
            let candidate_key=vec3<i32>(floor(candidate.author.xyz/step+vec3<f32>(0.5)));
            if all(candidate_key==key) && (u32(candidate.author.w)&3u)==axis && candidate.surface.w==step
                && all(abs(candidate.author.xyz-expected_center)<vec3<f32>(step*0.001))
                && dot(normalize(candidate.surface.xyz),normalize(cell.surface.xyz))>0.25
                && abs(dot(candidate.value.xyz-point,normalize(cell.surface.xyz)))<step*0.75 {
                return vec2<f32>(candidate.value.w,1.0);
            }
        }
    }
    return vec2<f32>(0.0); // 有效零密度和查无承托分开；不借用当前格造成粗簇过密。
}
fn volume_density_on_envelope(point:vec3<f32>)->vec2<f32> {
    let header=volume_texel(1u);
    for(var i=0u;i<min(u32(header.y),32u);i+=1u){
        let grid=volume_texel(u32(header.z)+i);
        let center=grid.xyz+floor((point-grid.xyz)/grid.w+vec3<f32>(0.5))*grid.w;
        let reference=VolumeAuthorCell(vec4<f32>(point,0.0),vec4<f32>(0.0,1.0,0.0,grid.w),vec4<f32>(center,1.0));
        let density=volume_density_lookup(point.xz,reference,1u);
        if density.y>0.5 {return density;}
    }
    return vec2<f32>(0.0);
}
fn volume_density_at(uv:vec2<f32>,cell:VolumeAuthorCell,axis:u32)->f32 {
    // 只有与室外包络连续的朝上顶面可以跨界取R8；地下层和壁面保持自己的作者概率。
    if axis==1u && cell.surface.y>0.25 && blade_clipmap.options.y>0.5
        && (grass_patch.grid.w&DENSITY_FIELD_ENABLED_FLAG)!=0u && !inside_volume_footprint(uv) {
        let exterior=sample_clipmap_surface(uv);
        let point=volume_surface_point(uv,cell,axis);
        if exterior.y>0.5 && abs(point.y-exterior.x)<cell.surface.w*0.75 {
            return sample_r8_density(uv,grass_patch.height_world);
        }
    }
    return volume_density_lookup(uv,cell,axis).x;
}
// 混合厘米三维作者格与米级原生XZ代理时，实际母格根只由最细已知支持格拥有。
fn volume_has_finer_support(point:vec3<f32>,cell:VolumeAuthorCell,axis:u32)->bool {
    if volume_texel(0u).w>=0.0 {return false;}
    let header=volume_texel(1u);
    for(var i=0u;i<min(u32(header.y),32u);i+=1u) {
        let grid=volume_texel(u32(header.z)+i);
        if grid.w>=cell.surface.w {continue;}
        let center=grid.xyz+floor((point-grid.xyz)/grid.w+vec3<f32>(0.5))*grid.w;
        let reference=VolumeAuthorCell(vec4<f32>(point,0.0),vec4<f32>(cell.surface.xyz,grid.w),vec4<f32>(center,f32(axis)));
        if volume_density_lookup(volume_uv(point,axis),reference,axis).y>0.5 {return true;}
    }
    return false;
}
// 与室外共享世界父簇抽签、屏幕阈值、安全边距和超远预算；仅承托高度与距离使用三维面。
fn volume_lod_selects(fine_cell:vec2<i32>,level:u32,spacing:f32,cell:VolumeAuthorCell,axis:u32)->bool {
    if (grass_patch.grid.w&HIERARCHICAL_LOD_FLAG)==0u {return true;}
    let count=content_lod_count();if level>=count {return false;}
    if grass_view_culling.camera_position_enabled.w<SCREEN_SPACE_LOD_MODE {
        return level+1u==count && (!content_uses_ultra_budget() || ultra_budget_selects(fine_cell));
    }
    for(var parent=count-1u;parent>0u;parent=parent-1u){
        let side=lod_parent_side(parent);let parent_cell=floor_parent_cell(fine_cell,side);
        let center=volume_surface_point(parent_center_world(parent_cell,side,spacing),cell,axis);
        let child=parent-1u;
        let world_center=(grass_patch.source_to_world*vec4<f32>(center,1.0)).xyz;
        let world_focus=(grass_patch.source_to_world*vec4<f32>(grass_patch.focus_radius.xyz,1.0)).xyz;
        let residency=lod_distance_residency_weight(length(world_center-world_focus),
            content_lod_residency_radius(child),content_lod_transition_width(child),content_lod_residency_transitions().w);
        let weight=lod_transition_weight(content_lod_pixel_pair(child),projected_reference_pixels_at(center))*residency;
        let selects_child=hash_world_cell_unit(parent_cell,lod_transition_salt(parent))<weight;
        if level==parent {return !selects_child && (parent!=count-1u || !content_uses_ultra_budget() || ultra_budget_selects(fine_cell));}
        if !selects_child {return false;}
    }
    return level==0u;
}
fn volume_camera_keep_probability(point:vec3<f32>,normal:vec3<f32>)->f32 {
    let world_point=(grass_patch.source_to_world*vec4<f32>(point,1.0)).xyz;
    let normal_matrix=transpose(mat3x3<f32>(grass_patch.world_to_source[0].xyz,grass_patch.world_to_source[1].xyz,grass_patch.world_to_source[2].xyz));
    let world_normal=normalize(normal_matrix*normal);
    if grass_view_culling.camera_position_enabled.w<SCREEN_SPACE_LOD_MODE {return 1.0;}
    let camera=grass_view_culling.camera_position_enabled.xyz;
    let along=clamp(dot(camera-world_point,world_normal),-max(grass_patch.vegetation_bounds.w,0.0),max(grass_patch.vegetation_bounds.x,0.05));
    let distance=max(length(world_point+world_normal*along-camera)-max(grass_patch.vegetation_bounds.y,0.01),0.0);
    return lod_transition_weight(content_camera_fade_distances(),distance);
}
fn classify_volume_candidate(index:u32) {
    let divisions=max(u32(grass_patch.density_world.x),1u);let subcells=divisions*divisions;
    let header=volume_texel(0u);let size=textureDimensions(volume_source);
    let count=select(size.x*size.y/3u,u32(max(header.x,0.0)),header.w<0.0);
    if index/subcells>=count {return;}
    let cell=volume_cell(index/subcells);
    let normal=normalize(cell.surface.xyz);let axis=u32(cell.author.w)&3u;
    if abs(normal[axis])<0.2 {return;}
    let uv=volume_uv(cell.author.xyz,axis);
    let spacing=max(grass_patch.density_world.y,0.01);
    let level=(grass_patch.grid.w>>LOD_LEVEL_SHIFT)&LOD_LEVEL_MASK;
    let side=lod_parent_side(level);let current_spacing=spacing*side;
    let lower=uv-vec2<f32>(cell.surface.w*0.5);
    let sub=index%subcells;
    let parent_cell=vec2<i32>(floor(lower/current_spacing))+vec2<i32>(i32(sub%divisions),i32(sub/divisions));
    let fine_cell=parent_cell*i32(side);
    let root=canonical_lod_root(fine_cell,level,spacing);
    if any(root<lower) || any(root>=uv+vec2<f32>(cell.surface.w*0.5)) {return;}
    let point=volume_surface_point(root,cell,axis);
    if volume_has_finer_support(point,cell,axis) {return;}
    // 半开作者格可越过覆盖域边缘；按实际根裁剪，室外与三维窗口只允许一个所有者。
    if blade_clipmap.options.y>0.5 && !inside_volume_footprint(point.xz) {return;}
    let world_point=(grass_patch.source_to_world*vec4<f32>(point,1.0)).xyz;
    let world_focus=(grass_patch.source_to_world*vec4<f32>(grass_patch.focus_radius.xyz,1.0)).xyz;
    let distance=length(world_point-world_focus);
    let seed=canonical_lod_seed(fine_cell,level);
    let hierarchical=(grass_patch.grid.w&HIERARCHICAL_LOD_FLAG)!=0u;
    let multiplier=quality_view_distance_multiplier();let limit=max(grass_patch.focus_radius.w*multiplier,0.01);
    if distance>=limit || !volume_lod_selects(fine_cell,level,spacing,cell,axis) {return;}
    var residency=1.0-smoothstep(limit-clamp(grass_patch.lod_density.z*multiplier,0.01,limit),limit,distance);
    if !hierarchical && grass_patch.lod_density.x>0.0 {
        let inner=clamp(grass_patch.lod_density.x*multiplier,0.0,limit-0.01);
        residency=residency*smoothstep(inner,inner+clamp(grass_patch.lod_density.y*multiplier,0.01,limit-inner),distance);
    }
    if probability_rejects(hash_world_cell_unit(fine_cell,RESIDENCY_FADE_SALT),residency) {return;}
    atomicAdd(&grass_state.distance_survivors,1u);
    var sample_uv=root;
    if hierarchical {let occupancy_side=max(4.0,side);sample_uv=parent_center_world(floor_parent_cell(fine_cell,occupancy_side),occupancy_side,spacing);}
    let probability=density_with_gain(volume_density_at(sample_uv,cell,axis));
    var coverage=1.0;
    if hierarchical {
        let selection=hierarchical_density_selection(fine_cell,level,probability);
        if !selection.survives {return;} coverage=selection.proxy_coverage;
    } else if probability_rejects(hash_unit(seed^0x165667b1u),probability) {return;}
    if probability_rejects(hash_world_cell_unit(fine_cell,CAMERA_PROXIMITY_SALT),volume_camera_keep_probability(point,normal)) {return;}
    atomicAdd(&grass_state.density_survivors,1u); atomicAdd(&grass_state.surface_survivors,1u);
    let bucket=0u; // 三维点分布不服从二维等面积假设，共享完整输出区而非提前分割容量。
    let slot=atomicAdd(&depth_buckets[bucket].reserved_instances,1u);
    if slot>=depth_buckets[bucket].capacity {return;}
    let output=depth_buckets[bucket].first_instance+slot;
    let normal_matrix=transpose(mat3x3<f32>(grass_patch.world_to_source[0].xyz,grass_patch.world_to_source[1].xyz,grass_patch.world_to_source[2].xyz));
    let world_normal=normalize(normal_matrix*normal);
    compact_grass[output]=CompactGrassInstance(world_point.x,world_point.y,world_point.z,pack_appearance(seed,root,grass_patch.grid.w,coverage),pack4x8snorm(vec4<f32>(world_normal,1.0)));
}

@compute @workgroup_size(1) // 单线程收尾在全部分类dispatch完成后生成安全间接实例数。
fn finalize_indirect( // Rust必须为每个近中远状态各执行一次该入口。
    @builtin(global_invocation_id) global_id: vec3<u32>, // 防御错误dispatch尺寸时只允许第一个线程写间接参数。
) { // 开始把预留计数转换为DrawIndexedIndirectArgs。
    if (any(global_id != vec3<u32>(0u))) { // 非零线程不允许重复覆盖诊断与间接参数。
        return; // 安全忽略错误配置产生的额外收尾线程。
    } // 完成单线程入口保护。
    var total_reserved = 0u; // 汇总八桶原始预留量供既有F3字段继续显示最终地表存活数。
    var total_safe_instances = 0u; // 汇总八条实际间接命令能够安全消费的实例数。
    var total_overflow = 0u; // 汇总分区容量不足造成的实例丢弃量。
    for (var bucket_index = 0u; bucket_index < DEPTH_BUCKET_COUNT; bucket_index = bucket_index + 1u) { // 单线程依次生成近到远八条间接参数。
        let bucket_reserved = atomicLoad(&depth_buckets[bucket_index].reserved_instances); // 读取所有分类工作组对当前桶的最终预留总数。
        let bucket_safe_instances = min(bucket_reserved, depth_buckets[bucket_index].capacity); // 当前桶永远不能越过自己的实例分区。
        let bucket_overflow = bucket_reserved - bucket_safe_instances; // 分区不足量保持精确且不会出现下溢。
        depth_buckets[bucket_index].instance_count = bucket_safe_instances; // 写入当前三十二字节记录的索引间接实例数。
        atomicStore(&depth_buckets[bucket_index].overflow_instances, bucket_overflow); // 发布当前桶溢出供GPU调试器检查。
        total_reserved = total_reserved + bucket_reserved; // 累加聚合原始存活量。
        total_safe_instances = total_safe_instances + bucket_safe_instances; // 累加聚合实际绘制量。
        total_overflow = total_overflow + bucket_overflow; // 累加聚合容量丢弃量。
    } // 完成八桶间接参数生成。
    grass_state.instance_count = total_safe_instances; // 既有异步回读继续得到所有桶实际绘制实例总数。
    atomicStore(&grass_state.reserved_instances, total_reserved); // 聚合原始预留量保持旧诊断ABI语义。
    atomicStore(&grass_state.overflow_instances, total_overflow); // 聚合八桶溢出供F3立即报警。
} // 完成当前环带八条安全间接参数与聚合状态收尾。
