# `gita` — CLI client for the Gita server

`gita` is a `gh`-like command-line client for the Gita self-hosted Git server.
It is the `gita` binary (workspace member `crates/gita`, sharing the REST
transport with `gita-admin` through the `gita-client` library) that talks to the Gita REST
API over `Authorization: Bearer <token>` and stores credentials in the OS
keyring, or an encrypted local file when no keyring works.

```
cargo build -p gita      # -> target/debug/gita
cargo build --release -p gita  # -> target/release/gita
```

## Configuration

| Source | Precedence | What |
|--------|-----------|------|
| `--url` / `GITA_URL` env | highest | Server base URL, e.g. `http://127.0.0.1:3000` |
| `~/.gita/config.toml` | middle | `[server]` / `url = "http://..."` |
| — | — | nothing configured: the CLI errors out instead of guessing a server |

The config lives in `~/.gita` (`$GITA_CONFIG_DIR` overrides it). The older
`~/.config/gita/config.toml` is still read when the new file is absent, so
an existing setup keeps working; writes always go to `~/.gita`.

```bash
./target/debug/gita config set-url http://127.0.0.1:3000
./target/debug/gita config show
```

## Authentication

Token resolution order (highest to lowest):

1. `GITA_TOKEN` env var
2. the credential store (OS keyring, or the encrypted local file)
3. none (anonymous)

Credentials are keyed by server URL, so several servers can be logged in at
once. `auth login` / `auth register` make the server they logged into the
default one — otherwise later commands (which resolve the default) would
look up a different URL than the one the token was stored under.

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
| `GITA_TOKEN` | Bearer token (overrides the credential store) |
| `GITA_PASSWORD` | Password for login (alternative to prompting) |
| `GITA_DISABLE_KEYRING` | Never touch the OS keyring (see below) |
| `GITA_CONFIG_DIR` | Client config directory (default `~/.gita`) |

## Credential store

Credentials live in the OS keyring under service `gita`, keyed by server URL
(account). On Linux this uses the Secret Service API (D-Bus) via the `keyring`
crate — no C dependencies.

Every keyring call is bounded by a 3-second deadline. On Linux the keyring can
*block* rather than fail: a locked Secret Service waits for an unlock agent
that will never arrive in a headless session, and the kernel keyring backend
waits in `request_key()` for a user keyring that `pam_keyinit` never created.
The CLI treats the keyring as best-effort, so a blocked call is abandoned
after the deadline.

### Encrypted local fallback

A keyring that cannot store the token must not lose the session: the CLI
would log in successfully and then be anonymous again on the next command. So
when the keyring is unavailable (missing, timed out, or disabled) the token
degrades to `~/.gita/credentials.enc` — encrypted with XChaCha20-Poly1305
under a machine-bound key. The key lives in the OS keyring when the keyring
works, otherwise in a `0600` `~/.gita/keyfile` (the directory is `0700`).
Reads prefer the keyring and fall back to the file; `auth logout` clears
both.

This is transparent — nothing is asked beyond the login already typed — and
it protects the realistic threat: the disk's contents leaving the machine
(backups, images, sync clients). An attacker who can run code as the user
would simply run `gita` itself, so the local keyfile costs nothing there. A
store that cannot be decrypted (the key was deleted, the file came from
another machine) reads as empty rather than failing: the credentials are
gone either way and the answer is a fresh login. A pre-encryption plaintext
`credentials.toml` is picked up transparently and migrated to the encrypted
file on the next write.

The CLI says when the fallback store was used:

```
warning: no usable OS keyring on this machine — the token is stored in /home/you/.gita/credentials.enc, encrypted with a local key (~/.gita/keyfile)
```

Set `GITA_DISABLE_KEYRING=1` to skip the keyring entirely — no token is read
from it and nothing is written to it (the encrypted fallback is still used):

```bash
GITA_DISABLE_KEYRING=1 ./target/debug/gita repo list
```

## Git credential helper

`gita auth credential` implements git's credential helper protocol, so
`git push` reuses the token you already logged in with instead of prompting
for it again:

```bash
git config --global credential.helper "gita auth credential"
```

Point it at the absolute path of the binary if it is not on `PATH`:

```bash
git config --global credential.helper "/path/to/gita auth credential"
```

The helper answers from the credential store, matched by scheme and host —
git only tells a helper the host, not the repository path.

## CI

GitHub Actions builds `gita` in `.github/workflows/ci.yml` and ships the
release binary via `.github/workflows/release.yml`. Integration tests live in
`integration/cli/tests/` and boot a real `gita_server::App` against
Postgres, driving the `gita` binary with `GITA_TOKEN` and
`GITA_DISABLE_KEYRING=1` (the keyring is never touched in CI).
