//! Public types for the GitHub-Actions-style workflow parser.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A trigger rule describing which events activate a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Trigger {
    /// Branch globs for push events (e.g. `main`, `release/*`). Empty
    /// means "all branches".
    pub branches: Vec<String>,
    /// Path globs relative to the repo root (e.g. `src/**`, `*.md`).
    pub paths: Vec<String>,
    /// Tags globs (e.g. `v*`).
    pub tags: Vec<String>,
}

/// Which event activates the workflow. `push` and `pull_request` are the
/// two we model today; the enum is open-ended so more event kinds can be
/// added without breaking existing callers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TriggerEvent {
    Push,
    PullRequest,
}

/// The `on:` section of a workflow file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TriggerConfig {
    /// Per-event trigger rules. At least one event must be present.
    pub events: IndexMap<TriggerEvent, Trigger>,
}

/// The value of `runs-on`: either a single label or an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOn {
    /// A single label, e.g. `ubuntu-latest`.
    Label(String),
    /// A list of labels, e.g. `[self-hosted, linux, x64]`.
    Labels(Vec<String>),
}

/// A `permissions:` block. Kept permissive: any GitHub permission keys
/// are accepted but not interpreted; gita does not enforce them yet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Permission {
    /// Raw permission key-value pairs from the YAML.
    pub values: BTreeMap<String, String>,
}

/// One axis of a `strategy.matrix`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixAxis {
    /// The matrix key, e.g. `os` or `node`.
    pub name: String,
    /// The values for that axis.
    pub values: Vec<serde_json::Value>,
}

/// A resolved matrix strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matrix {
    /// The matrix axes, in declaration order.
    pub axes: Vec<MatrixAxis>,
}

/// One step in a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// A human-readable name for the step.
    pub name: Option<String>,
    /// A shell command to run. Exactly one of `run` / `uses` is set.
    pub run: Option<String>,
    /// A third-party action reference, e.g. `actions/checkout@v4`.
    pub uses: Option<String>,
    /// Environment variables scoped to this step.
    pub env: BTreeMap<String, String>,
    /// Working directory relative to the job's workspace.
    pub working_directory: Option<String>,
    /// A boolean condition controlling whether this step runs.
    pub if_condition: Option<String>,
}

/// Credentials for pulling an image from a private registry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RegistryCredentials {
    pub username: String,
    pub password: String,
}

/// A service container (`jobs.<id>.services.<name>`).
///
/// GitHub starts one container per service on a per-job bridge network and
/// gives it the service's name as a network alias. `ports` use Docker's
/// `HOST:CONTAINER` spelling; `options` are passed to `docker create`
/// verbatim (that is where `--health-cmd` lives).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Service {
    /// The container image, e.g. `postgres:17`. An empty image means the
    /// service is skipped, matching GitHub's conditional-service idiom.
    pub image: String,
    /// Registry credentials, when the image is not public.
    pub credentials: Option<RegistryCredentials>,
    /// Environment variables for the container.
    pub env: BTreeMap<String, String>,
    /// `HOST:CONTAINER` port mappings, passed to `docker -p`.
    pub ports: Vec<String>,
    /// Volume mounts, passed to `docker -v` verbatim.
    pub volumes: Vec<String>,
    /// Extra `docker create` options, passed through unchanged.
    pub options: Option<String>,
    /// Overrides the image's `CMD`.
    pub command: Option<String>,
    /// Overrides the image's `ENTRYPOINT`.
    pub entrypoint: Option<String>,
}

/// The container a job's steps run in (`jobs.<id>.container`).
///
/// When present, GitHub runs the job inside this container instead of
/// directly on the runner, and services are reached by their network alias
/// rather than through published ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub image: String,
    pub credentials: Option<RegistryCredentials>,
    pub env: BTreeMap<String, String>,
    pub ports: Vec<String>,
    pub volumes: Vec<String>,
    pub options: Option<String>,
}

/// A job in a workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowJob {
    /// The job identifier (the mapping key in `jobs:`).
    pub name: String,
    /// Which runner labels this job requires.
    pub runs_on: RunOn,
    /// The steps to execute, in order.
    pub steps: Vec<Step>,
    /// The jobs this job depends on.
    pub needs: Vec<String>,
    /// Environment variables scoped to the job.
    pub env: BTreeMap<String, String>,
    /// The matrix strategy, if present.
    pub matrix: Option<Matrix>,
    /// The shell to use for `run:` steps. Defaults to `bash`.
    pub shell: Option<String>,
    /// A boolean condition controlling whether this job runs.
    pub if_condition: Option<String>,
    /// Service containers started alongside the job, keyed by the label
    /// steps use as the network hostname.
    pub services: BTreeMap<String, Service>,
    /// The container to run the job's steps in, if any.
    pub container: Option<Container>,
}

/// The top-level workflow document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    /// The workflow's human-readable name.
    pub name: Option<String>,
    /// The `on:` trigger configuration.
    pub trigger: TriggerConfig,
    /// The jobs in declaration order.
    pub jobs: IndexMap<String, WorkflowJob>,
}

/// A single resolved job instance, after matrix expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowJobInstance {
    /// The base job definition.
    pub job: WorkflowJob,
    /// The matrix combination this instance represents, keyed by axis.
    /// Empty for jobs without a matrix.
    pub matrix: BTreeMap<String, serde_json::Value>,
    /// A human-readable suffix for the instance, e.g. `(os: ubuntu, node: 20)`.
    pub matrix_label: String,
}

/// A fully resolved workflow: every job expanded into its instances, and
/// the instances ordered for execution (respecting `needs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSpec {
    pub workflow: Workflow,
    /// All job instances, in execution order.
    pub instances: Vec<WorkflowJobInstance>,
}

/// Errors produced by parsing or expanding a workflow.
#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("yaml syntax error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("workflow must have an 'on' trigger section")]
    MissingTrigger,
    #[error("workflow must declare at least one job under 'jobs'")]
    EmptyJobs,
    #[error("job '{0}' references unknown dependency '{1}'")]
    UnknownDependency(String, String),
    #[error("job '{0}' has no runnable steps")]
    EmptySteps(String),
    #[error("cycle detected in job needs graph: {0:?}")]
    Cycle(Vec<String>),
    #[error("step {0} must specify exactly one of 'run' or 'uses'")]
    StepMissingCommand(usize),
    #[error("step {0} specifies both 'run' and 'uses'")]
    StepAmbiguousCommand(usize),
    #[error("invalid expression in step {0}: {1}")]
    InvalidExpression(usize, String),
    #[error("matrix axis '{0}' has no values")]
    EmptyMatrixAxis(String),
    #[error("job '{0}' has unsupported key '{1}'")]
    UnknownJobKey(String, String),
    #[error("service label '{0}' is not a valid container network alias")]
    InvalidServiceLabel(String),
}

use thiserror::Error;
