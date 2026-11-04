//! 小型glTF作者参数解析：不从建筑名称或整个Mesh包围盒猜测屋檐。

use bevy::prelude::*; // 作者局部坐标。
use feathrise_environment::rain_shelter::RainShelterBox; // 与直接ECS接口共用输入。

pub(super) fn parse_extras(value: &str) -> Result<Option<RainShelterBox>, ()> {
    // 单节点最多64KiB，避免坏extras制造主线程大解析。
    if value.len() > 65_536 {
        return Ok(None);
    } // 长度门先于字符串搜索，不让无关大型extras拖慢主线程；已登记源由宿主显式拒绝。
    if !value.contains("\"rain_shelter\"") {
        return Ok(None);
    } // 无关extras不解析JSON。
    let value: serde_json::Value = serde_json::from_str(value).map_err(|_| ())?; // 仅解析有界作者数据。
    let Some(shelter) = value.get("rain_shelter") else {
        return Ok(None);
    }; // 字符串中提及字段不构成作者源。
    let vector = |key: &str| -> Result<Vec3, ()> {
        // 严格三项且有限，拒绝隐式默认盒子。
        let array = shelter
            .get(key)
            .and_then(|value| value.as_array())
            .ok_or(())?; // 字段必须是数组。
        if array.len() != 3 {
            return Err(());
        } // 不接受缺轴或多轴。
        let mut vector = Vec3::ZERO; // 三个值分别验证。
        for axis in 0..3 {
            vector[axis] = array[axis].as_f64().ok_or(())? as f32;
        } // 三次固定转换。
        if !vector.is_finite() {
            return Err(());
        } // f64超出f32范围同样拒绝。
        Ok(vector) // 返回受检向量。
    }; // 完成局部小解析器。
    let center = vector("center")?; // 局部中心必填，避免坐标约定歧义。
    let half_extents = vector("half_extents")?; // 明确三轴半径而非全宽。
    if half_extents.min_element() <= 0.0 {
        return Err(());
    } // 零厚度或负半径无效。
    Ok(Some(RainShelterBox {
        center,
        half_extents,
    })) // 不绑定是否存在Collider或Mesh。
} // 完成节点作者契约。
