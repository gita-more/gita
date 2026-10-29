#!/usr/bin/env bash
# =============================================================================
# build-rootfs.sh — 构建 gita 自有的 CI 虚拟机 rootfs
#
# 产物：一个 raw/ext4 格式的 rootfs 磁盘镜像，预装了 gita CI 所需环境：
#   - Rust stable toolchain（rustup + cargo）
#   - Node.js 22 + npm
#   - PostgreSQL 17（服务端 + 客户端）
#   - git / ssh / openssh-client / postgresql-client
#   - libdbus-1-dev / pkg-config（gita-client 编译依赖）
#   - Playwright chromium（web e2e）
#   - 一个开机自启的 CI agent（可选，--agent-script 传入）
#
# 镜像内容属于 gita；rcluster 只负责登记与挂载，不知道里面装了什么。
#
# 用法：
#   sudo ./build-rootfs.sh \
#     --output /data/gita/ci-rootfs.ext4 \
#     --size 8G \
#     --ubuntu-release noble \
#     [--agent-script /path/to/gita-ci-agent]
#
# 依赖宿主机工具：debootstrap、qemu-user-static（构建阶段）、unsquashfs
# =============================================================================
set -euo pipefail

# ---- 参数 ----
OUTPUT=""
SIZE="8G"
UBUNTU_RELEASE="noble"          # 24.04
AGENT_SCRIPT=""                # 可选：要嵌入镜像的 gita-ci-agent 可执行文件
WORKDIR=""

# ---- 解析参数 ----
while [[ $# -gt 0 ]]; do
  case "$1" in
    --output) OUTPUT="$2"; shift 2 ;;
    --size) SIZE="$2"; shift 2 ;;
    --ubuntu-release) UBUNTU_RELEASE="$2"; shift 2 ;;
    --agent-script) AGENT_SCRIPT="$2"; shift 2 ;;
    *) echo "未知参数: $1" >&2; exit 1 ;;
  esac
done

[[ -n "$OUTPUT" ]] || { echo "错误: 必须指定 --output" >&2; exit 1; }

# ---- 工作目录 ----
WORKDIR="$(mktemp -d /tmp/gita-image.XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

ROOT="$WORKDIR/root"
mkdir -p "$ROOT"

log() { echo "[gita-image] $*"; }

# 检查工具
for tool in debootstrap mkfs.ext4 mount umount chroot; do
  command -v "$tool" >/dev/null 2>&1 || { echo "缺少工具: $tool" >&2; exit 1; }
done

# =============================================================================
# 1. debootstrap 最小 rootfs
# =============================================================================
log "debootstrap $UBUNTU_RELEASE -> $ROOT"
debootstrap --variant=minbase --arch=amd64 "$UBUNTU_RELEASE" "$ROOT" \
  http://archive.ubuntu.com/ubuntu

# 挂载必要的伪文件系统，供 chroot 内 apt/服务安装使用
mount --bind /proc "$ROOT/proc"
mount --bind /sys "$ROOT/sys"
mount --bind /dev "$ROOT/dev"
mount --bind /dev/pts "$ROOT/dev/pts"
# 提供 DNS 解析
cp /etc/resolv.conf "$ROOT/etc/resolv.conf"

cleanup_mounts() {
  umount "$ROOT/dev/pts" 2>/dev/null || true
  umount "$ROOT/dev" 2>/dev/null || true
  umount "$ROOT/sys" 2>/dev/null || true
  umount "$ROOT/proc" 2>/dev/null || true
}
trap 'cleanup_mounts; rm -rf "$WORKDIR"' EXIT

# =============================================================================
# 2. 安装系统依赖
# =============================================================================
cat > "$ROOT/etc/apt/sources.list" <<EOF
deb http://archive.ubuntu.com/ubuntu $UBUNTU_RELEASE main universe
deb http://archive.ubuntu.com/ubuntu $UBUNTU_RELEASE-updates main universe
deb http://security.ubuntu.com/ubuntu $UBUNTU_RELEASE-security main universe
EOF

chroot "$ROOT" /bin/bash -c '
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  ca-certificates \
  git \
  ssh \
  openssh-client \
  openssh-server \
  curl \
  wget \
  gnupg \
  postgresql \
  postgresql-client \
  libdbus-1-dev \
  pkg-config \
  build-essential \
  python3 \
  sudo \
  systemd \
  iproute2 \
  iputils-ping \
  net-tools \
  rsync \
  xz-utils \
  zlib1g-dev \
  libssl-dev \
  libsqlite3-dev \
  unzip
# 清理 apt 缓存
apt-get clean
rm -rf /var/lib/apt/lists/*
'

# =============================================================================
# 3. 安装 Rust（rustup）
# =============================================================================
chroot "$ROOT" /bin/bash -c '
set -euo pipefail
export RUSTUP_HOME=/opt/rust
export CARGO_HOME=/opt/cargo
export PATH=/opt/cargo/bin:$PATH
curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh
chmod +x /tmp/rustup-init.sh
/tmp/rustup-init.sh -y --profile minimal --default-toolchain stable
rm -f /tmp/rustup-init.sh
# 让普通用户也能用
echo "export RUSTUP_HOME=/opt/rust" >> /etc/profile.d/rust.sh
echo "export CARGO_HOME=/opt/cargo" >> /etc/profile.d/rust.sh
echo "export PATH=/opt/cargo/bin:\$PATH" >> /etc/profile.d/rust.sh
# 预装 gita 编译常用的组件（加速 CI）
/opt/cargo/bin/rustup component add rustfmt clippy
'

# =============================================================================
# 4. 安装 Node.js 22（从 nodesource）
# =============================================================================
chroot "$ROOT" /bin/bash -c '
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
curl -fsSL https://deb.nodesource.com/setup_22.x | bash -
apt-get install -y nodejs npm
# 全局装 Playwright 依赖（浏览器 chromium 需要系统库）
npm install -g playwright
npx playwright install --with-deps chromium
apt-get clean
rm -rf /var/lib/apt/lists/*
'

# =============================================================================
# 5. 配置 SSH（允许 CI agent 登录）
# =============================================================================
chroot "$ROOT" /bin/bash -c '
set -euo pipefail
# 允许 root 登录（CI agent 可能需要）
sed -i "s/^#PermitRootLogin.*/PermitRootLogin prohibit-password/" /etc/ssh/sshd_config
sed -i "s/^PasswordAuthentication.*/PasswordAuthentication no/" /etc/ssh/sshd_config
# 确保有 host keys
ssh-keygen -A
# 创建 ci 用户
useradd -m -s /bin/bash ci || true
echo "ci ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/ci-ci
mkdir -p /home/ci/.ssh
chown -R ci:ci /home/ci/.ssh
chmod 700 /home/ci/.ssh
'

# =============================================================================
# 6. 安装开机自启的 CI agent（可选）
#
# --agent-script 传的是 crates/gita-ci-agent 编译出的可执行文件（CI VM 内
# 没有 Rust 工具链，先在宿主机交叉编译，例如：
#   cargo build --release -p gita-ci-agent --target x86_64-unknown-linux-musl
#），脚本会把它装成 systemd 服务开机自启。
# =============================================================================
if [[ -n "$AGENT_SCRIPT" ]]; then
  log "安装 CI agent: $AGENT_SCRIPT"
  install -m 0755 "$AGENT_SCRIPT" "$ROOT/usr/local/bin/gita-ci-agent"
  cat > "$ROOT/etc/systemd/system/gita-ci-agent.service" <<EOF
[Unit]
Description=gita CI agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/gita-ci-agent
Restart=on-failure
User=ci

[Install]
WantedBy=multi-user.target
EOF
  chroot "$ROOT" /bin/bash -c 'systemctl enable gita-ci-agent'
fi

# =============================================================================
# 7. 收尾：清理 + 打包为 raw ext4
# =============================================================================
cleanup_mounts
trap 'rm -rf "$WORKDIR"' EXIT

log "打包 rootfs 为 raw ext4: $OUTPUT ($SIZE)"
truncate -s "$SIZE" "$OUTPUT"
mkfs.ext4 -d "$ROOT" -F "$OUTPUT" >/dev/null

log "完成: $OUTPUT"
echo "提示: 用 gita-admin image register 登记此镜像。"