//! 领域错误类型

use gita_vm_api::quota::QuotaExceeded;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("实体不存在：{0}")]
    NotFound(String),

    #[error("实体已存在：{0}")]
    AlreadyExists(String),

    #[error("状态机非法转换：{0}")]
    InvalidTransition(String),

    #[error("调度失败：{0}")]
    SchedulingFailed(String),

    #[error("配额超限：{0}")]
    QuotaExceeded(#[from] QuotaExceeded),

    #[error("操作失败：{0}")]
    OperationFailed(String),

    #[error("IP 地址池耗尽：{0}")]
    IpPoolExhausted(String),

    #[error("验证失败：{0}")]
    Validation(String),

    #[error("存储错误：{0}")]
    Storage(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn not_found(what: impl std::fmt::Display) -> Self {
        Self::NotFound(what.to_string())
    }

    pub fn already_exists(what: impl std::fmt::Display) -> Self {
        Self::AlreadyExists(what.to_string())
    }
}