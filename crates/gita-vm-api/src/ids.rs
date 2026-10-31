//! ID 类型定义 —— 统一使用 i64，兼容 gita 数据库主键

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl $name {
            pub fn new(id: i64) -> Self {
                Self(id)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = std::num::ParseIntError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(s.parse()?))
            }
        }

        impl From<i64> for $name {
            fn from(id: i64) -> Self {
                Self(id)
            }
        }

        impl From<$name> for i64 {
            fn from(id: $name) -> i64 {
                id.0
            }
        }
    };
}

define_id!(VmId);
define_id!(TemplateId);
define_id!(ImageId);
define_id!(NodeId);
define_id!(NetworkId);
define_id!(VolumeId);
define_id!(OperationId);
define_id!(PolicyId);
define_id!(CellId);
define_id!(ClusterId);
define_id!(ApiKeyId);
define_id!(AgentId);
define_id!(StoragePoolId);
define_id!(SnapshotId);

/// 资源归属类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OwnerType {
    /// 系统级资源，所有租户可见
    System,
    /// 群组级资源，群组成员可见
    Group,
    /// 仓库级资源，仓库协作者可见
    Repo,
}

/// 资源归属引用
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OwnerRef {
    pub owner_type: OwnerType,
    pub owner_id: i64, // group_id 或 repo_id，system 时为 0
}

impl OwnerRef {
    pub fn system() -> Self {
        Self { owner_type: OwnerType::System, owner_id: 0 }
    }
    pub fn group(group_id: i64) -> Self {
        Self { owner_type: OwnerType::Group, owner_id: group_id }
    }
    pub fn repo(repo_id: i64) -> Self {
        Self { owner_type: OwnerType::Repo, owner_id: repo_id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn id_parse_and_display() {
        let id = VmId::from_str("123").unwrap();
        assert_eq!(id.0, 123);
        assert_eq!(id.to_string(), "123");
    }

    #[test]
    fn owner_ref_constructors() {
        assert_eq!(OwnerRef::system().owner_type, OwnerType::System);
        assert_eq!(OwnerRef::group(42).owner_id, 42);
        assert_eq!(OwnerRef::repo(100).owner_type, OwnerType::Repo);
    }
}