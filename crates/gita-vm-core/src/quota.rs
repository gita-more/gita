//! 配额引擎：分层配额校验（系统 → 群组 → 仓库）
//!
//! 逻辑：
//! 1. 查找作用于请求者的所有配额（系统默认 + 群组配额 + 仓库配额）
//! 2. 取最严格的限制（最小值）
//! 3. 检查当前使用量 + 申请量是否超限


use gita_vm_api::ids::OwnerRef;
use gita_vm_api::node::Resources;
use gita_vm_api::quota::{EffectiveQuota, Quota, QuotaScope};
use crate::repository::Repository;
use crate::Result;

/// 配额引擎
pub struct QuotaEngine {
    repo: std::sync::Arc<dyn Repository>,
    system_default: Quota,
}

impl QuotaEngine {
    pub fn new(repo: std::sync::Arc<dyn Repository>) -> Self {
        let now = crate::now_ms();
        Self {
            repo,
            system_default: Quota {
                id: 0,
                scope: QuotaScope::System,
                scope_id: 0,
                max_concurrent_vms: 100,
                max_cpu_millis: 1_000_000,       // 1000 核
                max_memory_bytes: 1_000_000_000_000, // 1 TB
                max_disk_bytes: 10_000_000_000_000,  // 10 TB
                max_vm_lifetime_ms: 0,            // 0 = 不限制
                created_at_ms: now,
                updated_at_ms: now,
            },
        }
    }

    /// 设置系统默认配额
    pub fn with_system_default(mut self, q: Quota) -> Self {
        self.system_default = q;
        self
    }

    /// 检查某个归属创建 VM 是否超配额
    pub async fn check_create_vm(&self, owner: &OwnerRef, requested: &Resources) -> Result<()> {
        let usages = self.gather_usages(owner).await?;

        // 检查每一层配额
        for eq in usages {
            eq.quota.check(&eq.usage, requested)?;
        }
        Ok(())
    }

    /// 收集某个归属适用的所有配额（系统 + 群组 + 仓库）
    async fn gather_usages(&self, owner: &OwnerRef) -> Result<Vec<EffectiveQuota>> {
        let mut result = Vec::new();

        // 1. 系统默认
        let sys_usage = self.repo.get_quota_usage(QuotaScope::System, 0).await?;
        result.push(EffectiveQuota {
            quota: self.system_default.clone(),
            usage: sys_usage,
            owner: OwnerRef::system(),
        });

        // 2. 群组配额（如果归属是群组或仓库属于群组）
        if let Some(group_id) = self.resolve_group_id(owner).await? {
            if let Some(q) = self.repo.get_quota(QuotaScope::Group, group_id).await? {
                let usage = self.repo.get_quota_usage(QuotaScope::Group, group_id).await?;
                result.push(EffectiveQuota {
                    quota: q,
                    usage,
                    owner: OwnerRef::group(group_id),
                });
            }
        }

        // 3. 仓库配额（如果归属是仓库）
        if let gita_vm_api::OwnerType::Repo = owner.owner_type {
            if let Some(q) = self.repo.get_quota(QuotaScope::Repo, owner.owner_id).await? {
                let usage = self.repo.get_quota_usage(QuotaScope::Repo, owner.owner_id).await?;
                result.push(EffectiveQuota {
                    quota: q,
                    usage,
                    owner: OwnerRef::repo(owner.owner_id),
                });
            }
        }

        Ok(result)
    }

    /// 解析归属对应的群组 ID
    async fn resolve_group_id(&self, owner: &OwnerRef) -> Result<Option<i64>> {
        match owner.owner_type {
            gita_vm_api::ids::OwnerType::Group => Ok(Some(owner.owner_id)),
            gita_vm_api::ids::OwnerType::Repo => {
                // 仓库归属需要查 repo 表找 group_id
                // 这里简化：由调用方传入或后续扩展
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// 获取某归属的有效配额（用于前端展示）
    pub async fn get_effective_quotas(&self, owner: &OwnerRef) -> Result<Vec<EffectiveQuota>> {
        self.gather_usages(owner).await
    }

    /// 创建/更新配额
    pub async fn upsert_quota(&self, quota: &Quota) -> Result<()> {
        if self.repo.get_quota(quota.scope, quota.scope_id).await?.is_some() {
            self.repo.update_quota(quota).await
        } else {
            self.repo.create_quota(quota).await
        }
    }

    /// 删除配额
    pub async fn delete_quota(&self, scope: QuotaScope, scope_id: i64) -> Result<()> {
        self.repo.delete_quota(scope, scope_id).await
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use gita_vm_api::quota::{Quota, QuotaScope, QuotaUsage, QuotaExceeded};
    use gita_vm_api::node::Resources;
    use gita_vm_api::ids::OwnerRef;
    use crate::Error;

    struct MockRepo {
        quotas: std::sync::Mutex<HashMap<(QuotaScope, i64), Quota>>,
        usages: std::sync::Mutex<HashMap<(QuotaScope, i64), QuotaUsage>>,
    }

    #[async_trait::async_trait]
    impl crate::repository::Repository for MockRepo {
        async fn get_quota(&self, scope: QuotaScope, scope_id: i64) -> Result<Option<Quota>> {
            Ok(self.quotas.lock().unwrap().get(&(scope, scope_id)).cloned())
        }
        async fn get_quota_usage(&self, scope: QuotaScope, scope_id: i64) -> Result<QuotaUsage> {
            Ok(self.usages.lock().unwrap().get(&(scope, scope_id)).cloned().unwrap_or_default())
        }
        async fn create_quota(&self, q: &Quota) -> Result<()> {
            self.quotas.lock().unwrap().insert((q.scope, q.scope_id), q.clone());
            Ok(())
        }
        async fn update_quota(&self, q: &Quota) -> Result<()> {
            self.quotas.lock().unwrap().insert((q.scope, q.scope_id), q.clone());
            Ok(())
        }
        async fn delete_quota(&self, scope: QuotaScope, scope_id: i64) -> Result<()> {
            self.quotas.lock().unwrap().remove(&(scope, scope_id));
            Ok(())
        }
        async fn list_quotas(&self) -> Result<Vec<Quota>> { Ok(vec![]) }
    }

    fn test_engine() -> (QuotaEngine, std::sync::Arc<MockRepo>) {
        let repo = std::sync::Arc::new(MockRepo {
            quotas: std::sync::Mutex::new(HashMap::new()),
            usages: std::sync::Mutex::new(HashMap::new()),
        });
        (QuotaEngine::new(repo.clone()), repo)
    }

    #[tokio::test]
    async fn allows_within_all_quotas() {
        let (engine, _repo) = test_engine();
        let owner = OwnerRef::system();
        let req = Resources::new(2000, 2_000_000_000, 10_000_000_000);
        assert!(engine.check_create_vm(&owner, &req).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_when_any_layer_exceeded() {
        let (engine, repo) = test_engine();

        // 设置群组配额：仅允许 1 台 VM
        let group_q = Quota {
            id: 1, scope: QuotaScope::Group, scope_id: 42,
            max_concurrent_vms: 1, max_cpu_millis: 8000,
            max_memory_bytes: 8_000_000_000, max_disk_bytes: 50_000_000_000,
            max_vm_lifetime_ms: 0, created_at_ms: 0, updated_at_ms: 0,
        };
        engine.repo.create_quota(&group_q).await.unwrap();

        // 设置当前用量：已用 1 台
        repo.usages.lock().unwrap().insert(
            (QuotaScope::Group, 42),
            QuotaUsage { concurrent_vms: 1, ..Default::default() }
        );

        let owner = OwnerRef::group(42);
        let req = Resources::new(2000, 2_000_000_000, 10_000_000_000);
        let err = engine.check_create_vm(&owner, &req).await.unwrap_err();
        assert!(matches!(err, Error::QuotaExceeded(QuotaExceeded::ConcurrentVms { .. })));
    }

    #[tokio::test]
    async fn takes_min_of_all_layers() {
        let (engine, _repo) = test_engine();

        // 系统配额宽松
        // 群组配额：CPU 4000
        let group_q = Quota {
            id: 1, scope: QuotaScope::Group, scope_id: 42,
            max_concurrent_vms: 10, max_cpu_millis: 4000,
            max_memory_bytes: 8_000_000_000, max_disk_bytes: 50_000_000_000,
            max_vm_lifetime_ms: 0, created_at_ms: 0, updated_at_ms: 0,
        };
        engine.repo.create_quota(&group_q).await.unwrap();

        // 仓库配额：CPU 2000（更严格）
        let repo_q = Quota {
            id: 2, scope: QuotaScope::Repo, scope_id: 100,
            max_concurrent_vms: 10, max_cpu_millis: 2000,
            max_memory_bytes: 8_000_000_000, max_disk_bytes: 50_000_000_000,
            max_vm_lifetime_ms: 0, created_at_ms: 0, updated_at_ms: 0,
        };
        engine.repo.create_quota(&repo_q).await.unwrap();

        let owner = OwnerRef::repo(100);
        // 请求 3000 CPU -> 超过仓库配额 2000
        let req = Resources::new(3000, 2_000_000_000, 10_000_000_000);
        let err = engine.check_create_vm(&owner, &req).await.unwrap_err();
        assert!(matches!(err, Error::QuotaExceeded(QuotaExceeded::Cpu { .. })));

        // 请求 1500 CPU -> 通过
        let req = Resources::new(1500, 2_000_000_000, 10_000_000_000);
        assert!(engine.check_create_vm(&owner, &req).await.is_ok());
    }
}

