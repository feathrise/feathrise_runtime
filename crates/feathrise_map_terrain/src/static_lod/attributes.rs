//! 全格式顶点属性的有界捕获与后台压紧；不能因LOD丢掉UV、切线或自定义通道。
use bevy::mesh::VertexAttributeValues;
use std::ops::Range;

// 此表只描述Bevy的标量存储，捕获与压紧共用它，避免两条路径支持的属性格式漂移。
macro_rules! attribute_variants {
    ($operation:ident, $($argument:expr),*) => {
        $operation!($($argument),*;
            Uint8, Uint8x2, Uint8x4, Sint8, Sint8x2, Sint8x4,
            Unorm8, Unorm8x2, Unorm8x4, Snorm8, Snorm8x2, Snorm8x4,
            Uint16, Uint16x2, Uint16x4, Sint16, Sint16x2, Sint16x4,
            Unorm16, Unorm16x2, Unorm16x4, Snorm16, Snorm16x2, Snorm16x4,
            Float16, Float16x2, Float16x4, Float32, Float32x2, Float32x3, Float32x4,
            Uint32, Uint32x2, Uint32x3, Uint32x4, Sint32, Sint32x2, Sint32x3, Sint32x4,
            Float64, Float64x2, Float64x3, Float64x4, Unorm10_10_10_2, Unorm8x4Bgra
        )
    };
}

/// 建立同格式的空属性，不克隆源数组；帧内捕获随后按字节预算追加。
pub fn empty_static_mesh_attribute(source: &VertexAttributeValues) -> VertexAttributeValues {
    macro_rules! empty {
        ($source:expr; $($variant:ident),*) => {
            match $source { $(VertexAttributeValues::$variant(_) => VertexAttributeValues::$variant(Vec::new()),)* }
        };
    }
    attribute_variants!(empty, source)
}

/// 复制调用方已核算预算的小片段。格式或范围错误时保持目标不变，不向后端传坏输入。
pub fn copy_static_mesh_attribute_range(
    source: &VertexAttributeValues,
    target: &mut VertexAttributeValues,
    range: Range<usize>,
) -> Result<(), String> {
    macro_rules! copy {
        ($source:expr, $target:expr, $range:expr; $($variant:ident),*) => {
            match ($source, $target) {
                $((VertexAttributeValues::$variant(values), VertexAttributeValues::$variant(out)) => {
                    let part = values.get($range).ok_or("静态网格属性捕获范围无效")?;
                    out.extend_from_slice(part);
                    Ok(())
                },)*
                _ => Err("静态网格属性捕获格式改变".to_owned()),
            }
        };
    }
    attribute_variants!(copy, source, target, range)
}

/// 只复制简化索引仍引用的原顶点，所有通道使用同一重映射，不重算法线或纹理坐标。
pub(super) fn remap_attribute(source: &VertexAttributeValues, vertices: &[usize]) -> VertexAttributeValues {
    macro_rules! remap {
        ($source:expr, $vertices:expr; $($variant:ident),*) => {
            match $source {
                $(VertexAttributeValues::$variant(values) => VertexAttributeValues::$variant(
                    $vertices.iter().map(|&index| values[index]).collect()
                ),)*
            }
        };
    }
    attribute_variants!(remap, source, vertices)
}
