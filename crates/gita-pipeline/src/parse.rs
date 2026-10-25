//! YAML parsing and validation for the `.gita-ci.yml` in-repo CI
//! configuration.
//!
//! Accepts both the top-level job spelling and a `jobs:` mapping, then
//! validates stages, jobs, triggers and produces a [`PipelineSpec`].

use serde::Deserialize;

use crate::types::{ParsedJob, PipelineParseError, PipelineSpec, TriggerRule, Triggers};

// ----- Raw YAML shapes --------------------------------------------------

/// Top-level keys that configure the pipeline itself rather than a job.
const RESERVED_TOP_LEVEL_KEYS: [&str; 4] = ["stages", "jobs", "only", "except"];

/// The keys a job definition may carry. A top-level mapping with none of
/// them is a typo in a top-level keyword, not a job.
const JOB_KEYS: [&str; 3] = ["stage", "script", "needs"];

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
