#!/usr/bin/env bash
# =============================================================================
# create-template.sh — 用 gita 登记的 CI 镜像创建 rcluster VmTemplate
#
# 模板是"半成品"：预装了 gita CI 环境的 base 系统。每次 CI job 从模板
# 实例化一台一次性 VM，跑完销毁。rcluster 通过 COW 克隆实现秒级复制。
#
# 用法：
#   ./create-template.sh \
#     --control-url http://127.0.0.1:8080 \
#     --image gita-ci-ubuntu-24.04 \
#     --template gita-ci-ubuntu-24.04 \
#     --cpu 4000 \
#     --memory 4G \
#     --disk 20G \
#     --network <network-id>
#
# 依赖：curl、jq
# =============================================================================
set -euo pipefail

# ---- 参数 ----
CONTROL_URL="http://127.0.0.1:8080"
IMAGE_NAME="gita-ci-ubuntu-24.04"
TEMPLATE_NAME="gita-ci-ubuntu-24.04"
CPU_MILLIS="4000"
MEMORY="4G"
DISK="20G"
NETWORK_ID=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --control-url) CONTROL_URL="$2"; shift 2 ;;
    --image) IMAGE_NAME="$2"; shift 2 ;;
    --template) TEMPLATE_NAME="$2"; shift 2 ;;
    --cpu) CPU_MILLIS="$2"; shift 2 ;;
    --memory) MEMORY="$2"; shift 2 ;;
    --disk) DISK="$2"; shift 2 ;;
    --network) NETWORK_ID="$2"; shift 2 ;;
    *) echo "未知参数: $1" >&2; exit 1 ;;
  esac
done

# ---- 大小换算 ----
to_bytes() {
  local v="$1"
  case "$v" in
    *G|*g) echo $(( ${v%[Gg]} * 1024 * 1024 * 1024 )) ;;
    *M|*m) echo $(( ${v%[Mm]} * 1024 * 1024 )) ;;
    *) echo "$v" ;;
  esac
}

MEMORY_BYTES="$(to_bytes "$MEMORY")"
DISK_BYTES="$(to_bytes "$DISK")"

echo "[gita-image] 创建模板:"
echo "  control-url : $CONTROL_URL"
echo "  template    : $TEMPLATE_NAME"
echo "  image       : $IMAGE_NAME"
echo "  cpu         : ${CPU_MILLIS} millis"
echo "  memory      : $MEMORY_BYTES bytes"
echo "  disk        : $DISK_BYTES bytes"
echo "  network     : $NETWORK_ID"

# ---- 确认镜像存在 ----
echo "[gita-image] 检查镜像 $IMAGE_NAME ..."
IMAGES=$(curl -fsS "$CONTROL_URL/api/v1/images")
if ! echo "$IMAGES" | jq -e --arg n "$IMAGE_NAME" '.images[]? | select(.name==$n)' >/dev/null 2>&1; then
  echo "错误: 镜像 $IMAGE_NAME 未登记（先运行 register-image.sh）" >&2
  exit 1
fi

# ---- 创建模板 ----
# rcluster 的模板请求体（参考 handlers create_template）：
#   name, cpu_millis, memory_bytes, disk_bytes, image{name}, networks[]
BODY=$(jq -n \
  --arg name "$TEMPLATE_NAME" \
  --argjson cpu "$CPU_MILLIS" \
  --argjson mem "$MEMORY_BYTES" \
  --argjson disk "$DISK_BYTES" \
  --arg image "$IMAGE_NAME" \
  --arg network "$NETWORK_ID" \
  '{name:$name, cpu_millis:$cpu, memory_bytes:$mem, disk_bytes:$disk,
    image:{name:$image}, networks:($network | if .=="" then [] else [{network:$network}] end)}')

RESP=$(curl -fsS \
  -X POST "$CONTROL_URL/api/v1/templates" \
  -H 'Content-Type: application/json' \
  -d "$BODY")

echo "[gita-image] 模板创建成功:"
echo "$RESP" | jq .