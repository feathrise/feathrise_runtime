//! 会话身份与接收顺序门；旧代次可丢弃，跨工程、重启旧回执和乱序必须拒绝。

use crate::{Envelope, Message, ProtocolError, SessionIdentity};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Current,
    StaleGeneration,
}

pub struct SessionGuard {
    identity: SessionIdentity,
    last_sequence: u64,
}

impl SessionGuard {
    pub fn new(identity: SessionIdentity) -> Self {
        Self {
            identity,
            last_sequence: 0,
        }
    }

    pub fn identity(&self) -> &SessionIdentity {
        &self.identity
    }

    pub fn advance_generation(&mut self, generation: u64) -> Result<(), ProtocolError> {
        if generation <= self.identity.generation {
            return Err(ProtocolError::Identity("切图代次必须严格递增".into()));
        }
        self.identity.generation = generation;
        Ok(())
    }

    pub fn accept(&mut self, message: &Envelope) -> Result<Disposition, ProtocolError> {
        if message.identity.project_id != self.identity.project_id
            || message.identity.session != self.identity.session
        {
            return Err(ProtocolError::Identity("工程或启动会话不匹配".into()));
        }
        if message.identity.generation < self.identity.generation {
            return Ok(Disposition::StaleGeneration);
        }
        if message.identity.generation != self.identity.generation {
            return Err(ProtocolError::Identity("尚未授权的新地图代次".into()));
        }
        if message.sequence == 0 || message.sequence <= self.last_sequence {
            return Err(ProtocolError::Identity("消息重复或接收顺序倒退".into()));
        }
        self.last_sequence = message.sequence;
        Ok(Disposition::Current)
    }

    /// 构建身份由宿主本次实际启动产物给出；不能只看管道可读就认定 SDK 兼容。
    pub fn verify_handshake(
        message: &Envelope,
        expected_build: &str,
        required: &[&str],
    ) -> Result<(), ProtocolError> {
        let (build_identity, capabilities) = match &message.message {
            Message::Hello {
                build_identity,
                capabilities,
            }
            | Message::Ready {
                build_identity,
                capabilities,
                ..
            } => (build_identity, capabilities),
            _ => return Err(ProtocolError::Identity("尚未收到握手消息".into())),
        };
        if build_identity != expected_build || expected_build.is_empty() {
            return Err(ProtocolError::Identity("实际构建身份不匹配".into()));
        }
        if let Some(missing) = required
            .iter()
            .find(|required| !capabilities.iter().any(|actual| actual == **required))
        {
            return Err(ProtocolError::Identity(format!(
                "运行实例缺少必需能力：{missing}"
            )));
        }
        Ok(())
    }
}
