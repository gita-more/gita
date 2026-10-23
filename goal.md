# Gita —— AI 原生 Git Server 详细规划

> 一个用 Rust 编写的、AI 原生的自托管 Git 服务器。在提供标准 Git 服务（clone/push/pull）的同时，把 AI 评审、Agent 自动化、语义搜索、MCP 接入作为一等公民内嵌到开发工作流中。

---

## 1. 产品定位与核心理念

**一句话定位**：像 Gitea 一样好用的自托管 Git 服务，但 Git 只是底座 —— 真正的产品是"AI 原生开发工作流"。

**AI 原生的含义**：
- 不是"在 Git 服务器上外挂一个 AI 插件"，而是从数据模型、事件流、API 设计上就让 AI 深度参与代码开发全流程。
- 每个 Git 事件（push、PR、评论）都是一个可被 AI 观察、响应、驱动的事件。
- AI 不是被动问答，而是主动的协作者：自动评审、自动修复、自动完成 issue→分支→提交→PR 的闭环。

**产品价值主张**：
- 对开发者：push 即有 AI 评审，语义搜索代替 grep，AI 可直接改代码。
- 对团队：AI Agent 作为"虚拟成员"参与 PR 全流程，减少人工评审负担。
- 对 AI 应用生态：通过 MCP Server 暴露标准接口，任何 MCP 客户端（Claude Desktop、Cline 等）都能操作仓库。

**竞品参照**：
- Gitea / 主流 self-hosted Git 服务：功能全面，但 AI 是外挂。
- Sourcegraph：优秀的代码语义搜索，但不是 Git 服务器。
- Greptile / CodeRabbit：AI 评审 SaaS，但依赖外部托管平台。
- **Gita 的差异化**：自托管 + 开源 + AI 原生数据模型 + MCP 一等公民。

---

## 2. 技术底座选型（已核实）

| 关注点 | 选型 | 理由 |
|---|---|---|
| Git 库 | **gitoxide（gix）** | 纯 Rust、高性能、无 C 依赖；`gix` 0.89 / `gix-protocol` 0.67 持续活跃更新 |
| HTTP 框架 | **axum** | tokio 生态、路由清晰、中间件丰富、与 tower 完美配合 |
| Web 前端 | **TUI/纯前端**：React + TypeScript（Vite）+ 轻量 UI 库，或先做服务端渲染的轻量页面 | 先 API 优先，UI 可迭代 |
| 数据库 | **PostgreSQL + SQLx** | 关系数据（用户/仓库/PR/Issue）+ `pgvector` 扩展做向量检索，一站式 |
| 向量检索 | **pgvector**（PostgreSQL 扩展） | 避免引入独立向量库，运维简单；`pgvector` crate 0.4 成熟 |
| 对象存储 | 本地文件系统（裸仓库目录） | 自托管第一版优先简单；后续可抽象 S3 |
| MCP Server | **mcp-server**（官方 Rust SDK） | crates.io `mcp-server` 0.1.0 是官方 SDK，支持 stdio/streamable HTTP |
| Embedding 模型 | 通过 REST 调用外部模型（OpenAI / 本地 Ollama） | 不绑定具体供应商，抽象 `EmbeddingProvider` trait |
| LLM 调用 | 异步 HTTP 调用，抽象 `LlmProvider` trait | 支持 OpenAI 兼容 API / Anthropic / Ollama 本地模型 |
| Agent 执行沙箱 | **`bubblewrap` + 独立用户 + 只读挂载**（或容器） | 提供隔离的代码执行环境，避免依赖重型容器 |
| 任务队列 | **`agora`** 或 Redis + tokio 后台任务 | 处理异步事件（push 触发评审、embedding 索引） |
| 鉴权 | JWT（API）+ 个人访问令牌（PAT） | 轻量、标准 |

---

## 3. 系统架构

```
┌─────────────────────────────────────────────────────────────┐
│                        客户端层                              │
│  git CLI (HTTP) │ 浏览器 Web UI │ MCP 客户端 │ REST API 调用者│
└──────────┬──────────────┬──────────────┬───────────┬────────┘
           │              │              │           │
           ▼              ▼              ▼           ▼
┌─────────────────────────────────────────────────────────────┐
│                       网关层 (axum)                         │
│   /git/*  Smart HTTP   │   /api/*  REST API  │  /mcp  MCP   │
│   (upload-pack / receive-pack)                (streamable)  │
└──────────┬──────────────────────────────┬───────────────────┘
           │                              │
           ▼                              ▼
┌──────────────────────┐   ┌─────────────────────────────────┐
│      Git 服务层       │   │          应用服务层              │
│  gix-repository 读写   │   │  仓库/PR/Issue/用户/权限         │
│  裸仓库存储 / 对象库    │   │  事件发布 (Event Bus)           │
│  引用管理 / 打包       │   │                                 │
└──────────┬───────────┘   └──────────┬──────────────────────┘
           │                          │
           ▼                          ▼
┌─────────────────────────────────────────────────────────────┐
│                        数据层                                │
│  PostgreSQL (+pgvector)  │  裸仓库文件系统  │  对象缓存/文件   │
└─────────────────────────────────────────────────────────────┘
           │
           ▼
┌─────────────────────────────────────────────────────────────┐
│                      AI 服务层 (独立模块)                    │
│  Embedding 索引器 │ 代码评审引擎 │ Agent 执行器 │ MCP 工具    │
│  (异步任务队列驱动，可水平扩展)                               │
└─────────────────────────────────────────────────────────────┘
```

### 模块划分（Cargo workspace）

```
gita/
├── crates/
│   ├── gita-core/          # 领域模型：仓库、PR、Issue、事件、用户
│   ├── gita-git/           # gitoxide 封装：裸仓库管理、协议服务端
│   ├── gita-db/            # SQLx 数据访问 + 迁移（含 pgvector schema）
│   ├── gita-api/           # REST API 路由 + 鉴权 + DTO
│   ├── gita-http/          # Smart HTTP Git 协议端点（/git/*）
│   ├── gita-events/        # 事件总线（发布/订阅，驱动 AI 任务）
│   ├── gita-web/           # 静态资源 + Web UI 服务
│   └── gita-server/        # 入口：axum app 组装、配置、启动
└── web/                    # React + TS 前端（Vite）
```

---

## 4. 功能规划（分版本）

### MVP（v0.1）—— 核心 Git + 基础 AI 评审
- [x] 仓库的创建/删除/列表、裸仓库管理
- [x] Smart HTTP 协议：clone / push / pull（`git-http-backend` 等价实现）
- [x] 用户注册/登录、JWT 认证、PAT
- [x] 基本 Web UI 仓库浏览、提交历史、文件树
- [x] REST API 基础版（仓库、提交、PR、Issue）

> 注：AI 评审与语义代码搜索功能已于 v0.0.1 后期移除，后续 AI 能力将以新的方式重新引入。

### v0.2 —— 语义搜索 + PR 工作流
- [x] PR（Pull Request）模型：分支对比、合并、冲突检测（创建/合并/关闭、**冲突检测、真实 fast-forward 与双父 merge commit** 均已实现）
- [ ] **语义代码搜索**：代码分块 → embedding → pgvector 检索
- [ ] 自然语言搜索 UI（"哪里处理了 JWT 过期？"）
- [x] Issue 系统（基础版）

### v0.3 —— Agent 自动化 + MCP
- [x] **MCP Server**：stdio 传输已实现（streamable HTTP 未做），暴露 `list_repositories` / `read_file` / `list_commits` / `create_pull_request` / `get_pull_request` / `create_issue` / `list_events` 等
- [ ] **Agent 沙箱执行**：bubblewrap 隔离、安全文件写入、git commit 能力
- [ ] **自动化工作流**：issue → agent 建分支 → 实现 → 提交 → 开 PR
- [ ] 工作流事件通知（webhook / 站内信）

### v0.4+ —— 多租户与扩展
- [ ] 团队/组织/角色权限（RBAC）
- [ ] 对象存储抽象（S3 兼容）
- [x] **In-repo CI**（Phase 1，§10.3 已完成）
- [ ] 高可用：任务队列独立化、AI 服务水平扩展

---

## 5. 详细技术设计

### 5.1 Git 协议服务（crates/gita-git）

- 基于 `gix-protocol`（0.67）实现 **smart HTTP** 的 `git-upload-pack` 和 `git-receive-pack` 语义。
- 认证：HTTP Basic（PAT）+ 可选 Bearer；匿名只读（可配置）。
- 关键流程：
  - `GET /git/<repo>/info/refs?service=git-upload-pack` → 返回 refs。
  - `POST /git/<repo>/git-upload-pack` → 处理 fetch 请求（gix 完成对象打包）。
  - `POST /git/<repo>/git-receive-pack` → 处理 push，校验权限，更新裸仓库引用。
  - 每个 push 完成后发布 `PushEvent`（含新 commit 列表、diff 统计）到事件总线。
- 仓库存储：`data/repos/<owner>/<name>.git`（裸仓库）。
- 用 `gix` 读取对象、遍历提交、计算 diff、生成 patch。

### 5.2 数据模型（PostgreSQL + pgvector）

核心表（示意）：
```
users(id, username, email, password_hash, created_at)
repositories(id, owner_id, name, description, is_private, default_branch, created_at)
branches(id, repo_id, name, head_commit)
commits(id, repo_id, sha, author_id, message, parent_shas, authored_at)
pull_requests(id, repo_id, number, title, body, source_branch, target_branch, status, author_id, base_sha, head_sha, created_at, merged_at)
issues(id, repo_id, number, title, body, status, author_id, created_at)
events(id, type, payload(jsonb), repo_id, actor_id, created_at)   -- 事件日志（AI 可读）
code_embeddings(id, repo_id, blob_sha, file_path, chunk_index, content, embedding vector(1536), updated_at)   -- 预留
```

**事件表是 AI 原生的核心**：所有 Git 活动都写成结构化事件，AI Agent 订阅事件流即可感知仓库动态。

> 注：`reviews` / `review_comments` 表已随 AI 评审功能一并移除，后续 AI 能力引入时按新设计重新建表。

### 5.3 AI 评审引擎（后续规划）

> AI 评审引擎（原 crates/gita-ai）已移除。后续 AI 能力将以其他方式引入，届时再补充设计。

- 触发：`PushEvent` / PR 创建 / PR 更新。
- 流程：
  1. 计算 diff（gix）。
  2. 按文件/函数切块（利用 git blame 定位变更函数）。
  3. 调用 LLM（`LlmProvider` trait）：输出结构化 JSON（severity、file_path、line、message、suggestion）。
  4. 落库为 `reviews` + `review_comments`。
  5. 计算风险分（0-100），可配置阈值自动合并/打回。
- `LlmProvider` 抽象：OpenAI 兼容 API / Anthropic / Ollama 本地，通过配置切换。
- 评审粒度可配置：按 push、按 PR、按文件。

### 5.4 语义代码搜索（后续规划）

- 索引策略：push 后异步对变更 blob 分块（~200 tokens/块，保留函数边界）→ embedding → upsert 到 `code_embeddings`。
- 查询：自然语言 → embedding → pgvector `<=>` 余弦距离检索 → 返回文件/行/代码片段。
- 支持过滤（仓库、语言、路径）。
- 模型默认 1536 维（OpenAI text-embedding-3-small）或本地 Ollama，维度在 schema 中可配置。

### 5.5 MCP Server（后续规划）

- 基于官方 `mcp-server` crate（0.1.0）。
- 传输：**stdio**（本地开发）+ **streamable HTTP**（远程）。
- 工具清单：
  | 工具 | 说明 |
  |---|---|
  | `list_repositories` | 列出仓库 |
  | `read_file` | 读取文件/文件树 |
  | `list_commits` | 提交历史 |
  | `create_pr` / `get_pr` | PR 工作流 |
  | `create_issue` | Issue 操作 |
  | `list_events` | AI 可读事件流 |
  | `apply_patch` | 通过补丁修改代码（配合沙箱，后续） |
- 鉴权：MCP 连接时携带 PAT。

> 注：`search_code` / `get_review` 工具已随 AI 功能移除。当前 MCP crate 已从 workspace 移除，后续随 AI 能力一并重新引入。

### 5.6 Agent 自动化工作流（后续规划）

- **执行沙箱**：`bubblewrap`（bwrap）+ 专用低权限用户 + 只读挂载仓库 + 可写临时工作区。
- 工作流引擎（基于事件 + 队列）：
  - 用户创建 Issue（或直接触发）。
  - Agent 接收任务 → 读取仓库上下文 → 语义搜索相关代码 → 生成实现方案。
  - Agent 在沙箱中修改文件、运行测试（可选）、`git commit`、创建分支、推送。
  - Agent 自动创建 PR，并附加实现说明。
  - 触发 AI 评审 → 如通过自动合并（可配置）。
- 安全边界：
  - 沙箱无网络（默认）/ 白名单网络（可配置）。
  - 严格控制文件系统访问范围。
  - 所有 Agent 行为记录为事件，可审计、可回滚。

### 5.7 REST API（crates/gita-api）

完整端点清单见 [`docs/api.md`](docs/api.md)（由 OpenAPI 文档生成），运行中的服务器
在 `/api/docs` 提供 Swagger UI、`/api/docs/openapi.json` 提供规格本身。下面只是
主干端点的摘录：

```
GET    /api/repos
POST   /api/repos
GET    /api/repos/{owner}/{repo}
PATCH  /api/repos/{owner}/{repo}          (改描述/可见性/改名/转移)
GET    /api/repos/{owner}/{repo}/tree?path=&ref=
GET    /api/repos/{owner}/{repo}/commits
GET    /api/repos/{owner}/{repo}/commits/{sha}
GET    /api/repos/{owner}/{repo}/branches
POST   /api/repos/{owner}/{repo}/branches
POST   /api/repos/{owner}/{repo}/prs
GET    /api/repos/{owner}/{repo}/prs/{number}
POST   /api/repos/{owner}/{repo}/prs/{number}/merge
POST   /api/repos/{owner}/{repo}/issues
GET    /api/events?repo=&type=            (AI 可读事件流)
GET    /api/search?type=&q=               (仓库 + 代码搜索)
```

统一响应格式、OpenAPI 文档（手写规格 + 与实际路由的漂移测试）、分页、错误码。

> 注：`POST /api/agents/run`（触发 agent 任务）**尚未实现**，Agent 自动化重新引入时再加。

### 5.8 Web UI（web/）

- React + TypeScript + Vite。
- 页面：登录/注册、仓库列表、仓库详情（文件树、提交、分支）、PR 列表/详情、Issue、Agent 运行记录（后续）。
- 与 API 通过 REST 通信；可后续加 WebSocket 推送事件。

---

## 6. 目录结构

```
gita/
├── Cargo.toml                # workspace
├── crates/
│   ├── gita-core/            # 领域模型 + 访问控制规则
│   ├── gita-git/             # 裸仓库管理（gix + git CLI）
│   ├── gita-db/              # SQLx 数据访问 + migrations/ 存放 SQL
│   ├── gita-api/             # REST API 路由 + 鉴权 + DTO + OpenAPI 规格
│   ├── gita-http/            # Smart HTTP Git 协议端点（/git/*）
│   ├── gita-ssh/             # 内嵌 SSH 服务器（Git over SSH）
│   ├── gita-events/          # 事件总线（发布/订阅，驱动 AI 任务）
│   ├── gita-pipeline/        # .gita-ci.yml 解析 + stages DAG 展开
│   ├── gita-push/            # push 之后的统一处理：发事件 + 入队 CI
│   ├── gita-runner/          # CI runner（独立二进制）
│   ├── gita-web/             # 静态资源 + Web UI 服务
│   └── gita-server/          # 入口：axum app 组装、配置、启动
├── web/
│   ├── src/
│   ├── package.json
│   └── vite.config.ts
├── integration/api/tests/    # REST API 集成测试
├── data/                     # 运行时数据（.gitignore）
│   └── repos/
├── config/
│   └── config.example.toml   # 配置示例
├── docs/
│   ├── design-review-2026-10-22.md   # 实现设计文档（架构 + 问题清单 + 修复进度）
│   ├── api.md                        # REST 端点参考（由 OpenAPI 规格生成）
│   ├── gita-ci-writing-guide.md      # .gita-ci.yml 编写指南
│   └── runner-design-notes.md        # CI runner 设计笔记
├── scripts/
└── goal.md
```

---

## 7. 开发路线图（里程碑 + 任务分解）

### Phase 0：脚手架（0.5-1 周）
- [x] Cargo workspace 初始化，各 crate 骨架
- [x] axum 服务器启动 + 配置加载（toml）
- [x] PostgreSQL 本地 docker-compose（含 pgvector 镜像 `pgvector/pgvector:pg16`）
- [x] SQLx migrate 初始化用户/仓库表（以启动时幂等建表 `init_schema` 实现）
- [x] 基础日志（tracing）

### Phase 1：Git 协议服务（2-3 周）
- [x] 裸仓库创建/删除（gix）
- [x] Smart HTTP `info/refs`（upload-pack / receive-pack）（通过 `git http-backend` 委托实现）
- [x] push/pull 权限校验
- [x] 事件发布：`PushEvent`
- [x] 集成测试：真实 `git clone/push/pull` 走通（在 gita-git 单测中验证 push 与浏览；无独立 `tests/` 目录）

### Phase 2：基础 API + Web UI（2-3 周）
- [x] REST API：仓库、提交、文件树
- [x] 用户注册/登录、JWT、PAT
- [x] Web UI 基础页面
- [x] 端到端：网页创建仓库 → git push → 网页看到提交

### Phase 3：AI 评审（已移除，后续重新规划）
> AI 评审功能已于 v0.0.1 后期整体移除（前端 + 后端 + crates/gita-ai）。后续 AI 能力将以其他方式引入，此阶段届时重新规划。

### Phase 4：语义搜索（后续规划）
- [ ] 代码分块 + 异步 embedding 索引
- [ ] pgvector 检索
- [ ] 搜索 API + UI
- [ ] 增量索引（push 触发）

### Phase 5：PR 工作流（2-3 周）
- [x] PR 模型、创建/合并/**冲突检测**（创建/合并/关闭、冲突检测、fast-forward 与双父 merge commit 均已实现）
- [x] PR UI

### Phase 6：MCP + Agent（后续规划）
- [ ] MCP Server（stdio + HTTP）（当前 MCP crate 已移除，后续随 AI 能力重新引入）
- [ ] 沙箱执行环境（bubblewrap）
- [ ] Agent 工作流：issue→PR 闭环
- [ ] 审计事件

### Phase 7：打磨与扩展（持续）
- [ ] 多租户/RBAC
- [ ] Webhook 出站
- [ ] 对象存储抽象
- [ ] 性能优化（并发 push、打包缓存）

---

## 8. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| gitoxide 服务端 API 尚未完全稳定 | 协议层工作量可能偏大 | 用 `gix-protocol` 官方例程起步；协议层独立成 crate，便于替换 |
| LLM 成本不可控 | AI 调用费用高 | 后续引入 AI 时采用成本限额、只评审 diff、模型可配置、本地 Ollama 选项 |
| Agent 沙箱执行有安全风险 | 恶意代码可能逃逸 | 默认无网络 + 低权限用户 + 只读仓库 + 超时 + 审计；后续支持容器隔离 |
| embedding 维度变更 | 索引需重建 | 维度参数化，迁移脚本预留 |
| 前端工作量分散精力 | UI 拖慢核心 | MVP 先做 API 优先，UI 保持极简 |

---

## 9. 非目标（明确不做）

- 不做原生 SSH Git 协议（第一版仅 HTTP；后续可用 `gix` 或集成 ssh 服务扩展）。
- **不做外部 CI 集成 / 出站 webhook**。CI 改由内置的 In-repo CI（见 §10）承担。
- 不做多租户 SaaS 计费（v0.4 后才考虑）。
- 不重造 git 对象格式（信任 gitoxide）。

---

## 10. In-repo CI（按阶段推进）

gita 自带一个轻量级 in-repo CI，借鉴主流 SaaS CI 平台的形态，但只做仓库内 CI（不替代大型 SaaS 流水线）。

### 10.1 设计要点

- **DSL：`.gita-ci.yml`**，贴近主流 CI YAML 风格。Phase 1 支持 `stages` / `jobs` / `stage` /
  `script` / `needs`（仅数组形式）/ `only` / `except`（顶层，作用于整条流水线）。
  其他字段（`image`、`artifacts`、`cache`、`when`、顶层 `services` 等）会被严格拒绝并
  在 UI 上报错；**顶层的未知 key 也会按名字报错**，不会再被误当成一个 job。
- **Pipeline 定义 in-repo**，由仓库自己管理；没有 server-side 管理面板。
- **Runner 是独立二进制** `gita-runner`，与 `gita-server` 解耦，跨进程通过 PostgreSQL 共享状态（`pipeline_runs` 表 + `FOR UPDATE SKIP LOCKED` 抢单），独立 worker 模型。
- **Phase 1 runtime 仅 shell**（`sh -c "<joined script>"`）；Docker / Podman 推迟到 Phase 3。
- 触发器：push（Phase 1）；PR 状态检查、定时触发推迟到 Phase 2。
  （SSE 日志流与 cancel 已经提前做掉了，见下。）

### 10.2 三阶段路线

| Phase | 内容 | 收尾标准 |
|-------|------|----------|
| **1（本阶段）** | push 触发 → `.gita-ci.yml` 解析 → shell 跑 job → 写 logs → UI 上看 run + 日志 | 推一个有 `.gita-ci.yml` 的仓，能在 SPA Pipelines 标签看到 run 列表和 job stdout |
| **2** | artifacts + cache + re-run + PR status check（**stages DAG、`only/except`、SSE 日志流、cancel 已提前完成**） | PR 合并前能等 CI pass |
| **3** | Docker 运行时（`image:`/`services:`）+ secrets 加密存储 + worker 注册 API + pipeline YAML Monaco 编辑器 | 类似主流 SaaS CI 基本可用 |

### 10.3 已完成（Phase 1）

- DB migration `20261020000000_pipelines.sql`：`pipelines` / `pipeline_runs` / `jobs` / `logs` 四张表 + 抢单用的 partial index。
- `crates/gita-pipeline/` —— `.gita-ci.yml` 解析 + stages DAG 展开。
- `crates/gita-runner/` —— 独立二进制。`FOR UPDATE SKIP LOCKED` 抢单、`git clone --depth 1`、`sh -c` 执行、stdout/stderr 行级写入 `logs`。
- `gita-server worker register --name <n> --user <u>` CLI 子命令，铸 PAT 给 runner 用作 clone 时的 Basic-auth 密码。（二进制名是 `gita-server`，不是 `gita`。）
- `crates/gita-http/`：push 事件 payload 升级到 `{ref, old_sha, new_sha, pusher_id}`（顺手修了 `actor_id: None` 那个旧 bug）；enqueue 路径检测新 commit 上的 `.gita-ci.yml`、upsert pipeline、insert run、发 `PipelineRequested` 事件。SSH push 走同一条路径（`crates/gita-push/`）。
- `crates/gita-api/`：**7 个** REST 端点（list pipelines / list runs / create run / get run / cancel / logs / logs stream）。
- SPA `web/`：Tab 多了 `pipelines`，Sidebar 加 RocketIcon 入口，`<PipelinesPanel>` 列出 run、点击展开 job 日志（**SSE 实时流**，不是轮询）。

### 10.4 取消的旧条目

> 原 §9 那行 "不做 CI/CD 平台（Webhook 出站对接外部 CI）" 已替换为现在的 §10。出站 webhook 与 in-repo CI 是两件事，本节只否决前者。

---

## 11. 验收标准（Definition of Done）

**MVP 完成标准**：
- [x] `git clone https://localhost/git/owner/repo` 可克隆
- [x] `git push` 可推送
- [x] Web UI 可查看仓库文件、提交
- [x] REST API 文档完整（OpenAPI）
- [x] 全部用 `cargo test` 通过的集成测试

**v0.3 完成标准**：
- [ ] 语义搜索可回答自然语言代码问题
- [ ] MCP 客户端（如 Claude）可通过工具创建 PR（MCP crate 已移除，后续重新引入）
- [ ] Agent 可从 Issue 自动产出 PR

---

*本文档是项目愿景与技术路线的权威参考。实现时以本文件为准，重大变更需同步更新本文件。*