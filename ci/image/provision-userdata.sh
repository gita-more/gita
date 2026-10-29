#!/usr/bin/env bash
# =============================================================================
# provision-userdata.sh — 生成每个 CI job 的 user_data（注入 VM guest）
#
# gita 镜像模板是"半成品"；每个 job 的差异（checkout 哪个 commit、跑什么命令、
# 用什么 SSH 公钥）通过 user_data 在启动时注入。
#
# 输出：一份 cloud-init 风格的 user-data，可通过 rcluster
#   POST /api/v1/vms 的 user_data 字段传入（Firecracker 走 MMDS）。
#
# 用法：
#   ./provision-userdata.sh \
#     --repo-url git@github.com:gita/gita.git \
#     --commit <sha> \
#     --ssh-private-key /path/to/deploy.key \
#     --test-command "cargo test --workspace" \
#     > userdata.yaml
#
# 依赖：cloud-init 在 guest 内运行（gita 镜像已预装）。
# =============================================================================
set -euo pipefail

# ---- 参数 ----
REPO_URL=""
COMMIT=""
SSH_PRIVATE_KEY_PATH=""
TEST_COMMAND="cargo test --workspace"
DATABASE_URL="postgres://gita:gita@localhost:55432/gita"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo-url) REPO_URL="$2"; shift 2 ;;
    --commit) COMMIT="$2"; shift 2 ;;
    --ssh-private-key) SSH_PRIVATE_KEY_PATH="$2"; shift 2 ;;
    --test-command) TEST_COMMAND="$2"; shift 2 ;;
    --database-url) DATABASE_URL="$2"; shift 2 ;;
    *) echo "未知参数: $1" >&2; exit 1 ;;
  esac
done

[[ -n "$REPO_URL" ]] || { echo "错误: 必须指定 --repo-url" >&2; exit 1; }
[[ -n "$COMMIT" ]] || { echo "错误: 必须指定 --commit" >&2; exit 1; }

# ---- 读取 SSH 私钥 ----
SSH_KEY_CONTENT=""
if [[ -n "$SSH_PRIVATE_KEY_PATH" ]]; then
  SSH_KEY_CONTENT="$(cat "$SSH_PRIVATE_KEY_PATH")"
fi

# ---- 生成 user-data ----
# 用 cloud-init 在 guest 启动时：
#   1. 把 CI 用户的 SSH 公钥写入 authorized_keys
#   2. clone 指定仓库并 checkout commit
#   3. 启动 postgres 并建 gita 库
#   4. 执行测试命令
cat <<EOF
#cloud-config
package_update: false
package_upgrade: false

write_files:
  - path: /home/ci/deploy.key
    content: |
      $SSH_KEY_CONTENT
    permissions: '0600'
    owner: ci:ci

runcmd:
  - |
    set -euo pipefail
    # 1. 配置 postgres（gita 测试库）
    pg_ctlcluster $(ls /etc/postgresql/ | head -1) main start || true
    su - postgres -c "psql -c \"CREATE USER gita WITH PASSWORD 'gita';\" || true"
    su - postgres -c "psql -c 'CREATE DATABASE gita OWNER gita;' || true"
    # 2. 克隆仓库
    export GIT_SSH_COMMAND="ssh -i /home/ci/deploy.key -o StrictHostKeyChecking=no"
    if [ ! -d /home/ci/gita ]; then
      git clone "$REPO_URL" /home/ci/gita
      cd /home/ci/gita
      git checkout "$COMMIT"
    fi
    # 3. 设置测试环境
    export DATABASE_URL="$DATABASE_URL"
    export GITA_TEST_DATABASE_URL="$DATABASE_URL"
    cd /home/ci/gita
    # 4. 跑测试
    $TEST_COMMAND

final_message: "gita-ci-job complete"
EOF

echo "[gita-image] user-data 已生成（写向 stdout）" >&2