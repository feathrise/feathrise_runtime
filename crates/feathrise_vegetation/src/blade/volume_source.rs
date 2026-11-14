//! 编码三维作者源及密度查找索引；只在宿主后台构建，不生成逐株姿态。
use super::components::GpuBladeVolumeSource;
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

const MAX_PROBES: usize = 16; // 与Shader固定查询上限一致；编码端保证每个作者格在上限内可找到。

fn cell_hash(cell: &[[f32; 4]; 3], salt: u32) -> u32 {
    let step = cell[1][3];
    let coordinates = [0, 1, 2].map(|i| (cell[2][i] / step + 0.5).floor() as i32 as u32);
    let mut h = coordinates[0].wrapping_mul(73856093)
        ^ coordinates[1].wrapping_mul(19349663)
        ^ coordinates[2].wrapping_mul(83492791)
        ^ step.to_bits()
        ^ (cell[2][3] as u32 & 3).wrapping_mul(0x9e3779b9)
        ^ salt;
    h = (h ^ (h >> 16)).wrapping_mul(0x7feb352d);
    h = (h ^ (h >> 15)).wrapping_mul(0x846ca68b);
    h ^ (h >> 16)
}

impl GpuBladeVolumeSource {
    /// 模型三角面样本不构建规则格索引；三个RGBA记录仍进入同一GPU候选/LOD/紧凑输出路径。
    pub fn image_from_surface_samples(cells: &[[[f32;4];3]], max_bytes: usize) -> Result<Image,String> {
        if cells.iter().any(|c| c.iter().flatten().any(|v|!v.is_finite())
            || c[0][3] < 0. || c[0][3] > 1. || c[1][3] <= 0.
            || Vec3::from_slice(&c[1][..3]).length_squared() < 0.5) {
            return Err("模型植被样本包含无效位置、法线或密度".into());
        }
        let height = cells.len().checked_mul(3).ok_or("模型样本大小溢出")?.max(1).div_ceil(256);
        let bytes = height.checked_mul(256*16).ok_or("模型样本大小溢出")?;
        if bytes > max_bytes || cells.len() >= 1 << 24 { return Err("模型植被源超过8MiB上传预算".into()); }
        let mut pixels = vec![[0f32;4];height*256];
        for (i,cell) in cells.iter().enumerate() { pixels[i*3..i*3+3].copy_from_slice(cell); }
        Ok(Image::new(Extent3d {width:256,height:height as u32,depth_or_array_layers:1},TextureDimension::D2,
            bytemuck::cast_slice(&pixels).to_vec(),TextureFormat::Rgba32Float,RenderAssetUsages::RENDER_WORLD))
    }
    /// 冻结每格三个RGBA32作者记录，并建立固定最多16探测的空间索引。
    /// 调用方应在已有后台任务中使用；预算包含行对齐、索引与作者记录，失败不截断原数据。
    pub fn image_from_cells(cells: &[[[f32; 4]; 3]], max_bytes: usize) -> Result<Image, String> {
        let slots = cells
            .len()
            .checked_mul(4) // 保持最多25%装填，真实大窗口也能满足固定16探测。
            .and_then(|n| n.max(4).checked_next_power_of_two())
            .ok_or("三维植被索引大小溢出")?;
        if cells.iter().any(|c| {
            c.iter().flatten().any(|v| !v.is_finite())
                || c[1][3] <= 0.0
                || c[2][3] < 0.0
                || c[2][3] >= 16777216.0
                || c[2][3] != (c[2][3] as u32) as f32
                || (c[2][3] as u32 & 3) > 2
        }) {
            return Err("三维植被作者源包含无效坐标或格距".into());
        }
        // 室外窗口没有三维作者面作查询起点；每个网格冻结一个锚点，保持不同体积根的相位和格距。
        // 上限沿用32个常驻体积域；只记录XZ作者网格，壁面不参与室外密度交接。
        let mut grids: Vec<[f32; 4]> = Vec::new();
        for cell in cells.iter().filter(|c| c[2][3] as u32 & 3 == 1) {
            let center = Vec3::from_slice(&cell[2][..3]);
            let step = cell[1][3];
            if grids.iter().any(|g| {
                let d = (center - Vec3::from_slice(&g[..3])) / step;
                g[3] == step && (d - d.round()).abs().cmplt(Vec3::splat(0.001)).all()
            }) {
                continue;
            }
            if grids.len() == 32 {
                return Err("三维植被跨界作者网格超过32域预算".into());
            }
            grids.push(center.extend(step).to_array());
        }
        let grid_offset = 4 + slots / 4;
        let records = grid_offset + grids.len();
        let texels = cells
            .len()
            .checked_mul(3)
            .and_then(|n| n.checked_add(records))
            .ok_or("三维植被源大小溢出")?;
        let height = texels.div_ceil(256);
        let bytes = height.checked_mul(256 * 16).ok_or("三维植被源大小溢出")?;
        if bytes > max_bytes || cells.len() >= 1 << 24 || records >= 1 << 24 || slots >= 1 << 24 {
            return Err("当前三维植被源窗口超过上传预算".into());
        }
        let mut table = vec![0u32; slots];
        let mut chosen = None;
        for salt in 0..8u32 {
            table.fill(0);
            let mut complete = true;
            for (index, cell) in cells.iter().enumerate() {
                let base = cell_hash(cell, salt) as usize;
                let Some(slot) = (0..MAX_PROBES)
                    .map(|p| (base + p) & (slots - 1))
                    .find(|s| table[*s] == 0)
                else {
                    complete = false;
                    break;
                };
                table[slot] = index as u32 + 1;
            }
            if complete {
                chosen = Some(salt);
                break;
            }
        }
        let salt = chosen.ok_or("三维植被密度索引超过固定探测预算，保留上一完整草集合")?;
        let mut pixels = vec![[0f32; 4]; height * 256];
        // 概率非负，负w明确区分新索引格式和既有无头三texel调用方；整数均可由f32精确携带。
        pixels[0] = [cells.len() as f32, slots as f32, records as f32, -2.0];
        pixels[1] = [salt as f32, grids.len() as f32, grid_offset as f32, 0.];
        let first = cells
            .first()
            .map_or(Vec3::ZERO, |c| Vec3::from_slice(&c[2][..3]));
        let (min, max) = cells.iter().fold((first, first), |(min, max), c| {
            let p = Vec3::from_slice(&c[2][..3]);
            (min.min(p), max.max(p))
        });
        pixels[2] = min.extend(0.0).to_array();
        pixels[3] = max.extend(0.0).to_array();
        for (i, entry) in table.iter().enumerate() {
            pixels[4 + i / 4][i % 4] = *entry as f32;
        }
        pixels[grid_offset..records].copy_from_slice(&grids);
        for (i, cell) in cells.iter().enumerate() {
            pixels[records + i * 3..records + i * 3 + 3].copy_from_slice(cell);
        }
        Ok(Image::new(
            Extent3d {
                width: 256,
                height: height as u32,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            bytemuck::cast_slice(&pixels).to_vec(),
            TextureFormat::Rgba32Float,
            RenderAssetUsages::RENDER_WORLD,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundary_grids_preserve_each_volumes_world_phase() {
        let cells = [0., 0.07]
            .into_iter()
            .flat_map(|origin| {
                (0..4).map(move |x| {
                    let center = x as f32 * 0.25 + origin;
                    [
                        [center, 2., 0., 1.],
                        [0., 1., 0., 0.25],
                        [center, 2., 0., 1.],
                    ]
                })
            })
            .collect::<Vec<_>>();
        let image = GpuBladeVolumeSource::image_from_cells(&cells, 8 * 1024 * 1024).unwrap();
        let data: &[[f32; 4]] = bytemuck::cast_slice(image.data.as_ref().unwrap());
        assert_eq!(data[0][3], -2.);
        assert_eq!(data[1][1], 2., "同一格距但不同世界相位不能错误合并");
        let offset = data[1][2] as usize;
        assert_eq!(data[offset], [0., 2., 0., 0.25]);
        assert_eq!(data[offset + 1], [0.07, 2., 0., 0.25]);
    }
    #[test]
    fn large_sparse_source_stays_within_the_fixed_gpu_probe_limit() {
        let cells = (0..65536)
            .map(|i| {
                let x = (i % 256) as f32 * 0.25 - 32.;
                let z = (i / 256) as f32 * 0.25 - 32.;
                [[x, 2., z, 0.5], [0., 1., 0., 0.25], [x, 2., z, 1.]]
            })
            .collect::<Vec<_>>();
        let image = GpuBladeVolumeSource::image_from_cells(&cells, 8 * 1024 * 1024).unwrap();
        assert!(image.data.as_ref().unwrap().len() <= 8 * 1024 * 1024);
    }
    #[test]
    fn source_index_is_bounded_and_covers_negative_world_cells() {
        let cells = (-8..8)
            .flat_map(|x| {
                (-8..8).map(move |z| {
                    [
                        [x as f32 * 0.25, 2.0, z as f32 * 0.25, 0.5],
                        [0.0, 1.0, 0.0, 0.25],
                        [x as f32 * 0.25, 2.0, z as f32 * 0.25, 1.0],
                    ]
                })
            })
            .collect::<Vec<_>>();
        let image = GpuBladeVolumeSource::image_from_cells(&cells, 8 * 1024 * 1024).unwrap();
        let data: &[[f32; 4]] = bytemuck::cast_slice(image.data.as_ref().unwrap());
        let slots = data[0][1] as usize;
        let offset = data[0][2] as usize;
        for (i, cell) in cells.iter().enumerate() {
            let base = cell_hash(cell, data[1][0] as u32) as usize;
            assert!((0..MAX_PROBES).any(|p| {
                let s = (base + p) & (slots - 1);
                data[4 + s / 4][s % 4] as usize == i + 1
            }));
            assert_eq!(&data[offset + 3 * i..offset + 3 * i + 3], cell);
        }
        assert!(GpuBladeVolumeSource::image_from_cells(&cells, 4096).is_err());
    }
}
