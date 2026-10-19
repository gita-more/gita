# Writing `.gita-ci.yml` — a complete user guide

This is the practical writing guide for gita's in-repo CI. It is
modelled on [GitLab CI's `.gitlab-ci.yml` reference](https://docs.gitlab.com/ee/ci/yaml/)
and uses the same vocabulary, but only documents the keywords that
gita understands **today**.

If you already know GitLab CI, this is a quick map; if you don't, the
[Quick start](#quick-start) walks you through a realistic first
pipeline.

## Quick start

A repo with the following file at its root will run `cargo build`
and `cargo test` on every push to `main`:

```yaml
stages:
  - build
  - test

build:
  stage: build
  script:
    - cargo build --release

unit:
  stage: test
  script: cargo test --lib
  needs: [build]
```

That is the entire Phase 1 surface. The runner picks up each push,
runs the jobs in the order the `stages` + `needs:` graph dictates,
and writes the stdout of each job to the database. The web UI shows
the run list, the per-job logs, and the run / job status badges.

## Top-level structure

```yaml
stages: [build, test, deploy]    # required, ≥1 stage, in execution order

# Optional trigger filters — see "Trigger filters" below.
only:
  branches: [main, release/*]
except:
  paths: ["docs/**"]

# Jobs. You may declare them under a top-level `jobs:` block, or
# top-level keys (GitLab's flat style). gita accepts either.
jobs:
  compile:
    stage: build
    script: [cargo build]
  unit:
    stage: test
    script: [cargo test --lib]
    needs: [compile]
```

### `stages:` (required)

An ordered list of stage names. Every job's `stage:` must appear in
this list. Stages run sequentially; jobs within a stage run serially
inside one runner today (Phase 2 will add per-stage parallelism).
You must declare at least one stage.

```yaml
stages: [lint, test, build, deploy]    # declaration order = execution order
```

If a job's `stage:` is missing or unknown, the pipeline is rejected
with `UnknownStage { job, stage, declared }`.

### `only:` / `except:` (Phase 2 trigger filters)

Push triggers run by default. To opt out, declare `only:` (allowlist)
or `except:` (denylist) at the top level. Both keys take the same
shape:

```yaml
only:
  branches: [main, release/*]   # branch globs
  paths:    [src/**]            # path globs (relative to repo root)

except:
  paths:    [docs/**, "*.md"]
```

The semantics are:

- **Empty `only:`** → no branch/path constraint (every event can run).
- **Non-empty `only:`** → at least one of `branches` or `paths` must
  match. (Push's branch matches one of the branch globs, *or* any
  touched path matches one of the path globs.)
- **`except:`** is applied after `only:`. Any matching `except.branches`
  or `except.paths` disqualifies the run.

Glob syntax: `*` matches any non-`/` characters, `**` matches across
directories, `?` matches one character. Patterns without metachars
fall back to plain equality.

> Phase 2 evaluates the **branch** filter immediately; path filtering
> needs the receive-pack handler to know which files a push touched
> (Phase 3 will plumb `git diff-tree` through).

### Job keys

Every other top-level (or `jobs:`-nested) entry is a job. A job name
must not be one of the reserved keywords (`image`, `services`,
`stages`, `types`, `before_script`, `after_script`, `variables`,
`cache`, `include`).

#### `stage:` (required, default: `test`)

The stage's name. Must appear in the top-level `stages:` list.

```yaml
unit:
  stage: test
  script: cargo test
```

#### `script:` (required)

The shell commands to run. Each list element is passed to `sh -c` in
sequence with ` && ` between them. The runner prepends `set -e`, so
the first non-zero exit code terminates the job.

```yaml
deploy:
  stage: deploy
  script:
    - ./scripts/build-binaries
    - cp ./bin/* /srv/app/bin/
    - systemctl restart app.service
```

A single string is also accepted:

```yaml
test:
  stage: test
  script: cargo test
```

#### `needs:`

Cross-stage (or same-stage) DAG predecessor jobs. The job waits for
every named job to finish successfully before starting.

```yaml
release:
  stage: deploy
  script: ./release.sh
  needs: [unit, lint]
```

`expand()` in `gita-pipeline` does a Kahn topological sort by
`(stage_index, needs)`. The runner emits jobs in an order that
always runs a job after all of its `needs` dependencies.

> Phase 1 only supports the simple list form (`needs: [a, b]`). The
> object form (`needs: [{ job: a, artifacts: false }, ...]`) is parsed
> but only `job` is honoured.

## Triggering

By default every push triggers a run — including pushes to any
branch, and pushes that delete a branch. The runner drops branch
deletions automatically (the receive-pack handler skips zero-SHA
new_sha). Use `only:` / `except:` for finer control.

You can also trigger a run through the web UI: open the Pipelines tab
on a repo and click **Run pipeline**. The trigger endpoint accepts
the same `only/except` semantics.

## What's intentionally deferred (Phase 2/3 roadmap)

The next two phases add these keywords:

| Phase | Keyword | What it adds |
| --- | --- | --- |
| 2 | `retry:` / `timeout:` | Retry the job on failure; wall-clock cap. |
| 2 | `parallel:` | Run a job N times as a matrix. |
| 2 | `interruptible:` | Allow a job to be cancelled by a newer push. |
| 2 | `cache:` | Persist files between runs (`paths`, `key`, `policy`). |
| 2 | `artifacts:` | Save job outputs (`paths`, `expire_in`, `reports`). |
| 2 | `environment:` | Named deploy targets with on-stop hooks. |
| 2 | `rules:` | Conditional job inclusion (replaces `only/except`). |
| 3 | `image:` / `services:` | Run jobs in Docker. |
| 3 | `include:` | Split config across files. |
| 3 | `variables:` | Per-pipeline / per-job env. |
| 3 | `secrets:` | Mount external secrets. |
| 3 | `trigger:` | Spawn child / multi-project pipelines. |
| ✗ | `dast_configuration:` | Dynamic AppSec Testing; n/a for a self-hosted git server. |
| ✗ | `pages:` | GitLab Pages; n/a here. |

## Three example pipelines

### A single-job "does it compile" pipeline

```yaml
stages: [check]
build:
  stage: check
  script: cargo check --all-targets
```

### A multi-stage matrix with `needs:` + `only:`

```yaml
stages: [build, test, deploy]

only:
  branches: [main, release/*]

build:
  stage: build
  script:
    - cargo build --release
    - mkdir -p .artifacts
    - cp target/release/app .artifacts/

unit:
  stage: test
  script: cargo test --lib
  needs: [build]

lint:
  stage: test
  script: cargo clippy --all-targets -- -D warnings
  needs: [build]

deploy:
  stage: deploy
  script: ./scripts/deploy.sh .artifacts/app
  needs: [unit, lint]
```

### "Only rebuild docs when docs changed" — `only:` + `except:` with paths

```yaml
stages: [docs, build]

except:
  paths:
    - docs/**
    - "*.md"

build:
  stage: build
  script: cargo build

docs:
  stage: docs
  script: mkdocs build
```

The `build` job runs on every push; the `docs` job only runs when the
push did **not** touch any markdown / doc path. (Path filtering
lights up once Phase 3 plumbs the changed-paths through the
receive-pack handler.)

## Where to look next

- `docs/gitlab-ci-yaml-reference.md` — the keyword matrix mapped to
  gita's Phase 1/2/3 support status.
- [GitLab CI/CD YAML syntax reference](https://docs.gitlab.com/ee/ci/yaml/)
  — the canonical source we mirror.
- [`rules:`](https://docs.gitlab.com/ee/ci/yaml/rules.html),
  [`needs:`](https://docs.gitlab.com/ee/ci/yaml/needs.html),
  [`artifacts:`](https://docs.gitlab.com/ee/ci/yaml/artifacts.html),
  [`cache:`](https://docs.gitlab.com/ee/ci/yaml/cache.html),
  [`trigger:`](https://docs.gitlab.com/ee/ci/yaml/trigger.html)
  — GitLab's per-keyword deep dives; many of the Phase 2/3 keywords
  inherit semantics from here.