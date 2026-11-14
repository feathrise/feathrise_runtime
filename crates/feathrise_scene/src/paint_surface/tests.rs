//! 实际三角表面的刷印、叠层隔离、几何变换及静态 GLB 来源回归。
use super::*;
use feathrise_scene_content::{ModelSurfaceVegetation,ScenePlacementAssetTarget,ScenePlacementRecord};
use std::{path::Path,sync::Arc};

fn floors()->Vec<MeshSurfacePrimitive> {
    vec![MeshSurfacePrimitive {primitive:0,
        positions:Arc::new([0.0,0.05].into_iter().flat_map(|y|[[-2.,y,-2.],[2.,y,-2.],[2.,y,2.],[-2.,y,2.]]).collect()),
        indices:Arc::new(vec![0,2,1,0,3,2,4,6,5,4,7,6]),primitive_to_object:Mat4::IDENTITY}]
}

#[test]
fn close_parallel_floors_paint_independently_and_erase_keeps_the_other_floor() {
    let mesh=MeshSurfaceSnapshot::build(floors()).unwrap();
    let mut data=ModelSurfaceVegetation::empty("bridge",mesh.topology(),0.25);
    let upper=mesh.ray_surface(Mat4::IDENTITY,Vec3::new(0.,1.,0.),Vec3::NEG_Y,2.).unwrap();
    assert!((upper.position.y-0.05).abs()<1e-6);
    mesh.paint_vegetation(&mut data,Mat4::IDENTITY,&upper,"grass",1.,1.,0.,false).unwrap();
    assert!(mesh.resolved_vegetation(&data,Mat4::IDENTITY).unwrap().iter().all(|s|(s.position.y-0.05).abs()<1e-6));
    let lower=mesh.ray_surface(Mat4::IDENTITY,Vec3::new(0.,0.025,0.),Vec3::NEG_Y,1.).unwrap();
    mesh.paint_vegetation(&mut data,Mat4::IDENTITY,&lower,"shrub",1.,1.,0.,false).unwrap();
    assert!(mesh.resolved_vegetation(&data,Mat4::IDENTITY).unwrap().iter().any(|s|s.position.y.abs()<1e-6));
    mesh.paint_vegetation(&mut data,Mat4::IDENTITY,&upper,"grass",1.,1.,0.,true).unwrap();
    let resolved=mesh.resolved_vegetation(&data,Mat4::IDENTITY).unwrap();
    assert!(resolved.iter().filter(|s|s.position.y.abs()<1e-6).all(|s|s.layers==vec![("shrub".into(),255)] || s.layers[0].0=="shrub"));
    data.validate().unwrap();
}

#[test]
fn off_grid_small_brush_anchor_is_erased_by_a_larger_brush_on_its_own_floor() {
    let mesh=MeshSurfaceSnapshot::build(floors()).unwrap();
    let mut data=ModelSurfaceVegetation::empty("bridge",mesh.topology(),0.25);
    let upper=mesh.ray_surface(Mat4::IDENTITY,Vec3::new(0.,1.,0.),Vec3::NEG_Y,2.).unwrap();
    // 笔刷小于规则密度格，必须保存命中点锚点；以后换大橡皮仍须找到它。
    mesh.paint_vegetation(&mut data,Mat4::IDENTITY,&upper,"grass",0.001,1.,0.,false).unwrap();
    assert_eq!(data.samples.len(),1);
    let lower=mesh.ray_surface(Mat4::IDENTITY,Vec3::new(0.,0.025,0.),Vec3::NEG_Y,1.).unwrap();
    mesh.paint_vegetation(&mut data,Mat4::IDENTITY,&lower,"grass",0.001,1.,0.,false).unwrap();
    assert_eq!(data.samples.len(),2);
    assert!(mesh.paint_vegetation(&mut data,Mat4::IDENTITY,&upper,"grass",1.,1.,0.,true).unwrap());
    let resolved=mesh.resolved_vegetation(&data,Mat4::IDENTITY).unwrap();
    assert_eq!(resolved.len(),1,"大橡皮须移除离格上层锚点，同时保留下层同图层");
    assert!(resolved[0].position.y.abs()<1e-6);
    assert_eq!(resolved[0].layers,vec![("grass".into(),255)]);
}

#[test]
fn off_grid_anchors_after_same_topology_vertex_edit_remain_erasable() {
    let mesh=MeshSurfaceSnapshot::build(floors()).unwrap();
    let hit=mesh.ray_surface(Mat4::IDENTITY,Vec3::new(0.,1.,0.),Vec3::NEG_Y,2.).unwrap();
    let mut data=ModelSurfaceVegetation::empty("bridge",mesh.topology(),0.25);
    mesh.paint_vegetation(&mut data,Mat4::IDENTITY,&hit,"grass",0.6,1.,0.,false).unwrap();
    assert!(data.samples.len()>1,"夹具须生成规则格锚点");
    let anchor=data.samples[0].anchor;
    let previous=data.samples[0].layers[0].1;
    let mut changed=floors();
    Arc::make_mut(&mut changed[0].positions).iter_mut().for_each(|point| {point[0]*=2.;point[2]*=2.;});
    let changed=MeshSurfaceSnapshot::build(changed).unwrap();
    assert_eq!(changed.topology(),mesh.topology());
    assert_ne!(changed.geometry_revision(),mesh.geometry_revision());
    // 三角边加长后规则格分段数改变；旧锚点继续跟随源顶点，不能因此失去擦除资格。
    let position=changed.resolve(anchor,Mat4::IDENTITY).unwrap().position;
    let eraser=changed.ray_surface(Mat4::IDENTITY,position+Vec3::Y,Vec3::NEG_Y,2.).unwrap();
    assert!(changed.paint_vegetation(&mut data,Mat4::IDENTITY,&eraser,"grass",1.,1.,0.,true).unwrap());
    assert!(data.samples.iter().find(|sample|sample.anchor==anchor)
        .is_none_or(|sample|sample.layers[0].1<previous),"同拓扑变形后的离格旧锚点须响应擦除");
    data.validate().unwrap();
}

#[test]
fn anchors_follow_nonuniform_transform_and_same_topology_vertex_edits() {
    let mesh=MeshSurfaceSnapshot::build(floors()).unwrap();
    let hit=mesh.ray_surface(Mat4::IDENTITY,Vec3::new(0.,1.,0.),Vec3::NEG_Y,2.).unwrap();
    let mut data=ModelSurfaceVegetation::empty("bridge",mesh.topology(),0.25);
    mesh.paint_vegetation(&mut data,Mat4::IDENTITY,&hit,"grass",0.1,1.,0.,false).unwrap();
    let frame=Mat4::from_scale_rotation_translation(Vec3::new(2.,3.,0.5),Quat::from_rotation_z(0.4),Vec3::new(5.,7.,9.));
    let before=mesh.resolved_vegetation(&data,Mat4::IDENTITY).unwrap();
    let after=mesh.resolved_vegetation(&data,frame).unwrap();
    for (a,b) in before.iter().zip(&after) {assert!(b.position.distance(frame.transform_point3(a.position))<1e-5);}
    let mut changed=floors();
    Arc::make_mut(&mut changed[0].positions).iter_mut().for_each(|p|p[1]+=0.2);
    let changed=MeshSurfaceSnapshot::build(changed).unwrap();
    assert_eq!(mesh.topology(),changed.topology());
    assert_ne!(mesh.geometry_revision(),changed.geometry_revision(),"同拓扑改顶点也必须使旧命中失效");
    let updated=changed.resolved_vegetation(&data,Mat4::IDENTITY).unwrap();
    assert!((updated[0].position.y-before[0].position.y-0.2).abs()<1e-5);
    let mut topology=floors();Arc::make_mut(&mut topology[0].indices).swap(0,1);
    let topology=MeshSurfaceSnapshot::build(topology).unwrap();
    let old=data.clone();
    assert!(topology.resolved_vegetation(&data,Mat4::IDENTITY).is_err());
    assert!(topology.paint_vegetation(&mut data,Mat4::IDENTITY,&hit,"grass",1.,1.,0.,false).is_err());
    assert_eq!(old,data,"拓扑失效不能吞掉作者数据");
}

#[test]
fn common_model_brush_refuses_sibling_targets_and_old_geometry_without_losing_author_data() {
    let mesh=MeshSurfaceSnapshot::build(floors()).unwrap();
    let target=SurfaceTarget::Model {object_id:"bridge".into()};
    let root=SceneInstancePath::root(feathrise_scene_content::SceneId::parse("MAP-surface").unwrap());
    let path=root.child(feathrise_scene_content::SceneInstanceId::parse("left").unwrap());
    let source=PaintSurfaceSnapshot::from_model(target.clone(),path.clone(),mesh.clone(),Mat4::IDENTITY).unwrap();
    let hit=source.ray_surface(Vec3::new(0.,1.,0.),Vec3::NEG_Y,2.).unwrap();
    assert_eq!(hit.target,target);assert_eq!(hit.instance_path,path);
    assert_eq!(hit.geometry_revision,mesh.geometry_revision());assert!(hit.model_anchor.is_some());
    let mut data=ModelSurfaceVegetation::empty("bridge",mesh.topology(),0.25);
    source.paint_model_vegetation(&mut data,&hit,"grass",0.5,1.,0.,false).unwrap();
    let before=data.clone();
    let sibling=PaintSurfaceSnapshot::from_model(target.clone(),
        root.child(feathrise_scene_content::SceneInstanceId::parse("right").unwrap()),mesh.clone(),Mat4::IDENTITY).unwrap();
    assert!(sibling.paint_model_vegetation(&mut data,&hit,"grass",0.5,1.,0.,false).unwrap_err().contains("身份"));
    let other=PaintSurfaceSnapshot::from_model(SurfaceTarget::Model {object_id:"other".into()},path.clone(),mesh.clone(),Mat4::IDENTITY).unwrap();
    assert!(other.paint_model_vegetation(&mut data,&hit,"grass",0.5,1.,0.,false).is_err());
    let mut changed=floors();
    Arc::make_mut(&mut changed[0].positions).iter_mut().for_each(|p|p[1]+=0.2);
    let changed=MeshSurfaceSnapshot::build(changed).unwrap();
    assert_eq!(changed.topology(),mesh.topology());
    let changed=PaintSurfaceSnapshot::from_model(target,path,changed,Mat4::IDENTITY).unwrap();
    assert!(changed.paint_model_vegetation(&mut data,&hit,"grass",0.5,1.,0.,false).unwrap_err().contains("几何"));
    assert_eq!(data,before,"身份过期不能清空或覆盖作者样本");
    let fresh=changed.ray_surface(Vec3::new(0.,1.,0.),Vec3::NEG_Y,2.).unwrap();
    changed.paint_model_vegetation(&mut data,&fresh,"grass",0.5,1.,0.,false).unwrap();
}

#[test]
fn model_surface_uses_builtin_geometry_even_when_collision_is_off() {
    let mut record:ScenePlacementRecord=serde_json::from_value(serde_json::json!({"id":"box","asset_path":"",
        "translation":[0.,0.,0.],"yaw_degrees":0.,"scale":[1.,1.,1.]})).unwrap();
    record.asset_target=ScenePlacementAssetTarget::Cuboid {size_mm:[2000,2000,2000],color_srgb:[100;3],roughness_milli:700,collision:false};
    let mesh=load_model_surface(Path::new("not-read-for-builtins"),&record).unwrap();
    let hit=mesh.ray_surface(Mat4::IDENTITY,Vec3::new(0.,3.,0.),Vec3::NEG_Y,10.).unwrap();
    assert_eq!(hit.position.y,1.);
}

#[test]
fn clipmap_height_surface_returns_actual_face_normal_and_instance_local_coordinates() {
    let ground = crate::scenes::dynamic_scene::test_ground_snapshot();
    let owner = ground.scene_id.clone();
    let id = feathrise_scene_content::TerrainId::parse("small-height").unwrap();
    let local_origin = Vec3::new(47.25, 50., 31.25);
    let local = ground.ray_surface(local_origin, Vec3::NEG_Y, 100.).unwrap();
    assert!(local.1.y > 0.5 && local.1.is_normalized());
    assert!(ground.three_dimensional_hit(local_origin, Vec3::NEG_Y, 100.).is_none(),
        "夹具没有体积网格；命中必须来自二维高度面");
    let transform = Transform { translation: Vec3::new(5., 3., 7.),
        rotation: Quat::from_rotation_z(0.2), scale: Vec3::new(2., 3., 0.5) };
    let frame = feathrise_scene_core::residency::SceneSourceFrame::from_transform(transform);
    let path = SceneInstancePath::root(owner).child(feathrise_scene_content::SceneInstanceId::parse("left-bridge").unwrap()).terrain(id.clone());
    let snapshot = PaintSurfaceSnapshot { target: SurfaceTarget::Terrain { terrain_id: id },
        instance_path: path.clone(), geometry_revision: [9; 32],
        geometry: PaintSurfaceGeometry::Terrain(TerrainObjectRaySnapshot::Clipmap(ground, frame)) };
    let hit = snapshot.ray_surface(frame.to_world(local_origin),
        frame.source_to_world.transform_vector3(Vec3::NEG_Y).normalize(), 300.).unwrap();
    assert!(hit.local_position.distance(local.0) < 0.0001);
    assert!(hit.local_normal.distance(local.1) < 0.0001);
    assert_eq!(hit.instance_path, path);
    assert_eq!(hit.geometry_revision, [9; 32]);
    assert!(hit.model_anchor.is_none());
}

#[test]
fn glb_scene_traversal_keeps_node_transforms_and_primitive_targets_are_object_local() {
    let positions=[[-1.0_f32,0.,-1.],[1.,0.,1.],[1.,0.,-1.]];
    let mut bin=positions.into_iter().flatten().flat_map(f32::to_le_bytes).collect::<Vec<_>>();
    let document=serde_json::json!({"asset":{"version":"2.0"},"scene":0,
        "scenes":[{"nodes":[0]}],"nodes":[{"translation":[0.,2.,0.],"mesh":0,"children":[1]},
        {"translation":[4.,0.,0.],"mesh":0}],"meshes":[{"primitives":[{"attributes":{"POSITION":0}}]}],
        "buffers":[{"byteLength":bin.len()}],"bufferViews":[{"buffer":0,"byteLength":bin.len()}],
        "accessors":[{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}]});
    let mut json=serde_json::to_vec(&document).unwrap();while json.len()%4!=0{json.push(b' ');}while bin.len()%4!=0{bin.push(0);}
    let mut bytes=b"glTF".to_vec();bytes.extend(2u32.to_le_bytes());bytes.extend(((12+8+json.len()+8+bin.len())as u32).to_le_bytes());
    bytes.extend((json.len()as u32).to_le_bytes());bytes.extend(b"JSON");bytes.extend(json);bytes.extend((bin.len()as u32).to_le_bytes());bytes.extend(b"BIN\0");bytes.extend(bin);
    let mesh=model_source::build_glb_surface(&bytes,ScenePlacementAssetTarget::Scene0).unwrap();
    let hit=mesh.ray_surface(Mat4::IDENTITY,Vec3::new(4.5,5.,0.),Vec3::NEG_Y,10.).unwrap();
    assert_eq!(hit.anchor.primitive,1);assert_eq!(hit.position.y,2.);
    let direct=model_source::build_glb_surface(&bytes,ScenePlacementAssetTarget::Primitive {mesh:0,primitive:0,material:None,collision:false}).unwrap();
    assert_eq!(direct.ray_surface(Mat4::IDENTITY,Vec3::new(0.5,5.,0.),Vec3::NEG_Y,10.).unwrap().position.y,0.);
}
