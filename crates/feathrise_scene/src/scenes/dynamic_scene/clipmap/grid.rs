//! Geometry Clipmap 的高度感知F12三角网格覆盖层。
//! 覆盖层共享正式组合Mesh与当前高度页，只创建独立线模式材质和纯渲染子实体。

#[cfg(test)]
#[path = "grid_gpu_tests.rs"]
mod gpu_tests; // 实际GPU像素验证，不能只凭Wireframe组件或按钮状态认定可见。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut, SceneInstanceContextAppExt}; // 高度页与线框会话必须属于同一个地形实例。
use feathrise_scene_core::residency::SceneResidencyAppExt;

use bevy::camera::visibility::NoFrustumCulling; // GPU位移后的高度不能使用Y=0 Mesh包围盒裁剪。
use bevy::light::{NotShadowCaster, NotShadowReceiver}; // 作者辅助层不参与世界阴影。
use bevy::material::OpaqueRendererMethod; // 专用线框必须留在前向主通道，不写Deferred GBuffer。
use bevy::pbr::{ExtendedMaterial, MaterialPlugin}; // 注册独立前向线框材质资产。
use bevy::prelude::*; // ECS、资产句柄、层级与插件类型。
use feathrise_scene_core::state::ScenePhase; // 离开动态场景时立即回收覆盖实体与材质。

use super::material::{ClipmapGridExtension, ClipmapGridMaterial}; // 复用正式高度Shader的专用线模式扩展。
use super::residency::SparseHeightPageRuntime; // 只读取当前完整采用的高度扩展及其修订。
use crate::shared::large_scene::terrain_grid_color; // 普通地形与Clipmap使用同一暖金色。
use crate::{DynamicSparseClipmapPrimitive, TerrainGridPreview, TerrainGridTarget}; // 公共开关与显式地形目标边界。

/// 纯渲染覆盖实体身份；不携带正式地形、碰撞、光追或作者拾取身份。
#[derive(Component)] // 私有零大小标记仅用于确定性清理与测试。
struct ClipmapTerrainGridOverlay; // 子实体随正式Clipmap变换，但不参与任何玩法系统。

/// 每个地形实例至多保留一个Clipmap覆盖层和一份材质，不能跨网格共享单一运行时。
#[derive(Resource, Default)] // 进程启动和每次离图都回到完全空状态。
struct ClipmapTerrainGridRuntime {
    source: Option<Entity>,      // 当前正式DynamicSparseClipmapPrimitive实体。
    overlay: Option<Entity>,     // 共享Mesh的纯渲染子实体。
    mesh: Option<AssetId<Mesh>>, // Mesh句柄换代时原位更新覆盖实体，不复制资产。
    material: Option<Handle<ClipmapGridMaterial>>, // 唯一专用线模式材质强句柄。
    extension_revision: u64,     // 已写入材质的完整高度页修订。
    binding: Option<super::material::ClipmapHeightExtension>, // 固定大小参数和图片句柄；整体移动和洞口改变不一定增加高度页修订。
} // 完成Clipmap网格轻量运行时定义。

/// 实例调度早于PostUpdate；共同边LOD晚发布时，同帧更新已存在的唯一线框材质。
pub(super) fn synchronize_stitches(world:&mut World,root:Entity,data:&feathrise_environment::vegetation::blade::GpuBladeClipmapStitches) {
    let update=world.get_resource_mut::<feathrise_scene_core::residency::InstanceResourceTable<ClipmapTerrainGridRuntime>>()
        .and_then(|mut table| {
            let runtime=table.values.get_mut(&root)?;let binding=runtime.binding.as_mut()?;
            if binding.params.stitches==*data{return None;}
            binding.params.stitches=*data;
            Some((runtime.material.clone()?,ClipmapGridExtension::from(&*binding)))
        });
    if let Some((handle,extension))=update && let Some(mut assets)=world.get_resource_mut::<Assets<ClipmapGridMaterial>>()
        && let Some(mut material)=assets.get_mut(&handle){material.extension=extension;}
}

/// 构造暖金色、无光照、前向且轻微前移的专用线框材质。
fn clipmap_grid_material(
    extension: &super::material::ClipmapHeightExtension,
) -> ClipmapGridMaterial {
    ExtendedMaterial {
        base: StandardMaterial {
            base_color: terrain_grid_color(), // 精确使用#E6B85C，与普通Wireframe一致。
            unlit: true,                      // 线条颜色不随昼夜与地表法线变暗。
            fog_enabled: false,               // 作者拓扑在远处仍保持可辨认颜色。
            depth_bias: 1.0,                  // 正值把共面线条轻微拉近相机，抑制与正式地表闪烁。
            cull_mode: None,                  // 从地表下方或编辑器翻转视角仍可检查真实三角边。
            opaque_render_method: OpaqueRendererMethod::Forward, // 禁止覆盖层进入Deferred GBuffer。
            ..default()                       // 其余StandardMaterial字段保持无纹理安全默认。
        }, // 完成线框基础材质。
        extension: ClipmapGridExtension::from(extension), // 克隆当前高度页句柄与固定大小参数。
    } // 返回可登记的唯一高度感知线框资产。
} // 完成Clipmap线框材质构造。

/// 回收当前覆盖实体与资产；Mesh和高度图片由正式运行时继续拥有。
fn clear_runtime(
    commands: &mut Commands, // 覆盖实体可能已随场景根销毁，使用try_despawn安全清理。
    materials: &mut Assets<ClipmapGridMaterial>, // 只移除本功能创建的专用材质资产。
    runtime: &mut ClipmapTerrainGridRuntime, // 归还全部轻量身份与强句柄。
) {
    if let Some(overlay) = runtime.overlay.take() {
        // 当前来源最多一个覆盖实体。
        commands.entity(overlay).try_despawn(); // 已随父级清理时保持幂等。
    } // 完成覆盖实体回收请求。
    if let Some(material) = runtime.material.take() {
        // 当前场景最多一个专用材质。
        materials.remove(material.id()); // 不触碰正式Clipmap地表或高度页资产。
    } // 完成专用材质回收。
    *runtime = ClipmapTerrainGridRuntime::default(); // 清除源、Mesh与高度修订身份。
} // 结束Clipmap覆盖层清理helper。

/// 只响应开关、源实体/Mesh换代与高度扩展修订；稳定帧走常数快速返回。
fn sync_clipmap_terrain_grid(
    mut commands: Commands,              // 创建、更新或销毁唯一纯渲染子实体。
    preview: Res<TerrainGridPreview>,    // 当前场景本进程网格选择，加载页会强制关闭。
    pages: InstanceRes<SparseHeightPageRuntime>, // 当前正式采用高度页与严格修订身份。
    sources: InstanceQuery<
        // 每个实例只允许一个正式Clipmap显示源；多个网格各有自己的上下文。
        (Entity, &Mesh3d, &GlobalTransform), // 共享原Mesh句柄，并为新子实体首帧复制已传播世界变换。
        (With<TerrainGridTarget>, With<DynamicSparseClipmapPrimitive>),
    >, // 完成全部当前Clipmap源查询。
    changed_sources: InstanceQuery<
        // 新目标与Mesh热换代唤醒覆盖同步。
        Entity, // 只需要确认存在变化，不遍历Mesh数据。
        (
            With<TerrainGridTarget>,                         // 公共显式地形目标。
            With<DynamicSparseClipmapPrimitive>,             // 只处理GPU位移Clipmap。
            Or<(Added<TerrainGridTarget>, Changed<Mesh3d>)>, // 初载与句柄替换边界。
        ),
    >, // 完成增量源查询。
    overlays: InstanceQuery<(), With<ClipmapTerrainGridOverlay>>, // 检测父级销毁后的运行时悬空身份。
    mut materials: ResMut<Assets<ClipmapGridMaterial>>, // 每个来源至多持有一份专用材质资产。
    mut runtime: InstanceResMut<ClipmapTerrainGridRuntime>, // 和当前查询、高度页使用同一实例，避免漏画独立网格或串用高度。
) {
    let source_invalid = runtime
        .source
        .is_some_and(|source| sources.get(source).is_err()); // O(1)核对旧源是否仍有效。
    let overlay_invalid = runtime
        .overlay
        .is_some_and(|overlay| overlays.get(overlay).is_err()); // O(1)核对覆盖实体是否仍存在。
    let source_changed = runtime.source.is_some_and(|source| changed_sources.contains(source)); // 已绑定后按实体O(1)核对，不在每个实例稳定帧遍历其全部归属实体。
    let extension_changed = pages.extension_revision > runtime.extension_revision; // 只接受正式页系统发布的更高单调修订，迟到旧值不得倒退。
    let binding_changed = pages.extension_revision >= runtime.extension_revision && pages.extension.as_ref()
        .is_some_and(|extension| runtime.binding.as_ref().is_none_or(|old| !old.same_height_binding(extension)));
    let awaiting_source = preview.visible && runtime.source.is_none(); // 开启后等待迟到正式地形或高度扩展。
    if !preview.is_changed() // 按钮、切换场景或加载页清零会突破快速门。
        && !source_invalid // 父源没有被场景切换销毁。
        && !overlay_invalid // 覆盖实体没有被外部回收。
        && !source_changed // Mesh与目标身份保持稳定。
        && !extension_changed // 高度页仍是同一完整修订。
        && !binding_changed // 固定参数比较不读取像素；源变换和洞口也必须与正式地表一致。
        && !awaiting_source
    // 已有完整覆盖时无需查询当前源集合。
    {
        return; // 稳定帧不访问材质资产、不写Commands且不扫描Mesh。
    } // 完成Clipmap网格同步快速路径。
    if !preview.visible {
        // 独立关闭网格或进入菜单/加载页时立即回收。
        clear_runtime(&mut commands, &mut materials, &mut runtime); // 正式地形继续渲染且不重建Mesh。
        return; // 关闭状态不创建任何替代实体。
    } // 完成关闭分支。
    let Some(extension) = pages.extension.as_ref() else {
        // 正式高度映射尚未完成时不能显示Y=0假线框。
        clear_runtime(&mut commands, &mut materials, &mut runtime); // 保持完全无覆盖状态。
        return; // 高度扩展修订到达后自动重试。
    }; // 完成正式高度绑定读取。
    let Ok((source, mesh, source_global)) = sources.single() else {
        // 只接受唯一正式Clipmap地形；空场景容器没有高度扩展，已在上方退出而不扫描子图源。
        clear_runtime(&mut commands, &mut materials, &mut runtime); // 无源或多源时拒绝歧义覆盖。
        return; // 等待后续Added/移除边界重新收口。
    }; // 完成当前源读取。
    if runtime.source != Some(source) {
        // 切换场景或同图重建根必须丢弃旧覆盖身份。
        clear_runtime(&mut commands, &mut materials, &mut runtime); // 旧材质不能绑定到新场景页表。
    } // 完成源换代处理。
    let material = match runtime.material.clone() {
        // 优先原位更新唯一资产。
        Some(handle) if materials.get(&handle).is_some() => {
            // 强句柄与资产均仍有效。
            if pages.extension_revision > runtime.extension_revision || binding_changed {
                // 只在正式页修订前进、源变换或洞口绑定改变时触发Asset::Modified。
                if let Some(mut asset) = materials.get_mut(&handle) {
                    // 资产在当前主世界表内仍存在。
                    asset.extension = ClipmapGridExtension::from(extension); // 原子替换高度参数与两个图片句柄。
                } // 完成现有材质高度绑定更新。
            } // Mesh换代不改写材质资产。
            handle // 继续复用同一专用材质句柄。
        } // 完成现有资产分支。
        _ => materials.add(clipmap_grid_material(extension)), // 首次开启或资产丢失时创建一份小型材质。
    }; // 完成当前覆盖材质取得。
    let overlay = match runtime
        .overlay
        .filter(|entity| overlays.get(*entity).is_ok())
    {
        // 优先复用当前纯渲染子实体。
        Some(entity) => {
            // Mesh或材质换代均原位替换组件。
            commands.entity(entity).insert((
                // 不更改父级、变换或任何正式地形组件。
                Mesh3d(mesh.0.clone()), // 克隆源Mesh强句柄，绝不复制Mesh资产。
                MeshMaterial3d(material.clone()), // 安装当前唯一高度感知线框材质。
            )); // 完成现有覆盖实体更新。
            entity // 保留覆盖实体身份。
        } // 完成覆盖复用分支。
        None => {
            // 首次开启或父级换代后创建新的纯渲染子实体。
            let entity = commands
                .spawn((
                    // 覆盖实体不携带DynamicSceneTerrainPrimitive或GLTF名称。
                    Name::new("编辑器地形三角网格"), // 世界检查器明确说明作者辅助用途。
                    ClipmapTerrainGridOverlay, // 私有清理身份，不进入正式地形查询。
                    Mesh3d(mesh.0.clone()),    // 与源共享同一组合Mesh句柄。
                    MeshMaterial3d(material.clone()), // 高度页位移与线模式由专用材质完成。
                    Transform::IDENTITY,       // 作为源子实体继承其细格吸附平移。
                    *source_global, // Commands在PostUpdate末应用，显式复制使首次提取也位于源地形而非世界原点。
                    Visibility::Inherited, // 源隐藏或场景离场时覆盖同步隐藏。
                    NoFrustumCulling, // 高度页位移后的真实范围不由Y=0 AABB裁剪。
                    NotShadowCaster, // 辅助线不投射世界阴影。
                    NotShadowReceiver, // 无光线条不被世界阴影压暗。
                    Pickable::IGNORE, // 场景选择继续命中真实地形或作者对象。
                ))
                .id(); // 完成唯一覆盖实体生成。
            commands.entity(source).add_child(entity); // 子实体自动继承Clipmap每帧焦点变换。
            entity // 返回新覆盖身份。
        } // 完成首次覆盖生成分支。
    }; // 完成覆盖实体取得。
    runtime.source = Some(source); // 保存当前正式源身份供O(1)失效核对。
    runtime.overlay = Some(overlay); // 保存唯一覆盖实体身份。
    runtime.mesh = Some(mesh.0.id()); // 保存共享Mesh资产ID供测试与诊断。
    runtime.material = Some(material); // 保持专用材质强句柄。
    if pages.extension_revision >= runtime.extension_revision {
        runtime.extension_revision = pages.extension_revision; // 只有当前完整正式修订可成为已采用值。
        runtime.binding = Some(extension.clone()); // 只克隆固定参数与强句柄，绝不复制网格或高度图片。
    }
} // 结束Clipmap高度感知网格同步。

/// 动态场景离场时确定性回收专用材质，即使父级已由DespawnOnExit先销毁。
fn clear_clipmap_terrain_grid(
    mut commands: Commands, // 安全回收可能仍存在的覆盖实体。
    mut materials: ResMut<Assets<ClipmapGridMaterial>>, // 删除本功能唯一材质资产。
    mut runtime: InstanceResMut<ClipmapTerrainGridRuntime>, // 清空当前来源跨帧身份。
) {
    clear_runtime(&mut commands, &mut materials, &mut runtime); // Mesh与高度图片继续由正式系统回收。
} // 结束Clipmap网格离场清理。

/// 只在test_panel构建注册专用材质、同步系统和离场回收。
pub(crate) struct ClipmapTerrainGridPlugin; // 动态场景插件显式安装一次。

/// 实例域仅取得调度入口，私有覆盖组件、材质和运行时不泄漏给场景协调器。
pub(crate) fn add_instance_grid_systems<M>(schedule: &mut Schedule, after: impl IntoSystemSet<M>) {
    schedule.add_systems(sync_clipmap_terrain_grid.after(after));
}

impl Plugin for ClipmapTerrainGridPlugin {
    fn build(&self, app: &mut App) {
        // 所有运行时依赖在首帧前完成登记。
        app.init_resident_resource::<TerrainGridPreview>() // 独立场景测试未装F12时仍默认关闭。
            .init_instance_resource::<ClipmapTerrainGridRuntime>() // 根与每个独立网格各自拥有轻量覆盖会话。
            .add_plugins(MaterialPlugin::<ClipmapGridMaterial>::default()) // 注册高度感知线模式材质管线。
            .add_resident_systems(PostUpdate, sync_clipmap_terrain_grid) // F12与高度页Update完成后同帧采用最终修订。
            .add_resident_systems(
                OnExit(ScenePhase::Active),
                clear_clipmap_terrain_grid,
            ); // 离场立即归还材质资产。
    } // 完成Clipmap网格插件装配。
} // 结束Clipmap网格插件实现。

#[cfg(test)] // 无窗口测试验证共享Mesh、修订换代与完整回收。
mod tests {
    use super::*; // 直接运行生产同步系统与私有运行时。
    use bevy::asset::RenderAssetUsages; // 构造最小主世界Mesh资产。
    use bevy::render::render_resource::PrimitiveTopology; // 测试Mesh只需合法三角列表拓扑。

    use crate::scenes::dynamic_scene::clipmap::{
        // 使用正式边界与高度扩展构造器建立真实绑定夹具。
        ClipmapBounds,
        ClipmapFlatHeightPage,
        ClipmapHeightExtension,
    }; // 完成测试Clipmap类型导入。

    /// 创建一份无需GPU上传的合法平面高度扩展。
    fn test_extension() -> ClipmapHeightExtension {
        ClipmapHeightExtension::new_flat(
            // 复用生产规格验证与Uniform构造。
            ClipmapBounds::new(Vec2::ZERO, Vec2::splat(64.0)).unwrap(), // 有限非退化场景边界。
            UVec2::splat(512),                                          // 0.125米实际作者格距。
            &ClipmapFlatHeightPage {
                heights: Handle::default(),    // 无窗口测试不采样GPU图片。
                page_table: Handle::default(), // 无窗口测试不读取页表。
            }, // 完成合法回退页夹具。
        )
        .unwrap() // 合法规格必须成功建立扩展。
    } // 完成测试高度扩展构造。

    /// 创建不含顶点数据的合法三角列表Mesh；句柄身份足以验证零复制。
    fn test_mesh() -> Mesh {
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD,
        ) // 覆盖系统从不读取Mesh内容。
    } // 完成测试Mesh构造。

    #[test]
    fn independent_clipmap_instances_keep_their_own_grid_and_height_pages() {
        use feathrise_scene_core::residency::{initialize_instance_context, InstanceResourceTable, SceneInstanceOwner, SceneRuntimeScope};
        let mut app = App::new();
        app.init_resource::<TerrainGridPreview>()
            .init_instance_resource::<SparseHeightPageRuntime>()
            .init_instance_resource::<ClipmapTerrainGridRuntime>()
            .insert_resource(Assets::<Mesh>::default())
            .insert_resource(Assets::<ClipmapGridMaterial>::default());
        let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(test_mesh());
        let instances = [0, 1].map(|index| {
            let instance = app.world_mut().spawn_empty().id();
            initialize_instance_context(app.world_mut(), instance);
            let source = app.world_mut().spawn((SceneInstanceOwner(instance), TerrainGridTarget,
                DynamicSparseClipmapPrimitive, Mesh3d(mesh.clone()), Transform::IDENTITY)).id();
            let mut extension = test_extension();
            extension.set_source_frame(Mat4::from_translation(Vec3::new(index as f32 * 40.0, 0.0, 0.0)));
            let mut pages = app.world_mut().resource_mut::<InstanceResourceTable<SparseHeightPageRuntime>>();
            let pages = pages.values.get_mut(&instance).unwrap();
            pages.extension = Some(extension);
            pages.extension_revision = 10 + index;
            (instance, source)
        });
        // 正式驱动为每个实例保留独立系统游标；不能用同一个RunSystemOnce掩盖第二个实例漏更新。
        let mut schedules = [Schedule::default(), Schedule::default()];
        for schedule in &mut schedules { schedule.add_systems(sync_clipmap_terrain_grid); }
        let run = |app: &mut App, schedules: &mut [Schedule; 2]| {
            for ((instance, _), schedule) in instances.iter().zip(schedules) {
                app.world_mut().resource_mut::<SceneRuntimeScope>().instance = Some(*instance);
                schedule.run(app.world_mut());
            }
            *app.world_mut().resource_mut::<SceneRuntimeScope>() = SceneRuntimeScope::default();
        };
        app.world_mut().resource_mut::<TerrainGridPreview>().visible = true;
        run(&mut app, &mut schedules);
        let overlays: Vec<_> = instances.iter().enumerate().map(|(index, (instance, source))| {
            let runtime = &app.world().resource::<InstanceResourceTable<ClipmapTerrainGridRuntime>>().values[instance];
            assert_eq!(runtime.source, Some(*source));
            assert_eq!(runtime.extension_revision, 10 + index as u64);
            let overlay = runtime.overlay.unwrap();
            assert_eq!(app.world().get::<ChildOf>(overlay).unwrap().parent(), *source);
            assert_eq!(app.world().get::<SceneInstanceOwner>(overlay).unwrap().0, *instance);
            assert!(app.world().resource::<Assets<ClipmapGridMaterial>>().get(runtime.material.as_ref().unwrap()).is_some());
            overlay
        }).collect();
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1);
        assert_eq!(app.world().resource::<Assets<ClipmapGridMaterial>>().len(), 2);
        assert!(app.world().resource::<ClipmapTerrainGridRuntime>().overlay.is_none());
        run(&mut app, &mut schedules); // 稳定帧不得重建任何覆盖或跨用第二个实例的高度。
        for ((instance, _), overlay) in instances.iter().zip(&overlays) {
            assert_eq!(app.world().resource::<InstanceResourceTable<ClipmapTerrainGridRuntime>>().values[instance].overlay, Some(*overlay));
        }
        let moved = Mat4::from_translation(Vec3::new(20.0, 4.0, -3.0));
        app.world_mut().resource_mut::<InstanceResourceTable<SparseHeightPageRuntime>>().values
            .get_mut(&instances[1].0).unwrap().set_source_frame(moved);
        run(&mut app, &mut schedules); // 整体移动没有重建高度页，也必须同步线框采样变换。
        let runtimes = app.world().resource::<InstanceResourceTable<ClipmapTerrainGridRuntime>>();
        assert_eq!(runtimes.values[&instances[1].0].binding.as_ref().unwrap().params.source_to_world, moved);
        assert_eq!(runtimes.values[&instances[0].0].binding.as_ref().unwrap().params.source_to_world, Mat4::IDENTITY);
        app.world_mut().entity_mut(instances[0].1).despawn();
        run(&mut app, &mut schedules); // 删除一个来源不能撤掉另一个覆盖层。
        assert!(app.world().get_entity(overlays[0]).is_err());
        assert!(app.world().get_entity(overlays[1]).is_ok());
        app.world_mut().resource_mut::<TerrainGridPreview>().visible = false;
        run(&mut app, &mut schedules);
        assert!(app.world().get_entity(overlays[1]).is_err());
        assert_eq!(app.world().resource::<Assets<ClipmapGridMaterial>>().len(), 0);
        assert!(app.world().get_entity(instances[1].1).is_ok());
    }

    #[test] // 开启、Mesh换代、高度修订与关闭都复用正式资产并完整回收辅助状态。
    fn clipmap_grid_shares_mesh_tracks_height_revision_and_cleans_up() {
        let mut app = App::new(); // 最小ECS应用不安装窗口或RenderApp。
        app.init_resource::<TerrainGridPreview>() // 当前场景网格默认关闭。
            .init_resource::<SparseHeightPageRuntime>() // 手工提供正式高度扩展修订。
            .init_resource::<ClipmapTerrainGridRuntime>() // 记录唯一覆盖实体与材质。
            .insert_resource(Assets::<Mesh>::default()) // 正式源Mesh资产表。
            .insert_resource(Assets::<ClipmapGridMaterial>::default()) // 专用材质资产表。
            .add_systems(Update, sync_clipmap_terrain_grid); // 直接运行生产增量同步。
        let first_mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(test_mesh()); // 唯一源Mesh句柄。
        let source = app
            .world_mut()
            .spawn((
                // 构造正式Clipmap可见地形源。
                TerrainGridTarget,             // 公共显式目标。
                DynamicSparseClipmapPrimitive, // 排除默认Wireframe并启用专用覆盖。
                Mesh3d(first_mesh.clone()),    // 当前组合Mesh强句柄。
                Transform::IDENTITY,           // 子覆盖继承本地变换。
                GlobalTransform::IDENTITY,     // 无窗口层级测试的完整变换组件。
            ))
            .id(); // 保存源实体供句柄核对。
        {
            let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>(); // 模拟正式页系统完整采用。
            pages.extension = Some(test_extension()); // 当前高度扩展已就绪。
            pages.extension_revision = 7; // 冻结第一份完整高度页修订。
        } // 完成第一页修订发布。
        app.world_mut().resource_mut::<TerrainGridPreview>().visible = true; // 模拟F12开启显示网格。
        app.update(); // 创建材质与共享Mesh覆盖子实体。
        let (overlay, material) = {
            let runtime = app.world().resource::<ClipmapTerrainGridRuntime>(); // 读取生产同步完成值。
            assert_eq!(runtime.source, Some(source)); // 覆盖绑定唯一正式源。
            assert_eq!(runtime.mesh, Some(first_mesh.id())); // 记录的Mesh就是源句柄。
            assert_eq!(runtime.extension_revision, 7); // 采用当前完整高度页修订。
            (runtime.overlay.unwrap(), runtime.material.clone().unwrap()) // 保存后续换代核对身份。
        }; // 完成首轮运行时证据读取。
        assert_eq!(
            app.world().get::<Mesh3d>(overlay).unwrap().0.id(),
            first_mesh.id()
        ); // 覆盖不产生Mesh副本。
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1); // 开启网格后资产表仍只有源Mesh。
        assert_eq!(
            app.world().resource::<Assets<ClipmapGridMaterial>>().len(),
            1
        ); // 当前只有一份小型线框材质。
        let second_mesh = app
            .world_mut()
            .resource_mut::<Assets<Mesh>>()
            .add(test_mesh()); // 模拟固定拓扑热换代的新句柄。
        app.world_mut()
            .entity_mut(source)
            .insert(Mesh3d(second_mesh.clone())); // 原位替换正式源Mesh。
        {
            let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>(); // 模拟新高度页完整原子采用。
            pages.extension = Some(test_extension()); // 新修订提供同结构但独立正式绑定快照。
            pages.extension_revision = 8; // 单调修订唤醒材质原位更新。
        } // 完成第二页修订发布。
        app.update(); // 同一帧更新覆盖Mesh与材质扩展。
        {
            let runtime = app.world().resource::<ClipmapTerrainGridRuntime>(); // 读取换代后稳定身份。
            assert_eq!(runtime.overlay, Some(overlay)); // Mesh与页修订不重建覆盖实体。
            assert_eq!(
                runtime.material.as_ref().map(Handle::id),
                Some(material.id())
            ); // 材质资产句柄原位复用。
            assert_eq!(runtime.extension_revision, 8); // 只采用当前正式页系统发布的新修订。
        } // 结束只读资源借用，允许随后修改开关。
        assert_eq!(
            app.world().get::<Mesh3d>(overlay).unwrap().0.id(),
            second_mesh.id()
        ); // 覆盖自动跟随Mesh句柄替换。
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 2); // 只有测试显式创建的两份正式Mesh，没有第三份复制。
        {
            let mut pages = app.world_mut().resource_mut::<SparseHeightPageRuntime>(); // 模拟同一源上迟到的旧高度页任务。
            pages.extension = Some(test_extension()); // 旧任务即使携带完整绑定也不能覆盖当前材质。
            pages.extension_revision = 7; // 小于已采用修订8，必须被单调门拒绝。
        } // 完成迟到旧修订发布模拟。
        app.update(); // 稳定覆盖应忽略倒退修订。
        {
            let runtime = app.world().resource::<ClipmapTerrainGridRuntime>(); // 读取迟到结果处理后的身份。
            assert_eq!(runtime.extension_revision, 8); // 已采用修订保持最新值，不倒退到7。
            assert_eq!(
                runtime.material.as_ref().map(Handle::id),
                Some(material.id())
            ); // 迟到修订也不重建或替换专用材质资产。
        } // 完成迟到修订拒绝断言。
        app.world_mut().resource_mut::<TerrainGridPreview>().visible = false; // 独立关闭不影响正式源。
        app.update(); // 回收覆盖实体和专用材质。
        assert!(app.world().get_entity(overlay).is_err()); // 纯渲染覆盖实体已销毁。
        assert!(app.world().get_entity(source).is_ok()); // 正式Clipmap源继续存在。
        assert_eq!(
            app.world().resource::<Assets<ClipmapGridMaterial>>().len(),
            0
        ); // 专用材质资产已回收。
        assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 2); // 正式Mesh资产完全未被移除。
    } // 完成Clipmap专用网格生命周期回归。
} // 结束Clipmap网格无窗口测试。
