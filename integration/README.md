# Gita Integration Tests

Gita 的集成测试（E2E）统一放在本目录，按子系统分为 `web` 与 `cli`。

```
integration/
├── README.md
├── web/                    # Web E2E（Playwright）
│   ├── package.json        #   独立 npm 包（仅依赖 @playwright/test）
│   ├── playwright.config.ts
│   └── tests/layout.spec.ts # 侧边栏布局测试
└── cli/                    # CLI / 服务端集成测试（Rust）
    ├── Cargo.toml          #   workspace 成员：gita-integration-cli
    └── tests/cli.rs        #   二进制 CLI 行为 + 应用初始化
```

## 测试层级

| 层级 | 位置 | 工具 | 覆盖范围 |
|---|---|---|---|
| 单元测试（UT） | 各 crate 内 `#[cfg(test)]` 模块 | cargo test | 函数级逻辑（哈希、JWT、解析等） |
| CLI 集成测试 | `integration/cli/tests/` | assert_cmd | `gita-server` 二进制 CLI、配置加载、应用装配 |
| Web E2E | `integration/web/tests/` | Playwright | 浏览器端真实 UI 行为（布局、导航、折叠） |

> 单元测试内嵌在各 crate 中，随 `cargo test` 一起运行；集成测试统一收口到 `integration/`。

## 运行方式

### Web E2E（需先启动 Gita 服务）

```bash
# 启动服务（若未运行）
cd <repo-root> && ./deploy.sh start

# 运行 web E2E
cd integration/web
npm install          # 首次
npm test             # 或 npx playwright test
```

可用环境变量：

- `GITA_URL`：服务地址，默认 `http://127.0.0.1:3100`
- `GITA_TEST_USER` / `GITA_TEST_PASS`：测试账号，默认 `testuser` / `testpass123`

浏览器使用本机缓存的 Chromium（与 `@playwright/test` 版本匹配），无需重新下载。

### CLI 集成测试

```bash
cd <repo-root>
cargo test -p gita-integration-cli
```

## 维护约定

- 新增 Web E2E 时，把 `.spec.ts` 放入 `integration/web/tests/`。
- 新增 CLI 集成测试时，把 `.rs` 放入 `integration/cli/tests/`。
- 不要在各子系统的 `web/tests` 或 `crates/*/tests` 下新增集成测试；集成测试统一收口到 `integration/`。
- 单元测试仍内嵌在各 crate 源码中（`#[cfg(test)] mod tests`）。