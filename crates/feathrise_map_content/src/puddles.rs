//! 独立积水潜力R8协议；零值禁止积水，非零值只表达保水倾向而非水深。
use serde::{Deserialize, Serialize}; // 作者清单与正式运行时共享格式。

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] // 小型元数据允许历史浅快照。
pub struct MapPuddleDescriptor {
    // 原始作者源与GPU派生图保持明确配对。
    pub source_path: String,  // 地图私有无Mip原始R8文件。
    pub texture_path: String, // 地图私有线性R8 KTX2派生纹理。
    pub resolution: [u32; 2], // 首尾纹素对应清单世界XZ端点。
} // 结束协议。

impl MapPuddleDescriptor {
    // 统一校验编辑与正式读取边界。
    pub fn validate(&self) -> Result<(), String> {
        // 不允许坏尺寸或越界资源进入GPU。
        if self
            .resolution
            .iter()
            .any(|&size| !(2..=4096).contains(&size))
        {
            // 最大16MiB R8。
            return Err("积水画布每轴必须在2至4096之间".into()); // 清晰诊断并保留旧有效结果。
        } // 尺寸检查完成。
        for (path, suffix) in [(&self.source_path, ".r8"), (&self.texture_path, ".ktx2")] {
            // 两个目标均验证。
            if !path.starts_with("maps/")
                || path.contains("..")
                || path.contains('\\')
                || path.contains(':')
                || !path.ends_with(suffix)
            {
                // 限制作者路径。
                return Err("积水文件必须是maps内的独立.r8源与.ktx2派生纹理".into()); // 拒绝非私有路径。
            } // 当前路径受检完成。
        } // 所有路径均通过。
        Ok(()) // 完整描述可被作者加载与保存。
    } // 结束公共校验。
} // 结束协议行为。
