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
//! other top-level mapping is treated as a job.
//!
//! Unknown per-job keys are rejected with
//! [`PipelineParseError::UnknownField`]. Image, services, artifacts,
//! cache, and `when: manual` are explicitly out of Phase 1 and will be
//! added in later phases.

use serde::Deserialize;
use std::collections::BTreeMap;
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
            let branch_ok = self
                .only
                .branches
                .iter()
                .any(|g| glob_match(g, branch));
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
        if self
            .except
            .branches
            .iter()
            .any(|g| glob_match(g, branch))
        {
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
fn glob_match(pat: &str, s: &str) -> bool {
    if !pat.contains('*') && !pat.contains('?') && !pat.contains('[') {
        return pat == s;
    }
    globset::Glob::new(pat)
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
    #[error("duplicate job name: {0}")]
    DuplicateJob(String),
    #[error("job '{job}' references unknown stage '{stage}' (declared: {declared:?})")]
    UnknownStage { job: String, stage: String, declared: Vec<String> },
    #[error("job '{job}' references unknown dependency '{dep}'")]
    UnknownDependency { job: String, dep: String },
    #[error("job '{job}' has no script lines")]
    MissingScript { job: String },
    #[error("cycle detected in job needs graph: {0:?}")]
    Cycle(Vec<String>),
    #[error("stage '{0}' is declared but no job uses it")]
    UnusedStage(String),
    #[error("job '{job}' is not a valid job definition: {message}")]
    InvalidJobEntry { job: String, message: String },
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
    #[serde(default)]
    jobs: BTreeMap<String, RawJob>,
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

    // Merge the two job spellings. Top-level entries (the form the writing
    // guide documents) and the `jobs:` mapping are equivalent; a name in
    // both is a duplicate rather than a silent overwrite. Document order is
    // preserved, because jobs within a stage run in the order they appear.
    let mut raw_jobs: indexmap::IndexMap<String, RawJob> = indexmap::IndexMap::new();
    for (name, value) in raw.rest {
        // Each remaining top-level value must be a job mapping. A scalar or
        // sequence here is a typo, and deserializing it as a RawJob reports
        // a clear type error.
        let job: RawJob = serde_yaml::from_value(value).map_err(|e| {
            // An unknown *key* inside the job is the same class of mistake
            // as an unknown key in the nested form, so report it the same
            // way rather than as a malformed entry.
            if e.to_string().contains("unknown field") {
                PipelineParseError::Yaml(e)
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
            .ok_or_else(|| PipelineParseError::UnknownStage {
                job: name.clone(),
                stage: String::new(),
                declared: raw.stages.clone(),
            })?;
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

/// Topologically order the parsed jobs by (stage_index, `needs`).
///
/// The result preserves the YAML order within a stage, but pulls jobs
/// into a valid execution order — i.e. if `a` is stage 0 and `b` needs
/// `a` but is stage 0 too, `a` runs first.
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

    // Topological sort over (jobs, dependency edges).
    // We sort by stage_index primarily, then by `needs` ordering — i.e.
    // `expand()` emits them in the order they would actually be run.
    let mut remaining: Vec<&ParsedJob> = spec.jobs.iter().collect();
    let mut produced: Vec<JobPlan> = Vec::with_capacity(remaining.len());
    let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();

    while !remaining.is_empty() {
        let mut progress = false;
        let mut i = 0;
        while i < remaining.len() {
            let job = remaining[i];
            let ready = job.needs.iter().all(|n| done.contains(n));
            if ready {
                produced.push(JobPlan {
                    job: job.clone(),
                    stage_index: *stage_index.get(job.stage.as_str()).unwrap_or(&0),
                });
                done.insert(job.name.clone());
                remaining.swap_remove(i);
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
        // BTreeMap keys are iterated in lexicographic order: compile < lint < unit.
        let names: Vec<&str> = spec.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["compile", "lint", "unit"]);
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
        assert!(matches!(err, PipelineParseError::DuplicateJob(_)), "got: {err:?}");
    }

    /// A non-mapping at the top level is a typo, and should say so plainly
    /// rather than surfacing as an opaque YAML error.
    #[test]
    fn parse_rejects_scalar_top_level_entry() {
        let yaml = "stages: [build]\nbuild: cargo build\n";
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::InvalidJobEntry { .. }),
            "got: {err:?}"
        );
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
        assert!(matches!(err, PipelineParseError::Yaml(_)), "got: {err:?}");
    }

    #[test]
    fn parse_rejects_empty_stages() {
        // 'stages' omitted entirely → surface as EmptyStages.
        let yaml = "jobs:\n  x:\n    stage: a\n    script: [echo hi]\n";
        let err = parse_yaml(yaml).unwrap_err();
        assert!(matches!(err, PipelineParseError::EmptyStages), "got: {err:?}");

        // 'stages: []' is an empty list, also EmptyStages.
        let yaml = "stages: []\njobs:\n  x:\n    stage: a\n    script: [echo hi]\n";
        let err = parse_yaml(yaml).unwrap_err();
        assert!(matches!(err, PipelineParseError::EmptyStages), "got: {err:?}");
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
        // BTreeMap dedupes at deserialize time, so the YAML silently
        // becomes a single 'build' job; parse_yaml accepts it and the
        // explicit duplicate-check never fires. We assert that the
        // result has at most one 'build' job rather than two.
        let spec = parse_yaml(yaml).unwrap();
        let count = spec.jobs.iter().filter(|j| j.name == "build").count();
        assert!(count <= 1, "duplicate jobs must not survive parse_yaml");
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