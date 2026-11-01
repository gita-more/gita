# 在容器内构建 gita 专用 CI rootfs。
#
# 为什么要用容器：build-rootfs.sh 依赖 chroot / mount / 操作用户与 systemd，
# 这些在宿主机上需要 root 权限。把整个构建搬进容器后，容器内天然是 root，
# 宿主机无需任何 root 权限，也不需要 proot/fakeroot 之类的用户态模拟。
#
# 构建产物（raw ext4 rootfs）写到挂载进来的 `--output` 路径。
#
# 用法（由 build-rootfs-in-docker.sh 调用）：
#   docker build -f ci/image/Dockerfile -t gita-ci-builder .
#   docker run --privileged \
#     -v "$OUTPUT_DIR:/output" \
#     -v "$AGENT_BIN:/agent:ro" \
#     gita-ci-builder \
#     /build-rootfs.sh --output /output/gita-ci-ubuntu.ext4 --size 8G \
#       --agent-script /agent

FROM debian:bookworm-slim

# 构建 rootfs 需要的工具（debootstrap + chroot 内 apt + 打包 ext4）。
# mount 需要 --privileged 容器；mkfs.ext4 来自 e2fsprogs。
RUN apt-get update && apt-get install -y --no-install-recommends \
    debootstrap \
    e2fsprogs \
    systemd \
    ca-certificates \
    curl \
    gnupg \
    && rm -rf /var/lib/apt/lists/*

# 把构建脚本拷进镜像
COPY ci/image/build-rootfs.sh /build-rootfs.sh
RUN chmod +x /build-rootfs.sh

# 容器内以 root 运行，宿主无需 root
ENTRYPOINT ["/build-rootfs.sh"]