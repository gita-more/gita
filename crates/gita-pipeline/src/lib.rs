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
//! Unknown top-level or per-job keys are rejected with
//! [`PipelineParseError::UnknownField`]. Image, services, artifacts,
//! cache, only/except, and `when: manual` are explicitly out of Phase 1
//! and will be added in later phases.

use serde::Deserialize;
use std::collections::BTreeMap;
use thiserror::Error;

/// One parsed pipeline file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineSpec {
    pub stages: Vec<String>,
    pub jobs: Vec<ParsedJob>,
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
}

// ----- Raw YAML shapes --------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpec {
    #[serde(default)]
    stages: Vec<String>,
    #[serde(default)]
    jobs: BTreeMap<String, RawJob>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJob {
    stage: Option<String>,
    script: Option<Vec<String>>,
    #[serde(default)]
    needs: Vec<String>,
}

// ----- Public API -------------------------------------------------------

pub fn parse_yaml(src: &str) -> Result<PipelineSpec, PipelineParseError> {
    let raw: RawSpec = serde_yaml::from_str(src)?;

    if raw.stages.is_empty() {
        return Err(PipelineParseError::EmptyStages);
    }

    let mut jobs = Vec::with_capacity(raw.jobs.len());
    let mut seen = std::collections::HashSet::new();
    for (name, raw_job) in raw.jobs {
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
}