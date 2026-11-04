//! 编辑器与工程常驻视口的进程合同：不依赖 Bevy、原游戏、作者实现或操作系统。
//!
//! 小控制和大快照共用有界帧；编解码、哈希及阻塞 I/O 只能由调用方后台线程执行。
//! 本库不暗中创建线程或持有进程。调用方负责专用管道、线程退出、超时和子进程收口。

mod codec;
mod error;
mod mailbox;
mod session;
mod types;

pub use codec::{read_frame, write_frame};
pub use error::ProtocolError;
pub use mailbox::{LatestMailbox, PublishResult};
pub use session::{Disposition, SessionGuard};
pub use feathrise_author_tools::{
    AuthorTool, AuthorToolPreview, AuthorToolValidationError, BrushPreview, BrushShape,
    ClipmapBrushSurface, SelectionPreview, TransformPreview, MAX_AUTHOR_BRUSHES,
    MAX_AUTHOR_SELECTIONS, MAX_AUTHOR_TOOLS, MAX_AUTHOR_TOOL_SEGMENTS, MAX_AUTHOR_TRANSFORMS,
};
pub use types::{
    AuthorViewState, CameraState, Envelope, Limits, MAX_VIEWPORT_EXCLUDED_RECTS, Message, Mode, OverlayLine,
    PROTOCOL_VERSION, SessionIdentity, PreviewKind, MAX_PREVIEW_BYTES,
    ViewportUiShape, MAX_VIEWPORT_EXCLUDED_SHAPES,
    FUNCTION_HOTPATCH_CAPABILITY,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod author_tools_tests;

#[cfg(test)]
mod hotpatch_tests;
