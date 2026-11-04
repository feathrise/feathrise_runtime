//! 单独运行天气植被桥回归，避免本crate其他历史单元测试签名错误阻断验证。

#[path = "../src/shared/vegetation_weather/mod.rs"]
// 直接编译正式桥源码与邻接测试，不复制映射逻辑。
mod vegetation_weather; // 独立集成测试只用于验证当前桥行为。
