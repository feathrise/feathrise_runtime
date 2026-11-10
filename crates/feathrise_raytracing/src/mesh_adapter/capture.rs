//! 每帧最多 2 MiB 的源数据捕获、两个后台任务和每帧一个派生 Mesh 采用。

use bevy::{
    mesh::{Indices, PrimitiveTopology, VertexAttributeValues}, // 精确验证并分片复制源Mesh格式。
    prelude::*, // Assets、Mesh3d、Commands与资源系统参数。
    tasks::{AsyncComputeTaskPool, futures::check_ready}, // 后台任务启动与主线程非阻塞轮询。
};

use crate::{
    constants::{
        MAX_ASYNC_TASKS, MAX_CAPTURE_BYTES_PER_FRAME, MAX_CPU_WORKING_SET_BYTES,
        MAX_UPLOAD_BYTES_PER_FRAME,
    },
    resources::{MeshRevisionRegistry, RaytracingRuntime, RollbackDisposition},
    types::{AdaptationIdentity, RaytracingPrototypeBlocker, RaytracingPrototypeStage},
};

use super::{
    CapturePhase, CapturedMeshInput, CompletedAdaptation, MeshCaptureJob, PendingAdaptationTask,
    adapt_captured_mesh,
    identity::{adaptation_identity_matches, record_task_failure_if_current},
};

impl MeshCaptureJob {
    /// 从发现阶段冻结的源统计建立空捕获缓冲，并先验证不会跨格式或拓扑继续运行。
    fn new(
        identity: AdaptationIdentity,
        mesh: &Mesh,
        expected_vertices: usize,
        expected_indices: usize,
        requires_source_uv: bool,
    ) -> Result<Self, String> {
        if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
            return Err(format!(
                "拓扑为{:?}，要求TriangleList",
                mesh.primitive_topology()
            ));
        }
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .ok_or_else(|| "缺少Float32x3 Position".to_string())?;
        let normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .and_then(VertexAttributeValues::as_float3)
            .ok_or_else(|| "缺少Float32x3 Normal".to_string())?;
        if positions.len() != expected_vertices || normals.len() != expected_vertices {
            return Err(format!(
                "发现后顶点属性数量变化：position={} normal={} expected={expected_vertices}",
                positions.len(),
                normals.len()
            ));
        }
        let source_uvs = mesh
            .attribute(Mesh::ATTRIBUTE_UV_0)
            .map(|values| {
                let VertexAttributeValues::Float32x2(values) = values else {
                    return Err("UV0存在但不是Float32x2".to_string());
                };
                Ok(values.as_slice())
            })
            .transpose()?;
        if requires_source_uv && source_uvs.is_none() {
            return Err("固定地形primitive缺少原UV0".to_string());
        }
        if source_uvs.is_some_and(|uvs| uvs.len() != expected_vertices) {
            return Err("UV0数量与顶点数量不一致".to_string());
        }
        let Some(indices) = mesh.indices() else {
            return Err("缺少显式索引".to_string());
        };
        if indices.len() != expected_indices {
            return Err(format!(
                "发现后索引数量变化：actual={} expected={expected_indices}",
                indices.len()
            ));
        }
        Ok(Self {
            identity,
            phase: CapturePhase::Positions,
            offset: 0,
            positions: Vec::new(), // 不在启动帧一次预留整项容量，后续扩容随2MiB复制预算渐进发生。
            normals: Vec::new(),   // 法线同样只随本帧实际复制数据增长。
            uvs: source_uvs.map(|_| Vec::new()), // 无UV内容不分配缓冲，有UV也不提前分配整项。
            indices: Vec::new(),   // U32索引只随每帧实际捕获区间增长，避免首帧大型分配峰值。
            expected_vertices,
            expected_indices,
            requires_source_uv,
        })
    }

    /// 在本帧剩余预算内复制尽可能多的连续元素；从不阻塞或等待资产。
    fn capture(&mut self, mesh: &Mesh, mut remaining_bytes: usize) -> Result<usize, String> {
        let initial_budget = remaining_bytes; // 返回真实复制字节供未来诊断核对硬门。
        while remaining_bytes > 0 && self.phase != CapturePhase::Complete {
            match self.phase {
                CapturePhase::Positions => {
                    let values = mesh
                        .attribute(Mesh::ATTRIBUTE_POSITION)
                        .and_then(VertexAttributeValues::as_float3)
                        .ok_or_else(|| "捕获期间Position格式失效".to_string())?;
                    let copied = copy_slice_budgeted(
                        values,
                        &mut self.positions,
                        &mut self.offset,
                        &mut remaining_bytes,
                        12,
                    );
                    if self.offset == values.len() {
                        self.phase = CapturePhase::Normals;
                        self.offset = 0;
                    } else if copied == 0 {
                        break; // 剩余字节不足一个元素时立即让出本帧。
                    }
                }
                CapturePhase::Normals => {
                    let values = mesh
                        .attribute(Mesh::ATTRIBUTE_NORMAL)
                        .and_then(VertexAttributeValues::as_float3)
                        .ok_or_else(|| "捕获期间Normal格式失效".to_string())?;
                    let copied = copy_slice_budgeted(
                        values,
                        &mut self.normals,
                        &mut self.offset,
                        &mut remaining_bytes,
                        12,
                    );
                    if self.offset == values.len() {
                        self.phase = CapturePhase::Uvs;
                        self.offset = 0;
                    } else if copied == 0 {
                        break;
                    }
                }
                CapturePhase::Uvs => {
                    let Some(target) = self.uvs.as_mut() else {
                        self.phase = CapturePhase::Indices; // 无UV路径不消耗主线程复制预算。
                        self.offset = 0;
                        continue;
                    };
                    let values = mesh
                        .attribute(Mesh::ATTRIBUTE_UV_0)
                        .and_then(|values| match values {
                            VertexAttributeValues::Float32x2(values) => Some(values.as_slice()),
                            _ => None,
                        })
                        .ok_or_else(|| "捕获期间UV0格式失效".to_string())?;
                    let copied = copy_slice_budgeted(
                        values,
                        target,
                        &mut self.offset,
                        &mut remaining_bytes,
                        8,
                    );
                    if self.offset == values.len() {
                        self.phase = CapturePhase::Indices;
                        self.offset = 0;
                    } else if copied == 0 {
                        break;
                    }
                }
                CapturePhase::Indices => {
                    let indices = mesh
                        .indices()
                        .ok_or_else(|| "捕获期间索引已移除".to_string())?;
                    let remaining_elements = indices.len().saturating_sub(self.offset);
                    let count = remaining_elements.min(remaining_bytes / 4); // 即使源为U16也按目标U32写入成本计预算。
                    if count == 0 {
                        break;
                    }
                    match indices {
                        Indices::U16(values) => self.indices.extend(
                            values[self.offset..self.offset + count]
                                .iter()
                                .map(|index| *index as u32),
                        ),
                        Indices::U32(values) => self
                            .indices
                            .extend_from_slice(&values[self.offset..self.offset + count]),
                    }
                    self.offset += count;
                    remaining_bytes -= count * 4;
                    if self.offset == indices.len() {
                        self.phase = CapturePhase::Complete;
                        self.offset = 0;
                    }
                }
                CapturePhase::Complete => break,
            }
        }
        Ok(initial_budget - remaining_bytes) // 本值永远不超过调用方传入2MiB。
    }

    fn is_complete(&self) -> bool {
        self.phase == CapturePhase::Complete // 完成前不得把部分输入交给后台任务。
    }

    fn into_input(self) -> Result<CapturedMeshInput, String> {
        if !self.is_complete()
            || self.positions.len() != self.expected_vertices
            || self.normals.len() != self.expected_vertices
            || self.indices.len() != self.expected_indices
            || self
                .uvs
                .as_ref()
                .is_some_and(|uvs| uvs.len() != self.expected_vertices)
            || (self.requires_source_uv && self.uvs.is_none())
        {
            return Err("分帧捕获未形成完整原子输入".to_string());
        }
        Ok(CapturedMeshInput {
            identity: self.identity,
            positions: self.positions,
            normals: self.normals,
            uvs: self.uvs,
            indices: self.indices,
        })
    }

    /// 返回当前捕获缓冲的真实容量载荷，把分片扩容后的未使用容量也计入工作集硬门。
    pub(crate) fn estimated_allocated_bytes(&self) -> u64 {
        self.positions.capacity() as u64 * 12
            + self.normals.capacity() as u64 * 12
            + self.uvs.as_ref().map_or(0, |uvs| uvs.capacity() as u64 * 8)
            + self.indices.capacity() as u64 * 4
    }
}

/// 在固定字节预算内复制一个 Copy 属性切片的连续区间。
fn copy_slice_budgeted<T: Copy>(
    source: &[T],
    target: &mut Vec<T>,
    offset: &mut usize,
    remaining_bytes: &mut usize,
    element_bytes: usize,
) -> usize {
    let remaining_elements = source.len().saturating_sub(*offset); // 属性热更缩短时由调用方完整性检查阻断。
    let count = remaining_elements.min(*remaining_bytes / element_bytes); // 不拆分单个顶点属性元素。
    if count == 0 {
        return 0;
    }
    target.extend_from_slice(&source[*offset..*offset + count]); // 只复制本帧允许的连续小片。
    *offset += count;
    *remaining_bytes -= count * element_bytes;
    count
}

/// 驱动当前唯一分帧捕获并在槽位允许时启动一个后台转换任务。
pub(crate) fn capture_mesh_inputs(
    meshes: Res<Assets<Mesh>>,     // 只读访问当前源Mesh，不修改或克隆完整资产。
    source_meshes: Query<&Mesh3d>, // 每帧核对源实体仍引用同一Mesh句柄。
    revisions: Res<MeshRevisionRegistry>, // 捕获期间热更会使任务身份立即失效。
    mut runtime: ResMut<RaytracingRuntime>, // 保存捕获游标、最多两个任务和失败事务。
) {
    if runtime.stage != RaytracingPrototypeStage::AdaptingMeshes
        || runtime.pending_rollback.is_some()
    {
        return; // 非适配阶段只由轮询系统回收过期任务。
    }
    if runtime.tasks.len() >= MAX_ASYNC_TASKS {
        return; // 两个槽都被当前或过期任务占用时立即让出主线程。
    }
    if runtime.capture_job.is_none() && runtime.capture_cursor < runtime.candidates.len() {
        let candidate = runtime.candidates[runtime.capture_cursor].clone(); // 先结束对runtime槽位的借用，再取得新任务票据。
        let Ok(current_mesh) = source_meshes.get(candidate.entity) else {
            fail_mesh_adaptation(&mut runtime, "源实体或Mesh3d已消失".to_string());
            return;
        };
        if current_mesh.0.id() != candidate.mesh_id
            || revisions.current(candidate.mesh_id) != candidate.source_revision
        {
            runtime.request_rollback(
                Some(RaytracingPrototypeBlocker::SourceChanged),
                RollbackDisposition::RasterReady,
            );
            return;
        }
        let Some(mesh) = meshes.get(candidate.mesh_id) else {
            fail_mesh_adaptation(&mut runtime, "源Mesh资产尚未就绪".to_string());
            return;
        };
        let root = runtime.root.expect("适配阶段必须已锁定受支持场景目标");
        let ticket = runtime.next_task_ticket();
        let identity = AdaptationIdentity {
            generation: runtime.generation,
            root,
            source_entity: candidate.entity,
            source_mesh: candidate.mesh_id,
            source_revision: candidate.source_revision,
            ticket,
            contract_index: candidate.contract_index,
        };
        match MeshCaptureJob::new(
            identity,
            mesh,
            candidate.vertex_count as usize,
            (candidate.triangle_count * 3) as usize,
            candidate.requires_source_uv,
        ) {
            Ok(job) => {
                runtime.candidates[candidate.contract_index].adaptation_ticket = Some(ticket); // 当前槽只接受本次捕获最终启动的唯一票据。
                runtime.capture_job = Some(job); // 本帧随后立即使用剩余2MiB预算开始复制。
            }
            Err(error) => {
                fail_mesh_adaptation(&mut runtime, error);
                return;
            }
        }
    }

    let Some(job_identity) = runtime.capture_job.as_ref().map(|job| job.identity) else {
        return; // 所有候选已启动且任务尚未完成时保持非阻塞。
    };
    let Some(mesh) = meshes.get(job_identity.source_mesh) else {
        fail_mesh_adaptation(&mut runtime, "捕获期间源Mesh资产被移除".to_string());
        return;
    };
    let capture_result = runtime
        .capture_job
        .as_mut()
        .expect("刚读取的捕获任务必须仍存在")
        .capture(mesh, MAX_CAPTURE_BYTES_PER_FRAME);
    if let Err(error) = capture_result {
        fail_mesh_adaptation(&mut runtime, error);
        return;
    }
    if !runtime
        .capture_job
        .as_ref()
        .is_some_and(MeshCaptureJob::is_complete)
    {
        runtime.budget.cpu_working_set_bytes = runtime.estimated_cpu_working_set();
        if runtime.budget.cpu_working_set_bytes > MAX_CPU_WORKING_SET_BYTES {
            let bytes = runtime.budget.cpu_working_set_bytes; // 固定记录越界帧的真实保守估算。
            runtime.request_rollback(
                Some(RaytracingPrototypeBlocker::CpuWorkingSetExceeded { bytes }), // F1显示中间捕获帧峰值。
                RollbackDisposition::Failed, // 捕获尚未完成也必须立即停止，不能等到任务启动。
            );
        }
        return; // 本帧2MiB预算耗尽，下一帧从同一offset继续。
    }

    let job = runtime.capture_job.take().expect("完整捕获任务必须存在");
    let input = match job.into_input() {
        Ok(input) => input,
        Err(error) => {
            fail_mesh_adaptation(&mut runtime, error);
            return;
        }
    };
    let Some(pool) = AsyncComputeTaskPool::try_get() else {
        fail_mesh_adaptation(&mut runtime, "AsyncComputeTaskPool未初始化".to_string());
        return;
    };
    let identity = input.identity;
    let estimated_bytes = runtime.candidates[identity.contract_index]
        .derived_bytes
        .saturating_mul(2); // 后台转换期间按完整输入与派生输出并存保守计入两份载荷。
    let task = pool.spawn(async move { adapt_captured_mesh(input) }); // 闭包不访问ECS、Assets或GPU。
    runtime.tasks.push(PendingAdaptationTask {
        identity,         // 主线程可在任务结束前标记完整身份过期。
        cancelled: false, // 新任务属于当前代次。
        estimated_bytes,  // 候选清空后仍能持续统计过期任务真实工作集。
        task,             // 保持句柄让Bevy任务池跨帧驱动。
    });
    runtime.capture_cursor += 1; // 下一帧才开始下一项，单帧捕获总量绝不叠加超过2MiB。
    runtime.budget.cpu_working_set_bytes = runtime.estimated_cpu_working_set();
    if runtime.budget.cpu_working_set_bytes > MAX_CPU_WORKING_SET_BYTES {
        let bytes = runtime.budget.cpu_working_set_bytes;
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::CpuWorkingSetExceeded { bytes }),
            RollbackDisposition::Failed,
        );
    }
}

/// 非阻塞回收全部已完成任务，并且每帧最多把一个有效Mesh登记到Assets。
pub(crate) fn poll_and_adopt_mesh_results(
    mut meshes: ResMut<Assets<Mesh>>, // 只有通过全部身份与预算门的单项结果才登记。
    source_meshes: Query<&Mesh3d>,    // 采用前再次核对源实体与句柄。
    revisions: Res<MeshRevisionRegistry>, // 采用前再次核对源修订。
    mut runtime: ResMut<RaytracingRuntime>, // 保存任务槽、完成队列与派生句柄。
) {
    let mut index = 0; // swap_remove后继续检查换入当前位置的任务。
    while index < runtime.tasks.len() {
        let Some(result) = check_ready(&mut runtime.tasks[index].task) else {
            index += 1; // 未完成任务立即跳过，主线程不等待。
            continue;
        };
        let pending = runtime.tasks.swap_remove(index); // 完成后释放一个异步槽。
        if pending.cancelled {
            continue; // 过期成功或失败结果都只Drop，不进入完成采用队列。
        }
        let completion: CompletedAdaptation = match result {
            Ok(output) => Ok(output),
            Err(error) => Err((pending.identity, error)),
        };
        runtime.completed.push_back(completion); // 顺序不影响身份核对，每帧仍只采用一项。
    }

    if runtime.stage != RaytracingPrototypeStage::AdaptingMeshes
        || runtime.pending_rollback.is_some()
    {
        runtime.completed.clear(); // 非当前阶段的正常结果也属于迟到结果，直接丢弃。
        runtime.budget.cpu_working_set_bytes = runtime.estimated_cpu_working_set();
        return;
    }
    let Some(completion) = runtime.completed.pop_front() else {
        runtime.budget.cpu_working_set_bytes = runtime.estimated_cpu_working_set();
        maybe_finish_adaptation(&mut runtime);
        return;
    };
    let output = match completion {
        Ok(output) => output,
        Err((identity, error)) => {
            record_task_failure_if_current(
                &mut runtime,   // 只有当前会话可被后台失败推进到Failed。
                identity,       // 失败结果携带的完整启动身份。
                error,          // F1公开的后台转换失败详情。
                &revisions,     // 当前源Mesh修订权威表。
                &source_meshes, // 当前源实体实际Mesh句柄。
            ); // 旧代任务失败不得污染新代状态。
            return;
        }
    };
    if !adaptation_identity_matches(&runtime, output.identity, &revisions, &source_meshes) {
        runtime.budget.cpu_working_set_bytes = runtime.estimated_cpu_working_set();
        return; // 场景、实体、AssetId、修订或票据任一变化都只丢弃迟到成功值。
    }
    if output.derived_bytes > MAX_UPLOAD_BYTES_PER_FRAME {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::UploadBudgetExceeded {
                bytes: output.derived_bytes,
            }),
            RollbackDisposition::Failed,
        );
        return;
    }
    let contract_index = output.identity.contract_index;
    let expected_bytes = runtime.candidates[contract_index].derived_bytes;
    if output.derived_bytes != expected_bytes {
        runtime.request_rollback(
            Some(RaytracingPrototypeBlocker::AsyncTaskFailed {
                detail: format!(
                    "派生字节{}与冻结预算{}不一致",
                    output.derived_bytes, expected_bytes
                ),
            }),
            RollbackDisposition::Failed,
        );
        return;
    }
    let handle = meshes.add(output.mesh); // 本帧唯一一次派生Mesh登记与后续GPU上传请求。
    runtime.candidates[contract_index].derived_mesh = Some(handle); // 保持强句柄直到回退或切换场景。
    runtime.candidates[contract_index].adaptation_ticket = None; // 完整采用后拒绝任何同槽重复或迟到结果。
    runtime.budget.cpu_working_set_bytes = runtime.estimated_cpu_working_set();
    maybe_finish_adaptation(&mut runtime); // 全部候选全部采用后进入GPU等待阶段。
}

fn maybe_finish_adaptation(runtime: &mut RaytracingRuntime) {
    let current_tasks_running = runtime
        .tasks
        .iter()
        .any(|task| !task.cancelled && task.identity.generation == runtime.generation);
    if runtime.capture_cursor == runtime.candidates.len()
        && runtime.capture_job.is_none()
        && !current_tasks_running
        && runtime.completed.is_empty()
        && runtime
            .candidates
            .iter()
            .all(|candidate| candidate.derived_mesh.is_some())
    {
        runtime.transition(RaytracingPrototypeStage::AwaitingGpuAssets); // 只在全部候选完整后推进。
    }
}

fn fail_mesh_adaptation(runtime: &mut RaytracingRuntime, detail: String) {
    runtime.request_rollback(
        Some(RaytracingPrototypeBlocker::MeshFormatUnsupported { detail }),
        RollbackDisposition::Failed,
    );
}

#[cfg(test)]
mod tests; // 分片捕获夹具独立放入同域子模块，保持运行时文件低于500行。
