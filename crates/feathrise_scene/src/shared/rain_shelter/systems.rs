//! 变化驱动的遮雨作者桥：当前最多32体原子发布，错误批次明确全部回退。

use super::{components::ExtrasRainShelter, helpers::parse_extras, resources::RainShelterRegistry}; // 来源标记、解析和小记录。
use super::{
    ownership::{OwnerQuery, insert_source, owner_active, remove_source, resolve_owner},
    types::ShelterSource,
}; // 有界祖先缓存和正式根活动门。
use crate::ScenePlacementInstance; // 只监听显式根新增，不枚举场景网格。
use bevy::gltf::GltfExtras; // glTF节点自定义属性，空节点也拥有该组件。
use bevy::prelude::*; // 实体、变更过滤和资源。
use feathrise_environment::rain_shelter::{
    RainShelterBox, RainShelterGpu, RainShelterState, RainShelterVolume,
}; // CPU/GPU共享受检代理。
use feathrise_scene_content::ActiveDynamicScene; // 动态场景实际身份。
use feathrise_scene_core::state::ScenePhase; // 正式玩法场景状态。
use std::collections::HashSet; // 层级事件合并，同源最多重建一次。

pub(super) fn sync_extras(
    // 只有小extras发生变化才产生工作。
    mut commands: Commands, // 成对维护作者组件与所有权标记。
    changed: Query<(Entity, &GltfExtras), Changed<GltfExtras>>, // 不查询Mesh或遍历所有稳定节点。
    owned: Query<(), With<ExtrasRainShelter>>, // 删除时不能影响玩法直接添加的盒。
    mut removed: RemovedComponents<GltfExtras>, // 卸载/热更移除作者属性也立即失效。
) {
    // 只处理真正变化的节点。
    for entity in removed.read() {
        // 按实际删除事件处理。
        if owned.contains(entity) {
            commands
                .entity(entity)
                .remove::<(RainShelterBox, ExtrasRainShelter)>();
        } // 移除属性即可取消遮雨。
    } // 完成移除事件。
    for (entity, extras) in &changed {
        // 解析次数与作者变更相关。
        if extras.value.len() > 65_536 {
            // 先检查长度，不扫描无界无关JSON字符串。
            if owned.contains(entity) {
                commands.entity(entity).insert(RainShelterBox {
                    center: Vec3::ZERO,
                    half_extents: Vec3::splat(f32::NAN),
                });
            } // 既有源超预算必须拒绝，不能继续显示旧代理。
            warn!(
                ?entity,
                "节点extras超过64KiB，跳过遮雨作者解析；请将rain_shelter放在独立小节点"
            ); // 未登记的大型无关extras不构成坏遮雨源。
            continue; // 不进入字符串搜索或JSON解析。
        } // 完成固定输入预算门。
        match parse_extras(&extras.value) {
            // 不依据名称、碰撞或粗包围盒推导。
            Ok(Some(source)) => {
                commands.entity(entity).insert((source, ExtrasRainShelter));
            } // 完整有效盒更新。
            Ok(None) => {
                if owned.contains(entity) {
                    commands
                        .entity(entity)
                        .remove::<(RainShelterBox, ExtrasRainShelter)>();
                }
            } // 属性删除恢复无代理。
            Err(()) => {
                commands.entity(entity).insert((
                    RainShelterBox {
                        center: Vec3::ZERO,
                        half_extents: Vec3::splat(f32::NAN),
                    },
                    ExtrasRainShelter,
                ));
            } // 保留明确无效来源，让发布整批拒绝而非漏掉单体。
        } // 完成单节点接管。
    } // 完成有界节点解析。
} // 完成extras桥。

#[allow(clippy::too_many_arguments)] // 生命周期事件和正式根查询各自独立，避免混淆所有权。
pub(super) fn publish(
    // 代理最多32时完整编码是固定3088字节小工作。
    scene: Res<State<ScenePhase>>, // 菜单或加载态必须立刻关闭旧场景代理。
    dynamic: Res<ActiveDynamicScene>, // 同DynamicSceneRoot下也检查真实SceneId。
    changed: Query<
        (Entity, &RainShelterBox, &GlobalTransform),
        Or<(Changed<RainShelterBox>, Changed<GlobalTransform>)>,
    >, // 父节点移动已由Transform传播捕获。
    sources: Query<(&RainShelterBox, &GlobalTransform)>, // 层级事件仅定向读取受影响源。
    owners: OwnerQuery,           // 每根稳定单查正式活动门。
    hierarchy: Query<
        Entity,
        Or<(
            Changed<ChildOf>,
            Added<ScenePlacementInstance>,
            Added<DespawnOnExit<ScenePhase>>,
        )>,
    >, // 祖先重挂/补上拥有者时定向失效。
    mut removed: RemovedComponents<RainShelterBox>, // 删除源时清除缓存。
    mut removed_transforms: RemovedComponents<GlobalTransform>, // 失去世界变换的源也不能继续遮雨。
    mut removed_parents: RemovedComponents<ChildOf>, // 祖先解除父级同样改变归属。
    mut registry: ResMut<RainShelterRegistry>, // 不存储Mesh或场景完整快照。
    mut state: ResMut<RainShelterState>, // 所有消费者在同帧观察完整批次。
) {
    // 不受虚拟时钟暂停影响，暂停编辑/移动仍正确。
    let context = match **scene {
        // 只在正式场景发布遮雨。
        ScenePhase::Active => dynamic.scene_id.clone().map(|id| (**scene, Some(id))), // 无ID时不发布。
        _ => None, // 菜单/加载安全关闭。
    }; // 完成当前场景身份。
    if registry.context != context {
        registry.context = context.clone();
    } // 稳定场景不重复写入身份缓存。
    for entity in removed.read().chain(removed_transforms.read()) {
        remove_source(&mut registry, entity);
    } // 实体定向O(1)清理与最多64级索引清理。
    let mut reparented = HashSet::new(); // 合并真实层级事件。
    for ancestor in hierarchy.iter().chain(removed_parents.read()) {
        // 不遍历稳定层级。
        if let Some(descendants) = registry.descendants.get(&ancestor) {
            reparented.extend(descendants.iter().copied());
        } // 只标记经过该祖先的已登记源。
    } // 完成事件到代理的反向定位。
    for (entity, source, transform) in &changed {
        // 单源固定矩阵构造不扫描顶点。
        if let Some(record) = registry.sources.get_mut(&entity) {
            record.volume = RainShelterVolume::from_box(*source, transform);
        }
        // 单纯移动只更新96字节体，不重走祖先。
        else {
            reparented.insert(entity);
        } // 首次源需要解析所有权。
    } // 完成源差分。
    for entity in reparented {
        // 每个真实受影响源最多解析一次64级路径。
        let previous = remove_source(&mut registry, entity); // 清理旧根和旧反向索引。
        let Ok((source, transform)) = sources.get(entity) else {
            continue;
        }; // 已卸载源不复活。
        let source_context = previous
            .as_ref()
            .map(|record| record.context.clone())
            .unwrap_or_else(|| context.clone()); // 重挂不改写创建时场景身份。
        let resolved = resolve_owner(entity, &owners); // 有界解析新链。
        let (owner, ancestors) = resolved.clone().unwrap_or((entity, vec![entity])); // 过深链保留明确坏源以关闭批次。
        insert_source(
            &mut registry,
            entity,
            ShelterSource {
                context: source_context,
                volume: if resolved.is_some() {
                    RainShelterVolume::from_box(*source, transform)
                } else {
                    None
                },
                owner,
                ancestors,
            },
        ); // 所有缓存一次提交。
    } // 完成真正层级变更接管。
    let mut next = RainShelterGpu::default(); // 错误、超限、非场景均采用显式无代理回退。
    let mut error = None; // 成功后清除去重错误。
    if context.is_some() {
        // 不把加载场景的数据提前暴露给渲染。
        if registry.owners.len() > 32 {
            // 最多32个独立活动/候选根，超限不逐根无限查询。
            error = Some("遮雨作者根超过32个，当前批次全部关闭；请将连续屋顶代理置于共同场景根下"); // 根检查预算独立且不静默遗漏。
        } else {
            // 已证实最多32个源，下面遍历具有固定上界。
            let current = context.as_ref().unwrap(); // 上层已确保正式场景身份存在。
            let mut active_roots = [None; 32]; // 固定栈数组避免每帧小堆分配。
            let mut root_count = 0; // 最多32次根单查。
            for owner in registry.owners.keys() {
                if owner_active(*owner, &owners, current) {
                    active_roots[root_count] = Some(*owner);
                    root_count += 1;
                }
            } // Pending/Retired根立即跳过，不逐候选源扫描。
            let mut inspected = 0; // 活动源检查也设置33次拒绝门。
            let mut adopted_sources = [None; 32]; // 固定栈数组保存首次活动来源。
            let mut adopted_count = 0; // 完整成功后最多绑定32个身份。
            'roots: for owner in active_roots.into_iter().flatten() {
                // 最多32个缓存根。
                for entity in &registry.owners[&owner] {
                    // 超32活动源立刻整批拒绝。
                    inspected += 1; // 计数先于读取源内容。
                    if inspected > 32 {
                        error = Some(
                            "遮雨活动代理超过32体，当前批次全部关闭；请合并不含庭院的连续屋顶代理",
                        );
                        break 'roots;
                    } // 没有无限作者循环。
                    let record = &registry.sources[entity]; // 索引始终与源记录同步。
                    if record.context.is_some() && record.context != context {
                        continue;
                    } // 旧图残留即使移动也不串图。
                    let Some(volume) = record.volume else {
                        error = Some(
                            "遮雨代理参数或变换无效，当前批次全部关闭；请修复rain_shelter中心、正半径或退化缩放",
                        );
                        break 'roots;
                    }; // 坏活动源拒绝整批。
                    let index = next.header.x as usize; // 最多32体。
                    next.shelters[index] = volume; // 完整96字节体复制。
                    next.header.y = if index == 0 {
                        volume.half_extents.w
                    } else {
                        next.header.y.max(volume.half_extents.w)
                    }; // 最高Y快速拒绝。
                    next.header.x += 1.0; // 一次发布完整数量。
                    if record.context.is_none() {
                        adopted_sources[adopted_count] = Some(*entity);
                        adopted_count += 1;
                    } // 不提前给隐藏候选或异图根认领身份。
                } // 完成一根当前代理。
            } // 完成固定大小整批组包。
            if error.is_none() {
                for entity in adopted_sources.into_iter().flatten() {
                    registry.sources.get_mut(&entity).unwrap().context = context.clone();
                }
            } // 整批成功才绑定首次激活身份。
        } // 完成预算门。
    } // 完成活动场景门。
    if error.is_some() {
        next = RainShelterGpu::default();
    } // 错误整批拒绝，禁止发布半成品。
    if registry.error != error {
        // 只有错误状态变化才记录日志。
        if let Some(message) = error {
            warn!("{message}");
        } // 避免稳定错误刷屏。
        registry.error = error; // 缓存当前诊断。
    } // 完成错误去重。
    if state.gpu != next {
        state.gpu = next;
    } // 暂停或相同结果不制造GPU写入。
} // 完成动态代理生命周期。
