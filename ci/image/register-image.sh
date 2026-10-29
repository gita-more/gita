#!/usr/bin/env bash
# =============================================================================
# register-image.sh — 把 gita 构建好的 rootfs 登记到 rcluster
#
# rcluster 是通用 VM 管理器，不知道镜像内容；本脚本通过其
# POST /api/v1/images 接口登记 gita 自有的 CI 镜像。
#
# 用法：
#   ./register-image.sh \
#     --control-url http://127.0.0.1:8080 \
#     --pool <storage-pool-id> \
#     --path /data/gita/ci-rootfs.ext4 \
#     --name gita-ci-ubuntu-24.04 \
#     --arch x86_64
#
# 依赖：curl、jq（可选，用于解析响应）
# =============================================================================
set -euo pipefail

# ---- 参数 ----
CONTROL_URL="http://127.0.0.1:8080"
POOL=""
IMAGE_PATH=""
IMAGE_NAME="gita-ci-ubuntu-24.04"
DIGEST=""
ARCH="x86_64"
KIND="rootfs"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --control-url) CONTROL_URL="$2"; shift 2 ;;
    --pool) POOL="$2"; shift 2 ;;
    --path) IMAGE_PATH="$2"; shift 2 ;;
    --name) IMAGE_NAME="$2"; shift 2 ;;
    --digest) DIGEST="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --kind) KIND="$2"; shift 2 ;;
    *) echo "未知参数: $1" >&2; exit 1 ;;
  esac
done

[[ -n "$POOL" ]] || { echo "错误: 必须指定 --pool（存储池 ID）" >&2; exit 1; }
[[ -n "$IMAGE_PATH" ]] || { echo "错误: 必须指定 --path（rootfs 文件路径）" >&2; exit 1; }

# 可选：计算 digest 帮助幂等
if [[ -z "$DIGEST" && -f "$IMAGE_PATH" ]]; then
  DIGEST="$(sha256sum "$IMAGE_PATH" | cut -d' ' -f1)"
  echo "[gita-image] 镜像 sha256: $DIGEST"
fi

# ---- 校验路径存在 ----
[[ -f "$IMAGE_PATH" ]] || { echo "错误: 镜像文件不存在: $IMAGE_PATH" >&2; exit 1; }

echo "[gita-image] 登记镜像到 rcluster:"
echo "  control-url : $CONTROL_URL"
echo "  name        : $IMAGE_NAME"
echo "  kind        : $KIND"
echo "  pool        : $POOL"
echo "  path        : $IMAGE_PATH"
echo "  arch        : $ARCH"
echo "  digest      : $DIGEST"

BODY=$(jq -n \
  --arg name "$IMAGE_NAME" \
  --arg pool "$POOL" \
  --arg digest "$DIGEST" \
  --arg kind "$KIND" \
  --arg path "$IMAGE_PATH" \
  --arg arch "$ARCH" \
  '{name:$name, pool:$pool, digest:$digest, kind:$kind, path:$path, arch:$arch}')

RESP=$(curl -fsS \
  -X POST "$CONTROL_URL/api/v1/images" \
  -H 'Content-Type: application/json' \
  -d "$BODY")

echo "[gita-image] 登记成功:"
echo "$RESP" | jq .