//! F12 植被贴地覆盖：共享正式 Clipmap 网格、变换与高度扩展，仅增加编辑视图的一次透明绘制。

use feathrise_scene_core::residency::InstanceResourceTable;
use feathrise_scene_core::residency::SceneResidencyAppExt;

use super::{ClipmapHeightMaterial, SparseClipmapSurfaceMaterial}; // 精确复用正式高度扩展。
use crate::DynamicSparseClipmapPrimitive;
use bevy::asset::{load_internal_asset, uuid_handle}; // 内置覆盖着色器。
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers}; // GPU 位移包围与 F12 独占渲染层。
use bevy::light::{NotShadowCaster, NotShadowReceiver}; // 覆盖不参与阴影。
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin}; // 覆盖共享正式高度材质。
use bevy::prelude::*; // ECS、材质和世界坐标。
use bevy::render::render_resource::{AsBindGroup, ShaderType}; // 固定覆盖参数。
use bevy::shader::ShaderRef; // 自定义片元入口。
use bevy::transform::TransformSystems; // 复制本帧正式变换后再传播。
use feathrise_map_content::{ActiveDynamicMap, MapId}; // 防止换图残留。 // 唯一运行时地形。

mod resources;
mod types;
mod evidence;
pub use evidence::{clipmap_brush_overlay_evidence, ClipmapBrushOverlayEvidence};
pub use resources::DynamicClipmapSurfaceOverlay;
pub use types::{ClipmapSurfaceCoverage, ClipmapSurfaceOverlayIdentity};

const OVERLAY_SHADER: Handle<Shader> = uuid_handle!("7931b3c4-7851-4c15-9f6f-f20102d0ed05"); // 独立覆盖入口。
pub const CLIPMAP_VEGETATION_OVERLAY_LAYER: usize = 31; // 与 F12 笔刷专属相机层一致。

/// 工具输入须在本帧共同边发布后绑定，再由唯一覆盖系统读取，避免并行调度使用旧地表身份。
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClipmapOverlaySet { Prepare, Publish }

/// 编辑器只发布小参数和已有密度图片，不向场景传入作者像素。
#[derive(Resource, Default)]
pub struct DynamicClipmapVegetationOverlay {
    pub brush: Option<ClipmapVegetationBrush>, // 无有效命中即取消刷圈。
    pub density: Option<ClipmapVegetationDensity>, // 当前已编译覆盖图的强句柄。
}

/// 世界 XZ 半径保持已有 R8 绘制语义，Y 由正式地形顶点决定。
#[derive(Clone, Debug)]
pub struct ClipmapVegetationBrush {
    pub map_id: MapId,              // 当前作者地图。
    pub revision: [u64; 3],         // 拾取命中的原始地图根、高度与环带修订。
    pub missing_pages: [UVec4; 16], // 命中时的局部缺页状态，不能跨页采用边界复用。
    pub center: Vec2,               // 精确拾取后的世界 XZ。
    pub radius: f32,                // 外圈米制半径。
    pub strength: f32,              // 内圈半径比例。
    pub color: LinearRgba,          // 保留悬停/绘制/擦除颜色。
}

/// 图片已经包含颜色与透明度；世界范围由正式地形高度扩展提供。
#[derive(Clone, Debug)]
pub struct ClipmapVegetationDensity {
    pub map_id: MapId,        // 防止上一图覆盖流入当前图。
    pub image: Handle<Image>, // 不复制或重新生成 R8 作者内容。
}

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
struct OverlayUniform {
    brush: Vec4,                // xy 中心、z 外圈、w 内圈比例；z=0 关闭刷圈。
    color: Vec4,                // 线性 RGBA 刷圈颜色。
    options: Vec4,              // x=1 显示密度，y 为权威最高高度页级别。
    missing_pages: [UVec4; 16], // 同代未就绪页；只裁剪受影响三角面。
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
struct OverlayExtension {
    #[uniform(126)]
    params: OverlayUniform, // 高度扩展使用120..125，覆盖层从126开始。
    #[texture(127)]
    #[sampler(128)]
    density: Option<Handle<Image>>, // 无密度时使用 Bevy 合法回退纹理。
}

impl MaterialExtension for OverlayExtension {
    fn vertex_shader() -> ShaderRef {
        OVERLAY_SHADER.clone().into()
    } // 复用正式高度函数并输出局部缺页有效性。
    fn fragment_shader() -> ShaderRef {
        OVERLAY_SHADER.clone().into()
    } // 只覆盖前向片元，顶点继承正式高度位移。
    fn alpha_mode() -> Option<AlphaMode> {
        Some(AlphaMode::Blend)
    } // 覆盖不写地形深度。
    fn enable_prepass() -> bool {
        false
    } // 不污染深度、法线和运动预通道。
    fn enable_shadows() -> bool {
        false
    } // 编辑器笔刷不投影。
}

type OverlayMaterial = ExtendedMaterial<ClipmapHeightMaterial, OverlayExtension>; // 保留完整原高度顶点入口。

#[derive(Component)]
struct ClipmapOverlayEntity; // 独立于正式地形标记，避免被地表仲裁接管。

#[derive(Resource, Default)]
struct OverlayState {
    entity: Option<Entity>,                    // 全会话至多一个覆盖实体。
    material: Option<Handle<OverlayMaterial>>, // 参数变化只更新现有资产。
}

#[cfg(test)]
mod surface_tests;
#[cfg(test)] // CPU语义和实际驱动编译回归。
#[path = "overlay_tests.rs"] // 测试不进入正式帧路径。
mod tests; // 直接验证生产 WGSL。 // 真实ECS验证独立地表通道、对象归属和唯一覆盖租约。

pub(super) struct ClipmapVegetationOverlayPlugin; // 随高度材质插件安装。

impl Plugin for ClipmapVegetationOverlayPlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(
            app,
            OVERLAY_SHADER,
            "shaders/vegetation_overlay.wgsl",
            Shader::from_wgsl
        ); // 使用内置生产源码。
        app.configure_sets(PostUpdate, (ClipmapOverlaySet::Prepare, ClipmapOverlaySet::Publish).chain()
                .after(super::stitches::StitchPublish).before(TransformSystems::Propagate));
        app.init_resident_resource::<DynamicClipmapVegetationOverlay>() // 无编辑器时默认不显示。
            .init_resident_resource::<DynamicClipmapSurfaceOverlay>() // 地表通道不被植被同步系统清空。
            .init_resident_resource::<OverlayState>() // 固定实体租约。
            .add_plugins(MaterialPlugin::<OverlayMaterial>::default()) // 不影响 GLB 材质管线。
            .add_resident_systems(PostUpdate, sync_overlay.in_set(ClipmapOverlaySet::Publish)); // 编辑 Update 与共同边发布后读取最终刷印和高度。
    }
}

/// 每帧只访问唯一正式地形和固定参数，不扫描网格或地图像素。
#[allow(clippy::too_many_arguments)]
fn sync_overlay(
    mut commands: Commands,                           // 只创建一次覆盖实体。
    input: Res<DynamicClipmapVegetationOverlay>,      // 编辑器发布输入。
    surface_input: Res<DynamicClipmapSurfaceOverlay>, // 地表已有RGBA图复用同一高度覆盖绘制。
    active: Res<ActiveDynamicMap>,
    ground: crate::TerrainObjectClipmapGround,
    owner: Option<Res<crate::TerrainObjectAuthoringTarget>>,
    frames: Query<&feathrise_scene_core::residency::MapSourceFrame>, // 仅核对所选源框架，拒绝跨姿态旧反馈。
    root_runtime: Res<super::super::sparse_runtime::SparseClipmapRuntime>,
    instances: Option<
        Res<InstanceResourceTable<super::super::sparse_runtime::SparseClipmapRuntime>>,
    >,
    surfaces: Res<Assets<SparseClipmapSurfaceMaterial>>, // 正式地表材质。
    fallback: Res<Assets<ClipmapHeightMaterial>>,        // 地表预热期间的同高材质。
    mut materials: ResMut<Assets<OverlayMaterial>>,      // 覆盖资产固定复用。
    mut state: ResMut<OverlayState>,                     // 单实体生命周期。
    source: Query<
        (
            &Mesh3d,
            &Transform,
            Option<&MeshMaterial3d<SparseClipmapSurfaceMaterial>>,
            Option<&MeshMaterial3d<ClipmapHeightMaterial>>,
        ),
        (
            With<DynamicSparseClipmapPrimitive>,
            Without<ClipmapOverlayEntity>,
        ),
    >, // 来源排除覆盖自身。
    mut targets: Query<(&mut Mesh3d, &mut Transform, &mut Visibility), With<ClipmapOverlayEntity>>,
) {
    let snapshot = ground
        .snapshot
        .as_ref()
        .filter(|snapshot| snapshot.ready && active.map_id.as_ref() == Some(&snapshot.map_id)); // 不跨内容修订显示旧地面覆盖。
    let brush = input.brush.as_ref().filter(|brush| {
        snapshot.is_some_and(|snapshot| {
            brush.map_id == snapshot.map_id
                && brush.revision == snapshot.revision
                && brush.missing_pages == snapshot.missing_pages
        })
    }); // 只显示命中原曲面的刷圈，不把旧点套到新高度。
    let density = input
        .density
        .as_ref()
        .filter(|density| active.map_id.as_ref() == Some(&density.map_id)); // 切图立即拒绝旧印。
    let parent = owner.as_ref().and_then(|owner| owner.instance());
    let surface = surface_input.coverage().filter(|coverage| {
        let identity = &coverage.identity;
        active.map_id.as_ref() == Some(&identity.map_id)
            && owner.as_ref().and_then(|owner| owner.applied_id()) == identity.terrain_id.as_ref()
            && (identity.terrain_id.is_none()
                || owner
                    .as_ref()
                    .is_some_and(|owner| owner.map.as_ref() == Some(&identity.map_id)))
            && identity.source_to_world.is_finite()
            && parent.map_or(identity.source_to_world == Mat4::IDENTITY, |parent| {
                frames
                    .get(parent)
                    .is_ok_and(|frame| frame.source_to_world == identity.source_to_world)
            })
            && snapshot.is_some_and(|snapshot| {
                snapshot
                    .authoring_height_revision()
                    .is_none_or(|revision| revision == identity.terrain_revision)
            })
    }); // 作者、对象和实际高度同域，关闭/失败时发布者撤回独立通道。
    let display = parent
        .and_then(|parent| instances.as_ref()?.values.get(&parent)?.display)
        .or_else(|| parent.is_none().then_some(root_runtime.display).flatten());
    let source = display.and_then(|entity| source.get(entity).ok()); // 只绑定所选对象，不能从同MapId猜测唯一根。
    let height = source.and_then(|(_, _, surface, flat)| {
        surface
            .and_then(|handle| surfaces.get(&handle.0).map(|material| &material.extension))
            .or_else(|| {
                flat.and_then(|handle| fallback.get(&handle.0).map(|material| &material.extension))
            })
    }); // 只取真实安装的完整高度版本。
    let surface = surface.filter(|coverage| {
        height.is_some_and(|height| {
            height.params.source_to_world == coverage.identity.source_to_world
        })
    }); // 姿态过渡不能混用已移动源与旧高度绑定。
    if (brush.is_none() && density.is_none() && surface.is_none())
        || height.is_none()
        || snapshot.is_none()
    {
        if let Some(entity) = state.entity.take() {
            commands.entity(entity).try_despawn();
        } // 关闭或切图时释放旧网格租约，不留跨图隐藏实体。
        if let Some(handle) = state.material.take() {
            materials.remove(handle.id());
        } // 释放旧高度图集强句柄，隐藏覆盖不能把上一图页预算永久留住。
        return; // 稳定无输入帧不分配或重复销毁。
    }
    let params = OverlayUniform {
        brush: brush.map_or(Vec4::ZERO, |brush| {
            Vec4::new(
                brush.center.x,
                brush.center.y,
                brush.radius.max(0.01),
                brush.strength.clamp(0.0, 1.0),
            )
        }), // 保留水平米制范围。
        color: brush.map_or(Vec4::ZERO, |brush| brush.color.to_vec4()), // 密度单独存在时无刷圈。
        options: Vec4::new(
            if surface.is_some() || density.is_some() {
                1.0
            } else {
                0.0
            },
            snapshot.expect("已验证地表同代").max_mip() as f32,
            0.0,
            0.0,
        ), // 缺页查询使用规格Mip，不被旧空白图集的零Mip截断。
        missing_pages: snapshot.expect("已验证地表同代").missing_pages, // 局部未知页绝不投射到伪平面。
    }; // 固定 304 字节：48 字节覆盖参数加 256 字节缺页表。
    let image = surface
        .map(|coverage| coverage.image.clone())
        .or_else(|| density.map(|density| density.image.clone())); // 地表反馈优先，仍只用一个覆盖实体和一次透明绘制。
    let height = height.expect("已验证正式高度存在"); // 上方就绪门保证来源。
    let handle = if let Some(handle) = state.material.as_ref() {
        if let Some(material) = materials.get(handle) {
            let changed = material.extension.params != params
                || material.extension.density != image
                || !material.base.extension.same_height_binding(height); // 稳定帧不制造资产修改。
            if changed && let Some(mut material) = materials.get_mut(handle) {
                material.base.extension = height.clone(); // 与本帧实际地形完全同代。
                material.extension = OverlayExtension {
                    params,
                    density: image,
                }; // 图片和参数一起采用。
            }
        }
        handle.clone() // 保持固定材质句柄。
    } else {
        let handle = materials.add(ExtendedMaterial {
            base: ExtendedMaterial {
                base: StandardMaterial {
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    depth_bias: 4.0,
                    ..default()
                },
                extension: height.clone(),
            }, // 偏移仅作用于深度比较，不抬高地面位置。
            extension: OverlayExtension {
                params,
                density: image,
            }, // 世界投影覆盖。
        }); // 首次有效输入才分配。
        state.material = Some(handle.clone()); // 会话持有强句柄。
        handle // 返回固定资产。
    };
    let (mesh, transform, _, _) = source.expect("高度必定来自有效源"); // 同一次查询保证网格/高度配对。
    if let Some(entity) = state.entity
        && let Ok((mut target_mesh, mut target_transform, mut visibility)) = targets.get_mut(entity)
    {
        if target_mesh.0 != mesh.0 {
            *target_mesh = mesh.clone();
        } // 热更仅交换共享句柄。
        if *target_transform != *transform {
            *target_transform = *transform;
        } // 环带逐细格移动与地形同步。
        *visibility = Visibility::Visible; // 输入有效且同图才能显示。
        if let Some(parent) = parent {
            commands.entity(entity).insert(ChildOf(parent));
        } else {
            commands.entity(entity).remove::<ChildOf>();
        }
    } else {
        state.entity = Some(
            commands
                .spawn((
                    Name::new("Clipmap植被贴地覆盖"), // 调试可辨认。
                    ClipmapOverlayEntity,             // 唯一覆盖租约。
                    mesh.clone(),                     // 与正式地形共享固定组合网格。
                    *transform,                       // 继承本帧实际环带中心。
                    MeshMaterial3d(handle),           // 独立透明覆盖材质。
                    Visibility::Visible,              // 已通过完整来源检查。
                    RenderLayers::layer(CLIPMAP_VEGETATION_OVERLAY_LAYER), // 只有 F12 相机拥有此层。
                    NoFrustumCulling,  // 静态平面 AABB 不代表 GPU 高度范围。
                    NotShadowCaster,   // 不投影。
                    NotShadowReceiver, // 编辑提示保持清晰。
                ))
                .id(),
        ); // 一个额外编辑器 draw，无地图规模分配。
        if let Some(parent) = parent {
            commands
                .entity(state.entity.unwrap())
                .insert(ChildOf(parent));
        }
    }
}
