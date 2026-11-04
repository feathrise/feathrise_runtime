//! 隐藏候选分帧登记和取消后的后台所有权回归。
use super::resources::UploadGarbage;
use super::*;

#[test]
fn upload_keeps_all_chunks_private_and_cancel_moves_remaining_meshes_to_retirement() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .init_resource::<Assets<Mesh>>();
    install(&mut app);
    let root = app
        .world_mut()
        .spawn((
            Transform::default(),
            Visibility::Hidden,
            TerrainMeshUpload::new(
                (0..40).map(|_| Mesh::from(Cuboid::default())).collect(),
                Handle::default(),
            ),
        ))
        .id();
    app.update();
    assert_eq!(app.world().resource::<Assets<Mesh>>().len(), 1);
    assert_eq!(
        app.world()
            .get::<TerrainMeshUpload>(root)
            .unwrap()
            .meshes
            .len(),
        39
    );
    assert_eq!(
        app.world().get::<Visibility>(root),
        Some(&Visibility::Hidden)
    );
    app.world_mut().despawn(root);
    let garbage = app.world().resource::<UploadGarbage>();
    assert_eq!(garbage.pending.len(), 1);
    assert_eq!(garbage.pending[0].len(), 39);
    assert_eq!(
        app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
        0
    );
    app.update();
    assert!(app.world().resource::<UploadGarbage>().pending.is_empty());
}
