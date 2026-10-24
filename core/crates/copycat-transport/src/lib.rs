pub mod conn;
pub mod discovery;
pub mod framed;
pub mod lan;
pub mod relay_client;
pub mod tls;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("帧错误: {0}")]
    Frame(#[from] copycat_proto::frame::FrameError),
    #[error("TLS 配置错误: {0}")]
    Tls(String),
    #[error("QUIC 连接错误: {0}")]
    Connect(String),
    #[error("连接已断开")]
    Closed,
    #[error("超时")]
    Timeout,
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("认证被拒绝: {0}")]
    Rejected(String),
}

pub type Result<T, E = TransportError> = core::result::Result<T, E>;
