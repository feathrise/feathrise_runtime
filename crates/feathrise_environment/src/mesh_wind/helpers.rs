//! 树木 GPU 风参数纯函数。
//! 所有 LOD 共享同一宏观弯曲强度，仅按树冠距离降低叶片局部细摆成本。
//! 【模块所有权】只接受共享风质量与部件参数，具体内容域负责在调用前完成身份转换。

use bevy::camera::primitives::Aabb; // 风高权重和剔除扩边都使用Bevy原生局部包围盒。
use bevy::prelude::*; // 引入Vec2、Vec3与基础数值类型。

use super::types::{TreeWindFrame, TreeWindGpuFrame, TreeWindUniform}; // CPU可读参数在唯一边界转换为固定八vec4 GPU布局。
use super::{MeshWindLodLevel, MeshWindPart}; // 共享质量档和茎干/叶簇语义决定局部细摆等级。

const TREE_WIND_MACRO_STRENGTH: f32 = 1.25; // 满档树梢最大局部偏移约一米二五，乘现有0.38至0.47缩放后保持克制。
const TREE_WIND_NOISE_SCALE: f32 = 0.006; // 世界空间低频噪声在单棵十二至十五米树内连续、相邻树之间自然错相。
const TREE_WIND_SCROLL_SPEED_SCALE: f32 = 0.045; // F1动画速度映射到缓慢林木惯性，明显慢于草叶高频摆动。
const TREE_WIND_CULL_HORIZONTAL_MARGIN: f32 = 1.8; // 覆盖宏观弯曲与高档叶片细摆的最坏局部水平位移。
const TREE_WIND_CULL_VERTICAL_MARGIN: f32 = 0.35; // 覆盖树冠上下bop并给坡面与浮点误差保留余量。

/// 单个树木 LOD 部件的 GPU 风质量档。
/// 树干只弯曲，树冠按高、中、低逐步减少微观噪声。
#[derive(Clone, Copy, Debug, PartialEq)] // 纯值配置便于测试锁定跨LOD连续性。
pub(super) struct TreeWindProfile {
    // 固定质量档只保存影响着色分支与局部幅度的四项数据。
    pub(super) low_quality: bool, // 低质量路径只采样宏观风，树干和低模树冠可省去额外细节计算。
    pub(super) micro_strength: f32, // 世界风方向上的局部高频叶片位移。
    pub(super) s_curve_strength: f32, // 树冠横向局部波形强度，避免叶簇像刚性外壳。
    pub(super) bop_strength: f32, // 树冠轻微上下起伏强度，打破单轴机械摆动。
} // 完成GPU风质量档定义。

/// 返回当前 LOD 部件的固定风质量档。
/// 所有树干只走根部固定的宏观树高弯曲。
pub(super) const fn tree_wind_profile(
    // 纯函数让材质创建与单元测试共享唯一质量分级。
    level: MeshWindLodLevel, // 高、中、低实体LOD或固定公告板。
    part: MeshWindPart,      // 当前节点属于茎干、叶簇或公告板。
) -> TreeWindProfile {
    // 返回值在材质创建后不再逐帧改变，避免运行期管线重建。
    match (level, part) {
        // 对全部LOD与部件组合做穷尽质量选择。
        (MeshWindLodLevel::High, MeshWindPart::Foliage) => TreeWindProfile {
            // 高档树冠启用完整局部细摆。
            low_quality: false,      // 近景树冠保留完整宏观风和三类局部细摆。
            micro_strength: 0.20,    // 近景叶簇使用最明显的世界空间高频噪声。
            s_curve_strength: 0.075, // 近景横向曲线可见但不超过宏观树梢位移。
            bop_strength: 0.025,     // 近景上下起伏只作为细节，避免整片树冠弹跳。
        }, // 完成高档树冠配置。
        (MeshWindLodLevel::Medium, MeshWindPart::Foliage) => TreeWindProfile {
            // 中档树冠保留减弱局部细摆。
            low_quality: false,   // 中景继续保留细摆，但降低幅度控制顶点视觉噪声。
            micro_strength: 0.11, // 中景叶片细摆约为高档一半。
            s_curve_strength: 0.04, // 中景横向波形同步降低。
            bop_strength: 0.015,  // 中景上下起伏只保留轮廓变化。
        }, // 完成中档树冠配置。
        (MeshWindLodLevel::Low, MeshWindPart::Foliage) => TreeWindProfile {
            // 低档树冠降级为宏观风。
            low_quality: true,     // 低模树冠只走一次宏观噪声采样。
            micro_strength: 0.0,   // 远景不再计算不可辨识的叶片高频位移。
            s_curve_strength: 0.0, // 远景关闭局部横向波形。
            bop_strength: 0.0,     // 远景关闭局部上下起伏。
        }, // 完成低档树冠配置。
        (_, MeshWindPart::Stem) => TreeWindProfile {
            // 三档树干统一走低成本宏观弯曲。
            low_quality: true,     // 树干只需要按高度平方增加的宏观弯曲。
            micro_strength: 0.0,   // 木质树干不使用叶片高频噪声。
            s_curve_strength: 0.0, // 树干不使用柔软叶簇横向波形。
            bop_strength: 0.0,     // 树干不做上下弹跳。
        }, // 完成全部实体LOD树干配置。
        _ => TreeWindProfile {
            // 公告板与异常组合使用静止回退。
            low_quality: true,     // 公告板不会安装本材质，回退配置仍保持最小成本。
            micro_strength: 0.0,   // 公告板固定不动。
            s_curve_strength: 0.0, // 公告板固定不动。
            bop_strength: 0.0,     // 公告板固定不动。
        }, // 完成不支持组合的安全回退。
    } // 完成LOD与部件风质量映射。
} // 完成GPU风质量档选择。

/// 把统一植被风场转换为当前LOD部件使用的有限GPU参数。
/// 强度归零时宏观与局部位移都会精确归零。
pub(super) fn tree_wind_parameters(
    // 每帧只为六份共享材质各计算一次有限Uniform。
    level: MeshWindLodLevel,  // 当前实体LOD决定叶簇细节等级。
    part: MeshWindPart,       // 茎干仅弯曲，叶簇可叠加局部细摆。
    direction_xz: Vec2,       // 与GPU草地共用的世界X/Z风向。
    normalized_strength: f32, // F1草尖强度归一化到零至一。
    animation_speed: f32,     // 与GPU草地共用的动画速度控制。
) -> TreeWindFrame {
    // 返回值同时供主渲染、阴影和运动向量预通道使用。
    let profile = tree_wind_profile(level, part); // 读取固定LOD细节预算。
    let safe_strength = if normalized_strength.is_finite() {
        // 只接受有限F1强度。
        normalized_strength.clamp(0.0, 1.0) // 合法输入限制在已标定最大偏移内。
    } else {
        // 异常输入进入静止安全分支。
        0.0 // 非有限强度按无风处理，禁止污染主渲染与阴影顶点。
    }; // 完成风力强度安全化。
    let direction_is_finite = direction_xz.x.is_finite() && direction_xz.y.is_finite(); // 两个方向分量必须同时有限。
    let safe_direction = if direction_is_finite {
        // 有限方向继续归一化。
        direction_xz.try_normalize().unwrap_or(Vec2::X) // 零向量安全回退到世界X正向。
    } else {
        // 异常方向使用固定世界轴。
        Vec2::X // NaN或无穷方向不进入GPU材质Uniform。
    }; // 完成世界风向安全化。
    let safe_animation_speed = if animation_speed.is_finite() {
        // 有限速度限制到公共范围。
        animation_speed.clamp(0.0, 8.0) // 与草地公开速度范围保持一致。
    } else {
        // 异常速度冻结风噪声。
        0.0 // 非有限速度冻结世界风噪声滚动。
    }; // 完成动画速度安全化。
    TreeWindFrame {
        // 构造主渲染与预通道共享的完整风参数。
        direction: safe_direction, // 随机树根朝向会在顶点着色器中转换回各自局部风向。
        strength: TREE_WIND_MACRO_STRENGTH * safe_strength, // 所有实体LOD和部件共享同一宏观弯曲，切档不跳。
        noise_scale: TREE_WIND_NOISE_SCALE, // 世界空间采样让固定树位自然错相且重进地图可重复。
        scroll_speed: TREE_WIND_SCROLL_SPEED_SCALE * safe_animation_speed, // 只缩放风场滚动速度，不另设树木控制项。
        micro_strength: profile.micro_strength * safe_strength, // 局部叶片噪声随F1强度同步归零。
        s_curve_speed: 2.8 * safe_animation_speed, // 叶簇波形比树干宏观漂移快但仍受统一速度控制。
        s_curve_strength: profile.s_curve_strength * safe_strength, // 高、中档树冠才保留横向局部波形。
        s_curve_frequency: 4.5, // 沿树高产生柔和相位滞后，树冠不同高度不会整齐平移。
        bop_speed: 2.1 * safe_animation_speed, // 上下起伏略慢于横向局部波形。
        bop_strength: profile.bop_strength * safe_strength, // 高、中档树冠才保留轻微上下变化。
    } // 完成当前材质帧参数构造。
} // 完成统一风场到GPU参数映射。

/// 把可读 CPU 风参数压入固定三个 `vec4` 的 GPU 单帧布局。
/// 该函数是Rust字段语义与WGSL ABI之间的唯一转换边界。
pub(super) fn tree_wind_gpu_frame(frame: TreeWindFrame) -> TreeWindGpuFrame {
    // 显式逐槽打包避免后续增加CPU字段时静默改变GPU布局。
    TreeWindGpuFrame {
        direction_strength_noise: Vec4::new(
            frame.direction.x, // x槽保存世界风向X。
            frame.direction.y, // y槽保存世界风向Z。
            frame.strength,    // z槽保存宏观树梢局部位移上限。
            frame.noise_scale, // w槽保存世界X/Z到噪声UV的比例。
        ), // 完成第一组方向与宏观参数打包。
        motion: Vec4::new(
            frame.scroll_speed,     // x槽保存世界噪声滚动速度。
            frame.micro_strength,   // y槽保存详细叶簇微观位移幅度。
            frame.s_curve_speed,    // z槽保存S曲线动画速度。
            frame.s_curve_strength, // w槽保存S曲线位移幅度。
        ), // 完成第二组动画参数打包。
        secondary: Vec4::new(
            frame.s_curve_frequency, // x槽保存沿树高的相位滞后频率。
            frame.bop_speed,         // y槽保存上下起伏速度。
            frame.bop_strength,      // z槽保存上下起伏幅度。
            0.0,                     // w槽默认零；项目天气桥可写入变速相位偏移。
        ), // 完成第三组次级动画参数打包。
    } // 返回固定48字节单帧GPU值。
} // 完成CPU帧到GPU帧转换。

/// 构造当前、上一帧和完整树高参考组成的固定128字节Uniform。
pub(super) fn tree_wind_uniform(
    current: TreeWindFrame,  // 主渲染与阴影读取的当前帧参数。
    previous: TreeWindFrame, // 运动向量读取的上一帧参数。
    reference_aabb: Aabb,    // 同一LOD全部部件共享且已固定地表根部的局部边界。
) -> TreeWindUniform {
    // 边界w槽固定为零，使Rust与WGSL都保持明确八vec4布局。
    TreeWindUniform {
        current: tree_wind_gpu_frame(current), // 当前帧压入前三个vec4。
        previous: tree_wind_gpu_frame(previous), // 上一帧压入接续三个vec4。
        aabb_min: Vec3::from(reference_aabb.min()).extend(0.0), // 第七个vec4保存局部参考最小边界。
        aabb_max: Vec3::from(reference_aabb.max()).extend(0.0), // 第八个vec4保存局部参考最大边界。
    } // 返回供MaterialExtension binding 100直接上传的值。
} // 完成固定材质Uniform构造。

/// 合并同一 LOD 树干与树冠的局部边界。
/// 两个材质必须共享完整树高，接缝才会沿同一宏观曲线移动。
pub(super) fn combine_tree_part_aabbs(trunk: Aabb, canopy: Aabb) -> Aabb {
    // 同一LOD只计算一次完整树高。
    let minimum = Vec3::from(trunk.min()).min(Vec3::from(canopy.min())); // 逐轴取两个部件的完整最小边界。
    let maximum = Vec3::from(trunk.max()).max(Vec3::from(canopy.max())); // 逐轴取两个部件的完整最大边界。
    Aabb::from_min_max(minimum, maximum) // 返回同一LOD树干与树冠共用的树高参考。
} // 完成树木部件边界合并。

/// 构造根部固定的风高参考边界。
/// 现有资产根须可进入负 Y，但地表零平面以下顶点必须保持不动。
pub(super) fn tree_wind_reference_aabb(source: Aabb) -> Aabb {
    // 将实际边界转换为风高度权重边界。
    let source_minimum = Vec3::from(source.min()); // 保留完整水平范围供材质Uniform结构一致。
    let source_maximum = Vec3::from(source.max()); // 树梢最高点决定零至一高度权重。
    let root_height = if source_minimum.y <= 0.0 && source_maximum.y > 0.0 {
        // 识别现有资产跨越地表零平面的根须。
        0.0 // 包围盒跨越地表零平面时，以资产原点作为固定树根。
    } else {
        // 非标准资产没有明确地表交点时使用真实最低点。
        source_minimum.y // 非标准资产安全回退到其真实最低点。
    }; // 完成树根参考高度选择。
    Aabb::from_min_max(
        // 只改变树高权重下界，不裁掉负Y真实网格。
        Vec3::new(source_minimum.x, root_height, source_minimum.z), // 负Y根须在着色器中钳为零高度权重。
        source_maximum, // 树梢继续获得完整宏观与局部位移。
    ) // 返回只用于风高权重的局部边界。
} // 完成根部固定参考边界构造。

/// 扩展实体局部剔除边界以覆盖顶点风最大位移。
/// 不改变用于树高权重的参考边界。
pub(super) fn expanded_tree_wind_culling_aabb(source: Aabb) -> Aabb {
    // 从静止完整树边界生成动态保守边界。
    let margin = Vec3::new(
        // 三轴分别使用水平与垂直最坏位移。
        TREE_WIND_CULL_HORIZONTAL_MARGIN, // X方向覆盖任意世界风向转入局部后的位移。
        TREE_WIND_CULL_VERTICAL_MARGIN,   // Y方向覆盖树冠轻微上下起伏。
        TREE_WIND_CULL_HORIZONTAL_MARGIN, // Z方向与X使用相同最坏位移预算。
    ); // 完成局部扩边向量。
    Aabb::from_min_max(
        // 同时外扩最小和最大端。
        Vec3::from(source.min()) - margin, // 最小边界向三个方向外扩。
        Vec3::from(source.max()) + margin, // 最大边界向三个方向外扩。
    ) // 返回供Bevy可见性系统使用的保守边界。
} // 完成GPU风剔除边界扩展。

#[cfg(test)] // 纯参数和边界回归无需启动渲染器。
mod tests {
    // GPU风纯函数回归集合。
    use super::*; // 直接验证正式风参数、LOD分级与根部固定边界。

    #[test] // 三个实体LOD必须保持相同宏观弯曲，交叉淡化时轮廓才能对齐。
    fn macro_wind_strength_is_continuous_across_entity_lods() {
        // 锁定LOD交叉期相同宏观树梢位置。
        let high = // 高档参数单独保存供跨档比较。
            tree_wind_parameters(MeshWindLodLevel::High, MeshWindPart::Foliage, Vec2::X, 0.7, 1.1); // 生成高档树冠参数。
        let medium = tree_wind_parameters(
            // 中档参数必须只降低局部细摆。
            MeshWindLodLevel::Medium, // 当前测试选择中档。
            MeshWindPart::Foliage,    // 当前测试比较树冠。
            Vec2::X,                  // 使用固定世界X风向排除方向差异。
            0.7,                      // 使用非端点强度证明同比缩放。
            1.1,                      // 使用正式默认动画速度。
        ); // 生成中档树冠参数。
        let low = tree_wind_parameters(
            MeshWindLodLevel::Low,
            MeshWindPart::Foliage,
            Vec2::X,
            0.7,
            1.1,
        ); // 生成低档树冠参数。
        assert_eq!(high.strength, medium.strength); // 高中档宏观树梢位置必须一致。
        assert_eq!(medium.strength, low.strength); // 中低档宏观树梢位置必须一致。
        assert_eq!(high.noise_scale, low.noise_scale); // 全部LOD必须采样同一世界空间宏观风场。
    } // 完成宏观LOD连续性回归。

    #[test] // 叶片细摆必须随LOD降低，而树干只保留宏观弯曲。
    fn canopy_micro_wind_degrades_by_lod_and_trunk_stays_rigid_near_root() {
        // 锁定树冠细节分级与树干刚度边界。
        let high = // 高档样本保留完整局部细摆。
            tree_wind_parameters(MeshWindLodLevel::High, MeshWindPart::Foliage, Vec2::X, 1.0, 1.0); // 读取高档完整细摆。
        let medium = tree_wind_parameters(
            // 中档样本验证幅度降级。
            MeshWindLodLevel::Medium, // 当前测试选择中档。
            MeshWindPart::Foliage,    // 当前测试读取树冠。
            Vec2::X,                  // 使用固定有限风向。
            1.0,                      // 使用满档强度比较配置原值。
            1.0,                      // 使用单位速度排除速度缩放。
        ); // 读取中档减弱细摆。
        let low = tree_wind_parameters(
            MeshWindLodLevel::Low,
            MeshWindPart::Foliage,
            Vec2::X,
            1.0,
            1.0,
        ); // 读取低档宏观风。
        let trunk = // 树干样本必须关闭全部叶片局部细摆。
            tree_wind_parameters(MeshWindLodLevel::High, MeshWindPart::Stem, Vec2::X, 1.0, 1.0); // 读取近景树干风。
        assert!(high.micro_strength > medium.micro_strength); // 近景叶片细节必须强于中景。
        assert!(medium.micro_strength > low.micro_strength); // 中景叶片细节必须强于远景。
        assert_eq!(low.micro_strength, 0.0); // 低档树冠不得保留高频噪声成本。
        assert_eq!(trunk.micro_strength, 0.0); // 树干不得像叶片一样局部抖动。
        assert!(tree_wind_profile(MeshWindLodLevel::Low, MeshWindPart::Foliage).low_quality); // 低档树冠必须选择单噪声采样路径。
    } // 完成LOD细节分级回归。

    #[test] // F1风力归零必须让树干与叶片所有位移项精确归零。
    fn zero_strength_disables_macro_and_micro_wind() {
        // 锁定F1零风时严格静止。
        let wind = // 使用高档树冠覆盖所有可能位移项。
            tree_wind_parameters(MeshWindLodLevel::High, MeshWindPart::Foliage, Vec2::X, 0.0, 1.1); // 生成无风高档树冠参数。
        assert_eq!(wind.strength, 0.0); // 宏观树高弯曲必须归零。
        assert_eq!(wind.micro_strength, 0.0); // 叶片高频噪声必须归零。
        assert_eq!(wind.s_curve_strength, 0.0); // 横向局部波形必须归零。
        assert_eq!(wind.bop_strength, 0.0); // 上下局部起伏必须归零。
    } // 完成无风静止回归。

    #[test] // 非有限方向、强度和速度不得把NaN或无穷上传到GPU。
    fn invalid_wind_inputs_fall_back_to_finite_still_values() {
        // 同时覆盖三个外部可变输入的安全分支。
        let wind = tree_wind_parameters(
            MeshWindLodLevel::High,             // 使用详细叶簇确保全部参数槽都会构造。
            MeshWindPart::Foliage,              // 覆盖微摆、S曲线和上下起伏槽。
            Vec2::new(f32::NAN, f32::INFINITY), // 两个非法方向分量必须整体回退。
            f32::NAN,                           // 非有限强度必须按零风处理。
            f32::INFINITY,                      // 非有限速度必须冻结动画。
        ); // 完成异常输入安全化。
        assert_eq!(wind.direction, Vec2::X); // 非有限方向固定回退世界X正向。
        assert_eq!(wind.strength, 0.0); // 非有限强度不得产生宏观位移。
        assert_eq!(wind.micro_strength, 0.0); // 详细叶簇微摆也必须随安全强度归零。
        assert_eq!(wind.s_curve_strength, 0.0); // 横向S曲线幅度必须归零。
        assert_eq!(wind.bop_strength, 0.0); // 上下起伏幅度必须归零。
        assert_eq!(wind.scroll_speed, 0.0); // 非有限速度必须冻结世界噪声滚动。
        assert_eq!(wind.s_curve_speed, 0.0); // 非有限速度必须冻结S曲线相位。
        assert_eq!(wind.bop_speed, 0.0); // 非有限速度必须冻结上下起伏相位。
        assert!(
            tree_wind_gpu_frame(wind)
                .direction_strength_noise
                .is_finite()
        ); // 打包后的首组GPU槽必须全部有限。
        assert!(tree_wind_gpu_frame(wind).motion.is_finite()); // 打包后的动画槽必须全部有限。
        assert!(tree_wind_gpu_frame(wind).secondary.is_finite()); // 打包后的次级槽必须全部有限。
    } // 完成非法风参数安全化回归。

    #[test] // 负Y根须应固定在地表以下，剔除边界则必须覆盖最大位移。
    fn wind_reference_fixes_ground_plane_and_culling_bounds_expand() {
        // 同时锁定地表根部与视锥剔除安全范围。
        let source = Aabb::from_min_max(Vec3::new(-2.0, -0.4, -1.5), Vec3::new(2.0, 15.0, 1.5)); // 构造与现有松树相同的跨地表边界。
        let reference = tree_wind_reference_aabb(source); // 构造着色器树高参考。
        let culling = expanded_tree_wind_culling_aabb(source); // 构造可见性保守边界。
        assert_eq!(reference.min().y, 0.0); // 地表及负Y根须必须获得零高度权重。
        let root_weight =
            ((0.0 - reference.min().y) / (reference.max().y - reference.min().y)).clamp(0.0, 1.0); // 复现WGSL根部高度权重公式。
        let buried_root_weight =
            ((-0.4 - reference.min().y) / (reference.max().y - reference.min().y)).clamp(0.0, 1.0); // 复现负Y根须钳制。
        assert_eq!(root_weight, 0.0); // 地表根部必须严格不产生任何位移。
        assert_eq!(buried_root_weight, 0.0); // 埋土根须也必须严格不产生任何位移。
        assert!(culling.min().x < source.min().x); // 水平最小边界必须向外扩。
        assert!(culling.max().z > source.max().z); // 水平最大边界必须向外扩。
        assert!(culling.max().y > source.max().y); // 树冠上下细摆必须计入剔除范围。
    } // 完成根部固定与剔除扩边回归。
} // 结束树木GPU风纯函数测试模块。
