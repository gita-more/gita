//! Public types for the `.gita-ci.yml` parser: the pipeline spec, jobs,
//! trigger filters, and the errors parse/expand can produce.

use thiserror::Error;

use crate::glob::glob_match;

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
