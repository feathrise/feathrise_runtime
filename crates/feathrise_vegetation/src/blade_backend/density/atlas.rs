//! 活动布局层到线性八位图集、完整 Mip 链和局部重建的纯数据转换。

use bevy::prelude::{IVec2, Vec2}; // 图集纯数据层只使用整数块坐标和世界映射。

use crate::layout::{VEGETATION_TILE_SIDE, VEGETATION_TILE_TEXEL_COUNT}; // 基础级严格按公共64×64块契约拼接。
use crate::{
    ActiveVegetationLayout, VegetationDirtyTile, VegetationLayerId, VegetationResolvedLayer,
    VegetationResolvedTile,
}; // 转换只读取一次已经原子发布的活动布局快照。

/// 单张解析叶片密度图允许的默认最大边长。
const MAX_BLADE_DENSITY_TEXTURE_SIDE: u32 = 8_192; // 防止极远稀疏块形成不可控的包含矩形。

/// 一层紧凑线性 R8 Mip 数据。
#[derive(Clone, Debug, PartialEq, Eq)]
struct DensityMipLevel {
    width: u32,      // 当前级宽度至少为一。
    height: u32,     // 当前级高度至少为一。
    pixels: Vec<u8>, // 行优先概率字节，不包含重复RGBA通道。
}

impl DensityMipLevel {
    fn zeroed(width: u32, height: u32) -> Result<Self, String> {
        let pixel_count = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| format!("vegetation density mip {width}x{height} overflowed"))?;
        Ok(Self {
            width,                        // 保存受检非零宽度。
            height,                       // 保存受检非零高度。
            pixels: vec![0; pixel_count], // 空洞、缺层与未写区域都严格为零密度。
        })
    }

    fn downsample_box(&self) -> Result<Self, String> {
        let next_width = (self.width / 2).max(1); // 与离线KTX2生成器相同，每级向下减半。
        let next_height = (self.height / 2).max(1); // 单像素轴继续保持一，直到另一轴也收敛。
        let mut next = Self::zeroed(next_width, next_height)?; // 精确分配下一层紧凑R8数组。
        let full = TexelRect {
            min_x: 0,                    // 整层重建从左边开始。
            min_y: 0,                    // 整层重建从上边开始。
            max_x: next_width as usize,  // 右边使用闭开坐标覆盖全部输出纹素。
            max_y: next_height as usize, // 下边使用闭开坐标覆盖全部输出纹素。
        };
        downsample_region(self, &mut next, full); // 复用局部路径，保证整图与脏区使用完全相同整数公式。
        Ok(next)
    }
}

/// 一层活动布局对应的CPU参考图集。
///
/// 基础级严格量化一次活动布局；后续级递归执行 `(sum + 2) / 4` 的整数 box average，
/// 与现有正式 R8 KTX2 生成器保持相同的可重复舍入口径。
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ResolvedDensityAtlas {
    layer: VegetationLayerId,     // 防止把一层的局部提交误应用到另一层缓存。
    has_tiles: bool,              // 缺失层使用独立1×1零图，而不是伪造世界块坐标。
    min_tile: IVec2,              // 基础级左上块坐标，可为负值。
    max_tile: IVec2,              // 基础级右下包含块坐标。
    texel_size: f32,              // 世界映射改变时必须退回整图重建。
    levels: Vec<DensityMipLevel>, // 基础级到1×1的完整链，局部更新逐级向父纹素传播。
    pub(super) world_min: Vec2,   // `GpuBladePatch`密度世界映射下界。
    pub(super) world_max: Vec2,   // `GpuBladePatch`密度世界映射上界。
}

impl ResolvedDensityAtlas {
    fn matches_geometry(&self, geometry: AtlasGeometry, layer: VegetationLayerId) -> bool {
        self.layer == layer
            && self.has_tiles == geometry.has_tiles
            && self.min_tile == geometry.min_tile
            && self.max_tile == geometry.max_tile
            && self.texel_size.to_bits() == geometry.texel_size.to_bits()
            && self
                .levels
                .first()
                .is_some_and(|base| base.width == geometry.width && base.height == geometry.height)
            && self.world_min == geometry.world_min
            && self.world_max == geometry.world_max // 任一范围变化都必须整图重建，禁止把局部字节写入错位图集。
    }

    fn rebuild_mips_from(&mut self, mut affected: TexelRect) {
        for level_index in 1..self.levels.len() {
            let (sources, destinations) = self.levels.split_at_mut(level_index); // 同时借用相邻两级且不复制整图。
            let source = &sources[level_index - 1]; // 当前级永远只读取刚更新完成的直接父级。
            let destination = &mut destinations[0]; // 当前输出级是切片中的第一项。
            affected = affected.downsampled(destination.width, destination.height); // 只扩大到真正受基础脏区影响的父纹素。
            downsample_region(source, destination, affected); // 使用与完整生成相同的整数box平均逐级传播。
        }
    }

    /// 把一个活动块坐标映射为本图集基础级的左上像素。
    pub(super) fn tile_origin_pixels(&self, coordinate: IVec2) -> Option<[u32; 2]> {
        if !self.has_tiles
            || coordinate.x < self.min_tile.x
            || coordinate.y < self.min_tile.y
            || coordinate.x > self.max_tile.x
            || coordinate.y > self.max_tile.y
        {
            return None; // 图集外提交块既不复制旧值，也不产生GPU越界写。
        }
        let offset_x = u32::try_from(i64::from(coordinate.x) - i64::from(self.min_tile.x)).ok()?; // 负世界块先做宽整数差值。
        let offset_y = u32::try_from(i64::from(coordinate.y) - i64::from(self.min_tile.y)).ok()?; // 图集内部偏移必须非负。
        Some([
            offset_x.checked_mul(VEGETATION_TILE_SIDE as u32)?, // 每个块固定占用64个基础级像素。
            offset_y.checked_mul(VEGETATION_TILE_SIDE as u32)?, // Y轴使用相同行优先块布局。
        ])
    }

    /// 返回与活动布局原子版本同源的世界纹素边长。
    pub(super) fn texel_size(&self) -> f32 {
        self.texel_size // 图集构造已经验证有限且严格大于零。
    }
}

mod image; // Bevy图片编码与候选基础级上传拆开，保持纯图集算法模块低于500行。

/// 从当前完整活动布局构建一层基础图集和全部 Mip。
pub(super) fn build_layer_density_atlas(
    active: &ActiveVegetationLayout,
    layer: VegetationLayerId,
) -> Result<ResolvedDensityAtlas, String> {
    let geometry = AtlasGeometry::for_layer(active, layer)?; // 先固定范围，后续任何错误都不会修改旧缓存。
    let mut base = DensityMipLevel::zeroed(geometry.width, geometry.height)?; // 空洞默认零密度。
    if geometry.has_tiles {
        for tile in active.tiles.values() {
            if let Some(resolved) = resolved_layer(tile, layer) {
                write_tile(&mut base, geometry, tile, resolved)?; // 每块写入互不重叠的64×64基础区域。
            }
        }
    }
    let mut levels = vec![base]; // 基础级必须逐字节保留活动布局的单次八位量化。
    while levels
        .last()
        .is_some_and(|level| level.width > 1 || level.height > 1)
    {
        let next = levels
            .last()
            .expect("density atlas always contains a base level")
            .downsample_box()?; // 每次只读上一层，形成完整递归box链。
        levels.push(next); // 顺序与Bevy/wgpu单层纹理上传顺序一致。
    }
    Ok(ResolvedDensityAtlas {
        layer,                           // 锁定图集所属无限层。
        has_tiles: geometry.has_tiles,   // 区分真实世界块与安全零图。
        min_tile: geometry.min_tile,     // 保存局部提交的坐标基准。
        max_tile: geometry.max_tile,     // 保存局部提交的包含边界。
        texel_size: geometry.texel_size, // 世界分辨率变化触发整图回退。
        levels,                          // 保存基础级至1×1完整Mip。
        world_min: geometry.world_min,   // 叶片窗口使用同一世界映射。
        world_max: geometry.world_max,   // 最大边仍对应包含块的外边界。
    })
}

/// 尝试只按提交脏块更新已有图集；范围变化返回 `None`，由调用方安全整图重建。
pub(super) fn patch_layer_density_atlas(
    previous: &ResolvedDensityAtlas,
    active: &ActiveVegetationLayout,
    layer: VegetationLayerId,
    dirty_tiles: &[VegetationDirtyTile],
) -> Result<Option<ResolvedDensityAtlas>, String> {
    let geometry = AtlasGeometry::for_layer(active, layer)?; // 新活动版本的范围是局部更新能否复用分配的唯一依据。
    if !previous.matches_geometry(geometry, layer) {
        return Ok(None); // 新增/移除边界块或纹素尺度改变时禁止错位局部写入。
    }
    let mut updated = previous.clone(); // 成功完成全部脏块与Mip前不触碰当前缓存。
    let mut affected = None::<TexelRect>; // 没有落在本图集内的脏块时保持字节不变。
    for dirty in dirty_tiles {
        if !geometry.contains_tile(dirty.0) {
            continue; // 其他稀疏区域或缺层块不影响当前层包含矩形。
        }
        let region = geometry.tile_region(dirty.0)?; // 64×64区域与基础级行优先字节一一对应。
        clear_region(
            updated
                .levels
                .first_mut()
                .expect("density atlas always contains a base level"),
            region,
        ); // 层在新版本被清空时不得残留上一版本概率。
        if let Some(tile) = active.tiles.get(dirty)
            && let Some(resolved) = resolved_layer(tile, layer)
        {
            write_tile(
                updated
                    .levels
                    .first_mut()
                    .expect("density atlas always contains a base level"),
                geometry,
                tile,
                resolved,
            )?; // 只有完整受检层数组才覆盖刚清零的基础区。
        }
        affected = Some(affected.map_or(region, |existing| existing.union(region))); // 合并为一次逐级局部Mip传播范围。
    }
    if let Some(affected) = affected {
        updated.rebuild_mips_from(affected); // 基础级全部成功后再更新所有受影响父纹素。
    }
    Ok(Some(updated)) // 调用方只在成功返回后替换缓存和图片资产。
}

/// 返回当前完整图集中真正驻留目标层的稳定块集合。
pub(super) fn full_layer_update_tiles(
    active: &ActiveVegetationLayout, // 当前活动布局决定哪些稀疏块拥有目标层。
    layer: VegetationLayerId,        // 一个GPU候选只更新自身绑定层所在的图集。
) -> Vec<VegetationDirtyTile> {
    active
        .tiles
        .values()
        .filter(|tile| resolved_layer(tile, layer).is_some()) // 图集空洞继续依赖候选初始零值。
        .map(|tile| tile.coordinate) // 块身份已经实现Y/X稳定全序。
        .collect() // `BTreeMap`遍历顺序直接成为GPU Z调度顺序。
}

/// 一次活动版本中某层的固定图集范围。
#[derive(Clone, Copy, Debug)]
struct AtlasGeometry {
    has_tiles: bool, // false表示使用不映射世界块的1×1安全零图。
    min_tile: IVec2, // 包含矩形最小块坐标。
    max_tile: IVec2, // 包含矩形最大块坐标。
    texel_size: f32, // 单个基础纹素覆盖的世界米数。
    width: u32,      // 基础级像素宽度。
    height: u32,     // 基础级像素高度。
    world_min: Vec2, // 密度世界映射下界。
    world_max: Vec2, // 密度世界映射上界。
}

impl AtlasGeometry {
    fn for_layer(
        active: &ActiveVegetationLayout,
        layer: VegetationLayerId,
    ) -> Result<Self, String> {
        if !active.texel_size.is_finite() || active.texel_size <= 0.0 {
            return Err(format!(
                "vegetation blade layer {:?} has invalid texel size {}",
                layer, active.texel_size
            )); // 损坏活动快照不得创建无效世界映射。
        }
        let mut bounds = None::<(IVec2, IVec2)>; // 单次扫描确定真实驻留块包含矩形。
        for tile in active.tiles.values() {
            if resolved_layer(tile, layer).is_none() {
                continue; // 其他无限层不会扩张当前叶片图集。
            }
            bounds = Some(bounds.map_or(
                (tile.coordinate.0, tile.coordinate.0),
                |(minimum, maximum)| {
                    (
                        minimum.min(tile.coordinate.0), // 负坐标同样按整数块范围参与。
                        maximum.max(tile.coordinate.0), // 最大边保留包含语义。
                    )
                },
            ));
        }
        let Some((min_tile, max_tile)) = bounds else {
            return Ok(Self {
                has_tiles: false,              // 缺层不伪造任何真实世界块。
                min_tile: IVec2::ZERO,         // 仅作稳定缓存身份占位。
                max_tile: IVec2::ZERO,         // 仅作稳定缓存身份占位。
                texel_size: active.texel_size, // 尺度仍参与缓存版本核对。
                width: 1,                      // 叶片后端始终可绑定合法非零图片。
                height: 1,                     // 安全零图只有一个纹素。
                world_min: Vec2::ZERO,         // 与旧缺层桥接保持相同映射。
                world_max: Vec2::ONE,          // 避免叶片uniform出现零宽范围。
            });
        };
        let span_x = i64::from(max_tile.x) - i64::from(min_tile.x) + 1; // 使用宽整数避免极端块坐标减法溢出。
        let span_y = i64::from(max_tile.y) - i64::from(min_tile.y) + 1; // 两轴独立验证包含跨度。
        let width = u32::try_from(span_x)
            .ok()
            .and_then(|span| span.checked_mul(VEGETATION_TILE_SIDE as u32))
            .ok_or_else(|| format!("vegetation blade layer {:?} width overflowed", layer))?;
        let height = u32::try_from(span_y)
            .ok()
            .and_then(|span| span.checked_mul(VEGETATION_TILE_SIDE as u32))
            .ok_or_else(|| format!("vegetation blade layer {:?} height overflowed", layer))?;
        if width > MAX_BLADE_DENSITY_TEXTURE_SIDE || height > MAX_BLADE_DENSITY_TEXTURE_SIDE {
            return Err(format!(
                "vegetation blade layer {:?} resolved atlas {}x{} exceeds safe {} side",
                layer, width, height, MAX_BLADE_DENSITY_TEXTURE_SIDE
            )); // 稀疏超大世界应拆成多个叶片批次，而不是分配巨型空洞图。
        }
        let tile_world_size = VEGETATION_TILE_SIDE as f32 * active.texel_size; // 块世界尺寸与布局采样完全一致。
        let world_min = min_tile.as_vec2() * tile_world_size; // 最小块左下边映射到图集零纹素端。
        let world_max = (max_tile.as_vec2() + Vec2::ONE) * tile_world_size; // 最大块外边映射到图集最大端点。
        if !world_min.is_finite() || !world_max.is_finite() {
            return Err(format!(
                "vegetation blade layer {:?} world bounds overflowed",
                layer
            )); // 极端坐标不得把NaN或无穷写入叶片uniform。
        }
        Ok(Self {
            has_tiles: true,               // 当前层至少驻留一个真实块。
            min_tile,                      // 保存包含矩形下界。
            max_tile,                      // 保存包含矩形上界。
            texel_size: active.texel_size, // 保存当前活动布局米制分辨率。
            width,                         // 每块固定64纹素后的总宽度。
            height,                        // 每块固定64纹素后的总高度。
            world_min,                     // 与块坐标和纹素尺度同源。
            world_max,                     // 与基础级完整包含范围同源。
        })
    }

    fn contains_tile(self, coordinate: IVec2) -> bool {
        self.has_tiles
            && coordinate.x >= self.min_tile.x
            && coordinate.y >= self.min_tile.y
            && coordinate.x <= self.max_tile.x
            && coordinate.y <= self.max_tile.y // 闭区间核对脏块是否映射到当前基础级。
    }

    fn tile_region(self, coordinate: IVec2) -> Result<TexelRect, String> {
        let offset_x = i64::from(coordinate.x) - i64::from(self.min_tile.x); // 宽整数保持负世界块偏移安全。
        let offset_y = i64::from(coordinate.y) - i64::from(self.min_tile.y); // 图集内部偏移必须非负。
        let base_x = usize::try_from(offset_x)
            .ok()
            .and_then(|offset| offset.checked_mul(VEGETATION_TILE_SIDE))
            .ok_or_else(|| format!("vegetation tile {coordinate:?} x offset overflowed"))?;
        let base_y = usize::try_from(offset_y)
            .ok()
            .and_then(|offset| offset.checked_mul(VEGETATION_TILE_SIDE))
            .ok_or_else(|| format!("vegetation tile {coordinate:?} y offset overflowed"))?;
        let max_x = base_x
            .checked_add(VEGETATION_TILE_SIDE)
            .ok_or_else(|| format!("vegetation tile {coordinate:?} x extent overflowed"))?;
        let max_y = base_y
            .checked_add(VEGETATION_TILE_SIDE)
            .ok_or_else(|| format!("vegetation tile {coordinate:?} y extent overflowed"))?;
        if max_x > self.width as usize || max_y > self.height as usize {
            return Err(format!(
                "vegetation tile {coordinate:?} lies outside resolved atlas {}x{}",
                self.width, self.height
            )); // 损坏坐标不得越界写入已有缓存。
        }
        Ok(TexelRect {
            min_x: base_x, // 当前块基础级左边。
            min_y: base_y, // 当前块基础级上边。
            max_x,         // 当前块基础级右侧闭开边。
            max_y,         // 当前块基础级下侧闭开边。
        })
    }
}

/// 一个行优先纹理内的闭开矩形。
#[derive(Clone, Copy, Debug)]
struct TexelRect {
    min_x: usize, // 包含的最小X。
    min_y: usize, // 包含的最小Y。
    max_x: usize, // 不包含的最大X。
    max_y: usize, // 不包含的最大Y。
}

impl TexelRect {
    fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x), // 合并后覆盖两个脏区最左边。
            min_y: self.min_y.min(other.min_y), // 合并后覆盖两个脏区最上边。
            max_x: self.max_x.max(other.max_x), // 合并后覆盖两个脏区最右边。
            max_y: self.max_y.max(other.max_y), // 合并后覆盖两个脏区最下边。
        }
    }

    fn downsampled(self, width: u32, height: u32) -> Self {
        let width = width as usize; // 当前图集最大8192，可安全转换为平台usize。
        let height = height as usize; // 当前图集最大8192，可安全转换为平台usize。
        Self {
            min_x: (self.min_x / 2).min(width), // 任一源纹素只影响其二分父坐标。
            min_y: (self.min_y / 2).min(height), // Y轴使用相同闭开映射。
            max_x: self.max_x.div_ceil(2).min(width), // 奇数闭开末端向上覆盖可能受影响父纹素。
            max_y: self.max_y.div_ceil(2).min(height), // 钳制处理非二次幂最后一行。
        }
    }
}

fn resolved_layer(
    tile: &VegetationResolvedTile,
    layer: VegetationLayerId,
) -> Option<&VegetationResolvedLayer> {
    tile.layers.iter().find(|resolved| resolved.layer == layer) // 层数组按稳定ID排序，但线性查找避免依赖外部排序正确性。
}

fn write_tile(
    base: &mut DensityMipLevel,
    geometry: AtlasGeometry,
    tile: &VegetationResolvedTile,
    resolved: &VegetationResolvedLayer,
) -> Result<(), String> {
    if resolved.density.len() != VEGETATION_TILE_TEXEL_COUNT {
        return Err(format!(
            "vegetation resolved tile {:?} layer {:?} has {} values instead of {}",
            tile.coordinate,
            resolved.layer,
            resolved.density.len(),
            VEGETATION_TILE_TEXEL_COUNT
        )); // 损坏活动快照不得造成图集越界或部分成功提交。
    }
    let region = geometry.tile_region(tile.coordinate.0)?; // 块坐标转换为基础级64×64闭开区域。
    for local_y in 0..VEGETATION_TILE_SIDE {
        let source_row = local_y * VEGETATION_TILE_SIDE; // 解析层数组固定为64纹素行优先。
        let target_row = (region.min_y + local_y) * base.width as usize + region.min_x; // 图集可能跨多个正负块。
        for local_x in 0..VEGETATION_TILE_SIDE {
            let value = resolved.density[source_row + local_x]; // 每个活动概率只在基础级量化一次。
            base.pixels[target_row + local_x] = (value.clamp(0.0, 1.0) * 255.0).round() as u8; // 保持旧桥接和WGSL八位端点语义。
        }
    }
    Ok(())
}

fn clear_region(level: &mut DensityMipLevel, region: TexelRect) {
    for y in region.min_y..region.max_y {
        let row_start = y * level.width as usize + region.min_x; // 定位当前闭开行片段。
        let row_end = y * level.width as usize + region.max_x; // 同一行只清除脏块覆盖字节。
        level.pixels[row_start..row_end].fill(0); // 新活动版本缺层或缺块时严格删除旧概率。
    }
}

fn downsample_region(
    source: &DensityMipLevel,
    destination: &mut DensityMipLevel,
    region: TexelRect,
) {
    for y in region.min_y..region.max_y {
        let source_y0 = (y * 2).min(source.height as usize - 1); // 二次幂正常命中偶数行。
        let source_y1 = (source_y0 + 1).min(source.height as usize - 1); // 单像素轴复用同一行。
        let source_row0 = source_y0 * source.width as usize; // 预计算上输入行起点。
        let source_row1 = source_y1 * source.width as usize; // 预计算下输入行起点。
        let destination_row = y * destination.width as usize; // 预计算当前输出行起点。
        for x in region.min_x..region.max_x {
            let source_x0 = (x * 2).min(source.width as usize - 1); // 二次幂正常命中偶数列。
            let source_x1 = (source_x0 + 1).min(source.width as usize - 1); // 单像素轴复用同一列。
            let total = u16::from(source.pixels[source_row0 + source_x0])
                + u16::from(source.pixels[source_row0 + source_x1])
                + u16::from(source.pixels[source_row1 + source_x0])
                + u16::from(source.pixels[source_row1 + source_x1]); // 四个R8样本最多1020，u16不会溢出。
            destination.pixels[destination_row + x] = ((total + 2) / 4) as u8; // 与离线生成器相同的最近整数平均。
        }
    }
}

#[cfg(test)]
mod gpu_tests; // 显式环境门控的真实GPU回读测试与CPU参考字节逐级比较。

#[cfg(test)]
mod tests; // 大图集回归拆到子文件，保持生产模块低于500行。
