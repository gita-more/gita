//! VMM 驱动错误类型

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("VM 不存在：{0}")]
    NotFound(String),

    #[error("VM 已存在：{0}")]
    AlreadyExists(String),

    #[error("操作失败：{0}")]
    OperationFailed(String),

    #[error("VMM 进程错误：{0}")]
    ProcessError(String),

    #[error("配置生成失败：{0}")]
    ConfigGeneration(String),

    #[error("权限不足：需要 root 权限")]
    PermissionDenied,

    #[error("超时：{0}")]
    Timeout(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
