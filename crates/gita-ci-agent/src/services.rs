//! Service containers for a CI job.
//!
//! The implementation lives in [`gita_ci::services`], because both sides
//! need it: the in-VM agent starts services before the job script, and the
//! runner does the same for a job it executes on its own host. Re-exported
//! here so the agent's code reads as one module.

pub use gita_ci::services::*;
