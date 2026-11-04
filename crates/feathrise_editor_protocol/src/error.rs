//! 可分类的协议失败；诊断交给宿主展示，协议层不吞掉断管或格式错误。

use std::{fmt, io};

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error),
    Invalid(String),
    Limit(String),
    Identity(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "IPC I/O 失败：{error}"),
            Self::Invalid(message) => write!(formatter, "IPC 数据无效：{message}"),
            Self::Limit(message) => write!(formatter, "IPC 超出预算：{message}"),
            Self::Identity(message) => write!(formatter, "IPC 身份不符：{message}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<io::Error> for ProtocolError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
