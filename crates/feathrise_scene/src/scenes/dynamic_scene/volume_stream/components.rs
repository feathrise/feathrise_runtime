//! 驻留根实际加载的空间身份；版本文件相同不代表窗口及玩家承托也相同。
use bevy::prelude::Component;

#[derive(Component, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct StreamedVolumeResident {
    pub center: [i32; 3],
    pub physics_center: Option<[i32; 3]>,
    pub lod_signature: u64,
}

/// 固定最多16档的完整位模式指纹；不会把显示配置变化误认为已有窗口。
pub fn volume_lod_signature(lods: &[feathrise_terrain::volume::sparse::paged::VolumeDisplayLod]) -> u64 {
    if lods.is_empty(){return 0;}
    lods.iter().flat_map(|l|[l.outer_half_extent_meters.to_bits(),l.spacing_meters.to_bits()])
        .fold(0xcbf29ce484222325u64,|h,v|(h^u64::from(v)).wrapping_mul(0x100000001b3))
}
