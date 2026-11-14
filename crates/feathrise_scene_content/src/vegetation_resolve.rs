//! 二维密度页与稀疏模型样本共用的单点作者组解析；调用方在后台冻结完整结果。
use std::collections::BTreeMap;
use crate::{SceneVegetationAsset, SceneVegetationAuthorGroup, SceneVegetationAuthorGroupMode,
    SceneVegetationOutputMergeMode};

/// 密度之外的稳定层参数；同一组内数组顺序就是作者目录顺序。
#[derive(Clone, Copy)]
pub struct VegetationGroupLayer { pub priority: i32, pub weight: f32 }

/// 组规则唯一实现，二维编译、模型草木和检查器均使用同一水填充及稳定覆盖选择。
#[allow(clippy::too_many_arguments)]
pub fn resolve_vegetation_group(mode: SceneVegetationAuthorGroupMode, capacity: f32,
    threshold: f32, seed: u64, cell_meters: f32, layers: &[VegetationGroupLayer],
    demands: &[f32], output: &mut [f32], position: [f32; 2]) {
    assert_eq!(layers.len(), demands.len());
    assert_eq!(output.len(), demands.len());
    output.fill(0.);
    match mode {
        SceneVegetationAuthorGroupMode::Additive => output.copy_from_slice(demands),
        SceneVegetationAuthorGroupMode::WeightedPartition => {
            let mut remaining = capacity.clamp(0., 1.);
            let mut priorities: Vec<_> = layers.iter().map(|l| l.priority).collect();
            priorities.sort_unstable_by(|a,b| b.cmp(a)); priorities.dedup();
            for priority in priorities {
                if remaining <= f32::EPSILON { break; }
                let tier: Vec<_> = layers.iter().enumerate().filter_map(|(i,l)|
                    (l.priority == priority && demands[i] > 0.).then_some(i)).collect();
                let total: f32 = tier.iter().map(|&i| demands[i]).sum();
                if total <= remaining + f32::EPSILON {
                    for i in tier { output[i] = demands[i]; }
                    remaining = (remaining-total).max(0.); continue;
                }
                let mut active = tier.clone();
                for _ in 0..=tier.len() {
                    if active.is_empty() || remaining <= f32::EPSILON { break; }
                    let total_weight: f32 = active.iter().map(|&i| demands[i]*layers[i].weight).sum();
                    if total_weight <= f32::EPSILON { break; }
                    let mut consumed = 0.; let mut next = Vec::with_capacity(active.len());
                    for &i in &active {
                        let share = remaining*demands[i]*layers[i].weight/total_weight;
                        let assigned = share.min((demands[i]-output[i]).max(0.));
                        output[i] += assigned; consumed += assigned;
                        if output[i]+f32::EPSILON < demands[i] { next.push(i); }
                    }
                    remaining = (remaining-consumed).max(0.);
                    if consumed <= f32::EPSILON { break; }
                    active = next;
                }
                break; // 当前优先级已消费全部剩余容量，低优先级不得重新分配。
            }
        }
        SceneVegetationAuthorGroupMode::PriorityOverride => {
            let Some(priority) = layers.iter().enumerate().filter_map(|(i,l)|
                (demands[i] > threshold).then_some(l.priority)).max() else { return; };
            let candidates: Vec<_> = layers.iter().enumerate().filter_map(|(i,l)|
                (l.priority == priority && demands[i] > threshold).then_some(i)).collect();
            let total: f32 = candidates.iter().map(|&i| demands[i]*layers[i].weight).sum();
            let x = (position[0]/cell_meters).floor() as i64;
            let z = (position[1]/cell_meters).floor() as i64;
            let mut hash = seed ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (z as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            hash ^= hash >> 30; hash = hash.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            hash ^= hash >> 27; hash = hash.wrapping_mul(0x94D0_49BB_1331_11EB); hash ^= hash >> 31;
            let mut cursor = ((hash >> 40) as f32)/((1u32 << 24) as f32)*total;
            let mut winner = *candidates.last().expect("最高优先级保证非空");
            for &i in &candidates {
                let span = demands[i]*layers[i].weight;
                if cursor < span { winner = i; break; }
                cursor -= span;
            }
            output[winner] = demands[winner]; // 取得所有权仍保留原需求，不把小密度放大到一。
        }
    }
}

struct Layer { id: String, output: usize, opacity: f32, parameters: VegetationGroupLayer }
struct Group { definition: SceneVegetationAuthorGroup, layers: Vec<Layer> }

/// 构建一次的后台样本编译器；模型样本保存作者层 ID，运行结果仅引用正式输出下标。
pub struct SurfaceVegetationCompiler { groups: Vec<Group>, outputs: BTreeMap<String, usize>, count: usize }
impl SurfaceVegetationCompiler {
    pub fn new(definition: &SceneVegetationAsset) -> Result<Self, String> {
        let outputs: BTreeMap<_,_> = definition.layers.iter().enumerate().map(|(i,l)| (l.id.clone(),i)).collect();
        if outputs.len() != definition.layers.len() { return Err("植被正式输出 ID 重复".into()); }
        let mut groups = Vec::new();
        if let Some(authoring) = &definition.authoring {
            authoring.validate(&definition.layers)?;
            for group in authoring.groups.iter().filter(|g| g.enabled) {
                let layers = authoring.layers.iter().filter(|l| l.enabled && l.group_id == group.id).map(|l| Layer {
                    id:l.id.clone(), output:outputs[&l.output_layer_id], opacity:l.opacity,
                    parameters:VegetationGroupLayer {priority:l.priority,weight:l.weight},
                }).collect();
                groups.push(Group {definition:group.clone(),layers});
            }
        }
        Ok(Self {groups,outputs,count:definition.layers.len()})
    }
    pub fn resolve(&self, source: &[(String,u8)], position: [f32;2]) -> Vec<(usize,u8)> {
        let mut result = vec![0u8;self.count];
        if self.groups.is_empty() {
            // 未含作者图的旧源只允许已存在的正式 ID；坏作者引用不能制造额外输出。
            for (id,value) in source { if let Some(&i) = self.outputs.get(id) { result[i] = *value; } }
        }
        for group in &self.groups {
            let parameters: Vec<_> = group.layers.iter().map(|l| l.parameters).collect();
            let demands: Vec<_> = group.layers.iter().map(|l| source.iter().find(|(id,_)| id == &l.id)
                .map_or(0.,|(_,p)| f32::from(*p)/255.*l.opacity).clamp(0.,1.)).collect();
            let mut resolved = vec![0.;demands.len()];
            let g = &group.definition;
            resolve_vegetation_group(g.mode,g.capacity,g.claim_threshold,g.selection_seed,
                g.selection_cell_meters,&parameters,&demands,&mut resolved,position);
            let mut values = vec![0u8;self.count];
            for (layer,value) in group.layers.iter().zip(resolved) {
                values[layer.output] = values[layer.output].saturating_add((value.clamp(0.,1.)*255.).round() as u8);
            }
            for (destination,value) in result.iter_mut().zip(values) {
                *destination = match g.output_merge {
                    SceneVegetationOutputMergeMode::Maximum => (*destination).max(value),
                    SceneVegetationOutputMergeMode::AddClamped => destination.saturating_add(value),
                    SceneVegetationOutputMergeMode::ReplaceNonZero if value > 0 => value,
                    SceneVegetationOutputMergeMode::ReplaceNonZero => *destination,
                };
            }
        }
        result.into_iter().enumerate().filter(|(_,p)| *p > 0).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weighted_tiers_do_not_amplify_demand_and_priority_claims_are_stable() {
        // 小需求拥有更高权重，初次分配超过自身需求后应把余额交还同优先级的大需求。
        let layers = [VegetationGroupLayer{priority:3,weight:4.},VegetationGroupLayer{priority:3,weight:1.},VegetationGroupLayer{priority:0,weight:1.}];
        let mut out=[0.;3];
        resolve_vegetation_group(SceneVegetationAuthorGroupMode::WeightedPartition,0.6,0.,7,8.,&layers,&[0.1,0.8,1.],&mut out,[-2.,3.]);
        assert!((out[0]-0.1).abs()<1e-6); assert!((out[1]-0.5).abs()<1e-6); assert_eq!(out[2],0.);
        resolve_vegetation_group(SceneVegetationAuthorGroupMode::PriorityOverride,1.,0.,7,8.,&layers,&[0.1,0.8,1.],&mut out,[-2.,3.]);
        let first=out;
        resolve_vegetation_group(SceneVegetationAuthorGroupMode::PriorityOverride,1.,0.,7,8.,&layers,&[0.1,0.8,1.],&mut out,[-2.,3.]);
        assert_eq!(out,first); assert_eq!(out.iter().filter(|p| **p>0.).count(),1); assert_eq!(out[2],0.);
    }
}
