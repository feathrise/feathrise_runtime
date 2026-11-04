//! 全Mip纹理到单个MAP_READ缓冲的对齐规划与紧凑解码。

/// `copy_texture_to_buffer`要求的公开行对齐。
const COPY_ROW_ALIGNMENT: u32 = 256; // Vulkan与DX12统一使用WebGPU核心契约。

/// 一个Mip在共享回读缓冲中的位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::blade_backend::density) struct GpuMipReadbackLevel {
    pub(in crate::blade_backend::density) mip_level: u32, // 复制命令选择的真实纹理Mip。
    pub(in crate::blade_backend::density) width: u32,     // 去除行填充时保留的有效像素宽。
    pub(in crate::blade_backend::density) height: u32,    // 当前级有效行数。
    pub(in crate::blade_backend::density) offset: u64,    // 当前级在共享MAP_READ缓冲中的对齐起点。
    pub(in crate::blade_backend::density) padded_row_bytes: u32, // WGPU复制使用的256字节对齐行跨度。
}

/// 基础级至1×1共享缓冲规划。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::blade_backend::density) struct GpuMipReadbackPlan {
    levels: Vec<GpuMipReadbackLevel>, // 级顺序与Bevy图片连续Mip字节顺序一致。
    buffer_size: u64,                 // 包含各级行填充且至少为一的MAP_READ缓冲大小。
    packed_size: usize,               // 去除填充后的全部RGBA8字节数。
}

impl GpuMipReadbackPlan {
    /// 从合法基础尺寸和声明级数构造完整回读计划。
    pub(in crate::blade_backend::density) fn new(
        base_width: u32,      // 候选GpuImage基础级宽。
        base_height: u32,     // 候选GpuImage基础级高。
        mip_level_count: u32, // 必须完整收敛到1×1且不多不少。
    ) -> Result<Self, String> {
        if base_width == 0 || base_height == 0 || mip_level_count == 0 {
            return Err(format!(
                "vegetation GPU mip readback requires non-zero extent and levels, got {base_width}x{base_height} with {mip_level_count} levels"
            )); // WebGPU禁止零纹理和零字节有效计划。
        }
        let mut width = base_width; // 每轮保存当前级宽。
        let mut height = base_height; // 高度独立收敛以支持非正方形图集。
        let mut cursor = 0_u64; // 下一级从当前对齐段之后开始。
        let mut packed_size = 0_usize; // 紧凑CPU比较字节不含GPU行填充。
        let mut levels = Vec::new(); // 真实级数很小，不依赖内容层数量。
        loop {
            let row_bytes = width
                .checked_mul(4)
                .ok_or_else(|| format!("vegetation GPU mip row {width} RGBA bytes overflowed"))?; // 每像素固定四字节。
            let padded_row_bytes = row_bytes
                .div_ceil(COPY_ROW_ALIGNMENT)
                .checked_mul(COPY_ROW_ALIGNMENT)
                .ok_or_else(|| format!("vegetation GPU mip padded row {row_bytes} overflowed"))?; // 复制行跨度满足256字节。
            let offset = cursor
                .div_ceil(u64::from(COPY_ROW_ALIGNMENT))
                .checked_mul(u64::from(COPY_ROW_ALIGNMENT))
                .ok_or_else(|| "vegetation GPU mip readback offset overflowed".to_string())?; // 每级起点也保守对齐。
            levels.push(GpuMipReadbackLevel {
                mip_level: u32::try_from(levels.len())
                    .map_err(|_| "vegetation GPU mip level index overflowed".to_string())?, // 当前最大边受限但仍显式验证。
                width,            // 保存当前有效列数。
                height,           // 保存当前有效行数。
                offset,           // 复制与解码共享同一位置。
                padded_row_bytes, // 复制与解码共享同一行跨度。
            });
            let level_packed = usize::try_from(row_bytes)
                .ok()
                .and_then(|row| row.checked_mul(height as usize))
                .ok_or_else(|| {
                    format!("vegetation GPU mip {width}x{height} packed bytes overflowed")
                })?; // 计算无填充比较长度。
            packed_size = packed_size
                .checked_add(level_packed)
                .ok_or_else(|| "vegetation GPU mip packed chain overflowed".to_string())?; // 累计基础级至当前级。
            cursor = offset
                .checked_add(u64::from(padded_row_bytes) * u64::from(height))
                .ok_or_else(|| "vegetation GPU mip readback buffer overflowed".to_string())?; // 下一段位于当前完整填充数据之后。
            if width == 1 && height == 1 {
                break; // 完整链唯一终点。
            }
            width = (width / 2).max(1); // 与CPU和WGSL相同的向下整除二。
            height = (height / 2).max(1); // 单像素轴保持一直到另一轴收敛。
        }
        if levels.len() != mip_level_count as usize {
            return Err(format!(
                "vegetation GPU mip level count {} does not match complete chain {} for {base_width}x{base_height}",
                mip_level_count,
                levels.len()
            )); // 禁止缺级或多级图片绕过完整链回读。
        }
        Ok(Self {
            levels,                     // 保存全部复制区。
            buffer_size: cursor.max(1), // WGPU缓冲尺寸始终非零。
            packed_size,                // 解码一次精确分配。
        })
    }

    /// 返回全部级复制区。
    pub(in crate::blade_backend::density) fn levels(&self) -> &[GpuMipReadbackLevel] {
        &self.levels // RenderGraph只读并逐级录入复制。
    }

    /// 返回MAP_READ缓冲总大小。
    pub(in crate::blade_backend::density) fn buffer_size(&self) -> u64 {
        self.buffer_size // 创建缓冲和完整映射使用同一范围。
    }

    /// 去除每行填充并恢复Bevy连续Mip紧凑字节顺序。
    pub(in crate::blade_backend::density) fn decode(
        &self,
        mapped: &[u8],
    ) -> Result<Vec<u8>, String> {
        if mapped.len() < self.buffer_size as usize {
            return Err(format!(
                "vegetation GPU mip mapped bytes {} are shorter than planned {}",
                mapped.len(),
                self.buffer_size
            )); // 短映射不得发生切片越界或部分成功。
        }
        let mut packed = Vec::with_capacity(self.packed_size); // 一次分配全部CPU比较字节。
        for level in &self.levels {
            let row_bytes = level.width as usize * 4; // 有效RGBA行长度不含填充。
            for row in 0..level.height as usize {
                let start = level.offset as usize + row * level.padded_row_bytes as usize; // 定位当前有效行。
                let end = start
                    .checked_add(row_bytes)
                    .ok_or_else(|| "vegetation GPU mip decode row overflowed".to_string())?; // 防御损坏计划。
                let bytes = mapped.get(start..end).ok_or_else(|| {
                    format!(
                        "vegetation GPU mip level {} row {} lies outside mapped buffer",
                        level.mip_level, row
                    ) // 映射不完整时返回明确层与行。
                })?;
                packed.extend_from_slice(bytes); // 丢弃当前行尾256字节填充。
            }
        }
        if packed.len() != self.packed_size {
            return Err(format!(
                "vegetation GPU mip decoded bytes {} do not match planned {}",
                packed.len(),
                self.packed_size
            )); // 完整链必须一次解码齐全。
        }
        Ok(packed) // 顺序可直接与CPU `rgba_mip_bytes`逐字节比较。
    }
}
