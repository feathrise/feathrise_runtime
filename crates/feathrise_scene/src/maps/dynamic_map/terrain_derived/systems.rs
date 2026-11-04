//! 动态地形源发现、分帧捕获、后台派生和原子采用系统。

use feathrise_scene_core::residency::InstanceQuery; // 实例实体查询只借用当前来源域。
use feathrise_scene_core::residency::{InstanceRes, InstanceResMut}; // 每个子地图借用独立状态，父图保留原资源。
use avian3d::prelude::{Collider, TrimeshFlags}; // 后台构建正式静态三角网格碰撞。
use bevy::asset::{AssetEvent, RenderAssetUsages}; // Mesh热更与内存Image主/渲染世界用途。
use bevy::gltf::{GltfMaterialName, GltfMeshName}; // 正式运行实体复制既有地表系统依赖的稳定GLB名称身份。
use bevy::image::ImageSampler; // 数值高度纹理固定最近采样。
use bevy::math::{Affine3A, Vec3}; // 世界变换和Collider顶点转换。
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues}; // 严格读取GLB TriangleList位置与索引。
use bevy::prelude::*; // ECS系统参数、Mesh3d、Image和Assets。
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat}; // 构造内存RGBA16_UNORM二维Image。
use bevy::tasks::{AsyncComputeTaskPool, futures::check_ready}; // 后台单飞与主线程非阻塞轮询。
use feathrise_map_content::{ActiveDynamicMap, MapManifestAsset}; // 当前MapId与清单采样规格。
use feathrise_map_terrain::{
    // 共享纯算法与编辑索引构建。
    TerrainMeshInput,
    TerrainSampleSpec,
    build_terrain_editing_index,
    derive_terrain,
}; // 完成共享地形算法导入。

use super::resources::{
    // 当前域公开缓存、状态与内部单飞任务类型。
    DynamicTerrainCaptureJob,
    DynamicTerrainCapturePhase,
    DynamicTerrainCommitState,
    DynamicTerrainDerivationState,
    DynamicTerrainHeightCache,
    DynamicTerrainHeightData,
    DynamicTerrainHeightPhase,
    DynamicTerrainHeightStatus,
    DynamicTerrainIdentity,
    DynamicTerrainInstallCandidate,
    DynamicTerrainTaskData,
    DynamicTerrainTaskResult,
}; // 完成当前域类型导入。
use crate::DynamicMapTerrainPrimitive; // 完整采用后的正式显示网格稳定标记。

use super::super::{
    // 访问动态地图根和启动门状态。
    resources::{DynamicMapRuntimePhase, DynamicMapRuntimeState, DynamicMapRuntimeStatus},
    systems::{DynamicMapTerrainRoot, DynamicMapTerrainSourcePrimitive},
    vegetation::DynamicVegetationState,
    water::DynamicWaterState,
}; // 完成父域运行时类型导入。

const CAPTURE_BYTES_PER_FRAME: usize = 512 * 1024; // 每帧最多约43k位置或131k索引，不执行整图同步复制。

/// 大型旧输入或候选只转移所有权；新退休任务异步接续上一任务，不在主线程释放或占满worker等待。
fn retire_large_payload<T: Send + 'static>(
    derivation: &mut DynamicTerrainDerivationState,
    payload: T,
) {
    let previous = derivation.retire_task.take(); // 只保留一条有序退休链的末尾句柄。
    derivation.retire_task = Some(AsyncComputeTaskPool::get().spawn(async move {
        if let Some(previous) = previous {
            previous.await; // 仅后台异步等待旧值释放，主线程从不等待任务。
        }
        drop(payload); // 可能释放数百 MiB 网格、Collider或编辑索引。
    })); // 即使离图后系统不再轮询，后台链也能自行排空。
}

/// 旧分帧捕获失效后仍需异步释放每个小块的实际内存。
fn retire_capture_job(derivation: &mut DynamicTerrainDerivationState) {
    if let Some(capture) = derivation.capture.take() {
        retire_large_payload(derivation, capture); // ECS资源只移走轻量 Vec 头，不在主线程遍历几千个块。
    }
}

/// 已完成的退休任务只占一个轻量句柄；新捕获等待整条任务链真正释放。
fn poll_retired_payloads(derivation: &mut DynamicTerrainDerivationState) {
    if derivation
        .retire_task
        .as_mut()
        .and_then(check_ready)
        .is_some()
    {
        derivation.retire_task = None; // 仅释放已完成任务的轻量句柄。
    }
}

/// 监听当前根内唯一地形 primitive 及其 Mesh 事件，合并成一个最新期望代次。
pub(crate) fn observe_dynamic_terrain_source(
    source_transform: feathrise_scene_core::residency::EntitySourceTransform,
    active_map: InstanceRes<ActiveDynamicMap>,         // 当前动态地图稳定身份。
    registry: Option<crate::maps::dynamic_map::sources::MapRuntimeCatalog>, // 取得当前清单强句柄。
    manifests: Res<Assets<MapManifestAsset>>,  // 读取已完整加载清单采样参数。
    candidates: InstanceQuery<(Entity, &Mesh3d, &GlobalTransform), With<DynamicMapTerrainSourcePrimitive>>, // 当前隐藏GLB输入候选。
    mut mesh_events: feathrise_scene_core::residency::ResidentAssetEventReader<Mesh>, // 同句柄内容热更也必须推进代次。
    mut derivation: InstanceResMut<DynamicTerrainDerivationState>, // 合并最新源身份并维持single-flight。
    mut cache: InstanceResMut<DynamicTerrainHeightCache>,          // 热更失败期间保留旧值并撤销未提交候选。
    mut height_status: InstanceResMut<DynamicTerrainHeightStatus>, // 公开等待和期望代次。
) {
    let Some(map_id) = active_map.map_id.as_ref() else {
        return; // 没有稳定MapId时由动态地图总入口发布清单错误。
    }; // 完成当前地图身份门。
    let Some(registry) = registry.as_deref() else {
        return; // 目录未加载时不猜测清单路径。
    }; // 完成目录门。
    let Some(handles) = registry.handles(map_id) else {
        return; // 目录缺失由父级守卫处理并阻断整条子加载链。
    }; // 完成地图目录项门。
    let Some(manifest) = manifests.get(&handles.manifest) else {
        height_status.phase = DynamicTerrainHeightPhase::Waiting; // 清单仍在标准AssetServer加载。
        height_status.map_id = Some(map_id.clone()); // 保留实际地图名称对应身份。
        height_status.detail = format!("正在等待地图{map_id}清单以生成内存高度"); // 明确等待项。
        return; // 没有规格不能启动源捕获。
    }; // 完成清单就绪门。
    let spec = TerrainSampleSpec {
        resolution: manifest.terrain.resolution, // 规则高度网格尺寸继续由清单定义。
        world_min: manifest.world_min,           // X/Z采样范围下界。
        world_max: manifest.world_max,           // X/Z采样范围上界。
        base_height: manifest.terrain.base_height, // u16最低世界Y。
        height_scale: manifest.terrain.height_scale, // u16完整世界高度跨度。
    }; // 完成采样规格快照。
    let mut matches = candidates.iter(); // 动态地图契约只允许一个正式primitive。
    let Some((entity, mesh, transform)) = matches.next() else {
        height_status.phase = DynamicTerrainHeightPhase::Waiting; // GLB层级仍在异步实例化。
        height_status.map_id = Some(map_id.clone()); // 保持当前地图身份。
        height_status.detail = format!("正在等待地图{map_id}唯一GLB地形网格"); // 明确不是读取R16。
        return; // 下一帧Added标记出现后继续。
    }; // 完成唯一候选存在门。
    if matches.next().is_some() {
        height_status.phase = DynamicTerrainHeightPhase::Failed; // 多primitive违反正式动态地形契约。
        height_status.map_id = Some(map_id.clone()); // 错误归属当前地图。
        height_status.detail =
            format!("地图{map_id}出现多个动态地形primitive，必须只保留一个白模网格"); // Blender作者可直接定位。
        return; // 不任意选择其中一个高度源。
    } // 完成唯一primitive门。
    let source_mesh = mesh.0.id(); // 强句柄身份用于事件过滤与迟到拒收。
    let transform_affine = source_transform.affine(entity, transform); // 整体实例变换不改变源图高度与碰撞派生身份。
    let transform_bits = affine_bits(transform_affine); // O(1)比较变换换代。
    let source_changed = derivation.desired.as_ref().is_none_or(|identity| {
        identity.map_id != *map_id
            || identity.source_entity != entity
            || identity.source_mesh != source_mesh
    }) || derivation.desired_transform_bits != Some(transform_bits)
        || derivation.spec != Some(spec); // 地图、实体、Mesh、变换或采样规格任一变化都建立新代次。
    let mesh_changed = mesh_events.read().any(|event| {
        event_asset_id(event) == source_mesh
            && matches!(
                event,
                AssetEvent::Modified { .. } | AssetEvent::Removed { .. }
            )
    }); // 只消费并匹配当前明确Mesh句柄。
    if !source_changed && !mesh_changed {
        return; // 稳定帧只完成常数身份比较，不扫描网格或高度。
    } // 完成稳定早退。
    derivation.generation = derivation.generation.wrapping_add(1).max(1); // 零保留给未开始状态。
    let identity = DynamicTerrainIdentity {
        map_id: map_id.clone(),            // 冻结当前地图身份。
        generation: derivation.generation, // 冻结最新输入代次。
        source_entity: entity,             // 冻结正式primitive实体。
        source_mesh,                       // 冻结源Mesh资产身份。
    }; // 完成目标身份构造。
    derivation.desired = Some(identity); // 后续捕获只接受该精确目标。
    derivation.desired_transform = Some(transform_affine); // 捕获开始使用同一世界变换。
    derivation.desired_transform_bits = Some(transform_bits); // 捕获每帧再次核对层级未变。
    derivation.spec = Some(spec); // 后台任务使用同清单规格。
    let old_capture = derivation.capture.take(); // 换代只移动部分捕获的所有权。
    let old_install = derivation.pending_install.take(); // 旧显示/碰撞候选不得进入新版本提交。
    let old_pending = cache.pending.take(); // 水域与植被不得继续基于过期高度准备。
    if old_capture.is_some() || old_install.is_some() || old_pending.is_some() {
        retire_large_payload(&mut derivation, (old_capture, old_install, old_pending)); // 大缓冲、Collider和编辑索引在后台释放。
    }
    derivation.dirty_after_task = derivation.task.is_some(); // 在途CPU任务自然完成后再启动最新目标。
    height_status.phase = if derivation.task.is_some() {
        DynamicTerrainHeightPhase::Building // 旧任务仍占唯一槽但结果将按身份丢弃。
    } else {
        DynamicTerrainHeightPhase::Waiting // 下一系统可在同帧创建捕获游标。
    }; // 完成公开阶段更新。
    height_status.map_id = Some(map_id.clone()); // 发布当前目标地图。
    height_status.desired_generation = derivation.generation; // 旧Ready等式立即失效。
    height_status.detail = if cache.ready_for(map_id).is_some() {
        format!("地图{map_id}地形已变化，保留上一完整高度并等待新代次") // 热更期间保持旧水域、植被与碰撞。
    } else {
        format!("地图{map_id}正在从GLB生成内存高度") // 首载明确不读取磁盘高度图。
    }; // 完成等待诊断。
} // 结束源观察系统。

/// 在固定字节预算内冻结当前源 POSITION 和逻辑索引，完整后启动唯一后台任务。
pub(crate) fn capture_dynamic_terrain_source(
    frame: feathrise_scene_core::residency::InstanceFrame,
    source_transform: feathrise_scene_core::residency::EntitySourceTransform,
    meshes: Res<Assets<Mesh>>, // 只读源Mesh属性，不克隆完整资产。
    sources: InstanceQuery<(&Mesh3d, &GlobalTransform), With<DynamicMapTerrainSourcePrimitive>>, // 每帧核对隐藏源实体与变换。
    mut derivation: InstanceResMut<DynamicTerrainDerivationState>, // 保存跨帧捕获游标和唯一任务槽。
    mut status: InstanceResMut<DynamicTerrainHeightStatus>,        // 公开捕获进度或输入失败。
) {
    poll_retired_payloads(&mut derivation); // 即使旧CPU任务仍在跑，也非阻塞回收已完成的释放句柄。
    if derivation.retire_task.is_some() {
        return; // 旧大缓冲释放完毕前不启动下一份捕获，限制热更峰值与任务数。
    }
    if derivation.task.is_some() {
        return; // 不可协作取消的旧任务真正完成前保持single-flight背压。
    } // 完成任务槽门。
    if derivation.desired.is_some() && derivation.attempted == derivation.desired {
        return; // 完成后的同代地形不得再制造pending，否则已提交的水域不会再次提供候选，草地将永久等不到共同提交。
    }
    if derivation.capture.is_none() {
        let Some(identity) = derivation.desired.clone() else {
            return; // 尚未发现正式地形源。
        }; // 完成目标身份门。
        let Ok((mesh_handle, transform)) = sources.get(identity.source_entity) else {
            return; // WorldAsset热重载过渡帧等待新目标观察。
        }; // 完成源实体存在门。
        if mesh_handle.0.id() != identity.source_mesh {
            return; // 观察系统下一帧会建立新Mesh身份。
        } // 完成源句柄门。
        let source_affine = source_transform.affine(identity.source_entity, transform);
        let transform_bits = affine_bits(source_affine); // 捕获与观察共用确定性的源图内部变换。
        if derivation.desired_transform_bits != Some(transform_bits) {
            return; // 变换换代等待观察系统推进代次。
        } // 完成源变换门。
        let Some(mesh) = meshes.get(identity.source_mesh) else {
            status.phase = DynamicTerrainHeightPhase::Waiting; // Mesh资产本体尚未进入Assets。
            status.detail = format!("正在等待地图{}地形Mesh资产", identity.map_id); // 明确等待项。
            return; // AssetEvent到达后继续。
        }; // 完成Mesh资产门。
        let job = match new_capture_job(identity, source_affine, transform_bits, mesh) {
            Ok(job) => job, // 合法轻量元数据建立跨帧游标。
            Err(error) => {
                status.phase = DynamicTerrainHeightPhase::Failed; // 输入形状错误无需启动后台任务。
                status.detail = error; // 保留具体顶点、索引或拓扑错误。
                return; // 等待GLB/Mesh后续热更恢复。
            } // 完成捕获任务构造失败分支。
        }; // 完成捕获任务构造。
        derivation.capture = Some(job); // 安装唯一分帧捕获任务。
    } // 完成捕获游标懒创建。
    let identity = derivation
        .capture
        .as_ref()
        .expect("上方已建立捕获任务")
        .identity
        .clone(); // 先冻结轻量身份避免借用冲突。
    let Ok((mesh_handle, transform)) = sources.get(identity.source_entity) else {
        retire_capture_job(&mut derivation); // 源实体消失时后台释放部分捕获。
        return; // 新WorldAsset primitive稍后重新发现。
    }; // 完成捕获期实体门。
    if mesh_handle.0.id() != identity.source_mesh
        || affine_bits(source_transform.affine(identity.source_entity, transform))
            != derivation
                .capture
                .as_ref()
                .expect("捕获任务仍存在")
                .transform_bits
    {
        retire_capture_job(&mut derivation); // 只比较源图内部变换；整体移动不取消捕获，内部输入换代仍后台释放旧块。
        return; // 观察系统建立最新代次。
    } // 完成捕获期身份门。
    let Some(mesh) = meshes.get(identity.source_mesh) else {
        retire_capture_job(&mut derivation); // Mesh被移除时后台释放部分缓冲。
        return; // 修复事件后重新捕获。
    }; // 完成捕获期Mesh门。
    let capture_result = derivation
        .capture
        .as_mut()
        .expect("捕获任务仍存在")
        .capture(mesh, CAPTURE_BYTES_PER_FRAME); // 每帧最多复制固定字节。
    if let Err(error) = capture_result {
        retire_capture_job(&mut derivation); // 损坏换代不得保留半份输入，释放仍在后台。
        status.phase = DynamicTerrainHeightPhase::Failed; // 公开精确捕获错误。
        status.detail = error; // F12可提示作者修复Blender资产。
        return; // 后续Mesh热更再重试。
    } // 完成捕获错误处理。
    let capture = derivation.capture.as_ref().expect("捕获任务仍存在"); // 读取完成度用于诊断。
    status.phase = DynamicTerrainHeightPhase::Capturing; // 任务未启动前不冒充Building。
    status.detail = format!(
        "正在捕获地图{}的GLB地形：顶点{}/{}，索引{}/{}",
        capture.identity.map_id,
        capture.captured_vertices,
        capture.expected_vertices,
        capture.captured_indices,
        capture.expected_indices
    ); // 公开真实跨帧进度。
    if capture.phase != DynamicTerrainCapturePhase::Complete {
        return; // 本帧预算耗尽，下一帧从同一offset继续。
    } // 完成捕获完整门。
    let capture = derivation.capture.take().expect("完整捕获任务必须存在"); // 转移完整拥有输入。
    let spec = derivation.spec.expect("发现源时必须冻结采样规格"); // 同身份规格进入CPU任务。
    let task_identity = capture.identity.clone(); // 后台完成值携带完整身份，分块输入整体移动到任务。
    derivation.attempted = Some(task_identity.clone()); // 启动边沿记录完整身份；失败也等待真实GLB/规格换代后再尝试。
    let physical_scale=frame.frame().source_to_world.to_scale_rotation_translation().0;
    derivation.task = Some(AsyncComputeTaskPool::get().spawn(async move {
        let result = (|| {
            let input = capture.into_input()?; // 只在后台预分配并合并整图连续数组；主线程不搬移大前缀。
            let editing_index = build_terrain_editing_index(&input)?; // 同一拓扑后台建立F12局部空间/邻接索引。
            let derived = derive_terrain(input, spec)?; // 空间分桶生成法线、高度与RGBA16字节。
            let render_mesh = build_render_mesh(&derived); // 大数组克隆与Mesh属性构造也留在后台任务。
            let mut collider = Collider::trimesh_with_config(
                derived
                    .positions
                    .iter()
                    .map(|position| Vec3::from_array(*position))
                    .collect(), // Collider取得独立世界顶点所有权。
                derived
                    .indices
                    .chunks_exact(3)
                    .map(|triangle| [triangle[0], triangle[1], triangle[2]])
                    .collect(), // 保持同序三角连接。
                TrimeshFlags::FIX_INTERNAL_EDGES, // 与原GLB层级构造器沿用相同内部边修正。
            ); // 完成后台Avian Trimesh构建。
            collider.set_scale(physical_scale,10); // 对象父级缩放的BVH在后台预热，物理提交只复用完成缓存。
            Ok(DynamicTerrainTaskData {
                derived,
                editing_index,
                render_mesh,
                collider,
            }) // 全部成功后形成一个原子完成值。
        })(); // 完成纯后台工作闭包。
        DynamicTerrainTaskResult {
            identity: task_identity,
            result,
        } // 成功或失败都带启动身份返回。
    })); // 启动唯一不可协作取消后台任务。
    derivation.dirty_after_task = false; // 当前任务对应启动时最新目标。
    status.phase = DynamicTerrainHeightPhase::Building; // 完整输入已离开主线程。
    status.detail = format!(
        "正在后台生成地图{}的内存高度、法线和Trimesh",
        identity.map_id
    ); // 明确生成内容。
} // 结束分帧捕获系统。

/// 非阻塞轮询唯一后台任务；完整结果只登记为候选，等待依赖系统共同提交。
pub(crate) fn poll_dynamic_terrain_derivation(
    mut images: ResMut<Assets<Image>>, // 每代最多登记一张RGBA16高度Image。
    mut meshes: ResMut<Assets<Mesh>>,  // 后台已构造的正式显示Mesh只登记为隐藏候选。
    mut derivation: InstanceResMut<DynamicTerrainDerivationState>, // 释放任务槽并核对当前目标。
    mut cache: InstanceResMut<DynamicTerrainHeightCache>, // 同时保留上一采用值和一个新候选。
    mut height_status: InstanceResMut<DynamicTerrainHeightStatus>, // 发布候选等待或失败证据。
    mut runtime: InstanceResMut<DynamicMapRuntimeState>, // 首载候选尚未共同提交时保持碰撞门关闭。
    mut runtime_status: InstanceResMut<DynamicMapRuntimeStatus>, // 首载失败进入地图结构化失败状态。
    roots: InstanceQuery<(), With<DynamicMapTerrainRoot>>, // 根必须仍属于当前动态地图。
    sources: InstanceQuery<
        (
            Option<&GltfMeshName>,
            Option<&GltfMaterialName>,
            Option<&MeshMaterial3d<StandardMaterial>>,
        ),
        With<DynamicMapTerrainSourcePrimitive>,
    >, // 候选建立前先确认首次提交所需GLB名称和白模材质仍完整。
) {
    let completed = derivation.task.as_mut().and_then(check_ready); // 未完成时立即返回，不等待线程池。
    let Some(completed) = completed else {
        return; // 稳定或计算帧保持常数成本。
    }; // 完成任务就绪门。
    derivation.task = None; // 真正完成后释放唯一CPU槽。
    if !completed_task_is_current(&derivation, &completed.identity) {
        retire_capture_job(&mut derivation); // 防御性清理任何过时代次的部分捕获。
        retire_large_payload(&mut derivation, completed); // 迟到结果可能持有完整Mesh/Trimesh，后台丢弃。
        return; // 迟到成功或失败都不污染公开当前状态。
    } // 完成任务身份门。
    let data = match completed.result {
        Ok(data) => data, // 全部派生成功才继续登记主世界资产。
        Err(error) => {
            let old_install = derivation.pending_install.take(); // 失败候选不能进入全局提交门。
            let old_pending = cache.pending.take(); // 下游继续使用上一完整已采用高度。
            if old_install.is_some() || old_pending.is_some() {
                retire_large_payload(&mut derivation, (old_install, old_pending)); // 旧候选在后台释放。
            }
            height_status.phase = DynamicTerrainHeightPhase::Failed; // 当前目标明确失败。
            height_status.detail = error.clone(); // F12和诊断面板显示同一具体原因。
            if cache.ready_for(&completed.identity.map_id).is_none() {
                runtime.collision_ready = false; // 首载没有旧Trimesh托底，启动门保持关闭。
                runtime_status.phase = DynamicMapRuntimePhase::Failed; // 地图不能以半套水域/植被进入玩法。
                runtime_status.detail = format!(
                    "地图{}从GLB生成内存高度失败：{error}",
                    completed.identity.map_id
                ); // 绑定实际地图身份。
            } // 热更失败则继续保留上一完整缓存与碰撞。
            return; // 等待GLB后续Modified事件恢复。
        } // 完成当前失败处理。
    }; // 完成后台结果拆分。
    let Some(root) = runtime.root.filter(|root| roots.contains(*root)) else {
        retire_large_payload(&mut derivation, data); // 无主根时大Mesh/Collider必须后台丢弃。
        return; // 切图或根换代后的完成值不得安装到无主实体。
    }; // 完成当前根存在门。
    let Ok((_mesh_name, _material_name, white_material)) =
        sources.get(completed.identity.source_entity)
    else {
        height_status.phase = DynamicTerrainHeightPhase::Failed; // 缺失源元数据时不能构造正式显示候选。
        height_status.detail = format!("地图{}地形源在候选建立前消失", completed.identity.map_id);
        retire_large_payload(&mut derivation, data); // 任何已完成的大数组只在后台释放。
        return;
    }; // 完成源身份存在门。
    if white_material.is_none() {
        height_status.phase = DynamicTerrainHeightPhase::Failed; // 单白模primitive必须具有StandardMaterial。
        height_status.detail = format!("地图{}地形源缺少白模材质", completed.identity.map_id);
        retire_large_payload(&mut derivation, data); // 不在失败帧释放完整Trimesh。
        return;
    } // 完成白模材质门。
    let extent = Extent3d {
        width: completed_identity_width(
            &data.derived,
            derivation.spec.expect("当前任务规格仍存在"),
        ), // 输出单元宽等于顶点宽减一。
        height: derivation.spec.expect("当前任务规格仍存在").resolution[1] - 1, // 输出单元高等于顶点高减一。
        depth_or_array_layers: 1, // 数值高度是普通二维纹理。
    }; // 完成内存Image尺寸。
    let mut image = Image::new(
        extent,                           // 严格使用清单顶点分辨率减一。
        TextureDimension::D2,             // GPU Blade使用二维textureLoad。
        data.derived.height_cells_rgba16, // 转移后台完成字节，不在主线程深拷贝。
        TextureFormat::Rgba16Unorm,       // GPU消费者继续使用既有RGBA16四角通道格式，但不再落盘。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // CPU诊断与GPU上传都保留。
    ); // 完成内存高度Image构造。
    image.sampler = ImageSampler::nearest(); // 数值四角不能被颜色线性插值或sRGB处理。
    let height_image = images.add(image); // 每代主线程只登记一张约2MiB上限Image。
    let spec = derivation.spec.expect("当前任务规格仍存在"); // 冻结完成值采样规格。
    let ready = DynamicTerrainHeightData {
        map_id: completed.identity.map_id.clone(), // 发布实际地图身份。
        generation: completed.identity.generation, // 发布本次输入代次。
        source_entity: completed.identity.source_entity, // 发布正式primitive实体。
        source_mesh: completed.identity.source_mesh, // 发布源Mesh身份。
        spec,                                      // 发布清单采样参数。
        positions: data.derived.positions.into(),  // Vec无拷贝冻结为Arc切片。
        normals: data.derived.normals.into(),      // Vec无拷贝冻结为Arc切片。
        indices: data.derived.indices.into(),      // Vec无拷贝冻结为Arc切片。
        heights: data.derived.heights.into(),      // Vec无拷贝冻结为Arc切片。
        height_image,                              // 保持Image强句柄直到缓存与消费者共同释放。
        editing_index: Arc::new(data.editing_index), // 编辑索引只构建一次并共享。
    }; // 完成共享缓存候选。
    let render_mesh = meshes.add(data.render_mesh); // 主线程只登记一张后台完成Mesh，不遍历顶点。
    let old_install = derivation
        .pending_install
        .replace(DynamicTerrainInstallCandidate {
            identity: completed.identity.clone(), // 全局提交前继续核对最新期望身份。
            root,                                 // 候选只属于当前动态地图根。
            render_mesh,                          // 等待同代水域和植被候选完成。
            collider: data.collider,              // Trimesh与显示Mesh同帧提交。
        }); // 完成地形安装候选。
    let old_pending = cache.pending.replace(ready); // 下游从该候选构建，但正式查询仍读取ready旧值。
    if old_install.is_some() || old_pending.is_some() {
        retire_large_payload(&mut derivation, (old_install, old_pending)); // 异常覆盖旧候选时不在主线程析构。
    }
    if cache.ready_for(&completed.identity.map_id).is_none() {
        runtime.collision_ready = false; // 首载必须等待四个域共同提交。
    }
    height_status.phase = DynamicTerrainHeightPhase::Building; // 派生完成不提前冒充全局Ready。
    height_status.map_id = Some(completed.identity.map_id.clone()); // 公开地图身份。
    height_status.detail = format!(
        "地图{}GLB候选已生成：{}个顶点、{}个三角形、{}个高度样本；正在等待同代水域与植被",
        completed.identity.map_id,
        cache
            .pending
            .as_ref()
            .expect("刚建立的候选必须存在")
            .positions
            .len(),
        cache
            .pending
            .as_ref()
            .expect("刚建立的候选必须存在")
            .indices
            .len()
            / 3,
        cache
            .pending
            .as_ref()
            .expect("刚建立的候选必须存在")
            .heights
            .len()
    ); // 公开真实预算和结果规模。
    runtime_status.phase = DynamicMapRuntimePhase::BuildingCollision; // 下游地表/边界仍由总启动门核对。
    runtime_status.detail = format!(
        "地图{}地形派生已完成，正在等待同代水域与植被",
        completed.identity.map_id
    );
} // 结束后台结果轮询系统。

/// 当地形候选及其水域、植被依赖均完整就绪时，建立只存活一个Update的共同提交令牌。
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_dynamic_terrain_commit(
    active_map: InstanceRes<ActiveDynamicMap>, // 只允许当前玩法地图建立提交令牌。
    registry: Option<crate::maps::dynamic_map::sources::MapRuntimeCatalog>, // 判断水域和植被是否为本地图必需依赖。
    cache: InstanceRes<DynamicTerrainHeightCache>, // 读取尚未采用的地形候选身份。
    derivation: InstanceRes<DynamicTerrainDerivationState>, // 核对显示/碰撞候选与高度候选同代。
    water: InstanceRes<DynamicWaterState>,     // 查询同代水域隐藏候选是否完整。
    vegetation: InstanceRes<DynamicVegetationState>, // 查询同代植被隐藏候选是否完整。
    water_status: InstanceRes<super::super::water::DynamicWaterStatus>, // 失败时形成明确阻塞诊断。
    vegetation_status: InstanceRes<super::super::vegetation::DynamicVegetationStatus>, // 同上。
    mut commit: InstanceResMut<DynamicTerrainCommitState>, // 全部条件满足后武装同帧令牌。
    mut height_status: InstanceResMut<DynamicTerrainHeightStatus>, // 公布等待或依赖失败原因。
    mut runtime_status: InstanceResMut<DynamicMapRuntimeStatus>, // 首载失败不能停在无说明等待。
) {
    if commit.is_armed() {
        return; // 上一系统链尚未消费令牌时不得覆盖身份。
    }
    let Some(map_id) = active_map.map_id.as_ref() else {
        return;
    };
    let Some(candidate) = cache.pending_for(map_id) else {
        return; // 尚未完成地形派生。
    };
    let Some(install) = derivation.pending_install.as_ref() else {
        return; // CPU/GPU高度与显示/碰撞候选必须成套存在。
    };
    if install.identity.map_id != *map_id
        || install.identity.generation != candidate.generation
        || derivation.desired.as_ref() != Some(&install.identity)
    {
        return; // 换图或连续GLB保存后的旧候选不允许建立令牌。
    }
    let Some(handles) = registry
        .as_deref()
        .and_then(|registry| registry.handles(map_id))
    else {
        return; // 目录换代窗口继续保留上一完整运行版本。
    };
    let water_ready =
        water.candidate_ready_for_terrain(map_id, candidate.generation, handles.water.is_some());
    let vegetation_ready = vegetation.candidate_ready_for_terrain(
        map_id,
        candidate.generation,
        handles.vegetation.is_some(),
    );
    if !water_ready || !vegetation_ready {
        let failed_dependency = (!water_ready
            && water_status.phase == super::super::water::DynamicWaterPhase::Failed)
            || (!vegetation_ready
                && vegetation_status.phase
                    == super::super::vegetation::DynamicVegetationPhase::Failed);
        height_status.phase = if failed_dependency {
            DynamicTerrainHeightPhase::Failed // 热更失败保留上一完整四域版本。
        } else {
            DynamicTerrainHeightPhase::Building
        };
        height_status.detail = match (water_ready, vegetation_ready) {
            (false, false) => format!("地图{map_id}地形候选正在等待同代水域与植被"),
            (false, true) => format!("地图{map_id}地形候选正在等待同代水域"),
            (true, false) => format!("地图{map_id}地形候选正在等待同代植被"),
            (true, true) => unreachable!("上方等待分支至少有一个依赖未就绪"),
        };
        if failed_dependency && cache.ready_for(map_id).is_none() {
            runtime_status.phase = DynamicMapRuntimePhase::Failed;
            runtime_status.detail = format!("地图{map_id}首个GLB地形候选的水域或植被派生失败");
        }
        return;
    }
    commit.arm(map_id.clone(), candidate.generation); // 后续三个提交系统在同一Update读取同一令牌。
} // 结束共同提交准备系统。

/// 在共同令牌下替换正式地形Mesh、Trimesh和公开高度缓存。
pub(crate) fn commit_dynamic_terrain_derivation(
    mut commands: Commands, // 同一渲染帧提交正式显示与碰撞组件。
    mut derivation: InstanceResMut<DynamicTerrainDerivationState>, // 消费显示/碰撞候选。
    mut cache: InstanceResMut<DynamicTerrainHeightCache>, // pending原子提升为ready。
    mut commit: InstanceResMut<DynamicTerrainCommitState>, // 任一意外失配立即撤销下游提交。
    mut height_status: InstanceResMut<DynamicTerrainHeightStatus>, // 只有此处推进applied_generation。
    mut runtime: InstanceResMut<DynamicMapRuntimeState>, // 共同提交后放行碰撞门。
    mut runtime_status: InstanceResMut<DynamicMapRuntimeStatus>, // 发布完整地形采用结果。
    roots: InstanceQuery<(), With<DynamicMapTerrainRoot>>, // 候选根必须仍在当前场景。
    sources: InstanceQuery<
        (
            Option<&GltfMeshName>,
            Option<&GltfMaterialName>,
            Option<&MeshMaterial3d<StandardMaterial>>,
        ),
        With<DynamicMapTerrainSourcePrimitive>,
    >, // 首次显示复制GLB稳定名称与白模材质。
    displays: InstanceQuery<Entity, With<DynamicMapTerrainPrimitive>>, // 热更保留稳定实体和正式地表材质。
) {
    let (Some(install), Some(ready)) =
        (derivation.pending_install.as_ref(), cache.pending.as_ref())
    else {
        return;
    };
    if !commit.matches(&install.identity.map_id, install.identity.generation)
        || ready.map_id != install.identity.map_id
        || ready.generation != install.identity.generation
        || runtime.root != Some(install.root)
        || !roots.contains(install.root)
    {
        commit.clear(); // 下游不得在地形身份失配后单独切换。
        return;
    }
    let mut display_iter = displays.iter();
    let first_display = display_iter.next();
    if display_iter.next().is_some() {
        height_status.phase = DynamicTerrainHeightPhase::Failed;
        height_status.detail = format!("地图{}出现多个正式地形显示实体", install.identity.map_id);
        commit.clear();
        return;
    }
    let source_metadata = if first_display.is_none() {
        match sources.get(install.identity.source_entity) {
            Ok((mesh_name, material_name, Some(white_material))) => Some((
                mesh_name.cloned().unwrap_or_else(|| {
                    GltfMeshName(super::super::DYNAMIC_TERRAIN_MESH_NAME.to_owned())
                }),
                material_name.cloned().unwrap_or_else(|| {
                    GltfMaterialName(super::super::DYNAMIC_TERRAIN_WHITE_MATERIAL_NAME.to_owned())
                }),
                white_material.clone(),
            )),
            _ => {
                height_status.phase = DynamicTerrainHeightPhase::Failed;
                height_status.detail =
                    format!("地图{}地形源在共同提交前消失", install.identity.map_id);
                commit.clear();
                return;
            }
        }
    } else {
        None // 热更沿用稳定显示实体上的正式地表材质。
    };

    let install = derivation
        .pending_install
        .take()
        .expect("上方已校验地形安装候选");
    let ready = cache.pending.take().expect("上方已校验地形高度候选");
    let display = if let Some(display) = first_display {
        commands.entity(display).insert(Mesh3d(install.render_mesh));
        display
    } else {
        let (mesh_name, material_name, white_material) =
            source_metadata.expect("首次显示已校验GLB元数据");
        let display = commands
            .spawn((
                Name::new(format!("DynamicMapTerrainDisplay_{}", ready.map_id)),
                DynamicMapTerrainPrimitive,
                Mesh3d(install.render_mesh),
                white_material,
                mesh_name,
                material_name,
                Transform::IDENTITY, // 顶点已烘焙完整GLB节点变换。
                Visibility::Inherited,
            ))
            .id();
        commands.entity(install.root).add_child(display);
        display
    };
    commands.entity(display).insert(install.collider); // 同一Update同时替换显示和Trimesh。
    runtime.terrain_display = Some(display);
    let vertex_count = ready.positions.len();
    let triangle_count = ready.indices.len() / 3;
    let height_count = ready.heights.len();
    let generation = ready.generation;
    let map_id = ready.map_id.clone();
    let old_ready = cache.ready.replace(ready); // 从此刻起正式查询、水域和植被都指向同代完成值。
    if let Some(old_ready) = old_ready {
        retire_large_payload(&mut derivation, old_ready); // 热更旧地形Arc/编辑索引可能为数百MiB，只在后台析构。
    }
    runtime.collision_ready = true;
    height_status.phase = DynamicTerrainHeightPhase::Ready;
    height_status.map_id = Some(map_id.clone());
    height_status.applied_generation = generation;
    height_status.detail = format!(
        "地图{map_id}已原子采用GLB派生版本{generation}：{vertex_count}个顶点、{triangle_count}个三角形、{height_count}个高度样本"
    );
    runtime_status.phase = DynamicMapRuntimePhase::BuildingCollision;
    runtime_status.detail =
        format!("地图{map_id}同代地形、碰撞、水域与植被已提交，正在等待其他地图内容");
} // 结束正式地形提交系统。

/// 三个提交系统运行后清除一次性令牌，防止下一帧重复切换。
pub(crate) fn finish_dynamic_terrain_commit(mut commit: InstanceResMut<DynamicTerrainCommitState>) {
    commit.clear();
} // 结束提交令牌清理系统。

/// 离图清理轻量目标与缓存；不可协作任务保留句柄直至完成后自然丢弃。
pub(crate) fn clear_dynamic_terrain_derivation(
    mut derivation: InstanceResMut<DynamicTerrainDerivationState>, // 清理当前目标和部分捕获。
    mut cache: InstanceResMut<DynamicTerrainHeightCache>,          // 离图释放大数组与Image强句柄。
    mut commit: InstanceResMut<DynamicTerrainCommitState>,         // 离图撤销尚未消费的共同提交令牌。
    mut status: InstanceResMut<DynamicTerrainHeightStatus>,        // 发布离图等待状态。
) {
    derivation.generation = derivation.generation.wrapping_add(1).max(1); // 使任何迟到任务身份失效。
    derivation.desired = None; // 下一地图必须重新发现唯一primitive。
    derivation.attempted = None; // 离图清除尝试身份，新地图不会继承旧完成门。
    derivation.desired_transform = None; // 释放旧节点变换。
    derivation.desired_transform_bits = None; // 清除旧变换身份。
    derivation.spec = None; // 清除旧清单采样规格。
    let old_capture = derivation.capture.take(); // 尚未启动CPU任务的部分缓冲只移动所有权。
    derivation.dirty_after_task = derivation.task.is_some(); // 旧CPU任务真正完成前仍占唯一槽。
    let old_install = derivation.pending_install.take(); // 未提交显示/碰撞候选不得跨地图。
    let old_ready = cache.ready.take(); // 离图不让下一地图读取旧高度。
    let old_pending = cache.pending.take(); // 候选CPU/GPU高度也不跨地图。
    if old_capture.is_some()
        || old_install.is_some()
        || old_ready.is_some()
        || old_pending.is_some()
    {
        retire_large_payload(
            &mut derivation,
            (old_capture, old_install, old_ready, old_pending),
        ); // 大缓冲在后台释放，资产与地图身份已同步撤销。
    }
    commit.clear(); // 下游提交系统不会在OnExit后读取旧令牌。
    *status = DynamicTerrainHeightStatus {
        detail: "已离开动态地图".to_owned(), // F12/F3显示明确空状态。
        ..default()
    }; // 完成公开状态重置。
} // 结束离图清理。

/// 从当前Mesh轻量元数据建立空捕获游标，不复制完整属性。
fn new_capture_job(
    identity: DynamicTerrainIdentity, // 当前目标完整身份。
    transform: Affine3A,              // 节点到世界变换。
    transform_bits: [u32; 12],        // 捕获期O(1)变换校验。
    mesh: &Mesh,                      // 当前源Mesh只读资产。
) -> Result<DynamicTerrainCaptureJob, String> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return Err(format!(
            "地图{}地形Mesh拓扑必须是TriangleList",
            identity.map_id
        )); // 不猜测条带或点线拓扑。
    } // 完成拓扑门。
    let positions = mesh
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(VertexAttributeValues::as_float3)
        .ok_or_else(|| format!("地图{}地形Mesh缺少Float32x3 POSITION", identity.map_id))?; // 只读取长度，不复制数组。
    if positions.is_empty() {
        return Err(format!("地图{}地形必须提供至少一个顶点", identity.map_id)); // 删除独立顶点上限后仍拒绝空Mesh。
    } // 完成非空顶点门。
    if u32::try_from(positions.len().saturating_sub(1)).is_err() {
        return Err(format!(
            "地图{}地形顶点数{}超过U32索引可表示范围",
            identity.map_id,
            positions.len()
        )); // 显式及规范化索引都使用U32，不能允许位置身份截断回绕。
    } // 完成顶点身份表示门；不再施加独立数量预算。
    let (expected_indices, indexed) = mesh
        .indices()
        .map_or((positions.len(), false), |indices| (indices.len(), true)); // 无索引TriangleList按连续顶点规范化。
    if expected_indices == 0 || !expected_indices.is_multiple_of(3) {
        return Err(format!(
            "地图{}地形逻辑索引数{}不是合法非空TriangleList",
            identity.map_id, expected_indices
        )); // 损坏索引不进入分帧捕获。
    } // 完成逻辑索引门。
    let triangle_count = expected_indices / 3; // TriangleList每三个逻辑索引形成一个三角形。
    if u32::try_from(triangle_count).is_err() {
        return Err(format!(
            "地图{}地形三角形数{}超过U32三角身份可表示范围",
            identity.map_id, triangle_count
        )); // 后台空间桶使用U32三角身份，不能允许身份回绕。
    } // 完成三角身份表示门；不再施加独立数量预算。
    if positions.len() > expected_indices {
        return Err(format!(
            "地图{}地形顶点数{}超过逻辑索引数{}，必然包含未引用顶点",
            identity.map_id,
            positions.len(),
            expected_indices
        )); // 三角形预算继续间接约束有效输入规模，同时不再限制合法高密度共享顶点。
    } // 完成顶点可引用性必要条件门。
    Ok(DynamicTerrainCaptureJob {
        identity,                                     // 保存完整任务身份。
        transform,                                    // 捕获每个位置时应用冻结世界变换。
        transform_bits,                               // 每帧核对层级未变。
        phase: DynamicTerrainCapturePhase::Positions, // 第一阶段复制位置。
        offset: 0,                                    // 从属性首元素开始。
        positions: Vec::new(), // 每帧追加一片固定小缓冲，不扩容既有整图前缀。
        indices: Vec::new(),   // 索引同样保存为当帧小片。
        captured_vertices: 0,  // 位置进度不再依赖外层块数量。
        captured_indices: 0,   // 索引进度不再依赖外层块数量。
        expected_vertices: positions.len(), // 完整采用数量门。
        expected_indices,      // 完整采用数量门。
        indexed,               // 记录源是否有显式索引。
    }) // 完成空捕获任务构造。
} // 结束捕获任务构造。

impl DynamicTerrainCaptureJob {
    /// 在本帧字节预算内继续复制世界位置或U32逻辑索引。
    fn capture(&mut self, mesh: &Mesh, mut remaining_bytes: usize) -> Result<(), String> {
        while remaining_bytes > 0 && self.phase != DynamicTerrainCapturePhase::Complete {
            match self.phase {
                DynamicTerrainCapturePhase::Positions => {
                    let values = mesh
                        .attribute(Mesh::ATTRIBUTE_POSITION)
                        .and_then(VertexAttributeValues::as_float3)
                        .ok_or_else(|| "捕获期间地形POSITION格式失效".to_owned())?; // 热更不能把两种格式拼接。
                    if values.len() != self.expected_vertices {
                        return Err("捕获期间地形顶点数量变化".to_owned()); // 数量换代必须从头重捕获。
                    } // 完成顶点数量稳定门。
                    let count = values
                        .len()
                        .saturating_sub(self.offset)
                        .min(remaining_bytes / 12); // 每位置固定十二字节预算。
                    if count == 0 {
                        break; // 剩余预算不足一个位置时立即让出本帧。
                    } // 完成单元素预算门。
                    let mut chunk = Vec::new(); // 此块独立拥有当帧位置，不扩容以前帧的大数组。
                    chunk
                        .try_reserve_exact(count)
                        .map_err(|error| format!("分帧捕获地形位置内存不足：{error}"))?; // 单次申请最多当前帧预算。
                    chunk.extend(
                        values[self.offset..self.offset + count]
                            .iter()
                            .map(|position| {
                                self.transform
                                    .transform_point3(Vec3::from_array(*position))
                                    .to_array() // 捕获时烘焙完整Blender节点变换。
                            }),
                    ); // 当前块只复制本帧允许的连续位置片段。
                    self.positions.push(chunk); // 外层仅增长一个轻量块描述符。
                    self.captured_vertices += count; // 记录真实完整进度。
                    self.offset += count; // 推进位置游标。
                    remaining_bytes -= count * 12; // 扣除真实目标字节。
                    if self.offset == self.expected_vertices {
                        self.phase = DynamicTerrainCapturePhase::Indices; // 位置完整后进入索引阶段。
                        self.offset = 0; // 新属性从零开始。
                    } // 完成位置阶段切换。
                } // 结束位置捕获分支。
                DynamicTerrainCapturePhase::Indices => {
                    let count = self
                        .expected_indices
                        .saturating_sub(self.offset)
                        .min(remaining_bytes / 4); // 统一按目标U32四字节预算。
                    if count == 0 {
                        break; // 剩余预算不足一个索引时立即让出本帧。
                    } // 完成单索引预算门。
                    if self.indexed {
                        let indices = mesh
                            .indices()
                            .ok_or_else(|| "捕获期间显式地形索引被移除".to_owned())?; // 热更不能切换索引模式。
                        if indices.len() != self.expected_indices {
                            return Err("捕获期间地形索引数量变化".to_owned()); // 数量换代必须从头重捕获。
                        } // 完成索引数量稳定门。
                        let mut chunk = Vec::new(); // 每片最多当前帧剩余的四字节索引预算。
                        chunk
                            .try_reserve_exact(count)
                            .map_err(|error| format!("分帧捕获地形索引内存不足：{error}"))?; // 不因旧索引前缀扩容导致整图同步搬移。
                        match indices {
                            Indices::U16(values) => chunk.extend(
                                values[self.offset..self.offset + count]
                                    .iter()
                                    .map(|value| *value as u32),
                            ), // U16逐片提升为U32。
                            Indices::U32(values) => {
                                chunk.extend_from_slice(&values[self.offset..self.offset + count])
                            } // U32逐片原样复制。
                        } // 完成源索引格式分支。
                        self.indices.push(chunk); // 当前小片独立常驻至后台合并。
                    } else {
                        let mut chunk = Vec::new(); // 无索引输入同样按固定小片规范化。
                        chunk
                            .try_reserve_exact(count)
                            .map_err(|error| format!("分帧捕获顺序索引内存不足：{error}"))?; // 不一次分配整图索引。
                        chunk.extend((self.offset..self.offset + count).map(|value| value as u32)); // 无索引TriangleList规范化为顺序U32。
                        self.indices.push(chunk); // 把该片所有权留到后台合并。
                    } // 完成无索引兼容分支。
                    self.captured_indices += count; // 记录真实完整索引进度。
                    self.offset += count; // 推进逻辑索引游标。
                    remaining_bytes -= count * 4; // 扣除真实目标字节。
                    if self.offset == self.expected_indices {
                        self.phase = DynamicTerrainCapturePhase::Complete; // 完整输入允许启动后台任务。
                        self.offset = 0; // 完成阶段不再保留属性内偏移。
                    } // 完成索引阶段切换。
                } // 结束索引捕获分支。
                DynamicTerrainCapturePhase::Complete => break, // 完成任务不再读取Mesh。
            } // 完成当前捕获阶段分支。
        } // 完成本帧固定预算循环。
        Ok(()) // 返回时要么完整，要么等待下一帧继续。
    } // 结束分帧捕获。

    /// 完整小片只在后台任务中合并为共享派生算法需要的连续输入。
    fn into_input(self) -> Result<TerrainMeshInput, String> {
        if self.phase != DynamicTerrainCapturePhase::Complete
            || self.captured_vertices != self.expected_vertices
            || self.captured_indices != self.expected_indices
        {
            return Err("分帧捕获地形块数量与预期不一致".to_owned()); // 不把半份网格交给派生算法。
        }
        let mut positions = Vec::new(); // 整图位置申请只发生在后台计算池。
        positions
            .try_reserve_exact(self.expected_vertices)
            .map_err(|error| format!("合并地形位置内存不足：{error}"))?; // 明确报告大图内存分配失败。
        for chunk in self.positions {
            positions.extend(chunk); // 在后台复制一个已冻结片并立即释放该片容量。
        }
        let mut indices = Vec::new(); // 整图索引同样只在后台形成。
        indices
            .try_reserve_exact(self.expected_indices)
            .map_err(|error| format!("合并地形索引内存不足：{error}"))?; // 不在发布帧申请或搬移索引。
        for chunk in self.indices {
            indices.extend(chunk); // U32顺序与原Mesh完全一致。
        }
        Ok(TerrainMeshInput { positions, indices }) // 交给法线、高度、编辑索引与碰撞的同一完整拓扑。
    }
} // 结束捕获任务实现。

/// 提取所有AssetEvent变体的类型化Mesh身份。
fn event_asset_id(event: &AssetEvent<Mesh>) -> AssetId<Mesh> {
    match event {
        AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::Removed { id }
        | AssetEvent::LoadedWithDependencies { id }
        | AssetEvent::Unused { id } => *id, // 所有变体都只返回轻量类型化ID。
    } // 完成资产事件身份读取。
} // 结束事件身份辅助函数。

/// 把Affine3A十二个Float32分量转为稳定位模式身份。
fn affine_bits(affine: Affine3A) -> [u32; 12] {
    let columns = affine.to_cols_array(); // Bevy/glam以十二个f32列主序公开Affine3A。
    columns.map(f32::to_bits) // NaN位模式也能稳定触发后续纯算法有限值失败。
} // 结束变换身份编码。

/// 返回完成值高度单元宽度并防御未来输出与规格不一致。
fn completed_identity_width(
    _derived: &feathrise_map_terrain::TerrainDerivedData,
    spec: TerrainSampleSpec,
) -> u32 {
    spec.resolution[0] - 1 // 派生算法已严格验证字节长度，主线程无需再扫描。
} // 结束高度Image宽度辅助函数。

/// 只有仍对应最新GLB目标、且运行期间没有收到新版本的后台结果才可采用。
fn completed_task_is_current(
    derivation: &DynamicTerrainDerivationState,
    identity: &DynamicTerrainIdentity,
) -> bool {
    derivation.desired.as_ref() == Some(identity) && !derivation.dirty_after_task // GLB再次变化会同时换代身份并标记在途任务过期。
} // 结束后台完成值身份门。

/// 在后台把派生数组复制进Bevy渲染Mesh，采用帧不再执行随整图增长的属性循环。
fn build_render_mesh(derived: &feathrise_map_terrain::TerrainDerivedData) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList, // 动态地形契约始终是一组显式三角形。
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD, // F12射线与GPU渲染都需要正式Mesh。
    ); // 完成空渲染Mesh构造。
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, derived.positions.clone()); // 后台复制世界位置到渲染资产。
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, derived.normals.clone()); // 使用共享面积加权重算法线。
    mesh.insert_indices(Indices::U32(derived.indices.clone())); // 保持原顶点顺序和三角连接关系。
    mesh // 返回可直接登记的完整Mesh。
} // 结束后台渲染Mesh构造。

use std::sync::Arc; // 采用时把编辑索引冻结为共享不可变值。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translated_and_rotated_instance_keeps_multiframe_terrain_capture() {
        use feathrise_scene_core::residency::{InstanceResourceTable, MapInstanceContextAppExt, MapInstanceOwner, MapRuntimeScope};
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin))
            .init_resource::<Assets<Mesh>>()
            .init_instance_resource::<DynamicTerrainDerivationState>()
            .init_instance_resource::<DynamicTerrainHeightStatus>()
            .add_systems(Update, capture_dynamic_terrain_source);
        let instance_transform = Transform::from_xyz(10.0, 5.0, -8.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let instance = app.world_mut().spawn(instance_transform).id();
        feathrise_scene_core::residency::initialize_instance_context(app.world_mut(), instance);
        // 超过单帧512KiB的真实网格，确保变换校验发生在捕获中途，而非只测试一次性小输入。
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::MAIN_WORLD);
        let positions: Vec<_> = (0..16_384)
            .flat_map(|_| [[0., 0., 0.], [1., 0., 0.], [0., 0., 1.], [1., 0., 1.]])
            .collect();
        let indices: Vec<_> = (0..16_384u32)
            .flat_map(|quad| {
                let start = quad * 4;
                [start, start + 2, start + 1, start + 1, start + 2, start + 3]
            })
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_indices(Indices::U32(indices));
        let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh);
        let source = app.world_mut().spawn((
            DynamicMapTerrainSourcePrimitive, Mesh3d(handle.clone()), Transform::IDENTITY,
            GlobalTransform::from(instance_transform), MapInstanceOwner(instance), ChildOf(instance),
        )).id();
        let identity = DynamicTerrainIdentity {
            source_entity: source, source_mesh: handle.id(), ..capture_identity()
        };
        *app.world_mut().resource_mut::<InstanceResourceTable<DynamicTerrainDerivationState>>()
            .values.get_mut(&instance).unwrap() = DynamicTerrainDerivationState {
            desired: Some(identity.clone()),
            desired_transform: Some(Affine3A::IDENTITY),
            desired_transform_bits: Some(affine_bits(Affine3A::IDENTITY)),
            spec: Some(TerrainSampleSpec {
                resolution: [2, 2], world_min: [0., 0.], world_max: [1., 1.],
                base_height: -1., height_scale: 2.,
            }),
            ..default()
        };
        app.world_mut().resource_mut::<MapRuntimeScope>().instance = Some(instance);
        app.update();
        let state = &app.world().resource::<InstanceResourceTable<DynamicTerrainDerivationState>>().values[&instance];
        assert!(state.capture.as_ref().is_some_and(|capture| capture.captured_vertices > 0),
            "非原点实例必须开始捕获，不能把世界矩阵与源矩阵比较后永久取消");
        app.world_mut().entity_mut(instance).get_mut::<Transform>().unwrap().translation.y += 3.0;
        for _ in 0..4 { app.update(); }
        let state = &app.world().resource::<InstanceResourceTable<DynamicTerrainDerivationState>>().values[&instance];
        assert_eq!(state.attempted.as_ref(), Some(&identity));
        assert!(state.task.is_some() && state.capture.is_none(), "整体移动后应完成同代捕获并启动正式后台派生");
    }

    #[test]
    fn completed_or_failed_terrain_identity_is_not_dispatched_again() {
        let mut app = App::new(); // 使用正式捕获系统验证完成后不会重复启动CPU任务。
        app.add_plugins(MinimalPlugins)
            .init_resource::<Assets<Mesh>>(); // 真实异步池和独立Mesh容器。
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD,
        ); // 有效小地形。
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![[0., 0., 0.], [1., 0., 0.], [0., 0., 1.], [1., 0., 1.]],
        ); // 四个顶点。
        mesh.insert_indices(Indices::U32(vec![0, 2, 1, 1, 2, 3])); // 两个三角形。
        let handle = app.world_mut().resource_mut::<Assets<Mesh>>().add(mesh); // 主世界完整源。
        let entity = app
            .world_mut()
            .spawn((
                DynamicMapTerrainSourcePrimitive,
                Mesh3d(handle.clone()),
                GlobalTransform::IDENTITY,
            ))
            .id(); // 真实可查询源，旧实现会再次捕获它。
        let identity = DynamicTerrainIdentity {
            source_entity: entity,
            source_mesh: handle.id(),
            ..capture_identity() // 完整同代身份。
        };
        app.insert_resource(DynamicTerrainDerivationState {
            desired: Some(identity.clone()),
            attempted: Some(identity), // 模拟任务已结束，候选等待、采用或失败均不应再派发。
            desired_transform: Some(Affine3A::IDENTITY),
            desired_transform_bits: Some(affine_bits(Affine3A::IDENTITY)), // 源没有发生变化。
            spec: Some(TerrainSampleSpec {
                resolution: [2, 2],
                world_min: [0., 0.],
                world_max: [1., 1.],
                base_height: 0.,
                height_scale: 1.,
            }), // 有效小型派生规格。
            ..default() // task、capture、pending_install均已消费，复现旧代码漏洞。
        })
        .init_resource::<DynamicTerrainHeightStatus>()
        .add_systems(Update, capture_dynamic_terrain_source); // 生产调度入口。
        for phase in [
            DynamicTerrainHeightPhase::Building,
            DynamicTerrainHeightPhase::Ready,
            DynamicTerrainHeightPhase::Failed,
        ] {
            app.world_mut()
                .resource_mut::<DynamicTerrainHeightStatus>()
                .phase = phase; // 候选等待、已提交和确定失败三个阶段。
            for _ in 0..3 {
                app.update(); // 稳定帧绝不再次制造同代地形pending。
                let state = app.world().resource::<DynamicTerrainDerivationState>(); // 检查真实任务槽。
                assert!(state.task.is_none() && state.capture.is_none()); // 旧实现第一帧就会重启任务。
                assert_eq!(
                    app.world().resource::<DynamicTerrainHeightStatus>().phase,
                    phase
                ); // 不把已完成阶段重新写回Building。
            }
        }
        app.world_mut()
            .resource_mut::<DynamicTerrainDerivationState>()
            .desired
            .as_mut()
            .unwrap()
            .generation += 1; // 真实源换代允许再次派生。
        app.update();
        let state = app.world().resource::<DynamicTerrainDerivationState>(); // 新代次仍走正常single-flight。
        assert!(state.task.is_some()); // 修复不能禁止正常GLB热更新。
        assert_eq!(state.attempted, state.desired); // 新启动记录新的完整身份。
    }

    const LEGACY_VERTEX_LIMIT: usize = 263_169; // 测试只锁定旧门已移除，生产代码不再导出该限制。
    const LEGACY_TRIANGLE_LIMIT: usize = 524_288; // 测试只锁定旧三角形门已移除。

    /// 构造最小任务身份，供纯Mesh元数据捕获测试复用。
    fn capture_identity() -> DynamicTerrainIdentity {
        DynamicTerrainIdentity {
            map_id: feathrise_map_content::MapId::parse("MAP-large-terrain-capture")
                .expect("测试MapId必须合法"), // 捕获错误需要稳定地图身份。
            generation: 1,                               // 单次测试使用首个有效代次。
            source_entity: Entity::PLACEHOLDER,          // 元数据构造不访问真实World实体。
            source_mesh: Handle::<Mesh>::default().id(), // 元数据构造只保存轻量资产身份。
        } // 完成测试身份。
    }

    #[test] // Bevy捕获入口必须与共享算法一致，不得残留旧顶点或三角形门。
    fn capture_accepts_counts_above_legacy_limits() {
        let vertex_count = LEGACY_VERTEX_LIMIT + 3; // 明确越过旧顶点上限。
        let triangle_count = LEGACY_TRIANGLE_LIMIT + 1; // 明确越过旧三角形上限。
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList, // 显式索引测试Mesh声明三角形列表。
            RenderAssetUsages::MAIN_WORLD,   // 纯主世界属性足以建立捕获游标。
        ); // 完成空测试Mesh。
        mesh.insert_attribute(
            Mesh::ATTRIBUTE_POSITION,            // 使用正式POSITION语义。
            vec![[0.0, 0.0, 0.0]; vertex_count], // 这里只验证元数据门，几何有效性仍由后台共享校验负责。
        ); // 安装超过旧上限的位置表。
        mesh.insert_indices(Indices::U32(vec![0; triangle_count * 3])); // 退化内容留给后台校验，本测试只锁定数量入口。

        let job = new_capture_job(
            capture_identity(),              // 使用稳定测试身份。
            Affine3A::IDENTITY,              // 不引入节点变换。
            affine_bits(Affine3A::IDENTITY), // 捕获期身份与变换完全匹配。
            &mesh,                           // 只读取Mesh轻量属性长度。
        )
        .expect("TriangleList数量不应再被旧顶点或三角形上限拒绝"); // 建立跨帧捕获游标。

        assert_eq!(job.expected_vertices, vertex_count); // 完整大顶点数进入分帧捕获计划。
        assert_eq!(job.expected_indices, triangle_count * 3); // 完整大三角形数进入分帧捕获计划。
        assert!(job.expected_vertices > LEGACY_VERTEX_LIMIT); // 测试必须确实覆盖旧顶点门。
        assert!(job.expected_indices / 3 > LEGACY_TRIANGLE_LIMIT); // 测试必须确实覆盖旧三角形门。
    }

    #[test]
    fn capture_uses_budgeted_chunks_and_merges_them_in_source_order() {
        let positions = vec![
            [0.0, 0.0, 0.0], // 当前单元北西角。
            [1.0, 0.0, 0.0], // 当前单元北东角。
            [0.0, 0.0, 1.0], // 当前单元南西角。
            [1.0, 0.0, 1.0], // 当前单元南东角。
        ]; // 测试真实POSITION顺序。
        let indices = vec![0u16, 2, 1, 1, 2, 3]; // U16源索引应按顺序提升为U32。
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD,
        ); // 仅需主世界属性读取。
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone()); // 安装四个位置。
        mesh.insert_indices(Indices::U16(indices.clone())); // 安装两个共享顶点三角形。
        let mut job = new_capture_job(
            capture_identity(),
            Affine3A::IDENTITY,
            affine_bits(Affine3A::IDENTITY),
            &mesh,
        )
        .expect("测试地形应建立轻量捕获游标"); // 初始化时不得预分配整图。
        assert!(job.positions.is_empty() && job.indices.is_empty()); // 开始帧无大型前缀数组。
        for _ in 0..10 {
            job.capture(&mesh, 16).expect("每帧16字节捕获必须成功"); // 强制切成多个远小于整图的块。
            assert!(job.positions.iter().all(|chunk| chunk.len() * 12 <= 16)); // 每个位置块都受当帧预算约束。
            assert!(job.indices.iter().all(|chunk| chunk.len() * 4 <= 16)); // 每个索引块都受当帧预算约束。
            if job.phase == DynamicTerrainCapturePhase::Complete {
                break; // 完整后交给后台合并阶段。
            }
        }
        assert_eq!(job.captured_vertices, positions.len()); // 进度准确累计跨帧位置。
        assert_eq!(job.captured_indices, indices.len()); // 进度准确累计跨帧索引。
        assert!(job.positions.len() > 1 && job.indices.len() > 1); // 测试确实经过分块路径。
        let input = job.into_input().expect("完整块必须可按原顺序合并"); // 单测同步执行合并，生产只在计算池调用。
        assert_eq!(input.positions, positions); // 世界位置不因分块边界重排。
        assert_eq!(
            input.indices,
            indices.into_iter().map(u32::from).collect::<Vec<_>>()
        ); // U16逻辑索引保持源拓扑。
    }

    #[test]
    fn invalidated_capture_moves_its_chunks_to_background_retirement() {
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new); // 纯函数单测没有App插件，需建立正式后台计算池。
        let mut state = DynamicTerrainDerivationState::default(); // 新目标尚未派生。
        state.capture = Some(DynamicTerrainCaptureJob {
            identity: capture_identity(),  // 旧代次身份会被直接丢弃。
            transform: Affine3A::IDENTITY, // 变换不影响后台释放。
            transform_bits: affine_bits(Affine3A::IDENTITY), // 保持任务结构完整。
            phase: DynamicTerrainCapturePhase::Positions, // 部分捕获最容易被热更取消。
            offset: 1,                     // 首元素已经冻结。
            positions: vec![vec![[0.0, 0.0, 0.0]]], // 模拟已经分配的位置块。
            indices: Vec::new(),           // 索引阶段尚未开始。
            captured_vertices: 1,          // 进度与位置块一致。
            captured_indices: 0,           // 无索引块。
            expected_vertices: 3,          // 当前块尚未覆盖所有位置。
            expected_indices: 3,           // 当前块尚未覆盖所有索引。
            indexed: false,                // 此字段仅供原任务使用。
        });
        retire_capture_job(&mut state); // 失效帧只移动小块Vec头到计算池。
        assert!(state.capture.is_none()); // 旧输入立即不可再与新代次拼接。
        assert!(state.retire_task.is_some()); // 单条后台退休链承担真实析构。
    }

    #[test]
    fn chained_retirement_finishes_after_leaving_without_main_thread_polling() {
        use std::{sync::mpsc, thread, time::Duration}; // 测试只等待后台析构通知，不依赖任何游戏帧。

        struct DropProbe {
            ordinal: u8,                                  // 验证两个退休值按入队顺序释放。
            sender: mpsc::Sender<(u8, thread::ThreadId)>, // 把真实析构线程身份送回测试。
        }
        impl Drop for DropProbe {
            fn drop(&mut self) {
                let _ = self.sender.send((self.ordinal, thread::current().id())); // 只在后台真实析构时发送通知。
            }
        }

        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new); // 无App测试需要显式初始化计算池。
        let mut state = DynamicTerrainDerivationState::default(); // 模拟离图前的资源写侧。
        let (sender, receiver) = mpsc::channel(); // 主测试线程只读取异步析构结果。
        let main_thread = thread::current().id(); // 明确区分游戏主线程与后台worker。
        retire_large_payload(
            &mut state,
            DropProbe {
                ordinal: 1,
                sender: sender.clone(),
            },
        ); // 首份旧地形进入退休链。
        retire_large_payload(&mut state, DropProbe { ordinal: 2, sender }); // 次份值接在同一异步链尾。
        for ordinal in [1, 2] {
            let (actual, dropped_on) = receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("离图后退休链仍应自行排空"); // 单测等待真实后台任务，生产帧不会阻塞。
            assert_eq!(actual, ordinal); // 后一份不得抢先释放前一份。
            assert_ne!(dropped_on, main_thread); // 大内存释放不得落在主线程。
        }
    }

    #[test]
    fn completed_task_is_rejected_after_glb_generation_changes() {
        let map_id =
            feathrise_map_content::MapId::parse("MAP-terrain-task-identity").expect("测试MapId必须合法");
        let old_identity = DynamicTerrainIdentity {
            map_id,
            generation: 4,
            source_entity: Entity::PLACEHOLDER,
            source_mesh: Handle::<Mesh>::default().id(),
        };
        let mut current_identity = old_identity.clone();
        current_identity.generation = 5; // 模拟旧任务运行期间同一GLB再次保存。
        let mut derivation = DynamicTerrainDerivationState {
            desired: Some(current_identity.clone()),
            ..default()
        };

        assert!(!completed_task_is_current(&derivation, &old_identity));
        assert!(completed_task_is_current(&derivation, &current_identity));

        derivation.dirty_after_task = true; // 即使轻量身份尚未来得及换代，变更事件也立即拒收。
        assert!(!completed_task_is_current(&derivation, &current_identity));
    }
}
