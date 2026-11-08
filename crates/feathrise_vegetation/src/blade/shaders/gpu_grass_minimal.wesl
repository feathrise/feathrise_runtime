// 通用 GPU 草地紧凑实例绘制着色器。
// 【功能设计】Compute已经完成候选、整株LOD、密度和高度计算；本文件只解包20字节实例、形变完整草叶并执行植被光照。

// 复用Bevy当前视图矩阵把绝对世界草叶位置转换到裁剪空间。
// 阴影视图只提供预通道view与globals绑定，不包含主画面的完整光照绑定组。
// position_world_to_clip在方向光阴影视图中继续读取group0 binding0。
// 风摆阴影仍需读取与主画面相同的全局时间。
import bevy_pbr::render::view_transformations::position_world_to_clip;
@if(GPU_GRASS_SHADOW) import bevy_pbr::render::mesh_view_bindings::view;
@if(GPU_GRASS_SHADOW) import bevy_render::globals::Globals;
@if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS) import bevy_pbr::render::mesh_view_bindings::view;
@if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) import bevy_pbr::render::pbr_prepass_functions::calculate_motion_vector;
@if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS) import bevy_render::globals::Globals;
@if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) import bevy_pbr::deferred::types as grass_deferred_types;
@if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) import bevy_pbr::render::rgb9e5::vec3_to_rgb9e5_;
@if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) import bevy_render::utils::octahedral_encode;
@if(!GPU_GRASS_SHADOW && !GPU_GRASS_PREPASS) import bevy_pbr::render::mesh_view_bindings as view_bindings;

@if(GPU_GRASS_SHADOW) @group(0) @binding(1) var<uniform> shadow_globals: Globals; // 对齐Bevy PrepassViewBindGroup的无运动向量布局。
// Normal/Motion/Deferred预通道共用精简视图绑定，Motion变体按需追加上一帧数据。
// 当前视图矩阵供裁剪位置与可选运动矢量换算。
// 与Bevy标准网格共用[-1,1]运动矢量语义。
// 当前风摆仍与同帧主颜色通道读取同一全局时间。
// Solari与RR必须从草叶自己的GBuffer解析材质引导，不能继续读取地下地形。
// 复用Bevy官方Rgba32Uint GBuffer量化与法线标志布局。
// 草叶无自发光，但GBuffer仍须按官方共享指数格式写零值。
// 把解析草叶世界法线编码到官方二十四位八面体表示。
// 完成只属于Deferred四附件变体的官方GBuffer工具导入。
@if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS) @group(0) @binding(1) var<uniform> prepass_globals: Globals; // binding2由官方函数读取PreviousViewUniforms。
// 主画面继续读取环境光、方向光、曝光和全局时间。
// 读取真实环境光、方向光与相机曝光完成轻量双面植被着色。
// 完成草地预通道与主画面绑定差异隔离。
// 完成主画面、运动矢量预通道与方向光阴影视图绑定隔离。

struct GrassInteraction { // 保持与Rust GpuBladeInteraction完全相同的176字节std140布局。
    wind: vec4<f32>, // xy为归一化世界风向，z为草尖最大风摆米数，w为动画速度。
    wind_phase: vec4<f32>, // x补偿天气变速和Bevy一小时回绕的相位跳变。
    interaction_bounds: vec4<f32>, // xy为全部局部源包围圆中心，z为半径，w为有效源数量。
    sources: array<vec4<f32>, 8>, // 每项xy为世界中心、z为半径、w为当前衰减强度。
} // 完成全部近中远草圈共享的固定动态输入。

struct BladeVegetationProfile { // 保持与Rust GpuBladeProfile十个vec4完全相同的160字节布局。
    geometry: vec4<f32>, // xy为最小/最大株高，z为根部下扎，w为最大静态倾斜。
    width_flexibility: vec4<f32>, // xy为最小/最大宽度倍率，zw为最硬/最软风摆响应。
    bending: vec4<f32>, // xy为交互/总弯曲上限，zw为风与交互响应倍率。
    atlas_uv: vec4<f32>, // xy为综合色图集UV缩放，zw为偏移。
    root_color: vec4<f32>, // xyz为线性根部综合色，w保留。
    tip_color: vec4<f32>, // xyz为线性尖端综合色，w保留。
    cool_tint: vec4<f32>, // xyz为单株冷色变化倍率，w表示粗级原型是否按LOD格距扩大XZ覆盖。
    warm_tint: vec4<f32>, // xyz为单株暖色变化倍率，w表示LOD3是否复用仅覆盖四乘四母格的作物远景网格。
    motion: vec4<f32>, // x为驱动类型（零风、一水流），yz为水流方向，w为水流动画速度。
    motion_params: vec4<f32>, // x为水流草尖摆幅，y为世界波形空间倍率，zw保留。
} // 完成单种叶片植被固定内容参数。

struct BladeVegetationProfiles { // 两位实例内容编号可直接索引固定四槽表。
    profiles: array<BladeVegetationProfile, 4>, // 四槽只表达批次内通用叶片外观，不附加内容类别。
} // 完成不会随候选或实例量增长的内容profile表。

@group(3) @binding(0) var<uniform> grass_interaction: GrassInteraction; // CPU每帧只覆写一份176字节风场、相位与足迹数据。
@group(3) @binding(1) var grass_base_color_texture: texture_2d<f32>; // 左半原草、右半麦叶/麦穗的共享综合色图集。
@group(3) @binding(2) var grass_base_color_sampler: sampler; // 过滤采样避免近景草片随镜头产生纹理锯齿。
@group(3) @binding(3) var<uniform> vegetation_profiles: BladeVegetationProfiles; // 全部窗口共享固定四槽高度、颜色和形变profile。
@group(3) @binding(4) var<uniform> previous_grass_interaction: GrassInteraction; // 上一完整渲染帧风场与足迹只用于预通道重建旧草叶姿态。

// 以从世界Y到表面法线的最短旋转倾斜草叶；水平体素面必须与零编码室外草完全相同。
// 不按坡度阈值切换参考轴，否则同一风场会在分块或相邻三角处突然旋转。
fn volume_basis(packed:u32)->mat3x3<f32> {
    if packed==0u {return mat3x3<f32>(vec3<f32>(1,0,0),vec3<f32>(0,1,0),vec3<f32>(0,0,1));}
    let n=normalize(unpack4x8snorm(packed).xyz);
    // 仅倒扣的精确南极没有唯一最短旋转，固定绕X翻转；snorm8其余法线不落入此分支。
    if n.y < -0.999999 {return mat3x3<f32>(vec3<f32>(1,0,0),vec3<f32>(0,-1,0),vec3<f32>(0,0,-1));}
    var factor=1.0/(1.0+n.y);
    if n.y<0.0 {factor=(1.0-n.y)/dot(n.xz,n.xz);} // 南半球避免1+n.y相消导致基底失去正交性。
    let tangent=vec3<f32>(1.0-n.x*n.x*factor,-n.x,-n.x*n.z*factor);
    let bitangent=vec3<f32>(-n.x*n.z*factor,-n.z,1.0-n.z*n.z*factor);
    return mat3x3<f32>(tangent,n,bitangent);
}

fn world_tip_to_surface(basis:mat3x3<f32>,offset:vec2<f32>)->vec2<f32> {
    // 风、水流和足迹输入本来就是世界XZ位移；先转为切向分量，再倾斜几何，不能把力再旋转一次。
    let world_offset=vec3<f32>(offset.x,0.0,offset.y);
    return vec2<f32>(dot(basis[0],world_offset),dot(basis[2],world_offset));
}

fn grass_surface_position(root:vec3<f32>,position:vec3<f32>,basis:mat3x3<f32>)->vec3<f32> {
    return root+basis*(position-root); // 主颜色、阴影、预通道与历史位置共用最终姿态。
}

struct GrassVertex { // 原始草网格顶点与Compute紧凑实例顶点流共同组成输入。
    @location(0) position: vec3<f32>, // 原始grass.glb Mesh0的局部顶点位置。
    @location(1) normal: vec3<f32>, // 保留原始网格法线位置契约供后续风与光照阶段使用。
    @location(2) uv: vec2<f32>, // 原始网格UV用于恢复草根到草尖的纵向比例。
    @location(3) root_world: vec3<f32>, // 实例slot1字节0至11依次为世界X、地表Y、世界Z。
    @location(5) surface_normal: u32,
    @location(4) packed_appearance: u32, // 实例slot1字节12保存cos、sin、scale、内容profile、覆盖份额与材质LOD。
} // 完成原型顶点加20字节实例记录输入布局。

struct GrassVertexOutput { // 顶点阶段向片元阶段传递裁剪位置、草色、法线与稳定材质参数。
    @builtin(position) @invariant clip_position: vec4<f32>, // Deferred输入与Forward覆盖必须得到逐位一致的裁剪位置，避免时间抖动下因管线变体浮点优化差异漏过深度测试并露出黑色底层。
// Motion按需插值两份位置；Normal/Deferred按需追加当前表面属性。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) @location(0) world_position: vec4<f32>, // 当前帧真实草叶世界位置。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) @location(1) previous_world_position: vec4<f32>, // 上一帧风场、足迹和时间共同重建的真实旧草叶位置。
// SSAO法线与Deferred表面必须复用同一份动态草叶解析法线。
    @if(GPU_GRASS_PREPASS && NORMAL_PREPASS_OR_DEFERRED_PREPASS) @location(2) world_normal: vec3<f32>, // 插值后的世界法线供NormalPrepass或GBuffer片元编码。
// RR活动的Deferred视图还需要与最终颜色属于同一草叶几何和基础材质的表面引导。
    @if(GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) @location(3) color: vec3<f32>, // 当前实例稳定色调供Deferred片元重建无光照综合色。
    @if(GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) @location(4) blade_uv: vec2<f32>, // 与主颜色使用同一图集坐标和根尖综合色。
    @if(GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) @location(5) @interpolate(flat) vegetation_profile_index: u32, // 整个primitive稳定选择普通草或作物profile。
// 完成可选运动位置、法线与Deferred材质插值布局。
    @if(!GPU_GRASS_PREPASS) @location(0) color: vec3<f32>, // 当前实例稳定随机色调乘数，基础草色由原纹理决定。
    @if(!GPU_GRASS_PREPASS) @location(1) world_normal: vec3<f32>, // 已随实例朝向旋转的世界法线供双面植被光照使用。
    @if(!GPU_GRASS_PREPASS) @location(2) blade_uv: vec2<f32>, // 原始纵向UV只用于保留官方纹理的根尖明暗层次。
    @if(!GPU_GRASS_PREPASS) @location(3) @interpolate(flat) vegetation_profile_index: u32, // 片元无插值读取当前普通草或麦田profile及图集区域。
// 完成预通道最小插值与主画面完整插值隔离。
} // 完成绘制阶段插值输出布局。

const APPEARANCE_CHANNEL_MASK: u32 = 0x000000ffu; // 四个外观分量都按八位无符号通道解包。
const STABLE_APPEARANCE_MASK: u32 = 0x00ffffffu; // 低二十四位只含世界草格决定的旋转与缩放，可跨窗口重定位稳定派生全部外观随机数。
const VEGETATION_PROFILE_MASK: u32 = 0x3u; // 最高外观字节低两位保存零至三内容profile编号。
const PROXY_COVERAGE_SHIFT: u32 = 26u; // 最高外观字节中间三位在完整u32中的起始位。
const PROXY_COVERAGE_MASK: u32 = 0x7u; // 三位覆盖编码表示八分之一至完整代理宽度。
const GEOMETRY_LOD_SHIFT: u32 = 29u; // 紧凑外观最高三位保存几何身份 0..=7。
const GEOMETRY_LOD_MASK: u32 = 0x7u; // 三位几何身份支持八档可变层级。
const REUSED_CROP_ULTRA_HORIZONTAL_SCALE: f32 = 4.0; // 复用远景网格的超远作物需以四倍水平覆盖代表六十四个最细格。
const INV_PI: f32 = 0.31830988618; // 漫反射光照使用一除以π保持与标准PBR能量量级一致。
const TWO_PI: f32 = 6.28318530718; // 中心重合时用稳定草根哈希构造任意水平压草方向。
const INTERACTION_SOURCE_CAPACITY: u32 = 8u; // 必须与Rust固定玩家和足迹源容量严格一致。

fn unpack_unit_channel(packed: u32, shift: u32) -> f32 { // 从指定八位通道恢复零到一单位值。
    return f32((packed >> shift) & APPEARANCE_CHANNEL_MASK) / f32(APPEARANCE_CHANNEL_MASK); // 使用255除数恢复Compute最近整数量化结果。
} // 完成单个八位单位通道解包。

fn unpack_signed_channel(packed: u32, shift: u32) -> f32 { // 从指定八位通道恢复负一到正一旋转分量。
    return unpack_unit_channel(packed, shift) * 2.0 - 1.0; // 逆转Compute中负一到正一映射为零到一的公式。
} // 完成单个有符号旋转通道解包。

fn reused_crop_ultra_horizontal_scale(
    profile: BladeVegetationProfile,
    packed_appearance: u32,
) -> f32 { // 只补偿显式声明复用远景网格的作物超远档单位面积轮廓。
    let geometry_lod = (packed_appearance >> GEOMETRY_LOD_SHIFT) & GEOMETRY_LOD_MASK; // 从最高三位恢复当前几何身份 0..=7。
    let reuses_far_crop_mesh = profile.warm_tint.w >= 0.5; // 内容profile显式标记远景原型复用关系，避免槽位复用时误吃额外补偿。
    return select(
        1.0,                                    // 普通草及作物近中远档保持原资产覆盖。
        REUSED_CROP_ULTRA_HORIZONTAL_SCALE,     // 作物超远档从十六格代理扩展为六十四格代理。
        geometry_lod >= 3u && reuses_far_crop_mesh, // 只允许第三级复用作物命中补偿，普通草专用超远资产不受影响。
    ); // 完成不改变株高、综合色和实例数量的水平覆盖补偿。
} // 完成作物第3档到第4档的几何覆盖守恒换算。

fn profile_proxy_horizontal_scale(profile: BladeVegetationProfile, packed_appearance: u32) -> f32 { // 让显式启用的粗级原型随世界格距扩展水平覆盖，类型必须与本文件实际profile声明一致。
    let geometry_lod = (packed_appearance >> GEOMETRY_LOD_SHIFT) & GEOMETRY_LOD_MASK; // 从最高三位恢复当前几何身份 0..=7。
    let grid_scale = f32(1u << min(geometry_lod, 7u)); // 启用代理覆盖的内容按实际档位扩展XZ，不改变株高。
    return select(
        1.0,       // 普通草和两类作物保持既有水平覆盖与已验收画面。
        grid_scale, // 芦苇与湿地草只扩大草簇XZ代理，不改变株高、颜色、密度概率或实例数量。
        profile.cool_tint.w >= 0.5, // 内容profile显式打开尾槽标志后才应用四级覆盖守恒。
    ); // 完成由profile控制的粗级代理覆盖守恒。
} // 结束四级水平覆盖换算。

fn hash_u32(input: u32) -> u32 { // 绘制阶段为草色、形态、风相位和交互方向生成稳定整数哈希。
    var value = input; // 使用局部可变值执行与Compute相同的高低位混合。
    value = value ^ (value >> 16u); // 第一次高低位异或扩散相邻世界根坐标。
    value = value * 0x7feb352du; // 第一奇数乘法扩大不同草根的随机间距。
    value = value ^ (value >> 15u); // 第二次异或继续混合高位信息。
    value = value * 0x846ca68bu; // 第二奇数乘法避免规则草格形成条纹。
    value = value ^ (value >> 16u); // 最终异或得到完整三十二位稳定结果。
    return value; // 返回不依赖帧号或相机的世界稳定哈希。
} // 完成绘制阶段整数哈希。

fn hash_unit(input: u32) -> f32 { // 把整数哈希映射为零至一草色或遮罩阈值。
    return f32(hash_u32(input) & 0x00ffffffu) / 16777215.0; // 使用二十四位尾数取得足够细的稳定单位值。
} // 完成绘制阶段单位随机值。

fn stable_appearance_hash(packed_appearance: u32) -> u32 { // 从不包含材质LOD的低二十四位恢复绘制阶段统一随机种子。
    return hash_u32((packed_appearance & STABLE_APPEARANCE_MASK) ^ 0x4f1bbcdcu); // 禁止再哈希草根f32位模式，消除窗口吸附跨界时的颜色和风相位跳变。
} // 完成二十字节实例ABI内的跨窗口稳定随机契约。

fn independent_width_scale( // 高度和宽度使用不同随机轴，避免全部植株只做同一比例缩放。
    profile: BladeVegetationProfile, // 当前普通草或麦田独立声明合法宽度范围。
    shape_variation: u32, // 稳定随机字节决定当前植株在内容范围内的宽窄。
    height_random: f32, // 高度随机只提供很弱相关性而不直接等比缩放。
) -> f32 {
    let width_random = unpack_unit_channel(shape_variation, 0u); // 第一变化字节产生与草高无直接线性关系的叶片宽窄变化。
    let height_correlation = mix(0.92, 1.08, height_random); // 只保留很弱的高草略宽关系，维持原模型辨识度。
    return mix(profile.width_flexibility.x, profile.width_flexibility.y, width_random)
        * height_correlation; // 输出当前内容profile约束的有界宽度。
} // 完成独立叶宽随机化。

fn blade_flexibility(profile: BladeVegetationProfile, shape_variation: u32) -> f32 { // 每种内容仍保留单株稳定软硬差。
    return mix( // 第二变化字节在当前profile的最硬与最软响应之间选择。
        profile.width_flexibility.z, // 麦秆下界显著低于软草，保持直立茎部。
        profile.width_flexibility.w, // 当前内容最软植株仍受自己的风响应倍率限制。
        unpack_unit_channel(shape_variation, 8u), // 使用与宽度独立的稳定随机字节。
    );
} // 完成单株柔韧度随机化。

fn static_tip_offset(profile: BladeVegetationProfile, shape_variation: u32) -> vec2<f32> { // 为每株植被生成不随时间变化的轻微天然倾斜。
    let lean_angle = unpack_unit_channel(shape_variation, 16u) * TWO_PI; // 第三变化字节决定方向并避免草片朝向与倒向锁死。
    let lean_amount = mix(
        min(0.012, profile.geometry.w), // 最大倾斜很小的硬秆不会被固定下限反向放大。
        profile.geometry.w, // 第四变化字节允许少数植株达到当前内容的最大静态倾斜。
        unpack_unit_channel(shape_variation, 24u),
    );
    return vec2<f32>(cos(lean_angle), sin(lean_angle)) * lean_amount; // 返回世界水平米制草尖静态偏移。
} // 完成单株静态倾斜。

fn grass_global_time() -> f32 { // 主画面和阴影通道必须使用同一帧全局时间，避免影子与风摆草叶错位。
// 方向光阴影视图使用精简预通道绑定布局。
    @if(GPU_GRASS_SHADOW) return shadow_globals.time; // group0 binding1提供Bevy当前回绕时间。
// 运动矢量预通道同样使用精简视图布局。
    @if(!GPU_GRASS_SHADOW && GPU_GRASS_PREPASS) return prepass_globals.time; // 当前草叶深度必须与随后主通道使用同一帧风摆姿态。
// 主画面使用完整MeshView绑定布局。
    @if(!GPU_GRASS_SHADOW && !GPU_GRASS_PREPASS) return view_bindings::globals.time; // 复用原有主画面全局时间来源。
// 完成预通道与主画面时间绑定选择。
// 完成三类视图时间绑定选择。
} // 结束草叶共享全局时间读取。

// 只有运动矢量预通道需要重建上一帧程序动画时间。
@if(GPU_GRASS_PREPASS) fn grass_previous_global_time() -> f32 {
    let previous_time = prepass_globals.time - prepass_globals.delta_time; // Bevy Globals每小时回绕，上一帧可能落在回绕前。
    return select(previous_time, previous_time + 3600.0, previous_time < 0.0); // 历史风参数尚未加回绕补偿，须用上一帧真实时刻重建运动向量。
}
// 主颜色与阴影无需为历史姿态保留额外时间函数。

fn instance_color_tint(profile: BladeVegetationProfile, surface_variation: u32) -> vec3<f32> { // 按内容色盘生成稳定明暗和冷暖变化。
    let brightness = mix(0.84, 1.12, unpack_unit_channel(surface_variation, 0u)); // 第一表面变化字节形成小幅明暗层次但不制造斑点噪声。
    let temperature = unpack_unit_channel(surface_variation, 8u); // 第二表面变化字节控制冷暖并与高度、形态或LOD身份解耦。
    return mix(profile.cool_tint.xyz, profile.warm_tint.xyz, temperature)
        * brightness; // 普通草保持冷暖绿，麦田则在青绿与金黄之间变化。
} // 完成独立草色变化。

fn wind_tip_offset( // 为单根草计算低成本多尺度世界连续风场。
    interaction: GrassInteraction, // 当前或上一帧完整风参数由调用方明确选择，禁止运动矢量误读本帧值。
    root_world_xz: vec2<f32>, // 世界绝对坐标保证草窗移动和相机旋转不改变风场位置。
    micro_phase_random: f32, // 已从共享变化值解包的稳定单位相位，避免在每个顶点重复完整哈希。
    flexibility: f32, // 单株柔韧度控制接受风力的比例而不改变全局滑块含义。
    profile: BladeVegetationProfile, // 当前内容独立缩放风响应，成熟麦秆不会继承软草全幅摆动。
    animation_time: f32, // 主通道传当前时间，运动矢量旧姿态传严格上一帧时间。
) -> vec2<f32> { // 返回不超过风力滑块最大米制摆幅的动态草尖偏移。
    let uses_water_flow = profile.motion.x >= 0.5; // 每个批次局部profile可独立选择世界风或开发者水流。
    let wind_vector = select(interaction.wind.xy, profile.motion.yz, uses_water_flow); // 水草不受全局风向改变，其他叶片保持既有风场。
    let wind_length = length(wind_vector); // 防御外部错误写入零向量，避免normalize产生NaN。
    let tip_strength = select(interaction.wind.z, profile.motion_params.x, uses_water_flow); // 水流摆幅与世界风力滑块正交。
    if (wind_length <= 0.000001 || tip_strength <= 0.0) { // 无方向或无摆幅时不执行三层波动计算。
        return vec2<f32>(0.0); // 零风严格保持只有静态倾斜与玩家踩踏。
    } // 完成安全早退。
    let wind_direction = wind_vector / wind_length; // 用显式长度归一化风向。
    let cross_direction = vec2<f32>(-wind_direction.y, wind_direction.x); // 垂直风向承载阵风横摆和细碎抖动。
    let animation_speed = select(interaction.wind.w, profile.motion.w, uses_water_flow); // 水流拥有独立速度，风驱动继续读取全局速度。
    let spatial_scale = select(1.0, profile.motion_params.y, uses_water_flow); // 开发者可调水流波长而不增加纹理采样。
    let wrapped_time = animation_time * animation_speed + select(interaction.wind_phase.x, 0.0, uses_water_flow); // 风速变化保持相位，水草仍使用独立水流动画。
    let macro_wave = sin( // 大尺度波形成远处也可读的缓慢风带。
        dot(root_world_xz, wind_direction * (0.052 * spatial_scale)) + wrapped_time * 0.62,
    ); // 完成约百米尺度主风波。
    let gust_wave = sin( // 中尺度阵风斜穿主风方向并受主波轻微调制，减少周期性平行条带。
        dot(root_world_xz, vec2<f32>(-0.137, 0.181) * spatial_scale) + wrapped_time * 1.13 + macro_wave * 0.45,
    ); // 完成数十米尺度阵风波。
    let micro_phase = micro_phase_random * TWO_PI; // 单株稳定相位让近景叶尖不再完全同步。
    let micro_wave = sin( // 微尺度波只占很小摆幅，避免逐株高速乱抖。
        dot(root_world_xz, vec2<f32>(0.47, -0.39) * spatial_scale) + wrapped_time * 2.35 + micro_phase,
    ); // 完成近景细碎叶尖变化。
    let gust_envelope = smoothstep(-0.25, 0.86, gust_wave); // 把正弦阵风整形成有静风间隔的连续脉冲。
    let along_strength = clamp( // 沿主风位移由常风、宏观波、阵风和极少微波共同组成。
        0.23 + macro_wave * 0.18 + gust_envelope * 0.52 + micro_wave * 0.07,
        0.08,
        1.0,
    ); // 完成零点零八至一的有界主风强度。
    let cross_strength = gust_wave * 0.10 + micro_wave * 0.045; // 横向摆动足以打散梳齿感但不让草尖绕圈。
    var normalized_field = wind_direction * along_strength + cross_direction * cross_strength; // 合成未钳制的二维风场方向和幅度。
    let field_length = length(normalized_field); // 横风叠加后可能略超全局最大幅度，需要恢复滑块上限契约。
    if (field_length > 1.0) { // 仅在极端同相波峰时执行归一化。
        normalized_field = normalized_field / field_length; // 保持方向并把最终场幅限制为一。
    } // 完成风场单位上限。
    return normalized_field
        * tip_strength
        * flexibility
        * profile.bending.z; // 全局风力先经单株软硬，再乘当前内容风响应倍率。
} // 完成不新增纹理采样的宏观、阵风和单株微摆三层风场。

fn interaction_tip_offset( // 只在固定足迹包围圆内计算最多八个局部源。
    interaction: GrassInteraction, // 当前或上一帧源数组由调用方显式传入，移动玩家不再被误报为静止草叶。
    root_world_xz: vec2<f32>, // 当前单根草根世界X/Z位置。
    root_hash: u32, // 草根恰好位于交互中心时提供稳定非零倒伏方向。
    profile: BladeVegetationProfile, // 麦秆等硬质内容按自己的交互响应和最大弯曲限制倒伏。
) -> vec2<f32> { // 返回当前单根草由玩家与短寿命足迹产生的草尖水平位移。
    let active_count = min( // f32小整数由CPU写入，可安全还原成零至八循环上限。
        u32(round(interaction.interaction_bounds.w)),
        INTERACTION_SOURCE_CAPACITY,
    ); // 完成固定数组安全上限保护。
    if (active_count == 0u || interaction.interaction_bounds.z <= 0.0) {
        return vec2<f32>(0.0); // 非草地图或玩家尚未生成时不进入任何足迹距离循环。
    }
    let bounds_delta = root_world_xz - interaction.interaction_bounds.xy; // 先计算当前草根到该帧全部足迹包围圆中心的距离。
    if (dot(bounds_delta, bounds_delta) > interaction.interaction_bounds.z * interaction.interaction_bounds.z) {
        return vec2<f32>(0.0); // 绝大多数中远景草只付出一次点积即可跳过八源循环。
    }
    var accumulated_bend = vec2<f32>(0.0); // 局部重叠源按方向和衰减权重合成。
    for (var source_index = 0u; source_index < active_count; source_index = source_index + 1u) {
        let source = interaction.sources[source_index]; // 读取调用帧连续有效源前缀中的位置、半径与强度。
        let source_delta = root_world_xz - source.xy; // 草叶始终朝远离玩家或足迹中心的方向倒伏。
        let distance_squared = dot(source_delta, source_delta); // 平方距离先执行廉价圆外剔除。
        if (distance_squared >= source.z * source.z) {
            continue; // 当前源范围外不执行平方根和归一化。
        }
        let distance_to_source = sqrt(max(distance_squared, 0.000001)); // 圆内才计算真实距离供平滑边缘使用。
        var away_direction = source_delta / distance_to_source; // 正常草根使用几何远离方向。
        if (distance_squared <= 0.000001) {
            let fallback_angle = hash_unit(root_hash ^ (source_index * 0x9e3779b9u)) * TWO_PI; // 中心重合时使用世界稳定随机角度。
            away_direction = vec2<f32>(cos(fallback_angle), sin(fallback_angle)); // 保证中心草也能倒伏且不随帧闪烁。
        }
        let radial_falloff = 1.0 - smoothstep(source.z * 0.18, source.z, distance_to_source); // 中心保持强弯曲，边缘连续回到直立。
        accumulated_bend = accumulated_bend + away_direction * radial_falloff * source.w; // 当前源寿命已由CPU折算进w强度。
    }
    let accumulated_length = length(accumulated_bend); // 多个相邻足迹可能重叠，需要统一幅度钳制。
    if (accumulated_length <= 0.000001) {
        return vec2<f32>(0.0); // 所有包围圆内源实际都未命中时保持直立。
    }
    return accumulated_bend / accumulated_length
        * min(accumulated_length, 1.0)
        * profile.bending.x
        * profile.bending.w; // 重叠足迹加强覆盖但不超过当前内容交互弯曲预算。
} // 完成逐草根独立响应的玩家踩踏形变。

// 阴影只需要几何轮廓，不采样综合色或编码材质表面。
@if(!GPU_GRASS_SHADOW) fn vegetation_surface_color( // 主颜色与Deferred GBuffer必须共享完全相同的无光照草叶综合色。
    profile: BladeVegetationProfile, // 当前内容提供图集区域与根尖综合色。
    blade_uv: vec2<f32>, // 原型UV决定图集采样点和根尖渐变位置。
    instance_tint: vec3<f32>, // 世界稳定随机值只产生有界明暗与冷暖变化。
) -> vec3<f32> {
    let root_to_tip = 1.0 - blade_uv.y; // 保持官方UV方向的草根到草尖比例。
    let atlas_uv = blade_uv * profile.atlas_uv.xy + profile.atlas_uv.zw; // 普通草与作物读取各自图集区域。
    let texture_color = textureSample( // 正式颜色纹理已由Bevy图片资产从sRGB自动解码到线性空间。
        grass_base_color_texture, // 所有内容共享消费者发布的综合色图集。
        grass_base_color_sampler, // 复用图片资产过滤采样器避免近景锯齿。
        atlas_uv, // 使用当前内容缩放和偏移后的最终UV。
    ).rgb; // 图集只提供纵向明暗细节，不直接覆盖内容色盘。
    let texture_luminance = dot(texture_color, vec3<f32>(0.2126, 0.7152, 0.0722)); // 以Rec.709亮度保留纹理结构。
    let palette_position = smoothstep(0.02, 0.98, root_to_tip); // 根尖颜色连续过渡，禁止统一亮尖硬边。
    let natural_palette = mix( // 平方曲线让中段保持深色，再平滑走向亮尖。
        profile.root_color.xyz, // 普通草深绿根或作物深橄榄下茎。
        profile.tip_color.xyz, // 普通草亮尖或成熟麦穗金黄尖端。
        palette_position * palette_position, // 把较亮色集中在上部而非整根草片。
    ); // 完成当前内容无光照根尖色盘。
    let texture_detail = mix(0.82, 1.28, clamp(texture_luminance * 3.2, 0.0, 1.0)); // 限制纹理明暗，避免色相和能量失控。
    return natural_palette * texture_detail * instance_tint; // 返回主颜色和RR diffuse_albedo共同消费的线性综合色。
} // 完成草叶单一无光照材质来源。

// 只有四附件Deferred变体需要手工编码Bevy官方GBuffer。
@if(!GPU_GRASS_SHADOW && GPU_GRASS_DEFERRED_SURFACE) const DEFERRED_FLAGS_GPU_VEGETATION_BIT: u32 = 1u << 7u; // Bevy 0.19只占bit0..2；项目用bit7标识GPU植被，升级Deferred ABI时必须复审。
@if(!GPU_GRASS_SHADOW && GPU_GRASS_DEFERRED_SURFACE) fn vegetation_deferred_gbuffer( // 把纯漫反射草叶表面编码为Solari与RR可解析的Rgba32Uint像素。
    base_color: vec3<f32>, // 必须与随后主颜色通道实际照明前的综合色完全一致。
    world_normal: vec3<f32>, // 作为Solari/RR表面引导；Forward最终颜色另用薄叶双面光照语义。
) -> vec4<u32> {
    let base_color_srgb = pow(max(base_color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)); // 对齐Bevy Deferred存储sRGB、解析时再转线性的契约。
    let base_roughness = grass_deferred_types::pack_unorm4x8_( // GBuffer第一字保存基础色与感知粗糙度。
        vec4<f32>(base_color_srgb, 1.0), // 自定义草光照无镜面项，因此以完全粗糙表面描述最终颜色。
    ); // 完成基础色与粗糙度量化。
    let emissive = vec3_to_rgb9e5_(vec3<f32>(0.0)); // 草叶没有自发光，禁止把稳定综合色误交给RR当作无限能量。
    let material_props = grass_deferred_types::pack_unorm4x8_( // GBuffer第三字保存反射率、金属度、遮蔽和清漆。
        vec4<f32>(0.0, 0.0, 1.0, 0.0), // 纯漫反射、非金属、无遮挡且无清漆，与自定义Lambert主颜色一致。
    ); // 完成无镜面草叶材质属性量化。
    let encoded_normal = octahedral_encode(normalize(world_normal)); // 使用官方八面体法线编码保持Solari解析一致。
    let normal_and_flags = grass_deferred_types::pack_24bit_normal_and_flags( // GBuffer末字保存二十四位法线和八位Deferred标志。
        encoded_normal, // 当前帧解析弯曲世界法线。
        4u | DEFERRED_FLAGS_GPU_VEGETATION_BIT, // 同时保留官方shadow receiver与项目植被类别，法线低二十四位不变。
    ); // 完成法线与Deferred表面标志编码。
    return vec4<u32>(base_roughness, emissive, material_props, normal_and_flags); // 返回与Bevy Rgba32Uint GBuffer ABI一致的完整像素。
} // 完成草叶Deferred GBuffer编码。
// 结束只供Solari/RR使用的草叶材质表面编码。
// 结束非阴影材质辅助函数。

fn reconstructed_grass_world_normal( // 主颜色与Deferred表面共享同一随风弯曲的解析法线。
    vertex: GrassVertex, // 原型顶点提供资产法线和草簇内部叶片宽轴。
    yaw_cos: f32, // 当前实例世界Y旋转余弦。
    yaw_sin: f32, // 当前实例世界Y旋转正弦。
    combined_tip_offset: vec2<f32>, // 当前帧合成并钳制后的世界水平草尖位移。
    bend_derivative: f32, // 当前顶点弯曲曲线沿根尖参数的一阶导数。
    blade_height: f32, // 当前随机株高决定未弯曲脊线竖向尺度。
    root_penetration: f32, // 根部下扎段参与完整脊线高度。
    vertical_tip_drop: f32, // 叶长补偿产生的当前帧草尖竖向回落。
    blade_root_to_tip: f32, // 根尖位置控制解析法线的向上曲面混合量。
) -> vec3<f32> {
    let rotated_normal_xz = vec2<f32>( // 原模型法线使用与草片位置相同的世界Y轴旋转。
        vertex.normal.x * yaw_cos - vertex.normal.z * yaw_sin, // 计算旋转后法线世界X。
        vertex.normal.x * yaw_sin + vertex.normal.z * yaw_cos, // 计算旋转后法线世界Z。
    ); // 完成当前实例法线水平旋转。
    var world_normal = vec3<f32>(rotated_normal_xz.x, vertex.normal.y, rotated_normal_xz.y); // 组合旋转后的三维资产法线。
    if (dot(world_normal, world_normal) < 0.000001) { // 防止损坏资产零法线在normalize时产生NaN。
        world_normal = vec3<f32>(0.0, 1.0, 0.0); // 零法线安全回退为向上草叶法线。
    } else { // 合法原始法线保持自身分段曲面方向。
        world_normal = normalize(world_normal); // 插值前统一归一化减少缩放或量化误差。
    } // 完成安全世界法线准备。
    var local_width_xz = vec2<f32>(vertex.normal.z, -vertex.normal.x); // 从资产法线恢复草簇内部当前叶片的水平宽轴。
    if (dot(local_width_xz, local_width_xz) < 0.000001) { // 损坏或恰好竖直的法线不能进入零向量归一化。
        local_width_xz = vec2<f32>(1.0, 0.0); // 回退到旧单片草局部X宽轴以保持有限结果。
    } else { // 合法宽轴只需保留方向。
        local_width_xz = normalize(local_width_xz); // 非均匀缩放不应改变受光强度。
    } // 完成簇内叶片宽轴校验。
    let blade_width_axis = vec3<f32>( // 把资产局部宽轴再随当前实例统一yaw旋转到世界空间。
        local_width_xz.x * yaw_cos - local_width_xz.y * yaw_sin, // 计算宽轴世界X分量。
        0.0, // 草叶宽轴保持水平，弯曲竖向变化由脊线切线承担。
        local_width_xz.x * yaw_sin + local_width_xz.y * yaw_cos, // 计算宽轴世界Z分量。
    ); // 完成解析法线宽轴输入。
    let bent_spine_tangent = normalize(vec3<f32>( // 直接从实际弯曲曲线导数恢复叶片中心脊线切线。
        combined_tip_offset.x * bend_derivative, // 世界X随根尖参数的水平变化率。
        blade_height + root_penetration - vertical_tip_drop * bend_derivative, // 世界Y包含完整株高与叶长补偿导数。
        combined_tip_offset.y * bend_derivative, // 世界Z使用同一水平形变曲线。
    )); // 完成不依赖额外网格属性的弯曲脊线切线。
    var analytic_normal = cross(blade_width_axis, bent_spine_tangent); // 宽轴与脊线切线叉乘得到真实弯曲叶面法线。
    if (dot(analytic_normal, analytic_normal) < 0.000001) { // 极端弯曲时必须避免归一化零向量。
        analytic_normal = world_normal; // 退回已校验资产法线保持有限受光结果。
    } else { // 正常叶片使用解析弯曲方向。
        analytic_normal = normalize(analytic_normal); // 统一解析法线长度。
    } // 完成解析法线安全保护。
    if (dot(analytic_normal, world_normal) < 0.0) { // 不同面片绕序必须保持与资产法线同半球。
        analytic_normal = -analytic_normal; // 翻转符号避免相邻三角产生黑亮断层。
    } // 完成解析法线朝向统一。
    world_normal = normalize(mix(world_normal, analytic_normal, 0.62)); // 保留资产曲面细节并让主体受光跟随实际弯曲。
    return normalize(mix( // 用向上分量近似圆润叶束并返回最终世界法线。
        world_normal, // 保留融合后的资产与解析弯曲法线作为主要方向。
        vec3<f32>(0.0, 1.0, 0.0), // 朝上分量形成柔和植被曲面受光。
        0.24 + blade_root_to_tip * 0.12, // 草尖略偏上，同时避免整片成为统一顶光板。
    )); // 返回主颜色和GBuffer严格共享的有限单位法线。
} // 完成草叶解析世界法线重建。

// 阴影深度管线不包含主画面灯光uniform，也无需编译植被着色函数。
// 运动矢量预通道同样不读取环境光、方向光或曝光。
@if(!GPU_GRASS_SHADOW && !GPU_GRASS_PREPASS) fn vegetation_lighting(base_color: vec3<f32>, world_normal: vec3<f32>) -> vec3<f32> { // 使用真实Bevy环境光和方向光完成轻量双面草叶着色。
    var light_energy = view_bindings::lights.ambient_color.rgb; // 无太阳或夜晚仍保留场景配置的环境亮度。
    let directional_light_count = view_bindings::lights.n_directional_lights; // 读取当前玩法视图真实方向光数量。
    for (var light_index = 0u; light_index < directional_light_count; light_index = light_index + 1u) { // 累加太阳或其他方向光。
        let light = view_bindings::lights.directional_lights[light_index]; // 读取方向、线性色和照度。
        let signed_normal_to_light = dot(world_normal, normalize(light.direction_to_light)); // 保留叶片正面受光与背面透光方向差异。
        let front_diffuse = max(signed_normal_to_light, 0.0); // 朝向太阳的一面保持正常漫反射层次。
        let back_transmission = max(-signed_normal_to_light, 0.0); // 背光面以较弱透射近似薄叶透光。
        let wrapped_diffuse = 0.24 + front_diffuse * 0.66 + back_transmission * 0.38; // 避免旧abs模型把草叶照成均匀荧光板。
        light_energy = light_energy + light.color.rgb * wrapped_diffuse * INV_PI; // 使用Lambert能量比例累加真实方向光。
    } // 完成全部方向光累加。
    return base_color * light_energy * view_bindings::view.exposure; // 与Bevy主场景使用同一相机曝光输出HDR线性色。
} // 完成不需要逐草材质绑定的轻量植被光照。
// 结束运动矢量预通道的主画面光照隔离。
// 结束只属于主画面片元阶段的轻量植被光照。

fn build_grass_vertex(vertex: GrassVertex) -> GrassVertexOutput { // 主画面和阴影共用完整草叶形变，避免投影轮廓与可见草错位。
    let surface_basis = volume_basis(vertex.surface_normal);
    let yaw_cos = unpack_signed_channel(vertex.packed_appearance, 0u); // 低八位恢复Compute每候选一次计算的旋转余弦。
    let yaw_sin = unpack_signed_channel(vertex.packed_appearance, 8u); // 次低八位恢复Compute每候选一次计算的旋转正弦。
    let scale_random = unpack_unit_channel(vertex.packed_appearance, 16u); // 次高八位恢复旧缩放公式的随机输入。
    let vegetation_profile_index =
        (vertex.packed_appearance >> 24u) & VEGETATION_PROFILE_MASK; // 最高字节低两位无损恢复普通草或麦田内容编号。
    let proxy_coverage =
        f32(((vertex.packed_appearance >> PROXY_COVERAGE_SHIFT) & PROXY_COVERAGE_MASK) + 1u) / 8.0; // 中间三位恢复一至八份非零覆盖。
    let profile = vegetation_profiles.profiles[vegetation_profile_index]; // 固定四槽索引已经由两位掩码保证合法。
    let root_hash = stable_appearance_hash(vertex.packed_appearance); // 统一从稳定低二十四位派生颜色、宽度、静态倾斜、柔韧度与风相位。
    let shape_variation = hash_u32(root_hash ^ 0x7a143595u); // 一次哈希的四个字节分别供宽度、柔韧度、倾斜方向和倾斜幅度复用。
    let surface_variation = hash_u32(root_hash ^ 0x94d049bbu); // 第二次哈希的字节分别供明暗、冷暖和微风相位复用。
    let blade_height = mix(profile.geometry.x, profile.geometry.y, scale_random); // 普通草保持0.5至0.9米，麦株使用独立1.05至1.35米范围。
    let root_penetration = profile.geometry.z; // 根部下扎深度与Compute保守包围使用同一内容规格。
    let blade_horizontal_scale = independent_width_scale(profile, shape_variation, scale_random)
        * proxy_coverage // 宽代理先按当前父格实际密度承担份额缩窄。
        * reused_crop_ultra_horizontal_scale(profile, vertex.packed_appearance) // 显式复用远景麦簇的超远作物再补偿八乘八母格覆盖。
        * profile_proxy_horizontal_scale(profile, vertex.packed_appearance); // 芦苇与湿地草按profile标志扩展粗级草簇占地。
    let flexibility = blade_flexibility(profile, shape_variation); // 单株稳定软硬差继续受当前内容上下界约束。

    var local_position = vertex.position; // 从原始grass.glb顶点局部位置开始变换。
    local_position.x = local_position.x * blade_horizontal_scale; // 草片局部X应用独立随机宽度，LOD覆盖由草簇资产自身守恒。
    local_position.z = local_position.z * blade_horizontal_scale; // 草片局部Z使用同一水平比例保持原始轮廓。
    let blade_root_to_tip = clamp(1.0 - vertex.uv.y, 0.0, 1.0); // 保持旧UV方向下根上尖的纵向参数。
    local_position.y =
        blade_root_to_tip * (blade_height + root_penetration) - root_penetration; // 由profile恢复当前内容完整株高和根段。
    let rotated_xz = vec2<f32>( // 绕世界Y轴旋转原始单片草的水平局部位置。
        local_position.x * yaw_cos - local_position.z * yaw_sin, // 计算旋转后世界X偏移。
        local_position.x * yaw_sin + local_position.z * yaw_cos, // 计算旋转后世界Z偏移。
    ); // 完成当前实例水平朝向变换。
    let root_world_xz = vertex.root_world.xz; // 风场和局部踩踏都以不会随窗口移动的绝对草根坐标为输入。
    let natural_lean_offset = static_tip_offset(profile, shape_variation); // 普通草与硬麦秆分别使用自己的稳定天然倾斜上限。
    let wind_offset = wind_tip_offset( // 多尺度连续风带叠加小幅单株相位差。
        grass_interaction, // 当前主世界提取值驱动本帧主颜色、阴影和预通道轮廓。
        root_world_xz, // 世界位置保证窗口移动后宏观风场不换相位。
        unpack_unit_channel(surface_variation, 16u), // 第三表面变化字节作为单株微摆稳定相位。
        flexibility, // 单株柔韧度缩放最大动态响应。
        profile, // 当前内容再控制整体风响应，麦田形成较硬的大尺度风浪。
        grass_global_time(), // 当前轮廓必须与同帧主颜色和阴影读取完全相同的回绕时间。
    ); // 完成本株风摆计算。
    let interaction_offset = interaction_tip_offset(
        grass_interaction, // 当前玩家与本帧仍存活的足迹共同决定可见草叶姿态。
        root_world_xz,
        root_hash,
        profile,
    ); // 只有包围圆内植株进入固定足迹循环并受内容上限约束。
    var combined_tip_offset = natural_lean_offset + world_tip_to_surface(surface_basis, wind_offset + interaction_offset); // 天然倾斜属于植株局部，风与倒伏保持世界方向。
    let unconstrained_tip_length = length(combined_tip_offset); // 读取合成后的原始水平草尖位移。
    let height_safe_tip_limit = min(profile.bending.y, blade_height * 0.92); // 总弯曲同时服从内容上限和当前随机株高。
    if (unconstrained_tip_length > height_safe_tip_limit) { // 极端重叠源或矮远草进入长度安全钳制。
        combined_tip_offset = combined_tip_offset / unconstrained_tip_length * height_safe_tip_limit; // 保持合成方向并同时满足AABB和叶长上限。
    } // 完成高度相关草尖位移保护。
    let combined_tip_length = length(combined_tip_offset); // 读取钳制后的真实水平位移供叶长补偿和法线重建共用。
    let vertical_tip_drop = blade_height - sqrt( // 弯曲后草尖沿近似圆弧降低，避免旧实现把草叶水平拉长。
        max(blade_height * blade_height - combined_tip_length * combined_tip_length, 0.0),
    ); // 完成保持草尖到根距离近似不变的竖向回落。
    let quadratic_bend = blade_root_to_tip * blade_root_to_tip; // 软草使用根部固定、上半叶渐弯的二次曲线。
    let cubic_bend = quadratic_bend * blade_root_to_tip; // 硬草把更多形变集中在叶尖，保持直立茎部。
    let bend_shape = mix(cubic_bend, quadratic_bend, flexibility); // 不使用逐顶点pow即可让每株草拥有不同弯曲性格。
    let bend_derivative = mix( // 法线重建需要同一弯曲曲线沿根尖参数的一阶导数。
        3.0 * quadratic_bend, // 三次硬草曲线导数为三乘根尖比例平方。
        2.0 * blade_root_to_tip, // 二次软草曲线导数为二乘根尖比例。
        flexibility, // 与位置完全相同的柔韧度插值避免受光和轮廓脱节。
    ); // 完成当前顶点弯曲导数。
    local_position.x = rotated_xz.x + vertex.root_world.x + combined_tip_offset.x * bend_shape; // 世界X叠加稳定风摆与局部踩踏。
    local_position.z = rotated_xz.y + vertex.root_world.z + combined_tip_offset.y * bend_shape; // 世界Z使用同一草尖形变曲线。
    local_position.y = local_position.y + vertex.root_world.y - vertical_tip_drop * bend_shape; // 把近似叶长补偿后的局部草高加到真实地表Y。
// 预通道始终写当前轮廓，Motion变体才额外重建历史姿态。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) let previous_wind_offset = wind_tip_offset(
        previous_grass_interaction, // RenderApp保存的上一完整渲染帧风向、强度与动画速度。
        root_world_xz, // 紧凑实例根由世界格稳定生成；同一实例的历史仍使用同一绝对根位置。
        unpack_unit_channel(surface_variation, 16u), // 单株微摆相位必须跨帧保持一致。
        flexibility,
        profile,
        grass_previous_global_time(), // 程序风显式回退一个Globals delta，不能把动态叶片报告成静止。
    ); // 完成上一帧世界连续风摆重建。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) let previous_interaction_offset = interaction_tip_offset(
        previous_grass_interaction, // 玩家移动、足迹新增或衰减均使用上一份固定小快照。
        root_world_xz,
        root_hash,
        profile,
    ); // 完成上一帧局部倒伏重建。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) var previous_combined_tip_offset =
        natural_lean_offset + world_tip_to_surface(surface_basis, previous_wind_offset + previous_interaction_offset); // 历史形变执行同一切向投影，运动矢量与当前姿态一致。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) let previous_unconstrained_tip_length = length(previous_combined_tip_offset); // 历史姿态必须执行与当前帧完全相同的弯曲上限。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) if (previous_unconstrained_tip_length > height_safe_tip_limit) {
        previous_combined_tip_offset = previous_combined_tip_offset
            / previous_unconstrained_tip_length
            * height_safe_tip_limit; // 防止历史位置越出当前草种的合法运动包围。
    }
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) let previous_combined_tip_length = length(previous_combined_tip_offset); // 历史水平位移同样决定叶长补偿的竖向回落。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) let previous_vertical_tip_drop = blade_height - sqrt(
        max(
            blade_height * blade_height
                - previous_combined_tip_length * previous_combined_tip_length,
            0.0,
        ),
    ); // 与当前姿态共用同一圆弧近似，运动矢量不会漏掉上下起伏。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) let previous_world_position = vec3<f32>(
        rotated_xz.x + vertex.root_world.x + previous_combined_tip_offset.x * bend_shape,
        local_position.y + (vertical_tip_drop - previous_vertical_tip_drop) * bend_shape,
        rotated_xz.y + vertex.root_world.z + previous_combined_tip_offset.y * bend_shape,
    ); // 当前世界Y反加本帧回落再减历史回落，精确恢复同一未弯曲局部高度。
    @if(GPU_GRASS_PREPASS) local_position = grass_surface_position(vertex.root_world, local_position, surface_basis);
    @if(GPU_GRASS_PREPASS) var output: GrassVertexOutput; // 构造深度、运动与可选Deferred材质附件共同消费的输出。
    @if(GPU_GRASS_PREPASS) output.clip_position = position_world_to_clip(local_position); // 与主通道使用完全相同的当前裁剪矩阵。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) output.world_position = vec4<f32>(local_position, 1.0); // 当前绝对世界位置供官方函数去抖后投影。
    @if(GPU_GRASS_PREPASS && MOTION_VECTOR_PREPASS) output.previous_world_position = vec4<f32>(grass_surface_position(vertex.root_world, previous_world_position, surface_basis), 1.0); // 官方函数再分别应用当前与上一帧相机矩阵，组合物体和相机运动。
// SSAO或Deferred路径才支付动态弯曲法线重建成本。
    @if(GPU_GRASS_PREPASS && NORMAL_PREPASS_OR_DEFERRED_PREPASS) output.world_normal = surface_basis * reconstructed_grass_world_normal( // 只在Normal或Deferred变体支付解析法线成本。
        vertex, // 当前原型顶点与资产法线。
        yaw_cos, // 当前实例旋转余弦。
        yaw_sin, // 当前实例旋转正弦。
        combined_tip_offset, // 当前帧实际合成草尖偏移。
        bend_derivative, // 当前顶点弯曲曲线导数。
        blade_height, // 当前随机株高。
        root_penetration, // 当前内容根部下扎长度。
        vertical_tip_drop, // 当前叶长补偿竖向回落。
        blade_root_to_tip, // 当前顶点根尖比例。
    ); // 完成与主颜色完全一致的当前草叶法线重建。
// 四附件路径必须让GBuffer材质和最终主颜色共享同一顶点属性。
    @if(GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) output.color = instance_color_tint(profile, surface_variation); // 与主颜色使用同一世界稳定综合色变化。
    @if(GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) output.blade_uv = vertex.uv; // Deferred材质使用与主颜色相同的图集采样坐标。
    @if(GPU_GRASS_PREPASS && GPU_GRASS_DEFERRED_SURFACE) output.vegetation_profile_index = vegetation_profile_index; // Deferred材质使用同一普通草或作物profile。
// 非Normal且非Deferred的Motion-only路径跳过法线与综合色工作。
    @if(GPU_GRASS_PREPASS) return output; // 返回当前路径所需的深度、运动与可选Deferred草叶表面。
    @if(!GPU_GRASS_PREPASS) let world_normal = surface_basis * reconstructed_grass_world_normal( // 主颜色与阴影共用当前帧解析弯曲法线。
        vertex, // 当前原型顶点与资产法线。
        yaw_cos, // 当前实例旋转余弦。
        yaw_sin, // 当前实例旋转正弦。
        combined_tip_offset, // 当前帧实际合成草尖偏移。
        bend_derivative, // 当前顶点弯曲曲线导数。
        blade_height, // 当前随机株高。
        root_penetration, // 当前内容根部下扎长度。
        vertical_tip_drop, // 当前叶长补偿竖向回落。
        blade_root_to_tip, // 当前顶点根尖比例。
    ); // 完成主颜色实际使用的世界法线。

    @if(!GPU_GRASS_PREPASS) local_position = grass_surface_position(vertex.root_world, local_position, surface_basis);
    @if(!GPU_GRASS_PREPASS) var output: GrassVertexOutput; // 构造与片元阶段匹配的顶点输出。
    @if(!GPU_GRASS_PREPASS) output.clip_position = position_world_to_clip(local_position); // 绝对世界草叶位置直接进入Bevy视图矩阵。
    @if(!GPU_GRASS_PREPASS) output.color = instance_color_tint(profile, surface_variation); // 普通草与麦田使用独立综合色变化轴。
    @if(!GPU_GRASS_PREPASS) output.world_normal = world_normal; // 片元阶段使用真实旋转法线和场景光照。
    @if(!GPU_GRASS_PREPASS) output.blade_uv = vertex.uv; // 原样传递网格UV供片元保留官方根尖亮度。
    @if(!GPU_GRASS_PREPASS) output.vegetation_profile_index = vegetation_profile_index; // 片元按同一内容编号选择图集和根尖综合色。
    @if(!GPU_GRASS_PREPASS) return output; // 返回不再包含任何候选判断的紧凑实例顶点结果。
// 完成时间重建最小顶点路径与主画面/阴影完整路径隔离。
} // 完成原始17顶点草网格变换。

// 前向写深度与运动；Deferred再写与最终颜色一致的草叶GBuffer表面。
@if(GPU_GRASS_PREPASS) @vertex
fn prepass_vertex(vertex: GrassVertex) -> GrassVertexOutput {
    return build_grass_vertex(vertex); // 复用主通道位置形变并走上方最小预通道返回分支。
}

@if(GPU_GRASS_PREPASS) struct GrassPrepassFragmentOutput { // 按专用宏与Bevy普通两附件或Deferred四附件RenderPass严格对齐。
    @if(NORMAL_PREPASS) @location(0) normal: vec4<f32>, // SSAO读取Bevy标准Rgb10a2世界法线编码，不能遗留地下地表值。
    @if(MOTION_VECTOR_PREPASS) @location(1) motion_vector: vec2<f32>, // TAA/RR存在时location1写官方[-1,1]UV偏移。
// Solari与RR必须读取草叶材质，不能把地下地形引导用于草叶颜色。
    @if(GPU_GRASS_DEFERRED_SURFACE) @location(2) deferred: vec4<u32>, // location2写Bevy官方Rgba32Uint草叶GBuffer。
    @if(GPU_GRASS_DEFERRED_SURFACE) @location(3) deferred_lighting_pass_id: u32, // location3选择默认第一Deferred照明层。
// 普通前向预通道没有2、3号颜色附件。
} // 完成普通与Deferred共用片元输出声明。

@if(GPU_GRASS_PREPASS) @fragment
fn prepass_fragment( // 普通时间预通道与Deferred草叶表面共用同一片元入口。
    input: GrassVertexOutput, // 当前草叶插值位置及Deferred材质属性。
    @builtin(front_facing) is_front: bool, // 双面叶片背面必须把法线翻到可见半球，匹配Bevy标准PBR行为。
) -> GrassPrepassFragmentOutput {
    var output: GrassPrepassFragmentOutput;
    @if(MOTION_VECTOR_PREPASS) output.motion_vector = calculate_motion_vector(
        input.world_position,
        input.previous_world_position,
    ); // 同一世界位置结合当前/上一视图矩阵，正确记录相机运动下的草叶像素位移。
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS) let normalized_world_normal = normalize(input.world_normal); // 插值后先恢复单位世界法线。
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS) let visible_world_normal = select( // 关闭背面剔除的单片叶必须让所有表面输入使用同一可见面法线。
        -normalized_world_normal, // 背面观察时按Bevy双面材质规则完整翻转法线。
        normalized_world_normal, // 正面观察时保持当前动态弯曲法线。
        is_front, // 光栅器给出当前primitive正反面身份，禁止从内容类型猜测。
    ); // NormalPrepass、Solari GBuffer与RR normal guide共用这一结果。
    @if(NORMAL_PREPASS) output.normal = vec4<f32>(visible_world_normal * 0.5 + vec3<f32>(0.5), 1.0); // 对齐Bevy 0.19官方世界法线[-1,1]到[0,1]编码。
// RR会在Solari开始时从本帧GBuffer解析四张材质引导图。
    @if(GPU_GRASS_DEFERRED_SURFACE) let profile = vegetation_profiles.profiles[input.vegetation_profile_index]; // 使用与主颜色相同的当前内容profile。
    @if(GPU_GRASS_DEFERRED_SURFACE) let base_color = vegetation_surface_color( // 重建主颜色照明前的同一线性草叶综合色。
        profile, // 当前普通草或作物的图集与根尖色盘。
        input.blade_uv, // 与主颜色完全相同的插值UV。
        input.color, // 与主颜色完全相同的世界稳定实例色调。
    ); // 完成RR diffuse_albedo所需的草叶基础色。
    @if(GPU_GRASS_DEFERRED_SURFACE) output.deferred = vegetation_deferred_gbuffer( // 覆盖地下地形残留GBuffer，建立同像素材质一致性。
        base_color, // 纯漫反射草叶基础色与最终颜色严格配对。
        visible_world_normal, // 当前可见面的动态弯曲法线供Solari和RR共同解析。
    ); // 完成草叶GBuffer写入。
    @if(GPU_GRASS_DEFERRED_SURFACE) output.deferred_lighting_pass_id = 1u; // 对齐Bevy默认PBR Deferred lighting pass id常量。
// 非Deferred路径不写材质GBuffer，Normal/Motion各自由自己的宏控制。
    return output;
}
// 主画面保留完整颜色、法线、UV和内容编号插值。
@if(!GPU_GRASS_PREPASS && !GPU_GRASS_SHADOW) @vertex // 紧凑实例已经全部可见，顶点阶段不再读取纹理、环带参数或执行候选剔除。
fn vertex(vertex: GrassVertex) -> GrassVertexOutput { // 主画面入口复用与阴影完全一致的草叶形变。
    return build_grass_vertex(vertex); // 返回完整主画面插值数据。
} // 结束GPU草地主画面顶点入口。

@if(!GPU_GRASS_PREPASS && !GPU_GRASS_SHADOW) @fragment // 每个进入紧凑输出的实例都绘制完整草叶；整簇LOD选择已在Compute完成。
fn fragment(input: GrassVertexOutput) -> @location(0) vec4<f32> { // 完成统一色彩基线与双面植被光照。
    let profile = vegetation_profiles.profiles[input.vegetation_profile_index]; // flat编号保证整块primitive读取同一普通草或麦田profile。
    let gradient_color = vegetation_surface_color( // 主颜色与Deferred/RR引导只允许从这一个函数取得无光照综合色。
        profile, // 当前普通草或作物的图集与根尖色盘。
        input.blade_uv, // 当前插值图集坐标。
        input.color, // 当前世界稳定实例色调。
    ); // 完成与GBuffer完全一致的线性草叶基础色。
    let lit_color = vegetation_lighting(gradient_color, normalize(input.world_normal)); // 使用真实场景环境光、方向光和曝光。
    return vec4<f32>(lit_color, 1.0); // 完整草叶片元不透明并输出最终颜色；深度写权限由视图专用管线决定。
} // 完成整株LOD草叶的双面植被材质输出。
// 方向光阴影只保留裁剪位置和老旧后端需要的未裁剪深度。
@if(!GPU_GRASS_PREPASS && GPU_GRASS_SHADOW) struct GrassShadowVertexOutput { // 与主画面共用形变结果，但不向阴影图输出颜色插值。
    @builtin(position) clip_position: vec4<f32>, // 当前草叶顶点在方向光正交视图中的裁剪位置。
// 缺少原生深度裁剪控制时把原始深度传入片元。
    @if(GPU_GRASS_SHADOW_UNCLIPPED_DEPTH_EMULATION) @location(0) unclipped_depth: f32, // 保存钳制前的反向Z深度。
// 完成可选未裁剪深度插值字段。
} // 完成草地阴影顶点最小输出。

@if(!GPU_GRASS_PREPASS && GPU_GRASS_SHADOW) @vertex // 近景草阴影只运行LOD0紧凑实例，不重新执行Compute或生成逐草实体。
fn shadow_vertex(vertex: GrassVertex) -> GrassShadowVertexOutput { // 方向光阴影顶点入口。
    let grass_output = build_grass_vertex(vertex); // 复用主画面的高度、宽度、风摆和玩家踩踏形变。
    var shadow_output: GrassShadowVertexOutput; // 构造不携带颜色数据的阴影输出。
    shadow_output.clip_position = grass_output.clip_position; // 使用同一世界轮廓投射深度。
// 老旧后端需要手工保留方向光视锥外深度。
    @if(GPU_GRASS_SHADOW_UNCLIPPED_DEPTH_EMULATION) shadow_output.unclipped_depth = grass_output.clip_position.z; // 保存钳制前深度供片元写回。
    @if(GPU_GRASS_SHADOW_UNCLIPPED_DEPTH_EMULATION) shadow_output.clip_position.z = min(grass_output.clip_position.z, 1.0); // 先钳制顶点避免硬件提前裁掉合法阴影。
// 完成方向光未裁剪深度回退准备。
    return shadow_output; // 返回仅供Shadow阶段消费的最小顶点结果。
} // 结束近景草阴影顶点入口。

// 原生深度裁剪控制可用时完全省略片元阶段。
@if(!GPU_GRASS_PREPASS && GPU_GRASS_SHADOW && GPU_GRASS_SHADOW_UNCLIPPED_DEPTH_EMULATION) @fragment // 只在回退后端恢复顶点阶段保存的真实方向光深度。
fn shadow_fragment(input: GrassShadowVertexOutput) -> @builtin(frag_depth) f32 { // 最小阴影片元入口。
    return input.unclipped_depth; // 写入未钳制深度并保持Bevy方向光阴影语义。
} // 结束近景草阴影片元深度回退。
// 完成老旧后端方向光深度回退。
// 结束主画面与方向光阴影入口分支。
// 结束运动矢量预通道与既有主画面/阴影入口分支。
