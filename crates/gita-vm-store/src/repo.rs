//! PostgreSQL Repository 实现

use crate::models::*;
use async_trait::async_trait;
use gita_vm_api::ids::{ImageId, NetworkId, NodeId, OperationId, OwnerRef, TemplateId, VmId};
use gita_vm_api::image::Image;
use gita_vm_api::network::{IpLease, Network};
use gita_vm_api::node::Node;
use gita_vm_api::operation::Operation;
use gita_vm_api::quota::{Quota, QuotaScope, QuotaUsage};
use gita_vm_api::template::VmTemplate;
use gita_vm_api::vm::{Vm, VmLinkage};
use gita_vm_core::repository::Repository;
use gita_vm_core::{Error, Result};
use sqlx::PgPool;
use sqlx::Row;

/// PostgreSQL 存储实现
#[derive(Clone)]
pub struct VmStore {
    pool: PgPool,
}

impl VmStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    fn owner_params(owner: &OwnerRef) -> (String, i64) {
        let db: DbOwnerRef = owner.clone().into();
        (db.owner_type, db.owner_id)
    }
}

#[async_trait]
impl Repository for VmStore {
    // ===== VM =====
    async fn create_vm(&self, vm: &Vm) -> Result<()> {
        let (owner_type, owner_id) = Self::owner_params(&vm.owner);
        sqlx::query(
            "INSERT INTO vm_instances
             (name, template_id, owner_type, owner_id, node_id, spec, status,
              generation, observed_generation, conditions, rootfs_path, ip_address,
              console_url, created_at_ms, message, expires_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(&vm.name)
        .bind(vm.spec.template.map(|t| t.0))
        .bind(&owner_type)
        .bind(owner_id)
        .bind(vm.node.map(|n| n.0))
        .bind(serde_json::to_value(&vm.spec).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(
            serde_json::to_value(&vm.status)
                .map_err(|e| Error::Storage(e.to_string()))?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "pending".into()),
        )
        .bind(vm.generation as i64)
        .bind(vm.observed_generation as i64)
        .bind(serde_json::to_value(&vm.conditions).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(&vm.rootfs_path)
        .bind(&vm.ip_address)
        .bind(&vm.console_url)
        .bind(vm.created_at_ms)
        .bind(&vm.message)
        .bind(vm.expires_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    /// CI 用：创建 VM 并写入 gita 侧关联，返回真实 id。
    ///
    /// `create_vm` 不写 `repo_id` / `pipeline_run_id` / `job_id`，而且不
    /// 返回 id——调用方拿到的永远是 0。CI 场景两样都需要：关联决定谁来
    /// 回收这台 VM，id 是 runner 回收它的唯一凭据。
    async fn create_linked_vm(&self, vm: &Vm, linkage: VmLinkage) -> Result<VmId> {
        let (owner_type, owner_id) = Self::owner_params(&vm.owner);
        let row = sqlx::query(
            "INSERT INTO vm_instances
             (name, template_id, owner_type, owner_id, node_id, spec, status,
              generation, observed_generation, conditions, rootfs_path, ip_address,
              console_url, created_at_ms, message, expires_at_ms,
              repo_id, pipeline_run_id, job_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16,
                     $17, $18, $19)
             RETURNING id",
        )
        .bind(&vm.name)
        .bind(vm.spec.template.map(|t| t.0))
        .bind(&owner_type)
        .bind(owner_id)
        .bind(vm.node.map(|n| n.0))
        .bind(serde_json::to_value(&vm.spec).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(
            serde_json::to_value(&vm.status)
                .map_err(|e| Error::Storage(e.to_string()))?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "pending".into()),
        )
        .bind(vm.generation as i64)
        .bind(vm.observed_generation as i64)
        .bind(serde_json::to_value(&vm.conditions).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(&vm.rootfs_path)
        .bind(&vm.ip_address)
        .bind(&vm.console_url)
        .bind(vm.created_at_ms)
        .bind(&vm.message)
        .bind(vm.expires_at_ms)
        .bind(linkage.repo_id)
        .bind(linkage.pipeline_run_id)
        .bind(linkage.job_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        let id: i64 = sqlx::Row::try_get(&row, "id").map_err(|e| Error::Storage(e.to_string()))?;
        Ok(VmId(id))
    }

    async fn get_vm_linkage(&self, id: VmId) -> Result<Option<VmLinkage>> {
        let row = sqlx::query_as::<_, DbVm>("SELECT * FROM vm_instances WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(|v| VmLinkage {
            repo_id: v.repo_id,
            pipeline_run_id: v.pipeline_run_id,
            job_id: v.job_id,
        }))
    }

    async fn get_vm(&self, id: VmId) -> Result<Option<Vm>> {
        let row = sqlx::query_as::<_, DbVm>("SELECT * FROM vm_instances WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(Vm::from))
    }

    async fn update_vm(&self, vm: &Vm) -> Result<()> {
        sqlx::query(
            "UPDATE vm_instances SET
                name = $2, node_id = $3, spec = $4, status = $5,
                generation = $6, observed_generation = $7, conditions = $8,
                rootfs_path = $9, ip_address = $10, console_url = $11,
                message = $12, expires_at_ms = $13
             WHERE id = $1",
        )
        .bind(vm.id.0)
        .bind(&vm.name)
        .bind(vm.node.map(|n| n.0))
        .bind(serde_json::to_value(&vm.spec).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(
            serde_json::to_value(&vm.status)
                .map_err(|e| Error::Storage(e.to_string()))?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "pending".into()),
        )
        .bind(vm.generation as i64)
        .bind(vm.observed_generation as i64)
        .bind(serde_json::to_value(&vm.conditions).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(&vm.rootfs_path)
        .bind(&vm.ip_address)
        .bind(&vm.console_url)
        .bind(&vm.message)
        .bind(vm.expires_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn delete_vm(&self, id: VmId) -> Result<()> {
        sqlx::query("DELETE FROM vm_instances WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn list_vms(&self, owner: Option<OwnerRef>) -> Result<Vec<Vm>> {
        let rows = if let Some(owner) = owner {
            let (owner_type, owner_id) = Self::owner_params(&owner);
            sqlx::query_as::<_, DbVm>(
                "SELECT * FROM vm_instances WHERE owner_type = $1 AND owner_id = $2 ORDER BY id DESC",
            )
            .bind(&owner_type)
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        } else {
            sqlx::query_as::<_, DbVm>("SELECT * FROM vm_instances ORDER BY id DESC")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?
        };
        Ok(rows.into_iter().map(Vm::from).collect())
    }

    async fn list_vms_by_node(&self, node_id: NodeId) -> Result<Vec<Vm>> {
        let rows =
            sqlx::query_as::<_, DbVm>("SELECT * FROM vm_instances WHERE node_id = $1 ORDER BY id")
                .bind(node_id.0)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(rows.into_iter().map(Vm::from).collect())
    }

    // ===== Template =====
    async fn create_template(&self, t: &VmTemplate) -> Result<()> {
        let (owner_type, owner_id) = Self::owner_params(&t.owner);
        sqlx::query(
            "INSERT INTO vm_templates
             (name, version, spec, image_id, kernel_image_id, owner_type, owner_id, tags, description, created_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&t.name)
        .bind(t.version as i64)
        .bind(serde_json::to_value(&t.spec).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(t.image_id.map(|i| i.0))
        .bind(t.kernel_image_id.map(|i| i.0))
        .bind(&owner_type)
        .bind(owner_id)
        .bind(&t.tags)
        .bind(&t.description)
        .bind(t.created_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_template(&self, id: TemplateId) -> Result<Option<VmTemplate>> {
        let row = sqlx::query_as::<_, DbTemplate>("SELECT * FROM vm_templates WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(VmTemplate::from))
    }

    async fn update_template(&self, t: &VmTemplate) -> Result<()> {
        sqlx::query(
            "UPDATE vm_templates SET
                name = $2, version = $3, spec = $4, image_id = $5,
                kernel_image_id = $6, tags = $7, description = $8
             WHERE id = $1",
        )
        .bind(t.id.0)
        .bind(&t.name)
        .bind(t.version as i64)
        .bind(serde_json::to_value(&t.spec).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(t.image_id.map(|i| i.0))
        .bind(t.kernel_image_id.map(|i| i.0))
        .bind(&t.tags)
        .bind(&t.description)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn delete_template(&self, id: TemplateId) -> Result<()> {
        sqlx::query("DELETE FROM vm_templates WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn list_templates(&self, owner: Option<OwnerRef>) -> Result<Vec<VmTemplate>> {
        let rows = if let Some(owner) = owner {
            let (owner_type, owner_id) = Self::owner_params(&owner);
            sqlx::query_as::<_, DbTemplate>(
                "SELECT * FROM vm_templates WHERE owner_type = $1 AND owner_id = $2 ORDER BY id",
            )
            .bind(&owner_type)
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        } else {
            sqlx::query_as::<_, DbTemplate>("SELECT * FROM vm_templates ORDER BY id")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?
        };
        Ok(rows.into_iter().map(VmTemplate::from).collect())
    }

    // ===== Node =====
    async fn create_node(&self, n: &Node) -> Result<()> {
        let (owner_type, owner_id) = Self::owner_params(&n.owner);
        sqlx::query(
            "INSERT INTO vm_nodes
             (name, cell_id, cpu_millis, memory_bytes, disk_bytes,
              capabilities_vmm, capabilities_arch, nested_virt, gpu_passthrough,
              max_vcpus, max_memory_bytes, alloc_cpu_millis, alloc_memory_bytes,
              alloc_disk_bytes, running_vms, phase, last_heartbeat_ms,
              taints, labels, owner_type, owner_id, conditions,
              created_at_ms, updated_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24)",
        )
        .bind(&n.name)
        .bind(n.cell.map(|c| c.0))
        .bind(n.spec.cpu_millis as i64)
        .bind(n.spec.memory_bytes as i64)
        .bind(n.spec.disk_bytes as i64)
        .bind(n.spec.capabilities.vmm.iter().map(|v| v.as_str().to_string()).collect::<Vec<_>>())
        .bind(&n.spec.capabilities.arch)
        .bind(n.spec.capabilities.nested_virt)
        .bind(n.spec.capabilities.gpu_passthrough)
        .bind(n.spec.capabilities.max_vcpus as i32)
        .bind(n.spec.capabilities.max_memory_bytes as i64)
        .bind(n.status.alloc_cpu_millis as i64)
        .bind(n.status.alloc_memory_bytes as i64)
        .bind(n.status.alloc_disk_bytes as i64)
        .bind(n.status.running_vms as i32)
        .bind(match n.status.phase {
            gita_vm_api::node::NodePhase::Joining => "joining",
            gita_vm_api::node::NodePhase::Online => "online",
            gita_vm_api::node::NodePhase::Maintenance => "maintenance",
            gita_vm_api::node::NodePhase::Draining => "draining",
            gita_vm_api::node::NodePhase::Offline => "offline",
        })
        .bind(n.status.last_heartbeat_ms)
        .bind(serde_json::to_value(&n.taints).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(serde_json::to_value(&n.labels).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(&owner_type)
        .bind(owner_id)
        .bind(serde_json::to_value(&n.conditions).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(n.created_at_ms)
        .bind(n.updated_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_node(&self, id: NodeId) -> Result<Option<Node>> {
        let row = sqlx::query_as::<_, DbNode>("SELECT * FROM vm_nodes WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(Node::from))
    }

    async fn update_node(&self, n: &Node) -> Result<()> {
        sqlx::query(
            "UPDATE vm_nodes SET
                cpu_millis = $2, memory_bytes = $3, disk_bytes = $4,
                capabilities_vmm = $5, capabilities_arch = $6,
                nested_virt = $7, gpu_passthrough = $8, max_vcpus = $9,
                max_memory_bytes = $10,
                alloc_cpu_millis = $11, alloc_memory_bytes = $12,
                alloc_disk_bytes = $13, running_vms = $14, phase = $15,
                last_heartbeat_ms = $16, taints = $17, labels = $18,
                conditions = $19, updated_at_ms = $20
             WHERE id = $1",
        )
        .bind(n.id.0)
        .bind(n.spec.cpu_millis as i64)
        .bind(n.spec.memory_bytes as i64)
        .bind(n.spec.disk_bytes as i64)
        .bind(
            n.spec
                .capabilities
                .vmm
                .iter()
                .map(|v| v.as_str().to_string())
                .collect::<Vec<_>>(),
        )
        .bind(&n.spec.capabilities.arch)
        .bind(n.spec.capabilities.nested_virt)
        .bind(n.spec.capabilities.gpu_passthrough)
        .bind(n.spec.capabilities.max_vcpus as i32)
        .bind(n.spec.capabilities.max_memory_bytes as i64)
        .bind(n.status.alloc_cpu_millis as i64)
        .bind(n.status.alloc_memory_bytes as i64)
        .bind(n.status.alloc_disk_bytes as i64)
        .bind(n.status.running_vms as i32)
        .bind(match n.status.phase {
            gita_vm_api::node::NodePhase::Joining => "joining",
            gita_vm_api::node::NodePhase::Online => "online",
            gita_vm_api::node::NodePhase::Maintenance => "maintenance",
            gita_vm_api::node::NodePhase::Draining => "draining",
            gita_vm_api::node::NodePhase::Offline => "offline",
        })
        .bind(n.status.last_heartbeat_ms)
        .bind(serde_json::to_value(&n.taints).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(serde_json::to_value(&n.labels).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(serde_json::to_value(&n.conditions).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(gita_vm_core::now_ms())
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn delete_node(&self, id: NodeId) -> Result<()> {
        sqlx::query("DELETE FROM vm_nodes WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn list_nodes(&self, owner: Option<OwnerRef>) -> Result<Vec<Node>> {
        let rows = if let Some(owner) = owner {
            let (owner_type, owner_id) = Self::owner_params(&owner);
            sqlx::query_as::<_, DbNode>(
                "SELECT * FROM vm_nodes WHERE owner_type = $1 AND owner_id = $2 ORDER BY id",
            )
            .bind(&owner_type)
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        } else {
            sqlx::query_as::<_, DbNode>("SELECT * FROM vm_nodes ORDER BY id")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?
        };
        Ok(rows.into_iter().map(Node::from).collect())
    }

    // ===== Image =====
    async fn create_image(&self, img: &Image) -> Result<()> {
        let (owner_type, owner_id) = Self::owner_params(&img.owner);
        sqlx::query(
            "INSERT INTO vm_images
             (name, kind, status, pool_id, path, size_bytes, digest, format, arch,
              owner_type, owner_id, tags, metadata, created_at_ms, last_used_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(&img.name)
        .bind(match img.kind {
            gita_vm_api::image::ImageKind::Kernel => "kernel",
            gita_vm_api::image::ImageKind::CloudInit => "cloud-init",
            gita_vm_api::image::ImageKind::Rootfs => "rootfs",
        })
        .bind(match img.status {
            gita_vm_api::image::ImageStatus::Ready => "ready",
            gita_vm_api::image::ImageStatus::Failed => "failed",
            gita_vm_api::image::ImageStatus::Deleting => "deleting",
            gita_vm_api::image::ImageStatus::Importing => "importing",
        })
        .bind(img.pool_id.0 as i64)
        .bind(&img.path)
        .bind(img.size_bytes as i64)
        .bind(&img.digest)
        .bind(&img.format)
        .bind(&img.arch)
        .bind(&owner_type)
        .bind(owner_id)
        .bind(&img.tags)
        .bind(serde_json::to_value(&img.metadata).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(img.created_at_ms)
        .bind(img.last_used_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_image(&self, id: ImageId) -> Result<Option<Image>> {
        let row = sqlx::query_as::<_, DbImage>("SELECT * FROM vm_images WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(Image::from))
    }

    async fn update_image(&self, img: &Image) -> Result<()> {
        sqlx::query(
            "UPDATE vm_images SET
                name = $2, kind = $3, status = $4, pool_id = $5, path = $6,
                size_bytes = $7, digest = $8, format = $9, arch = $10,
                tags = $11, metadata = $12, last_used_at_ms = $13
             WHERE id = $1",
        )
        .bind(img.id.0)
        .bind(&img.name)
        .bind(match img.kind {
            gita_vm_api::image::ImageKind::Kernel => "kernel",
            gita_vm_api::image::ImageKind::CloudInit => "cloud-init",
            gita_vm_api::image::ImageKind::Rootfs => "rootfs",
        })
        .bind(match img.status {
            gita_vm_api::image::ImageStatus::Ready => "ready",
            gita_vm_api::image::ImageStatus::Failed => "failed",
            gita_vm_api::image::ImageStatus::Deleting => "deleting",
            gita_vm_api::image::ImageStatus::Importing => "importing",
        })
        .bind(img.pool_id.0 as i64)
        .bind(&img.path)
        .bind(img.size_bytes as i64)
        .bind(&img.digest)
        .bind(&img.format)
        .bind(&img.arch)
        .bind(&img.tags)
        .bind(serde_json::to_value(&img.metadata).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(img.last_used_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn delete_image(&self, id: ImageId) -> Result<()> {
        sqlx::query("DELETE FROM vm_images WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn list_images(&self, owner: Option<OwnerRef>) -> Result<Vec<Image>> {
        let rows = if let Some(owner) = owner {
            let (owner_type, owner_id) = Self::owner_params(&owner);
            sqlx::query_as::<_, DbImage>(
                "SELECT * FROM vm_images WHERE owner_type = $1 AND owner_id = $2 ORDER BY id",
            )
            .bind(&owner_type)
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        } else {
            sqlx::query_as::<_, DbImage>("SELECT * FROM vm_images ORDER BY id")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?
        };
        Ok(rows.into_iter().map(Image::from).collect())
    }

    // ===== Network =====
    async fn create_network(&self, net: &Network) -> Result<()> {
        let (owner_type, owner_id) = Self::owner_params(&net.owner);
        sqlx::query(
            "INSERT INTO vm_networks
             (name, provider, bridge, cidr, gateway, dhcp_start, dhcp_end, mtu, vlan,
              owner_type, owner_id, is_default, created_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        )
        .bind(&net.name)
        .bind(match net.provider {
            gita_vm_api::network::NetProvider::Bridge => "bridge",
            gita_vm_api::network::NetProvider::MacVTap => "macvtap",
            gita_vm_api::network::NetProvider::Passt => "passt",
            gita_vm_api::network::NetProvider::Cni => "cni",
        })
        .bind(&net.bridge)
        .bind(&net.cidr)
        .bind(&net.gateway)
        .bind(&net.dhcp_start)
        .bind(&net.dhcp_end)
        .bind(net.mtu as i32)
        .bind(net.vlan.map(|v| v as i16))
        .bind(&owner_type)
        .bind(owner_id)
        .bind(net.is_default)
        .bind(net.created_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_network(&self, id: NetworkId) -> Result<Option<Network>> {
        let row = sqlx::query_as::<_, DbNetwork>("SELECT * FROM vm_networks WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(Network::from))
    }

    async fn update_network(&self, net: &Network) -> Result<()> {
        sqlx::query(
            "UPDATE vm_networks SET
                name = $2, provider = $3, bridge = $4, cidr = $5, gateway = $6,
                dhcp_start = $7, dhcp_end = $8, mtu = $9, vlan = $10, is_default = $11
             WHERE id = $1",
        )
        .bind(net.id.0)
        .bind(&net.name)
        .bind(match net.provider {
            gita_vm_api::network::NetProvider::Bridge => "bridge",
            gita_vm_api::network::NetProvider::MacVTap => "macvtap",
            gita_vm_api::network::NetProvider::Passt => "passt",
            gita_vm_api::network::NetProvider::Cni => "cni",
        })
        .bind(&net.bridge)
        .bind(&net.cidr)
        .bind(&net.gateway)
        .bind(&net.dhcp_start)
        .bind(&net.dhcp_end)
        .bind(net.mtu as i32)
        .bind(net.vlan.map(|v| v as i16))
        .bind(net.is_default)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn delete_network(&self, id: NetworkId) -> Result<()> {
        sqlx::query("DELETE FROM vm_networks WHERE id = $1")
            .bind(id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn list_networks(&self, owner: Option<OwnerRef>) -> Result<Vec<Network>> {
        let rows = if let Some(owner) = owner {
            let (owner_type, owner_id) = Self::owner_params(&owner);
            sqlx::query_as::<_, DbNetwork>(
                "SELECT * FROM vm_networks WHERE owner_type = $1 AND owner_id = $2 ORDER BY id",
            )
            .bind(&owner_type)
            .bind(owner_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        } else {
            sqlx::query_as::<_, DbNetwork>("SELECT * FROM vm_networks ORDER BY id")
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?
        };
        Ok(rows.into_iter().map(Network::from).collect())
    }

    async fn get_default_network(&self) -> Result<Option<Network>> {
        let row = sqlx::query_as::<_, DbNetwork>(
            "SELECT * FROM vm_networks WHERE is_default = TRUE LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(Network::from))
    }

    // ===== IP Lease =====
    async fn create_ip_lease(&self, lease: &IpLease) -> Result<i64> {
        let row = sqlx::query(
            "INSERT INTO vm_ip_leases
             (network_id, ip, mac, vm_id, node_id, state, expires_at_ms, created_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             RETURNING id",
        )
        .bind(lease.network_id.0)
        .bind(&lease.ip)
        .bind(&lease.mac)
        .bind(lease.vm_id.map(|v| v.0))
        .bind(lease.node_id.map(|n| n.0))
        .bind(match lease.state {
            gita_vm_api::network::LeaseState::Allocated => "allocated",
            gita_vm_api::network::LeaseState::Reserved => "reserved",
            gita_vm_api::network::LeaseState::Released => "released",
            gita_vm_api::network::LeaseState::Expired => "expired",
        })
        .bind(lease.expires_at_ms)
        .bind(lease.created_at_ms)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.try_get("id").unwrap_or(0))
    }

    async fn get_ip_lease_by_ip(&self, ip: &str) -> Result<Option<IpLease>> {
        let row = sqlx::query_as::<_, DbIpLease>("SELECT * FROM vm_ip_leases WHERE ip = $1")
            .bind(ip)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(IpLease::from))
    }

    async fn get_ip_lease_by_vm(&self, vm_id: VmId) -> Result<Option<IpLease>> {
        let row = sqlx::query_as::<_, DbIpLease>("SELECT * FROM vm_ip_leases WHERE vm_id = $1")
            .bind(vm_id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(IpLease::from))
    }

    async fn get_ip_lease_by_id(&self, id: i64) -> Result<Option<IpLease>> {
        let row = sqlx::query_as::<_, DbIpLease>("SELECT * FROM vm_ip_leases WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(IpLease::from))
    }

    async fn update_ip_lease(&self, lease: &IpLease) -> Result<()> {
        sqlx::query(
            "UPDATE vm_ip_leases SET
                network_id = $2, ip = $3, mac = $4, vm_id = $5, node_id = $6,
                state = $7, expires_at_ms = $8
             WHERE id = $1",
        )
        .bind(lease.id)
        .bind(lease.network_id.0)
        .bind(&lease.ip)
        .bind(&lease.mac)
        .bind(lease.vm_id.map(|v| v.0))
        .bind(lease.node_id.map(|n| n.0))
        .bind(match lease.state {
            gita_vm_api::network::LeaseState::Allocated => "allocated",
            gita_vm_api::network::LeaseState::Reserved => "reserved",
            gita_vm_api::network::LeaseState::Released => "released",
            gita_vm_api::network::LeaseState::Expired => "expired",
        })
        .bind(lease.expires_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn release_ip_lease_for_vm(&self, vm_id: VmId) -> Result<()> {
        sqlx::query(
            "UPDATE vm_ip_leases SET state = 'released', vm_id = NULL,
                expires_at_ms = $2
             WHERE vm_id = $1",
        )
        .bind(vm_id.0)
        .bind(gita_vm_core::now_ms())
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn list_ip_leases(&self, network_id: NetworkId) -> Result<Vec<IpLease>> {
        let rows = sqlx::query_as::<_, DbIpLease>(
            "SELECT * FROM vm_ip_leases WHERE network_id = $1 ORDER BY ip",
        )
        .bind(network_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(rows.into_iter().map(IpLease::from).collect())
    }

    // ===== Operation =====
    async fn create_operation(&self, op: &Operation) -> Result<()> {
        let row = sqlx::query(
            "INSERT INTO vm_operations
             (kind, vm_id, node_id, steps, current_step, status, priority,
              created_at_ms, started_at_ms, finished_at_ms, error)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             RETURNING id",
        )
        .bind(
            serde_json::to_value(&op.kind)
                .map_err(|e| Error::Storage(e.to_string()))?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "create_vm".into()),
        )
        .bind(op.vm_id.0)
        .bind(op.node_id.map(|n| n.0))
        .bind(serde_json::to_value(&op.steps).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(op.current_step as i32)
        .bind(
            serde_json::to_value(&op.status)
                .map_err(|e| Error::Storage(e.to_string()))?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "pending".into()),
        )
        .bind(op.priority)
        .bind(op.created_at_ms)
        .bind(op.started_at_ms)
        .bind(op.finished_at_ms)
        .bind(&op.error)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        let id: i64 = row.try_get("id").unwrap_or(0);
        // Operation 的 id 是 pub 字段，但这里 op 是 & 引用——调用方需要 ID 时应重新查询
        let _ = id;
        Ok(())
    }

    async fn get_operation(&self, id: OperationId) -> Result<Option<Operation>> {
        let row = sqlx::query_as::<_, DbOperation>("SELECT * FROM vm_operations WHERE id = $1")
            .bind(id.0)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(Operation::from))
    }

    async fn update_operation(&self, op: &Operation) -> Result<()> {
        sqlx::query(
            "UPDATE vm_operations SET
                node_id = $2, steps = $3, current_step = $4, status = $5,
                started_at_ms = $6, finished_at_ms = $7, error = $8, priority = $9
             WHERE id = $1",
        )
        .bind(op.id.0)
        .bind(op.node_id.map(|n| n.0))
        .bind(serde_json::to_value(&op.steps).map_err(|e| Error::Storage(e.to_string()))?)
        .bind(op.current_step as i32)
        .bind(
            serde_json::to_value(&op.status)
                .map_err(|e| Error::Storage(e.to_string()))?
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "pending".into()),
        )
        .bind(op.started_at_ms)
        .bind(op.finished_at_ms)
        .bind(&op.error)
        .bind(op.priority)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn list_operations(&self, node_id: Option<NodeId>) -> Result<Vec<Operation>> {
        let rows = if let Some(node_id) = node_id {
            sqlx::query_as::<_, DbOperation>(
                "SELECT * FROM vm_operations WHERE node_id = $1 ORDER BY created_at_ms DESC",
            )
            .bind(node_id.0)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        } else {
            sqlx::query_as::<_, DbOperation>(
                "SELECT * FROM vm_operations ORDER BY created_at_ms DESC",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        };
        Ok(rows.into_iter().map(Operation::from).collect())
    }

    async fn list_pending_operations(
        &self,
        node_id: Option<NodeId>,
        limit: usize,
    ) -> Result<Vec<Operation>> {
        let rows = if let Some(node_id) = node_id {
            sqlx::query_as::<_, DbOperation>(
                "SELECT * FROM vm_operations
                 WHERE status = 'pending' AND (node_id = $1 OR node_id IS NULL)
                 ORDER BY priority DESC, created_at_ms LIMIT $2",
            )
            .bind(node_id.0)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        } else {
            sqlx::query_as::<_, DbOperation>(
                "SELECT * FROM vm_operations WHERE status = 'pending'
                 ORDER BY priority DESC, created_at_ms LIMIT $1",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?
        };
        Ok(rows.into_iter().map(Operation::from).collect())
    }

    // ===== Quota =====
    async fn create_quota(&self, q: &Quota) -> Result<()> {
        sqlx::query(
            "INSERT INTO vm_quotas
             (scope, scope_id, max_concurrent_vms, max_cpu_millis, max_memory_bytes,
              max_disk_bytes, max_vm_lifetime_ms, created_at_ms, updated_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(match q.scope {
            QuotaScope::System => "system",
            QuotaScope::Group => "group",
            QuotaScope::Repo => "repo",
            QuotaScope::User => "user",
        })
        .bind(q.scope_id)
        .bind(q.max_concurrent_vms as i32)
        .bind(q.max_cpu_millis as i64)
        .bind(q.max_memory_bytes as i64)
        .bind(q.max_disk_bytes as i64)
        .bind(q.max_vm_lifetime_ms)
        .bind(q.created_at_ms)
        .bind(q.updated_at_ms)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_quota(&self, scope: QuotaScope, scope_id: i64) -> Result<Option<Quota>> {
        let scope_str = match scope {
            QuotaScope::System => "system",
            QuotaScope::Group => "group",
            QuotaScope::Repo => "repo",
            QuotaScope::User => "user",
        };
        let row = sqlx::query_as::<_, DbQuota>(
            "SELECT * FROM vm_quotas WHERE scope = $1 AND scope_id = $2",
        )
        .bind(scope_str)
        .bind(scope_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(row.map(Quota::from))
    }

    async fn update_quota(&self, q: &Quota) -> Result<()> {
        sqlx::query(
            "UPDATE vm_quotas SET
                max_concurrent_vms = $3, max_cpu_millis = $4, max_memory_bytes = $5,
                max_disk_bytes = $6, max_vm_lifetime_ms = $7, updated_at_ms = $8
             WHERE scope = $1 AND scope_id = $2",
        )
        .bind(match q.scope {
            QuotaScope::System => "system",
            QuotaScope::Group => "group",
            QuotaScope::Repo => "repo",
            QuotaScope::User => "user",
        })
        .bind(q.scope_id)
        .bind(q.max_concurrent_vms as i32)
        .bind(q.max_cpu_millis as i64)
        .bind(q.max_memory_bytes as i64)
        .bind(q.max_disk_bytes as i64)
        .bind(q.max_vm_lifetime_ms)
        .bind(gita_vm_core::now_ms())
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn delete_quota(&self, scope: QuotaScope, scope_id: i64) -> Result<()> {
        sqlx::query("DELETE FROM vm_quotas WHERE scope = $1 AND scope_id = $2")
            .bind(match scope {
                QuotaScope::System => "system",
                QuotaScope::Group => "group",
                QuotaScope::Repo => "repo",
                QuotaScope::User => "user",
            })
            .bind(scope_id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(())
    }

    async fn list_quotas(&self) -> Result<Vec<Quota>> {
        let rows = sqlx::query_as::<_, DbQuota>("SELECT * FROM vm_quotas ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(rows.into_iter().map(Quota::from).collect())
    }

    async fn get_quota_usage(&self, scope: QuotaScope, scope_id: i64) -> Result<QuotaUsage> {
        // 根据范围统计当前活跃 VM 的资源占用
        let rows: Vec<(Option<i64>, Option<i64>, Option<i64>, Option<i64>)> = match scope {
            QuotaScope::System => {
                sqlx::query_as(
                    "SELECT COUNT(*),
                            COALESCE(SUM((spec->>'cpu_millis')::bigint), 0),
                            COALESCE(SUM((spec->>'memory_bytes')::bigint), 0),
                            COALESCE(SUM((spec->>'disk_bytes')::bigint), 0)
                     FROM vm_instances
                     WHERE status IN ('pending', 'provisioning', 'running', 'stopping', 'migrating')",
                )
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?
            }
            QuotaScope::Group => {
                sqlx::query_as(
                    "SELECT COUNT(*),
                            COALESCE(SUM((spec->>'cpu_millis')::bigint), 0),
                            COALESCE(SUM((spec->>'memory_bytes')::bigint), 0),
                            COALESCE(SUM((spec->>'disk_bytes')::bigint), 0)
                     FROM vm_instances
                     WHERE owner_type = 'group' AND owner_id = $1
                       AND status IN ('pending', 'provisioning', 'running', 'stopping', 'migrating')",
                )
                .bind(scope_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?
            }
            QuotaScope::Repo => {
                sqlx::query_as(
                    "SELECT COUNT(*),
                            COALESCE(SUM((spec->>'cpu_millis')::bigint), 0),
                            COALESCE(SUM((spec->>'memory_bytes')::bigint), 0),
                            COALESCE(SUM((spec->>'disk_bytes')::bigint), 0)
                     FROM vm_instances
                     WHERE owner_type = 'repo' AND owner_id = $1
                       AND status IN ('pending', 'provisiooning', 'running', 'stopping', 'migrating')",
                )
                .bind(scope_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| Error::Storage(e.to_string()))?
            }
            QuotaScope::User => {
                // 用户范围暂不支持（需要 user_id 关联），返回 0
                return Ok(QuotaUsage::default());
            }
        };

        let (count, cpu, mem, disk) =
            rows.first()
                .cloned()
                .unwrap_or((Some(0), Some(0), Some(0), Some(0)));
        Ok(QuotaUsage {
            concurrent_vms: count.unwrap_or(0) as u32,
            used_cpu_millis: cpu.unwrap_or(0) as u64,
            used_memory_bytes: mem.unwrap_or(0) as u64,
            used_disk_bytes: disk.unwrap_or(0) as u64,
        })
    }

    // ===== Event =====
    async fn append_event(&self, _event: &gita_vm_api::Event) -> Result<()> {
        // 事件由 gita-events 统一管理，VM 模块通过 gita-db 的事件表写入。
        // 此处暂为空实现，控制面直接使用 gita_events::EventBus。
        Ok(())
    }

    async fn list_events(
        &self,
        _repo_id: Option<i64>,
        _event_type: Option<&str>,
        _limit: i64,
        _offset: i64,
    ) -> Result<Vec<gita_vm_api::Event>> {
        Ok(vec![])
    }
}
