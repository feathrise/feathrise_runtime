//! 绘制水面候选资产登记、GPU预热承接与真实互动实体生成入口。

use bevy::asset::RenderAssetUsages; // 上传完成后只保留RenderWorld副本，避免主世界重复驻留大型连续数组。
use bevy::camera::primitives::Aabb; // 后台缓存的局部包围盒随候选移交到正式水面实体。
use bevy::camera::visibility::NoFrustumCulling; // GPU波峰可能越过静止平面AABB。
use bevy::light::NotShadowCaster; // 透明水不投射整片阴影，但接收方向光阴影以正确遮挡天体直射高光。
use bevy::mesh::Indices; // 后台完成值使用U32三角索引。
use bevy::prelude::*; // Commands、Mesh、StandardMaterial与变换组件。
use bevy::render::render_resource::PrimitiveTopology; // 水面使用三角列表。
use feathrise_scene_core::view::CameraRaycastIgnore; // 相机防穿墙射线忽略无碰撞水面。
use feathrise_scene_content::{SceneId, SceneWaterLayer}; // 作者材质参数与稳定水面身份。

use super::super::components::{DynamicWaterCpuInteraction, DynamicWaterSurface, WaterJumpCycle}; // 只有拥有真实互动足迹的水面进入CPU波纹更新查询。
use super::super::constants::{PLAYER_RIPPLE_CAPACITY, WATER_TRANSPARENT_SORT_DEPTH_BIAS};
use super::super::footprint::WaterInteractionFootprint; // 湖面、斜河与禁用瀑布统一选择实际互动足迹。
use super::super::material::{
    DynamicWaterMaterial, create_dynamic_water_material, rebase_dynamic_water_material,
    update_dynamic_water_material,
}; // 复用正式Blender水面的MaterialExtension及候选批次时间重置。
use super::{
    PaintedWaterMeshData, PaintedWaterPurpose, PaintedWaterSurface, WaterSurfaceDescriptor,
}; // 候选登记与真实实体生成复用父模块稳定公开数据契约。
mod chunks;
pub use chunks::stage_water_surface_chunk;

/// 主线程安装结果持有实体与资产句柄，替换时可显式回收旧孤儿资产。
pub struct PaintedWaterSurfaceSpawn {
    pub entity: Entity,
    pub mesh: Handle<Mesh>,
    pub material: Handle<DynamicWaterMaterial>,
    pub additional_meshes: Vec<Handle<Mesh>>, // 保留全部子块所有权，切换场景/失败时显式回收。
    pub additional_entities: Vec<Entity>,
}

impl PaintedWaterSurfaceSpawn {
    pub fn retire(self, commands: &mut Commands) {
        super::super::retirement::retire_surface(
            commands,
            super::super::retirement::RetiredSurface {
                root: Some(self.entity),
                mesh: self.mesh,
                material: self.material,
                children: self.additional_entities,
                meshes: self.additional_meshes,
            },
        );
    }
}

/// 已登记主世界资产、尚未生成互动实体的不可见候选水面。
pub struct StagedWaterSurface {
    descriptor: WaterSurfaceDescriptor,
    local_aabb: Option<Aabb>, // 构建线程完成的静止几何范围，空水面保持None。
    mesh: Handle<Mesh>,
    material: Handle<DynamicWaterMaterial>,
    interaction_footprint: WaterInteractionFootprint,
    triangle_count: usize,
    additional_meshes: Vec<Handle<Mesh>>,
    additional_entities: Vec<Entity>,
    candidate_root: Option<Entity>, // 子块登记到隐藏根，采用时只切换一个根。
}

/// 从完整水面描述创建项目统一的透明PBR动态水材质。
///
/// F12独立样品预览只复用这一固定材质构造入口，不生成场景水体marker，也不参与玩家互动。
pub fn create_water_surface_material(
    descriptor: &WaterSurfaceDescriptor, // 已通过目录或作者服务校验的固定大小水面外观。
    elapsed_seconds: f32, // 首帧当前与历史时间保持一致，避免材质首次准备时产生运动脉冲。
) -> DynamicWaterMaterial {
    let mut base_material = StandardMaterial {
        base_color: Color::srgba(
            descriptor.base_color_srgba[0], // 复用作者水色红通道。
            descriptor.base_color_srgba[1], // 复用作者水色绿通道。
            descriptor.base_color_srgba[2], // 复用作者水色蓝通道。
            descriptor.base_color_srgba[3], // 复用作者整体透明度。
        ), // 完成线性渲染使用的sRGBA颜色构造。
        alpha_mode: AlphaMode::Blend, // 水面保持透明前向PBR路径。
        metallic: 0.0,                // 水是非金属介质。
        perceptual_roughness: descriptor.perceptual_roughness, // 复用预设粗糙度。
        reflectance: descriptor.reflectance, // 复用预设介质反射率。
        double_sided: true,           // 斜河和预览相机都允许从水面下侧观察。
        cull_mode: None,              // 与双面契约一致地关闭背面裁剪。
        ..default()                   // 其余StandardMaterial字段保持项目既有默认值。
    }; // 完成基础PBR材质构造。
    base_material.depth_bias = WATER_TRANSPARENT_SORT_DEPTH_BIAS; // 水面先绘制，名称、粒子和作者辅助仍稳定叠加在后。
    create_dynamic_water_material(
        base_material,             // 完整基础PBR参数进入项目唯一水Shader。
        elapsed_seconds,           // 当前与历史帧从同一时间基线开始。
        descriptor.flow_direction, // 静水统一方向或样条安全回退方向。
        descriptor.flow_speed,     // 流速与四层独立风浪速度继续分离。
        Some(descriptor),          // 四层宏观波、两层微法线和岸边参数全部来自同一描述。
    ) // 返回可直接登记到Assets的动态水材质。
}

/// 推进一张纯视觉水材质的时间，不给它安装场景互动组件或玩家波源。
///
/// F12独立样品河面通过稳定材质句柄调用本入口，使滚动微法线与正式水面使用同一时间协议，
/// 同时继续保持它不属于场景水网、雨水高度捕获或玩家接触查询。
pub fn advance_water_surface_material(
    material: &mut DynamicWaterMaterial, // 独立样品当前完整水材质。
    elapsed_seconds: f32,                // RenderWorld同源的连续应用时间。
    flow_direction: Vec2,                // 样品长轴方向，固定成本写入当前帧参数。
    flow_speed: f32,                     // 样品观察用的米制流速。
) {
    update_dynamic_water_material(
        material,
        elapsed_seconds,
        &[], // 独立样品没有玩家接触，因此统一八槽始终为空。
        flow_direction,
        flow_speed,
    );
}

impl StagedWaterSurface {
    pub fn mesh_handles(&self) -> impl Iterator<Item = &Handle<Mesh>> {
        std::iter::once(&self.mesh).chain(&self.additional_meshes)
    }

    pub fn discard(
        self,
        commands: &mut Commands,
        _meshes: &mut Assets<Mesh>,
        _materials: &mut Assets<DynamicWaterMaterial>,
    ) {
        super::super::retirement::retire_surface(
            commands,
            super::super::retirement::RetiredSurface {
                root: self.candidate_root,
                mesh: self.mesh,
                material: self.material,
                children: self.additional_entities,
                meshes: self.additional_meshes,
            },
        );
        super::super::retirement::retire_water_cpu(commands, self.interaction_footprint);
    }
    pub fn mesh_handle(&self) -> &Handle<Mesh> {
        &self.mesh
    }

    pub fn material_handle(&self) -> &Handle<DynamicWaterMaterial> {
        &self.material
    }

    pub fn triangle_count(&self) -> usize {
        self.triangle_count
    }

    /// 修改材质后必须重新等待RenderApp回执，不能沿用修改前的PreparedMaterial证据。
    pub fn rebase_material(
        &self,
        materials: &mut Assets<DynamicWaterMaterial>,
        elapsed_seconds: f32,
    ) -> Result<(), String> {
        let mut material = materials
            .get_mut(&self.material)
            .ok_or_else(|| format!("候选水面{}的主世界材质已丢失", self.descriptor.id))?;
        rebase_dynamic_water_material(
            &mut material,
            elapsed_seconds,
            self.descriptor.flow_direction,
            self.descriptor.flow_speed,
        );
        Ok(())
    }
}

/// 只登记一张候选水面的Mesh与材质；候选尚无实体，不会提前进入互动或透明绘制。
pub fn stage_water_surface_assets(
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<DynamicWaterMaterial>,
    descriptor: WaterSurfaceDescriptor,
    data: PaintedWaterMeshData,
    elapsed_seconds: f32,
) -> StagedWaterSurface {
    let triangle_count = data.triangle_count();
    let PaintedWaterMeshData {
        local_aabb, // 连续顶点移入Mesh前保留小型范围完成值。
        positions,
        normals,
        tangents,
        uvs,
        uvs_b,
        colors,
        indices,
        footprint,
        ribbon_footprint,
    } = data;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD, // 上传后释放主世界大数组；候选强句柄继续拥有RenderAsset身份。
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    if let Some(tangents) = tangents {
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, tangents); // 样条网格把局部流向显式送入主/预/Deferred顶点路径。
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    if let Some(uvs_b) = uvs_b {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uvs_b); // UV1逐顶点携带局部流速与高位样条身份加水体样式。
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    let mesh = meshes.add(mesh);
    let material = materials.add(create_water_surface_material(&descriptor, elapsed_seconds)); // 正式水面与独立样品预览共用唯一PBR/波浪构造入口。
    let interaction_footprint = if !descriptor.interaction_enabled {
        WaterInteractionFootprint::Disabled // 瀑布永远不进入玩家八槽波源状态机。
    } else if let Some(ribbon_footprint) = ribbon_footprint {
        WaterInteractionFootprint::Ribbon(ribbon_footprint) // 河流按局部XZ查询实际斜面Y与作者深度。
    } else {
        WaterInteractionFootprint::Painted(footprint) // 湖泊继续使用R8可见网格足迹。
    };
    StagedWaterSurface {
        descriptor,
        local_aabb, // 候选等待GPU回执期间不丢失定位范围。
        mesh,
        material,
        interaction_footprint,
        triangle_count,
        additional_meshes: Vec::new(),
        additional_entities: Vec::new(),
        candidate_root: None,
    }
}

/// GPU资产回执通过后才创建真实水面实体；同批调用可与旧批回收在一次Commands提交中切换。
pub fn spawn_staged_water_surface(
    commands: &mut Commands,
    scene_id: SceneId,
    purpose: PaintedWaterPurpose,
    staged: StagedWaterSurface,
) -> PaintedWaterSurfaceSpawn {
    let StagedWaterSurface {
        descriptor,
        local_aabb, // 真实实体安装同一帧直接取得后台范围。
        mesh,
        material,
        interaction_footprint,
        triangle_count: _,
        additional_meshes,
        additional_entities,
        candidate_root,
    } = staged;
    let cpu_interaction_enabled =
        !matches!(&interaction_footprint, WaterInteractionFootprint::Disabled); // 瀑布与未来纯视觉水面只由GPU全局时间动画，不按primitive逐帧写材质。
    let entity = candidate_root.unwrap_or_else(|| commands.spawn_empty().id());
    commands.entity(entity).insert((
        Name::new(format!("统一水面_{}_{}", scene_id, descriptor.id)),
        PaintedWaterSurface {
            scene_id,
            layer_id: descriptor.id.clone(),
            kind: descriptor.kind,
            purpose,
        },
        Mesh3d(mesh.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(0.0, descriptor.translation_y, 0.0),
        DynamicWaterSurface {
            interaction_footprint,
            rest_water_level: descriptor.rest_water_level,
            flow_direction: descriptor.flow_direction,
            flow_speed: descriptor.flow_speed,
            player_ripples: Vec::with_capacity(PLAYER_RIPPLE_CAPACITY),
            last_player_takeoff_sequence: None, // 新绘制水面首次看到玩家只建立实际起跳序号基线。
            previous_player_local_xz: None,
            previous_player_local_y: None,
            distance_since_ripple: 0.0,
            player_was_in_water: false,
            jump_cycle: WaterJumpCycle::Idle,
        },
        NoFrustumCulling,
        CameraRaycastIgnore,
        NotShadowCaster,
        Visibility::Inherited, // 全部Mesh/GPU信用已完成，唯一根一次解除隐藏。
    ));
    if let Some(local_aabb) = local_aabb {
        commands.entity(entity).insert(local_aabb); // NoFrustumCulling会跳过自动计算，因此显式安装后台完成值。
    }
    if cpu_interaction_enabled {
        commands.entity(entity).insert(DynamicWaterCpuInteraction); // 湖泊与斜河保留既有入水、起跳和落水八槽波纹。
    }
    PaintedWaterSurfaceSpawn {
        entity,
        mesh,
        material,
        additional_meshes,
        additional_entities,
    }
}

/// 为候选顶点布局创建无水面marker、无互动状态的离屏透明PBR管线承接实体。
pub fn spawn_staged_water_warmup(commands: &mut Commands, staged: &StagedWaterSurface) -> Entity {
    commands
        .spawn((
            Name::new(format!("水面GPU管线预热_{}", staged.descriptor.id)),
            feathrise_scene_core::residency::SceneInstanceDetached, // 预热必须离开隐藏候选树，真实PBR管线才能返回就绪证据；归属仍随实例退役。
            Mesh3d(staged.mesh.clone()),
            MeshMaterial3d(staged.material.clone()),
            Transform::from_xyz(0.0, -100_000.0, 0.0).with_scale(Vec3::splat(0.000_001)),
            Visibility::Inherited,
            NoFrustumCulling,
            CameraRaycastIgnore,
            NotShadowCaster,
        ))
        .id() // 没有PaintedWaterSurface或DynamicWaterSurface，绝不会提前参与踏水与波纹。
}

/// 兼容单张水面的既有立即生成调用；整图运行时与F12使用上方分阶段入口。
#[allow(clippy::too_many_arguments)]
pub fn spawn_water_surface(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<DynamicWaterMaterial>,
    scene_id: SceneId,
    descriptor: &WaterSurfaceDescriptor,
    purpose: PaintedWaterPurpose,
    data: PaintedWaterMeshData,
    elapsed_seconds: f32,
) -> PaintedWaterSurfaceSpawn {
    let staged =
        stage_water_surface_assets(meshes, materials, descriptor.clone(), data, elapsed_seconds);
    spawn_staged_water_surface(commands, scene_id, purpose, staged)
}

/// 保留编辑器和既有静水调用点；内部立即转换为统一水面描述并走同一安装路径。
#[allow(clippy::too_many_arguments)]
pub fn spawn_painted_water_surface(
    commands: &mut Commands,                      // 主世界实体命令缓冲。
    meshes: &mut Assets<Mesh>,                    // 主线程Mesh资产仓库。
    materials: &mut Assets<DynamicWaterMaterial>, // 主线程扩展材质资产仓库。
    scene_id: SceneId,                                // 当前场景稳定身份。
    layer: &SceneWaterLayer,                        // 既有水平静水作者层。
    purpose: PaintedWaterPurpose,                 // 正式运行时或F12作者预览用途。
    data: PaintedWaterMeshData,                   // 后台完成的R8静水连续数组。
    elapsed_seconds: f32,                         // 当前GPU绝对时间基线。
) -> PaintedWaterSurfaceSpawn {
    let descriptor = WaterSurfaceDescriptor::from_layer(layer); // 旧静水资产转换为统一描述。
    spawn_water_surface(
        commands,        // 统一入口登记实体。
        meshes,          // 统一入口登记Mesh。
        materials,       // 统一入口登记扩展材质。
        scene_id,          // 保留当前场景身份。
        &descriptor,     // 传入静水统一描述。
        purpose,         // 保留当前所有权用途。
        data,            // 移交后台完成值。
        elapsed_seconds, // 初始化current/previous同一GPU时间。
    )
}
