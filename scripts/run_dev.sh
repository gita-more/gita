#!/usr/bin/env bash
# =============================================================================
# Gita 本地开发服务管理脚本
#
# 通过 Docker/Podman 在本地启动 Gita 完整栈（后端 + 前端 + Postgres）：
#   start    启动服务（首次会自动构建；--build 强制重新构建）
#   stop     停止服务
#   restart  重启服务（--build 强制重新构建镜像）
#   status   查看容器状态
#   logs     跟踪查看日志（-f 跟随）
#   rebuild  重新构建前端 + 后端 + Docker 镜像并重启
#
# 仅用于本地开发/集成测试环境；不替代 CI/CD 的正式部署流程。
#
# 兼容 Docker 与 rootless Podman：
#   start    启动服务（首次会自动构建；--build 强制重新构建）
#   stop     停止服务
#   restart  重启服务（--build 强制重新构建镜像）
#   status   查看容器状态
#   logs     跟踪查看日志（-f 跟随）
#   rebuild  重新构建前端 + 后端 + Docker 镜像并重启
#
# 兼容 Docker 与 rootless Podman：
#   - 若检测到 DOCKER_HOST 指向 podman.sock，则自动使用它
#   - docker-compose v1 与 podman-compose 均可工作
#
# 用法示例：
#   ./scripts/run_dev.sh start
#   ./scripts/run_dev.sh start --build
#   ./scripts/run_dev.sh restart
#   ./scripts/run_dev.sh restart --build
#   ./scripts/run_dev.sh stop
#   ./scripts/run_dev.sh status
#   ./scripts/run_dev.sh logs -f
#   ./scripts/run_dev.sh rebuild
# =============================================================================

set -euo pipefail

# 脚本所在目录（项目根目录）
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${SCRIPT_DIR}"

# compose 文件固定在 docker/ 下，相对 build 上下文（项目根）保持 "./"
COMPOSE_FILE="${SCRIPT_DIR}/docker/docker-compose.yml"

# 颜色输出（仅 tty 时启用）
if [[ -t 1 ]]; then
  C_GREEN=$'\033[32m'; C_YELLOW=$'\033[33m'; C_RED=$'\033[31m'
  C_CYAN=$'\033[36m'; C_BOLD=$'\033[1m'; C_RESET=$'\033[0m'
else
  C_GREEN=""; C_YELLOW=""; C_RED=""; C_CYAN=""; C_BOLD=""; C_RESET=""
fi

info()  { echo -e "${C_CYAN}[gita]${C_RESET} $*"; }
ok()    { echo -e "${C_GREEN}[gita]${C_RESET} $*"; }
warn()  { echo -e "${C_YELLOW}[gita]${C_RESET} $*"; }
err()   { echo -e "${C_RED}[gita]${C_RESET} $*" >&2; }

# ---------------------------------------------------------------------------
# 环境检测
# ---------------------------------------------------------------------------

detect_compose() {
  # 优先 docker-compose v1；其次 podman-compose；再其次 docker compose 插件
  if command -v docker-compose >/dev/null 2>&1; then
    COMPOSE="docker-compose"
  elif command -v podman-compose >/dev/null 2>&1; then
    COMPOSE="podman-compose"
  elif docker compose version >/dev/null 2>&1; then
    COMPOSE="docker compose"
  else
    err "未找到可用的 compose 命令（需要 docker-compose / podman-compose / docker compose 之一）"
    exit 1
  fi
  info "使用 compose 命令: ${COMPOSE}"
}

setup_podman_host() {
  # 如果是 rootless Podman（/usr/bin/docker 指向 podman），设置 DOCKER_HOST
  if ! command -v podman >/dev/null 2>&1; then
    return
  fi
  # 已显式设置 DOCKER_HOST 则尊重用户配置
  if [[ -n "${DOCKER_HOST:-}" ]]; then
    return
  fi
  # 检测 docker 是否为 podman 包装脚本
  if [[ -f /usr/bin/docker ]] && grep -q "podman" /usr/bin/docker 2>/dev/null; then
    local sock="${XDG_RUNTIME_DIR:-}/podman/podman.sock"
    if [[ -S "${sock}" ]]; then
      export DOCKER_HOST="unix://${sock}"
      info "检测到 rootless Podman，设置 DOCKER_HOST=${DOCKER_HOST}"
    fi
  fi
}

compose_cmd() {
  # 子命令 list 形式（无选项前缀）：仅追加 -f。其余统一走 up 子命令风格：
  # 对于 `up`/`down`/`logs`/`build`/`ps` 都需要 -f；
  # 这里约定 compose_cmd 收到的 $@ 第一个是子命令，后续是参数 + flags。
  #
  # compose 文件在 docker/，但 .env 必须从项目根读（compose 不会跨目录找），
  # 因此显式 --env-file。只有 .env 存在时才传，避免首次部署没建 .env 时报错。
  local env_args=()
  if [[ -f "${SCRIPT_DIR}/.env" ]]; then
    env_args=(--env-file "${SCRIPT_DIR}/.env")
  fi
  if [[ "${COMPOSE}" == "docker compose" ]]; then
    docker compose -f "${COMPOSE_FILE}" "${env_args[@]}" "$@"
  else
    "${COMPOSE}" -f "${COMPOSE_FILE}" "${env_args[@]}" "$@"
  fi
}

# ---------------------------------------------------------------------------
# CNI 网络修复（rootless Podman + docker-compose v1 兼容层）
#
# 问题背景：docker-compose v1 生成的网络配置是 cniVersion 1.0.0，但
# Podman 3.4.4 自带的 firewall CNI 插件只支持 0.4.0，导致每次 compose
# 重建网络时报 "CNI network "gita_default" not found"。
# 解决办法：把 conflist 的 cniVersion 降为 0.4.0，并移除 firewall 的
# backend 字段（空 backend 在 0.4.0 下会报错）。
# ---------------------------------------------------------------------------

NETWORK_CONFLIST="${XDG_CONFIG_HOME:-${HOME}/.config}/cni/net.d/gita_default.conflist"

fix_network_conflist() {
  [[ -f "${NETWORK_CONFLIST}" ]] || return 0

  # 用 Python 原子地修复 JSON：把 cniVersion 降为 0.4.0，
  # 并移除 firewall 插件的 backend 字段（Podman 3.4.4 不支持）。
  local output
  output=$(python3 - "${NETWORK_CONFLIST}" <<'PYEOF'
import json, sys
path = sys.argv[1]
with open(path, 'r', encoding='utf-8') as f:
    data = json.load(f)
changed = False
if data.get('cniVersion') != '0.4.0':
    data['cniVersion'] = '0.4.0'
    changed = True
for plugin in data.get('plugins', []):
    if plugin.get('type') == 'firewall' and 'backend' in plugin:
        plugin.pop('backend', None)
        changed = True
if changed:
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(data, f, indent=3, ensure_ascii=False)
        f.write('\n')
    print('1')
else:
    print('0')
PYEOF
)
  if [[ "${output}" == "1" ]]; then
    ok "CNI 网络配置已修复 (${NETWORK_CONFLIST})"
  fi
}

# 执行 compose up，若因 CNI 网络版本失败则自动修复并重试一次
compose_up() {
  # 若网络已存在（上次失败残留），先修复再启动
  fix_network_conflist
  if compose_cmd up -d "$@"; then
    return 0
  fi
  warn "compose up 失败，尝试修复 CNI 网络配置后重试..."
  fix_network_conflist
  compose_cmd up -d "$@"
}

# ---------------------------------------------------------------------------
# 构建步骤
# ---------------------------------------------------------------------------

build_frontend() {
  info "构建前端 (npm run build)..."
  if [[ ! -d web ]]; then
    err "未找到 web/ 目录，请确认在项目根目录运行"
    exit 1
  fi
  (cd web && npm run build)
  ok "前端构建完成"
}

build_backend() {
  info "构建后端 (cargo build --release)..."
  if command -v cargo >/dev/null 2>&1; then
    cargo build --release
  else
    err "未找到 cargo，无法构建后端二进制"
    exit 1
  fi
  if [[ ! -f target/release/gita-server ]]; then
    err "后端二进制 target/release/gita-server 不存在，构建失败"
    exit 1
  fi
  ok "后端构建完成"
}

build_image() {
  info "构建 Docker 镜像 (${COMPOSE} build)..."
  compose_cmd build
  ok "镜像构建完成"
}

# ---------------------------------------------------------------------------
# 动作
# ---------------------------------------------------------------------------

cmd_start() {
  local build="${1:-0}"
  if [[ "${build}" == "1" ]]; then
    build_frontend
    build_backend
  fi
  setup_podman_host
  detect_compose

  # 若镜像不存在，或明确要求 --build，则先构建
  local image_missing=0
  if ! compose_cmd images -q gita_gita >/dev/null 2>&1; then
    image_missing=1
  fi
  if [[ "${build}" == "1" || "${image_missing}" == "1" ]]; then
    if [[ "${image_missing}" == "1" ]]; then
      warn "镜像不存在，首次部署将自动构建"
    fi
    build_frontend
    build_backend
    build_image
  fi

  info "启动服务..."
  compose_up
  ok "Gita 已启动"
  print_url
}

cmd_stop() {
  setup_podman_host
  detect_compose
  info "停止服务..."
  compose_cmd down
  ok "Gita 已停止"
}

cmd_restart() {
  local build="${1:-0}"
  if [[ "${build}" == "1" ]]; then
    build_frontend
    build_backend
  fi
  setup_podman_host
  detect_compose
  if [[ "${build}" == "1" ]]; then
    build_image
  fi
  info "重启服务..."
  compose_up --force-recreate
  ok "Gita 已重启"
  print_url
}

cmd_rebuild() {
  setup_podman_host
  detect_compose
  build_frontend
  build_backend
  build_image
  info "重启服务..."
  compose_up --force-recreate
  ok "Gita 已重新构建并启动"
  print_url
}

cmd_status() {
  setup_podman_host
  detect_compose
  compose_cmd ps
}

cmd_logs() {
  setup_podman_host
  detect_compose
  compose_cmd logs "$@"
}

print_url() {
  local port
  port=$(grep -E '^GITA_PORT=' .env 2>/dev/null | cut -d'=' -f2 | tr -d '[:space:]')
  port="${port:-3100}"
  echo
  ok "访问地址: ${C_BOLD}http://127.0.0.1:${port}${C_RESET}"
}

# ---------------------------------------------------------------------------
# 帮助
# ---------------------------------------------------------------------------

usage() {
  cat <<EOF
${C_BOLD}Gita 本地开发服务管理脚本${C_RESET}

用法: ./scripts/run_dev.sh <command> [options]

命令:
  start [--build]    启动服务（--build 强制重新构建前端/后端/镜像）
  stop              停止服务
  restart [--build] 重启服务（--build 强制重新构建镜像）
  rebuild           重新构建前端 + 后端 + 镜像并重启
  status            查看容器状态
  logs [-f]         查看容器日志
  help              显示本帮助

说明:
  - 兼容 Docker 与 rootless Podman（自动设置 DOCKER_HOST）
  - 首次 start 会自动构建；代码变更后使用 start --build 或 rebuild
EOF
}

# ---------------------------------------------------------------------------
# 主入口
# ---------------------------------------------------------------------------

main() {
  if [[ $# -lt 1 ]]; then
    usage
    exit 1
  fi

  local cmd="$1"; shift

  case "${cmd}" in
    start)
      local build=0
      [[ "${1:-}" == "--build" ]] && build=1
      cmd_start "${build}"
      ;;
    stop)
      cmd_stop
      ;;
    restart)
      local build=0
      [[ "${1:-}" == "--build" ]] && build=1
      cmd_restart "${build}"
      ;;
    rebuild)
      cmd_rebuild
      ;;
    status)
      cmd_status
      ;;
    logs)
      cmd_logs "$@"
      ;;
    help|-h|--help)
      usage
      ;;
    *)
      err "未知命令: ${cmd}"
      usage
      exit 1
      ;;
  esac
}

main "$@"