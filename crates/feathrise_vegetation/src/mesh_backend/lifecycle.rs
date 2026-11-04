//! 通用Mesh原型与活动布局的确定性根实体生命周期。
//! 【功能设计】只处理显式稳定落点；同代提交未完成、输入被拒绝或旧根占用时保留回退，不制造第二个可见所有者。

use std::collections::{BTreeMap, BTreeSet}; // 计划、所有者和稳定ID都按数字身份确定性排序。

use bevy::prelude::{Commands, Entity, IVec2, Query, Res, ResMut, Transform}; // 单个PostUpdate系统执行差分并延迟装配层级。

use crate::{
    ActiveVegetationLayout, VegetationCandidateSource, VegetationLayerConfig, VegetationLayerId,
    VegetationLayoutCommit, VegetationLayoutState, VegetationPrototypeId, VegetationRealization,
    deterministic_candidate, resolved_candidate_enabled,
}; // 生命周期复用公共候选、活动布局和正交实现策略。

use super::components::{VegetationMeshInstance, VegetationMeshManagedRoot}; // 托管标记严格区分旧根元数据和自动所有者。
use super::resources::{
    ActiveVegetationMeshRoot, VegetationMeshAdoptionBlocker, VegetationMeshAdoptionPhase,
    VegetationMeshAdoptionStatus, VegetationMeshLayerAdoptionStatus, VegetationMeshOwnerTopology,
    VegetationMeshPrototypeRegistry, VegetationMeshRuntime,
}; // 系统只写私有活动表和一次完整公开状态快照。
use super::spawn::spawn_vegetation_mesh_instance; // 自动与手动路径共用完全相同的层级装配。

/// 当前同代布局中一个应由通用后端生成的稳定根。
#[derive(Clone, Copy, Debug, PartialEq)]
struct DesiredVegetationMeshRoot {
    prototype: VegetationPrototypeId, // 层配置声明的当前原型身份。
    prototype_revision: u64,          // 注册表同ID热替换时要求重建。
    transform: Transform,             // 显式稳定落点的完整最终变换。
} // 期望表不保存内容名称或项目状态。

type VegetationMeshRootKey = (VegetationLayerId, u64); // 层与稳定实例ID构成全生命周期唯一主键。

/// 一个同步帧内现有旧根和自动托管根的完整所有权快照。
#[derive(Default)]
struct ExistingVegetationMeshOwners {
    external: BTreeMap<VegetationMeshRootKey, usize>, // 非托管旧根按稳定身份计数。
    managed: BTreeMap<Entity, VegetationMeshRootKey>, // 托管根按实体反查稳定身份。
} // 一个参数传递完整快照，避免差分阶段混用不同扫描时点。

/// 在布局Update原子提交完成后同步通用Mesh根。
#[allow(clippy::too_many_arguments)] // 核心、注册表、运行表、状态和所有者查询分别承担独立一致性门。
pub(super) fn synchronize_mesh_lifecycle(
    mut commands: Commands, // 生成和回收根及其linked-spawn子层级。
    layout: Option<Res<ActiveVegetationLayout>>, // 独立安装Mesh插件时允许核心尚不存在。
    commit: Option<Res<VegetationLayoutCommit>>, // revision与配置generation必须来自同次原子提交。
    state: Option<Res<VegetationLayoutState>>, // 只读当前层、候选源和配置generation。
    registry: Res<VegetationMeshPrototypeRegistry>, // 消费侧显式登记后才允许接管。
    owner_topology: Res<VegetationMeshOwnerTopology>, // 根身份结构变化时才重扫单一所有者。
    mut runtime: ResMut<VegetationMeshRuntime>, // 私有表只记录本系统实际生成的根。
    mut status: ResMut<VegetationMeshAdoptionStatus>, // 每次差分后一次替换公开证据。
    roots: Query<(
        Entity,
        &VegetationMeshInstance,
        Option<&VegetationMeshManagedRoot>,
    )>, // 同层同ID旧根优先保留。
) {
    let registry_generation = registry.generation(); // 原型登记、替换与移除共享稳定变化身份。
    let owner_topology_generation = owner_topology.generation(); // 旧根出现/移除不依赖伪造布局或原型换版。
    let core_identity = match (&layout, &commit, &state) {
        (Some(layout), Some(_), Some(state)) => {
            Some((layout.revision, state.generation())) // 核心存在时使用真实活动与配置身份。
        }
        _ => None, // 独立安装Mesh插件时使用安全空身份。
    }; // 完成稳定快速路径所需核心身份提取。
    let (layout_revision, configuration_generation) = core_identity.unwrap_or((0, 0)); // 缺核心时零值只用于跳过重复空同步。
    if !runtime.needs_sync(
        layout_revision,           // 活动分配场变化触发候选差分。
        configuration_generation,  // 新配置先进入等待，不能直接套旧布局。
        registry_generation,       // 原型登记、替换或移除触发差分。
        owner_topology_generation, // 根结构变化触发单一所有权修复。
    ) {
        return; // 稳态不扫描显式清单或现有上千个根。
    } // 完成稳定三身份快速路径。
    let owners = collect_existing_owners(&roots); // 只在同步帧扫描现有通用身份根。
    reconcile_runtime_records(
        &mut commands, // 孤儿托管根需要连同Children一起回收。
        &registry,     // 原型移除即使核心暂不可用也必须释放托管根。
        &owners,       // 同一扫描快照同时核对旧根冲突与托管实体。
        &mut runtime,  // 清除不存在、失去原型或失去所有权的记录。
    ); // 完成进入布局一致性判断前的所有权修复。

    let (Some(layout), Some(commit), Some(state)) = (layout, commit, state) else {
        *status = VegetationMeshAdoptionStatus {
            phase: VegetationMeshAdoptionPhase::CoreUnavailable, // 缺布局核心时不生成任何新根。
            layout_revision: 0,                                  // 没有可核对的活动布局身份。
            configuration_generation: 0,                         // 没有可核对的配置代次。
            registry_generation,                                 // 原型移除仍可被诊断观察。
            managed_instances: runtime.active.len(),             // 已失去原型的根已经在上方回收。
            layers: Vec::new(),                                  // 没有核心配置就没有合法层快照。
        }; // 一次替换完整核心不可用状态。
        runtime.mark_synced(
            0,                         // 缺核心时锁定安全空布局身份。
            0,                         // 缺核心时锁定安全空配置身份。
            registry_generation,       // 原型变化仍会触发下一次释放同步。
            owner_topology_generation, // 根身份变化仍会触发下一次所有权同步。
        ); // 独立Mesh插件稳态不再逐帧扫描现有根。
        return; // 保留仍有原型的旧托管根，避免依赖暂缺造成可见空洞。
    }; // 完成可选核心资源解包。

    if commit.revision() != layout.revision
        || commit.configuration_generation() != configuration_generation
    {
        let layers = waiting_layer_statuses(
            state.layers(),   // 只报告全部非GPU实现层。
            &owners.external, // 旧根身份仍明确显示为回退。
            &runtime,         // 上一同代托管根继续保留。
        ); // 多帧脏块处理中不读取新候选对旧布局做差分。
        *status = VegetationMeshAdoptionStatus {
            phase: if layers.is_empty() {
                VegetationMeshAdoptionPhase::Unbound // 空配置没有需要等待的Mesh层。
            } else {
                VegetationMeshAdoptionPhase::WaitingForLayout // 至少一层等待同代原子提交。
            },
            layout_revision: layout.revision, // 状态明确显示仍在使用的上一活动版本。
            configuration_generation,         // 状态明确显示尚未被活动提交采用的新代次。
            registry_generation,              // 同帧注册表身份仍完整可见。
            managed_instances: runtime.active.len(), // 等待期间保留上一有效托管根。
            layers,                           // 每层独立报告候选、托管和旧根数量。
        }; // 一次替换等待状态。
        runtime.mark_synced(
            layout.revision,           // 下次活动revision变化会重新同步。
            configuration_generation,  // 同一等待代次不逐帧重复扫描。
            registry_generation,       // 原型变化仍会重新进入本系统。
            owner_topology_generation, // 旧根变化仍会重新核对单一所有者。
        ); // 锁定本次等待快照。
        return; // 同代布局提交前禁止改写可见集合。
    } // 完成布局与配置generation原子一致门。

    let mut desired = BTreeMap::<VegetationMeshRootKey, DesiredVegetationMeshRoot>::new(); // 当前完整采用目标。
    let mut preserved_layers = BTreeSet::<VegetationLayerId>::new(); // 拒绝的新配置继续保留上一托管集合。
    let mut layer_statuses = Vec::<VegetationMeshLayerAdoptionStatus>::new(); // 最终按层ID稳定排序发布。
    for layer in state
        .layers()
        .iter()
        .filter(|layer| mesh_realization(layer.realization))
    {
        let fallback_instances = external_owner_count(layer.id, &owners.external); // 任意旧根都阻止本层宣称完整接管。
        let candidate_instances = explicit_candidate_count(&layer.source); // 非显式源在本阶段显示零并结构化拒绝。
        let Some((prototype_revision, _)) = registry.entry(layer.prototype) else {
            layer_statuses.push(VegetationMeshLayerAdoptionStatus {
                layer: layer.id,            // 保留缺失原型所属稳定层。
                prototype: layer.prototype, // 指出尚未登记或已移除的原型ID。
                phase: if fallback_instances == 0 {
                    VegetationMeshAdoptionPhase::Unbound // 没有注册也没有旧根时完全未绑定。
                } else {
                    VegetationMeshAdoptionPhase::FallbackRetained // 旧实现继续承担可见所有权。
                },
                blocker: Some(VegetationMeshAdoptionBlocker::MissingPrototype), // 原型移除同时触发托管根回收。
                candidate_instances,   // 显式清单数量仍可供诊断。
                selected_instances: 0, // 缺原型时不构建采用目标。
                managed_instances: 0,  // 后续统一从私有活动表回填。
                fallback_instances,    // 旧根数量不会被通用后端删除。
            }); // 完成缺原型层状态。
            continue; // 缺原型层不进入期望根表。
        }; // 完成当前注册原型查询。
        let VegetationCandidateSource::ExplicitPlacements(placements) = &layer.source else {
            preserved_layers.insert(layer.id); // 不支持的新源不能破坏上一有效托管集合。
            layer_statuses.push(rejected_layer_status(
                layer,                                                     // 报告当前配置层与原型。
                VegetationMeshAdoptionBlocker::UnsupportedCandidateSource, // 程序网格边界尚未定义。
                candidate_instances, // 非显式源没有有限候选总数。
                fallback_instances,  // 旧根继续保持单一可见所有者。
            )); // 完成非显式源拒绝状态。
            continue; // 本小阶段不隐式枚举无限网格。
        }; // 完成显式候选源限制。
        if placements.len() > i32::MAX as usize {
            preserved_layers.insert(layer.id); // 无法无损映射公共候选索引时保留上一集合。
            layer_statuses.push(rejected_layer_status(
                layer,                                                         // 报告溢出所属层。
                VegetationMeshAdoptionBlocker::ExplicitPlacementIndexOverflow, // 禁止usize截断为i32。
                placements.len(),   // 保留真实过大清单数量。
                fallback_instances, // 旧根继续可见。
            )); // 完成索引溢出拒绝状态。
            continue; // 不生成具有错误稳定候选映射的根。
        } // 完成显式索引范围保护。
        let mut unique_ids = BTreeSet::<u64>::new(); // 同层稳定ID是确定性差分主键。
        if placements
            .iter()
            .any(|placement| !unique_ids.insert(placement.instance_id))
        {
            preserved_layers.insert(layer.id); // 新清单非法时保留上一完整集合。
            layer_statuses.push(rejected_layer_status(
                layer,                                              // 报告重复ID所属层。
                VegetationMeshAdoptionBlocker::DuplicateInstanceId, // 禁止后项覆盖前项。
                placements.len(),                                   // F3仍显示实际清单长度。
                fallback_instances,                                 // 旧根不受非法新表影响。
            )); // 完成重复稳定ID拒绝状态。
            continue; // 整层原子拒绝，不生成部分根。
        } // 完成显式清单唯一身份验证。
        let mut selected_instances = 0_usize; // 只统计当前活动分配场真实选择的候选。
        for index in 0..placements.len() {
            let world_cell = IVec2::new(index as i32, 0); // 显式源把非负X索引映射为稳定表项。
            let candidate = deterministic_candidate(&layer.source, world_cell)
                .expect("受检显式落点索引必须生成候选"); // 前置范围和清单长度保证存在。
            if !resolved_candidate_enabled(&layout, layer.id, &candidate) {
                continue; // 当前分配场未选择的落点不创建自动根。
            } // 完成活动候选门。
            selected_instances += 1; // 候选被当前同代布局稳定选中。
            let key = (layer.id, candidate.instance_id); // 层与稳定ID共同构成唯一所有者身份。
            if owners.external.contains_key(&key) {
                continue; // 旧树木/景观根已占用时绝不生成第二个可见根。
            } // 完成旧所有者优先门。
            desired.insert(
                key, // 同层重复ID已在整层验证中拒绝。
                DesiredVegetationMeshRoot {
                    prototype: layer.prototype,     // 保存当前层原型身份。
                    prototype_revision,             // 保存当前注册原型版本。
                    transform: candidate.transform, // 保存公共候选最终变换。
                },
            ); // 登记本次完整采用目标。
        } // 完成当前显式层全部候选枚举。
        layer_statuses.push(VegetationMeshLayerAdoptionStatus {
            layer: layer.id,            // 当前层稳定身份。
            prototype: layer.prototype, // 当前登记原型身份。
            phase: if fallback_instances == 0 {
                VegetationMeshAdoptionPhase::CurrentLayoutAdopted // 没有旧根阻挡时可完整采用。
            } else {
                VegetationMeshAdoptionPhase::FallbackRetained // 任意旧根存在都保持最保守阶段。
            },
            blocker: (fallback_instances > 0)
                .then_some(VegetationMeshAdoptionBlocker::ExistingOwner), // 结构化说明没有重复生成。
            candidate_instances: placements.len(), // 显式清单有限总数。
            selected_instances,                    // 当前布局实际选择数。
            managed_instances: 0,                  // 差分完成后统一回填。
            fallback_instances,                    // 当前层全部非托管旧根身份数。
        }); // 完成有效显式层初始状态。
    } // 完成全部可管理层计划。

    apply_desired_roots(
        &mut commands,     // 实体替换与回收延迟到PostUpdate末尾。
        layout.revision,   // 托管标记记录本次同代活动布局。
        &registry,         // 生成时取得完整当前原型。
        &desired,          // 只包含没有旧根冲突的稳定身份。
        &preserved_layers, // 被拒绝层保留上一托管集合。
        &owners,           // 同一快照核对旧根冲突与托管实体。
        &mut runtime,      // 一次更新完整活动所有权表。
    ); // 完成当前同代布局差分。
    for layer_status in &mut layer_statuses {
        layer_status.managed_instances = runtime
            .active
            .keys()
            .filter(|(layer, _)| *layer == layer_status.layer)
            .count(); // F3直接读取每层托管根分子。
    } // 完成全部层托管数量回填。
    layer_statuses.sort_by_key(|layer_status| layer_status.layer); // 配置输入顺序不影响公开状态字节顺序。
    *status = VegetationMeshAdoptionStatus {
        phase: aggregate_phase(&layer_statuses), // 取全部层最保守迁移阶段。
        layout_revision: layout.revision,        // 锁定已完成差分的活动布局。
        configuration_generation,                // 锁定该布局实际消费的配置与源代次。
        registry_generation,                     // 锁定生成根使用的注册表代次。
        managed_instances: runtime.active.len(), // 全部自动托管根总数。
        layers: layer_statuses,                  // 发布稳定升序完整层状态。
    }; // 一次替换公开采用证据。
    runtime.mark_synced(
        layout.revision,           // 下次布局换版才重新枚举。
        configuration_generation,  // 下次配置或源变化先进入等待。
        registry_generation,       // 下次原型变化重新差分。
        owner_topology_generation, // 下次根身份变化重新核对所有权。
    ); // 完成本次生命周期身份锁定。
} // 完成通用Mesh生命周期同步系统。

/// 收集非托管旧根与仍存在的自动托管根。
fn collect_existing_owners(
    roots: &Query<(
        Entity,
        &VegetationMeshInstance,
        Option<&VegetationMeshManagedRoot>,
    )>, // 只扫描带通用根身份的实体。
) -> ExistingVegetationMeshOwners {
    let mut owners = ExistingVegetationMeshOwners::default(); // 同一遍扫描构建完整所有权快照。
    for (entity, instance, marker) in roots.iter() {
        let key = (instance.layer, instance.instance_id); // 原型变化不改变同层稳定所有权主键。
        if marker.is_some() {
            owners.managed.insert(entity, key); // 私有表稍后验证实体与主键一一对应。
        } else {
            *owners.external.entry(key).or_default() += 1; // 同身份已有旧根时通用后端不再生成。
        } // 完成当前根所有者分类。
    } // 完成所有通用根身份扫描。
    owners // 返回本次同步帧稳定所有者快照。
} // 完成现有所有者收集。

/// 修复孤儿托管实体、已移除原型和旧根抢占。
fn reconcile_runtime_records(
    commands: &mut Commands, // 回收托管根及其linked-spawn子层级。
    registry: &VegetationMeshPrototypeRegistry, // 原型移除是显式释放信号。
    owners: &ExistingVegetationMeshOwners, // 同一时点的旧根与托管根完整快照。
    runtime: &mut VegetationMeshRuntime, // 私有活动表需要与世界实体一致。
) {
    for (entity, key) in &owners.managed {
        let tracked = runtime
            .active
            .get(key)
            .is_some_and(|active| active.entity == *entity); // 实体和稳定主键都必须匹配私有记录。
        if !tracked {
            commands.entity(*entity).despawn(); // Bevy Children使用linked_spawn，根回收会递归清理LOD节点。
        } // 完成孤儿托管根回收。
    } // 完成世界托管标记扫描。
    let active_keys = runtime.active.keys().copied().collect::<Vec<_>>(); // 避免遍历时修改有序活动表。
    for key in active_keys {
        let Some(active) = runtime.active.get(&key) else {
            continue; // 前一轮可能已经移除当前稳定身份。
        }; // 完成活动记录读取。
        let entity_matches = owners.managed.get(&active.entity) == Some(&key); // 根必须仍存在且保留原身份。
        let prototype_exists = registry.contains(active.prototype); // 显式注销原型要求释放全部托管根。
        let external_conflict = owners.external.contains_key(&key); // 旧根出现时自动根主动让出。
        if entity_matches && prototype_exists && !external_conflict {
            continue; // 当前记录仍拥有合法且唯一的稳定身份。
        } // 完成活动记录保留判断。
        if entity_matches {
            commands.entity(active.entity).despawn(); // 只回收本插件带托管标记的根及其LOD子节点。
        } // 已被外部删除的实体不重复排队despawn。
        runtime.active.remove(&key); // 私有表立即释放该稳定身份。
    } // 完成全部活动记录修复。
} // 完成运行表所有权修复。

/// 应用完整期望表，并原子保留被拒绝层上一集合。
fn apply_desired_roots(
    commands: &mut Commands,                    // 生成、替换和回收自动托管根。
    layout_revision: u64,                       // 当前同代活动布局身份写入托管标记。
    registry: &VegetationMeshPrototypeRegistry, // 生成时读取完整当前原型。
    desired: &BTreeMap<VegetationMeshRootKey, DesiredVegetationMeshRoot>, // 当前合法且无旧根冲突的目标。
    preserved_layers: &BTreeSet<VegetationLayerId>, // 非显式或重复ID层保持上一有效根。
    owners: &ExistingVegetationMeshOwners,          // 同一时点的旧根与托管根完整快照。
    runtime: &mut VegetationMeshRuntime,            // 差分后保存完整自动所有权。
) {
    let active_keys = runtime.active.keys().copied().collect::<Vec<_>>(); // 有序快照允许安全修改活动表。
    for key in active_keys {
        let Some(active) = runtime.active.get(&key).cloned() else {
            continue; // 前置修复可能已经移除当前记录。
        }; // 完成当前活动根复制。
        if owners.external.contains_key(&key) {
            commands.entity(active.entity).despawn(); // 旧根出现时自动根及LOD立即让出所有权。
            runtime.active.remove(&key); // 同步释放私有稳定身份。
            continue; // 不允许同一稳定身份进入后续保留分支。
        } // 完成旧根冲突处理。
        if preserved_layers.contains(&key.0) {
            continue; // 被拒绝的新层配置不破坏上一有效托管根。
        } // 完成拒绝回退保留。
        let keep = desired.get(&key).is_some_and(|target| {
            active.prototype == target.prototype
                && active.prototype_revision == target.prototype_revision
                && active.transform == target.transform
                && owners.managed.get(&active.entity) == Some(&key) // 根仍存在且身份未被外部修改。
        }); // 原型、版本、变换和实体身份全部一致才复用根。
        if keep {
            let target = desired.get(&key).expect("保留根必须拥有期望记录"); // 前置判断已经证明存在。
            commands
                .entity(active.entity)
                .insert(VegetationMeshManagedRoot {
                    layout_revision,                               // 同一根已经核对当前活动布局。
                    prototype_revision: target.prototype_revision, // 同一根仍使用当前注册原型。
                }); // 更新只读托管证据，不重建共享层级。
            continue; // 完成当前稳定根复用。
        } // 完成活动根等价判断。
        commands.entity(active.entity).despawn(); // 候选移除、变换或原型变化时回收整棵层级。
        runtime.active.remove(&key); // 释放旧记录，后续可按同键生成新根。
    } // 完成全部旧活动根差分。
    for (key, target) in desired {
        if runtime.active.contains_key(key) {
            continue; // 完全等价根已在上方保留并更新托管revision。
        } // 完成已存在根快速路径。
        let Some((prototype_revision, prototype)) = registry.entry(target.prototype) else {
            continue; // 同一系统执行期间注册表不会改变，仍保留防御式释放门。
        }; // 完成当前完整原型读取。
        if prototype_revision != target.prototype_revision {
            continue; // 期望计划与注册表版本不一致时等待下一变化代次。
        } // 完成原型版本一致性门。
        let entity =
            spawn_vegetation_mesh_instance(commands, key.0, prototype, key.1, target.transform); // 共享装配生成根和全部LOD/部件子节点。
        commands.entity(entity).insert(VegetationMeshManagedRoot {
            layout_revision,    // 标记本根采用的活动布局revision。
            prototype_revision, // 标记本根采用的注册原型revision。
        }); // 只有生命周期生成的根携带自动托管所有权。
        runtime.active.insert(
            *key, // 层与稳定ID构成唯一活动主键。
            ActiveVegetationMeshRoot {
                entity,                      // 保存根实体供后续确定性回收。
                prototype: target.prototype, // 保存当前原型身份。
                prototype_revision,          // 保存当前原型热替换版本。
                transform: target.transform, // 保存当前显式落点变换。
            },
        ); // 完成自动根私有所有权登记。
    } // 完成全部缺失期望根生成。
} // 完成期望根差分应用。

/// 构造同代活动提交尚未完成时的逐层等待状态。
fn waiting_layer_statuses(
    layers: &[VegetationLayerConfig], // 当前新配置层表。
    external_owners: &BTreeMap<VegetationMeshRootKey, usize>, // 现有旧根身份。
    runtime: &VegetationMeshRuntime,  // 上一有效托管根继续可见。
) -> Vec<VegetationMeshLayerAdoptionStatus> {
    let mut statuses = layers
        .iter()
        .filter(|layer| mesh_realization(layer.realization))
        .map(|layer| VegetationMeshLayerAdoptionStatus {
            layer: layer.id,                                      // 当前等待层稳定身份。
            prototype: layer.prototype,                           // 当前等待配置声明的原型。
            phase: VegetationMeshAdoptionPhase::WaitingForLayout, // 禁止新配置套用旧布局。
            blocker: Some(VegetationMeshAdoptionBlocker::LayoutGenerationPending), // 明确等待同代提交。
            candidate_instances: explicit_candidate_count(&layer.source), // 显式层仍显示有限清单数。
            selected_instances: 0, // 同代活动布局完成前不计算新选择结果。
            managed_instances: runtime
                .active
                .keys()
                .filter(|(active_layer, _)| *active_layer == layer.id)
                .count(), // 上一有效托管集合继续承担画面。
            fallback_instances: external_owner_count(layer.id, external_owners), // 旧根继续拥有自身稳定身份。
        })
        .collect::<Vec<_>>(); // 完成等待层快照构建。
    statuses.sort_by_key(|status| status.layer); // 配置顺序不影响公开状态。
    statuses // 返回稳定升序等待状态。
} // 完成等待状态构建。

/// 构造一个整层原子拒绝并保留上一集合的状态。
fn rejected_layer_status(
    layer: &VegetationLayerConfig,          // 当前被拒绝层配置。
    blocker: VegetationMeshAdoptionBlocker, // 结构化拒绝原因。
    candidate_instances: usize,             // 可表达时保留候选总数。
    fallback_instances: usize,              // 当前非托管旧根身份数。
) -> VegetationMeshLayerAdoptionStatus {
    VegetationMeshLayerAdoptionStatus {
        layer: layer.id,                                             // 被拒绝层稳定身份。
        prototype: layer.prototype,                                  // 被拒绝配置声明的原型。
        phase: VegetationMeshAdoptionPhase::RejectedKeepingFallback, // 上一集合继续可见。
        blocker: Some(blocker),                                      // F3和测试读取确定性原因。
        candidate_instances,                                         // 保留诊断总量。
        selected_instances: 0,                                       // 拒绝层不发布部分期望结果。
        managed_instances: 0, // 差分后由调用方回填上一托管数量。
        fallback_instances,   // 旧根保持不变。
    } // 返回完整拒绝状态。
} // 完成拒绝状态构造。

/// 判断一层是否由通用Mesh后端表达远场/全量视觉根。
fn mesh_realization(realization: VegetationRealization) -> bool {
    matches!(
        realization,
        VegetationRealization::Entities | VegetationRealization::HybridNearField { .. }
    ) // GpuOnly层继续完全交给叶片或其他GPU后端。
} // 完成Mesh实现策略判断。

/// 返回显式清单有限候选数；程序网格在本阶段没有有限枚举边界。
fn explicit_candidate_count(source: &VegetationCandidateSource) -> usize {
    match source {
        VegetationCandidateSource::ExplicitPlacements(placements) => placements.len(), // HN树木与景观使用稳定清单。
        VegetationCandidateSource::DenseGrid { .. }
        | VegetationCandidateSource::JitteredGrid { .. } => 0, // 无限网格等待后续显式空间驻留边界。
    } // 完成候选源分派。
} // 完成有限候选数查询。

/// 返回一层现有非托管旧根的稳定身份数。
fn external_owner_count(
    layer: VegetationLayerId, // 需要统计的稳定层身份。
    external_owners: &BTreeMap<VegetationMeshRootKey, usize>, // 旧根按联合主键保存。
) -> usize {
    external_owners
        .keys()
        .filter(|(owner_layer, _)| *owner_layer == layer)
        .count() // 同一稳定身份即使已有重复旧根也只占一个迁移分母。
} // 完成旧所有者身份统计。

/// 聚合全部层最保守的通用Mesh采用阶段。
fn aggregate_phase(
    layers: &[VegetationMeshLayerAdoptionStatus], // 已按层ID稳定排序的完整状态。
) -> VegetationMeshAdoptionPhase {
    if layers.is_empty() {
        return VegetationMeshAdoptionPhase::Unbound; // 没有可管理层不能宣称采用。
    } // 完成空层快速路径。
    for phase in [
        VegetationMeshAdoptionPhase::RejectedKeepingFallback, // 拒绝优先暴露。
        VegetationMeshAdoptionPhase::FallbackRetained,        // 其次暴露旧根回退。
        VegetationMeshAdoptionPhase::WaitingForLayout,        // 再暴露同代布局等待。
        VegetationMeshAdoptionPhase::Unbound,                 // 缺原型阻止全局采用。
        VegetationMeshAdoptionPhase::CurrentLayoutAdopted,    // 只有全部层采用时最终返回。
    ] {
        if layers.iter().any(|status| status.phase == phase) {
            return phase; // 返回第一个最保守已出现阶段。
        } // 完成当前优先级检查。
    } // 完成全部已知阶段聚合。
    VegetationMeshAdoptionPhase::CoreUnavailable // 防御未来新增枚举但未更新聚合顺序。
} // 完成全局采用阶段聚合。
