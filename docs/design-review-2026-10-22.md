# Gita 实现设计文档

> 基于 `main` 分支 `5f4a0ce`（2026-10-21）的**实测**分析。
> 本文描述的是**代码里实际存在的东西**，不是 goal.md 里的规划。
> 差异清单见 [§9 设计文档与实现的偏差](#9-设计文档与实现的偏差)。

---

## 1. 产品定位与实际形态

goal.md 最初的定位是"AI 原生的自托管 Git Server"——以事件流为 AI 感知层、以 MCP 为 Agent 入口。
实际交付下来的是：

| 维度 | 定位 | 实际 |
|---|---|---|
| 定位 | AI 原生 Git Server | **功能完整的自托管 Git Server**，带 In-repo CI 和静态站点托管 |
| AI 能力 | 评审引擎、语义搜索、MCP | **全部移除**（commit `11ea5f7` "chore: remove AI review and code search functionality"） |
| AI 残留 | — | 事件表 + `/api/events` + `/api/mcp/tools` **工具清单端点**（纯静态 JSON，无实际 MCP 传输） |
| 协议 | Git over HTTP | Git over HTTP **+** 嵌入式 SSH 服务器（commit `4c93444`） |
| 协作 | PR / Issue | PR / Issue / 评论 / 星标 / 群组 / Webhook / 搜索 / 分支标签 / Fork / 头像 |
| 部署 | 单节点 | 单节点（存储纯本地 FS，事件总线纯进程内） |

**一句话现状**：Gita 已经是一个能用的 Gitea 精简版，AI 部分只剩下"事件可读"这一层骨架。

---

## 2. 系统架构

```
┌──────────────────────────────────────────────────────────┐
│  客户端                                                     │
│  git CLI(HTTP) │ git CLI(SSH) │ React SPA │ API 调用者      │
└───────┬──────────────┬──────────────┬──────────────┬────────┘
        │              │              │              │
┌───────▼──────────────▼──────────────▼──────────────▼────────┐
│  axum Router (gita-server)                                  │
│  /api/*  → gita-api     /git/* → gita-http                  │
│  /<owner>/<repo>/* → gita-api/pages_site                   │
│  fallback → gita-web (SPA)                                   │
└───────┬──────────────────────────────┬───────────────────────┘
        │                              │
┌───────▼──────────────┐   ┌──────────▼──────────────────────┐
│  gita-git            │   │  gita-api (REST + 鉴权 + DTO)      │
│  裸仓库管理           │   │  routes/ 14 组路由                 │
│  gix: refs/树遍历     │   │  visibility.rs 集中可见性          │
│  git CLI: 协议/合并   │   │  middleware.rs 限流               │
└───────┬──────────────┘   └──────────┬──────────────────────┘
        │                             │
┌───────▼─────────────────────────────▼──────────────────────┐
│  数据层                                                    │
│  PostgreSQL (gita-db, 19 张表)  │  裸仓库 FS (data/repos/)   │
│  gita-events: tokio broadcast (纯进程内)                    │
└────────────────────────────────────────────────────────────┘
```

### 2.1 Crate 职责与依赖

```
gita-core    (430 行)  领域模型 + EventType 枚举           ← 最底层
gita-db      (3257 行) SQLx 数据访问 + 10 个 migration
gita-git     (2149 行) 裸仓库 / refs / diff / merge
gita-events  (360 行)  进程内事件总线
gita-pipeline(716 行)  .gita-ci.yml 解析 + needs 拓扑排序
gita-runner  (769 行)  独立 CI runner 二进制
gita-http    (889 行)  /git/* Smart HTTP
gita-ssh     (515 行)  嵌入式 SSH 服务器
gita-api     (6604 行) REST API + OpenAPI
gita-web     (316 行)  SPA 静态托管
gita-server  (951 行)  入口：配置 / 组装 / 启动
```

依赖方向单向向下，无环。`gita-runner` 只依赖 `gita-db` + `gita-pipeline`（**不依赖 `gita-events`**——
因为它是独立进程，够不到进程内总线）。

### 2.2 Git 操作的双轨制

`gita-git` 同时用两套 Git 实现，**分工是清晰的**：

| 用 `git` CLI（CGI / 子进程） | 用 `gix`（Rust crate） |
|---|---|
| `git http-backend` 承接 Smart HTTP 协议 | `gix::init_bare` 建裸仓库 |
| `git merge-base` / `merge-tree` 冲突检测 | `gix::refs::transaction` ref 增删改 |
| `git rev-parse` / `cat-file` / `log` | `gix::Tree` 树遍历 |
| `git commit-tree` 建 merge commit | `gix::ObjectId` |
| `git clone --bare` fork | |
| `git log` 历史 | |

**结论**：协议层委托给 Git 官方实现（正确且省事），ref 操作用 gix 的原子事务
（`PreviousValue::MustExistAndMatch` = CAS，并发 push 不会互相覆盖）。这个选择是合理的。

---

## 3. 数据模型

19 张表，10 个 migration，**全部 `CREATE TABLE IF NOT EXISTS`，依赖顺序正确**：

```
20260101000000_init.sql          ← 2026-01-01，数值上最小，所以先跑（命名易误解但正确）
  users, repositories, commits, pull_requests, issues, events, access_tokens
20261018000000_groups.sql        + groups, group_members, repositories.group_id
20261019000000_locale.sql        + users.locale
20261020000000_pipelines.sql     + pipelines, pipeline_runs, jobs, logs
20261020010000_comments.sql      + comments
20261020020000_stars.sql         + stars
20261020030000_webhooks.sql      + webhooks
20261020040000_password_reset_tokens.sql
20261020050000_ssh_keys.sql
20261020060000_pages.sql         + repo_pages
```

### 3.1 关键设计约定

| 约定 | 说明 | 代价 |
|---|---|---|
| **时间戳存 TEXT** | 全部 RFC3339 字符串，非 `timestamptz` | 文本排序脆弱；仅密码重置路径做了 `::timestamptz` 转换 |
| **`is_private` 是 BIGINT** | 0/1，非 BOOLEAN | 与 `upsert_pages` 的真 bool 混用 |
| **无事务** | 每个 `Db` 方法是单条自动提交语句 | `MAX(number)+1` 后再 INSERT → 并发创建 PR/Issue 会撞 `UNIQUE` |
| **无编译期 SQL 校验** | 全用 `sqlx::query()` 运行时 API，无 `query!` 宏 | 编译期不检查 SQL；无 `.sqlx/` 离线缓存 |
| **主键统一 BIGINT** | `GENERATED ALWAYS AS IDENTITY`（`pipelines.sql` 用 BIGSERIAL，风格不统一） | 不用 UUID，无法跨节点生成 |

### 3.2 引用完整性缺口

| 表 | 到 `repositories` 的外键 | 删除仓库后 |
|---|---|---|
| `pipelines` / `pipeline_runs` / `comments` / `stars` / `webhooks` / `repo_pages` | ✅ `ON DELETE CASCADE` | 正确清理 |
| `pull_requests` | ❌ **无** | **永久孤儿** |
| `issues` | ❌ **无** | **永久孤儿** |
| `commits` | ❌ **无** | **永久孤儿**（且该表生产环境**从未被写入**——`insert_commit` 无任何调用方） |
| `events` | ❌ 无（有意为之，事件日志是审计轨迹） | 保留，但 `?repo=owner/name` 过滤会因仓库已删而 404，该仓库事件变得不可查 |

`repositories.owner_id` / `access_tokens.user_id` 也没有外键——但**根本没有删除用户的 API**，
所以这个缺口暂时不会暴露。

---

## 4. 核心机制

### 4.1 权限模型：一条规则，三处实现

**规则**：仓库可读 = `!is_private || caller.id == repo.owner_id`；仓库可写 = `caller.id == repo.owner_id`。

| 路径 | 实现位置 | 是否正确 |
|---|---|---|
| Git over HTTP | `gita-http/src/lib.rs:479` `authorize_git_request` | ✅ |
| Git over SSH | `gita-ssh/src/lib.rs:271` | ✅ |
| REST API（读） | `gita-api/src/routes/visibility.rs` `load_readable_repo` | ✅ |
| REST API（**写**） | 各 handler 直接 `get_repository_by_name` | ❌ **绕过可见性** |

`visibility.rs` 的设计是对的——集中化、返回 404 而非 403（不泄露存在性）、公共仓匿名可读。
**问题是写路径没有走它**。详见 [§8 P0-1](#p0-1写路径全面绕过可见性检查idor)。

### 4.2 可见性模型本身没有 RBAC

`groups` / `group_members` 有 `role` 字段（member/developer/maintainer/guest），管理 API 也齐
（增删改成员、禁止把 owner 角色给别人）。**但这些角色完全不参与访问决策**：

- `visibility::can_read` 只看 `repo.owner_id`
- 群组仓的 `owner_id` 是**群主**的 id
- 结论：**群组成员对自己所在群组的仓库没有任何访问权**。RBAC 是个空壳。

同样**不存在 collaborator 概念**——`repositories` 只有 `owner_id`，没有成员表。
所以"谁能 push" = "只有 owner"。

### 4.3 认证

| 方式 | 载体 | 说明 |
|---|---|---|
| JWT | `Authorization: Bearer <jwt>` | 7 天过期，HS256，`sub` + `username` |
| PAT | `Authorization: Bearer <pat>` | 32 字节随机 hex，**明文存库** |
| Git Basic | `Authorization: Basic base64(user:token)` | 密码位是 token（PAT 或 JWT） |

- 密码哈希：Argon2 默认参数
- 无 cookie、无 CSRF 面（纯 header 认证）
- 前端 token 存 `localStorage`（非 httpOnly cookie）
- **没有 token 吊销机制**（JWT 签发后 7 天内无法作废，除非改 secret）

### 4.4 限流

三个独立 bucket，**都在 handler 内部手动 `check_key`**（不是中间件）：

| 端点 | 配额 | 键 |
|---|---|---|
| `/api/auth/login` | 5/s, burst 10 | 客户端 IP |
| `/api/auth/register` | 1/s, burst 3 | 客户端 IP |
| `/api/auth/password-reset/{request,confirm}` | 1/s, burst 3 | 客户端 IP |

IP 解析：优先用 `ConnectInfo` 的真实对端地址（不可伪造）；无对端地址时才用
`X-Forwarded-For` 的**最右**一节（防止伪造左端新增 bucket）。这个反伪造设计是对的。

`middleware::rate_limit_by_ip` 这个中间件函数**是死代码**——没有任何 router 装它。

### 4.5 密码重置

设计相当扎实：

- 请求端点对"存在/不存在"返回**完全相同**的响应体；`reset_token` 键在开关关闭时是**缺失**而非 null
  （存在性不能用来枚举账户）
- 令牌 32 字节随机 hex，1 小时过期
- **单次使用**：`claim_password_reset_token` 用同一条语句读+标记（原子 claim），并发兑换只有一个赢家
- 令牌总是写进 server log（无邮件通道，这是唯一可达路径）
- `allow_insecure_password_reset` 开关控制是否**也**放进 HTTP 响应

### 4.6 事件流

```
publish(event)
   ├─→ persist hook (同步调用，但内部 tokio::spawn 异步写库 + fan-out webhook)
   └─→ tx.send() → tokio broadcast(1024)
```

**关键事实**：

1. **总线纯进程内**。`gita-events` 不依赖 `gita-db`/`sqlx`，没有 LISTEN/NOTIFY，没有 Redis。
2. **生产环境零订阅者**。`.subscribe()` 只在 `gita-events` 自己的单测里出现。
   "AI 订阅事件流"是**能力**，不是**现实**。
3. **持久化是异步 fire-and-forget**。`publish()` 返回时，`events` 行可能还不存在。
   `Event.id` 在内存里恒为 0，DB 行却是真实 id——**订阅者看到的和 `/api/events` 看到的是两个不同 id**。
4. **7/19 个 EventType 变体从不被发布**：`AgentRunStarted/Completed`、
   `PipelineStarted/Completed/Failed`、`JobStarted/Completed`。
   runner 是独立进程，够不到总线，所以 **CI 的 run/job 生命周期在事件流里完全不可见**。
5. `pipeline_runs.trigger_event_id` 只有 push 路径会填（用 `publish_persisted` 手动先插库拿 id）；
   手动触发路径用 `bus.publish`，该列恒为 NULL。

### 4.7 In-repo CI 的实际机制

```
git push (HTTP)
   ├─→ EventType::Push → events 表 → webhook fan-out
   └─→ 【同级兄弟，不是事件的下游】CI 触发：
         读 pushed commit 的 .gita-ci.yml
         → 算 changed_paths
         → parse_yaml + only/except 过滤
         → upsert_pipeline
         → insert_pipeline_run(status='queued')
         → insert_event(PipelineRequested) → 回填 trigger_event_id
```

**这里有个概念偏差**：`PipelineRequested` 事件是在 run **已经存在之后**才发的，
它是"入队通知"而不是"入队原因"。CI 不是事件驱动的，是 push handler 里的命令式代码。

runner 侧：

```
轮询 → claim_one_run (FOR UPDATE SKIP LOCKED)
     → git clone --depth 1 → fetch <head_sha> → checkout --detach
     → 解析 .gita-ci.yml → expand() 按 needs 拓扑排序
     → 逐个 job: sh -c "set -e\n<joined script>"
     → stdout/stderr 逐行写 logs 表
```

### 4.8 Pages

GitHub Pages 风格：仓库内选分支 + 目录（限 `/` 或 `/docs`），站点挂在 `/<owner>/<repo>/`。

**路径安全做到了三层**（结论：没有穿越漏洞）：

1. `sanitize_site_path` 拒绝 `..`、`.`、空段、`\`、`\0`、`.git*` 开头段
2. git 树查找是**逐段字面匹配**，不做规范化
3. git 树是扁平命名空间，没有父目录概念

---

## 5. 功能关系图（功能之间怎么互相咬合）

```
                    ┌──────────────┐
                    │  用户注册/登录 │
                    └──────┬───────┘
                           │ 创建
                    ┌──────▼───────┐
                    │   仓库       │◄────┐
                    └──────┬───────┘     │
                           │             │ fork
        ┌──────────────────┼──────────────┴──────────────┐
        │                  │                             │
┌───────▼────────┐ ┌───────▼────────┐          ┌─────────▼─────────┐
│  git push      │ │  Web UI 操作    │          │  群组仓            │
│  (HTTP/SSH)    │ │                │          │  (owner_id=群主)   │
└───────┬────────┘ └───────┬────────┘          └───────────────────┘
        │                  │
        │ push 事件         │ 写路径（BYPASS 可见性 ← P0-1）
        ├──→ events 表 ──→ webhook fan-out (HMAC-SHA256 签名)
        │
        │ 【同级】CI 触发
        ├──→ 读 .gita-ci.yml → only/except → pipelines/runs 表
        │                          │
        │                          ▼
        │                   gita-runner (独立进程)
        │                   claim → clone → sh -c → logs
        │                          │
        │                          ▼
        │                   SSE 日志流 (EventSource, 1Hz DB 轮询)
        │
        └──→ PR merge → 真实 git merge (FF 或双父 merge commit)
                      → update_pr_status → PullRequestMerged 事件
```

**没有咬合的地方**（孤岛）：

- 事件总线 ↔ 事件总线：生产零订阅者，事件只是写表
- 群组成员角色 ↔ 访问控制：完全脱节
- CI 状态 ↔ 事件流：runner 够不到总线
- AI 能力（评审/搜索/MCP 传输）：已移除，只剩 `/api/mcp/tools` 静态清单
- 多节点：事件总线纯进程内 + 存储纯本地 FS，**两个节点 = 两个分叉的 Git 服务器共享一个数据库**

---

## 6. API 清单（实测，共 14 组路由）

| 分组 | 端点 |
|---|---|
| **auth** | `POST /api/auth/register` · `POST /api/auth/login` · `GET/POST /api/auth/tokens` · `DELETE /api/auth/tokens/{id}` · `GET/PATCH /api/auth/me` · `POST /api/auth/password-reset/request` · `POST /api/auth/password-reset/confirm` |
| **repo** | `POST/GET /api/repos` · `GET/DELETE/PATCH /api/repos/{o}/{r}` · `GET .../tree` · `GET .../file` · `GET .../commits` · `GET .../commits/{sha}` · `PUT/DELETE/GET .../star` · `POST .../fork` · `GET/POST .../hooks` · `DELETE .../hooks/{id}` · `GET/POST .../branches` · `DELETE .../branches/{b}` · `GET/POST .../tags` · `DELETE .../tags/{t}` |
| **pr** | `POST/GET .../prs` · `GET .../prs/{n}` · `POST .../prs/{n}/merge` · `POST .../prs/{n}/close` · `GET .../prs/{n}/diff` · `GET .../prs/{n}/files` |
| **issue** | `POST/GET .../issues` · `GET .../issues/{n}` · `POST .../issues/{n}/close` · `GET /api/issues`（全局） |
| **comment** | `GET/POST .../prs/{n}/comments` · `GET/POST .../issues/{n}/comments` · `DELETE /api/comments/{id}` |
| **group** | `POST/GET /api/groups` · `GET /api/groups/{g}` · `POST /api/groups/{g}/repos` · `GET/POST .../members` · `DELETE/PATCH .../members/{username}` |
| **pipeline** | `GET .../pipelines` · `GET/POST .../pipelines/{path}/runs` · `GET .../runs/{id}` · `POST .../runs/{id}/cancel` · `GET .../runs/{id}/jobs/{jid}/logs` · `GET .../runs/{id}/jobs/{jid}/logs/stream`（SSE） |
| **pages** | `GET/PUT/DELETE .../pages` · `GET .../pages/status` |
| **search** | `GET /api/search` · `GET /api/search/repos` · `GET /api/search/code` |
| **avatar** | `POST /api/users/me/avatar` · `GET /api/users/{username}/avatar` |
| **ssh_key** | `GET/POST /api/auth/keys` · `DELETE /api/auth/keys/{id}` |
| **event** | `GET /api/events` |
| **misc** | `GET /api/health` · `GET /api/mcp/tools` |
| **docs** | `GET /api/docs`（Swagger UI） · `GET /api/docs/openapi.json` |

---

## 7. 实测基线（2026-10-22）

| 项 | 结果 |
|---|---|
| `cargo test --workspace` | **213 passed / 0 failed**（5 个 ignored 是同一个 cleanup 测试编进 5 个 binary） |
| Playwright e2e | **17 passed** |
| vitest（前端单测） | **9 passed** |
| `cargo fmt --check` | **失败 455 处**（44 个文件） |
| `cargo clippy --workspace --all-targets` | 0 error / ~40 warning |
| `cargo build --workspace` | 通过（2 个 warning） |

单测分布：gita-git 23、gita-db 20、gita-api 18、gita-pipeline 15、gita-server 12、gita-events 10、
gita-core 9、gita-web 7、gita-http 6、gita-ssh 5、**gita-runner 0**。

### 7.1 基础功能端到端实测（独立实例，2026-10-22）

在一台**全新的本地实例**（独立 Postgres、独立 data_dir、端口 3200）上，不走任何测试夹具，
用真实 HTTP + 真实 `git` 客户端把基础功能走了一遍。**除 AI 相关部分外，主体功能都能用。**

**跑通的主链路**（全部返回预期状态码）：

| 功能 | 结果 |
|---|---|
| 注册 / 登录 / 改邮箱 / 改密码 / 改语言 | ✅ |
| 仓库 CRUD（建、列、读、改描述） | ✅ |
| `git push` → 提交入库、文件树、读文件 | ✅ |
| `git clone` → 文件完整 | ✅ |
| 分支 / 标签（建、列、删） | ✅ |
| PR 全流程（建、列、详情、diff、files、评论、合并、状态变 merged） | ✅ |
| Issue 全流程（建、列、详情、评论、关闭、全局列表） | ✅ |
| 评论（PR / issue 双向） | ✅ |
| 搜索（仓库 / 代码 / 聚合） | ✅ |
| Star / Fork / 群组（建群、成员管理、群组建仓） | ✅ |
| PAT（建、列、吊销后失效 401） | ✅ |
| 事件流（41 条） | ✅ |
| 头像上传 / 读取 | ✅ |
| SSH key 增删列 | ✅ |
| Pages（配置、状态、站点服务、目录 URL、CSP 头） | ✅ |
| Webhook（建、列、事件过滤） | ✅ |
| 密码重置（响应体逐字节一致、错 token 404、限流 429） | ✅ |
| CI 入队（push 含 `.gita-ci.yml` → run 变 `queued`） | ✅ |
| 输入校验（用户名/仓库名穿越字符、非法 locale、密码长度） | ✅ |
| 匿名读公共仓（tree/commits/prs/issues/branches/tags 全部 200） | ✅ 设计如此 |

**这次实测新发现的问题**（见 §8 的 P1-23 ~ P1-27）——都是**契约不一致**，
不是核心功能缺失：

- `GET /commits/{ref}` 只认 40 位 hex，`HEAD`/`main`/`master` 全部 500；
  而 `tree?ref=HEAD`、`file?ref=HEAD` 正常——**同一套 ref 解析在两个端点不一致**
- 不存在的 sha → **500**（应为 404）
- 不存在的 ref 的 tree → **200 + 空 entries**（应为 404）
- 无 body 的 POST（如 `POST /fork`）→ **415**（`Json<T>` 提取器强制要求 content-type）
- 重复注册同一把 SSH 公钥 → **200**，可无限累积（`ssh_keys` 表无唯一约束）

---

## 8. 问题清单（按严重度）

### P0-1　私有仓可被匿名完整读取（dumb-HTTP 绕过鉴权）

**性质**：最严重的安全漏洞。**已实机 PoC 确认**——私有仓的全部源码被匿名下载。

`gita-http/src/lib.rs:492-497` 用**字符串匹配**判定一个请求是不是"读"：

```rust
let is_read = path_info.contains("git-upload-pack")
    || path_info.contains("info/refs")
    || query_string.as_deref().map(|q| q.contains("service=git-upload-pack")).unwrap_or(false);
```

而 `gita-git/src/lib.rs:209` 对**每个请求**都设了 `GIT_HTTP_EXPORT_ALL=1`，
使 `git http-backend` 的 **dumb 文件服务路径**可用（提供 `HEAD`、`objects/**`、`.pack`、`.idx`）。

dumb 路径的请求**既不匹配 `is_read` 也不匹配 `is_write`** →
`authorize_git_request` 的两个检查都被跳过 → `Ok(())` 放行。

**实测**（A 的私有仓，匿名请求，无 Authorization 头）：

```
  info/refs?service=git-upload-pack -> HTTP 401   ✅ smart 路径正确拦截
  /HEAD                             -> HTTP 200   ❌
  /objects/info/packs               -> HTTP 200   ❌
  /objects/f4/c5ae8ac...  (37 bytes)-> HTTP 200   ❌
      解压后 -> blob 21 DUMPHTTP_CANARY_zz11      ❌ 源码内容
  /HEAD body -> "ref: refs/heads/main"            ❌ 泄露默认分支
```

`objects/info/packs` 会列出所有 pack 文件名，`.idx` 里是全部对象 id——
所以攻击者不需要猜 sha，**能完整重建私有仓的历史**。

**修法**（任选其一，推荐第一个）：

1. 把 `is_read` 判定改成"**路径落在 `/git/{owner}/{repo}.git/` 下就是对该仓的读**"，
   而不是匹配 `info/refs` / `git-upload-pack` 两个字符串
2. 或者**拒绝所有非 smart 路径**（dumb HTTP 本来就是遗留协议，现代 git 客户端都走 smart）
3. 或者不设 `GIT_HTTP_EXPORT_ALL=1`，改为按请求、按仓库决定

**附带问题**：同名判断用 `path_info.contains(...)` 是子串匹配——
一个叫 `git-receive-pack` 的仓库会让它的**所有**请求都被判成写操作。

---

### P0-2　PR 合并会静默丢弃目标分支的改动（数据丢失）

**性质**：数据丢失。**已实机 PoC 确认**。

`gita-git/src/lib.rs:1125-1137`：非快进合并时，merge commit 的 tree 用的是
**head 的 tree**，而不是 `check_merge_conflict` 里已经算好的 **merge tree**：

```rust
// The tree for the merge commit is the head commit's tree (a clean
// merge with no conflicts means the head tree already contains all
// changes; git's default strategy would normally re-derive it).
let head_tree = git_cmd().args(["rev-parse", &format!("{head_sha}^{{tree}}")])...
```

**这个注释是错的**。"无冲突"只说明两边没有**冲突的改动**，
不意味着 head 的 tree 已经包含了 target 分支独有的改动。

**实测**（main 独有 `only_main.txt`，feature 独有 `only_feature.txt`，两边各有 `shared.txt`）：

```
合并前 main 的树:  ['only_main.txt', 'shared.txt']
合并后 main 的树:  ['only_feature.txt', 'shared.txt']
                                     ^^^^^^^^^^^^^ only_main.txt 被静默丢弃
```

**目标分支独有的文件在合并后消失了**。用户会以为合并成功（API 返回 200 + merge_sha），
但合并实际上**回退了目标分支的一部分改动**。

**修法**：`check_merge_conflict` 已经用 `read-tree -m` + `write-tree` 算出了正确的 merge tree
（`gita-git:1004, 1020`）——把那个 tree 复用为 merge commit 的 tree 即可。
即：让 `merge_branch` 调用 `check_merge_conflict` 时**返回 merge tree sha**，而不是重新用 head 的 tree。

---

### P0-3　被拒绝的 push 也会发 PushEvent 并触发 CI

**性质**：事件正确性 + CI 误触发。

`gita-http/src/lib.rs:147` 的条件是 `path_info.contains("git-receive-pack") && status < 300`，
而 `status` 是 **CGI 的 HTTP 状态码**，不是 receive-pack 的**结果**。

`git http-backend` 对**被拒绝的 push 也返回 200**——拒绝信息在响应体的 `report-status`
pkt-line 里（`unpack ok` / `ng <ref> <reason>`），不在 HTTP 状态里。

代码还明确不解析 `report-status`（注释说"is streamed in chunks and hard to recover synchronously"），
改用**请求体**里客户端声明的 ref update —— 但**声明 ≠ 结果**。

**后果**：一个被拒绝的 push（非快进、hook 拒绝等）会：
1. 写一条 `push` 事件到 `events` 表，`new_sha` 是一个**从未被写入的 sha**
2. 所有 webhook 订阅者收到一个**幽灵 push**
3. **CI 触发块也会跑**（`gita-http:186-311`），可能入队一个 `PipelineRequested`

**修法**：解析 CGI 响应体里的 `report-status` / `report-status-v2` 段，
只对**真正成功**的 ref 发事件。或者至少校验 `new_sha` 在 push 后确实存在于该 ref 上。

---

### P0-4　写路径全面绕过可见性检查（IDOR 越权）

**性质**：安全漏洞。**已实机 PoC 确认**。

`visibility::load_readable_repo` 只在**读**路径被调用。所有**写**路径直接用
`get_repository_by_name`，完全不看 `is_private`：

| 文件:行 | 端点 | 影响 |
|---|---|---|
| `pr.rs:101` | `POST .../prs` | 任意登录用户可在他人的**私有**仓建 PR |
| `pr.rs:215` | `POST .../prs/{n}/merge` | **任意登录用户可合并他人私有仓的 PR** |
| `pr.rs:346` | `POST .../prs/{n}/close` | 可关闭他人私有仓的 PR |
| `issue.rs:103` | `POST .../issues` | 可在他人的私有仓建 issue |
| `issue.rs:178` | `POST .../issues/{n}/close` | 可关闭他人私有仓的 issue |
| `comment.rs:71` | `POST .../prs/{n}/comments` | 可在他人私有仓的 PR 下评论 |
| `comment.rs:139` | `POST .../issues/{n}/comments` | 可在他人私有仓的 issue 下评论 |
| `repo.rs:503/525/544` | `PUT/DELETE/GET .../star` | 可给他人私有仓加星 |
| `repo.rs:587` | `POST .../fork` | **可 fork 他人的私有仓**（`git clone --bare` 真复制对象） |
| `pipeline.rs:314` | `POST .../runs/{id}/cancel` | 可取消**全系统任意** run（连 owner 都不校验） |

**PoC 实测输出**（A 私有仓 / B 非属主）：

```
B GET  /api/repos/A/secret              -> 404   ✅ 读路径正确
B POST .../issues                        -> 200   ❌
B POST .../issues/1/comments             -> 200   ❌
B PUT  .../star                          -> 200   ❌
B POST .../prs/1/merge                   -> 200   ❌ "pull request merged"
A 查 PR 状态                             -> status: merged  ❌ 代码真的进了 main
```

**修法**：把所有写路径的 `get_repository_by_name` 换成 `load_readable_repo`（对私有仓非属主返回 404），
再按语义补属主/写权限检查。`merge_pr` 需要显式要求 `repository.owner_id == user.id`。

---

### P0-5　Web UI 给的 clone URL 缺 `/git` 前缀

**性质**：功能直接不可用。**已实机确认**。

`EmptyRepoWelcome.tsx:14-28` 生成 `{origin}/{owner}/{name}.git` 和
`{origin}/groups/{g}/{r}.git`，但后端只在 `/git/*` 挂载 Smart HTTP（`gita-http/src/lib.rs:35`）。

**实测**：

```
GET /testuser/cloneurl_probe.git/info/refs?service=git-upload-pack
  -> HTTP 200, Content-Type: text/html    ← SPA 首页！git clone 会失败
GET /git/testuser/cloneurl_probe.git/info/refs?service=git-upload-pack
  -> HTTP 200, Content-Type: application/x-git-upload-pack-advertisement  ← 正确
```

**更糟的是**：`integration/web/tests/empty-repo.spec.ts:40` 断言的正是这个错误值，
**e2e 测试把 bug 固化了**——所以 17 个 e2e 全绿，但功能是坏的。

**修法**：URL 改为 `{origin}/git/{owner}/{name}.git`（群组仓同理，仍用 `/git/groups/{g}/{r}.git`），
并同步改 e2e 断言。

---

### P0-6　CI runner 无沙箱、无超时、无资源限制、继承 runner 环境

**性质**：多租户部署下等于远程代码执行。

`gita-runner/src/executor.rs:245-258`：

```rust
let child = Command::new("sh").arg("-c").arg(&script)
    .current_dir(&repo_dir)          // 共享的 run 级 clone，不是 per-job
    .process_group(0)
    .spawn();                         // 无 .env_clear()、无超时、无 rlimit
```

问题清单：

1. **无沙箱**：同 UID、同文件系统、同网络命名空间
2. **无超时**：`sleep infinity` 永久占用 runner
3. **无资源限制**：无内存/CPU/pid 限制、无磁盘配额
4. **继承 runner 全部环境变量**（无 `.env_clear()`）→ 脚本 `env` 一下就能拿到
   `GITA_DATABASE_URL`（**含 Postgres 密码**）和 `GITA_RUNNER_TOKEN`
5. **job 之间不隔离**：cwd 是共享的 run 级 clone；`job_work_dir` 建了又删，
   注释说"通过 symlink 共享"但**实际没建 symlink**
6. **信任边界为零**：`.gita-ci.yml` 直接从**被推送的那个 commit** 读出并执行，
   没有受保护分支、没有签名、没有 review gate。默认分支从不参与触发判断。

`docs/runner-design-notes.md:62-66` 对此是坦白的（"That script can read every other job's
workspace, exfiltrate the runner's environment, fill the disk, and never time out"）。

**修法（按性价比）**：`.env_clear()` + 显式白名单注入 → 加 job 超时 →
按 uid/namespace 隔离 workspace → 再考虑 bubblewrap/容器。

---

### P0-7　代码搜索泄露私有仓内容给匿名调用者

**性质**：安全漏洞。**代码层确认**（`search_code` 遍历整个裸仓库根目录，从不查 `is_private`）。

`gita-git/src/lib.rs:358-430` 的 `search_code` 直接 `read_dir` 仓库根目录、逐个 blob 扫描内容，
**完全不知道哪些仓是私有的**。`routes/search.rs:72-98` 的 `search_code` handler **连认证提取器都没有**，
匿名可调。`/api/search`（组合端点）的 code 分支同理。

后果：匿名调用者 `GET /api/search/code?q=<关键词>` 可以拿到**私有仓库的文件路径和源码行内容**。

对比：`search_repos`（仓库搜索）走 DB 且硬编码 `AND is_private = 0`（`gita-db:475`）——仓库搜索是对的，
**只有代码搜索漏了**。

**修法**：`search_code` 需要拿到调用者身份，逐仓判断可见性（公开仓 or 属主）后再决定是否扫描；
或者让代码搜索只走 DB 里已登记且可见的仓列表，而不是扫磁盘。

---

### P0-8　Fork 私有仓 → 泄露为公开仓

**性质**：安全漏洞。**代码层确认**（`repo.rs:606` 硬编码 `false`）。

`post_fork`（`routes/repo.rs:561-624`）：

1. 不调用 `load_readable_repo`/`ensure_readable` → 任何登录用户可 fork 任何人的**私有**仓
2. `state.git.fork_repo(...)` 是真的 `git clone --bare` → **完整历史被复制**
3. `create_repository(..., false, ...)` → **新 fork 被建成公开仓**（`repo.rs:606` 硬编码 `false`）

结果：一个私有仓的全部历史内容，变成攻击者命名空间下的**公开仓**，任何人可匿名 clone。

**修法**：fork 前检查源仓可见性；fork 的 `is_private` 应继承源仓（或至少在源仓私有时强制私有/禁止 fork）。

---

### P0-9　`POST .../prs` / `POST .../issues` 返回 `"id": 0`

`create_pull_request`（`gita-db/src/lib.rs:669`）和 `create_issue` 的 INSERT **没有 `RETURNING`**，
域对象用 `id: 0` 构造，API 直接把它序列化出去。**实测确认**：

```json
{"number": 1, "id": 0, ...}
```

前端若用 `id` 做 key 或后续引用会出问题。

---

### P1-1　runner 的 HTTP clone URL 也是错的（缺 `/git`）

`gita-runner/src/workspace.rs:40-44` 构造 `https://x-access-token:TOKEN@host/{owner}/{repo}.git`，
**同样缺 `/git` 前缀**。集成测试全用 `file://` 绕过，所以这条生产路径**零覆盖**——
真跑起来会去 clone SPA fallback。

同时 token 内嵌在 URL 里 → 出现在 `/proc/*/cmdline`，且写进 `<workspace>/repo/.git/config`。

---

### P1-2　runner 崩溃后 run 永久卡在 `running`

`claim.rs` 的 lease（`claimed_until`，默认 300s）**只在 claim 时写一次，永不续期**。
claim 查询要求 `status = 'queued'`，而 claim 后状态变成 `running`——
所以 `claimed_until < now` 那个条件**是死代码**（只能匹配仍为 `queued`、而 queued 行从没有 `claimed_until` 的行）。

没有 reaper、没有心跳、没有启动恢复扫描。runner `kill -9` → run 和 job 永远 `running`，
既不重试也不标记失败。唯一恢复方式是手工改库。

---

### P1-3　`stage_index` 是死字段，`stages` 顺序不生效

`gita-pipeline/src/lib.rs:357-380` 算了 `stage_index` 存进 `JobPlan`，**全仓库无人读取**。
`expand()` 只按 `needs` 拓扑排序。后果：一个 `stage: deploy` 且**没有 `needs`** 的 job，
可以排在 `stage: build` 的 job **前面**执行。

代码里的文档注释（`lib.rs:365-366`）写的是"先按 stage_index 排，再按 needs 排"——**这是假的**。
`docs/gita-ci-writing-guide.md:152-153` 也这么写。集成测试
`stages_dag_runs_in_dependency_order` 之所以通过，是因为那个 YAML 里所有跨 stage 的 job 都写了 `needs`。

---

### P1-4　`stages` 语义与文档不符 + `deny_unknown_fields` 只覆盖 job 内部

goal.md §10.1 声称"Phase 1 仅支持 `stages`/`script`/`needs` 三种 key，其他字段会被 serde 严格拒绝"。

实际：

- `RawSpec` **没有** `deny_unknown_fields`（因为有 `#[serde(flatten)]` 兜底），
  所以**顶层**的 `image:`/`cache:`/`variables:` 不会被当作"未知字段"拒绝，
  而是**被当成一个 job 名**去解析（然后因为形状不对报 `InvalidJobEntry`）
- `RawJob` **有** `deny_unknown_fields` → job **内部**的未知 key 是硬错误 ✅
- `only`/`except` 已被完整实现，但 goal.md 说它们会被拒绝
- `needs` 只支持数组形式，**对象形式 `{job: a, artifacts: false}` 直接解析失败**
  （writing-guide 说"被解析但只认 job"——假的）
- `stage` **是必填的**，writing-guide 说"缺省为 test"——假的
- 没有"保留名"检查：叫 `cache` 或 `variables` 的 job 能正常解析

---

### P1-5　没有优雅关闭

`gita-server/src/lib.rs:597-602` 直接 `axum::serve(...)`，**没有 `with_graceful_shutdown`**。
仓库里搜不到任何 `ctrl_c()` / SIGTERM 处理。

后果：SIGINT/SIGTERM 立即杀进程 → 进行中的请求被截断、事件行丢失、webhook 投递丢失、
SSH 会话不收尾。容器 `docker stop` 就会触发。

**已修（第五批）**。三件事分开处理：

- **进行中的请求**：`axum::serve(...).with_graceful_shutdown(...)` 停止接受新连接，
  但让已经在跑的请求跑完。
- **事件行与 webhook 投递**：这两件事都是从后台任务里写的，进程一死就没了。
  新增 `gita_server::shutdown::BackgroundTasks` 追踪这些任务，关闭时等它们完成
  （drain 期间新产生的任务也会被等到）。
- **SSH 会话**：`SshServerHandle::shutdown()` 让 accept 循环停下来，
  已经打开的会话跑完。

**实测**：向服务器发 SIGTERM，日志显示
"received SIGTERM, shutting down" → "waiting for 1 background task(s) to finish"
→ "Gita stopped"，退出码 0。信号前 1 毫秒创建的仓库，其事件行完整落库。

新增测试：3 个进程级测试（真的跑 `gita-server` 二进制并发 SIGTERM），
外加 3 个 `BackgroundTasks` 单测。**其中"webhook 投递进行中时关闭"那个测试
真的能抓住回归**——把 drain 去掉，服务器 57ms 就退出了（而不是等投递完成）。

---

### P1-6　`ApiError::Database` / `Io` 把内部错误原文回给客户端

`gita-api/src/error.rs:57-66` 直接 `self.to_string()` 序列化给客户端。
`Database(sqlx::Error)` 的 Display 包含**表名、列名、约束名**；`gita-http/src/lib.rs:507`
更直接：`format!("db error: {e}")` 整个回显。

**修法**：对 `Database`/`Io`/`Internal` 变体，客户端只回通用文案 + 一个关联 id，
详情只进日志。

---

### P1-7　`cancel_run` 完全没有鉴权

见 P0-1 表格最后一行。`pipeline.rs:314-337` 取了 `_user` 和 `(_owner, _repo)` 但**都不用**。
对比同文件的 `trigger_run`（`pipeline.rs:252-256`）是检查了 `owner_id` 的——不一致。

---

### P1-8　`.env.example` 里三个 Pages/重置环境变量名是错的

`.env.example` 写 `GITA_PAGES_ENABLED` / `GITA_PAGES_URL_PREFIX` / `GITA_ALLOW_INSECURE_PASSWORD_RESET`，
但 `docker-compose.yml` 读的是 `GITA_SERVER_PAGES_ENABLED` / `GITA_SERVER_PAGES_URL_PREFIX` /
`GITA_SERVER_ALLOW_INSECURE_PASSWORD_RESET`。**`.env.example` 里那三个键是死的**。

---

### P1-9　`server.database_schema` 被静默忽略

声明在 `gita-server/src/lib.rs:75`，在 `:309` 被消费，但 `Config::load` **从不把它从 TOML 拷进 config**。
`grep database_schema` 在 `lib.rs` 里只有 3 处命中，没有一处是 merge。
只有测试通过直接改 struct 绕过。文档字符串读起来像个支持的特性，实际是惰性的。

---

### P1-10　`GET /api/events` 泄露私有仓的事件流

`routes/misc.rs:22-48` 只要求 `AuthUserExtractor`；带 `?repo=owner/name` 时**不查可见性**就返回该仓事件。
不带参数时返回**全局**事件流（含各私有仓的事件），`limit` 上限 500。

任意登录用户可以读到所有私有仓的 `repo_created` / `push` / `issue_created` 等事件及 payload。

**修法**：加 `?repo=` 时走 `load_readable_repo`；全局流按调用者可见的仓过滤。

---

### P1-11　注册接口是账户 / 邮箱枚举 oracle

`POST /api/auth/register` 对已存在的用户名返回 `409 "username already taken"`，
对已注册的邮箱返回 `409 "email already registered"`（`routes/auth.rs:157-162`）。

匿名调用者可借此枚举**所有用户名和所有注册邮箱**（邮箱在任何其他接口都不暴露）。

（密码重置路径的防枚举做得是对的——统一响应、键缺失而非 null。注册路径是另一处漏洞。）

---

### P1-12　`GET /api/auth/tokens` 重新暴露 PAT 明文

`list_tokens`（`routes/auth.rs:243-258`）把 `t.token` 原样放进响应。
PAT 本身在库里也是**明文存储**（`init.sql:86`，无哈希），且**永不过期**。

所以每次调用 list 都会把全部 PAT 原文重新吐出来——会进浏览器 devtools、代理日志、屏幕共享。

**修法**：PAT 改为只存哈希（`sha256(token)`），列表只返回前缀+哈希+创建时间；原文只在创建时显示一次。

---

### P1-13　`/api/*` 没有 JSON 404 —— 未知 API 路径返回 200 HTML

API router 上**没有** `.fallback()` / `.fallback_service()`，所以未知 `/api/*` 路径会落到 SPA 的
`fallback_service` → 返回 **200 + SPA 的 index.html**。API 客户端拼错路径会拿到一个 200 的 HTML 页面。

同时方法不匹配（`DELETE /api/health`）返回 **405 + 空 body**；axum 自身的 rejection
（坏 JSON、坏路径参数）返回 400 但**不是** `{"error":{...}}` 格式。

结果：错误响应格式有四种（ApiError JSON / axum 的空 body / axum 的 null body / SPA 的 HTML）。

**修法**：给 API router 加一个返回 JSON 404 的 fallback。

---

### P1-14　500 响应泄露绝对路径和 SQL 原文

`ApiError::Database(sqlx::Error)` 和 `ApiError::from(anyhow::Error)` 都直接 `to_string()` 回客户端。
`gita-git` 的错误里带**绝对路径**（`gita-git:194` `open bare repo {path}`），
sqlx 的错误里带**表名/列名/约束名**。

对任何调用者都披露了服务端目录布局和数据库 schema。

---

### P1-15　OpenAPI 规格与实际路由有出入

`docs.rs` 是**手写的静态 JSON 字符串**（不是 utoipa——但 `error.rs:11` 和 `goal.md:236` 都还写着 utoipa）。

漂移测试 `openapi_documents_every_registered_api_route` 只比对**路径**、不比**方法**，所以漏了 3 个
path+method 组合，其中两个正是最敏感的：

- `PATCH /api/auth/me`（改密码/邮箱）—— 规格里只有 `get`
- `PATCH /api/repos/{owner}/{repo}`（**改可见性/改名/转移**）—— 规格里只有 `get`/`delete`
- `POST /api/repos/{owner}/{repo}/branches` —— 规格里只有 `get`

反过来，3 个搜索端点在规格里标了 `security: [{bearerAuth}]`，**实际完全不需要认证**——
照规格写客户端的人不会知道 `/api/search/code` 是匿名的私有内容泄露口。

另外规格里**没有 `components/schemas/Error`**，响应 schema 大面积缺失。

---

### P1-16　`OptionalAuthUser` 把无效 token 静默降级为匿名

`extract.rs:59-65`：token 存在但解析失败时返回 `None`（匿名），**不返回 401**。
用户 token 过期后，UI 会静默退化成"只看到公开内容"的无错误状态，而不是提示重新登录。

---

### P1-17　JWT 不可吊销

无 `jti`、无吊销列表、**无 logout 接口**。7 天有效期的 JWT：
改密码不禁用、吊销 PAT 不影响它、删除用户也不影响它。

---

### P1-18　`delete_repo` 用字符串比较，其他地方用 id 比较

`repo.rs:221` 是 `if owner != user.username`（比较**路径里的 owner 字符串**和用户名），
而其他所有属主检查都是 `repository.owner_id != user.id`。

后果：**群组仓永远无法被其拥有者删除**——路径里的 owner 是群组名，不等于任何用户名。

---

### P1-19　SSH 传输完全不产生事件，也不触发 CI

`gita-ssh` 里没有任何 `bus.publish`——**通过 SSH push 不会产生 `PushEvent`，也不会触发 CI**。
（`gita-http` 里有完整的 push 事件 + CI 触发逻辑。）

后果：同一个仓库，走 HTTP push 会跑 CI，走 SSH push 不会。
用户会以为"CI 没触发"是配置问题，实际上是传输方式的差异。

**已修（第三批之后）**。事件发布与 CI 触发抽到新 crate `gita-push`，
两个传输层都调它。SSH 侧的做法与 HTTP 侧**完全一致**：解析客户端发来的
ref-update 命令（SSH 的 stdin 与 HTTP 的请求体是同一种 pkt-line 格式），
再逐个核对 ref 是否真的更新了。

**为什么不用服务端返回的 `report-status`**（原修法建议的做法）：
git 客户端在 SSH 下会请求 `side-band-64k` 能力，服务端一旦接受，
它写的 stdout 就被包进 side-band 帧，不再是裸的 pkt-line 流。
按 report-status 解析会读到帧头而不是内容。（实测确认。）

**顺带修掉的静默失败**：CI 触发那几步原本用 `.ok()` 吞掉所有错误——
`.gita-ci.yml` 写错（比如把 `script:` 写成 `steps:`，解析器不认）
会导致 CI 静默不跑，没有任何日志。现在每一步都记录错误。

新增 2 个 SSH 集成测试：push 产生事件并入队 CI run；被拒绝的 push
不产生任何事件。

---

### P1-20　鉴权逻辑在五处重复，且语义已经不一致

| 层 | 位置 | 公共仓匿名读 | 私有仓拒绝方式 |
|---|---|---|---|
| Git HTTP (smart) | `gita-http:479` | ✅ 可以 | **401** |
| Git HTTP (dumb) | 同上 | ✅ 可以 | **401**（第一批修好） |
| Git SSH | `gita-ssh:258` | ❌ **必须已认证**（SSH 总要公钥） | "permission denied" |
| REST API 读路径 | `visibility.rs:23` | ✅ 可以 | **404**（刻意不泄露存在性） |
| REST API 写路径 | `visibility.rs:79` | ❌ 仅属主 | **404** |

五处各自实现"能不能访问这个仓"，改一次策略要动五个文件。

更麻烦的是**群组维度整个缺失**：`gita_core::Repository` 没有 `group_id` 字段，
而 `repositories` 表里有。群组仓的 `owner_id` 存的是**群组创建者的用户 id**，
于是所有基于 `owner_id` 的判定对普通群组成员都不成立——成员在群组仓里
连 issue 都提不了，即使那个仓是公开的，即使他的角色写着 `developer`。
`group_members.role` 存了但完全不参与任何访问决策。

另外 HTTP 的 Basic 认证**完全忽略用户名**：
`text.split_once(':')?` 之后只把**密码位**当 token 解析（`gita-http:585-586`）。
所以 `Basic alice:<bob的token>` 会认证成 **bob**。实际不构成越权
（属主判定用的是 `user.id == repo.owner_id`），但这个字段是纯装饰，容易误导。

**修法**：把"can_read / can_write"抽成一个共享函数，所有层调用它。
（已于第三批完成，见 §10。）

---

### P1-21　`default_branch` 未校验就进 `git symbolic-ref`

`gita-git:104-110` 把未校验的 `default_branch` 直接拼进 `git symbolic-ref HEAD refs/heads/{default_branch}`。
仓库名有字符白名单（`[A-Za-z0-9._-]`），但 `default_branch` 走的是**另一条**路径——
`create_repo` / `update_repo` 传进来时没有同等约束。

---

### P1-22　git 命令的参数没有 `--` 分隔

`gita-git:786-898` 的 `git diff` / `git show` / `git rev-list` 把用户可控的 ref
作为**位置参数**传入，没有 `--` 也没有 `--end-of-options`。
`git diff --output=<file>` 是一个真实的写原语。

（`safe_repo_path` 对仓库**名**的校验是严的，但 ref 值来自 API 参数，校验更松。）

---

### P1-23　`/commits/{ref}` 与 `/tree?ref=` 的 ref 解析不一致

**实测**（同一个仓、同一批 ref）：

```
GET .../commits/HEAD            -> 500  internal error: invalid sha HEAD: A hash sized 4 hexadecimal characters is invalid
GET .../commits/main             -> 500
GET .../commits/master           -> 500
GET .../commits/refs/heads/main   -> 200
GET .../tree?ref=HEAD            -> 200
GET .../file?path=a.txt&ref=HEAD -> 200
```

根因：`gita-git/src/lib.rs:908-916` 的 `get_commit` 直接用
`gix::ObjectId::from_hex(sha)`，**完全绕过了同文件里的 `classify_reference`（:1203）**。
而 `list_tree` / `read_file` 都走了 `classify_reference`，所以能认 `HEAD` 和短分支名。

结果：**只有 `refs/heads/x` 和 40 位 hex 能用**，`HEAD` 和短分支名不能——
但 `git` 的常规用法里 `HEAD` 是最常用的一个。

**修法**：`get_commit` 改用 `classify_reference` + `resolve_reference_id`（和 `list_tree` 同一套）。

---

### P1-24　不存在的 sha 返回 500，应为 404

```
GET .../commits/deadbeef -> 500 {"error":{"message":"internal error: invalid sha deadbeef: A hash sized 8 hexadecimal characters is invalid"}}
```

`gix::ObjectId::from_hex` 的失败被当成 `anyhow` 错误 → `ApiError::Internal` → 500。
一个"资源不存在"被报成"服务器内部错误"，也让客户端无法区分"没这个 commit"和"服务器坏了"。

**修法**：先判 sha 长度/字符集，不是合法 object id 就返回 404；
`find_commit` 失败同理。

---

### P1-25　未知 ref 的 tree 返回 200 + 空 entries，应为 404

```
GET .../tree?ref=nonexistent -> 200 {"entries":[],"path":"","ref":"nonexistent"}
```

同一个接口里，`file?ref=nonexistent` 正确返回 **404 "file not found"**。
所以 tree 和 file 对"ref 不存在"的处理也不一致。

**后果**：前端拿到一个"看起来正常的空目录"，无法区分"这个目录真的是空的"和
"分支名写错了"——用户会以为仓库是空的。

**修法**：tree 解析 ref 失败时返回 404（与 file 一致）。

---

### P1-26　无 body 的 POST 返回 415

```
POST /api/repos/{o}/{r}/fork            (无 body, 无 Content-Type) -> 415 Unsupported Media Type
POST /api/repos/{o}/{r}/branches        (无 body, 无 Content-Type) -> 415
POST /api/repos/{o}/{r}/prs/{n}/close   (无 body, 无 Content-Type) -> 200  ✅
PUT  /api/repos/{o}/{r}/star            (无 body, 无 Content-Type) -> 200  ✅
```

axum 的 `Json<T>` 提取器要求 `Content-Type: application/json`，
**没有 body 时直接 415**。而 `close_pr` 这类不声明 body 的端点就正常。

问题在于：这两个端点的 body 字段是**全可选**的
（`ForkRequest { name: Option<String> }`、`CreateBranchRequest { name, ref, ... }`），
调用方很自然会直接 POST 不带 body。

现有 Web UI 不触发（`api.ts` 总是 `JSON.stringify(...)`），
但任何第三方 API 客户端按 REST 惯例写都会撞上。

**修法**：用 `Option<Json<T>>` 或在提取器前加一个允许空 body 的包装；
或者对这些端点显式 `Json<Option<ForkRequest>>`。

---

### P1-27　SSH 公钥可重复注册，无唯一约束

```
POST /api/auth/keys  (同一把公钥) -> 200, 200, 200
GET  /api/auth/keys  -> 3 条一模一样的记录（id 不同，title 不同）
```

`ssh_keys` 表**没有唯一约束**（`20261020050000_ssh_keys.sql` 只有 `user_id` 索引），
`add_ssh_key` 是无条件 INSERT。

**后果**：
- 列表里出现重复项，用户以为添加失败
- `find_ssh_key_by_fingerprint` 是全表扫描逐个算 SHA-256（见 P2-3），
  重复 key 让扫描量线性膨胀
- 没有"这把 key 已经加过了吗"的反馈

**修法**：加 `UNIQUE (user_id, public_key)`，插入时用 `ON CONFLICT` 返回已有行或报 409。

---

### P2-1　无事务导致的并发缺陷

| 位置 | 问题 |
|---|---|
| `gita-db:661` `create_pull_request` | `MAX(number)+1` 后再 INSERT，无锁 → 并发创建撞 `UNIQUE(repo_id, number)`，一个请求 500 |
| `gita-db:841` `create_issue` | 同上 |
| `gita-db:1803` `create_group` | 插 group 和插 owner membership 是两句 → 中间崩溃留下无 owner 的群组 |
| `gita-http:254-310` CI 入队 | `insert_pipeline_run` → `insert_event` → `set_pipeline_run_trigger_event` 三句无事务 → 第三句失败则 `trigger_event_id` 为 NULL（代码有兜底 warn） |

---

### P2-2　`list_prs` 每个 PR 起一个 `git merge-base` 子进程

`pr.rs:174-179` 对每个 PR 调 `with_conflicts` → `check_merge_conflict` →
`git merge-base` + 临时目录三方合并。`limit` 上限 200 → **单次请求最多 200 个 git 子进程**。

---

### P2-3　`find_ssh_key_by_fingerprint` 每次 SSH 认证扫全表

`gita-db:1214-1229` 加载**全系统所有 SSH 公钥**并逐条算 SHA256。每次 SSH 认证都做一遍。
应改为入库时存指纹、查询时直接索引。

---

### P2-4　`groups.name` 全局唯一，可与用户名冲突

`groups.sql:11` 是 `name TEXT NOT NULL UNIQUE`（无命名空间）。
`repositories` 是 `UNIQUE(owner_name, name)`，而 `get_repository_by_name` 只按 `owner_name`+`name` 查
——用户和同名群组的仓库会**无法区分**。

---

### P2-5　事件持久化非原子

`gita-server/src/lib.rs:334` 的 persist hook 整体在 `tokio::spawn` 里 → `publish()` 返回时
`events` 行可能还不存在。`gita-http:302-308` 那条路径在 `insert_event` 失败时会用 `id=0` 再发一次，
persist hook 会**再插一次** → 可能出现**重复行**。

---

### P2-6　SPA 的 `spa_fallthrough` 返回 200 且不带安全头

`pages_site.rs:258-272` 返回 200 + SPA 外壳，**不带 CSP / nosniff**（Pages 正常响应是带的）。
且对任何两段式 URL 都返回 200，服务器永远不会对这类 URL 返回 404。

---

### P2-7　Pages 的 `url` 字段忽略 `pages_url_prefix`

`pages.rs:161-165` 总是返回 `/{owner}/{repo}/`，但配了 `pages_url_prefix = "/p"` 时站点实际在
`/p/{owner}/{repo}/`。前端 `PagesPanel` 会把这个错误的 URL 显示给用户。

---

### P2-8　Pages 站点永久遮蔽 SPA 的同名仓库页

`/{owner}/{repo}` 同时是 Pages 站点路由和 SPA 仓库页路由。启用了 Pages 就没有任何办法看仓库 UI。

---

### P2-9　PAT 明文存储

`access_tokens.token` 是明文 TEXT + UNIQUE，无哈希、无过期列。DB 泄露 = 全部 token 泄露。
（另见 P1-12：`GET /api/auth/tokens` 还会把原文重新吐出来。）

---

### P2-10　`commits` 表生产环境从未被写入

`insert_commit`（`gita-db:567`）**无任何调用方**。提交历史全部从 git 现读。该表和该函数是死代码。

---

### P2-11　删仓库顺序错 + `pull_requests`/`issues` 无外键

`repo.rs:229-230` 先删 DB 行再删目录 → 文件系统失败则目录变孤儿（API 再也访问不到，
同名重建会撞车）。且 `pull_requests`/`issues`/`commits` 表**没有到 `repositories` 的外键**
（后加的表都加了，这三张 init 里的没有）→ 删仓后永久孤儿。

---

### P2-12　无用户删除路径

全仓库 `DELETE FROM users` 零命中。`repositories.owner_id` / `access_tokens.user_id` 无外键，
所以一旦有删除能力，会连带孤儿化用户的仓库、PAT、群组、事件。

---

### P2-13　测试的 teardown 是坏的 —— 已泄漏 6630 个测试 schema

`integration/api/tests/common.rs:30-47` 的 `TestAppGuard::drop` 里用 `handle.spawn(...)` 起了一个
fire-and-forget 任务删 schema —— 但 `#[tokio::test]` 的 runtime 在测试返回时就被销毁，
**这个任务被取消，schema 永远删不掉**。dev 库里的 `gita_it_*` / `gita_test_*` schema 已累积到 6630 个，
每次跑测试还在涨。

---

### P2-14　sqlx 迁移用数据库级 advisory lock → 测试串行化

sqlx 的 `migrate!` 用 `pg_advisory_lock(current_database())`，是**数据库全局锁**。
所有并行测试进程都排在这个锁上，实测 25+ 后端在等 `advisory` lock（等了 93 秒）。
整套测试因此要 ~7 分钟，其中 `gita-db` 单个 binary 就要 129 秒。

---

### P2-15　release tarball 的 Dockerfile / compose 不能用

`release.yml:105-126` 打包的布局是 `gita-<TAG>/{gita-server, web/, Dockerfile, docker-compose.yml, ...}`，
但 tarball 里的 `Dockerfile` 写的是 `COPY target/release/gita-server /app/gita-server`（路径不存在），
`docker-compose.yml` 写的是 `context: ..` + `dockerfile: docker/Dockerfile`（tarball 里没有 `docker/` 目录）。
用户解压后直接 `docker build` 会失败。

另外 **`gita-runner` 不在 Docker 镜像里，也没有 compose 服务** → 容器化部署根本跑不了 CI。

---

### P2-16　其他

| # | 问题 | 位置 |
|---|---|---|
| 1 | **`dotenvy` 是死依赖**（声明了从不调用，`.env` 不会被二进制自动加载） | `gita-server/Cargo.toml:26` |
| 2 | **`middleware::rate_limit_by_ip` 是死代码**（限流全在 handler 内手动做） | `middleware.rs:89` |
| 3 | **`auth_routes_except_login()` 是死代码** | `routes/auth.rs:93` |
| 4 | **`/api/health` 永远返回 200**（DB 挂了也是 `status: degraded`），compose healthcheck 用 `curl -f` 抓不到；且只查 `SELECT 1`，schema 坏了也报 ok | `misc.rs:60` |
| 5 | **无 CORS 层**（`tower-http` 的 cors feature 声明了但没用），也无 `TraceLayer` | `Cargo.toml:33` |
| 6 | **无 `cargo fmt --check` / `cargo clippy` 门禁**（且 fmt 当前失败 455 处，加门禁要先跑一次 fmt） | `.github/workflows/` |
| 7 | **Playwright e2e 不在 CI 里**（最脆弱的前端部分无 CI 保护） | `.github/workflows/ci.yml` |
| 8 | **三个不同的 Postgres 版本**：CI `postgres:17`、compose `pgvector/pgvector:pg16`、本地 `pg17`；且 `pgvector` 没用上（无向量列） | 三处 |
| 9 | **前端大量已实现但无 UI 的 API**：fork、hooks、群组成员管理、SSH keys、密码修改、头像上传、事件流、cancel run、删仓库、群组建仓 | `web/src/lib/api.ts` |
| 10 | **README 说"Settings → SSH keys"注册公钥，但 UI 里没有这个页面** | `README.md:147` |
| 11 | **`web/README.md` 是 Vite 模板原文**，没写任何 Gita 相关内容 | `web/README.md` |
| 12 | **`i18n` 的 `ERROR_MAP` 对多词前缀失效**：正则 `^\w+:\s*` 匹配不了 `not found: ` / `bad request: ` / `rate limited: `，**9 个后端错误分类里 6 个直接漏英文** | `i18n/index.tsx:134` |
| 13 | **`error.notFound` 键在 zh/en 两个字典里都不存在**，404 页面直接显示键名 | `App.tsx:140` |
| 14 | **Pages 标签页在侧边栏不高亮**（`Layout.tsx:290-297` 没有 `settings` 分支） | `Layout.tsx:290` |
| 15 | **SSE 日志流无法鉴权**：`EventSource` 发不了 `Authorization` 头，私有仓的日志流 404 | `api.ts:445` |
| 16 | **前端无 401 处理**：`api.ts` 从不检查 401，token 过期后 UI 静默卡在登录态 | `web/src/lib/api.ts` |
| 17 | **`useAuth.refresh()` 从不被调用**，`localStorage` 里的用户信息永不校验 | `web/src/hooks/useAuth.ts:81` |
| 18 | **搜索框无 catch**：搜索失败时是 unhandled rejection，且用户看到的是"无结果" | `Layout.tsx:517` |
| 19 | **`run_dev.sh` 查 `gita_gita` 镜像，实际是 `docker_gita`** → 每次 start 都无谓重建前后端 | `run_dev.sh:224` |
| 20 | **仓库搜索硬编码 `is_private = 0`**，用户搜不到自己的私有仓 | `gita-db:471` |
| 21 | **`goal.md` §6 列的 `docs/architecture.md`、`docs/api.md`、`docs/mcp.md` 三个文件都不存在** | `goal.md` |
| 22 | **`goal.md` §5.7 列的 `POST /api/agents/run` 不存在** | `goal.md:232` |

---

## 9. 设计文档与实现的偏差

| 文档 | 声称 | 实际 |
|---|---|---|
| `goal.md:115` / `:309` | "仅支持 fast-forward 合并"、"冲突检测未做" | **两者都已实现**（真实 FF + 双父 merge commit + 三方 merge 冲突检测） |
| `goal.md:355` | "Phase 1 仅支持 stages/script/needs，其他字段严格拒绝" | 实际还有 `jobs:`/`only:`/`except:`；顶层未知字段不被当未知字段拒；`needs` 对象形式不支持；`stage` 必填 |
| `goal.md:357` / `:364` / `:373` | "SSE 日志流、cancel 推迟到 Phase 2"、"5 个 REST 端点"、"SPA 2s 轮询" | SSE 和 cancel **都已实现**；端点是 **7 个**；SPA 用的是 SSE 不是轮询 |
| `goal.md:372` | "`gita worker register`" | 实际是 `gita-server worker register`（二进制名是 `gita-server`） |
| `goal.md` §6 | `docs/architecture.md`、`docs/api.md`、`docs/mcp.md` | **三个文件都不存在** |
| `goal.md` §5.5 | "MCP Server stdio 已实现" | crate 已移除，只剩 `/api/mcp/tools` 静态清单 |
| `README.md:147` | "在 web UI 的 Settings → SSH keys 注册公钥" | **UI 里没有这个页面** |
| `README.md:116` | `git clone http://.../git/<you>/<repo>.git` | **正确**，但 Web UI 给的是错的（少 `/git`） |
| `README.md` 架构树 | 8 个 crate | 实际 11 个（漏了 pipeline/runner/ssh） |
| `README.md:171` | 本地跑指向 `localhost:5432` | compose 的 postgres **没暴露宿主端口** |
| `docs/gita-ci-writing-guide.md` | 多处 | stage 默认值、needs 对象形式、stage_index 排序、glob `*` 语义、保留名、CI 变量表 —— 见 P1-3/P1-4 |
| `docs/runner-design-notes.md` | 并发/secret/心跳 三行 | 与实际不符（并发未限、secret 功能不存在、无心跳），其余准确 |

---

## 10. 修复进度

> **编号说明**：本文 §8 的问题编号是权威编号。本节早期版本另起了一套编号，
> 与 §8 相差 1～4（例：§8 的 P1-11「注册枚举」在本节旧表里写作 P1-10）。
> 下表统一采用 **§8 的编号**，旧编号不再使用。

### 第一批（2026-10-22）

第一批修复已完成并实机验证。**`cargo test --workspace`：225 passed / 0 failed**
（修复前 213；新增 12 个测试）。

### 已修复

| 编号 | 问题 | 修法 | 验证 |
|---|---|---|---|
| **P0-1** | 私有仓可被匿名完整读取（dumb-HTTP 绕过） | `is_read` 判定从子串匹配改为**路径结构化**（`request_verb` 取第三段）；只有 `git-receive-pack` 是写，其余一律是读 | 实测 `HEAD`/`objects/**`/`objects/info/packs`/blob 全部 401；新增 2 个单测 |
| **P0-2** | PR 合并丢目标分支的改动 | 抽出 `compute_merge_tree`（`read-tree -m` + `write-tree`），`merge_branch` 改用**merge tree** 而非 head 的 tree | 实测合并后 `only_main.txt` 保留；新增单测，回退修复即失败 |
| **P0-3** | 被拒的 push 也发事件、也触发 CI | 新增 `ref_update_applied`：比对 ref 当前值，只对**真正生效**的 ref 更新发事件 | 代码层 + 与既有 CI 测试兼容 |
| **P0-4** | 写路径 16 个 handler 绕过可见性 | 新增 `visibility::load_writable_repo` / `ensure_writable`，替换 PR / issue / comment / star / fork 的裸查询 | 实测非属主全部 404；新增 5 个安全回归测试 |
| **P0-5** | 代码搜索泄露私有仓内容 | `search_code` 接受"允许扫描的仓集合"；API 层按调用者可见性计算 | 实测匿名/非属主 0 命中，属主正常命中 |
| **P0-6** | fork 私有仓 → 公开仓 | fork 前校验源仓可见性；fork **继承源仓的 `is_private`** | 实测非属主 fork 私有仓 → 404 |
| **P0-7** | `GET /api/events` 泄露私有仓事件 | 带 `?repo=` 时走 `load_readable_repo`；全局流按可见仓过滤 | 实测非属主 404 |
| **P0-9** | PR / Issue 返回 `"id": 0` | INSERT 加 `RETURNING id` | 实测返回真实 id |
| **P0-8** | clone URL 缺 `/git`（前端 + runner） | UI 与 `gita-runner` 的 clone URL 都加 `/git` 前缀；e2e 断言同步更新 | 新增 e2e：**真的跑 `git clone`** 验证 UI 给的 URL 可用 |
| **P1-7** | `cancel_run` 无鉴权 | 校验 owner + run 属于该仓 | 代码层 |
| **P1-11** | 注册接口是账户枚举 oracle | 见第二批 | — |
| **P1-12** | `GET /api/auth/tokens` 重返 PAT 明文 | PAT 改哈希存储；列表不再返回原文（`token` 字段为空串） | 提交 `79477b3` |
| **P1-19** | `/commits/{ref}` 只认 40 位 hex | `get_commit` 改走 `classify_reference` + `resolve_reference_id` | 实测 `HEAD`/`main` 均 200 |
| **P1-20** | 不存在的 sha → 500 | 新增 `not_found_or_internal` 映射 | 实测 404 |
| **P1-21** | 未知 ref 的 tree → 200 空列表 | 显式检查 ref 存在性；**空仓例外**（无任何 commit 时返回空列表，否则空仓欢迎卡会 404） | 实测未知 ref → 404，空仓 → 200 |
| **P1-22** | 无 body 的 POST → 415 | 改用 axum 原生 `Option<Json<T>>` | 实测 fork 无 body → 200 |
| **P1-23** | SSH 公钥可重复注册 | 新 migration 加 `UNIQUE (user_id, public_key)` + 幂等插入 | 新增单测 |

### CI runner 的加固

| 项 | 修法 |
|---|---|
| job 继承 runner 全部环境变量（含 DB 密码和 runner token） | `env_clear()` + 白名单注入；`GITA_DATABASE_URL` / `GITA_RUNNER_TOKEN` 显式移除 |
| job 无超时 | 新增 `GITA_JOB_TIMEOUT_SECS`（默认 3600），超时杀整个进程组，状态记 `failed` |
| job 之间共享同一个工作目录 | 每个 job 用 `git worktree` 得到独立 checkout |
| fetch 失败静默改跑分支 tip | 改为**让 run 失败**，避免"报告的 sha ≠ 实际执行的 commit" |

### 未修复（下一批）

- **P1-5** 无优雅关闭
- **P2-\*** 大部分 P2 项
- **P0-6 的长期项**：CI runner 仍无真正的沙箱（bubblewrap / 容器）——第一批只做了环境隔离、超时、目录隔离
- **P1-19** SSH push 仍不产生事件、不触发 CI
- **P1-15** OpenAPI 规格与实际路由仍有出入（漂移测试只比对 path、不比对 method）

### 第二批（2026-10-23）

第二批修复完成并实机验证。**新增 10 个集成测试 + 2 个 e2e**；
`cargo test --workspace` 233 passed / 0 failed，Playwright 20 passed。

| 编号 | 问题 | 修法 | 验证 |
|---|---|---|---|
| **P1-11** | 注册接口是账户 / 邮箱枚举 oracle | 冲突时统一返回同一条 message（"username or email already in use"），用户名冲突与邮箱冲突的**响应体逐字节相同**；额外算一次 dummy 密码哈希，把响应时间拉平 | 新增 2 个集成测试；冲突响应用 `assert_eq!(body_a, body_b)` 断言不可区分 |
| **P1-13** | `/api/*` 无 JSON 404（未知路径返回 200 HTML） | API router 加显式 catch-all `route("/api/{*rest}", any(fallback_404))`，返回标准 `{"error":{...}}` 信封 | 实测未知路径 → 404 + `content-type: application/json`；新增 2 个集成测试（GET + 全方法） |
| **P1-14** | 500 响应泄露绝对路径和 SQL 原文 | `ApiError::Internal`/`Database`/`Io` 对客户端只回 "internal error"，详情只进日志 | 提交 `79477b3`（第一批已修，本批更新文档） |
| **P1-16** | `OptionalAuthUser` 把无效 token 静默降级为匿名 | 解析失败**返回 401**，不再降级为匿名；无 token 仍走匿名 | 实测无效 token → 401、匿名 → 200；新增集成测试 |
| **P1-17** | JWT 不可吊销 | 新增 `jwt_revocation_list` 表（migration `20261022090000`）；JWT 带 `jti`；`POST /api/auth/logout` 把 `jti` 加入吊销表；`resolve_bearer` 每次校验吊销状态 | 实测 logout 前 `me`→200、logout 后同 token→401；新增 3 个集成测试 + 2 个 e2e |
| **P1-18** | `delete_repo` 用字符串比较 | 改为比较 `repository.owner_id == user.id`，与全站其它属主判定一致 | 新增集成测试；非属主 403、属主 200 |

**JWT 吊销的两个设计决定**（值得记下来，避免以后被当成 bug 改回去）：

1. **吊销的是"这一个 token"，不是"这个用户"**。同一用户两次登录得到两个 JWT，
   登出其中一个不影响另一个——这与"登出=结束当前会话"一致，
   也与 GitHub/Gitea 的行为一致。
2. **用 PAT 调 logout 不会吊销 PAT**。PAT 是长生命周期凭据，没有"会话"可结束。
   接口会明确告知"PAT 未被吊销，请用 `DELETE /api/auth/tokens/{id}` 撤销"，
   而不是假装登出成功。

**logout 的实现细节**：`verify_jwt` 被拆成 `decode_jwt`（纯解码，不查库）
和 `verify_jwt`（解码 + 查吊销表）。logout 只用 `decode_jwt`——调用方的
extractor 已经完成认证（含吊销检查），再查一次是多余的 DB 往返。

**旧 token 的兼容**：`jti` 是 `Option<String>` + `#[serde(default)]`，
而不是必填。升级前签发的 JWT 没有 `jti`，若把它设成必填字段，
serde 解码会因 "missing field `jti`" 失败——**等于把每一个在线用户
强制登出**。现在的做法是：`decode_jwt` 发现没有 `jti` 时，用
`"legacy:" + sha256(token)` 派生一个**稳定**的 id。它是确定性的，
所以同一个旧 token 永远映射到同一条吊销记录，旧 token 同样可以被登出。

**API 404 的实现约束**：`api_router` **不能**用 axum 的 `.fallback()`——
server 层要把它和 `gita-web` 的 SPA fallback `merge` 到一起，
而两个带 fallback 的 Router 无法 merge（`Cannot merge two Routers that both
have a fallback`）。所以 API 的 404 走**显式通配路由**而不是 fallback。
具体路径永远优先于通配路由，真实端点不受影响。

**前端配套**：登出按钮原本只清 localStorage，服务端的 JWT 仍然有效 7 天。
现在 `useAuth.logout()` 会先 `POST /api/auth/logout`（fire-and-forget，
失败不阻塞 UI）再清本地状态；新增 e2e 断言"登出后原 token 在服务端返回 401"。

### 仍未修复

- **P1-15** OpenAPI 规格漂移（只比对 path 不比对 method；3 个搜索端点标了 auth 但实际匿名）
- **P2-\*** 大部分 P2 项
- **CI runner 仍无真正的沙箱**（bubblewrap / 容器）

### 第五批（2026-10-23）：优雅关闭

**P1-5**。服务器以前是被"杀"而不是被"停"的：`SIGINT`/`SIGTERM` 直接终结进程，
进行中的请求被截断，后台任务里的事件行和 webhook 投递一起消失。
`docker stop` 发的正是 SIGTERM。

三件事分开处理：

- **进行中的请求** —— `axum::serve(...).with_graceful_shutdown(...)`：
  停止接受新连接，但让已经在跑的请求跑完。
- **事件行与 webhook 投递** —— 这两件事都是从后台任务里写的，进程一死就没了。
  新增 `gita_server::shutdown::BackgroundTasks` 追踪它们，关闭时等完成
  （drain 期间新产生的任务也会被等到，不会漏）。
- **SSH 会话** —— `SshServerHandle::shutdown()` 让 accept 循环停下来，
  已经打开的会话跑完。

  这里有个坑值得记下来：关闭信号**不能绑在 handle 的生命周期上**。
  用 `watch::channel` 时，handle 一被 drop，channel 就关闭，
  接收端的 `changed()` 立即返回 `Err`——监听循环会立刻把它当成
  "该停了"而退出。而 handle 经常在读出端口后就被丢掉
  （集成测试里的 `start_ssh` 就是只返回端口），于是**服务器在启动
  之后立刻就死了**，表现为 "Connection refused"。

  现在的做法是 `Arc<ShutdownSignal>`：`AtomicBool` + `Notify`，
  handle 和监听任务各持一份。handle 被 drop 不影响服务器，
  只有显式调用 `shutdown()` 才会停它。

**实测**：发 SIGTERM，日志显示 "received SIGTERM, shutting down" →
"waiting for 1 background task(s) to finish" → "Gita stopped"，退出码 0。
信号前 1 毫秒创建的仓库，事件行完整落库。

新增测试：3 个进程级测试（真的跑 `gita-server` 二进制并发 SIGTERM）+
3 个 `BackgroundTasks` 单测。

**其中"webhook 投递进行中时关闭"那个测试真的能抓住回归**——把 drain 去掉，
服务器 57ms 就退出了，而不是等投递完成。这类测试如果只断言"事件行还在"
是抓不住的：事件写入快，信号来之前就写完了。

### 第四批（2026-10-23）：SSH push 也产生事件、也触发 CI

**P1-19**。事件发布与 CI 触发原本是 HTTP 传输层的私有逻辑；SSH 传输层
跑同一个 `git receive-pack`、更新同一批 ref，然后什么都不做。同一个仓库
走 HTTP push 会跑 CI，走 SSH push 不会，用户只会以为是配置问题。

新 crate **`gita-push`** 承接这段逻辑，两个传输层都调它：

- `publish_push()`：发 push 事件、按 `.gita-ci.yml` 入队 CI run
- `parse_receive_pack_commands()`：解析客户端发来的 ref-update 命令
- `ref_update_applied()`：核对某个声称的更新是否真的落地了

**两个传输层用的是同一套判定**：都解析客户端的命令（SSH 的 stdin 与
HTTP 的请求体是同一种 pkt-line 格式），再逐个核对 ref 是否真的更新了。
所以被拒绝的 push 在两边都只发布零个事件、触发零个 CI。

**没有采用服务端返回的 `report-status`**（原修法建议的做法）：git 客户端
在 SSH 下会请求 `side-band-64k`，服务端一旦接受，它写的 stdout 就被包进
side-band 帧，不再是裸 pkt-line 流——按 report-status 解析会读到帧头。
这是实测确认的，不是推测。

**顺带修掉一个静默失败**：CI 触发那几步原本用 `.ok()` 吞掉所有错误。
`.gita-ci.yml` 写错了格式（例如把 `script:` 写成 `steps:`）会导致 CI
静默不跑，日志里一个字都没有。现在每一步都记录错误。

新增测试：2 个 SSH 集成测试（push 产生事件并入队 CI run；被拒绝的 push
不产生事件），4 个 `gita-push` 单测（命令解析、能力段剥离、删除识别、
垃圾输入不 panic）。

### 第三批（2026-10-23）：访问控制收敛成一个规则

**P1-20**。这是本文反复指向的那件事：P0-1、P0-4、P0-7、P0-8 四个漏洞
是同一个问题的不同表现。现在它只有一个实现了。

**规则写在 `gita-core::access`**，是个纯函数，不依赖数据库：

```rust
pub enum Access { Read, Write, Push, Admin }
pub fn can_access(repo: &Repository, principal: &Principal, action: Access) -> bool
```

四个等级的划分不是随意的——它们对应仓库里真实存在的操作强度：

| 等级 | 含义 | 谁 |
|---|---|---|
| Read | 浏览、克隆、读文件/提交/issue | 公开仓任何人；私有仓属主 |
| Write | 提 issue、评论、PR、star、fork | 属主（群组仓：任意成员） |
| Push | push 代码、建/删分支和标签 | 属主（群组仓：developer 以上） |
| Admin | 改名、转移、改可见性、删除、webhook | 属主（群组仓：仅群组 owner） |

`Push` 与 `Admin` 分开，是因为**触发 CI 运行属于 Push**——CI 会执行仓库里
的代码，所以能跑 CI 的人必须能改代码。这两件事以前都是 `owner_id` 一刀切，
现在按实际风险分级。

**群组角色终于参与判定了**（这解决了 §11 第 35 条的待决问题）。
`group_members.role` 原本存了但没人读。现在按 GitLab 语义：

| 角色 | Read | Write | Push | Admin |
|---|---|---|---|---|
| owner（群组创建者） | ✓ | ✓ | ✓ | ✓ |
| maintainer | ✓ | ✓ | ✓ | |
| developer | ✓ | ✓ | ✓ | |
| member | ✓ | ✓ | | |
| guest | 仅公开仓 | | | |

角色是 API 已有的那五个（`owner` 保留给创建者，API 不允许分配）。
**数据库里没定义过的角色按 `guest` 处理**——权限最保守的读法，
一个拼写错误不该换来更多权限。

**修好了群组的一个真 bug**：普通成员此前在群组仓里**连 issue 都提不了**
（哪怕是公开仓、哪怕角色是 developer），因为所有写路径要求
`user.id == repository.owner_id`，而群组仓的 `owner_id` 是群组创建者。
现在成员能读私有群组仓、能提 issue/PR；developer 以上能 push。

**为此要给模型加字段**：`gita_core::Repository` 原本没有 `group_id`，
而 `repositories` 表里有——这个维度在访问模型里整个不存在。
`row_to_repo` 也没读那一列。现在模型、INSERT、SELECT 都带上了它，
群组仓的 `group_id` 也在 INSERT 时一次写入（以前是建完仓再 `UPDATE`，
中间存在一个"没有群组的群组仓"的状态）。

**拒绝方式仍然是各传输层自己的**，这不是遗留问题，是协议差异：

- REST API 用 **404**（"不存在"和"不是你的"无法区分——告诉匿名调用者
  "它存在但你看不到"本身就是泄露）
- Git over HTTP 用 **401 + `WWW-Authenticate`**——git 客户端只有在收到
  401 挑战时才会去问凭据。如果这里返回 404，一个**有权限**的用户 clone
  私有仓也会失败，因为他根本不会被提示发送 token
- Git over SSH 只能说 "permission denied"（没有状态码）

所以这次统一的是**判定**，不是**应答**。原先 REST 内部也不统一
（读路径 404，写路径 403），现已全部是 404。

**Basic 认证的用户名不再被忽略**（P1-20 的附带项）。原先
`Basic alice:<bob的token>` 会认证成 bob。现在用户名必须与 token 属于
同一用户，否则拒绝。

**顺带修掉的**：`gita-db` 的 `create_repository` 增加 `group_id` 参数，
群组仓不再需要建完再 UPDATE。

### 验证

- `gita-core` 新增 **14 个**访问规则单测（纯函数，无需数据库）
- 新增 `integration/api/tests/group_access.rs`：**5 个**集成测试
  （成员提 issue、私有群组仓可见性、guest 限制、仅群组 owner 能管理、
  群组成员权限不外溢到别的仓）
- 新增 SSH e2e：**群组 developer 能通过 SSH push**（此前不可能）
- 新增 HTTP 单测：**Basic 认证的用户名必须匹配 token**
- 5 个既有集成测试的断言从 403 改为 404（统一拒绝方式的行为变更）

---

## 11. 剩余问题的建议修复顺序

**第 0 批（立刻修，都是已实机确认的严重问题）**

1. **dumb-HTTP 绕过私有仓鉴权** → P0-1。匿名可下载私有仓全部源码。
   改 `is_read` 判定为"路径落在该仓目录下"，或直接拒绝非 smart 路径。
2. **PR 合并丢弃目标分支改动** → P0-2。复用 `check_merge_conflict` 已算出的 merge tree。
3. **被拒绝的 push 也发事件/触发 CI** → P0-6。解析 `report-status`，只对成功的 ref 发事件。
4. **写路径统一走 `load_readable_repo` + 属主检查** → P0-4（16 个 handler）。
5. **`search_code` 加可见性过滤** → P0-5（匿名可读私有仓源码）。
6. **`post_fork` 加源仓可见性检查 + fork 继承源仓 `is_private`** → P0-6。
7. **`cancel_run` 校验 owner + run 属于该仓** → P0-1 末行 / P1-7。
8. **`GET /api/events` 加可见性检查** → P1-10。
9. **修 clone URL**（前端 + e2e 断言 + runner 的 `workspace.rs`）→ P0-5 / P1-1。
10. **注册接口的 409 改成统一响应** → P1-11。
11. **`GET /api/auth/tokens` 不再返回 PAT 原文 + PAT 改哈希存储** → P1-12 / P2-10。

**第 1 批（CI 安全与正确性）**

12. runner 加 `.env_clear()` + job 超时 → P0-6。
13. runner workspace 隔离 + 后续引入 bubblewrap/容器 → P0-6。
14. SSH push 补事件（与 HTTP 统一）→ P1-19。
15. `stage_index` 真正参与排序，或从文档里删掉 → P1-3。
16. runner 崩溃的 run 重排队（lease 续期或 reaper）→ P1-2。
17. 解析器语义对齐文档 → P1-4。

**第 2 批（数据正确性）**

18. PR/Issue INSERT 加 `RETURNING` → P0-9。
19. `MAX(number)+1` 改单语句或加锁 → P2-1。
20. `pull_requests`/`issues` 加外键 → P2-14。
21. 删仓库顺序改成先删目录后删行 → P2-14。
22. `groups.name` 加命名空间或加消歧 → P2-4。

**第 3 批（工程基建）**

23. 优雅关闭 → P1-5。
24. 错误信息脱敏 → P1-14。
25. API router 加 JSON 404 fallback，统一错误格式 → P1-13。
26. 鉴权判定抽成共享函数，三层共用 → P1-20。
27. 修测试 teardown 的 schema 泄漏 + 跑一次 `cargo fmt` → P2-13。
28. CI 加 `fmt`/`clippy` 门禁、接 Playwright → P2-16。
29. 修 release tarball 的 Dockerfile/compose 布局 + 把 `gita-runner` 放进镜像 → P2-15。

**第 4 批（产品完整性）**

30. 前端补齐已有的后端能力（SSH keys、密码修改、fork、hooks、群组成员、删仓库、cancel run、头像上传、事件流）。
31. 修 i18n 的 `ERROR_MAP` 和缺失的 `error.notFound` → P2-16。
32. JWT 吊销机制 + logout 接口 → P1-17。
33. 修 OpenAPI 规格的 3 个 path+method 缺失和搜索端点的 auth 标注 → P1-15。
34. 补 `docs/architecture.md` / `docs/api.md`，更新 goal.md 的过期条目。
35. 决定 RBAC 的去留：要么让群组角色真正参与访问控制，要么明确标注为"预留"。

---

## 12. 一句话总结

> **这是 2026-10-22 审查当时的快照。** 下表描述的是**当时**的状态；
> 此后已完成三批修复（见 §10），其中本表列出的 P0 级问题**全部**已修。
> 表格保留是为了说明问题的全貌，不是当前状态。

Gita 的**主体已经建成且质量不低**——密码重置的防枚举和单次消费做得扎实、
Pages 的路径安全有三层防护、CI 的解析器是真严格拒绝的、git 层的路径穿越防护很扎实、
测试有 213 个全绿。

但**它是一串"只做了一半"的组合**，而且**"一半"都落在最危险的地方**：

| 层 | 状态（审查当时） |
|---|---|
| 私有仓保护 | **smart HTTP 正确，dumb HTTP 完全绕过**——私有仓源码可匿名下载 → ✅ 已修 |
| 写路径鉴权 | 读路径对，**写路径 16 个 handler 全绕过**——可合并他人私有仓的 PR → ✅ 已修 |
| 代码搜索 | 仓库搜索对，**代码搜索无过滤**——私有仓源码可匿名搜到 → ✅ 已修 |
| Fork | 无可见性检查，且**强制建成公开仓**——私有仓历史被复制成公开仓 → ✅ 已修 |
| PR 合并 | 是真实的 git merge，但**丢目标分支的改动**（数据丢失）→ ✅ 已修 |
| Push 事件 | **被拒绝的 push 也会发事件并触发 CI** → ✅ 已修 |
| SSH | 鉴权正确，但**不产生任何事件、不触发 CI** → ❌ 仍待修（事件部分） |
| Clone URL | UI 和 runner 给的 URL **都少 `/git`**，必然失败——而 e2e 把 bug 固化了 → ✅ 已修 |
| CI runner | **无沙箱、无超时、继承 runner 全部环境变量**（含 DB 密码）→ 部分修复（环境隔离/超时/目录隔离），**无沙箱** |
| RBAC | 群组角色存了但**完全不参与访问决策** → ✅ 已修（第三批，GitLab 式角色） |
| 事件流 | 生产**零订阅者**；CI 生命周期在事件流里**完全不可见** → ❌ 仍待修 |
| AI 能力 | 已移除，只剩一个静态工具清单端点 |

**最该做的一件事**：把"能不能访问这个仓"这一个问题**收敛成一个函数**，
让 smart HTTP、dumb HTTP、SSH、REST API 的读路径和写路径**全部走它**。
→ **第三批已完成**（见 §10）：规则在 `gita_core::access`，五个等级
（Read / Write / Push / Admin），群组角色参与判定。

**第二个该做的一件事**：修 clone URL。UI 给用户的那条 `git clone` 命令原来是**必然失败**的，
而 17 个 e2e 全绿——这说明当时的测试防线在关键处是**假阳性**。
→ **已修**，并补了"真的跑 `git clone`"的 e2e。新增测试如果沿用
"断言 UI 显示什么"而非"断言功能可用"的写法，会继续固化 bug。
