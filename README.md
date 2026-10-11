# Gita

> AI-native, self-hosted Git server written in Rust.

Gita is a self-hosted Git server (clone / push / pull) with AI-native
capabilities built in: automatic AI code review on push, an AI-readable
event stream, semantic code search, and MCP (Model Context Protocol)
integration — so AI agents can participate in the development workflow as
first-class citizens.

**Status: v0.0.1 — early development preview.** APIs and behavior may
change between releases.

## Features

- **Smart HTTP Git protocol** — clone / push / pull via `git http-backend`,
  with per-repository permissions (public anonymous-read, private owner-only,
  push requires the owner).
- **User accounts & auth** — register / login, JWT sessions, personal access
  tokens (PAT), Argon2 password hashing.
- **REST API** — repositories, file tree, commits, branches, pull requests
  (create / merge / close), issues, AI reviews, event stream, code search.
- **AI code review** — automatically triggered on push, produces line-level
  comments and a risk score; reviews can be discussed, edited, and comments
  resolved.
- **MCP Server** — stdio transport, implemented as a library exposing
  `list_repositories`, `read_file`, `list_commits`, `search_code`,
  `create_pull_request`, `get_pull_request`, `get_review`, `create_issue`,
  `list_events`. (Runnable CLI entrypoint not yet shipped.)
- **Web UI** — React frontend for browsing repositories, files, commits,
  pull requests, issues and AI reviews.
- **OpenAPI docs** — Swagger UI at `/api/docs`.

## Architecture

Cargo workspace with focused crates:

```
crates/
├── gita-core    # domain models, events
├── gita-db      # SQLx data access (SQLite default, PostgreSQL optional)
├── gita-git      # bare repo management, git protocol via gix + git CLI
├── gita-events  # event bus (pushes, reviews, PRs, issues)
├── gita-ai      # LLM review engine, embedding abstractions
├── gita-http    # Smart HTTP Git protocol endpoint (/git/*)
├── gita-api     # REST API (/api/*) + OpenAPI docs
├── gita-mcp     # MCP server (stdio)
├── gita-web     # static frontend serving
└── gita-server  # entrypoint: config, app assembly, startup
```

## Requirements

- **Rust** (stable toolchain, e.g. via [rustup](https://rustup.rs/))
- **git** CLI on `PATH` (used by the git protocol backend and diff/merge)
- **Node.js + npm** (only needed to build the frontend; a prebuilt `dist`
  can be served instead)

## Quick Start

### 1. Build

```bash
# Backend
cargo build --release

# Frontend (optional — build the React UI)
cd web
npm install
npm run build
cd ..
```

### 2. Configure

```bash
cp config/config.example.toml config/gita.toml
# edit config/gita.toml — at minimum change jwt_secret!
```

Configuration is TOML:

| Key | Default | Description |
|---|---|---|
| `server.host` | `127.0.0.1` | Bind address (`0.0.0.0` to expose) |
| `server.port` | `3000` | HTTP port |
| `server.data_dir` | `data` | Repositories and database location |
| `server.database_url` | `sqlite://data/gita.db` | SQLite or PostgreSQL URL |
| `server.jwt_secret` | `dev-secret-change-me` | **Change in production!** |
| `llm.base_url` | empty | OpenAI-compatible API base URL (e.g. Ollama) |
| `llm.api_key` | empty | API key (empty = stub reviews) |
| `llm.model` | `gpt-4o-mini` | Model name |

**Security:** the default `jwt_secret` is a well-known development value.
Change it before exposing Gita to a network.

**AI reviews:** with `llm.base_url` / `llm.api_key` empty, Gita uses a stub
review provider (returns a canned review). Point it at any OpenAI-compatible
endpoint — including a local Ollama instance — for real reviews.

### 3. Run

```bash
cargo run --release -- -c config/gita.toml
```

Then open <http://127.0.0.1:3000>:

1. Register a user.
2. Create a repository.
3. Clone and push:

```bash
git clone http://127.0.0.1:3000/git/<you>/<repo>.git
cd <repo>
git push -u origin main
```

A push automatically triggers an AI review; open the repository in the web
UI to see it.

### PostgreSQL (optional)

The default is SQLite (zero dependencies). For PostgreSQL with pgvector,
use the provided `docker-compose.yml`:

```bash
docker compose up -d
```

Then set `database_url = "postgres://gita:gita@localhost:5432/gita"` in
`config/gita.toml`.

## MCP Client Setup

**Status:** the MCP server is implemented as a library (`gita-mcp`,
stdio transport). A runnable CLI entrypoint is not yet available in
v0.0.1 — this will ship in a later iteration.

## Development

```bash
cargo test          # run all Rust tests
cd web && npm run lint && npm run build
```

## Roadmap

See [goal.md](goal.md) for the full product plan. Notable upcoming work:

- Semantic code search (pgvector embeddings)
- PR conflict detection and non-fast-forward merges
- Agent sandbox execution and issue → PR automation
- MCP streamable HTTP transport
- Webhooks, RBAC, multi-tenant

## License

[MIT](LICENSE)