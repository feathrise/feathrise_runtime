//! 命中连通层的确定性稀疏密度笔刷；仅在计算 worker 中编译完整结果后交给调用方原子采用。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use bevy::prelude::*;
use feathrise_scene_content::{ModelSurfaceAnchor, ModelSurfaceVegetation, ModelSurfaceVegetationSample};
use super::{MeshSurfaceSnapshot, PaintSurfaceGeometry, PaintSurfaceHit, PaintSurfaceSnapshot, SurfaceHit,
    geometry::{MAX_BRUSH_CANDIDATES, valid_matrix}};

#[derive(Clone, Debug)]
pub struct ResolvedSurfaceVegetation {
    pub anchor: ModelSurfaceAnchor,
    pub position: Vec3,
    pub normal: Vec3,
    pub layers: Vec<(String, u8)>,
    /// 当前变换下每个确定性样本代表的表面积；运行时可按原密度生成多个草根。
    pub cell_area: f32,
}

type AnchorKey = (u32, u32, u32, u32);
fn key(anchor: ModelSurfaceAnchor) -> AnchorKey {
    (anchor.primitive, anchor.triangle, anchor.barycentric[0].to_bits(), anchor.barycentric[1].to_bits())
}

impl PaintSurfaceSnapshot {
    /// 公共命中只可写回同一目标、完整实例和几何代次；底层模型适配独占连通层采样规则。
    #[allow(clippy::too_many_arguments)]
    pub fn paint_model_vegetation(&self, data: &mut ModelSurfaceVegetation, hit: &PaintSurfaceHit,
        layer_id: &str, radius: f32, strength: f32, flow_exposure: f32, erase: bool,
    ) -> Result<bool, String> {
        if data.target != self.target || hit.target != self.target || hit.instance_path != self.instance_path {
            return Err("模型种植目标或实例身份已换代，未写入作者数据".into());
        }
        if hit.geometry_revision != self.geometry_revision {
            return Err("模型几何已换代，等待重新命中；作者数据已保留".into());
        }
        let PaintSurfaceGeometry::Model { mesh, object_to_world } = &self.geometry else {
            return Err("当前表面不使用模型稀疏密度".into());
        };
        let anchor = hit.model_anchor.ok_or("模型命中缺少表面锚点")?;
        let hit = SurfaceHit { anchor, local_position: hit.local_position, local_normal: hit.local_normal,
            position: hit.position, normal: hit.normal, distance: 0.0 };
        mesh.paint_vegetation(data, *object_to_world, &hit, layer_id, radius, strength, flow_exposure, erase)
    }
}

impl MeshSurfaceSnapshot {
    /// 完整返回前不修改作者数据；预算或锚点失败时上一版本与撤销历史均保持可用。
    #[allow(clippy::too_many_arguments)]
    pub fn paint_vegetation(&self, data: &mut ModelSurfaceVegetation, object_to_world: Mat4,
        hit: &SurfaceHit, layer_id: &str, radius: f32, strength: f32, flow_exposure: f32, erase: bool,
    ) -> Result<bool, String> {
        data.validate()?;
        if data.topology != self.topology() { return Err("模型拓扑已改变，植被数据已保留，请重新绑定".into()); }
        if !valid_matrix(object_to_world) || !radius.is_finite() || radius <= 0.0
            || !strength.is_finite() || !(0.0..=1.0).contains(&strength)
            || !flow_exposure.is_finite() || flow_exposure < 0.0
            || layer_id.is_empty() || layer_id.len() > 256 || layer_id.chars().any(char::is_control) {
            return Err("模型植被笔刷参数无效".into());
        }
        let fresh_hit = self.resolve(hit.anchor, object_to_world).ok_or("模型命中面已失效")?;
        // 冻结的命中必须来自同一对象姿态，不能把旧鼠标位置应用到后来移动的对象上。
        if fresh_hit.position.distance_squared(hit.position) > 1e-8 || fresh_hit.normal.dot(hit.normal) < 0.999 {
            return Err("模型命中姿态已换代，等待重新命中".into());
        }
        let connected = self.connected_triangles(hit, object_to_world, radius)?;
        let connected_keys: BTreeSet<_> = connected.iter().map(|&index| self.0.triangles[index].anchor).collect();
        let old: HashMap<_, _> = data.samples.iter().enumerate().map(|(i,s)| (key(s.anchor), i)).collect();
        let mut updates = BTreeMap::<AnchorKey, ModelSurfaceVegetationSample>::new();
        let mut visited = 0usize;
        for index in connected {
            let triangle = &self.0.triangles[index];
            let points = triangle.points.map(|p| object_to_world.transform_point3(p));
            let e = points[1]-points[0]; let f = points[2]-points[0];
            let ee=e.dot(e); let ff=f.dot(f); let ef=e.dot(f); let denominator=ee*ff-ef*ef;
            if denominator <= 0.0 || !denominator.is_finite() { continue; }
            let gradient_u=(e*ff-f*ef)/denominator; let gradient_v=(f*ee-e*ef)/denominator;
            let offset=hit.position-points[0];
            let u=gradient_u.dot(offset); let v=gradient_v.dot(offset);
            let longest=(triangle.points[1]-triangle.points[0]).length()
                .max((triangle.points[2]-triangle.points[0]).length())
                .max((triangle.points[2]-triangle.points[1]).length());
            let divisions=(longest/data.spacing_meters).ceil().max(1.0);
            if !divisions.is_finite() || divisions > u32::MAX as f32 { return Err("模型密度格距超出安全采样范围".into()); }
            let n=divisions as u32;
            let interval = |center: f32, spread: f32| {
                let low=((center-spread)*divisions-1.0/3.0).floor().max(0.0).min((n-1) as f32) as u32;
                let high=((center+spread)*divisions-1.0/3.0).ceil().max(0.0).min((n-1) as f32) as u32;
                (low,high)
            };
            let (i0,i1)=interval(u,radius*gradient_u.length());
            let (j0,j1)=interval(v,radius*gradient_v.length());
            if u64::from(i1-i0+1)*u64::from(j1-j0+1) > MAX_BRUSH_CANDIDATES as u64 {
                return Err("模型笔刷采样超过局部后台预算".into());
            }
            for j in j0..=j1 {
                for i in i0..=i1.min(n-1-j.min(n-1)) {
                    visited+=1;
                    if visited > MAX_BRUSH_CANDIDATES { return Err("模型笔刷采样超过局部后台预算".into()); }
                    let barycentric=[(i as f32+1.0/3.0)/divisions,(j as f32+1.0/3.0)/divisions];
                    let position=points[0]*(1.0-barycentric[0]-barycentric[1])+points[1]*barycentric[0]+points[2]*barycentric[1];
                    let distance=position.distance(hit.position);
                    if distance >= radius { continue; }
                    let anchor=ModelSurfaceAnchor { primitive:triangle.anchor.0, triangle:triangle.anchor.1,barycentric };
                    prepare_update(&mut updates, &old, &data.samples, anchor, layer_id,
                        response(strength,flow_exposure,1.0-distance/radius),erase);
                }
            }
        }
        // 小于来源格距的笔刷仍能种到真实命中点；重心坐标量化后同一点连续刷印不会积累重复锚点。
        if updates.is_empty() {
            let mut anchor=hit.anchor;
            anchor.barycentric=anchor.barycentric.map(|v| (v*65535.0).round()/65535.0);
            if anchor.barycentric[0]+anchor.barycentric[1]>1.0 { anchor.barycentric[1]=1.0-anchor.barycentric[0]; }
            prepare_update(&mut updates,&old,&data.samples,anchor,layer_id,response(strength,flow_exposure,1.0),erase);
        }
        // 小笔刷命中点及同拓扑变形后的旧锚点可能偏离当前规则格；它们仍属于作者表面。
        // 只在后台将同一连通面、世界半径内的既有样本并入完整更新集，不能跨桥层补刷。
        // 更新始终以原作者值计算，同键规则格不会叠加曝光，超预算也不会部分写入。
        for sample in &data.samples {
            if !connected_keys.contains(&(sample.anchor.primitive,sample.anchor.triangle)) { continue; }
            let position=self.resolve(sample.anchor,object_to_world)
                .ok_or("模型植被锚点已失效，原数据已保留")?.position;
            let distance=position.distance(hit.position);
            if distance >= radius { continue; }
            visited+=1;
            if visited > MAX_BRUSH_CANDIDATES { return Err("模型笔刷采样超过局部后台预算".into()); }
            prepare_update(&mut updates,&old,&data.samples,sample.anchor,layer_id,
                response(strength,flow_exposure,1.0-distance/radius),erase);
        }
        let added=updates.iter().filter(|(key,s)| !old.contains_key(key) && !s.layers.is_empty()).count();
        let estimated=(data.samples.len()+added).saturating_mul(64)
            .saturating_add(data.samples.iter().map(|s| s.layers.iter().map(|(id,_)| id.len()+8).sum::<usize>()).sum::<usize>());
        if estimated > 64*1024*1024 { return Err("模型植被作者密度超过64MiB后台工作预算".into()); }
        let mut changed=false;
        for (key,sample) in updates {
            if let Some(&index)=old.get(&key) {
                if data.samples[index]!=sample { data.samples[index]=sample; changed=true; }
            } else if !sample.layers.is_empty() { data.samples.push(sample); changed=true; }
        }
        if changed { data.samples.retain(|s| !s.layers.is_empty()); }
        Ok(changed)
    }

    pub fn resolved_vegetation(&self, data: &ModelSurfaceVegetation, object_to_world: Mat4)
        -> Result<Vec<ResolvedSurfaceVegetation>, String> {
        data.validate()?;
        if data.topology!=self.topology() { return Err("模型拓扑已改变，植被数据已保留，请重新绑定".into()); }
        if !valid_matrix(object_to_world) { return Err("模型植被对象变换无效".into()); }
        let mut result=Vec::with_capacity(data.samples.len());
        for sample in &data.samples {
            let hit=self.resolve(sample.anchor,object_to_world).ok_or("模型植被锚点已失效，原数据已保留")?;
            let triangle=self.triangle(sample.anchor).ok_or("模型植被三角面已失效")?;
            let edge=(triangle.points[1]-triangle.points[0]).length()
                .max((triangle.points[2]-triangle.points[0]).length())
                .max((triangle.points[2]-triangle.points[1]).length());
            let n=(edge/data.spacing_meters).ceil().max(1.0);
            let [a,b,c]=triangle.points.map(|p| object_to_world.transform_point3(p));
            let cell_area=(b-a).cross(c-a).length()/(n*(n+1.0));
            result.push(ResolvedSurfaceVegetation {anchor:sample.anchor,position:hit.position,normal:hit.normal,
                layers:sample.layers.clone(),cell_area});
        }
        Ok(result)
    }
}

fn response(strength:f32,exposure:f32,falloff:f32)->f32 {
    let stamp=(strength*falloff).clamp(0.0,1.0);
    stamp+(1.0-stamp)*-(-exposure*falloff).exp_m1()
}
fn prepare_update(updates:&mut BTreeMap<AnchorKey,ModelSurfaceVegetationSample>,old:&HashMap<AnchorKey,usize>,
    source:&[ModelSurfaceVegetationSample],anchor:ModelSurfaceAnchor,id:&str,amount:f32,erase:bool) {
    let key=key(anchor);
    let mut sample=old.get(&key).map(|&i|source[i].clone()).unwrap_or(ModelSurfaceVegetationSample {anchor,layers:Vec::new()});
    let previous=sample.layers.iter().find(|(layer,_)|layer==id).map_or(0.0,|(_,w)|f32::from(*w)/255.0);
    let next=if erase {previous*(1.0-amount)}else{previous+(1.0-previous)*amount};
    let density=(next.clamp(0.0,1.0)*255.0).round() as u8;
    sample.layers.retain(|(layer,_)|layer!=id);
    if density>0 {sample.layers.push((id.to_owned(),density));sample.layers.sort_by(|a,b|a.0.cmp(&b.0));}
    updates.insert(key,sample);
}
