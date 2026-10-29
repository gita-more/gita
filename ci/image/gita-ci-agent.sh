#!/usr/bin/env bash
# =============================================================================
# gita-ci-agent.sh — 运行在 gita CI VM 内的 agent
#
# 开机自启（由 build-rootfs.sh 安装为 systemd 服务），主动轮询 CI 编排器：
#   - 领取 job（repo URL、commit、命令）
#   - 执行
#   - 上报结果
#
# 这是 GitHub runner 的"runner"角色。默认从环境变量/参数读取编排器地址。
#
# 用法（在 VM 内）：
#   GITA_CI_ORCHESTRATOR_URL=http://<orchestrator>:<port> gita-ci-agent.sh
#
# 本脚本是模板；实际协议需与 gita 侧的编排器对齐。
# =============================================================================
set -euo pipefail

ORCHESTRATOR_URL="${GITA_CI_ORCHESTRATOR_URL:-}"
AGENT_ID="$(hostname)"
POLL_INTERVAL="${GITA_CI_POLL_INTERVAL:-5}"

log() { echo "[gita-ci-agent:$AGENT_ID] $*"; }

[[ -n "$ORCHESTRATOR_URL" ]] || {
  log "错误: 未设置 GITA_CI_ORCHESTRATOR_URL" >&2
  exit 1
}

# ---- 上报就绪 ----
report_ready() {
  curl -fsS -X POST "$ORCHESTRATOR_URL/agents/register" \
    -H 'Content-Type: application/json' \
    -d "{\"agent_id\":\"$AGENT_ID\",\"status\":\"ready\"}" >/dev/null 2>&1 \
    && log "已向编排器注册" || log "注册失败（编排器可能未就绪，稍后重试）"
}

# ---- 领取并执行 job ----
run_job() {
  local job_id="$1" repo_url="$2" commit="$3" cmd="$4"
  log "领取 job $job_id: $repo_url @ $commit"

  # 上报 running
  curl -fsS -X POST "$ORCHESTRATOR_URL/agents/$AGENT_ID/jobs/$job_id/status" \
    -H 'Content-Type: application/json' -d '{"status":"running"}' >/dev/null 2>&1 || true

  # 准备目录
  local workdir="/home/ci/work/$job_id"
  mkdir -p "$workdir"

  # clone + checkout
  ( cd "$workdir" \
    && git clone "$repo_url" repo \
    && cd repo \
    && git checkout "$commit" )

  # 执行命令，收集退出码
  local rc=0
  ( cd "$workdir/repo" && bash -c "$cmd" ) || rc=$?

  # 上报结果
  if [[ $rc -eq 0 ]]; then
    curl -fsS -X POST "$ORCHESTRATOR_URL/agents/$AGENT_ID/jobs/$job_id/status" \
      -H 'Content-Type: application/json' -d '{"status":"succeeded"}' >/dev/null 2>&1 || true
    log "job $job_id 成功"
  else
    curl -fsS -X POST "$ORCHESTRATOR_URL/agents/$AGENT_ID/jobs/$job_id/status" \
      -H 'Content-Type: application/json' -d "{\"status\":\"failed\",\"exit_code\":$rc}" >/dev/null 2>&1 || true
    log "job $job_id 失败 (rc=$rc)"
  fi
}

# ---- 主循环 ----
log "gita-ci-agent 启动，编排器: $ORCHESTRATOR_URL"

while true; do
  report_ready

  # 领取 job（假设编排器有 /agents/{id}/jobs/next 端点，返回 job 或空）
  JOB_JSON=$(curl -fsS "$ORCHESTRATOR_URL/agents/$AGENT_ID/jobs/next" 2>/dev/null || echo "")

  if [[ -n "$JOB_JSON" ]] && [[ "$JOB_JSON" != "null" ]] && [[ "$JOB_JSON" != "{}" ]]; then
    JOB_ID=$(echo "$JOB_JSON" | jq -r '.job_id // empty')
    REPO_URL=$(echo "$JOB_JSON" | jq -r '.repo_url // empty')
    COMMIT=$(echo "$JOB_JSON" | jq -r '.commit // empty')
    CMD=$(echo "$JOB_JSON" | jq -r '.command // empty')
    if [[ -n "$JOB_ID" ]] && [[ -n "$REPO_URL" ]] && [[ -n "$COMMIT" ]]; then
      run_job "$JOB_ID" "$REPO_URL" "$COMMIT" "$CMD"
    fi
  fi

  sleep "$POLL_INTERVAL"
done