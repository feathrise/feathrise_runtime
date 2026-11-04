//! 使用草式抖动网格、解析布局、通用多部件 Mesh 和近场代理生成程序化稠密树林。

use std::collections::BTreeMap; // 近场可视代理按层与实例ID稳定去重。

use bevy::app::AppExit; // 可选隐藏烟雾到时以成功状态退出。
use bevy::camera::primitives::Aabb; // GPU圆柱公告板使用覆盖全部水平朝向的显式保守边界。
use bevy::prelude::*; // 示例只使用Bevy程序化几何、材质、灯光和相机。
use bevy::window::WindowPlugin; // 自动烟雾隐藏窗口，普通示例保持可观察。
use feathrise_vegetation::{
    ActiveVegetationLayout, MarkVegetationDirty, VegetationCandidateSource,
    VegetationClaimSampling, VegetationCompetition, VegetationCompetitionGroup,
    VegetationCompetitionGroupId, VegetationCompetitionMode, VegetationDensityField,
    VegetationFootprint, VegetationFootprintScale, VegetationFootprintShape,
    VegetationHeightWeightedWind, VegetationLayerConfig, VegetationLayerId, VegetationLayoutState,
    VegetationMeshBillboard, VegetationMeshBillboardFrontAxis, VegetationMeshBillboardMaterial,
    VegetationMeshBillboardPlugin, VegetationMeshLod, VegetationMeshLodLevel, VegetationMeshPart,
    VegetationMeshPlugin, VegetationMeshPrototype, VegetationNearFieldEntered,
    VegetationNearFieldExited, VegetationNearFieldObserver, VegetationNearFieldPlugin,
    VegetationPlugin, VegetationPrototypeId, VegetationRealization, VegetationSourceField,
    VegetationWeight, create_vegetation_mesh_billboard_material, deterministic_candidate,
    spawn_resolved_vegetation_mesh_instance,
}; // 不引用游戏GLB、PNG、物理crate或项目私有路径。

const FOREST_HALF_EXTENT: f32 = 36.0; // 程序树林覆盖七十二米见方。
const FOREST_LAYER: VegetationLayerId = VegetationLayerId(100); // 无限层ID不占GPU叶片外观槽。
const FOREST_PROTOTYPE: VegetationPrototypeId = VegetationPrototypeId(1); // 原型ID不声明内容类别。
const FOREST_GROUP: VegetationCompetitionGroupId = VegetationCompetitionGroupId(7); // 示例只有一个叠加组。

fn main() {
    let smoke_seconds = std::env::var("VEGETATION_DENSE_FOREST_SMOKE_SECONDS")
        .ok()
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0); // 未设置时保持正常交互示例。
    let plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            visible: smoke_seconds.is_none(), // 自动烟雾不弹出窗口。
            ..default()
        }),
        ..default()
    });
    let mut app = App::new();
    app.add_plugins((
        plugins,
        VegetationPlugin,              // 解析竞争与固定64×64脏块。
        VegetationMeshPlugin,          // 远场使用通用多部件PBR实例。
        VegetationMeshBillboardPlugin, // 最远档在GPU顶点阶段按每个渲染视图朝向。
        VegetationNearFieldPlugin,     // 近场只发布稳定ID与Transform消息。
    ))
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (
            spawn_resolved_forest, // 活动布局换版后差分重建远场可见集合。
            orbit_forest_observer, // 移动观察者持续演示近场滞回。
            sync_proxy_visuals,    // 示例消费消息；公共核心不依赖物理或玩法。
            exit_public_smoke,     // CI可在有限时间后成功退出。
        ),
    );
    if let Some(seconds) = smoke_seconds {
        app.insert_resource(PublicSmokeTimer(Timer::from_seconds(
            seconds,
            TimerMode::Once,
        ))); // 双后端烟雾可验证布局、Mesh和近场代理链实际启动。
    }
    app.run();
}

/// 可选公共烟雾的墙钟退出计时器。
#[derive(Resource)]
struct PublicSmokeTimer(Timer);

/// 稠密分布与通用原型只创建一次；布局换版时复用同一候选算法。
#[derive(Resource)]
struct ForestRuntime {
    source: VegetationCandidateSource, // 远场和近场配置共享同一个抖动网格。
    prototype: VegetationMeshPrototype, // 所有实例共享Mesh、Material和LOD部件句柄。
    roots: Vec<Entity>,                // 活动布局换版时确定性回收旧集合。
    layout_revision: u64,              // 稳定布局不产生逐帧实例重建。
}

/// 示例把近场代理消息可视化为根部标记；实际游戏可改为Avian碰撞或交互实体。
#[derive(Resource)]
struct ProxyVisuals {
    mesh: Handle<Mesh>,                 // 全部近场标记共享一个程序化网格。
    material: Handle<StandardMaterial>, // 全部标记共享一个综合色PBR材质。
    active: BTreeMap<(VegetationLayerId, u64), Entity>, // 层和实例共同构成唯一身份。
}

/// 标记会沿树林内圈移动的观察相机。
#[derive(Component)]
struct OrbitingForestObserver;

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut billboard_materials: ResMut<Assets<VegetationMeshBillboardMaterial>>,
    mut layout: ResMut<VegetationLayoutState>,
    mut dirty: MessageWriter<MarkVegetationDirty>,
) {
    let ground_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.18, 0.28, 0.08), // 综合色地面无需纹理资产。
        perceptual_roughness: 1.0,                 // 粗糙土草地不产生强镜面。
        ..default()
    });
    commands.spawn((
        Name::new("ProceduralForestGround"),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(72.0, 72.0))), // 程序化平面地面。
        MeshMaterial3d(ground_material),                                // 使用程序化综合色材质。
    ));

    let trunk_mesh = meshes.add(Cuboid::new(0.55, 4.0, 0.55)); // 立方柱作为无外部资产树干。
    let canopy_mesh = meshes.add(Cuboid::new(2.8, 3.2, 2.8)); // 低多边形体块作为程序化树冠。
    let trunk_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.26, 0.11, 0.04), // 树干综合色。
        perceptual_roughness: 0.95,                // 木质保持粗糙。
        ..default()
    });
    let canopy_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.05, 0.32, 0.08), // 树冠综合色。
        perceptual_roughness: 0.9,                 // 叶簇保持克制高光。
        ..default()
    });
    let billboard_mesh = meshes.add(Rectangle::new(3.2, 6.4)); // 局部XY竖直矩形用正Z作为资产正面。
    let billboard_material = billboard_materials.add(
        create_vegetation_mesh_billboard_material(
            StandardMaterial {
                base_color: Color::srgb(0.045, 0.27, 0.07), // 无贴图fixture用综合色展示远档转向。
                perceptual_roughness: 0.92,                 // 保持与实体树冠接近的粗糙度。
                double_sided: true,                         // 极端退化回退姿态下仍可从背面观察。
                cull_mode: None,                            // 薄平面显式关闭背面裁剪。
                ..default()
            }, // 基础PBR语义继续由StandardMaterial负责。
            VegetationMeshBillboard::Cylindrical {
                front_axis: VegetationMeshBillboardFrontAxis::PositiveZ, // Rectangle局部正Z朝向当前视图。
            }, // 圆柱模式让程序树始终竖直而不随相机俯仰倾倒。
        ), // 构造一次材质后由全部远档实例共享和批处理。
    ); // 完成GPU公告板材质资产登记。
    let prototype = VegetationMeshPrototype {
        id: FOREST_PROTOTYPE, // 内容原型没有树或岩石枚举。
        lods: vec![
            VegetationMeshLod {
                level: VegetationMeshLodLevel(0), // 近档保留程序树干与实体树冠。
                visibility_range: bevy::camera::visibility::VisibilityRange {
                    start_margin: 0.0..0.0, // 相机近处立即完整显示实体档。
                    end_margin: 18.0..22.0, // 与GPU公告板共享四米交叉宽域。
                    use_aabb: false,        // 按稳定实例根距离切档。
                }, // 完成近档可见范围。
                parts: vec![
                    VegetationMeshPart {
                        part_id: 1, // 消费者自行把部件一解释为树干。
                        mesh: trunk_mesh,
                        material: trunk_material.into(), // 标准PBR句柄通过通用类型安全材质绑定安装。
                        local_transform: Transform::from_xyz(0.0, 2.0, 0.0), // 树干底部落在地面。
                        culling_aabb: None, // 程序立方体没有GPU顶点变形，使用Bevy自动边界。
                        casts_shadow: true,
                        billboard: None, // 树干是普通实体部件，不进入公告板朝向系统。
                        wind: Some(VegetationHeightWeightedWind {
                            root_height: 0.0, // 地面根部固定。
                            tip_height: 4.0,  // 树干顶端达到完整权重。
                            strength: 0.12,   // 示例只登记克制摆幅参数。
                            frequency: 0.45,  // 低频宏观风。
                        }),
                        batch_vertex_mode: Default::default(), // 示例继续使用逐实例生成入口。
                    },
                    VegetationMeshPart {
                        part_id: 2, // 消费者自行把部件二解释为树冠。
                        mesh: canopy_mesh,
                        material: canopy_material.into(), // 标准PBR句柄与扩展材质共用同一绑定契约。
                        local_transform: Transform::from_xyz(0.0, 5.0, 0.0), // 树冠位于树干上方。
                        culling_aabb: None, // 程序立方体没有GPU顶点变形，使用Bevy自动边界。
                        casts_shadow: true,
                        billboard: None, // 实体树冠不进入公告板GPU顶点路径。
                        wind: Some(VegetationHeightWeightedWind {
                            root_height: 3.0, // 树冠连接处保持较低权重。
                            tip_height: 6.5,  // 树冠顶部达到完整权重。
                            strength: 0.35,   // 树冠比树干更明显地摆动。
                            frequency: 0.8,   // 叶簇使用稍高频率。
                        }),
                        batch_vertex_mode: Default::default(), // 示例继续使用逐实例生成入口。
                    },
                ], // 完成近档两个实体部件。
            },
            VegetationMeshLod {
                level: VegetationMeshLodLevel(1), // 远档使用单个GPU圆柱公告板。
                visibility_range: bevy::camera::visibility::VisibilityRange {
                    start_margin: 18.0..22.0, // 与实体档共享完全相同交叉宽域。
                    end_margin: 100.0..110.0, // 覆盖完整示例树林并限制最坏可见范围。
                    use_aabb: false,          // 按稳定实例根距离切档。
                }, // 完成远档可见范围。
                parts: vec![VegetationMeshPart {
                    part_id: 3, // 独立稳定部件身份表示最远公告板。
                    mesh: billboard_mesh,
                    material: billboard_material.into(), // 具体扩展材质类型触发GPU逐视图顶点Shader。
                    local_transform: Transform::from_xyz(0.0, 3.2, 0.0), // Rectangle底边落在实例根地面。
                    culling_aabb: Some(Aabb::from_min_max(
                        Vec3::new(-1.6, -3.2, -1.6), // 水平半径覆盖Rectangle绕Y轴任意GPU朝向。
                        Vec3::new(1.6, 3.2, 1.6),    // 垂直边界保持原6.4米完整高度。
                    )), // 显式保守AABB让远档继续参与Bevy CPU视锥剔除。
                    casts_shadow: false, // 程序远档退出阴影视图以保持示例预算清晰。
                    billboard: Some(VegetationMeshBillboard::Cylindrical {
                        front_axis: VegetationMeshBillboardFrontAxis::PositiveZ, // 必须与材质编码逐值一致。
                    }), // 节点技术身份与GPU材质共同表达动态公告板契约。
                    wind: None,          // 最远平面不叠加另一套顶点变形。
                    batch_vertex_mode: Default::default(), // 示例公告板由原扩展材质顶点入口处理。
                }], // 完成远档单部件公告板。
            },
        ], // 示例同时覆盖实体PBR档与GPU动态公告板档。
    };
    let source = VegetationCandidateSource::JitteredGrid {
        spacing: 3.6,      // 使用与草相同的世界网格思想表达茂密树林。
        jitter: 0.42,      // 候选在各自格内稳定抖动。
        seed: 0xD3A5_EF19, // 固定种子保证重启与窗口移动不洗牌。
    };
    let bounds = Rect::from_corners(
        Vec2::splat(-FOREST_HALF_EXTENT),
        Vec2::splat(FOREST_HALF_EXTENT),
    );
    layout
        .set_configuration(
            vec![VegetationCompetitionGroup {
                id: FOREST_GROUP,
                mode: VegetationCompetitionMode::Additive, // 单层满密度保持全部候选。
                selection_cell_size: 3.6,
                selection_seed: 0xD3A5_EF19,
            }],
            vec![VegetationLayerConfig {
                id: FOREST_LAYER,
                prototype: FOREST_PROTOTYPE,
                source: source.clone(), // 核心近场枚举与远场运行时保留同一值。
                density: VegetationDensityField {
                    world_bounds: bounds,
                    gain: 1.0, // 100%表示候选容量全部启用。
                },
                competition: VegetationCompetition {
                    group: FOREST_GROUP,
                    priority: 0,
                    weight: VegetationWeight::default(),
                    claim_sampling: VegetationClaimSampling::Point,
                    excludes_groups: Vec::new(),
                },
                footprint: VegetationFootprint {
                    shape: VegetationFootprintShape::Circle { radius: 0.35 },
                    scale: VegetationFootprintScale::InheritTransform,
                    exclusion_margin: 0.0,
                    recovery_width: 0.0,
                },
                realization: VegetationRealization::HybridNearField {
                    enter_radius: 16.0, // 观察者进入内圈时发布代理。
                    exit_radius: 20.0,  // 更大外圈形成稳定滞回。
                },
            }],
        )
        .expect("程序化树林配置必须合法");
    layout.set_source_field(
        FOREST_LAYER,
        VegetationSourceField::new(bounds, 64, 64, vec![1.0; 64 * 64])
            .expect("程序化满密度字段必须合法"),
    ); // 1.125米解析纹素足以表达固定满密度示例并覆盖四个世界块。
    dirty.write(MarkVegetationDirty {
        min_tile: IVec2::splat(-1),
        max_tile: IVec2::ZERO,
    }); // 世界原点对齐块覆盖负、正X/Z两侧，完成后一次原子发布。
    commands.insert_resource(ForestRuntime {
        source,
        prototype,
        roots: Vec::new(),
        layout_revision: 0,
    });

    let proxy_mesh = meshes.add(Cuboid::new(0.9, 0.08, 0.9)); // 薄方片标出当前近场代理根。
    let proxy_material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.32, 0.04, 0.9), // 橙色便于穿过树根观察代理集合。
        emissive: LinearRgba::new(0.35, 0.04, 0.0, 1.0), // 轻微自发光避免阴影中不可见。
        ..default()
    });
    commands.insert_resource(ProxyVisuals {
        mesh: proxy_mesh,
        material: proxy_material,
        active: BTreeMap::new(),
    });
    commands.spawn((
        Name::new("ForestObserverCamera"),
        Camera3d::default(),
        VegetationNearFieldObserver, // 核心只观察显式标记，不绑定相机类别。
        OrbitingForestObserver,
        Transform::from_xyz(22.0, 14.0, 0.0).looking_at(Vec3::new(0.0, 3.0, 0.0), Vec3::Y),
    ));
    commands.spawn((
        Name::new("ForestSun"),
        DirectionalLight {
            shadow_maps_enabled: true, // 近景树干和树冠展示标准阴影。
            illuminance: 8_000.0,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.9, -0.7, 0.0)),
    ));
}

/// 活动布局只在完整换版后重建远场集合，稳定阶段没有逐帧竞争或实例操作。
fn spawn_resolved_forest(
    mut commands: Commands,
    active: Res<ActiveVegetationLayout>,
    runtime: Option<ResMut<ForestRuntime>>,
) {
    let Some(mut runtime) = runtime else {
        return; // Startup命令尚未应用时等待下一帧。
    };
    if active.revision == 0 || runtime.layout_revision == active.revision {
        return; // 布局处理中或稳定时继续使用上一有效远场集合。
    }
    for root in runtime.roots.drain(..) {
        commands.entity(root).despawn(); // 换版时确定性释放旧布局选择。
    }
    let source = runtime.source.clone();
    let prototype = runtime.prototype.clone();
    for z in -9..=9 {
        for x in -9..=9 {
            let candidate =
                deterministic_candidate(&source, IVec2::new(x, z)).expect("网格候选始终存在");
            if let Some(root) = spawn_resolved_vegetation_mesh_instance(
                &mut commands,
                &active,
                FOREST_LAYER,
                &prototype,
                &candidate,
            ) {
                runtime.roots.push(root); // 远场和近场共享候选ID、Transform和解析密度。
            }
        }
    }
    runtime.layout_revision = active.revision; // 同一活动版本不再重建。
}

/// 缓慢移动观察者，使候选依次跨过16/20米内外圈并验证无重复代理。
fn orbit_forest_observer(
    time: Res<Time>,
    mut observers: Query<&mut Transform, With<OrbitingForestObserver>>,
) {
    let angle = time.elapsed_secs() * 0.12; // 低速环绕让内外圈变化可观察且不造成镜头眩晕。
    let position = Vec3::new(angle.cos() * 22.0, 14.0, angle.sin() * 22.0); // 固定半径环绕树林。
    for mut transform in &mut observers {
        *transform =
            Transform::from_translation(position).looking_at(Vec3::new(0.0, 3.0, 0.0), Vec3::Y); // 相机与近场观察源使用同一Transform。
    }
}

/// 消费近场消息并用轻量标记展示；换成碰撞适配器时无需修改公共核心。
fn sync_proxy_visuals(
    mut commands: Commands,
    mut entered: MessageReader<VegetationNearFieldEntered>,
    mut exited: MessageReader<VegetationNearFieldExited>,
    visuals: Option<ResMut<ProxyVisuals>>,
) {
    let Some(mut visuals) = visuals else {
        return; // Startup命令尚未应用时等待下一帧。
    };
    for message in exited.read() {
        if let Some(entity) = visuals.active.remove(&(message.layer, message.instance_id)) {
            commands.entity(entity).despawn(); // 越过外圈或布局删除时完整回收。
        }
    }
    for message in entered.read() {
        let key = (message.layer, message.instance_id);
        if let Some(previous) = visuals.active.remove(&key) {
            commands.entity(previous).despawn(); // 原型或Transform换版先回收旧代理。
        }
        let marker = commands
            .spawn((
                Name::new(format!(
                    "NearFieldProxy_{}_{}",
                    message.layer.0, message.instance_id
                )),
                Mesh3d(visuals.mesh.clone()),
                MeshMaterial3d(visuals.material.clone()),
                Transform::from_translation(message.transform.translation + Vec3::Y * 0.04)
                    .with_rotation(message.transform.rotation), // 标记与远场候选根严格同位。
            ))
            .id();
        visuals.active.insert(key, marker); // 多观察者或重复消息不产生重复实体。
    }
}

/// 到达公共烟雾时限后返回成功。
fn exit_public_smoke(
    timer: Option<ResMut<PublicSmokeTimer>>,
    time: Res<Time<Real>>,
    mut exits: MessageWriter<AppExit>,
) {
    let Some(mut timer) = timer else {
        return; // 普通clone后运行保持交互窗口。
    };
    if timer.0.tick(time.delta()).just_finished() {
        exits.write(AppExit::Success); // 已运行足够时间覆盖布局发布、远场生成与近场差分。
    }
}
