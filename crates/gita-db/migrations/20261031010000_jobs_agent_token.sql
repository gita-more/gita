-- 0032_vm_agent_jobs.sql — VM 执行的 job 凭证。
--
-- 一个 `vm:` job 不在 runner 宿主机上执行：runner 创建一台虚拟机，
-- 把执行材料（仓库、提交、脚本）通过 cloud-init 注入，VM 里的 agent
-- 跑完脚本后把日志和结果回报到服务器。
--
-- agent_token 存的是凭证的 SHA-256 哈希（与 access_tokens 同一套做法）：
-- 明文只出现在注入 VM 的 user data 里。VM 专用：本地执行的 job 此列为
-- NULL，回报端点会拒绝。
ALTER TABLE jobs ADD COLUMN agent_token TEXT;
