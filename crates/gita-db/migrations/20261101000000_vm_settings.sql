-- 控制面运行期持久化的集群设置（k/v）。
--
-- 目前只有一个键：`vm_node_api_key` —— 节点代理访问 /api/v1/vm/internal/*
-- 的共享密钥。部署未在配置里显式给出时，gita-admin / gita-vm-control
-- 第一次启动会自动生成一个随机值写进这张表，之后保持不变
-- （见 gita-vm-control::internal::resolve_node_api_key）。
CREATE TABLE IF NOT EXISTS vm_settings (
    name           TEXT PRIMARY KEY,
    value          TEXT NOT NULL,
    updated_at_ms  BIGINT NOT NULL
);
