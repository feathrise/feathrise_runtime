//! 真实正式Cuboid网格顶面边的取证；最多读取128个根、64个子节点和64个顶点，像素工作只在后台做。
use bevy::{mesh::VertexAttributeValues, prelude::*};
use feathrise_scene_content::ScenePlacementAssetTarget;
use feathrise_scene::{ScenePlacementInstance, ScenePlacementPrimitiveVisual, author_view::ShadingView};
use serde_json::{Value, json};

pub(in crate::project::embedded) fn project(world: &mut World) -> Option<Value> {
    let camera_entity = world.query_filtered::<Entity,With<feathrise_scene_core::view::PrimarySceneView>>()
        .single(world).ok()?;
    if !world.get::<ShadingView>(camera_entity).is_some_and(|view| view.options.x == 0.) { return None; }
    Some(project_cube(world,camera_entity).unwrap_or_else(|error|json!({
        "source":"projected-real-mesh-edges","error":error,"matched_edges":0,"required_edges":4})))
}

fn project_cube(world: &mut World, camera_entity: Entity) -> Result<Value,String> {
    let mut query = world.query::<(Entity,&ScenePlacementInstance)>();
    let mut roots = Vec::new();
    for (index,(entity,instance)) in query.iter(world).enumerate().take(129) {
        if index == 128 { return Err("线框取证超过128个正式根预算".into()); }
        if instance.is_active() && matches!(instance.asset_target(),ScenePlacementAssetTarget::Cuboid {..}) {
            roots.push((entity,instance.id().to_owned()));
            if roots.len() > 1 { return Err("线框夹具需要唯一活动正式Cuboid".into()); }
        }
    }
    let (root,object_id) = roots.pop().ok_or("线框夹具没有活动正式Cuboid")?;
    let children = world.get::<Children>(root).ok_or("正式Cuboid尚无可视子节点")?;
    if children.len() > 64 { return Err("正式Cuboid子节点超过取证预算".into()); }
    let mut visuals = children.iter().filter(|child| world.get::<ScenePlacementPrimitiveVisual>(*child).is_some());
    let entity = visuals.next().ok_or("正式Cuboid可视网格尚未建立")?;
    if visuals.next().is_some() { return Err("正式Cuboid存在多个可视网格".into()); }
    if !world.get::<InheritedVisibility>(entity).is_some_and(|visibility| visibility.get()) {
        return Err("正式Cuboid可视网格当前隐藏".into());
    }
    let handle = world.get::<Mesh3d>(entity).ok_or("正式Cuboid缺Mesh3d")?;
    let mesh = world.resource::<Assets<Mesh>>().get(&handle.0).ok_or("正式Cuboid CPU网格尚未就绪")?;
    if mesh.attribute(feathrise_scene::ATTRIBUTE_AUTHORING_WIRE).is_none() {
        return Err("正式Cuboid缺真实面边布局标记".into());
    }
    let Some(VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return Err("正式Cuboid缺Float32x3顶点源".into());
    };
    if positions.len() > 64 { return Err("正式Cuboid顶点超过取证预算".into()); }
    let corners = top_corners(positions)?;
    let transform = world.get::<GlobalTransform>(entity).ok_or("正式Cuboid缺实际世界变换")?;
    let camera = world.get::<Camera>(camera_entity).ok_or("正式相机尚未建立")?;
    let camera_transform = world.get::<GlobalTransform>(camera_entity).ok_or("正式相机缺世界变换")?;
    let scale = camera.target_scaling_factor().ok_or("正式相机DPI尚未建立")?;
    let mut edges = Vec::with_capacity(4);
    for edge in 0..4 {
        let a = transform.transform_point(corners[edge]);
        let b = transform.transform_point(corners[(edge+1)%4]);
        let mut samples = Vec::with_capacity(7);
        for sample in 1..=7 {
            let point = camera.world_to_viewport(camera_transform,a.lerp(b,sample as f32/8.))
                .map_err(|error|format!("正式顶面边无法投影：{error}"))? * scale;
            if !point.is_finite() { return Err("正式顶面边投影不是有限物理坐标".into()); }
            samples.push(json!({"position":point.to_array()}));
        }
        edges.push(json!({"world_endpoints":[a.to_array(),b.to_array()],"samples":samples}));
    }
    Ok(json!({"source":"projected-real-mesh-edges","object_id":object_id,"mesh_entity":entity.to_bits(),
        "camera_entity":camera_entity.to_bits(),"target_scale":scale,"edges":edges,
        "required_edges":4,"samples_per_edge":7,"required_samples":5,"radius":2,"maximum_rgb":28}))
}

fn top_corners(positions: &[[f32;3]]) -> Result<[Vec3;4],String> {
    if positions.is_empty() { return Err("正式Cuboid顶点源为空".into()); }
    let mut minimum = Vec3::splat(f32::INFINITY);
    let mut maximum = Vec3::splat(f32::NEG_INFINITY);
    for position in positions {
        let position = Vec3::from_array(*position);
        if !position.is_finite() { return Err("正式Cuboid顶点不是有限值".into()); }
        minimum = minimum.min(position); maximum = maximum.max(position);
    }
    let corners = [Vec3::new(minimum.x,maximum.y,minimum.z),Vec3::new(maximum.x,maximum.y,minimum.z),
        Vec3::new(maximum.x,maximum.y,maximum.z),Vec3::new(minimum.x,maximum.y,maximum.z)];
    if !corners.iter().all(|corner| positions.iter().any(|position| Vec3::from_array(*position) == *corner)) {
        return Err("正式Cuboid实际顶点不含四个顶面角，不能以参数合成代替真实网格".into());
    }
    Ok(corners)
}

pub(in crate::project::embedded) fn inspect(
    mut geometry: Value, width: u32, height: u32, pixel: impl Fn(u32,u32)->[u8;4],
) -> Value {
    geometry["width"] = json!(width); geometry["height"] = json!(height);
    let mut matched_edges = 0;
    if let Some(edges) = geometry["edges"].as_array_mut() {
        for edge in edges.iter_mut().take(4) {
            let mut matched = 0;
            if let Some(samples) = edge["samples"].as_array_mut() {
                for sample in samples.iter_mut().take(7) {
                    let position = sample["position"].as_array().and_then(|point| {
                        if point.len() != 2 { return None; }
                        Some([point[0].as_f64()?,point[1].as_f64()?])
                    });
                    let Some([x,y]) = position else { continue; };
                    let inside = x.is_finite() && y.is_finite() && x >= 0. && y >= 0. && x < f64::from(width) && y < f64::from(height);
                    let [x,y] = [x.round() as i32,y.round() as i32];
                    let inside = inside && x >= 0 && y >= 0 && x < width as i32 && y < height as i32;
                    let center = inside.then(||pixel(x as u32,y as u32));
                    let mut nearest = [255_u8;4];
                    for dy in -2..=2 { for dx in -2..=2 {
                        let [sx,sy] = [x+dx,y+dy];
                        if sx >= 0 && sy >= 0 && sx < width as i32 && sy < height as i32 {
                            let candidate = pixel(sx as u32,sy as u32);
                            if candidate[..3].iter().max() < nearest[..3].iter().max() { nearest = candidate; }
                        }
                    }}
                    let hit = inside && nearest[..3].iter().all(|channel| *channel <= 28);
                    sample["inside"] = json!(inside); sample["rgba"] = json!(center);
                    sample["nearest_rgba"] = json!(nearest); sample["matched"] = json!(hit);
                    matched += usize::from(hit);
                }
            }
            edge["matched"] = json!(matched); matched_edges += usize::from(matched >= 5);
        }
    }
    geometry["matched_edges"] = json!(matched_edges);
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_cuboid_vertices_and_projected_pixel_edges_are_both_required() {
        let positions = [[-1.,-1.,-1.],[1.,-1.,-1.],[-1.,1.,-1.],[1.,1.,-1.],
            [-1.,-1.,1.],[1.,-1.,1.],[-1.,1.,1.],[1.,1.,1.]];
        assert!(top_corners(&positions).is_ok());
        assert!(top_corners(&positions[..7]).is_err());
        let points = (1..=7).map(|i| json!({"position":[i*4,16]})).collect::<Vec<_>>();
        let source = json!({"edges":(0..4).map(|_|json!({"samples":points})).collect::<Vec<_>>()});
        let clear = inspect(source.clone(),32,32,|_,_|[70,70,70,255]);
        assert_eq!(clear["matched_edges"],0,"网格背景或图片hash不得冒充真实边");
        let lines = inspect(source.clone(),32,32,|_,y| if y==16 {[10,10,10,255]} else {[70,70,70,255]});
        assert_eq!(lines["matched_edges"],4);
        assert_eq!(lines["edges"][0]["matched"],7);
        let clipped = inspect(source,8,8,|_,_|[0,0,0,255]);
        assert_eq!(clipped["matched_edges"],0,"画面外投影不得借近邻黑像素通过");
    }
}
