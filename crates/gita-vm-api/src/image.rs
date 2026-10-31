//! 镜像相关类型

use crate::ids::{ImageId, StoragePoolId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 镜像种类
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageKind {
    /// 根文件系统镜像（启动盘）
    Rootfs,
    /// 内核镜像
    Kernel,
    /// 云初始化 ISO
    CloudInit,
}

/// 镜像状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImageStatus {
    /// 正在导入/构建
    Importing,
    /// 就绪可用
    Ready,
    /// 导入失败
    Failed,
    /// 标记删除，等待 GC
    Deleting,
}

/// 镜像元数据
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Image {
    pub id: ImageId,
    pub name: String,
    pub kind: ImageKind,
    pub status: ImageStatus,
    /// 所属存储池
    pub pool_id: StoragePoolId,
    /// 文件路径（宿主机视角）
    pub path: String,
    /// 文件大小（字节）
    pub size_bytes: u64,
    /// 内容摘要（SHA256 hex）
    pub digest: String,
    /// 格式
    pub format: String, // qcow2, raw, vmdk, iso...
    /// 架构
    pub arch: String,
    /// 资源归属
    pub owner: crate::ids::OwnerRef,
    /// 标签（版本标签）
    #[serde(default)]
    pub tags: Vec<String>,
    /// 额外元数据
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// 创建时间（毫秒）
    pub created_at_ms: i64,
    /// 最后使用时间（毫秒）
    #[serde(default)]
    pub last_used_at_ms: Option<i64>,
}

impl Image {
    pub fn is_ready(&self) -> bool {
        matches!(self.status, ImageStatus::Ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_ready_check() {
        let img = Image {
            id: ImageId(1),
            name: "test".into(),
            kind: ImageKind::Rootfs,
            status: ImageStatus::Ready,
            pool_id: StoragePoolId(1),
            path: "/var/lib/gita/images/test.qcow2".into(),
            size_bytes: 1000,
            digest: "sha256:abc".into(),
            format: "qcow2".into(),
            arch: "x86_64".into(),
            owner: crate::ids::OwnerRef::system(),
            tags: vec!["latest".into()],
            metadata: BTreeMap::new(),
            created_at_ms: 0,
            last_used_at_ms: None,
        };
        assert!(img.is_ready());
    }
}