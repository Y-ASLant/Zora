use std::path::PathBuf;

use thiserror::Error;

/// 远程传输层错误。
#[derive(Debug, Error)]
pub enum TransportError {
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("SSH 错误: {0}")]
    Ssh(String),

    #[error("SFTP 错误: {0}")]
    Sftp(String),

    #[error("连接失败: {0}")]
    ConnectionFailed(String),

    #[error("认证失败: {0}")]
    AuthenticationFailed(String),

    #[error("操作超时")]
    Timeout,

    #[error("文件过大: {size} 字节，最大允许 {max} 字节")]
    FileTooLarge { size: u64, max: u64 },

    #[error("文件未找到: {0}")]
    NoSuchFile(PathBuf),

    #[error("权限不足: {0}")]
    PermissionDenied(PathBuf),

    #[error("路径不合法: {0}")]
    InvalidPath(String),

    #[error("传输已取消")]
    Cancelled,

    #[error("不支持的远程操作: {0}")]
    Unsupported(String),

    #[error("操作失败: {0}")]
    General(String),
}

pub type Result<T> = std::result::Result<T, TransportError>;

pub(crate) fn sftp_error(error: impl std::fmt::Display) -> TransportError {
    TransportError::Sftp(error.to_string())
}

pub(crate) fn ssh_error(error: impl std::fmt::Display) -> TransportError {
    TransportError::Ssh(error.to_string())
}
