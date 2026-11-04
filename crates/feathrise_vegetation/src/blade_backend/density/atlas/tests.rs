//! 解析密度图集基础级、完整 Mip 与局部更新的逐字节回归。

use std::sync::Arc; // 活动布局测试块与正式布局相同地使用不可变共享所有权。

use bevy::prelude::{IVec2, Rect}; // 构造正负块和对应世界范围。
use bevy::render::render_resource::TextureUsages; // 核对正式候选图片包含GPU存储写用途。

use crate::{VegetationResolvedLayer, VegetationResolvedTile}; // 直接构造已通过核心解析的活动块。

use super::*; // 测试纯图集转换内部数据，不启动窗口或RenderApp。

fn resolved_tile(
    coordinate: IVec2,
    layer: VegetationLayerId,
    density: Vec<f32>,
) -> VegetationResolvedTile {
    VegetationResolvedTile {
        coordinate: VegetationDirtyTile(coordinate), // 坐标同时决定图集块位置。
        world_bounds: Rect::from_corners(
            coordinate.as_vec2() * 64.0,                // 测试固定每纹素一米。
            (coordinate + IVec2::ONE).as_vec2() * 64.0, // 块外边保持闭开范围。
        ),
        layers: vec![VegetationResolvedLayer { layer, density }], // 每个测试块只需要一个无限层。
    }
}

fn constant_tile(
    coordinate: IVec2,
    layer: VegetationLayerId,
    density: f32,
) -> VegetationResolvedTile {
    resolved_tile(
        coordinate,
        layer,
        vec![density; VEGETATION_TILE_TEXEL_COUNT], // 固定值便于精确核对八位量化。
    )
}

#[test]
fn negative_and_positive_tiles_share_one_stable_density_atlas() {
    let layer = VegetationLayerId(77); // 任意稳定无限层ID。
    let mut layout = ActiveVegetationLayout {
        revision: 3,     // 版本不参与纯字节转换。
        texel_size: 1.0, // 每个基础纹素覆盖一米。
        ..Default::default()
    };
    layout.tiles.insert(
        VegetationDirtyTile(IVec2::new(-1, 0)),
        Arc::new(constant_tile(IVec2::new(-1, 0), layer, 0.25)), // 左块四分之一密度量化为64。
    );
    layout.tiles.insert(
        VegetationDirtyTile(IVec2::new(0, 0)),
        Arc::new(constant_tile(IVec2::new(0, 0), layer, 1.0)), // 右块保持满密度。
    );
    let atlas = build_layer_density_atlas(&layout, layer).unwrap();
    let image = atlas.to_image().unwrap(); // 验证最终Bevy上传字节而不只验证R8中间态。
    assert_eq!(image.width(), 128); // 两个水平块拼成128纹素。
    assert_eq!(image.height(), 64); // 单行块保持64纹素高。
    assert_eq!(atlas.world_min, Vec2::new(-64.0, 0.0)); // 负块左边正确映射到负世界。
    assert_eq!(atlas.world_max, Vec2::new(64.0, 64.0)); // 正块外边正确映射到正世界。
    let data = image.data.as_ref().unwrap();
    assert_eq!(&data[0..4], &[64, 64, 64, 255]); // 基础级首纹素保持旧RGBA桥接格式。
    assert_eq!(&data[64 * 4..64 * 4 + 4], &[255, 255, 255, 255]); // 右块首纹素没有被Mip数据错位。
}

#[test]
fn absent_layer_produces_safe_zero_density_image() {
    let layout = ActiveVegetationLayout {
        revision: 1,     // 已发布布局也可能不再包含某个叶片层。
        texel_size: 1.0, // 缺层零图仍保留合法尺度身份。
        ..Default::default()
    };
    let atlas = build_layer_density_atlas(&layout, VegetationLayerId(5)).unwrap();
    let image = atlas.to_image().unwrap();
    assert_eq!(image.data.as_deref(), Some(&[0, 0, 0, 255][..])); // 单像素图片同时是完整Mip链。
    assert_eq!(image.texture_descriptor.mip_level_count, 1); // 1×1只需要基础级。
    assert_eq!(atlas.world_min, Vec2::ZERO); // 保持旧安全映射下界。
    assert_eq!(atlas.world_max, Vec2::ONE); // 保持非零安全映射上界。
}

#[test]
fn complete_mip_chain_uses_the_offline_integer_box_rounding() {
    let level = DensityMipLevel {
        width: 2,                 // 最小可降采样二维基础级。
        height: 2,                // 四个像素共同生成一个父纹素。
        pixels: vec![0, 1, 2, 3], // 总和六，加二后整除四得到二。
    };
    let downsampled = level.downsample_box().unwrap();
    assert_eq!(downsampled.width, 1); // 水平收敛到一。
    assert_eq!(downsampled.height, 1); // 垂直收敛到一。
    assert_eq!(downsampled.pixels, vec![2]); // 精确锁定 `(sum + 2) / 4` 舍入。

    let layer = VegetationLayerId(9);
    let mut layout = ActiveVegetationLayout {
        revision: 1,
        texel_size: 1.0,
        ..Default::default()
    };
    layout.tiles.insert(
        VegetationDirtyTile(IVec2::ZERO),
        Arc::new(constant_tile(IVec2::ZERO, layer, 0.5)), // 64×64基础级应生成7级完整链。
    );
    let atlas = build_layer_density_atlas(&layout, layer).unwrap();
    let image = atlas.to_image().unwrap();
    assert_eq!(image.texture_descriptor.mip_level_count, 7); // 64、32、16、8、4、2、1。
    assert!(
        image
            .texture_descriptor
            .usage
            .contains(TextureUsages::STORAGE_BINDING)
    ); // 正式图集已具备P5.2b逐级GPU写Mip所需的存储纹理用法。
    assert_eq!(atlas.levels.last().unwrap().pixels, vec![128]); // 常量128在全部Mip保持不变。
    let expected_pixels = atlas
        .levels
        .iter()
        .map(|level| level.pixels.len())
        .sum::<usize>();
    assert_eq!(image.data.as_ref().unwrap().len(), expected_pixels * 4); // 图片包含全部级而非只有基础级。
}

#[test]
fn gpu_candidate_uploads_only_the_valid_base_level() {
    let layer = VegetationLayerId(10); // 单个64×64块会拥有七级完整链。
    let mut layout = ActiveVegetationLayout {
        revision: 2,     // 候选图片字节不依赖版本。
        texel_size: 1.0, // 使用合法一米基础纹素。
        ..Default::default()
    };
    layout.tiles.insert(
        VegetationDirtyTile(IVec2::ZERO),
        Arc::new(constant_tile(IVec2::ZERO, layer, 0.5)), // CPU参考全部级本应保持128。
    );
    let atlas = build_layer_density_atlas(&layout, layer).unwrap(); // 建立完整CPU权威。
    let expected = atlas.rgba_mip_bytes().unwrap(); // 保存基础级至1×1参考链。
    let candidate = atlas.to_gpu_candidate_image().unwrap(); // 正式RenderApp候选只上传基础级。
    let candidate_bytes = candidate.data.as_ref().unwrap(); // 提取前检查连续Mip字节。
    let base_bytes = VEGETATION_TILE_TEXEL_COUNT * 4; // 64×64 RGBA基础级长度。
    assert_eq!(&candidate_bytes[..base_bytes], &expected[..base_bytes]); // GPU输入基础级必须与CPU参考完全相同。
    assert!(candidate_bytes[base_bytes..].iter().all(|byte| *byte == 0)); // 所有子Mip显式为零，成功结果不可能来自CPU预填充。
    assert_eq!(candidate_bytes.len(), expected.len()); // 候选仍满足Bevy完整Mip上传长度契约。
}

#[test]
fn local_dirty_patch_matches_full_rebuild_at_every_mip_byte() {
    let layer = VegetationLayerId(13);
    let left = VegetationDirtyTile(IVec2::new(-1, 0)); // 使用负块验证局部偏移不依赖世界正象限。
    let right = VegetationDirtyTile(IVec2::new(0, 0)); // 相邻块让高层Mip跨越脏块边界取样。
    let mut initial = ActiveVegetationLayout {
        revision: 1,
        texel_size: 1.0,
        ..Default::default()
    };
    initial
        .tiles
        .insert(left, Arc::new(constant_tile(left.0, layer, 0.25))); // 未变左块作为局部缓存基线。
    initial
        .tiles
        .insert(right, Arc::new(constant_tile(right.0, layer, 0.75))); // 即将由非均匀值替换的右块。
    let previous = build_layer_density_atlas(&initial, layer).unwrap();

    let changed_values = (0..VEGETATION_TILE_TEXEL_COUNT)
        .map(|index| (index % 256) as f32 / 255.0)
        .collect::<Vec<_>>(); // 同一块内包含全部八位端点，能暴露行、列和舍入差异。
    let mut changed = initial.clone();
    changed.revision = 2; // 模拟下一次完整原子提交。
    changed.tiles.insert(
        right,
        Arc::new(resolved_tile(right.0, layer, changed_values)), // 只替换提交清单中的右块。
    );
    let patched = patch_layer_density_atlas(&previous, &changed, layer, &[right])
        .unwrap()
        .expect("unchanged atlas bounds should allow a local patch");
    let rebuilt = build_layer_density_atlas(&changed, layer).unwrap();
    assert_eq!(patched, rebuilt); // 基础级、全部Mip、世界范围和缓存身份逐项完全相同。
    assert_eq!(
        patched.to_image().unwrap().data,
        rebuilt.to_image().unwrap().data
    ); // 最终RGBA上传字节也必须逐字节一致。
}

#[test]
fn atlas_extent_change_requires_safe_full_rebuild() {
    let layer = VegetationLayerId(21);
    let origin = VegetationDirtyTile(IVec2::ZERO);
    let mut initial = ActiveVegetationLayout {
        revision: 1,
        texel_size: 1.0,
        ..Default::default()
    };
    initial
        .tiles
        .insert(origin, Arc::new(constant_tile(origin.0, layer, 1.0))); // 初始图集只有64×64。
    let previous = build_layer_density_atlas(&initial, layer).unwrap();

    let added = VegetationDirtyTile(IVec2::new(1, 0));
    let mut expanded = initial.clone();
    expanded.revision = 2;
    expanded
        .tiles
        .insert(added, Arc::new(constant_tile(added.0, layer, 0.5))); // 新边界把基础级扩为128×64。
    assert!(
        patch_layer_density_atlas(&previous, &expanded, layer, &[added])
            .unwrap()
            .is_none()
    ); // 调用方必须整图重建，不能把新块写到旧分配之外。
}
