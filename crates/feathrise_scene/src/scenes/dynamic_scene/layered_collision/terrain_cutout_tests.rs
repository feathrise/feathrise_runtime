//! 用真实Parry射线确认小洞口、重叠洞口和封口恢复；不以自有采样算法充当真值。
use super::*;
use bevy::prelude::Vec3;
use feathrise_scene_content::SceneTerrainCutout;

#[test]
fn exact_holes_remove_collision_and_restore_without_reusing_open_blocks() {
    let closed = request();
    let closed_layout = build::prepare_layout(&closed).unwrap();
    let mut opened = closed.clone();
    opened.holes = Arc::from([
        SceneTerrainCutout {
            min: [128.1, 128.1],
            max: [128.35, 128.35],
        },
        SceneTerrainCutout {
            min: [128.25, 128.2],
            max: [128.5, 128.4],
        },
    ]); // 面积小于粗碰撞单元，且两个洞口重叠。
    let layout = build::prepare_layout_cached(&opened, Some((&closed, &closed_layout))).unwrap();
    assert!(layout.reused_blocks > 0 && layout.rebuilt_blocks > 0); // 仅相关块失效。
    let stamps = blank_stamps(&layout);
    let cooked = build::cook(
        &opened,
        layout,
        stamps,
        BTreeSet::new(),
        BTreeMap::new(),
        Default::default(),
    )
    .unwrap();
    let hit = |shapes: &[(
        super::super::types::ChunkPlan,
        SourceStamp,
        avian3d::prelude::Collider,
    )],
               point: Vec2| {
        let ray = avian3d::parry::query::Ray::new(Vec3::new(point.x, 10.0, point.y), Vec3::NEG_Y);
        shapes
            .iter()
            .any(|(_, _, shape)| shape.shape().cast_local_ray(&ray, 20.0, false).is_some())
    };
    assert!(!hit(&cooked.shapes, Vec2::new(128.2, 128.2))); // 第一小洞内部。
    assert!(!hit(&cooked.shapes, Vec2::new(128.4, 128.3))); // 第二洞独有部分。
    assert!(hit(&cooked.shapes, Vec2::new(128.0, 128.2))); // 洞外贴近边缘仍有承托。
    let restored =
        build::prepare_layout_cached(&closed, Some((&opened, cooked.layout.as_deref().unwrap())))
            .unwrap();
    assert!(restored.rebuilt_blocks > 0);
    let stamps = blank_stamps(&restored);
    let restored_cooked = build::cook(
        &closed,
        restored,
        stamps,
        BTreeSet::new(),
        BTreeMap::new(),
        Default::default(),
    )
    .unwrap();
    assert!(hit(&restored_cooked.shapes, Vec2::new(128.2, 128.2))); // 关闭或撤销并保存后不能复用开洞形状。
}
