# gita-ci — the complete user guide

gita-ci is the in-repo CI built into gita. A pipeline is a single
`.gita-ci.yml` file at the repo root, modelled on [GitLab CI's
`.gitlab-ci.yml`][gl-yaml]; the keyword set, the examples, and the
reference structure are tracked directly here.

[gl-yaml]: https://docs.gitlab.com/ee/ci/yaml/

## Quick start

```yaml
stages: [build, test, deploy]

build:
  stage: build
  script:
    - cargo build --release

unit:
  stage: test
  script: cargo test --lib
  needs: [build]

deploy:
  stage: deploy
  script: ./scripts/deploy.sh
  needs: [unit]
```

That is the entire Phase 1 surface. The runner picks up each push,
runs the jobs in the order the `stages:` + `needs:` graph dictates,
and writes each job's stdout to the database. The web UI shows the
run list, the per-job logs, and the run / job status badges.

---

## Global keywords

Top-level keys. The pipeline is a single `.gita-ci.yml` file at the
repo root.

| Keyword | Phase | What it does |
| --- | --- | --- |
| `stages:` | **1** | Required ordered list of stage names; jobs run stage-by-stage. |
| `only:` | 2 | Push-trigger allowlist (`branches` / `paths` globs). |
| `except:` | 2 | Push-trigger denylist (`branches` / `paths` globs). |
| `variables:` | 3 | Pipeline-level CI/CD variables injected into every job's env. |
| `include:` | 3 | Import other YAML files (local / remote / template / project / artifact). |
| `default:` | 3 | Per-keyword defaults applied to every job that omits the keyword. |
| `workflow:` | 3 | Controls which pipelines run (`workflow: { rules: ... }`). |

> Reserved names: jobs cannot be named `image`, `services`,
> `stages`, `types`, `before_script`, `after_script`, `variables`,
> `cache`, or `include`.

### `stages:` (required)

An ordered list of stage names. Every job's `stage:` must appear
in this list. Stages run sequentially; jobs within a stage run
serially inside one runner today (Phase 2 will add per-stage
parallelism).

```yaml
stages: [lint, test, build, deploy]
```

If a job's `stage:` is missing or unknown, the pipeline is rejected
at parse time with `UnknownStage { job, stage, declared }`.

---

## Job keywords

Every other top-level (or `jobs:`-nested) entry is a job.

| Keyword | Phase | What it does |
| --- | --- | --- |
| `stage:` | **1** | Which stage this job belongs to. |
| `script:` | **1** | The shell command(s) the job runs (joined with `&&`). |
| `needs:` | **1** | DAG-style predecessor jobs (list form). |
| `image: / services / variables / etc.` | 3 | Docker, sidecars, env, secrets, etc. |
| `cache:` | 2 | Per-job / global cache files persisted across runs. |
| `artifacts:` | 2 | Files produced by the job, persisted and downloadable. |
| `retry: / timeout: / interruptible:` | 2 | Failure handling. |
| `parallel: / matrix:` | 2 | Run a job N times as a matrix. |
| `environment:` | 2 | Named deploy target (track, on-stop, kubernetes, etc.). |
| `rules:` | 2 | Conditional job inclusion (replaces `only/except`). |
| `trigger:` | 3 | Spawn child / multi-project pipelines. |
| `extends:` | 3 | Reuse config from another job in the same file (YAML anchors). |

### `script:`

The runner executes `script` as `sh -c "<joined with ' && '>"`. Shell
metacharacters (`;`, `|`, `&&`, `||`) are interpreted by the shell.
Phase 1 prepends `set -e`, so any non-zero exit stops the job.

```yaml
build:
  stage:    build
  script:
    - cargo build --release
    - cp target/release/app ./bin/
```

A single-string form is also accepted:

```yaml
test:
  stage:  test
  script: cargo test
```

### `stage:`

Names the stage this job belongs to. Must appear in the top-level
`stages:` list. Defaults to `test` if no `stage:` is given.

### `needs:`

Cross-stage (or same-stage) DAG predecessor jobs. The job waits for
every named job to finish successfully before starting.

```yaml
release:
  stage:  deploy
  script: ./release.sh
  needs: [unit, lint]
```

`expand()` in `gita-pipeline` does a Kahn topological sort by
`(stage_index, needs)`. The runner emits jobs in an order that
always runs a job after all of its `needs:` predecessors.

The runner emits jobs in an order that always runs a job after
all of its `needs:` predecessors.

> Phase 1 only supports the simple list form (`needs: [a, b]`). The
> object form (`needs: [{ job: a, artifacts: false }, ...]`) is
> parsed but only `job` is honoured.

---

## `rules:` (Phase 2)

Replaces `only/except`. `rules:` is an **array of rule objects**
evaluated top-to-bottom; the first rule that matches determines
whether the job runs and with which `when:` value.

Each rule can use:

- `if:` — a CI/CD-variable expression.
- `changes:` — globs of file paths; the rule matches if **any** of
  the paths touched in the push satisfies a glob.
- `exists:` — globs of files that must (or must not) exist in the
  repo.
- `when:` — `on_success` / `on_failure` / `always` / `manual` /
  `delayed` / `never`.
- `allow_failure:` — if set, a failure doesn't break the pipeline.
- `needs:` — override stage-level needs for this rule.

### `rules:if`

The `if:` expression evaluates against CI/CD variables. Operators:

- `==` / `!=` — string equality
- `=~` / `!~` — regex match / negative regex match
- `&&` / `||` — boolean AND / OR

```yaml
job:
  script: echo "Hello, Rules!"
  rules:
    - if: $CI_MERGE_REQUEST_SOURCE_BRANCH_NAME =~ /^feature/ && \
            $CI_MERGE_REQUEST_TARGET_BRANCH_NAME != $CI_DEFAULT_BRANCH
      when: never
    - if: $CI_MERGE_REQUEST_SOURCE_BRANCH_NAME =~ /^feature/
      when: manual
      allow_failure: true
    - if: $CI_MERGE_REQUEST_SOURCE_BRANCH_NAME
```

### `rules:changes`

`changes:` accepts an array of file globs. The rule matches if
**any** of the paths touched in the push satisfies a glob.

```yaml
docker build:
  script: docker build -t my-image:$CI_COMMIT_REF_SLUG .
  rules:
    - if: $CI_PIPELINE_SOURCE == "merge_request_event"
      changes:
        - Dockerfile
      when: manual
      allow_failure: true
```

Glob syntax:

- `*` — matches any non-`/` characters within a single segment
- `**` — matches across directory segments
- `?` — matches one character
- `*.{rb,py,sh}` — brace expansion
- `*.json` — matches dotfiles too

GitLab uses Ruby `File.fnmatch` with
`FNM_PATHNAME | FNM_DOTMATCH | FNM_EXTGLOB`. Up to **50 patterns**
per `changes:` block.

> Use `changes:` with branch / MR pipelines. For tag / scheduled /
> manual pipelines (which don't have a "list of changed files"),
> `changes:` is always false — gate it with `rules: - if:
> $CI_PIPELINE_SOURCE == "push"`. To compare paths against an older
> ref, use `compare_to` (GitLab 15.5+).

### `rules:exists`

`exists:` checks whether a file or directory exists in the repo
after the push. Supports glob patterns and CI/CD variables; up to
**10,000 patterns** are evaluated.

```yaml
job:
  script: build
  rules:
    - exists:
        - Dockerfile
```

### Job-inclusion logic

A job is **added** when an `if`, `changes`, or `exists` matches with
`when: on_success` (default), `delayed`, or `always` — OR a rule is
reached that is *only* one of those `when` values.

A job is **not added** when no rules match, or a matching rule is
`when: never`.

> Phase 1 implements `only/except` only. Phase 2 will add a strict
> subset of `rules:` (likely just `if:` + `changes:`).

---

## `needs:` (DAG)

`needs:` creates a **directed-acyclic-graph** pipeline where a job
can start as soon as all its named predecessors finish — without
waiting for an entire stage to complete.

- Up to **50 jobs** in the array.
- `needs: []` (empty) starts the job as soon as the pipeline is
  created.
- `optional: true` lets a `needs:` predecessor be absent (e.g.
  when filtered out by another `rules:` block).
- `artifacts: true | false` controls whether the predecessor's
  artifacts are downloaded into this job's workspace. Cannot combine
  with the older `dependencies:` keyword.

### Object form (Phase 2+)

```yaml
needs:
  - job: build
    artifacts: true
  - job: test
    optional: true
```

### `needs:project:` (Phase 3, GitLab Premium/Ultimate)

Pulls artifacts from up to 5 jobs in another project's latest
successful pipeline. Requires `project: <path>`, `job: <name>`,
`ref: <branch>`, `artifacts: true`.

### `needs:pipeline:job:` (Phase 3, GitLab 13.7+)

Crosses between parent and child pipelines.

### DAG patterns

```yaml
build:
  stage: build

test-job1:
  stage: test

test-job2:
  stage: test
  rules:
    - if: $CI_COMMIT_BRANCH == $CI_DEFAULT_BRANCH

deploy-job:
  stage: deploy
  needs:
    - job: test-job2
      optional: true
    - job: test-job1
  environment: production

review-job:
  stage: deploy
  needs:
    - job: test-job2
      optional: true
  environment: review
```

- If the optional job is present → the dependent job waits for it.
- If the optional job is not present → the dependent job proceeds
  when other needs are met.
- If all needs are optional and none are present → the job starts
  immediately (same as `needs: []`).
- If `optional: false` (default) and the job is missing → pipeline
  fails to start with `'job1' job needs 'job2' job, but it was not
  added to the pipeline'`.

---

## `cache:` (Phase 2)

```yaml
cache:
  key: ${CI_COMMIT_REF_SLUG}
  paths:
    - node_modules/
  policy: pull-push
  untracked: true
```

Sub-keys:

- `paths:` — array of file globs. Same syntax as `artifacts.paths`.
- `key:` — identifier used to share caches across pipelines. Defaults
  to `default`. Set to a per-branch / per-job key to scope caches.
- `policy:` — `pull`, `push`, or `pull-push` (default). `pull`
  downloads but doesn't upload; `push` uploads but doesn't download;
  `pull-push` does both.
- `untracked: true` — also cache any Git-untracked file in the project.
- `when:` — `on_success` (default) / `on_failure` / `always`.
- `fallback_keys:` — list of keys tried in order if the primary key
  misses.

### Cache lookup order

When `cache: key: K` is set, the runner looks up caches in this
order and stops at the first hit:

1. `cache:key: K`
2. Each entry in `cache:fallback_keys:` in order
3. The pipeline-level variable `CACHE_FALLBACK_KEY` (if set)

```yaml
variables:
  CACHE_FALLBACK_KEY: fallback-key

test-job:
  stage: build
  cache:
    key: "cache-$CI_COMMIT_REF_SLUG"
    fallback_keys: ["cache-$CI_DEFAULT_BRANCH", "cache-default"]
    paths: [vendor/ruby]
```

### Sharing cache across jobs

Two jobs share a cache if they have the same `key:`, the same
`policy:` direction includes the producer's write, and the consumer's
read. A typical pattern:

```yaml
prepare-dependencies:
  script: bundle install
  cache:
    key: gems
    paths: vendor/bundle
    policy: push

faster-test:
  script: bundle exec rspec
  cache:
    key: gems
    paths: vendor/bundle
    policy: pull
```

### Disable per-job

```yaml
job:
  script: ...
  cache: []   # disable cache for this job
```

### Global cache via `default:`

```yaml
default:
  cache: &global_cache
    key: $CI_COMMIT_REF_SLUG
    paths: [vendor/]

job1:
  cache: *global_cache
```

---

## `artifacts:` (Phase 2)

```yaml
job:
  artifacts:
    paths: [binaries/]
    expire_in: 1 week
```

Sub-keys:

- `paths:` — array of file globs (relative to the repo root).
  Supports `*`, `**`, `?`, brace expansion. Uploaded when the job
  finishes (subject to `expire_in` and the job's `when:`).
- `name:` — overrides the archive name (defaults to `artifacts.zip`).
  Supports CI/CD variables, e.g.
  `name: "$CI_JOB_NAME-$CI_COMMIT_SHORT_SHA"`.
- `expire_in:` — how long the artifact is kept. Units: `s`, `sec(s)`,
  `m`, `min(s)`, `hr(s)`, `d(ay(s))`, `wk(s)`, `mo(s)`, `y(ear(s))`.
  Composite: `"2 hrs 20 min"`. `never` disables expiry.
  **Important:** does NOT affect the latest job / pipeline artifacts
  (those are kept regardless of expiry unless disabled at project
  or instance level).
- `expose_as:` — exposes the artifact in the merge-request UI.
  Up to 10 jobs per MR. Glob patterns unsupported. Single files open
  directly; directories open the artifact browser. Renders `.html`,
  `.htm`, `.txt`, `.json`, `.xml`, `.log` automatically when GitLab
  Pages is enabled.
- `reports:` — collect generated reports (`junit:`, `coverage:`,
  `codequality:`, etc.). Reports upload even on failure; use
  `expire_in:` to bound retention.
- `when:` — `on_success` (default) / `on_failure` / `always`
  (except on timeout).

### Examples

```yaml
# Binaries uploaded for downstream jobs to consume
build:
  script: cargo build --release
  artifacts:
    paths: [target/release/]
    expire_in: 1 week

# Test reports (always uploaded, even on failure)
rspec:
  script:
    - bundle install
    - rspec --format RspecJunitFormatter --out rspec.xml
  artifacts:
    paths: [coverage/]
    reports:
      junit: rspec.xml
    expire_in: 30 days

# MR-side artifact browser (up to 10 per MR)
manual-test:
  script: ./build-manual-fixture.sh
  artifacts:
    expose_as: "manual_test_fixture"
    paths:
      - fixture.bin       # file
      - reports/          # directory must end with /
```

---

## `trigger:` (Phase 3)

`trigger:` spawns a **child pipeline** (same project) or a
**multi-project pipeline** (different project) as a job outcome.

### Parent–child pipelines (same project)

```yaml
microservice_a:
  trigger:
    include: path/to/microservice_a.yml
```

Up to 3 entries (`local:` / `template:` / `artifact:` / `project:`):

```yaml
microservice_a:
  trigger:
    include:
      - local: path/to/microservice_a.yml
      - template: Security/SAST.gitlab-ci.yml
    strategy: depend
```

- `strategy: depend` — parent waits for the child to finish.
- `strategy: mirror` — parent job status mirrors the downstream
  pipeline's status.
- Default (no `strategy`) — fire-and-forget.

### Multi-project pipelines (different project)

```yaml
staging:
  stage: deploy
  trigger:
    project: my-group/deployment-project
    branch: stable
```

Requires the downstream project to grant the trigger role.

### Variable forwarding

```yaml
parent:
  trigger:
    include: child.yml
    forward:
      yaml_variables: true
      pipeline_variables: true
```

`$CI_PIPELINE_SOURCE` inside the child is set to `parent_pipeline`.

### Dynamic child pipelines (GitLab 13.12+)

Generate YAML at runtime, then trigger it:

```yaml
generator-job:
  script: ./generate-config.sh > generated-config.yml

trigger:
  include:
    artifact: generated-config.yml
    job: generator-job
```

### Depth limits

Max child-pipeline depth is **2** (since GitLab 13.4). A child
pipeline can trigger a grandchild, but no deeper.

---

## Triggering (Phase 1 push default)

By default every push triggers a run — including pushes to any
branch. The runner drops branch deletions automatically (the
receive-pack handler skips zero-SHA `new_sha`).

Use `only:` / `except:` to filter (Phase 2):

```yaml
only:
  branches: [main, release/*]
  paths:    [src/**]
except:
  paths:    [docs/**, "*.md"]
```

Glob semantics:
- `*` matches any non-`/` characters
- `**` matches across directories
- `?` matches one character
- A pattern without metachars is plain equality

Semantics:

- Empty `only:` → no branch/path constraint (every event can run).
- Non-empty `only:` → at least one of `branches` / `paths` must match.
- `except:` disqualifies a push after `only:` is checked.

You can also trigger a run through the web UI: open the Pipelines
tab on a repo and click **Run pipeline**.

---

## Reserved job-name conflicts

These names cannot be used as job keys:

```
image, services, stages, types, before_script, after_script,
variables, cache, include
```

---

## Reference tables

### Operators in `if:` expressions

| Op | Meaning |
| --- | --- |
| `==` | string equality |
| `!=` | string inequality |
| `=~` | regex match |
| `!~` | negative regex match |
| `&&` | logical AND |
| `\|\|` | logical OR |

### Time-unit strings in `expire_in:`, `timeout:`, `start_in:`

`42`, `42 seconds`, `3 mins`, `4 sec`, `2 hrs 20 min`, `2h20min`,
`6 mos`, `1 day`, `47 yrs 6 mos and 4d`, `3 weeks and 2 days`,
`never`.

### Common CI/CD variables (selected)

| Variable | Meaning |
| --- | --- |
| `CI_COMMIT_BRANCH` | The branch the commit is on. |
| `CI_COMMIT_TAG` | The tag name if the commit is a tag. |
| `CI_COMMIT_REF_NAME` | Branch or tag name. |
| `CI_COMMIT_REF_SLUG` | Lowercased, dash-replaced branch/tag for safe filenames. |
| `CI_COMMIT_SHORT_SHA` | First 8 chars of the commit SHA. |
| `CI_COMMIT_MESSAGE` | Full commit message. |
| `CI_PIPELINE_SOURCE` | `push` / `merge_request_event` / `schedule` / `web` / `parent_pipeline` / `trigger` / `api`. |
| `CI_PROJECT_DIR` | Absolute path to the repo on the runner. |
| `CI_DEFAULT_BRANCH` | The repo's default branch (usually `main`). |
| `CI_JOB_NAME` / `CI_JOB_STAGE` | Current job name and stage. |
| `CACHE_FALLBACK_KEY` | Pipeline-level cache fallback (see `cache:` above). |

---

## Three example pipelines

### 1. Single-job "does it compile"

```yaml
stages: [check]
build:
  stage: check
  script: cargo check --all-targets
```

### 2. Multi-stage matrix with `needs:` + `only:`

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

### 3. "Only rebuild docs when docs changed" — `only:` + `except:` with paths

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

---

## What's deferred (Phase 2/3)

| Phase | Keyword | What it adds |
| --- | --- | --- |
| 2 | `retry:` / `timeout:` | Retry the job on failure; wall-clock cap. |
| 2 | `parallel:` / `matrix:` | Run a job N times as a matrix. |
| 2 | `interruptible:` | Allow a job to be cancelled by a newer push. |
| 2 | `cache:` | Persist files between runs (`paths`, `key`, `policy`, `fallback_keys`). |
| 2 | `artifacts:` | Save job outputs (`paths`, `expire_in`, `expose_as`, `reports`). |
| 2 | `environment:` | Named deploy targets with on-stop hooks. |
| 2 | `rules:` | Conditional job inclusion (replaces `only/except`). |
| 3 | `image:` / `services:` | Run jobs in Docker. |
| 3 | `include:` | Split config across files. |
| 3 | `variables:` | Per-pipeline / per-job env. |
| 3 | `secrets:` | Mount external secrets. |
| 3 | `trigger:` | Spawn child / multi-project pipelines. |
| ✗ | `dast_configuration:` | Dynamic AppSec Testing; n/a for a self-hosted git server. |
| ✗ | `pages:` | GitLab Pages; n/a here. |

---

## Where to look next

- `docs/gitlab-ci-yaml-reference.md` — the keyword matrix mapped to
  gita's Phase 1/2/3 support status.
- [GitLab CI/CD YAML syntax reference][gl-yaml] — the canonical source
  this guide mirrors.

[gl-yaml]: https://docs.gitlab.com/ee/ci/yaml/