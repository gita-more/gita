# Gita

> AI-native, self-hosted Git server written in Rust.

Gita is a self-hosted Git server (clone / push / pull) with an AI-readable
event stream and MCP (Model Context Protocol) integration — so AI agents can
participate in the development workflow as first-class citizens.

**Status: v0.0.1 — early development preview.** APIs and behavior may
change between releases.

## Features

- **Smart HTTP Git protocol** — clone / push / pull via `git http-backend`,
  with per-repository permissions (public anonymous-read, private owner-only,
  push requires the owner).
- **User accounts & auth** — register / login, JWT sessions, personal access
  tokens (PAT), Argon2 password hashing.
- **REST API** — repositories, file tree, commits, branches, pull requests
  (create / merge / close), issues, event stream.
- **MCP tool discovery** — an API endpoint (`/api/mcp/tools`) exposing
  `list_repositories`, `read_file`, `list_commits`,
  `create_pull_request`, `get_pull_request`, `create_issue`,
  `list_events` for MCP clients. (The standalone `gita-mcp` crate was
  removed in v0.0.1; MCP will be reintroduced with the AI features.)
- **Web UI** — React frontend for browsing repositories, files, commits,
  pull requests and issues.
- **OpenAPI docs** — Swagger UI at `/api/docs`.

## Architecture

Cargo workspace with focused crates:

```
crates/
├── gita-core    # domain models, events
├── gita-db      # SQLx data access (SQLite default, PostgreSQL optional)
├── gita-git      # bare repo management, git protocol via gix + git CLI
├── gita-events  # event bus (pushes, PRs, issues)
├── gita-http    # Smart HTTP Git protocol endpoint (/git/*)
├── gita-api     # REST API (/api/*) + OpenAPI docs
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

**Security:** the default `jwt_secret` is a well-known development value.
Change it before exposing Gita to a network.

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

### Docker deployment (optional)

Gita ships with a `Dockerfile` and `docker-compose.yml` for containerized
deployment. This is the easiest way to run with PostgreSQL.

```bash
# 1. Prepare environment variables
cp .env.example .env
#    edit .env — at minimum change JWT_SECRET!

# 2. Build and start
docker-compose up -d --build
```

Notes:

- The app listens on port **3000** inside the container; the host port is
  mapped via `GITA_PORT` in `.env` (default `3100`) to avoid conflicts.
  Open <http://127.0.0.1:3100>.
- By default (`DATABASE_URL` empty in `.env`) the app uses SQLite stored in
  the `gita-data` volume. To use PostgreSQL, set
  `DATABASE_URL=postgres://gita:gita@postgres:5432/gita` in `.env`.
- If your host has no `docker compose` plugin (e.g. Podman 3.x), the
  standalone `docker-compose` v1 works with the same file.
- On rootless Podman, point `docker-compose` at the user socket first:

  ```bash
  export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock
  docker-compose up -d
  ```

To run the server directly instead of Docker, follow the Quick Start above
and use PostgreSQL:

```bash
docker-compose up -d postgres
# then in config/gita.toml:
#   database_url = "postgres://gita:gita@localhost:5432/gita"
```

## MCP Client Setup

**Status:** the standalone `gita-mcp` crate was removed in v0.0.1. MCP
integration will be reintroduced together with the AI features; the API
currently exposes `/api/mcp/tools` for tool discovery.

## Development

```bash
cargo test          # run all Rust tests
cd web && npm run lint && npm run build
```

## Roadmap

See [goal.md](goal.md) for the full product plan. Notable upcoming work:

- PR conflict detection and non-fast-forward merges
- Agent sandbox execution and issue → PR automation
- MCP streamable HTTP transport
- Webhooks, RBAC, multi-tenant

## License

[MIT](LICENSE)