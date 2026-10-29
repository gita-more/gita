# gita CI VM 镜像构建

本目录用于构建 **gita 自有的 CI 虚拟机镜像**（rootfs），并通过 rcluster 的镜像登记接口注册。

## 架构边界（重要）

```
rcluster：通用 VM 生命周期管理器
    - 只认 kernel / rootfs / disk 三类镜像
    - 不关心镜像里装了什么（不知道 gita 还是 WordPress）
    - 通过 POST /api/v1/images 接受外部登记的镜像
    - 负责存储挂载 / COW 克隆 / 调度 / 网络 / VM 生命周期

gita：作为 rcluster 的"客户"
    - 自己构建 CI 镜像（本目录）
    - 通过 rcluster 镜像登记 API 注册
    - 用模板实例化 VM 跑 CI job
```

**rcluster 不知道也不应该知道 gita 镜像里装了什么。** 本目录是 gita 侧负责"客户环境"的地方。

## rcluster 提供的镜像工具

rcluster **不提供**"从零构建客户镜像"的工具，它只提供：

| 能力 | 入口 |
|------|------|
| 登记外部镜像 | `POST /api/v1/images`（body: name/pool/digest/kind/path/arch） |
| 列出镜像 | `GET /api/v1/images`、`rcluster image list` |
| 删除镜像 | `DELETE /api/v1/images/{id}`、`rcluster image delete` |
| 模板管理 | `POST /api/v1/templates`、`rcluster template` |
| VM 生命周期 | `POST /api/v1/vms` + start/stop/delete |

所以镜像的**构建**完全由 gita 自己完成，本目录就是干这个的。

## 本目录脚本

| 脚本 | 作用 |
|------|------|
| `build-rootfs.sh` | 从 Ubuntu cloud image 构建 gita CI rootfs（预装依赖） |
| `register-image.sh` | 把构建好的 rootfs 登记到 rcluster（`POST /images`） |
| `create-template.sh` | 用登记的镜像创建 rcluster VmTemplate |
| `provision-userdata.sh` | 生成每个 CI job 的 user_data（配置 SSH/拉代码/跑测试） |

## 依赖栈（gita CI 需要）

- Rust stable toolchain（workspace 多 crate）
- Node.js 22（`.nvmrc`）
- PostgreSQL 17
- git / ssh / openssh-client
- libdbus-1-dev、pkg-config（client 编译依赖）
- postgresql-client（测试连 DB）
- Playwright chromium（web e2e）

## 使用流程

```bash
# 1. 构建 rootfs（产出 ci-rootfs.ext4 + 内核）
./ci/image/build-rootfs.sh --output /path/to/ci-rootfs.ext4

# 2. 登记到 rcluster
./ci/image/register-image.sh \
  --control-url http://127.0.0.1:8080 \
  --pool <pool-id> \
  --path /path/to/ci-rootfs.ext4 \
  --name gita-ci-ubuntu-24.04

# 3. 创建模板
./ci/image/create-template.sh \
  --control-url http://127.0.0.1:8080 \
  --image gita-ci-ubuntu-24.04 \
  --cpu 4000 --memory 4G --disk 20G

# 4. 每次 CI job：从模板建 VM + 注入 user_data
./ci/image/provision-userdata.sh --commit <sha> --ssh-key <key> > userdata.yaml
```

## 说明

- 构建需要能访问 Ubuntu cloud image 源。
- 构建出的 rootfs 是 **raw/ext4** 格式（rcluster 存储驱动统一 raw）。
- 内核由 rcluster 提供（`fetch-guest-assets.sh` 下载 vmlinux），本目录只构建 rootfs。