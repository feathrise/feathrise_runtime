//! extras派生代理标记，避免清除玩法代码直接添加的遮雨组件。

use bevy::prelude::*; // ECS标记。

#[derive(Component)] // 与RainShelterBox成对安装和移除。
pub(super) struct ExtrasRainShelter; // 只标记显式rain_shelter字段的所有权。
