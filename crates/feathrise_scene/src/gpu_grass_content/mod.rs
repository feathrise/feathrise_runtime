//! 数据预设解析与植被运行时桥；游戏专用模型和外观配置由游戏入口安装。

pub(crate) mod trail; // 动态场景共享固定容量玩家足迹。
mod preset_profiles; // 开发者目录中的完整 Blade 数据解析成当前修订拥有型规格。
mod runtime_profiles; // 动态场景相机启动视图基线，不内置工程预设身份。

pub use preset_profiles::*; // 动态场景只消费稳定预设解析结果，不读取目录 GPU 批次细节。
pub use runtime_profiles::*; // 相机共享通用启动视图参数。
