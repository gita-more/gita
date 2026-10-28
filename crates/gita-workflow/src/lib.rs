//! Pure parser for GitHub-Actions-style `.github/workflows/*.yml` used by
//! gita's in-repo CI.
//!
//! gita targets a deliberately small subset of GitHub Actions semantics so
//! that a workflow file written for GitHub can run unchanged on gita:
//!
//! ```yaml
//! name: CI
//!
//! on:
//!   push:
//!     branches: [main]
//!
//! jobs:
//!   test:
//!     runs-on: ubuntu-latest
//!     steps:
//!       - run: cargo test
//! ```
//!
//! The model mirrors GitHub's naming (`jobs`, `runs-on`, `steps`, `uses`,
//! `run`, `env`, `if`, `strategy.matrix`, `needs`) so the subset feels
//! familiar and stays a subset — anything gita does not support yet is
//! rejected explicitly rather than silently ignored.
//!
//! The crate is pure logic with no I/O, so the whole parser and its
//! expansion can be unit-tested without a server or database.

pub mod expand;
pub mod parse;
pub mod types;

pub use expand::expand;
pub use parse::parse_workflow;
pub use types::{
    Matrix, MatrixAxis, Permission, RunOn, Step, Trigger, TriggerEvent, Workflow, WorkflowError,
    WorkflowJob, WorkflowJobInstance, WorkflowSpec,
};
