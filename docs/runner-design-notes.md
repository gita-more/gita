# gita-runner — design notes from GitHub Actions

This document captures what gita-runner could learn from GitHub
Actions' runner architecture, and how to close the gap between the
two. It is a forward-looking reference, not a record of work already
done.

The current gita-runner is a single Rust process that polls
`pipeline_runs` in Postgres, claims a run with `FOR UPDATE SKIP
LOCKED`, and executes the job's shell script directly inside the
runner process via `tokio::process::Command::new("sh") -c "..."`.
That is fine for a developer tool, but it is not the model GitHub
Actions uses and not the model gita should ship for production.

## Background: how GitHub Actions runs a job

When a workflow references `services: postgres`, GitHub Actions does
**not** start `docker-compose`. The flow is:

1. A runner (GitHub-hosted VM or your self-hosted machine) is
   allocated to the job.
2. The runner process invokes Docker to start **one container per
   service** plus **one container for the job steps themselves**.
3. All of these containers are joined to the **same Linux network
   namespace** as the runner. That is why `localhost:55432` (host)
   reaches postgres' port 5432 inside its container, and why the
   runner and the job step container can talk to each other without
   extra plumbing.
4. `services.ports: "HOST:CONTAINER"` controls which host port is
   mapped onto the container's listening port. Format is `HOST:CONTAINER`
   (Docker convention), which is the opposite of how some people
   guess.
5. Health is declared with `--health-cmd pg_isready ...` and steps
   that need the service can wait on it explicitly.

This gives every job a **fresh, isolated execution environment** with
optional sidecar services, plus clean resource accounting.

## gita-runner today vs GitHub Actions

| Concern                       | GitHub Actions runner                  | gita-runner (today)                  |
|-------------------------------|----------------------------------------|---------------------------------------|
| Execution isolation           | Fresh container per job                | `sh -c` inside the runner process     |
| Network isolation             | Per-job container, shared with services | None (everything shares the host)    |
| Sidecar services              | Declarative `services:` block          | None                                 |
| Matrix builds                 | First-class                            | None                                 |
| Artifact upload               | `actions/upload-artifact`              | Logs only (via `logs` table)          |
| Dependency cache              | `actions/cache`                        | None (cargo rebuilds every time)     |
| Job timeout                   | Per-job + per-step                     | None                                 |
| Runner health heartbeat       | `actions/runner` heartbeat             | Implicit via successful claim         |
| Runner labelling              | `self-hosted`, `linux`, `gpu`, ...     | Single global runner                 |
| Per-resource limits           | `--memory`, `--cpus`, `--pids-limit`   | None                                 |
| Step-level conditionals       | `if:`, `continue-on-error`             | Plain shell `set -e`                 |
| Secrets                       | Encrypted env from repo                | Env only                             |
| Concurrency                   | Multiple jobs per runner, isolated     | Single-job-at-a-time                 |

## What gita-runner should borrow

The recommendations are ordered by impact-to-effort ratio.

### 1. Sandbox each job with Docker

Today `executor.rs` runs the user's shell script inside the runner
process. That script can read every other job's workspace, exfiltrate
the runner's environment, fill the disk, and never time out.

Borrow the GitHub model: each job runs inside a short-lived container
created from a configurable image (`runs-on: ubuntu-latest` →
`image = "ubuntu:22.04"`). Resource limits (`--memory`, `--cpus`,
`--pids-limit`) and a read-only root filesystem with a tmpfs for
`/tmp` give a baseline of safety. The job's workspace directory is
bind-mounted into the container.

```
docker run --rm \
  --network=job-NET \
  --memory=2g --cpus=2 --pids-limit=200 \
  --read-only --tmpfs /tmp:size=512m \
  -v <workspace>:/workspace \
  -v<cache>:/cache \
  -e GIT_TOKEN=... \
  image:latest sh -c "<user script>"
```

`JobRegistry` (currently just a `HashMap<run_id, Child>`) extends to
track the docker container id, and `kill_run` becomes
`docker kill <id>`.

### 2. Per-job and per-step timeouts

A malicious or just buggy script will otherwise run forever and tie
up the runner. Add `RunnerConfig.job_timeout` (default 30 min) and
`RunnerConfig.step_timeout` (default 10 min). The executor enforces
with `tokio::time::timeout` wrapping the docker wait, and the
JobRegistry's `kill_run` becomes the cancellation path.

### 3. Service containers

If gita-ci is going to host real test suites, those suites need
Postgres, Redis, etc. Add a `services:` block to `.gita-ci.yml`:

```yaml
jobs:
  test:
    runs-on: ubuntu
    services:
      postgres:
        image: postgres:17
        env:
          POSTGRES_USER: gita
          POSTGRES_PASSWORD: gita
          POSTGRES_DB: gita
        ports:
          - 55432:5432
    steps:
      - run: |
          PGPASSWORD=gita psql -h localhost -p 55432 -U gita -d gita -c "SELECT 1"
```

The executor builds a per-job Docker network, starts each service
container on it, and runs the job container on the same network so
that `localhost:55432` reaches postgres. The host:container port
format follows Docker's convention.

### 4. Runner registration + heartbeat + labels

Today `gita-server worker register` mints a token; the runner then
just runs. Borrow GitHub's model: a `runner_registrations` table
holds the runner's id, name, labels, last heartbeat, and current
status (`idle` / `busy` / `offline`). The runner heartbeats every
~30 s. The server only assigns jobs to runners whose labels match
the job's `runs-on` and whose last heartbeat is fresh.

This unlocks multiple runners, GPU runners, ARM runners, and so on,
all from the same control plane.

### 5. Dependency cache

`cargo build --release` and `npm ci` dominate wall-clock time.
Borrow `actions/cache`:

- Before the job, the runner tries to restore `<key>` from the cache
  table, writing into `<mount>` inside the container.
- After the job, the runner tar's `<mount>` and stores it under
  `<key>` (a hash of the lockfile).

Keys are derived from the lockfile's hash, so cache entries are
reused across runners and across runs.

### 6. Artifact upload

Today only the stream of log lines is preserved. Borrow
`actions/upload-artifact`: at the end of a job, the runner tars a
configurable directory (e.g. `./dist`, `./target/release/deps/`)
and uploads it to the server. A new `artifacts` table stores
metadata; the bytes go to either Postgres (large bytea) or S3 /
MinIO.

### 7. Matrix builds

Useful for cross-version testing (`rust 1.75`, `rust 1.76`) and
cross-OS testing (`ubuntu`, `macos`). Add a `matrix:` keyword that
expands one job into N. The server creates N pipeline runs; runners
filter by the matrix axis via labels.

### 8. Step conditionals

`if: failure()` / `continue-on-error: true` are powerful for
running cleanup steps only on failure, or for tolerating flaky
steps. Adds a small prelude to the generated shell script; no
architectural change needed beyond the executor.

## Recommended implementation order

| # | Feature               | Effort | Impact | One-line reason                                  |
|---|-----------------------|--------|--------|--------------------------------------------------|
| 1 | Docker sandbox        | Medium | Huge   | Safety is the single biggest gap.               |
| 2 | Job timeout           | Small  | High   | Stops a bad script from pinning a runner.        |
| 3 | Service containers    | Medium | High   | Unlocks CI that needs a database.                |
| 4 | Runner labels + heartbeat | Small  | Medium | Required for multi-runner deployments.        |
| 5 | Cache                 | Medium | Medium | Cuts the cost of repeat CI runs.                |
| 6 | Artifact upload       | Medium | Medium | Builds need binaries, logs need bundles.         |
| 7 | Matrix                | Medium | Medium | Cross-version testing.                           |
| 8 | Step conditionals     | Small  | Low    | Nice polish.                                      |

Items 1, 2, and 4 together turn gita-runner from "developer tool"
into "production runner".

## Reference architecture

```
gita-server (control plane)
  ├── pipeline_runs         (existing)
  ├── runner_registrations  (new: name, labels, last_seen, status)
  ├── artifacts             (new: blob + metadata)
  └── cache                 (new: <key> -> tarball)

gita-runner (worker node, possibly many)
  ├── heartbeat tick (every 30s, updates runner_registrations)
  ├── claim loop    (FOR UPDATE SKIP LOCKED + label match)
  └── for each job:
      ├── docker network create job-NET
      ├── docker run -d --network=job-NET <each service>
      ├── docker run --rm \
      │     --network=job-NET \
      │     --memory=2g --cpus=2 --pids-limit=200 \
      │     --read-only --tmpfs /tmp:size=512m \
      │     -v <workspace>:/workspace \
      │     -v<cache>:/cache \
      │     -e GIT_TOKEN=... \
      │     image:latest sh -c "<user script>"
      │   wrapped in tokio::time::timeout(job_timeout)
      ├── docker run --network=job-NET artifact-uploader ./dist ./target
      ├── docker run --network=job-NET cache-saver /cache
      └── docker network rm job-NET
```

This is essentially GitHub Actions' runner model, expressed in gita's
own crates.

## Things to NOT borrow

- **GitHub's size of platform** — gita has one product; K8s-style
  pluggable runners is overkill.
- **GitHub's webhooks for triggers** — gita's database polling is
  simpler and fine for self-hosted scale.
- **Org/enterprise separation** — gita is single-tenant per
  deployment.

## See also

- `gita-ci-writing-guide.md` — what users write in `.gita-ci.yml`.
- `crates/gita-runner/src/executor.rs` — current single-process
  executor (the surface that needs the biggest rewrite for item 1).