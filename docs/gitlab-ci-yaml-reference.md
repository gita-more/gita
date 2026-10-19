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

We deliberately do **not** ship a vendored copy of those pages. This
document is a curated mapping from GitLab CI concepts to gita's
Phases 1–3, and a living record of which keywords work today.