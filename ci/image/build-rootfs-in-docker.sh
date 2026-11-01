#!/usr/bin/env bash
# =============================================================================
# build-rootfs-in-docker.sh — 在 Docker/Podman 容器内构建 gita CI rootfs
#
# 宿主机无需 root 权限。构建过程（debootstrap / chroot / apt / rustup /
# node / playwright）全部在容器内完成；容器内天然是 root。
#
# 注意：仅适用于 **rootful** docker/podman（system 级服务）。
# 在 rootless podman/docker（默认）下，debootstrap 的 mknod 会被内核
# 的用户命名空间拦截而失败，此时请改用 build-rootfs.sh + sudo。
#
# 用法：
#   ./ci/image/build-rootfs-in-docker.sh \
#     --output /path/to/gita-ci-ubuntu.ext4 \
#     --size 8G \
#     [--agent-script /path/to/gita-ci-agent]
#
# 参数与 build-rootfs.sh 一致，只是多包一层容器。产物写到 --output。
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
IMAGE_NAME="gita-ci-builder"

OUTPUT=""
SIZE="8G"
AGENT_SCRIPT=""
BUILD_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output) OUTPUT="$2"; shift 2 ;;
    --size) SIZE="$2"; shift 2 ;;
    --agent-script) AGENT_SCRIPT="$2"; shift 2 ;;
    *) echo "未知参数: $1" >&2; exit 1 ;;
  esac
done

[[ -n "$OUTPUT" ]] || { echo "错误: 必须指定 --output" >&2; exit 1; }

# 输出目录必须存在，否则容器挂不进去
OUTPUT_DIR="$(dirname "$OUTPUT")"
mkdir -p "$OUTPUT_DIR"
OUTPUT_BASENAME="$(basename "$OUTPUT")"

echo "[gita] 构建 docker 镜像 $IMAGE_NAME ..."
if command -v docker >/dev/null 2>&1; then
  DOCKER=docker
else
  DOCKER=podman
fi
"$DOCKER" build -f "${SCRIPT_DIR}/Dockerfile" -t "$IMAGE_NAME" "$PROJECT_ROOT"

# 容器参数：--privileged 允许 mount /proc /sys /dev；挂载输出目录。
VOLUMES=(-v "${OUTPUT_DIR}:/output")
ARGS_EXTRA=()
if [[ -n "$AGENT_SCRIPT" ]]; then
  # 把 agent 二进制只读挂进容器
  VOLUMES+=(-v "${AGENT_SCRIPT}:/agent:ro")
  ARGS_EXTRA+=(--agent-script /agent)
fi

echo "[gita] 在容器内构建 rootfs -> $OUTPUT (size $SIZE) ..."
"$DOCKER" run --rm \
  --security-opt label=disable \
  --cap-add MKNOD \
  --cap-add SYS_ADMIN \
  --cap-add SYS_CHROOT \
  --cap-add NET_ADMIN \
  --cap-add SYS_PTRACE \
  --device-cgroup-rule 'c 1:* rmw' \
  --device-cgroup-rule 'c 5:* rmw' \
  --device /dev/fuse \
  "${VOLUMES[@]}" \
  "$IMAGE_NAME" \
  --output "/output/${OUTPUT_BASENAME}" \
  --size "$SIZE" \
  "${ARGS_EXTRA[@]}"

echo "[gita] 完成: $OUTPUT"
echo "提示: 用 gita-admin image register 登记此镜像。"