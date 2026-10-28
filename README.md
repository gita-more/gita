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
- **Git over SSH** — embedded SSH server (Ed25519 host key), clone / push via
  `git@host:owner/repo.git`, public-key authentication backed by the
  `ssh_keys` table.
- **User accounts & auth** — register / login, JWT sessions, personal access
  tokens (PAT), Argon2 password hashing.
- **In-repo CI** — a `.gita-ci.yml` in the repo defines pipeline stages; a push
  enqueues a run, a runner claims jobs and executes them, with live logs (SSE)
  and cancellation. See [docs/gita-ci-writing-guide.md](docs/gita-ci-writing-guide.md).
- **Pages** — GitHub-style static site hosting from a repository branch,
  served at `/<owner>/<repo>/` (see below).
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
├── gita-core    # domain models, events, the shared access-control rule
├── gita-db      # SQLx data access (PostgreSQL)
├── gita-git      # bare repo management, git protocol via gix + git CLI
├── gita-events  # event bus (pushes, PRs, issues)
├── gita-http    # Smart HTTP Git protocol endpoint (root /owner/repo.git)
├── gita-ssh     # embedded SSH server (Git over SSH)
├── gita-api     # REST API (/api/*) + OpenAPI docs
├── gita-pipeline # .gita-ci.yml parser and stage DAG
├── gita-push    # what a push does afterwards: events + CI enqueue
├── gita-runner  # CI runner (separate binary)
├── gita-web     # static frontend serving
└── gita-server  # entrypoint: config, app assembly, startup
client/           # `gita` CLI client (top-level, see below)
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
| `server.database_url` | `postgres://gita:gita@localhost:5432/gita` | PostgreSQL connection URL |
| `server.jwt_secret` | `dev-secret-change-me` | **Change in production!** |
| `server.ssh.enabled` | `false` | Enable the embedded SSH server (Git over SSH) |
| `server.ssh.host` | `127.0.0.1` | SSH bind address |
| `server.ssh.port` | `2222` | SSH port |
| `server.ssh.host_key` | `data/ssh_host_ed25519_key` | Path to the OpenSSH host private key (auto-generated if missing) |
| `server.pages_enabled` | `true` | Serve GitHub-Pages-style static sites at `/<owner>/<repo>/` |
| `server.pages_url_prefix` | *(empty)* | Optional URL prefix for Pages, e.g. `/p` serves `/p/<owner>/<repo>/` |
| `server.allow_insecure_password_reset` | `false` | Return the password-reset token in the API response (see below) |

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
git clone http://127.0.0.1:3000/<you>/<repo>.git
cd <repo>
git push -u origin main
```

## CLI (`gita`)

Gita ships with [`client/`](client/) — a `gh`-like command-line client named
`gita`, built as a separate workspace member (`gita-client`). It talks to the
Gita REST API over `Authorization: Bearer <token>` and stores credentials in
the OS keyring (never plaintext).

Build it once:

```bash
cargo build -p gita-client
```

The binary lands at `target/debug/gita` (or `target/release/gita`).

### Authentication

```bash
# Register a new account (prints a JWT token stored in the keyring)
./target/debug/gita auth register --username alice --email alice@example.com \
  --password hunter22

# Log in
./target/debug/gita auth login --username alice --password hunter22

# Verify who you are
./target/debug/gita auth whoami
./target/debug/gita auth status
```

Token resolution order: `GITA_TOKEN` env > OS keyring (per server URL) > none.
Server URL resolution: `GITA_URL` env > `~/.config/gita/config.toml` >
`http://127.0.0.1:3000`.

Keyring access is bounded by a 3-second deadline — on Linux the keyring can
block forever in a headless session — and `GITA_DISABLE_KEYRING=1` turns it off
entirely. See [client/README.md](client/README.md#keyring).

### Common commands

```bash
./target/debug/gita repo create --name demo
./target/debug/gita repo list
./target/debug/gita repo view alice demo
./target/debug/gita pr list alice demo
./target/debug/gita issue list alice demo
./target/debug/gita search repos demo
./target/debug/gita event list
./target/debug/gita config show
```

Use `--json` for machine-readable output (scripting friendly). See
[`client/README.md`](client/README.md) for the full command reference.

### Docker deployment (optional)

Gita ships with a `Dockerfile` and `docker-compose.yml` (both in `docker/`).
This is the easiest way to run with PostgreSQL.

```bash
# 1. Prepare environment variables
cp .env.example .env
#    edit .env — at minimum change JWT_SECRET!

# 2. Build and start (the compose file now lives in docker/)
docker compose -f docker/docker-compose.yml up -d --build \
  --env-file .env
# or the recommended wrapper:
./scripts/run_dev.sh start
```

Notes:

- The app listens on port **3000** inside the container; the host port is
  mapped via `GITA_PORT` in `.env` (default `3100`) to avoid conflicts.
  Open <http://127.0.0.1:3100>.
- **Git over SSH:** the container runs an embedded SSH server when
  `GITA_SSH_ENABLED=true` (see `.env.example`). SSH is disabled by default and
  the `2222` host port mapping is commented out in `docker-compose.yml`; to
  use it, set `GITA_SSH_ENABLED=true` in `.env`, uncomment the `2222` port
  mapping, then `git clone ssh://git@<host>:<GITA_SSH_PORT>/owner/repo.git`.
  The web UI has no SSH-key page yet, so register the public key through the
  API first:

  ```bash
  curl -X POST http://127.0.0.1:3100/api/auth/keys \
    -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    -d '{"title":"laptop","public_key":"ssh-ed25519 AAAA... laptop"}'
  ```
- The bundled `postgres` service is the default. Override `DATABASE_URL` in
  `.env` to point at an external Postgres (for managed DBs or multi-node
  setups).
- If your host has no `docker compose` plugin (e.g. Podman 3.x), the
  standalone `docker-compose` v1 works with the same file:

  ```bash
  docker-compose -f docker/docker-compose.yml \
    --env-file .env up -d --build
  ```
- On rootless Podman, point `docker-compose` at the user socket first:

  ```bash
  export DOCKER_HOST=unix://$XDG_RUNTIME_DIR/podman/podman.sock
  docker-compose -f docker/docker-compose.yml \
    --env-file .env up -d
  ```

To run the server directly instead of Docker, follow the Quick Start above
and use PostgreSQL:

```bash
docker compose -f docker/docker-compose.yml --env-file .env up -d postgres
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

## Pages

Repository sites can be published straight from a branch — the same
"deploy from a branch" model as GitHub Pages. No build step: the tree at the
configured ref is served verbatim.

- **Configure** — open a repository → **Pages** tab → pick a branch and a
  publishing directory. The directory is restricted to the repository root
  (`/`) or `/docs`, matching GitHub.
- **Visit** — the site is served at `http://<host>/<owner>/<repo>/`
  (add a trailing slash for the site root). Pages settings show the exact URL.
- **Directory URLs** serve `index.html`; a missing file serves the site's own
  `404.html` (with a 404 status) if present.
- **Privacy** — a private repository's site requires authentication. In this
  version that means a client sending `Authorization: Bearer <token>`; a plain
  browser navigation falls back to the sign-in page.
- **Serving headers** — `X-Content-Type-Options: nosniff` and a strict
  Content-Security-Policy are applied, so sites relying on inline scripts or
  third-party resources may not render as expected.

Turn the feature off entirely with `server.pages_enabled = false`.

## Password reset

`POST /api/auth/password-reset/request` starts a reset and
`POST /api/auth/password-reset/confirm` completes it with the token.

No mail transport is wired up, so a reset token has no way to reach the user
by mail. The request endpoint therefore **never** returns the token by
default: it always answers with the same uniform message, whether or not the
account exists, so it cannot be used to enumerate accounts.

That leaves the token reachable only through the server log, which prints the
exact `POST /api/auth/password-reset/confirm` invocation to use. For a
deployment with no mail transport, set

```toml
[server]
allow_insecure_password_reset = true
```

and the request response carries the token in a `reset_token` field, so an
operator can complete the flow by hand. **Leave it off in production**: the
response is still uniform for existing and non-existent accounts, so returning
the token lets an anonymous caller both enumerate accounts and take them over.

Both endpoints are rate limited per IP, and a reset token is single-use: it is
consumed by the same statement that validates it, so it can never be redeemed
twice.

## Roadmap

See [goal.md](goal.md) for the full product plan. Notable upcoming work:

- Agent sandbox execution and issue → PR automation
- MCP streamable HTTP transport
- RBAC, multi-tenant, object-storage abstraction
- Semantic (embedding-based) code search
- Pages v2: browser-accessible private sites, configurable CSP

## License

[MIT](LICENSE)