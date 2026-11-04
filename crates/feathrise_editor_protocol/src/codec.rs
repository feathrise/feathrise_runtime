//! 完整帧编解码：固定头先验预算，JSON 只装元信息，大正文按原字节读写并共同校验。

use crate::{Envelope, Limits, PROTOCOL_VERSION, ProtocolError};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Read, Write},
    sync::Arc,
};

const MAGIC: &[u8; 8] = b"FTHRIPC1";
const HEADER_BYTES: usize = 56;

/// 固定头只能先检查总预算；解析消息类型后收紧分类预算，防止作者显示额度扩散。
fn check_metadata_budget(
    message: &Envelope,
    length: usize,
    limits: &Limits,
) -> Result<(), ProtocolError> {
    let kind_limit = match message.message {
        crate::Message::SetOverlay { .. } => 512 * 1024,
        crate::Message::SetAuthorTools { .. } => 128 * 1024,
        _ => 32 * 1024,
    };
    if length > limits.metadata_bytes.min(kind_limit) {
        return Err(ProtocolError::Limit("消息元信息超过其分类字节预算".into()));
    }
    Ok(())
}

fn checked_lengths(
    header: &[u8; HEADER_BYTES],
    limits: &Limits,
) -> Result<(usize, usize), ProtocolError> {
    if &header[..8] != MAGIC
        || u32::from_le_bytes(header[8..12].try_into().unwrap()) != PROTOCOL_VERSION
    {
        return Err(ProtocolError::Invalid("帧标识或协议版本不匹配".into()));
    }
    let metadata = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
    let payload = usize::try_from(u64::from_le_bytes(header[16..24].try_into().unwrap()))
        .map_err(|_| ProtocolError::Limit("快照长度无法在当前平台表示".into()))?;
    if metadata == 0 || metadata > limits.metadata_bytes || payload > limits.snapshot_bytes {
        return Err(ProtocolError::Limit(
            "声明长度超过元信息或完整快照预算".into(),
        ));
    }
    metadata
        .checked_add(payload)
        .ok_or_else(|| ProtocolError::Limit("帧长度溢出".into()))?;
    Ok((metadata, payload))
}

fn allocate(length: usize) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|error| ProtocolError::Limit(format!("分配完整帧失败：{error}")))?;
    bytes.resize(length, 0);
    Ok(bytes)
}

/// 阻塞读取只允许出现在专属后台线程；返回 None 仅代表完整帧边界上的正常 EOF。
pub fn read_frame(
    reader: &mut impl Read,
    limits: &Limits,
) -> Result<Option<Envelope>, ProtocolError> {
    let mut header = [0_u8; HEADER_BYTES];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    // 部分头、部分正文和半关闭都走 UnexpectedEof，绝不能发布为有效快照。
    reader.read_exact(&mut header[1..])?;
    let (metadata_length, payload_length) = checked_lengths(&header, limits)?;
    let mut metadata = allocate(metadata_length)?;
    reader.read_exact(&mut metadata)?;
    let mut message: Envelope = serde_json::from_slice(&metadata)
        .map_err(|error| ProtocolError::Invalid(format!("元信息 JSON 无效：{error}")))?;
    check_metadata_budget(&message, metadata_length, limits)?;
    // 即使头长度有界，普通控制也不得偷带大正文来绕过消息种类的合同。
    if matches!(message.message, crate::Message::Snapshot { .. } | crate::Message::Preview { .. }) != (payload_length != 0) {
        return Err(ProtocolError::Invalid("消息种类与正文长度不一致".into()));
    }
    // 先依据小元信息收紧分类预算，再分配正文；不能先分配128MiB才拒绝超大的预览。
    if matches!(message.message, crate::Message::Preview { .. }) && payload_length > crate::MAX_PREVIEW_BYTES {
        return Err(ProtocolError::Limit("作者增量预览超过64MiB预算".into()));
    }
    let mut payload = allocate(payload_length)?;
    reader.read_exact(&mut payload)?;
    let mut digest = Sha256::new();
    digest.update(&header[..24]);
    digest.update(&metadata);
    digest.update(&payload);
    if digest.finalize().as_slice() != &header[24..56] {
        return Err(ProtocolError::Invalid(
            "完整帧 SHA256 不匹配，拒绝采用".into(),
        ));
    }
    if payload_length != 0 {
        message.payload = Some(Arc::new(payload));
    }
    message.validate(limits)?;
    Ok(Some(message))
}

/// 唯一写线程保持帧头、元信息和正文连续；主线程只能投递共享正文，不能调用本函数。
pub fn write_frame(
    writer: &mut impl Write,
    message: &Envelope,
    limits: &Limits,
) -> Result<(), ProtocolError> {
    message.validate(limits)?;
    let metadata = serde_json::to_vec(message)
        .map_err(|error| ProtocolError::Invalid(format!("元信息编码失败：{error}")))?;
    check_metadata_budget(message, metadata.len(), limits)?;
    let payload = message
        .payload
        .as_ref()
        .map_or(&[][..], |bytes| bytes.as_slice());
    let mut header = [0_u8; HEADER_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&PROTOCOL_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(
        &u32::try_from(metadata.len())
            .map_err(|_| ProtocolError::Limit("元信息超过可编码长度".into()))?
            .to_le_bytes(),
    );
    header[16..24].copy_from_slice(&(payload.len() as u64).to_le_bytes());
    checked_lengths(&header, limits)?;
    let mut digest = Sha256::new();
    digest.update(&header[..24]);
    digest.update(&metadata);
    digest.update(payload);
    header[24..56].copy_from_slice(&digest.finalize());
    writer.write_all(&header)?;
    writer.write_all(&metadata)?;
    writer.write_all(payload)?;
    writer.flush()?;
    Ok(())
}
