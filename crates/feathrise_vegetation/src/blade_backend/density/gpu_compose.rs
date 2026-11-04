//! 正式布局输入到GPU基础级竞争结果的冷路径ABI。

use std::sync::Arc; // 多个绑定层共享同一组输入字节，避免正式三层重复保留百兆级编码。

use bevy::asset::uuid_handle; // 内置竞争Shader使用稳定句柄进入Bevy资产缓存。
use bevy::prelude::{Handle, Shader, UVec2}; // 管线句柄与基础级尺寸进入跨世界作业。
use bevy::render::render_resource::ShaderType; // Rust uniform必须与WGSL三十二字节布局严格一致。

use crate::{VegetationCompetitionGroupId, VegetationDirtyTile, VegetationLayerId}; // 输入共享身份和目标索引仍使用内容无关公共ID。

mod encode; // 主世界只在布局加载或换版时把正式源采样编码为紧凑存储缓冲。
pub(super) mod pipeline; // RenderStartup排队独立基础级竞争Compute管线。

pub(super) use encode::build_gpu_competition_input; // 候选构建器不接触GPU资源创建细节。

/// 单个GPU竞争组允许的层数上限。
pub(super) const GPU_COMPETITION_MAX_LAYERS: usize = 32; // 公共层总数仍无限；超大组应拆成多个后端批次。

/// 单个存储输入缓冲的固定CPU侧上限。
pub(super) const GPU_COMPETITION_MAX_BUFFER_BYTES: usize = 128 * 1024 * 1024; // 与常见桌面WebGPU最低可用大缓冲档对齐并限制加载峰值。

/// 一个64纹素提交块在8×8工作组下的单轴组数。
pub(super) const GPU_COMPETITION_TILE_WORKGROUPS: u32 = 8; // 每个Z切片恰好覆盖一块64×64基础级区域。

/// WGSL识别的完全叠加模式。
pub(super) const GPU_COMPETITION_MODE_ADDITIVE: u32 = 0; // 目标层保持自身实际密度。

/// WGSL识别的加权容量分区模式。
pub(super) const GPU_COMPETITION_MODE_WEIGHTED: u32 = 1; // 目标层参与优先级和封顶水填充。

/// WGSL识别的优先覆盖模式。
pub(super) const GPU_COMPETITION_MODE_PRIORITY: u32 = 2; // 最高优先级声明层取得稳定世界所有权。

/// 正式GPU基础级竞争Shader的稳定句柄。
pub(super) const GPU_COMPOSE_SHADER_HANDLE: Handle<Shader> =
    uuid_handle!("9c4a18e2-cba8-4974-9c97-6d39a8655824"); // 独立UUID避免与Mip和叶片Compute管线冲突。

/// 正式GPU竞争Shader源码，供真实设备门控测试复用。
#[cfg(test)]
pub(super) const GPU_COMPOSE_SHADER: &str = include_str!("shaders/vegetation_layout_compose.wgsl"); // 运行时与测试禁止维护两份竞争算法。

/// 一个竞争组跨绑定层共享的不可变CPU输入编码。
#[derive(Clone, Debug)]
pub(super) struct VegetationGpuCompetitionInput {
    pub(super) group: VegetationCompetitionGroupId, // 同组绑定层才能共享样本和上下文。
    pub(super) width: u32,                          // 候选基础级真实宽度。
    pub(super) height: u32,                         // 候选基础级真实高度。
    pub(super) tiles: Arc<[VegetationDirtyTile]>,   // 稳定提交块用于共享计划核对和诊断。
    pub(super) layer_ids: Arc<[VegetationLayerId]>, // CPU按稳定ID查找当前目标索引。
    pub(super) layer_bytes: Arc<[u8]>, // 每层一个`vec4<u32>`：优先级位、权重位和保留字段。
    pub(super) tile_bytes: Arc<[u8]>,  // 每块一个`vec4<u32>`：输出原点、样本起点和保留标志。
    pub(super) sample_bytes: Arc<[u8]>, // 每纹素逐层一个`vec2<f32>`实际/所有权需求。
    pub(super) context_bytes: Arc<[u8]>, // 每纹素一个`vec2<f32>`世界选择值/足迹影响。
    pub(super) mode: u32,              // 只允许三个已验证WGSL模式常量。
    pub(super) mode_value: f32,        // Weighted容量或Priority声明阈值；Additive固定为零。
}

impl VegetationGpuCompetitionInput {
    /// 返回当前绑定层在稳定组数组中的GPU索引。
    pub(super) fn target_layer_index(&self, layer: VegetationLayerId) -> Result<u32, String> {
        let index = self
            .layer_ids
            .iter()
            .position(|candidate| *candidate == layer)
            .ok_or_else(|| {
                format!(
                    "vegetation GPU competition group {:?} does not contain target layer {:?}",
                    self.group, layer
                )
            })?; // 错组绑定不能静默写入另一层结果。
        u32::try_from(index).map_err(|_| {
            format!(
                "vegetation GPU competition target layer {:?} index overflowed",
                layer
            )
        }) // 受32层上限约束的正常索引必然可转换。
    }

    /// 核对同组另一绑定层是否可以复用当前大输入缓冲。
    pub(super) fn matches_plan(
        &self,
        width: u32,                    // 另一层候选基础级宽。
        height: u32,                   // 另一层候选基础级高。
        tiles: &[VegetationDirtyTile], // 另一层完整或局部提交块序列。
    ) -> bool {
        self.width == width && self.height == height && self.tiles.as_ref() == tiles // 只有完全同图集和同脏区才共享GPU写入坐标。
    }

    /// 当前作业实际调度的提交块数量。
    pub(super) fn tile_count(&self) -> Result<u32, String> {
        u32::try_from(self.tiles.len()).map_err(|_| {
            format!(
                "vegetation GPU competition tile count {} overflowed",
                self.tiles.len()
            )
        }) // 图集8192边上限下正常块数远小于u32容量。
    }
}

/// Rust与WGSL共享的单层基础级竞争参数。
#[derive(Clone, Copy, Debug, ShaderType)]
pub(super) struct GpuCompetitionUniform {
    pub(super) atlas_size: UVec2,       // 输出基础级真实宽高。
    pub(super) layer_count: u32,        // 当前竞争组稳定层数。
    pub(super) target_layer_index: u32, // 当前串行候选要写出的层索引。
    pub(super) mode: u32,               // Additive、Weighted或Priority常量。
    pub(super) tile_count: u32,         // Z工作组不得越过提交块数组。
    pub(super) mode_value: f32,         // 容量或声明阈值。
    pub(super) padding: u32,            // 补齐三十二字节uniform布局。
}

#[cfg(test)]
mod tests; // 输入打包、脏块共享与模式ABI回归独立存放。
