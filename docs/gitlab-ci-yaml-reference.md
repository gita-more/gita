# `.gita-ci.yml` reference (and where it differs from GitLab CI)

This is the user-facing reference for writing in-repo CI configuration in
gita. It is modelled on the [GitLab CI/CD YAML syntax reference][glgitlab
yaml] and the surrounding job-keyword pages ([rules][rules],
[needs][needs], [artifacts][artifacts], [cache][cache],
[trigger][trigger]). Every GitLab CI keyword is listed below with a
short description and a `Phase` tag — **1** for what we support in the
first cut, **2/3** for what is on the roadmap, and **✗** for what
we deliberately do not plan to support.

> The full source for this list is the [GitLab CI/CD YAML syntax
> reference][glgitlab yaml]; the descriptions here are paraphrased.

[glgitlab yaml]: https://docs.gitlab.com/ee/ci/yaml/
[rules]: https://docs.gitlab.com/ee/ci/yaml/rules.html
[needs]: https://docs.gitlab.com/ee/ci/yaml/needs.html
[artifacts]: https://docs.gitlab.com/ee/ci/yaml/artifacts.html
[cache]: https://docs.gitlab.com/ee/ci/yaml/cache.html
[trigger]: https://docs.gitlab.com/ee/ci/yaml/trigger.html

## Quick example (Phase 1 subset)

```yaml
stages: [build, test, deploy]

build:
  stage: build
  script: [cargo build --release]

unit:
  stage: test
  script: [cargo test --lib]
  needs: [build]

deploy:
  stage: deploy
  script: [deploy.sh]
  needs: [unit]
```

## Global keywords

Top-level keys. The pipeline is a single `.gita-ci.yml` file at the
repo root.

| Keyword | Phase | Notes |
| --- | --- | --- |
| `default` | 3 | Per-keyword defaults applied to every job that omits the keyword. |
| `include` | 3 | Import other YAML files (local paths or remote URLs). Out of Phase 1 by design. |
| `stages` | 1 | Ordered list of stage names; jobs run stage-by-stage. |
| `variables` | 3 | Pipeline-level variables injected into every job's env. |
| `workflow` | 3 | Controls which pipelines run (`workflow: { rules: ... }`). |

> Reserved names: jobs cannot be named `image`, `services`,
> `stages`, `types`, `before_script`, `after_script`, `variables`,
> `cache`, or `include`.

## Job keywords

Every job in `jobs:` (or any top-level key — gita is flat; no `include`).

| Keyword | Phase | Notes |
| --- | --- | --- |
| `after_script` | 3 | Shell lines run after `script` (always, even on failure). |
| `allow_failure` | 3 | Job can fail without breaking the pipeline. |
| `artifacts` | 2 | Files produced by the job, persisted and downloadable. |
| `before_script` | 3 | Shell lines run before `script`. |
| `cache` | 2 | Per-job or global cache files persisted across runs. |
| `coverage` | 3 | Code-coverage threshold; fails the job if below. |
| `dast_configuration` | ✗ | Dynamic Application Security Testing config. n/a for a self-hosted git server. |
| `dependencies` | 3 | Older DAG-style predecessor spec; `needs:` supersedes it. |
| `environment` | 2 | Named deploy environment (track, on-stop, kubernetes, etc.). |
| `extends` | 3 | Reuse config from another job in the same file (YAML anchors). |
| `identity` | 3 | Token-claim for federated identity. |
| `image` | 3 | Docker image the job runs in. Phase 2 brings Docker/Podman runtime. |
| `inherit` | 3 | Per-job opt-in to global defaults/variables. |
| `interruptible` | 2 | Allow a job to be cancelled by a newer pipeline run on the same branch. |
| `manual_confirmation` | 3 | Block a job until a human clicks "Play" in the UI. |
| `needs` | 1 | DAG-style predecessor jobs (we accept only the simple `needs: [job-name, ...]` form). |
| `pages` | 3 | Upload `public/` as GitLab Pages. Not applicable here. |
| `parallel` | 2 | Number of matrix shards to run in parallel (`parallel: N`). |
| `release` | 3 | Create a git tag / release on success. |
| `resource_group` | 3 | Mutual-exclusion key so only one job runs at a time per group. |
| `retry` | 2 | Retry on failure (`max: N`, `when: ...`). |
| `rules` | 3 | Replaces `only/except`. Phase 2 may implement a strict subset. |
| `script` | 1 | The shell command(s) the job runs. Joined with `&&`. |
| `secrets` | 3 | Mount external secrets into the job. |
| `services` | 3 | Sidecar Docker containers (Postgres, Redis, etc.). |
| `stage` | 1 | Which stage this job belongs to. |
| `start_in` | 3 | Delayed-start (`start_in: 30 minutes`). |
| `tags` | 3 | Runner tag matching. |
| `timeout` | 2 | Wall-clock timeout per job (`timeout: 30 minutes`). |
| `trigger` | ✗ | Multi-project / child pipeline trigger — a different pipeline concept. |
| `variables` | 3 | Per-job variables. |
| `when` | 2 | `on_success` / `on_failure` / `always` / `manual` / `delayed`. |

### `script`

The runner executes `script` as `sh -c "<joined with ' && '>"`. Shell
metacharacters (`;`, `|`, `&&`) are interpreted by the shell. Phase 1
prepends `set -e`, so any non-zero exit stops the job.

```yaml
build:
  stage: build
  script:
    - cargo build --release
    - cp target/release/app ./bin/
```

### `stage`

Names the stage this job belongs to. Stages run in the order
declared in the top-level `stages:` list. Jobs in the same stage run
serially within the runner today (Phase 2 will add per-stage
parallelism).

### `needs`

Phase-1 syntax is the simple list form:

```yaml
deploy:
  stage: deploy
  needs: [unit, lint]   # run after both unit and lint succeed
  script: [./deploy.sh]
```

`expand()` in `gita-pipeline` does a Kahn topological sort by
`(stage_index, needs)`; jobs are emitted in an order that always runs
a job after its `needs` dependencies.

### `stages`

```yaml
stages: [build, test, deploy]
```

Required. Must contain at least one stage name. Every job's `stage`
must appear in this list or the pipeline is rejected at parse time.

## Removed keywords

GitLab CI deprecated/removed these from newer versions; we never
supported them.

| Keyword | Status |
| --- | --- |
| `only` / `except` | **Phase 1, kept for now.** Phase 2 will likely deprecate in favour of `rules`. We support the branch-glob and path-glob forms. |
| `types` | Removed in GitLab 12.0. |
| `when` values `success` / `failure` / `always` | Phase 1 has no concept of these (every job has an implicit `on_success`). |

## Reserved job-name conflicts

These names cannot be used as job keys:

```
image, services, stages, types, before_script, after_script,
variables, cache, include
```

## Concrete Phase 1 example

A reasonably complete `.gita-ci.yml` you can paste into a fresh
repo today:

```yaml
stages: [build, test, deploy]

only:
  branches: [main, release/*]

build:
  stage: build
  script:
    - cargo build --release

unit:
  stage: test
  script: [cargo test --lib]
  needs: [build]

lint:
  stage: test
  script: [cargo clippy -- -D warnings]
  needs: [build]

deploy:
  stage: deploy
  script: [./scripts/deploy.sh]
  needs: [unit, lint]
```

## Where the full reference lives

The authoritative keyword definitions are at the GitLab CI/CD YAML
syntax reference:

- [CI/CD YAML syntax reference (latest)](https://docs.gitlab.com/ee/ci/yaml/)
- [`rules:`](https://docs.gitlab.com/ee/ci/yaml/rules.html)
- [`needs:`](https://docs.gitlab.com/ee/ci/yaml/needs.html)
- [`artifacts:`](https://docs.gitlab.com/ee/ci/yaml/artifacts.html)
- [`cache:`](https://docs.gitlab.com/ee/ci/yaml/cache.html)
- [`trigger:`](https://docs.gitlab.com/ee/ci/yaml/trigger.html)

## Per-keyword deep dives (synthesised)

These summaries track the upstream GitLab docs as of Phase 1
(2026-10). Treat them as a primer — for authoritative semantics,
follow the link at the end of each section.

### `rules:` (GitLab 12.3+, replaces `only`/`except`)

`rules:` is an **array of rule objects** evaluated top-to-bottom; the
first rule that matches determines whether the job runs and with
which `when:` value.

Each rule can use:

- `if:` — a CI/CD-variable expression (the same one used by `rules:if`
  everywhere in GitLab CI).
- `changes:` — globs of file paths; the rule matches if **any** of
  the paths touched in the push satisfies a glob.
- `exists:` — globs of files that must (or must not) exist in the repo.
- `when:` — `on_success` / `on_failure` / `always` / `manual` /
  `delayed` / `never`.
- `allow_failure:`, `needs:` (override stage-level needs).

Example:

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

`changes:` supports `*` (within a path segment), `**` (across
segments), brace expansion (`*.{rb,py,sh}`), and dotfiles (`*.json`).
The glob uses Ruby `File.fnmatch` with `FNM_PATHNAME |
FNM_DOTMATCH | FNM_EXTGLOB`. Up to 50 patterns per `changes:` block.

A job is **added** to the pipeline when a matching rule has `when:
on_success`, `delayed`, or `always` (or is a `when`-only rule). A job
is **not added** when no rule matches or the matching rule is `when:
never`.

> gita roadmap: Phase 2 may implement a strict subset of `rules:`,
> starting with `if:` against pre-defined variables.

### `needs:` (DAG; replaces `dependencies:`)

`needs:` creates a **directed-acyclic-graph** pipeline where a job can
start as soon as all its named predecessors finish — without waiting
for an entire stage to complete.

- Up to 50 jobs in the array.
- `needs: []` (empty) starts the job as soon as the pipeline is
  created.
- `optional: true` lets a `needs:` predecessor be absent (e.g.
  when filtered out by another `rules:` block).
- `artifacts: true | false` controls whether the predecessor's
  artifacts are downloaded into this job's workspace. Cannot combine
  with the older `dependencies:` keyword.

Object form (Phase 2+):

```yaml
needs:
  - job: build
    artifacts: true
  - job: test
    optional: true
```

`needs: project:` (Premium/Ultimate tier in GitLab — Phase 3 in gita) pulls
artifacts from another project's latest successful pipeline. Requires
`project: <path>`, `job: <name>`, `ref: <branch>`, `artifacts: true`.

`needs: pipeline: job:` (GitLab 13.7+) crosses between parent and
child pipelines.

### `artifacts:` (Phase 2 in gita)

Sub-keys:

- `paths:` — array of file globs (relative to `$CI_PROJECT_DIR` /
  the repo root). Supports `*`, `**`, `?`, brace expansion. The
  archive is uploaded when the job finishes (subject to `expire_in`
  and the job's `when:`).
- `name:` — overrides the archive name (defaults to `artifacts`,
  downloads as `artifacts.zip`). Supports CI/CD variables, e.g.
  `name: "$CI_JOB_NAME-$CI_COMMIT_SHORT_SHA"`.
- `expire_in:` — how long the artifact is kept. Units: `s`, `sec(s)`,
  `m`, `min(s)`, `hr(s)`, `d(ay(s))`, `wk(s)`, `mo(s)`, `y(ear(s))`.
  Composite: `"2 hrs 20 min"`. `never` disables expiry (GitLab 13.3+).
- `expose_as:` — exposes the artifact in the merge-request UI
  (GitLab 12.5+). Up to 10 jobs per MR. Directories must end with `/`.
  Renders `.html` / `.txt` / `.json` / `.xml` / `.log` directly when
  GitLab Pages is enabled.
- `reports:` — collect generated reports (`junit:`, `coverage:`
  paths, etc.). Reports upload even on failure; use `expire_in:` to
  bound retention.

Example:

```yaml
rspec:
  stage: test
  script:
    - bundle install
    - rspec --format RspecJunitFormatter --out rspec.xml
  artifacts:
    paths: [coverage/]
    reports:
      junit: rspec.xml
    expire_in: 1 week
```

### `cache:` (Phase 2 in gita)

Sub-keys:

- `paths:` — array of file globs. Same glob syntax as `artifacts.paths`.
- `key:` — identifier used to share caches across pipelines. Defaults
  to `default`. Set to a per-branch / per-job key to scope caches.
- `policy:` — `pull`, `push`, or `pull-push` (default). `pull`
  downloads but doesn't upload; `push` uploads but doesn't download;
  `pull-push` does both.
- `untracked: true` — also cache any Git-untracked file in `$CI_PROJECT_DIR`.
- `when:` — `on_success` (default) / `on_failure` / `always`.

Example:

```yaml
cache:
  key: ${CI_COMMIT_REF_SLUG}
  paths:
    - node_modules/
  policy: pull-push
  untracked: true
```

### `trigger:` (Phase 3 in gita)

`trigger:` spawns a **child pipeline** (same project) or a
**multi-project pipeline** (different project) as a job outcome.

Same project:

```yaml
microservice_a:
  trigger:
    include: path/to/microservice_a.yml
```

Or with multiple entries (up to 3):

```yaml
microservice_a:
  trigger:
    include:
      - local: path/to/microservice_a.yml
      - template: Security/SAST.gitlab-ci.yml
    strategy: depend
```

`strategy: depend` makes the parent pipeline wait for the child to
finish. Add `rules: - if: $CI_MERGE_REQUEST_ID` to gate on MRs only.

Multi-project:

```yaml
staging:
  stage: deploy
  trigger:
    project: my-group/deployment-project
    branch: stable
```

`forward:` (newer) lets the child pipeline inherit variables from
the parent: `forward: { yaml_variables: true, pipeline_variables: true }`.

Dynamic child pipelines generate YAML at runtime and trigger via
`include: artifact: <file> job: <generator>` (since GitLab 13.12).

`$CI_PIPELINE_SOURCE` is set to `parent_pipeline` inside child
pipelines, useful for `rules:`.

Max child-pipeline depth is 2 (since GitLab 13.4).

We deliberately do **not** ship a vendored copy of those pages. This
document is a curated mapping from GitLab CI concepts to gita's
Phases 1–3, and a living record of which keywords work today.