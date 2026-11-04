//! 近场测试按行为、空间索引和并行策略拆分，公共fixture集中复用。

mod behavior; // 真实Bevy应用验证滞回、层清空和索引失效生命周期。
mod fixture; // 集中构造合法层配置、显式落点与全一活动布局。
mod parallel; // 锁定并行门槛及真实TaskPool串并行结果一致性。
mod spatial; // 验证负坐标、边界、多观察者和全量扫描等价性。
