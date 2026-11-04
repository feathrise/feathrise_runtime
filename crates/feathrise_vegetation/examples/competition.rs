//! 完全程序化的三种空间竞争语义示例。

use feathrise_vegetation::{
    VegetationCompetitionGroup, VegetationCompetitionGroupId, VegetationCompetitionMode,
    VegetationLayerDemand, VegetationLayerId, VegetationWeight, resolve_competition,
}; // 示例不读取任何游戏资产或私有路径。

fn main() {
    let demands = [
        VegetationLayerDemand {
            layer: VegetationLayerId(1), // 第一层可由消费者解释为任意内容。
            density: 1.0,                // 本层100%候选容量需求。
            claim_density: 1.0,          // 示例中所有权与实际需求一致。
            priority: 0,                 // 两层同优先级。
            weight: VegetationWeight::new(1.0).expect("示例权重合法"), // 第一层权重一。
        },
        VegetationLayerDemand {
            layer: VegetationLayerId(2), // 第二层仍不携带草或作物类别。
            density: 1.0,                // 第二层同样请求100%。
            claim_density: 1.0,          // 示例中所有权与实际需求一致。
            priority: 0,                 // 与第一层同级。
            weight: VegetationWeight::new(3.0).expect("示例权重合法"), // 第二层权重三。
        },
    ]; // 完成1:3需求构造。
    for mode in [
        VegetationCompetitionMode::Additive, // 输出100/100。
        VegetationCompetitionMode::WeightedPartition { capacity: 1.0 }, // 输出25/75。
        VegetationCompetitionMode::PriorityOverride {
            claim_threshold: 0.0,
        }, // 固定世界选择值只选择一个同级层。
    ] {
        let group = VegetationCompetitionGroup {
            id: VegetationCompetitionGroupId(1), // 示例只使用一个竞争组。
            mode,                                // 当前循环选择明确语义。
            selection_cell_size: 1.0,            // 同级覆盖使用一米固定世界格。
            selection_seed: 42,                  // 固定种子保证重启不洗牌。
        };
        let shares = resolve_competition(group, &demands, 0.2).expect("程序化竞争配置必须合法"); // 使用固定世界选择值演示结果。
        println!("{mode:?}: {shares:?}"); // 示例输出只用于观察纯解析结果。
    }
}
