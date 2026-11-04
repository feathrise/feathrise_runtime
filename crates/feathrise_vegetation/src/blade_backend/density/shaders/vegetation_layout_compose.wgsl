// 正式布局输入只在加载或换版时执行的基础级GPU竞争合成。

const MODE_ADDITIVE: u32 = 0u; // 与Rust稳定模式ABI一致。
const MODE_WEIGHTED: u32 = 1u; // 与Rust稳定模式ABI一致。
const MODE_PRIORITY: u32 = 2u; // 与Rust稳定模式ABI一致。
const DENSITY_EPSILON: f32 = 0.000001; // 与CPU封顶水填充终止阈值一致。
const TILE_SIDE: u32 = 64u; // 每个Z切片固定覆盖一个公共脏块。

struct CompetitionParams {
    atlas_size: vec2<u32>, // 候选基础级真实宽高。
    layer_count: u32, // 当前竞争组稳定层数。
    target_layer_index: u32, // 当前串行候选要写出的层索引。
    mode: u32, // Additive、Weighted或Priority。
    tile_count: u32, // 有效提交块数量。
    mode_value: f32, // Weighted容量或Priority声明阈值。
    padding: u32, // 补齐三十二字节uniform布局。
}

@group(0) @binding(0) var<storage, read> layers: array<vec4<u32>>; // 优先级位、权重位和两个保留标量。
@group(0) @binding(1) var<storage, read> tiles: array<vec4<u32>>; // 输出原点、上下文起点和保留标志。
@group(0) @binding(2) var<storage, read> samples: array<vec2<f32>>; // 实际密度与所有权密度。
@group(0) @binding(3) var<storage, read> contexts: array<vec2<f32>>; // 世界选择值与足迹影响。
@group(0) @binding(4) var output_base: texture_storage_2d<rgba8unorm, write>; // 只写候选Mip0。
@group(0) @binding(5) var<uniform> params: CompetitionParams; // 当前目标层和模式参数。

fn layer_priority(index: u32) -> i32 {
    return bitcast<i32>(layers[index].x); // CPU二补码位无损恢复为WGSL有符号优先级。
}

fn layer_weight(index: u32) -> f32 {
    return bitcast<f32>(layers[index].y); // CPU受检正权重无损恢复为f32。
}

fn sample_at(texel_base: u32, layer_index: u32) -> vec2<f32> {
    return samples[texel_base * params.layer_count + layer_index]; // 纹素内按稳定层ID紧密排列。
}

fn additive_density(texel_base: u32) -> f32 {
    return clamp(sample_at(texel_base, params.target_layer_index).x, 0.0, 1.0); // 叠加模式保持目标层自身需求。
}

fn weighted_density(texel_base: u32) -> f32 {
    let target_index = params.target_layer_index; // 目标索引在整个函数中保持不变。
    let target_priority = layer_priority(target_index); // 目标所属优先级决定可消费剩余容量。
    let target_density = sample_at(texel_base, target_index).x; // 目标需求是最终封顶上限。
    var remaining = params.mode_value; // 高优先级先从组容量中消费。
    var priority_ceiling = 2147483647; // 第一次查找从最大i32以下开始。
    loop {
        if remaining <= DENSITY_EPSILON {
            return 0.0; // CPU在容量耗尽后保持所有低层显式零份额。
        }
        var found = false; // 当前轮是否存在尚未处理且高于目标的优先级。
        var next_priority = -2147483647 - 1; // i32最小值作为无候选占位。
        for (var index = 0u; index < params.layer_count; index = index + 1u) {
            let priority = layer_priority(index); // 按稳定层顺序扫描全部描述。
            if priority > target_priority && priority < priority_ceiling && (!found || priority > next_priority) {
                next_priority = priority; // 选择尚未处理的最高优先级层级。
                found = true; // 当前轮稍后按CPU层序累计该层级需求。
            }
        }
        if !found {
            break; // 所有高于目标的层级都已按数值降序消费容量。
        }
        var tier_demand = 0.0; // 当前高层级的总实际需求。
        for (var index = 0u; index < params.layer_count; index = index + 1u) {
            if layer_priority(index) == next_priority {
                tier_demand = tier_demand + sample_at(texel_base, index).x; // 与CPU稳定层顺序相加。
            }
        }
        let tier_capacity = min(remaining, tier_demand); // 不足需求不得被权重自动放大。
        remaining = max(remaining - tier_capacity, 0.0); // 把未用容量传给下一优先级。
        priority_ceiling = next_priority; // 下一轮只查找更低但仍高于目标的层级。
    }
    if remaining <= DENSITY_EPSILON {
        return 0.0; // 高层消费后数值极小余量按CPU终止阈值归零。
    }
    var target_tier_demand = 0.0; // 目标同级总需求决定是否需要加权分区。
    var active_mask = 0u; // 最多三十二层由一个u32记录尚未封顶项。
    for (var index = 0u; index < params.layer_count; index = index + 1u) {
        if layer_priority(index) == target_priority {
            target_tier_demand = target_tier_demand + sample_at(texel_base, index).x; // 稳定层序累计同级需求。
            active_mask = active_mask | (1u << index); // 同级层进入封顶水填充。
        }
    }
    let target_tier_capacity = min(remaining, target_tier_demand); // 当前同级最多得到自身总需求。
    if target_tier_demand <= target_tier_capacity + DENSITY_EPSILON {
        return target_density; // 总需求不足时严格保持原密度。
    }
    var allocation_remaining = target_tier_capacity; // 水填充余量只属于目标优先级。
    loop {
        if active_mask == 0u || allocation_remaining <= DENSITY_EPSILON {
            return 0.0; // 目标未被写入时保持CPU显式零结果。
        }
        var weight_sum = 0.0; // 当前尚未封顶层的正权重总和。
        for (var index = 0u; index < params.layer_count; index = index + 1u) {
            if (active_mask & (1u << index)) != 0u {
                weight_sum = weight_sum + layer_weight(index); // 与CPU活动层顺序相加。
            }
        }
        var capped_mask = 0u; // 本轮按权重提议达到自身需求上限的层。
        for (var index = 0u; index < params.layer_count; index = index + 1u) {
            if (active_mask & (1u << index)) != 0u {
                let density = sample_at(texel_base, index).x; // 当前层自身需求上限。
                let proposed = allocation_remaining * layer_weight(index) / weight_sum; // 容量紧张时按权重分配。
                if proposed >= density - DENSITY_EPSILON {
                    capped_mask = capped_mask | (1u << index); // 小需求层先封顶并释放余量。
                }
            }
        }
        if capped_mask == 0u {
            return min(
                target_density,
                allocation_remaining * layer_weight(target_index) / weight_sum,
            ); // 所有活动层都未触顶时本轮直接完成。
        }
        if (capped_mask & (1u << target_index)) != 0u {
            return target_density; // 目标在本轮封顶时取得完整自身需求。
        }
        for (var index = 0u; index < params.layer_count; index = index + 1u) {
            if (capped_mask & (1u << index)) != 0u {
                allocation_remaining = max(
                    allocation_remaining - sample_at(texel_base, index).x,
                    0.0,
                ); // 与CPU稳定层序逐项扣除已满足需求。
            }
        }
        active_mask = active_mask & (~capped_mask); // 已封顶层退出后续轮次。
    }
    return 0.0; // Naga要求声明返回类型的函数覆盖保守控制流；正常循环路径已在上方全部返回。
}

fn priority_density(texel_base: u32, selector: f32) -> f32 {
    let claim_threshold = params.mode_value; // 任意高于阈值的声明层参与空间所有权。
    var found = false; // 没有声明层时该组保持空。
    var highest_priority = -2147483647 - 1; // i32最小值不会误压过合法优先级。
    for (var index = 0u; index < params.layer_count; index = index + 1u) {
        let claim = sample_at(texel_base, index).y; // 所有权密度与实际渲染密度正交。
        let priority = layer_priority(index); // 数值越大越高。
        if claim > claim_threshold && (!found || priority > highest_priority) {
            highest_priority = priority; // 较低层即使实际密度更高也不能参与最终平局。
            found = true; // 至少一个合法所有权声明存在。
        }
    }
    if !found {
        return 0.0; // 与CPU显式零份额一致。
    }
    var score_sum = 0.0; // 同一最高优先级的声明强度乘权重总和。
    var last_contender = 0u; // 浮点尾差回退到最后一个稳定合法层。
    for (var index = 0u; index < params.layer_count; index = index + 1u) {
        let claim = sample_at(texel_base, index).y; // 当前层所有权声明。
        if layer_priority(index) == highest_priority && claim > claim_threshold {
            score_sum = score_sum + claim * layer_weight(index); // 与CPU稳定层序累计权重区间。
            last_contender = index; // 记录最后一个合法平局层。
        }
    }
    let selection_target = selector * score_sum; // CPU已经编码[0,1)稳定世界选择值；名称避开WGSL保留字。
    var cursor = 0.0; // 稳定层顺序累计区间边界。
    var chosen = last_contender; // 保留CPU浮点尾差回退语义。
    for (var index = 0u; index < params.layer_count; index = index + 1u) {
        let claim = sample_at(texel_base, index).y; // 当前层所有权声明。
        if layer_priority(index) == highest_priority && claim > claim_threshold {
            cursor = cursor + claim * layer_weight(index); // 同CPU顺序累加当前层区间。
            if selection_target < cursor {
                chosen = index; // 固定世界选择值命中当前所有权层。
                break; // 首次命中即完成稳定选择。
            }
        }
    }
    if chosen == params.target_layer_index {
        return sample_at(texel_base, chosen).x; // 被选层只输出自己的实际密度，禁止所有权向外膨胀可见内容。
    }
    return 0.0; // 未取得所有权的目标层保持零。
}

fn resolved_density(texel_base: u32, context: vec2<f32>) -> f32 {
    var density = 0.0; // 非法模式不会产生可见内容。
    if params.mode == MODE_ADDITIVE {
        density = additive_density(texel_base); // 完全叠加只读取目标需求。
    } else if params.mode == MODE_WEIGHTED {
        density = weighted_density(texel_base); // 加权容量分区在GPU完成。
    } else if params.mode == MODE_PRIORITY {
        density = priority_density(texel_base, context.x); // 优先覆盖在GPU完成稳定所有权选择。
    }
    return clamp(density * context.y, 0.0, 1.0); // 竞争完成后只乘一次目标组足迹并集影响。
}

fn quantized_rgba(density: f32) -> vec4<f32> {
    let byte_value = u32(clamp(density, 0.0, 1.0) * 255.0 + 0.5); // 复刻CPU非负`round(value * 255)`。
    let normalized = f32(byte_value) / 255.0; // 写入精确八位端点供RGBA8Unorm转换。
    return vec4<f32>(normalized, normalized, normalized, 1.0); // RGB同值且Alpha固定255。
}

@compute @workgroup_size(8, 8, 1)
fn compose_density(@builtin(global_invocation_id) invocation: vec3<u32>) {
    if invocation.z >= params.tile_count || invocation.x >= TILE_SIDE || invocation.y >= TILE_SIDE {
        return; // 尾线程或空作业不得读取提交块缓冲。
    }
    let tile = tiles[invocation.z]; // 当前Z切片对应一个稳定提交块。
    let output_coord = tile.xy + invocation.xy; // 基础级只覆盖该块64×64闭开区域。
    if output_coord.x >= params.atlas_size.x || output_coord.y >= params.atlas_size.y {
        return; // 损坏映射也不能写出候选纹理。
    }
    let local_index = invocation.y * TILE_SIDE + invocation.x; // 块内行优先纹素索引。
    let texel_base = tile.z + local_index; // 当前纹素在上下文和逐层样本数组中的起点。
    let density = resolved_density(texel_base, contexts[texel_base]); // 只由正式未决输入产生结果。
    textureStore(output_base, vec2<i32>(output_coord), quantized_rgba(density)); // 当前目标层写入候选Mip0。
}
