//! 预览专用管道帧：固定头部与像素预算，日志不经过这条通道，编解码仅在后台线程。
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const WIDTH: u32 = 320;
pub const HEIGHT: u32 = 320;
pub const MAX_HEADER: usize = 448 * 1024;
pub const MAX_PIXELS: usize = (WIDTH * HEIGHT * 4) as usize;
pub const MAX_EVIDENCE: usize = 4096;

/// 在克隆前检查固定结构/字节上限；不先序列化可能很大的领域JSON，也不遍历无界目录。
pub fn evidence_within_budget(data: &serde_json::Value) -> bool {
    fn charge(remaining: &mut usize, amount: usize) -> bool {
        if let Some(next) = remaining.checked_sub(amount) {
            *remaining = next;
            true
        } else {
            false
        }
    }
    fn visit(
        value: &serde_json::Value,
        remaining: &mut usize,
        nodes: &mut usize,
        depth: usize,
    ) -> bool {
        if depth > 8 || !charge(nodes, 1) {
            return false;
        }
        match value {
            serde_json::Value::Null | serde_json::Value::Bool(_) => charge(remaining, 5),
            serde_json::Value::Number(_) => charge(remaining, 32),
            serde_json::Value::String(text) => text
                .len()
                .checked_mul(6)
                .and_then(|n| n.checked_add(2))
                .is_some_and(|n| charge(remaining, n)),
            serde_json::Value::Array(values) => {
                values.len() <= *nodes
                    && charge(remaining, 2 + values.len())
                    && values
                        .iter()
                        .all(|value| visit(value, remaining, nodes, depth + 1))
            }
            serde_json::Value::Object(values) => {
                values.len() <= *nodes
                    && charge(remaining, 2 + values.len())
                    && values.iter().all(|(key, value)| {
                        key.len()
                            .checked_mul(6)
                            .and_then(|n| n.checked_add(3))
                            .is_some_and(|n| charge(remaining, n))
                            && visit(value, remaining, nodes, depth + 1)
                    })
            }
        }
    }
    let mut bytes = MAX_EVIDENCE;
    let mut nodes = 64;
    visit(data, &mut bytes, &mut nodes, 0)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub project: String,
    pub session: u64,
    pub revision: u64,
    pub message: String,
    pub kind: String,
    #[serde(default)]
    pub data: serde_json::Value,
}
impl Packet {
    pub fn validate(&self) -> Result<(), String> {
        if self.project.is_empty()
            || self.project.len() > 128
            || self.session == 0
            || self.kind.len() > 128
            || ![
                "hello", "ready", "preview", "clear", "camera", "frame", "error", "stop", "stopped",
            ]
            .contains(&self.message.as_str())
        {
            return Err("作者预览帧身份或消息无效".into());
        }
        if ["preview", "frame"].contains(&self.message.as_str())
            && (self.revision == 0 || self.kind.is_empty())
        {
            return Err("作者预览缺少文档修订或领域身份".into());
        }
        if self.message == "camera"
            && ["yaw", "pitch", "distance"]
                .iter()
                .any(|key| self.data[*key].as_f64().is_none_or(|v| !v.is_finite()))
        {
            return Err("作者预览相机参数必须是有限数字".into());
        }
        if self.message == "frame" && !evidence_within_budget(&self.data) {
            return Err("作者预览执行证据超过4KiB、64节点或8层预算".into());
        }
        Ok(())
    }
}
pub fn write(output: &mut impl Write, packet: &Packet, pixels: &[u8]) -> Result<(), String> {
    packet.validate()?;
    let header = serde_json::to_vec(packet).map_err(|e| e.to_string())?;
    if header.len() > MAX_HEADER
        || pixels.len() > MAX_PIXELS
        || (packet.message == "frame") != (pixels.len() == MAX_PIXELS)
        || (packet.message != "frame" && !pixels.is_empty())
    {
        return Err("作者预览正文或像素超过固定预算".into());
    }
    output
        .write_all(&(header.len() as u32).to_le_bytes())
        .and_then(|_| output.write_all(&(pixels.len() as u32).to_le_bytes()))
        .and_then(|_| output.write_all(&header))
        .and_then(|_| output.write_all(pixels))
        .and_then(|_| output.flush())
        .map_err(|e| e.to_string())
}
pub fn read(input: &mut impl Read) -> Result<Option<(Packet, Vec<u8>)>, String> {
    let mut lengths = [0u8; 8];
    match input.read(&mut lengths[..1]) {
        Ok(0) => return Ok(None),
        Ok(_) => (),
        Err(e) => return Err(e.to_string()),
    }
    input
        .read_exact(&mut lengths[1..])
        .map_err(|e| e.to_string())?;
    let header = u32::from_le_bytes(lengths[..4].try_into().unwrap()) as usize;
    let pixels = u32::from_le_bytes(lengths[4..].try_into().unwrap()) as usize;
    if header == 0 || header > MAX_HEADER || pixels > MAX_PIXELS {
        return Err("作者预览帧长度超限".into());
    }
    let mut bytes = vec![0; header];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let packet: Packet = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    packet.validate()?;
    if (packet.message == "frame") != (pixels == MAX_PIXELS)
        || (packet.message != "frame" && pixels != 0)
    {
        return Err("作者预览像素尺寸不完整".into());
    }
    let mut bytes = vec![0; pixels];
    input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(Some((packet, bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_roundtrip_and_budget() {
        let p = Packet {
            project: "工程".into(),
            session: 3,
            revision: 4,
            message: "frame".into(),
            kind: "fixture".into(),
            data: serde_json::Value::Null,
        };
        let pixels = vec![23; MAX_PIXELS];
        let mut bytes = Vec::new();
        write(&mut bytes, &p, &pixels).unwrap();
        let (actual, image) = read(&mut bytes.as_slice()).unwrap().unwrap();
        assert_eq!(actual.revision, 4);
        assert_eq!(image, pixels);
        assert!(write(&mut Vec::new(), &p, &[]).is_err());
        assert!(read(&mut [255u8; 8].as_slice()).is_err());
    }
    #[test]
    fn execution_evidence_rejects_large_and_deep_values_before_clone() {
        assert!(evidence_within_budget(
            &serde_json::json!({"initialized":true,"elapsed":0.2,"completed":false,"vfx_emitted":3})
        ));
        assert!(!evidence_within_budget(&serde_json::Value::String(
            "x".repeat(MAX_EVIDENCE)
        )));
        assert!(!evidence_within_budget(&serde_json::Value::Array(vec![
            serde_json::json!(
                0
            );
            128
        ])));
        let mut nested = serde_json::Value::Null;
        for _ in 0..10 {
            nested = serde_json::Value::Array(vec![nested]);
        }
        assert!(!evidence_within_budget(&nested));
    }
}
