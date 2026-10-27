# `gita` — CLI client for the Gita server

`gita` is a `gh`-like command-line client for the Gita self-hosted Git server.
It's a standalone workspace member (`gita-client`) that talks to the Gita REST
API over `Authorization: Bearer <token>` and stores credentials in the OS
keyring (never in plaintext config).

```
cargo build -p gita-client      # -> target/debug/gita
cargo build --release -p gita-client  # -> target/release/gita
```

## Configuration

| Source | Precedence | What |
|--------|-----------|------|
| `GITA_URL` env | highest | Server base URL, e.g. `http://127.0.0.1:3000` |
| `~/.config/gita/config.toml` | middle | `server_url = "http://..."` |
| default | lowest | `http://127.0.0.1:3000` |

```bash
./target/debug/gita config set-url http://127.0.0.1:3000
./target/debug/gita config show
```

## Authentication

Token resolution order (highest to lowest):

1. `GITA_TOKEN` env var
2. OS keyring (service `gita`, account = server URL)
3. none (anonymous)

Keyring tokens persist across invocations — `GITA_TOKEN` is only needed when
you don't want to log in (e.g. CI):

```bash
# Log in; token is stored in the OS keyring
./target/debug/gita auth login --username alice --password hunter22

# Or register first
./target/debug/gita auth register --username alice \
  --email alice@example.com --password hunter22

# Verify
./target/debug/gita auth whoami
./target/debug/gita auth status

# Log out (revokes token server-side, clears keyring entry)
./target/debug/gita auth logout
```

Passwords may also be supplied via `GITA_PASSWORD` env or `--password-stdin`.

Personal access tokens (PATs):

```bash
./target/debug/gita auth token create --name ci-token   # print once; save it
./target/debug/gita auth token list
./target/debug/gita auth token delete --id 3
```

## Repository commands

```bash
./target/debug/gita repo create --name demo [--description "..."] [--private] \
  [--default-branch main]
./target/debug/gita repo list [owner]
./target/debug/gita repo view alice demo
./target/debug/gita repo delete alice demo
./target/debug/gita repo branches alice demo
./target/debug/gita repo tags alice demo
./target/debug/gita repo commits alice demo [--ref main]
./target/debug/gita repo commit alice demo <sha>
./target/debug/gita repo tree alice demo [--path docs] [--ref main]
./target/debug/gita repo file alice demo README.md
./target/debug/gita repo star alice demo
./target/debug/gita repo unstar alice demo
./target/debug/gita repo fork alice demo --name demo-copy
```

### Clone

No REST endpoint — clones via smart HTTP:

```bash
./target/debug/gita repo clone alice/demo --dir ./demo
```

Builds `https://<user>:<token>@<host>/<owner>/<repo>.git` (token from keyring
or `GITA_TOKEN`) and runs `git clone`. Without a token, clones anonymously
(public repos only).

## Pull requests

```bash
./target/debug/gita pr list alice demo [--status open]
./target/debug/gita pr view alice demo 4
./target/debug/gita pr create alice demo --title "..." --body "..." \
  --source-branch feat --target-branch main
./target/debug/gita pr comment alice demo 4 --body "LGTM"
./target/debug/gita pr merge alice demo 4
./target/debug/gita pr close alice demo 4
./target/debug/gita pr diff alice demo 4
```

## Issues

```bash
./target/debug/gita issue list alice demo [--status open]
./target/debug/gita issue view alice demo 3
./target/debug/gita issue create alice demo --title "..." --body "..."
./target/debug/gita issue comment alice demo 3 --body "..."
./target/debug/gita issue close alice demo 3
```

## Groups

```bash
./target/debug/gita group list
./target/debug/gita group create --name team [--description "..."]
./target/debug/gita group view team
./target/debug/gita group members team
./target/debug/gita group repo team --name demo
```

## Search

```bash
./target/debug/gita search repos demo
./target/debug/gita search code "function foo"
```

## Pipelines / CI

```bash
./target/debug/gita pipeline list alice demo
./target/debug/gita pipeline runs alice demo
./target/debug/gita pipeline trigger alice demo --ref main
./target/debug/gita pipeline logs alice demo <run> <job>
./target/debug/gita pipeline cancel alice demo <run>
```

## Events

```bash
./target/debug/gita event list [--limit 50]
```

## Output format

Every command prints a table by default. Pass `--json` for pretty-printed JSON
(useful for scripting):

```bash
./target/debug/gita repo list alice --json
```

## Environment variables

| Variable | Meaning |
|----------|---------|
| `GITA_URL` | Server base URL (overrides config file) |
| `GITA_TOKEN` | Bearer token (overrides keyring) |
| `GITA_PASSWORD` | Password for login (alternative to prompting) |

## Keyring

Credentials live in the OS keyring under service `gita`, keyed by server URL
(account). On Linux this uses the Secret Service API (D-Bus) via the `keyring`
crate — no C dependencies. Nothing is ever written to disk in plaintext.

## CI

GitHub Actions builds `gita-client` in `.github/workflows/ci.yml` and ships the
release binary via `.github/workflows/release.yml`. Integration tests live in
`integration/cli/tests/gita_cli.rs` and boot a real `gita_server::App` against
Postgres, driving the `gita` binary with `GITA_TOKEN` (keyring is never touched
in CI).
