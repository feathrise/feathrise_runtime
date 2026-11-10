//! 多primitive完整碰撞和固定捕获额度回归，不依赖窗口或GPU。
use super::{capture::Capture, resources::{CaptureBudget, Identity}};
use bevy::{prelude::*, mesh::{Indices, PrimitiveTopology}, asset::RenderAssetUsages};

#[derive(Resource)]
struct TestCapture { capture: Option<Capture>, result: Option<avian3d::prelude::Collider>, frames: usize }
fn capture(mut state: ResMut<TestCapture>, meshes: Res<Assets<Mesh>>, children: Query<&Children>, visuals: Query<(&Mesh3d, &GlobalTransform, Option<&crate::StaticMeshLodOriginal>)>, names: Query<&Name>) {
    let mut budget = CaptureBudget::default(); budget.bytes = 24; budget.nodes = 2;
    let complete = state.capture.as_mut().unwrap().step(&meshes, &children, &visuals, &names, &mut budget).unwrap();
    assert!(budget.bytes <= 24);
    state.frames += 1;
    if complete { state.result = Some(state.capture.take().unwrap().build().unwrap()); }
}
fn plane() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::MAIN_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.,0.,0.],[1.,0.,0.],[1.,0.,1.],[0.,0.,1.]]);
    mesh.insert_indices(Indices::U32(vec![0,2,1,0,3,2])); mesh
}
#[test]
fn full_scene_triangle_shape_keeps_each_node_transform_and_all_primitives() {
    let mut app = App::new(); app.init_resource::<Assets<Mesh>>().add_systems(Update, capture);
    let root = app.world_mut().spawn(GlobalTransform::from_translation(Vec3::new(20.,0.,0.))).id();
    let mesh = app.world_mut().resource_mut::<Assets<Mesh>>().add(plane());
    let a = app.world_mut().spawn((Mesh3d(mesh.clone()), GlobalTransform::from_translation(Vec3::new(20.,3.,0.)), ChildOf(root))).id();
    let b = app.world_mut().spawn((Mesh3d(mesh.clone()), GlobalTransform::from_translation(Vec3::new(24.,7.,0.)), ChildOf(root))).id();
    app.world_mut().spawn((Name::new("规划_文字_标记"), Mesh3d(mesh.clone()), GlobalTransform::from_translation(Vec3::new(20.,15.,0.)), ChildOf(root)));
    app.insert_resource(TestCapture { capture: Some(Capture::new(Identity { root, epoch: 1, revision: 0, scale: [1f32.to_bits();3] },
        Mat4::from_translation(Vec3::new(-20.,0.,0.)))), result: None, frames: 0 });
    for _ in 0..100 { if app.world().resource::<TestCapture>().result.is_some() { break; } app.update(); }
    let state = app.world().resource::<TestCapture>(); assert!(state.frames > 1);
    let collider = state.result.as_ref().expect("全部primitive应完成捕获");
    for (x, y) in [(0.5,3.0),(4.5,7.0)] {
        let ray = avian3d::parry::query::Ray::new(Vec3::new(x,20.,0.5), Vec3::NEG_Y);
        let distance = collider.shape().cast_local_ray(&ray, 100., false).unwrap();
        assert!((20. - distance - y).abs() < 0.001);
    }
    assert_eq!(app.world().get::<Mesh3d>(a).unwrap().id(), mesh.id());
    assert_eq!(app.world().get::<Mesh3d>(b).unwrap().id(), mesh.id());
}
