//! 按原始作者线段生成固定批次地表轮廓，端点高度与编辑器调用同一函数。
use bevy::prelude::*;
use feathrise_scene_content::{
    SceneBoundariesAsset, SceneManifestAsset, boundary_display_point, boundary_footprint_lines,
};
const PARTS_PER_BATCH: usize = 256; // 每批最多1536顶点；隐藏路径也占预算。
#[derive(Clone, Copy, Default)]
pub(super) struct Cursor {
    path: usize,
    segment: usize,
}
pub(super) struct Batch {
    pub asset: GizmoAsset,
    pub next: Option<Cursor>,
}
pub(super) fn build(
    document: &SceneBoundariesAsset,
    manifest: &SceneManifestAsset,
    heights: &[u16],
    mut cursor: Cursor,
) -> Batch {
    let mut asset = GizmoAsset::default();
    let mut budget = PARTS_PER_BATCH;
    while cursor.path < document.paths.len() && budget > 0 {
        let path = &document.paths[cursor.path];
        let len = path.points.len();
        let count = len.saturating_sub(1) + usize::from(path.closed && len >= 3);
        if !path.visible || cursor.segment >= count {
            cursor.path += 1;
            cursor.segment = 0;
            budget -= 1; // 空/隐藏路径同样消耗预算，不能绕过上限扫描百万条。
            continue;
        }
        let start = path.points[cursor.segment];
        let end = path.points[(cursor.segment + 1) % len];
        if let Some(lines) = boundary_footprint_lines(start, end, path.thickness) {
            for (start, end) in lines {
                asset.line(
                    boundary_display_point(Some(heights), manifest, start),
                    boundary_display_point(Some(heights), manifest, end),
                    Color::srgba(0.45, 0.68, 0.67, 0.55),
                ); // 普通选择轮廓，不含控制点/整面/高亮。
            }
        }
        cursor.segment += 1;
        budget -= 1;
    }
    let next = (cursor.path < document.paths.len()).then_some(cursor);
    Batch { asset, next }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editor_outline_uses_original_endpoints_and_heightfield_even_for_disabled_walls() {
        let manifest: SceneManifestAsset = serde_json::from_str(r#"{
            "schema_version":2,"id":"MAP-outline","display_name":"轮廓","runtime_scene":"data_driven",
            "world_min":[0,0],"world_max":[1000,1000],"default_spawn":[0,0,0],
            "terrain":{"kind":"heightfield","glb_path":"scenes/MAP-outline/terrain/terrain.glb","height_path":"scenes/MAP-outline/terrain/height.r16","resolution":[2,2],"base_height":0,"height_scale":10},"asset_roots":[]
        }"#).unwrap();
        let mut document = SceneBoundariesAsset::rectangular_default(
            &manifest.id,
            manifest.world_min,
            manifest.world_max,
            -100.0,
            1000.0,
        )
        .unwrap();
        document.paths[0].points = vec![[10.0, 10.0], [900.0, 10.0]];
        document.paths[0].closed = false;
        document.paths[0].enabled = false; // 编辑器允许显示停用碰撞的作者墙。
        let heights = [u16::MAX; 4];
        let first = build(&document, &manifest, &heights, Cursor::default());
        let vertices = &first.asset.buffer().list_positions;
        assert_eq!(vertices.len(), 6); // 890m原始段仍是3条完整线，不能使用128m物理切段制造额外贴地点。
        assert!(vertices.iter().all(|p| (p.y - 10.35).abs() < 0.0001));
        assert_eq!(vertices[0], Vec3::new(10.0, 10.35, 10.0));
        assert_eq!(vertices[1], Vec3::new(900.0, 10.35, 10.0));
        document.paths[0].bottom_y = 1_000_000.0;
        document.paths[0].top_y = 2_000_000.0;
        let tall = build(&document, &manifest, &heights, Cursor::default());
        assert_eq!(vertices, &tall.asset.buffer().list_positions); // 地表轮廓不受物理墙高影响。
        document.paths[0].visible = false;
        assert!(
            build(&document, &manifest, &heights, Cursor::default())
                .asset
                .buffer()
                .list_positions
                .is_empty()
        );
    }
}
