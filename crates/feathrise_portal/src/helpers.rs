// 传送门 — 辅助函数（传送门实体生成）

use bevy::camera::visibility::RenderLayers; // 渲染层
use bevy::color::LinearRgba; // HDR自发光颜色让光环和光束在Bloom开启时自然溢光。
use bevy::prelude::*; // Commands、Mesh、Material、Transform 等
use feathrise_billboard_text::BillboardText; // 通用离屏烘焙文字 Billboard（朝向相机的标签）
use feathrise_scene_core::view::CameraRaycastIgnore; // 使路标箭头不参与相机防穿墙射线
use feathrise_scene_core::constants::render_layers::PORTAL_EFFECT_LAYER; // 主相机与F12可见、小场景不可见的独立传送特效层。
use feathrise_grounding::GroundSnap; // 传送门视觉根和独立标签按真实地形高度摆放。
use feathrise_scene_core::state::ScenePhase; // 游戏场景枚举

use crate::components::{Portal, PortalLabel}; // 传送门组件
use crate::constants::PORTAL_EDGE_BEAM_HEIGHT; // 视觉根使用共享十四米渐隐光幕高度。
use crate::resources::PortalVisualMeshes; // 正式场景全部传送点复用一套固定程序网格。

/// 单个传送门的内容、场景归属与路标外观配置。
#[derive(Clone, Copy, Debug)]
pub struct PortalSpawnSpec<'a> {
    pub position: Vec3,                 // 路标世界坐标。
    pub target_scene: ScenePhase,        // 目标场景。
    pub target_scene_id: Option<&'a str>, // 动态场景目标ID；静态场景不填写。
    pub target_display_name: &'a str,   // 目标场景清单中的实际显示名称。
    pub spawn_position: Vec3,           // 到达目标场景后的出生坐标。
    pub label: &'a str,                 // 作者原文与调试实体名来源；显示翻译由消费应用负责。
    pub color: Color,                   // 箭头基础色和自发光颜色。
    pub owner_scene: ScenePhase,         // 离开所属场景时自动销毁。
    pub direction_degrees: f32,         // 顺时针路标朝向角度。
}

/// 一座传送门只公开视觉/碰撞根和独立文字锚点，根的全部光效子实体随关系级联回收。
#[derive(Clone, Debug)]
pub struct PortalSpawnedEntities {
    /// 同时承载传送碰撞、地面吸附和全部共享视觉子实体的根。
    pub root: Entity,
    /// Billboard文字请求锚点。
    pub label_anchor: Entity,
}

impl PortalSpawnedEntities {
    /// 返回固定两实体数组，调用侧只清理关系根和独立Billboard锚点。
    pub fn into_array(self) -> [Entity; 2] {
        [self.root, self.label_anchor] // 根销毁会级联清理底座、箭头、边缘光环和边缘直光幕。
    }
}

/// 保留作者色相并覆盖透明度，供不同强度的叠加光层复用。
fn with_alpha(mut color: Color, alpha: f32) -> Color {
    color.set_alpha(alpha); // AlphaMode::Add仍用透明度调制能量，避免白天出现实心色柱。
    color // 返回不改变RGB的作者颜色。
}

/// 把作者颜色转换为HDR自发光值，透明度固定为一并由base_color独立控制。
fn emissive(color: Color, intensity: f32) -> LinearRgba {
    let mut linear = LinearRgba::from(color); // sRGB作者色先转换到线性空间再放大能量。
    linear.red *= intensity; // 红通道保持作者色比并提高Bloom亮度。
    linear.green *= intensity; // 绿通道与红通道使用相同能量倍率。
    linear.blue *= intensity; // 蓝通道同样保持原色相。
    linear.alpha = 1.0; // 自发光透明度不承担混合，几何透明度由base_color统一控制。
    linear // 返回可直接写入StandardMaterial的HDR颜色。
}

/// 按目标场景正式显示名生成传送点世界名称；场景改名后所有消费者都应重新派生而非沿用旧序号。
pub fn portal_destination_label(target_display_name: &str) -> String {
    let target_display_name = target_display_name.trim(); // 清单名称允许作者误带首尾空白，展示边界统一去除。
    if target_display_name.is_empty() {
        return "前往目标场景".to_owned(); // 资产未就绪或损坏边界仍不泄露SceneId和内部portal ID。
    }
    format!("前往{target_display_name}") // 用户确认的固定命名规则不插入额外空格或流水序号。
}

/// 判断旧作者器自动生成的“传送点_N”名称；这种流水名不得压过新的可读默认名称。
fn is_legacy_generated_portal_label(label: &str) -> bool {
    label
        .strip_prefix("传送点_") // 旧实现使用固定中文前缀和十进制序号。
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        }) // 只迁移明确流水名，不误伤作者自定义中文。
}

/// 解析传送点最终显示名称：未自定义时跟随目标场景，自定义后完整保留作者区域语义。
pub fn portal_display_label(
    author_label: &str,        // JSON中保存的名称或旧默认快照。
    label_customized: bool,    // false表示仍采用自动默认命名。
    target_display_name: &str, // 当前目标清单显示名，场景改名后可刷新默认名称。
) -> String {
    let author_label = author_label.trim(); // 输入框和旧JSON可能带首尾空白，显示边界统一去除。
    let default_label = portal_destination_label(target_display_name); // 自动名固定为“前往{目标场景}”。
    if !label_customized
        || author_label.is_empty()
        || is_legacy_generated_portal_label(author_label)
        || author_label == default_label
    {
        return default_label; // 自动名、旧流水名和未实际改写的默认快照都继续跟随场景名称。
    }
    author_label.to_owned() // “前往王府”“前往河道”等作者名称不再被目标场景名覆盖。
}

/// 构造传送门上方的显示文字 Billboard 请求，正式实体与F12作者预览共用同一可读性规格和显式字体。
pub fn portal_label_billboard(text: String, font: Handle<Font>) -> BillboardText {
    BillboardText {
        text,
        font, // 调用侧必须传入包含中文字形的项目字体，类型签名阻止生产路径静默使用默认字体。
        color: Color::WHITE,
        world_height: 0.5,       // 标签世界高度（米）
        follow: true,            // 跟随锚点（即原地），保证朝向相机
        distance_scaling: false, // 路标文字不做近大远小，保持稳定可读
        ..default()
    }
}

/// 辅助函数：生成带边缘竖直光幕的地面箭头路标传送门。
/// 样式：地面圆形底座 + 发光箭头（三角形头 + 矩形杆） + 边缘直光幕 + 目的地文字。
/// 箭头贴地，朝向由 direction_degrees 参数控制
///
/// # direction_degrees 角度约定（顺时针）
/// - 0°  = 北 (-Z 方向)
/// - 90° = 东 (+X 方向)
/// - 180°= 南 (+Z 方向)
/// - 270°= 西 (-X 方向)
pub fn portal_arrow_layout(direction_degrees: f32) -> (Quat, Vec3) {
    // Bevy绕+Y的正旋转会把北方-Z转向西方-X；作者角度规定顺时针增加，因此这里必须取负号。
    let yaw = Quat::from_rotation_y(-direction_degrees.to_radians());
    let flat_rotation = yaw * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2); // 把局部XY箭头铺到世界XZ后再应用顺时针朝向。
    let shaft_offset = yaw * Vec3::new(0.0, 0.0, 1.2); // 局部+Z始终位于箭头尖端的反方向。
    (flat_rotation, shaft_offset)
}

/// 生成带边缘竖直光幕的完整正式传送点实体。
pub fn spawn_portal_entity(
    commands: &mut Commands,                          // 命令队列
    visual_meshes: &PortalVisualMeshes,               // 全部正式传送点共享的固定网格句柄。
    materials: &mut ResMut<Assets<StandardMaterial>>, // 材质资源库
    spec: PortalSpawnSpec<'_>,                        // 内容、场景归属与路标视觉配置
) -> PortalSpawnedEntities {
    let ground_pos = Vec3::new(spec.position.x, 0.0, spec.position.z); // XZ 不变，Y 归零（贴地）
    let (flat_rotation, shaft_offset) = portal_arrow_layout(spec.direction_degrees); // 正式与F12预览共享同一顺时针方位换算，避免东西镜像。

    let arrow_material = materials.add(StandardMaterial {
        base_color: spec.color,       // 基础颜色
        emissive: spec.color.into(),  // 自发光
        alpha_mode: AlphaMode::Blend, // 半透明混合
        double_sided: true,           // 双面渲染
        unlit: true,                  // 不受光照
        ..default()
    });

    let base_material = materials.add(StandardMaterial {
        base_color: Color::srgba(0.15, 0.15, 0.15, 0.45), // 深灰半透明
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        unlit: true,
        ..default()
    });

    let edge_material = materials.add(StandardMaterial {
        base_color: with_alpha(spec.color, 0.58), // 边缘环在草根缝隙中保持清晰但不盖住地面箭头。
        emissive: emissive(spec.color, 7.0),      // HDR高亮在启用Bloom时形成克制外溢光。
        alpha_mode: AlphaMode::Add,               // 叠加混合符合能量光环，不产生深色透明边。
        double_sided: true,                       // 斜坡和高低视角都能看到完整环面。
        cull_mode: None,                          // 显式关闭背面裁剪兑现双面材质语义。
        unlit: true,                              // 昼夜光照不改变传送识别色。
        ..default()                               // 其余PBR字段保持无纹理轻量材质。
    });

    let edge_beam_material = materials.add(StandardMaterial {
        base_color: with_alpha(spec.color, 0.085), // 环形筒壁保持通透，中心视野不会被实心光柱遮住。
        emissive: emissive(spec.color, 2.8),       // 直光幕高于草层后依靠HDR溢光提供远距定位。
        alpha_mode: AlphaMode::Add,                // 叠加混合只显示边缘能量，不产生透明暗边。
        double_sided: true,                        // 玩家从传送范围内外都能看到同一圈直光。
        cull_mode: None,                           // 无顶盖筒壁的内外两面都必须可见。
        unlit: true,                               // 昼夜与太阳方向不改变传送点识别色。
        ..default()                                // 不创建真实动态灯光或逐帧粒子。
    });

    // 1. 关系根：一次地面吸附同时驱动箭头、边缘光环和边缘竖直光幕。
    let root = commands
        .spawn((
            Name::new(format!("PortalRoot_{}", spec.label)), // 根名称便于World检查器定位完整传送视觉。
            Portal {
                target_scene: spec.target_scene, // 碰撞后切换到目标清单绑定的正式场景。
                target_scene_id: spec.target_scene_id.map(str::to_owned), // 动态目标额外保存稳定SceneId。
                target_display_name: spec.target_display_name.to_owned(), // 加载页直接使用目标场景实际名称。
                spawn_position: spec.spawn_position, // 玩家最终出生坐标已经由跨场景解析完整校验。
                label: spec.label.to_string(),       // 运行时日志继续保留作者标签。
            },
            Transform::from_translation(ground_pos), // X/Z沿用作者位置，Y由统一GroundSnap解算。
            Visibility::default(),                   // 关系根提供子实体可见性传播所需组件。
            GroundSnap::new(0.04), // 整组视觉抬高四厘米，避免地表和草根处的深度闪烁。
            DespawnOnExit(spec.owner_scene), // 离开所属场景时级联清理全部视觉子实体。
        ))
        .with_children(|parent| {
            parent.spawn((
                Name::new(format!("PortalBase_{}", spec.label)), // 深色底盘继续表达实际触发范围。
                Mesh3d(visual_meshes.base.clone()),              // 复用启动时登记的固定圆盘网格。
                MeshMaterial3d(base_material), // 每个传送点保留同一克制深色底材质。
                Transform::from_translation(Vec3::Y * 0.02).with_rotation(flat_rotation), // 二维圆盘铺到本地XZ平面。
                RenderLayers::layer(PORTAL_EFFECT_LAYER), // 主相机和F12可见，小场景不重复绘制底盘。
                CameraRaycastIgnore,                      // 纯视觉底盘不参与相机防穿墙射线。
            ));
            parent.spawn((
                Name::new(format!("PortalArrowHead_{}", spec.label)), // 箭头头部仍明确表达出口方向。
                Mesh3d(visual_meshes.arrow_head.clone()),             // 全部路标共享同一三角网格。
                MeshMaterial3d(arrow_material.clone()), // 头部和杆身共享作者颜色材质。
                Transform::from_translation(Vec3::Y * 0.05).with_rotation(flat_rotation), // 头部在底盘上方避免共面。
                RenderLayers::layer(PORTAL_EFFECT_LAYER), // 只进入玩法主视图和作者视口。
                CameraRaycastIgnore,                      // 地面箭头不影响相机距离。
            ));
            parent.spawn((
                Name::new(format!("PortalArrowShaft_{}", spec.label)), // 箭头杆身与头部共同表达方向。
                Mesh3d(visual_meshes.arrow_shaft.clone()),             // 复用固定矩形网格。
                MeshMaterial3d(arrow_material), // 使用同一作者颜色自发光材质。
                Transform::from_translation(Vec3::Y * 0.05 + shaft_offset)
                    .with_rotation(flat_rotation), // 局部偏移旋转后保持原箭头比例。
                RenderLayers::layer(PORTAL_EFFECT_LAYER), // 不让小场景重复绘制世界空间箭头。
                CameraRaycastIgnore,            // 纯视觉杆身不阻挡相机射线。
            ));
            parent.spawn((
                Name::new(format!("PortalEdgeGlow_{}", spec.label)), // 地面边缘光环在高草中先暴露圆形轮廓。
                Mesh3d(visual_meshes.edge_ring.clone()),             // 复用固定低面数环面。
                MeshMaterial3d(edge_material), // 边缘环使用比体积束更亮的叠加材质。
                Transform::from_translation(Vec3::Y * 0.08), // 环面略高于箭头，减少草根和地表遮挡。
                RenderLayers::layer(PORTAL_EFFECT_LAYER), // 光环只属于传送世界特效层。
                CameraRaycastIgnore,           // 发光环不应把相机误判为实体障碍。
            ));
            parent.spawn((
                Name::new(format!("PortalEdgeBeamWall_{}", spec.label)), // 筒壁从传送范围边缘垂直向上发光。
                Mesh3d(visual_meshes.edge_beam_wall.clone()), // 所有正式传送点共享同一十四米顶部渐隐圆柱壁。
                MeshMaterial3d(edge_beam_material), // 低透明叠加材质形成一圈直光而非中心光芯。
                Transform::from_translation(Vec3::Y * (PORTAL_EDGE_BEAM_HEIGHT * 0.5)), // 固定半径筒壁底端贴地、顶端十四米渐隐归零。
                RenderLayers::layer(PORTAL_EFFECT_LAYER), // 边缘直光不进入小场景相机。
                CameraRaycastIgnore,                      // 透明筒壁绝不参与防穿墙射线或玩法碰撞。
            ));
        })
        .id();

    // 2. 目的地文字标签——委托通用 billboard_text crate 离屏烘焙为始终朝向相机的 Billboard。
    //    以一个带 DespawnOnExit 的锚点承载请求并让 Billboard 跟随它，
    //    场景退出时锚点销毁，Billboard 也随之自动清理。
    let label_anchor = commands
        .spawn((
            Name::new(format!("PortalLabel_{}", spec.label)),
            Transform::from_translation(ground_pos + Vec3::Y * 2.2), // 标签高于常见草层并位于光束内部下段。
            GroundSnap::new(2.2), // 文字锚点始终位于当地真实地表上方2.2米。
            Visibility::default(),
            PortalLabel {
                source: spec.label.to_string(),
            },
            portal_label_billboard(spec.label.to_owned(), visual_meshes.label_font.clone()), // 只提交原文和字体，游戏可在显示边界翻译，不更改作者标签。
            DespawnOnExit(spec.owner_scene),
        ))
        .id();

    PortalSpawnedEntities { root, label_anchor }
}

#[cfg(test)]
mod tests {
    use super::{portal_arrow_layout, portal_destination_label, portal_display_label}; // 回归方向、默认名与作者覆盖共同规则。
    use bevy::prelude::Vec3; // 直接验证铺地后三维箭头朝向。

    fn assert_arrow_direction(degrees: f32, expected_forward: Vec3) {
        let (rotation, shaft_offset) = portal_arrow_layout(degrees);
        let actual_forward = rotation * Vec3::Y; // 三角箭头尖端在原始Triangle2d局部+Y。
        assert!(
            actual_forward.distance(expected_forward) < 0.000_01,
            "{degrees}°实际朝向{actual_forward:?}，期望{expected_forward:?}"
        );
        assert!(
            shaft_offset.normalize().distance(-expected_forward) < 0.000_01,
            "箭头杆必须始终位于尖端反方向"
        );
    }

    #[test]
    fn clockwise_portal_angles_do_not_mirror_east_and_west() {
        assert_arrow_direction(0.0, Vec3::NEG_Z); // 北方是零度。
        assert_arrow_direction(90.0, Vec3::X); // 顺时针九十度必须向东而不是向西。
        assert_arrow_direction(180.0, Vec3::Z); // 半圈指向南方。
        assert_arrow_direction(270.0, Vec3::NEG_X); // 顺时针二百七十度必须向西。
    }

    #[test]
    fn portal_name_is_derived_from_target_scene_display_name() {
        assert_eq!(portal_destination_label("b"), "前往b"); // 场景a通往场景b时必须显示用户指定名称。
        assert_eq!(portal_destination_label(" 黄河古道 "), "前往黄河古道"); // 作者名称首尾空白不进入世界标签。
    }

    #[test]
    fn custom_portal_label_overrides_default_but_legacy_sequence_does_not() {
        assert_eq!(portal_display_label("前往b", false, "b"), "前往b"); // 新建点未自定义时使用目标场景默认名。
        assert_eq!(portal_display_label("传送点_7", true, "b"), "前往b"); // 旧流水名自动迁移为可读默认名。
        assert_eq!(portal_display_label("前往王府", true, "b"), "前往王府"); // 区块名称必须完整保留。
    }

    #[test]
    fn production_portal_spawn_keeps_authored_text_without_language_resources() {
        use bevy::prelude::*;
        use feathrise_billboard_text::BillboardText;

        use super::{PortalSpawnSpec, spawn_portal_entity};
        use crate::{Portal, PortalLabel, PortalVisualMeshes};
        use feathrise_scene_core::state::ScenePhase;

        const AUTHOR_LABEL: &str = "沿溪 / Riverside 🚪";
        let font: Handle<Font> =
            bevy::asset::uuid_handle!("b94ec067-8e3e-4ca7-abdf-d4aa27507e26");

        fn spawn_authored_portal(
            mut commands: Commands,
            visuals: Res<PortalVisualMeshes>,
            mut materials: ResMut<Assets<StandardMaterial>>,
        ) {
            spawn_portal_entity(
                &mut commands,
                &visuals,
                &mut materials,
                PortalSpawnSpec {
                    position: Vec3::new(2.0, 0.0, -7.0),
                    target_scene: ScenePhase::Active,
                    target_scene_id: Some("project-map-b"),
                    target_display_name: "目的地_遠景",
                    spawn_position: Vec3::new(8.0, 0.0, 9.0),
                    label: AUTHOR_LABEL,
                    color: Color::WHITE,
                    owner_scene: ScenePhase::Active,
                    direction_degrees: 90.0,
                },
            );
        }

        // 不装配游戏、翻译或渲染插件，真实生成链路只需材质存储和调用方显式提供的视觉资产。
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .insert_resource(PortalVisualMeshes {
                base: Handle::default(),
                arrow_head: Handle::default(),
                arrow_shaft: Handle::default(),
                edge_ring: Handle::default(),
                edge_beam_wall: Handle::default(),
                label_font: font.clone(),
            })
            .add_systems(Startup, spawn_authored_portal);
        app.update();
        app.update();

        let world = app.world_mut();
        let portal = world.query::<&Portal>().single(world).unwrap();
        assert_eq!(portal.label, AUTHOR_LABEL);
        assert_eq!(portal.target_scene_id.as_deref(), Some("project-map-b"));
        let (label, request) = world
            .query::<(&PortalLabel, &BillboardText)>()
            .single(world)
            .unwrap();
        assert_eq!(label.source, AUTHOR_LABEL);
        assert_eq!(request.text, AUTHOR_LABEL);
        assert_ne!(font, Handle::<Font>::default());
        assert_eq!(request.font, font, "世界标签不能静默丢弃项目字体");
    }
}
