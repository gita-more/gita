//! Pure parser for the `.gita-ci.yml` in-repo CI configuration.
//!
//! Phase 1 schema is intentionally a small subset of GitLab-CI:
//!
//! ```yaml
//! stages: [build, test]
//!
//! build:
//!   stage: build
//!   script: [cargo build --release]
//!
//! test:
//!   stage: test
//!   script: [cargo test]
//!   needs: [build]
//! ```
//!
//! Jobs may also be nested under a `jobs:` mapping, which is equivalent:
//!
//! ```yaml
//! stages: [build]
//! jobs:
//!   build:
//!     stage: build
//!     script: [cargo build --release]
//! ```
//!
//! Both spellings are accepted so a file can be written either way. The
//! reserved top-level keys are `stages`, `jobs`, `only` and `except`; every
//! other top-level mapping is treated as a job. A top-level key that is
//! *not* a job (a typo such as `stges:`, or a key belonging to a later
//! phase such as `image:`) is rejected by name rather than being
//! mistaken for a job.
//!
//! Unknown per-job keys are rejected with
//! [`PipelineParseError::UnknownField`]. Image, services, artifacts,
//! cache, and `when: manual` are explicitly out of Phase 1 and will be
//! added in later phases.
//!
//! `expand()` orders the jobs the way they run: stages in the order they
//! are declared, and within a stage the `needs` graph decides. A job may
//! not depend on a job in a later stage.

use serde::Deserialize;
use thiserror::Error;

/// One parsed pipeline file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineSpec {
    pub stages: Vec<String>,
    pub jobs: Vec<ParsedJob>,
    pub triggers: Triggers,
}

/// Top-level trigger filters. A push triggers the pipeline if
/// `only` matches (any non-empty `only` field matches) AND `except`
/// doesn't disqualify it (none of `except`'s non-empty fields match).
/// Empty filters match everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Triggers {
    pub only: TriggerRule,
    pub except: TriggerRule,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TriggerRule {
    /// Branch globs (e.g. `main`, `release/*`, `feature/**`).
    pub branches: Vec<String>,
    /// Path globs relative to the repo root (e.g. `src/**`, `*.md`).
    pub paths: Vec<String>,
}

/// Result of [`Triggers::should_run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerDecision {
    Run,
    Skip,
}

impl Triggers {
    /// Decide whether a pipeline should run for a push at `branch`
    /// that touched any of `changed_paths`.
    ///
    /// Rules:
    /// - `only`: if non-empty, **any** matching field (branch OR path)
    ///   passes; otherwise the push is skipped.
    /// - `except`: if any matching field (branch OR path) is non-empty
    ///   and matches, the push is skipped.
    pub fn should_run(&self, branch: &str, changed_paths: &[&str]) -> TriggerDecision {
        let only_active = !self.only.branches.is_empty() || !self.only.paths.is_empty();
        if only_active {
            let branch_ok = self.only.branches.iter().any(|g| glob_match(g, branch));
            let path_ok = self
                .only
                .paths
                .iter()
                .any(|g| changed_paths.iter().any(|p| glob_match(g, p)));
            // `only` requires any-field match.
            if !(branch_ok || path_ok) {
                return TriggerDecision::Skip;
            }
        }

        // `except` short-circuits if either field matches.
        if self.except.branches.iter().any(|g| glob_match(g, branch)) {
            return TriggerDecision::Skip;
        }
        if self
            .except
            .paths
            .iter()
            .any(|g| changed_paths.iter().any(|p| glob_match(g, p)))
        {
            return TriggerDecision::Skip;
        }

        TriggerDecision::Run
    }
}

/// Match `pat` (a glob) against `s`. `*` matches any non-`/` chars,
/// `?` matches one char. Falls back to plain equality when the
/// pattern has no glob metachars.
///
/// `literal_separator(true)` is what makes `*` stop at a `/`, which is
/// the convention every other CI platform follows and what the writing
/// guide documents. Without it globset's `*` also matches `/`, so
/// `release/*` would match the branch `release/a/b` — a branch the
/// author did not mean to select. `**` still crosses directories.
fn glob_match(pat: &str, s: &str) -> bool {
    if !pat.contains('*') && !pat.contains('?') && !pat.contains('[') {
        return pat == s;
    }
    globset::GlobBuilder::new(pat)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher().is_match(s))
        .unwrap_or(false)
}

/// One job parsed from the YAML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedJob {
    pub name: String,
    pub stage: String,
    pub script: Vec<String>,
    pub needs: Vec<String>,
}

/// A [`ParsedJob`] plus its resolved position in the stage DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobPlan {
    pub job: ParsedJob,
    pub stage_index: usize,
}

/// Errors that can come out of [`parse_yaml`] / [`expand`].
#[derive(Debug, Error)]
pub enum PipelineParseError {
    #[error("yaml syntax error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("unknown field: {0}")]
    UnknownField(String),
    #[error("stages list must contain at least one stage")]
    EmptyStages,
    #[error("duplicate stage name: {0}")]
    DuplicateStage(String),
    #[error("duplicate job name: {0}")]
    DuplicateJob(String),
    #[error("job '{job}' references unknown stage '{stage}' (declared: {declared:?})")]
    UnknownStage {
        job: String,
        stage: String,
        declared: Vec<String>,
    },
    #[error("job '{job}' has no stage")]
    MissingStage { job: String },
    #[error("job '{job}' references unknown dependency '{dep}'")]
    UnknownDependency { job: String, dep: String },
    #[error("job '{job}' has no script lines")]
    MissingScript { job: String },
    #[error("cycle detected in job needs graph: {0:?}")]
    Cycle(Vec<String>),
    #[error("job '{job}' needs '{dep}', which runs in a later stage ('{dep_stage}') than '{job}' ('{job_stage}')")]
    ForwardDependency {
        job: String,
        dep: String,
        job_stage: String,
        dep_stage: String,
    },
    #[error("job '{job}' is not a valid job definition: {message}")]
    InvalidJobEntry { job: String, message: String },
    #[error("unknown top-level key '{key}': it is not a job. A job is a mapping with at least one of {hints:?}; the reserved top-level keys are {reserved:?}")]
    UnknownTopLevelKey {
        key: String,
        hints: Vec<String>,
        reserved: Vec<String>,
    },
}

// ----- Raw YAML shapes --------------------------------------------------

/// The whole `.gita-ci.yml` document, flattened so that top-level job
/// entries and a `jobs:` mapping can be merged before validation.
///
/// `stages` / `jobs` / `only` / `except` are reserved; every other
/// top-level mapping is a job. Keeping the raw document around lets us
/// accept both spellings without duplicating the validation rules.
#[derive(Debug, Deserialize)]
struct RawSpec {
    #[serde(default)]
    stages: Vec<String>,
    /// The nested spelling of a job list. An `IndexMap` rather than a
    /// `BTreeMap` so the jobs keep the order they are written in — the
    /// same order the top-level spelling keeps.
    #[serde(default)]
    jobs: indexmap::IndexMap<String, RawJob>,
    #[serde(default)]
    only: Option<RawTriggerRule>,
    #[serde(default)]
    except: Option<RawTriggerRule>,
    /// Every other top-level key, captured so it can be treated as a job.
    /// The order is preserved: jobs run in the order they appear in the
    /// file, so an alphabetically-sorted map would silently change
    /// execution order.
    #[serde(flatten)]
    rest: indexmap::IndexMap<String, serde_yaml::Value>,
}

#[derive(Debug, Deserialize, Default)]
struct RawTriggerRule {
    #[serde(default)]
    branches: Vec<String>,
    #[serde(default)]
    paths: Vec<String>,
}

/// Top-level keys that configure the pipeline itself rather than a job.
const RESERVED_TOP_LEVEL_KEYS: [&str; 4] = ["stages", "jobs", "only", "except"];

/// The keys a job definition may carry. A top-level mapping with none of
/// them is a typo in a top-level keyword, not a job.
const JOB_KEYS: [&str; 3] = ["stage", "script", "needs"];

/// Whether `value` is a job definition.
///
/// A job is a *mapping* carrying at least one of the job keys. Anything
/// else at the top level — a scalar, a sequence, or a mapping without a
/// single job key — is not a job, and is almost always a mistyped
/// pipeline keyword (`stges:`, `image:`, `cache:` …) or a job written
/// as a bare command (`build: cargo build`).
fn is_a_job(value: &serde_yaml::Value) -> bool {
    let Some(mapping) = value.as_mapping() else {
        return false;
    };
    mapping
        .keys()
        .any(|k| k.as_str().map(|s| JOB_KEYS.contains(&s)).unwrap_or(false))
}

/// A job's `script:` — either a list of command lines or a single string.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawScript {
    One(String),
    Many(Vec<String>),
}

impl RawScript {
    fn into_lines(self) -> Vec<String> {
        match self {
            RawScript::One(s) => vec![s],
            RawScript::Many(v) => v,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJob {
    stage: Option<String>,
    script: Option<RawScript>,
    #[serde(default)]
    needs: Vec<String>,
}

// ----- Public API -------------------------------------------------------

pub fn parse_yaml(src: &str) -> Result<PipelineSpec, PipelineParseError> {
    let raw: RawSpec = serde_yaml::from_str(src)?;

    if raw.stages.is_empty() {
        return Err(PipelineParseError::EmptyStages);
    }

    // A repeated stage name would make "which stage is this job in?"
    // ambiguous, and `expand()` would emit the stage's jobs twice.
    let mut seen_stages = std::collections::HashSet::new();
    for stage in &raw.stages {
        if !seen_stages.insert(stage.clone()) {
            return Err(PipelineParseError::DuplicateStage(stage.clone()));
        }
    }

    // Merge the two job spellings. Top-level entries (the form the writing
    // guide documents) and the `jobs:` mapping are equivalent; a name in
    // both is a duplicate rather than a silent overwrite. Document order is
    // preserved, because jobs within a stage run in the order they appear.
    let mut raw_jobs: indexmap::IndexMap<String, RawJob> = indexmap::IndexMap::new();
    for (name, value) in raw.rest {
        // Every remaining top-level value must be a *job*. A key that is
        // not one — a typo in a pipeline keyword (`stges:`, `image:`) or a
        // job written as a bare command — is reported by name. Treating
        // it as a job used to produce "job 'stges' references unknown
        // stage ''", which points at the wrong problem entirely.
        if !is_a_job(&value) {
            return Err(PipelineParseError::UnknownTopLevelKey {
                key: name.clone(),
                hints: JOB_KEYS.iter().map(|s| s.to_string()).collect(),
                reserved: RESERVED_TOP_LEVEL_KEYS
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            });
        }
        // Each remaining top-level value must be a job mapping. A scalar or
        // sequence here is a typo, and deserializing it as a RawJob reports
        // a clear type error.
        let job: RawJob = serde_yaml::from_value(value).map_err(|e| {
            // An unknown *key* inside the job is the same class of mistake
            // as an unknown key in the nested form, so report it the same
            // way rather than as a malformed entry.
            if e.to_string().contains("unknown field") {
                PipelineParseError::UnknownField(e.to_string())
            } else {
                PipelineParseError::InvalidJobEntry {
                    job: name.clone(),
                    message: e.to_string(),
                }
            }
        })?;
        raw_jobs.insert(name, job);
    }
    for (name, job) in raw.jobs {
        if raw_jobs.insert(name.clone(), job).is_some() {
            return Err(PipelineParseError::DuplicateJob(name));
        }
    }

    let mut jobs = Vec::with_capacity(raw_jobs.len());
    let mut seen = std::collections::HashSet::new();
    for (name, raw_job) in raw_jobs {
        if !seen.insert(name.clone()) {
            return Err(PipelineParseError::DuplicateJob(name));
        }
        let stage = raw_job
            .stage
            .ok_or_else(|| PipelineParseError::MissingStage { job: name.clone() })?;
        if !raw.stages.contains(&stage) {
            return Err(PipelineParseError::UnknownStage {
                job: name.clone(),
                stage,
                declared: raw.stages.clone(),
            });
        }
        let script = raw_job
            .script
            .map(RawScript::into_lines)
            .ok_or_else(|| PipelineParseError::MissingScript { job: name.clone() })?;
        if script.is_empty() {
            return Err(PipelineParseError::MissingScript { job: name.clone() });
        }
        jobs.push(ParsedJob {
            name,
            stage,
            script,
            needs: raw_job.needs,
        });
    }

    Ok(PipelineSpec {
        stages: raw.stages,
        jobs,
        triggers: Triggers {
            only: raw
                .only
                .map(|r| TriggerRule {
                    branches: r.branches,
                    paths: r.paths,
                })
                .unwrap_or_default(),
            except: raw
                .except
                .map(|r| TriggerRule {
                    branches: r.branches,
                    paths: r.paths,
                })
                .unwrap_or_default(),
        },
    })
}

/// Order the parsed jobs the way they are actually run.
///
/// Two rules, in this order:
///
/// 1. **Stages run in the order they are declared.** A job in stage `test`
///    never starts before every job in stage `build` has finished, even if
///    the `test` job is written first in the file and has no `needs`.
/// 2. **Within a stage, `needs` decides the order.** Jobs whose
///    dependencies are all done come first; ties keep the file order.
///
/// A job may only depend on jobs in its own stage or an earlier one.
/// Depending on a *later* stage is rejected: it contradicts rule 1, and
/// the old code only noticed it as a bogus "cycle detected" error.
pub fn expand(spec: &PipelineSpec) -> Result<Vec<JobPlan>, PipelineParseError> {
    use std::collections::HashMap;

    // Index jobs by name and by stage.
    let mut by_name: HashMap<&str, &ParsedJob> = HashMap::new();
    for j in &spec.jobs {
        by_name.insert(j.name.as_str(), j);
    }
    for j in &spec.jobs {
        for dep in &j.needs {
            if !by_name.contains_key(dep.as_str()) {
                return Err(PipelineParseError::UnknownDependency {
                    job: j.name.clone(),
                    dep: dep.clone(),
                });
            }
        }
    }

    let stage_index: HashMap<&str, usize> = spec
        .stages
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();

    // A dependency on a later stage contradicts the stage order, so say so
    // plainly instead of letting the topological sort report a cycle.
    for job in &spec.jobs {
        let job_stage_idx = stage_index.get(job.stage.as_str()).copied().unwrap_or(0);
        for dep in &job.needs {
            let Some(dep_job) = by_name.get(dep.as_str()) else {
                continue;
            };
            let dep_stage_idx = stage_index
                .get(dep_job.stage.as_str())
                .copied()
                .unwrap_or(0);
            if dep_stage_idx > job_stage_idx {
                return Err(PipelineParseError::ForwardDependency {
                    job: job.name.clone(),
                    dep: dep.clone(),
                    job_stage: job.stage.clone(),
                    dep_stage: dep_job.stage.clone(),
                });
            }
        }
    }

    // Walk the stages in declaration order. Within a stage, emit the jobs
    // whose `needs` are all satisfied, keeping the file order for jobs that
    // are ready at the same time.
    let mut produced: Vec<JobPlan> = Vec::with_capacity(spec.jobs.len());
    let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (idx, stage) in spec.stages.iter().enumerate() {
        let mut remaining: Vec<&ParsedJob> =
            spec.jobs.iter().filter(|j| &j.stage == stage).collect();

        while !remaining.is_empty() {
            let mut progress = false;
            let mut i = 0;
            while i < remaining.len() {
                let job = remaining[i];
                // A dependency in an earlier stage is already done; a
                // dependency in this same stage has to be done first.
                let ready = job.needs.iter().all(|n| done.contains(n));
                if ready {
                    produced.push(JobPlan {
                        job: job.clone(),
                        stage_index: idx,
                    });
                    done.insert(job.name.clone());
                    // `remove`, not `swap_remove`: swap_remove would move
                    // the last job into this slot and shuffle the order of
                    // the jobs that follow, breaking the file-order
                    // tie-break within a stage.
                    remaining.remove(i);
                    progress = true;
                } else {
                    i += 1;
                }
            }
            if !progress {
                let names: Vec<String> = remaining.iter().map(|j| j.name.clone()).collect();
                return Err(PipelineParseError::Cycle(names));
            }
        }
    }

    // Every job belongs to a declared stage (parse_yaml checks that), so
    // nothing can be left over — but be explicit rather than silently
    // dropping a job if that ever stops being true.
    if produced.len() != spec.jobs.len() {
        let names: Vec<String> = spec
            .jobs
            .iter()
            .filter(|j| !done.contains(&j.name))
            .map(|j| j.name.clone())
            .collect();
        return Err(PipelineParseError::Cycle(names));
    }

    Ok(produced)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_happy_path_two_stages_three_jobs() {
        let yaml = r#"
stages: [build, test]
jobs:
  compile:
    stage: build
    script: [cargo build]
  unit:
    stage: test
    script: [cargo test]
    needs: [compile]
  lint:
    stage: test
    script: [cargo clippy]
    needs: [compile]
"#;
        let spec = parse_yaml(yaml).unwrap();
        assert_eq!(spec.stages, vec!["build", "test"]);
        assert_eq!(spec.jobs.len(), 3);
        // The jobs keep the order they are written in, in both spellings.
        let names: Vec<&str> = spec.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["compile", "unit", "lint"]);
    }

    #[test]
    fn parse_rejects_unknown_field() {
        let yaml = r#"
stages: [build]
jobs:
  compile:
    stage: build
    script: [cargo build]
    image: "rust:1.78"
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(matches!(err, PipelineParseError::Yaml(_)), "got: {err:?}");
    }

    /// The writing guide documents jobs as top-level keys. That form was
    /// rejected outright — the parser only understood a `jobs:` mapping — so
    /// every example in the guide, including its Quick start, produced a
    /// pipeline that never ran.
    #[test]
    fn parse_accepts_top_level_jobs() {
        let yaml = r#"
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
"#;
        let spec = parse_yaml(yaml).expect("top-level job form should parse");
        assert_eq!(spec.stages, vec!["build", "test", "deploy"]);
        let names: Vec<&str> = spec.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["build", "unit", "deploy"]);

        // The job fields carry through exactly as in the nested form.
        let unit = spec.jobs.iter().find(|j| j.name == "unit").unwrap();
        assert_eq!(unit.stage, "test");
        assert_eq!(unit.script, vec!["cargo test --lib".to_string()]);
        assert_eq!(unit.needs, vec!["build".to_string()]);
    }

    /// Both spellings may appear in one file; a name used twice is an error
    /// rather than a silent overwrite.
    #[test]
    fn parse_merges_top_level_and_nested_jobs() {
        let yaml = r#"
stages: [build, test]
build:
  stage: build
  script: [cargo build]
jobs:
  unit:
    stage: test
    script: [cargo test]
"#;
        let spec = parse_yaml(yaml).expect("mixed form should parse");
        let names: Vec<&str> = spec.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["build", "unit"]);
    }

    #[test]
    fn parse_rejects_duplicate_job_across_both_spellings() {
        let yaml = r#"
stages: [build]
build:
  stage: build
  script: [cargo build]
jobs:
  build:
    stage: build
    script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::DuplicateJob(_)),
            "got: {err:?}"
        );
    }

    /// A non-mapping at the top level is a typo, and should say so plainly
    /// rather than surfacing as an opaque YAML error.
    #[test]
    fn parse_rejects_scalar_top_level_entry() {
        let yaml = "stages: [build]\nbuild: cargo build\n";
        let err = parse_yaml(yaml).unwrap_err();
        match err {
            PipelineParseError::UnknownTopLevelKey { key, .. } => {
                assert_eq!(key, "build");
            }
            other => panic!("got: {other:?}"),
        }
    }

    /// Unknown keys inside a job are still rejected, in either spelling.
    #[test]
    fn parse_rejects_unknown_field_in_top_level_job() {
        let yaml = r#"
stages: [build]
build:
  stage: build
  script: [cargo build]
  image: "rust:1.78"
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::UnknownField(_)),
            "got: {err:?}"
        );
    }

    /// A top-level key that is not a job is reported by name. Treating it
    /// as a job used to produce "job 'stges' references unknown stage ''",
    /// which points at the wrong problem: the writer mistyped a keyword.
    #[test]
    fn parse_rejects_typo_in_top_level_keyword() {
        let yaml = r#"
stages: [build]
stges: [build]
build:
  stage: build
  script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        match err {
            PipelineParseError::UnknownTopLevelKey { key, .. } => {
                assert_eq!(key, "stges");
            }
            other => panic!("got: {other:?}"),
        }
    }

    /// A top-level key from a later phase (`image:`) is rejected by name
    /// too, rather than being mistaken for a job.
    #[test]
    fn parse_rejects_phase_two_top_level_key() {
        let yaml = r#"
stages: [build]
image: "rust:1.78"
build:
  stage: build
  script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::UnknownTopLevelKey { .. }),
            "got: {err:?}"
        );
    }

    /// A job without a `stage` says so, instead of claiming it references
    /// an unknown stage named "".
    #[test]
    fn parse_rejects_job_without_stage() {
        let yaml = r#"
stages: [build]
build:
  script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::MissingStage { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn parse_rejects_empty_stages() {
        // 'stages' omitted entirely → surface as EmptyStages.
        let yaml = "jobs:\n  x:\n    stage: a\n    script: [echo hi]\n";
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::EmptyStages),
            "got: {err:?}"
        );

        // 'stages: []' is an empty list, also EmptyStages.
        let yaml = "stages: []\njobs:\n  x:\n    stage: a\n    script: [echo hi]\n";
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::EmptyStages),
            "got: {err:?}"
        );
    }

    #[test]
    fn parse_rejects_duplicate_job_name() {
        let yaml = r#"
stages: [build]
jobs:
  build:
    stage: build
    script: [echo a]
  build:
    stage: build
    script: [echo b]
"#;
        // A duplicate job name is a mistake either way: the YAML parser may
        // reject it outright, or the map may keep only one of the two. What
        // must not happen is two jobs with the same name surviving.
        match parse_yaml(yaml) {
            Err(_) => {}
            Ok(spec) => {
                let count = spec.jobs.iter().filter(|j| j.name == "build").count();
                assert!(count <= 1, "duplicate jobs must not survive parse_yaml");
            }
        }
    }

    /// Stages are the primary ordering mechanism: a job in a later stage
    /// never starts before every job in an earlier stage has finished,
    /// even when it is written first in the file and has no `needs`.
    ///
    /// The old implementation sorted by `needs` alone, so the `test` job
    /// below ran *before* the `build` job — the stage list was decorative.
    #[test]
    fn expand_orders_by_stage_even_without_needs() {
        let spec = parse_yaml(
            r#"
stages: [build, test]
jobs:
  test:
    stage: test
    script: [echo t]
  build:
    stage: build
    script: [echo b]
"#,
        )
        .unwrap();
        let plan = expand(&spec).unwrap();
        let names: Vec<&str> = plan.iter().map(|p| p.job.name.as_str()).collect();
        assert_eq!(names, vec!["build", "test"]);
        // The stage index is the position in the declared stage list.
        assert_eq!(plan[0].stage_index, 0);
        assert_eq!(plan[1].stage_index, 1);
    }

    /// Within one stage, the file order is the tie-break, and `needs`
    /// still pulls a job ahead of the ones that depend on it.
    #[test]
    fn expand_keeps_file_order_within_a_stage() {
        let spec = parse_yaml(
            r#"
stages: [test]
jobs:
  first:
    stage: test
    script: [echo 1]
  second:
    stage: test
    script: [echo 2]
    needs: [first]
  third:
    stage: test
    script: [echo 3]
"#,
        )
        .unwrap();
        let plan = expand(&spec).unwrap();
        let names: Vec<&str> = plan.iter().map(|p| p.job.name.as_str()).collect();
        assert_eq!(names, vec!["first", "second", "third"]);
    }

    /// A job may not depend on a job in a later stage: that contradicts the
    /// stage order. The old code reported it as a "cycle", which pointed at
    /// the wrong problem.
    #[test]
    fn expand_rejects_dependency_on_a_later_stage() {
        let spec = parse_yaml(
            r#"
stages: [build, test]
jobs:
  build:
    stage: build
    script: [echo b]
    needs: [test]
  test:
    stage: test
    script: [echo t]
"#,
        )
        .unwrap();
        let err = expand(&spec).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::ForwardDependency { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn expand_orders_jobs_by_stage_then_topo() {
        let spec = parse_yaml(
            r#"
stages: [build, test, deploy]
jobs:
  build:
    stage: build
    script: [echo b]
  test1:
    stage: test
    script: [echo t1]
    needs: [build]
  test2:
    stage: test
    script: [echo t2]
    needs: [build]
  deploy:
    stage: deploy
    script: [echo d]
    needs: [test1, test2]
"#,
        )
        .unwrap();
        let plan = expand(&spec).unwrap();
        let names: Vec<&str> = plan.iter().map(|p| p.job.name.as_str()).collect();
        // build must come before test1/test2; deploy must come last.
        let i_build = names.iter().position(|n| *n == "build").unwrap();
        let i_t1 = names.iter().position(|n| *n == "test1").unwrap();
        let i_t2 = names.iter().position(|n| *n == "test2").unwrap();
        let i_deploy = names.iter().position(|n| *n == "deploy").unwrap();
        assert!(i_build < i_t1);
        assert!(i_build < i_t2);
        assert!(i_t1 < i_deploy);
        assert!(i_t2 < i_deploy);
    }

    #[test]
    fn parse_rejects_unknown_stage() {
        let yaml = r#"
stages: [build]
jobs:
  compile:
    stage: nonexistent
    script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(matches!(err, PipelineParseError::UnknownStage { .. }));
    }

    #[test]
    fn parse_rejects_missing_script() {
        let yaml = r#"
stages: [build]
jobs:
  compile:
    stage: build
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(matches!(err, PipelineParseError::MissingScript { .. }));
    }

    #[test]
    fn parse_only_branches_globs() {
        let yaml = r#"
stages: [build]
only:
  branches: [main, release/*]
jobs:
  b:
    stage: build
    script: [echo b]
"#;
        let spec = parse_yaml(yaml).unwrap();
        assert_eq!(spec.triggers.only.branches, vec!["main", "release/*"]);
        assert!(spec.triggers.only.paths.is_empty());
    }

    #[test]
    fn should_run_logic() {
        let spec_yaml = r#"
stages: [build]
only:
  branches: [main, release/*]
  paths: ["src/**"]
except:
  paths: ["docs/**"]
jobs:
  b:
    stage: build
    script: [echo b]
"#;
        let spec = parse_yaml(spec_yaml).unwrap();

        // Branch matches main → run.
        assert_eq!(
            spec.triggers.should_run("main", &["README.md"]),
            TriggerDecision::Run
        );
        // Branch matches release/1.x via glob, path matches src/** → run.
        assert_eq!(
            spec.triggers.should_run("release/1.x", &["src/lib.rs"]),
            TriggerDecision::Run
        );
        // Branch is feature/foo (no `only` branch match), path matches
        // src/** → still run because `only` is OR-of-branches-and-paths.
        assert_eq!(
            spec.triggers.should_run("feature/foo", &["src/main.rs"]),
            TriggerDecision::Run
        );

        // Branch doesn't match `only.branches`, path doesn't match
        // `only.paths` → skip.
        assert_eq!(
            spec.triggers.should_run("feature/foo", &["README.md"]),
            TriggerDecision::Skip
        );

        // Branch matches, but path matches `except.paths` → skip.
        assert_eq!(
            spec.triggers.should_run("main", &["docs/intro.md"]),
            TriggerDecision::Skip
        );
    }

    /// `*` stops at a `/`; `**` crosses directories. This is the
    /// convention every other CI platform uses, and what the writing
    /// guide documents.
    ///
    /// The matcher used to be globset's default, where `*` also matches
    /// `/` — so `release/*` selected the branch `release/a/b`, which the
    /// author never meant to select.
    #[test]
    fn glob_star_does_not_cross_a_directory_separator() {
        // Branch globs.
        assert!(glob_match("release/*", "release/1.x"));
        assert!(!glob_match("release/*", "release/1.x/rc"));
        assert!(glob_match("release/**", "release/1.x/rc"));

        // Path globs.
        assert!(glob_match("src/**", "src/lib.rs"));
        assert!(glob_match("src/**", "src/deep/nested/file.rs"));
        assert!(!glob_match("src/*", "src/deep/nested/file.rs"));
        assert!(glob_match("*.md", "README.md"));
        assert!(!glob_match("*.md", "docs/README.md"));
        assert!(glob_match("**/*.md", "docs/README.md"));

        // A pattern without metachars is plain equality.
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "mainline"));
    }

    #[test]
    fn should_run_default_when_triggers_empty() {
        // No `only` and no `except` → always run.
        let spec = parse_yaml(
            r#"
stages: [build]
jobs:
  b:
    stage: build
    script: [echo b]
"#,
        )
        .unwrap();
        for branch in ["main", "feature/x", "release/1.x"] {
            assert_eq!(
                spec.triggers.should_run(branch, &["anything.md"]),
                TriggerDecision::Run
            );
        }
    }
}
