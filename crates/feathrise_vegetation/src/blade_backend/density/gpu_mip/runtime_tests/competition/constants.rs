//! P5.2b2b RenderApp 竞争烟雾的稳定组、层、票据与世界范围。

use bevy::prelude::{Rect, Vec2}; // 两个相邻64纹素块使用一米端点式世界范围。

use crate::{VegetationCompetitionGroupId, VegetationLayerId}; // 技术身份只验证通用后端，不表达内容类别。

pub(super) const ADD_GROUP: VegetationCompetitionGroupId = VegetationCompetitionGroupId(0xB200); // 单层叠加组验证直接需求和足迹影响。
pub(super) const WEIGHTED_GROUP: VegetationCompetitionGroupId =
    VegetationCompetitionGroupId(0xB210); // 三层容量组验证优先级与封顶水填充。
pub(super) const PRIORITY_GROUP: VegetationCompetitionGroupId =
    VegetationCompetitionGroupId(0xB220); // 三层覆盖组验证声明与稳定同级选择。
pub(super) const EXCLUDER_GROUP: VegetationCompetitionGroupId =
    VegetationCompetitionGroupId(0xB230); // 稀疏显式层向三个目标组输出同一圆形恢复带。

pub(super) const ADD_LAYER: VegetationLayerId = VegetationLayerId(0xB201); // 局部换版专门修改本层右块。
pub(super) const WEIGHT_SMALL_LAYER: VegetationLayerId = VegetationLayerId(0xB211); // 小需求层应先封顶并释放容量。
pub(super) const WEIGHT_LARGE_LAYER: VegetationLayerId = VegetationLayerId(0xB212); // 大需求层消费封顶后的剩余容量。
pub(super) const WEIGHT_HIGH_LAYER: VegetationLayerId = VegetationLayerId(0xB213); // 高优先级层先于两个同级层扣除容量。
pub(super) const PRIORITY_LOW_LAYER: VegetationLayerId = VegetationLayerId(0xB221); // 作物声明存在时必须归零。
pub(super) const PRIORITY_FIRST_LAYER: VegetationLayerId = VegetationLayerId(0xB222); // 两个高层按世界选择值分配空间。
pub(super) const PRIORITY_SECOND_LAYER: VegetationLayerId = VegetationLayerId(0xB223); // 第二高层暴露目标索引和稳定层序错误。
pub(super) const EXCLUDER_LAYER: VegetationLayerId = VegetationLayerId(0xB231); // 单一显式候选在右块产生硬净空和软恢复。

pub(super) const FULL_TICKET_ADDITIVE: u64 = 0xB201; // 不同组票据使RenderApp明确重建输入缓冲。
pub(super) const FULL_TICKET_WEIGHTED: u64 = 0xB211; // 同票据不同目标层验证共享Arc缓冲复用。
pub(super) const FULL_TICKET_PRIORITY: u64 = 0xB221; // 同票据两个目标层各执行一次目标索引分支。
pub(super) const LOCAL_TICKET_ADDITIVE: u64 = 0xB202; // 新票据只提交右块并复制上一基础级。

/// 返回fixture两个相邻块的一米端点式世界范围。
pub(super) fn fixture_bounds() -> Rect {
    Rect::from_corners(Vec2::new(-64.0, 0.0), Vec2::new(64.0, 64.0)) // 左右各64×64输出纹素。
}
