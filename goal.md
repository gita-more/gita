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
- Gitea / GitLab：功能全面，但 AI 是外挂。
- Sourcegraph：优秀的代码语义搜索，但不是 Git 服务器。
- Greptile / CodeRabbit：AI 评审 SaaS，但依赖 GitHub 等第三方托管。
- **Gita 的差异化**：自托管 + 开源 + AI 原生数据模型 + MCP 一等公民。

---

## 2. 技术底座选型（已核实）

| 关注点 | 选型 | 理由 |
|---|---|---|
| Git 库 | **gitoxide（gix）** | 纯 Rust、高性能、无 C 依赖；`gix` 0.89 / `gix-protocol` 0.67 持续活跃更新 |
| HTTP 框架 | **axum** | tokio 生态、路由清晰、中间件丰富、与 tower 完美配合 |
| Web 前端 | **TUI/纯前端**：React + TypeScript（Vite）+ 轻量 UI 库，或先做服务端渲染的轻量页面 | 先 API 优先，UI 可迭代 |
| 数据库 | **PostgreSQL + SQLx** | 关系数据（用户/仓库/PR/评审）+ `pgvector` 扩展做向量检索，一站式 |
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
│  gix-repository 读写   │   │  仓库/PR/Issue/评审/用户/权限     │
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
│   ├── gita-core/          # 领域模型：仓库、PR、Issue、评审、事件、用户
│   ├── gita-git/           # gitoxide 封装：裸仓库管理、协议服务端
│   ├── gita-db/            # SQLx 数据访问 + 迁移（含 pgvector schema）
│   ├── gita-api/           # REST API 路由 + 鉴权 + DTO
│   ├── gita-http/          # Smart HTTP Git 协议端点（/git/*）
│   ├── gita-events/        # 事件总线（发布/订阅，驱动 AI 任务）
│   ├── gita-ai/            # AI 编排：评审、embedding、agent 调度
│   ├── gita-mcp/           # MCP Server（stdio + streamable HTTP）
│   ├── gita-web/           # 静态资源 + Web UI 服务
│   └── gita-server/        # 入口：axum app 组装、配置、启动
└── web/                    # React + TS 前端（Vite）
```

---

## 4. 功能规划（分版本）

### MVP（v0.1）—— 核心 Git + 基础 AI 评审
- [ ] 仓库的创建/删除/列表、裸仓库管理
- [ ] Smart HTTP 协议：clone / push / pull（`git-http-backend` 等价实现）
- [ ] 用户注册/登录、JWT 认证、PAT
- [ ] 基本 Web UI：仓库浏览、提交历史、文件树
- [ ] **基础 AI 代码评审**：push 触发，LLM 分析 diff，产出行级评论 + 风险分
- [ ] 评审结果展示在 Web UI
- [ ] REST API 基础版（仓库、提交、评审）

### v0.2 —— 语义搜索 + PR 工作流
- [ ] PR（Pull Request）模型：分支对比、合并、冲突检测
- [ ] **语义代码搜索**：代码分块 → embedding → pgvector 检索
- [ ] 自然语言搜索 UI（"哪里处理了 JWT 过期？"）
- [ ] Issue 系统（基础版）
- [ ] 评审结果可 comment / 可修改

### v0.3 —— Agent 自动化 + MCP
- [ ] **MCP Server**：以工具暴露 `read_repo` / `search_code` / `create_pr` / `create_issue` / `apply_patch` / `get_pr_review` 等
- [ ] **Agent 沙箱执行**：bubblewrap 隔离、安全文件写入、git commit 能力
- [ ] **自动化工作流**：issue → agent 建分支 → 实现 → 提交 → 开 PR
- [ ] 工作流事件通知（webhook / 站内信）

### v0.4+ —— 多租户与扩展
- [ ] 团队/组织/角色权限（RBAC）
- [ ] 对象存储抽象（S3 兼容）
- [ ] Webhook 出站（对接 CI/CD）
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
reviews(id, pr_id, reviewer_agent_id, status, summary, risk_score, created_at)
review_comments(id, review_id, file_path, line, severity, message, created_at)
issues(id, repo_id, number, title, body, status, author_id, created_at)
events(id, type, payload(jsonb), repo_id, actor_id, created_at)   -- 事件日志（AI 可读）
code_embeddings(id, repo_id, blob_sha, file_path, chunk_index, content, embedding vector(1536), updated_at)
```

**事件表是 AI 原生的核心**：所有 Git 活动都写成结构化事件，AI Agent 订阅事件流即可感知仓库动态。

### 5.3 AI 评审引擎（crates/gita-ai）

- 触发：`PushEvent` / PR 创建 / PR 更新。
- 流程：
  1. 计算 diff（gix）。
  2. 按文件/函数切块（利用 git blame 定位变更函数）。
  3. 调用 LLM（`LlmProvider` trait）：输出结构化 JSON（severity、file_path、line、message、suggestion）。
  4. 落库为 `reviews` + `review_comments`。
  5. 计算风险分（0-100），可配置阈值自动合并/打回。
- `LlmProvider` 抽象：OpenAI 兼容 API / Anthropic / Ollama 本地，通过配置切换。
- 评审粒度可配置：按 push、按 PR、按文件。

### 5.4 语义代码搜索

- 索引策略：push 后异步对变更 blob 分块（~200 tokens/块，保留函数边界）→ embedding → upsert 到 `code_embeddings`。
- 查询：自然语言 → embedding → pgvector `<=>` 余弦距离检索 → 返回文件/行/代码片段。
- 支持过滤（仓库、语言、路径）。
- 模型默认 1536 维（OpenAI text-embedding-3-small）或本地 Ollama，维度在 schema 中可配置。

### 5.5 MCP Server（crates/gita-mcp）

- 基于官方 `mcp-server` crate（0.1.0）。
- 传输：**stdio**（本地开发）+ **streamable HTTP**（远程）。
- 工具清单：
  | 工具 | 说明 |
  |---|---|
  | `read_repo` | 列出仓库/读取文件树/读文件 |
  | `search_code` | 语义代码搜索 |
  | `get_commit` / `list_commits` | 提交历史 |
  | `create_branch` / `list_branches` | 分支操作 |
  | `create_pr` / `get_pr` / `merge_pr` | PR 工作流 |
  | `create_issue` / `update_issue` | Issue 操作 |
  | `get_pr_review` | 获取 AI 评审 |
  | `apply_patch` | 通过补丁修改代码（配合沙箱） |
- 鉴权：MCP 连接时携带 PAT。

### 5.6 Agent 自动化工作流（crates/gita-ai）

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

```
GET    /api/repos
POST   /api/repos
GET    /api/repos/{owner}/{repo}
GET    /api/repos/{owner}/{repo}/tree?path=&ref=
GET    /api/repos/{owner}/{repo}/commits
GET    /api/repos/{owner}/{repo}/commits/{sha}
POST   /api/repos/{owner}/{repo}/prs
GET    /api/repos/{owner}/{repo}/prs/{number}
GET    /api/repos/{owner}/{repo}/prs/{number}/review
POST   /api/repos/{owner}/{repo}/prs/{number}/review   (手动触发评审)
GET    /api/repos/{owner}/{repo}/search?q=自然语言
POST   /api/repos/{owner}/{repo}/issues
GET    /api/events?repo=&type=        (AI 可读事件流)
POST   /api/agents/run                 (触发 agent 任务)
```

统一响应格式、OpenAPI 文档（utoipa）、分页、错误码。

### 5.8 Web UI（web/）

- React + TypeScript + Vite。
- 页面：登录/注册、仓库列表、仓库详情（文件树、提交、分支）、PR 列表/详情、Issue、AI 评审视图（行级评论 + 风险分）、语义搜索页、Agent 运行记录。
- 与 API 通过 REST 通信；可后续加 WebSocket 推送评审完成事件。

---

## 6. 目录结构

```
gita/
├── Cargo.toml                # workspace
├── crates/
│   ├── gita-core/
│   ├── gita-git/
│   ├── gita-db/              # migrations/ 存放 SQL
│   ├── gita-api/
│   ├── gita-http/
│   ├── gita-events/
│   ├── gita-ai/
│   ├── gita-mcp/
│   ├── gita-web/
│   └── gita-server/
├── web/
│   ├── src/
│   ├── package.json
│   └── vite.config.ts
├── data/                     # 运行时数据（.gitignore）
│   └── repos/
├── config/
│   └── config.example.toml   # 配置示例
├── docs/
│   ├── architecture.md
│   ├── api.md
│   └── mcp.md
├── tests/                    # 集成测试
└── goal.md
```

---

## 7. 开发路线图（里程碑 + 任务分解）

### Phase 0：脚手架（0.5-1 周）
- [ ] Cargo workspace 初始化，各 crate 骨架
- [ ] axum 服务器启动 + 配置加载（toml）
- [ ] PostgreSQL 本地 docker-compose（含 pgvector 镜像 `pgvector/pgvector:pg16`）
- [ ] SQLx migrate 初始化用户/仓库表
- [ ] 基础日志（tracing）

### Phase 1：Git 协议服务（2-3 周）
- [ ] 裸仓库创建/删除（gix）
- [ ] Smart HTTP `info/refs`（upload-pack / receive-pack）
- [ ] push/pull 权限校验
- [ ] 事件发布：`PushEvent`
- [ ] 集成测试：真实 `git clone/push/pull` 走通

### Phase 2：基础 API + Web UI（2-3 周）
- [ ] REST API：仓库、提交、文件树
- [ ] 用户注册/登录、JWT、PAT
- [ ] Web UI 基础页面
- [ ] 端到端：网页创建仓库 → git push → 网页看到提交

### Phase 3：AI 评审（2-3 周）
- [ ] diff 计算 + 变更函数定位
- [ ] `LlmProvider` 抽象 + OpenAI 兼容实现
- [ ] 评审引擎：结构化输出、行级评论落库
- [ ] 评审展示 UI
- [ ] 事件驱动：push 自动触发评审

### Phase 4：语义搜索（2 周）
- [ ] 代码分块 + 异步 embedding 索引
- [ ] pgvector 检索
- [ ] 搜索 API + UI
- [ ] 增量索引（push 触发）

### Phase 5：PR 工作流（2-3 周）
- [ ] PR 模型、创建/合并/冲突检测
- [ ] PR 评审集成
- [ ] PR UI

### Phase 6：MCP + Agent（3-4 周）
- [ ] MCP Server（stdio + HTTP）
- [ ] MCP 工具实现
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
| LLM 成本不可控 | AI 调用费用高 | 成本限额、只评审 diff、模型可配置、本地 Ollama 选项 |
| Agent 沙箱执行有安全风险 | 恶意代码可能逃逸 | 默认无网络 + 低权限用户 + 只读仓库 + 超时 + 审计；后续支持容器隔离 |
| embedding 维度变更 | 索引需重建 | 维度参数化，迁移脚本预留 |
| 前端工作量分散精力 | UI 拖慢核心 | MVP 先做 API 优先，UI 保持极简 |

---

## 9. 非目标（明确不做）

- 不做原生 SSH Git 协议（第一版仅 HTTP；后续可用 `gix` 或集成 ssh 服务扩展）。
- 不做 CI/CD 平台（Webhook 出站对接外部 CI）。
- 不做多租户 SaaS 计费（v0.4 后才考虑）。
- 不重造 git 对象格式（信任 gitoxide）。

---

## 10. 验收标准（Definition of Done）

**MVP 完成标准**：
- [ ] `git clone https://localhost/git/owner/repo` 可克隆
- [ ] `git push` 可推送，且触发 AI 评审
- [ ] Web UI 可查看仓库文件、提交、评审结果
- [ ] REST API 文档完整（OpenAPI）
- [ ] 全部用 `cargo test` 通过的集成测试

**v0.3 完成标准**：
- [ ] 语义搜索可回答自然语言代码问题
- [ ] MCP 客户端（如 Claude）可通过工具创建 PR
- [ ] Agent 可从 Issue 自动产出 PR

---

*本文档是项目愿景与技术路线的权威参考。实现时以本文件为准，重大变更需同步更新本文件。*