-- gita-vm 初始表结构
-- 与 gita 现有迁移风格一致：时间戳用毫秒 bigint，JSON 用 jsonb，ID 用 bigserial

-- 启用 uuid 扩展（如未启用）
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- ============================================
-- 网络表
-- ============================================
CREATE TABLE vm_networks (
    id              BIGSERIAL PRIMARY KEY,
    name            TEXT NOT NULL,
    provider        TEXT NOT NULL DEFAULT 'bridge',  -- bridge, macvtap, passt, cni
    bridge          TEXT NOT NULL,                    -- 宿主机网桥/接口名
    cidr            TEXT NOT NULL,                    -- 如 "10.42.0.0/24"
    gateway         TEXT NOT NULL,                    -- 如 "10.42.0.1"
    dhcp_start      TEXT,                             -- DHCP 起始 IP
    dhcp_end        TEXT,                             -- DHCP 结束 IP
    mtu             INTEGER NOT NULL DEFAULT 1500,
    vlan            INTEGER,                          -- 可选 VLAN ID
    owner_type      TEXT NOT NULL DEFAULT 'system',   -- system, group, repo
    owner_id        BIGINT NOT NULL DEFAULT 0,        -- group_id 或 repo_id
    is_default      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at_ms   BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT
);

CREATE INDEX idx_vm_networks_owner ON vm_networks(owner_type, owner_id);
CREATE UNIQUE INDEX idx_vm_networks_default ON vm_networks(is_default) WHERE is_default = TRUE;

-- ============================================
-- IP 租约表
-- ============================================
CREATE TABLE vm_ip_leases (
    id              BIGSERIAL PRIMARY KEY,
    network_id      BIGINT NOT NULL REFERENCES vm_networks(id) ON DELETE CASCADE,
    ip              TEXT NOT NULL,
    mac             TEXT NOT NULL,
    vm_id           BIGINT,                           -- 关联 VM，释放时置 NULL
    node_id         BIGINT,                           -- 关联节点
    state           TEXT NOT NULL DEFAULT 'allocated', -- allocated, reserved, released, expired
    expires_at_ms   BIGINT,                           -- 租约过期时间
    created_at_ms   BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT
);

CREATE INDEX idx_vm_ip_leases_network ON vm_ip_leases(network_id);
CREATE INDEX idx_vm_ip_leases_vm ON vm_ip_leases(vm_id);
CREATE INDEX idx_vm_ip_leases_ip ON vm_ip_leases(ip);
CREATE UNIQUE INDEX idx_vm_ip_leases_unique_active ON vm_ip_leases(network_id, ip) WHERE state IN ('allocated', 'reserved');

-- ============================================
-- 节点表
-- ============================================
CREATE TABLE vm_nodes (
    id                  BIGSERIAL PRIMARY KEY,
    name                TEXT NOT NULL UNIQUE,
    cell_id             BIGINT,                       -- 预留：多区域/Cell
    -- 规格
    cpu_millis          BIGINT NOT NULL,
    memory_bytes        BIGINT NOT NULL,
    disk_bytes          BIGINT NOT NULL,
    -- 能力
    capabilities_vmm    TEXT[] NOT NULL DEFAULT '{}', -- 支持的 VMM: qemu, cloud-hypervisor, firecracker
    capabilities_arch   TEXT[] NOT NULL DEFAULT '{}', -- 支持的架构
    nested_virt         BOOLEAN NOT NULL DEFAULT FALSE,
    gpu_passthrough     BOOLEAN NOT NULL DEFAULT FALSE,
    max_vcpus           INTEGER NOT NULL DEFAULT 64,
    max_memory_bytes    BIGINT NOT NULL,
    -- 状态（动态）
    alloc_cpu_millis    BIGINT NOT NULL DEFAULT 0,
    alloc_memory_bytes  BIGINT NOT NULL DEFAULT 0,
    alloc_disk_bytes    BIGINT NOT NULL DEFAULT 0,
    running_vms         INTEGER NOT NULL DEFAULT 0,
    phase               TEXT NOT NULL DEFAULT 'joining', -- joining, online, maintenance, draining, offline
    last_heartbeat_ms   BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT,
    -- 污点与标签
    taints              JSONB NOT NULL DEFAULT '[]',    -- [{"key": "k", "value": "v", "effect": "NoSchedule"}]
    labels              JSONB NOT NULL DEFAULT '{}',
    -- 归属
    owner_type          TEXT NOT NULL DEFAULT 'system',
    owner_id            BIGINT NOT NULL DEFAULT 0,
    -- 条件
    conditions          JSONB NOT NULL DEFAULT '[]',
    created_at_ms       BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT,
    updated_at_ms       BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT
);

CREATE INDEX idx_vm_nodes_owner ON vm_nodes(owner_type, owner_id);
CREATE INDEX idx_vm_nodes_phase ON vm_nodes(phase);

-- ============================================
-- 镜像表
-- ============================================
CREATE TABLE vm_images (
    id              BIGSERIAL PRIMARY KEY,
    name            TEXT NOT NULL,
    kind            TEXT NOT NULL DEFAULT 'rootfs', -- rootfs, kernel, cloud-init
    status          TEXT NOT NULL DEFAULT 'importing', -- importing, ready, failed, deleting
    pool_id         BIGINT,                         -- 存储池 ID（预留）
    path            TEXT NOT NULL,                  -- 宿主机文件路径
    size_bytes      BIGINT NOT NULL,
    digest          TEXT NOT NULL,                  -- SHA256 hex
    format          TEXT NOT NULL DEFAULT 'qcow2',  -- qcow2, raw, vmdk, iso
    arch            TEXT NOT NULL DEFAULT 'x86_64',
    owner_type      TEXT NOT NULL DEFAULT 'system',
    owner_id        BIGINT NOT NULL DEFAULT 0,
    tags            TEXT[] NOT NULL DEFAULT '{}',
    metadata        JSONB NOT NULL DEFAULT '{}',
    created_at_ms   BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT,
    last_used_at_ms BIGINT
);

CREATE INDEX idx_vm_images_owner ON vm_images(owner_type, owner_id);
CREATE INDEX idx_vm_images_status ON vm_images(status);
CREATE UNIQUE INDEX idx_vm_images_name_owner ON vm_images(name, owner_type, owner_id);

-- ============================================
-- 模板表
-- ============================================
CREATE TABLE vm_templates (
    id                  BIGSERIAL PRIMARY KEY,
    name                TEXT NOT NULL,
    version             BIGINT NOT NULL DEFAULT 1,
    spec                JSONB NOT NULL,               -- VmSpec 完整序列化
    image_id            BIGINT REFERENCES vm_images(id),
    kernel_image_id     BIGINT REFERENCES vm_images(id),
    owner_type          TEXT NOT NULL DEFAULT 'system',
    owner_id            BIGINT NOT NULL DEFAULT 0,
    tags                TEXT[] NOT NULL DEFAULT '{}',
    description         TEXT NOT NULL DEFAULT '',
    created_at_ms       BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT
);

CREATE INDEX idx_vm_templates_owner ON vm_templates(owner_type, owner_id);
CREATE UNIQUE INDEX idx_vm_templates_name_version_owner ON vm_templates(name, version, owner_type, owner_id);

-- ============================================
-- VM 实例表
-- ============================================
CREATE TABLE vm_instances (
    id                  BIGSERIAL PRIMARY KEY,
    name                TEXT NOT NULL,
    template_id         BIGINT REFERENCES vm_templates(id),
    owner_type          TEXT NOT NULL DEFAULT 'repo',
    owner_id            BIGINT NOT NULL,              -- repo_id (或 group_id)
    repo_id             BIGINT REFERENCES repositories(id), -- 关联 gita 仓库
    pipeline_run_id     BIGINT REFERENCES pipeline_runs(id), -- 关联 CI 运行
    job_id              BIGINT REFERENCES jobs(id),          -- 关联 CI Job
    node_id             BIGINT REFERENCES vm_nodes(id),
    spec                JSONB NOT NULL,               -- VmSpec 完整序列化
    status              TEXT NOT NULL DEFAULT 'pending', -- pending, provisioning, running, stopping, stopped, migrating, failed, quarantined
    generation          BIGINT NOT NULL DEFAULT 1,
    observed_generation BIGINT NOT NULL DEFAULT 1,
    conditions          JSONB NOT NULL DEFAULT '[]',
    rootfs_path         TEXT,                         -- 宿主机 rootfs 路径
    ip_address          TEXT,                         -- 分配的 IP
    console_url         TEXT,                         -- VNC/控制台 URL
    created_at_ms       BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT,
    message             TEXT,
    expires_at_ms       BIGINT                        -- 过期自动回收（CI 临时 VM）
);

CREATE INDEX idx_vm_instances_owner ON vm_instances(owner_type, owner_id);
CREATE INDEX idx_vm_instances_repo ON vm_instances(repo_id);
CREATE INDEX idx_vm_instances_pipeline_run ON vm_instances(pipeline_run_id);
CREATE INDEX idx_vm_instances_job ON vm_instances(job_id);
CREATE INDEX idx_vm_instances_node ON vm_instances(node_id);
CREATE INDEX idx_vm_instances_status ON vm_instances(status);
CREATE INDEX idx_vm_instances_expires ON vm_instances(expires_at_ms) WHERE expires_at_ms IS NOT NULL;

-- ============================================
-- 操作表
-- ============================================
CREATE TABLE vm_operations (
    id                  BIGSERIAL PRIMARY KEY,
    kind                TEXT NOT NULL,                -- create_vm, start_vm, stop_vm, delete_vm, migrate_vm, ...
    vm_id               BIGINT NOT NULL REFERENCES vm_instances(id) ON DELETE CASCADE,
    node_id             BIGINT REFERENCES vm_nodes(id),
    steps               JSONB NOT NULL DEFAULT '[]',  -- OperationStep[]
    current_step        INTEGER NOT NULL DEFAULT 0,
    status              TEXT NOT NULL DEFAULT 'pending', -- pending, running, succeeded, failed, cancelling, cancelled
    priority            INTEGER NOT NULL DEFAULT 0,
    created_at_ms       BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT,
    started_at_ms       BIGINT,
    finished_at_ms      BIGINT,
    error               TEXT
);

CREATE INDEX idx_vm_operations_vm ON vm_operations(vm_id);
CREATE INDEX idx_vm_operations_node ON vm_operations(node_id);
CREATE INDEX idx_vm_operations_status ON vm_operations(status);

-- ============================================
-- 配额表
-- ============================================
CREATE TABLE vm_quotas (
    id                      BIGSERIAL PRIMARY KEY,
    scope                   TEXT NOT NULL,              -- system, group, repo, user
    scope_id                BIGINT NOT NULL,
    max_concurrent_vms      INTEGER NOT NULL DEFAULT 10,
    max_cpu_millis          BIGINT NOT NULL DEFAULT 40000,
    max_memory_bytes        BIGINT NOT NULL DEFAULT 64000000000,
    max_disk_bytes          BIGINT NOT NULL DEFAULT 200000000000,
    max_vm_lifetime_ms      BIGINT NOT NULL DEFAULT 0,  -- 0 = 无限制
    created_at_ms           BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT,
    updated_at_ms           BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT
);

CREATE UNIQUE INDEX idx_vm_quotas_scope ON vm_quotas(scope, scope_id);

-- ============================================
-- 事件表（复用 gita events 表，这里只记录 VM 相关事件类型）
-- gita 已有 events 表，VM 事件直接写入该表，event_type 使用 vm.* 前缀
-- ============================================