//! Parsing and expansion of GitHub-Actions-style workflow files.
//!
//! A thin wrapper over [`gita_workflow`] so callers (and tests) have one
//! entry point for "workflow YAML in, ordered job instances out". Execution
//! is not here: jobs run inside VMs, see [`crate::workflow_run`].

use gita_workflow::{expand, parse_workflow, WorkflowError, WorkflowSpec};

/// Parse and expand a workflow file's content into an executable plan.
pub fn prepare_workflow(content: &str) -> Result<WorkflowSpec, WorkflowError> {
    let wf = parse_workflow(content)?;
    expand(wf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_parses_and_expands_matrix() {
        let content = r#"
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu, windows]
    steps:
      - run: echo ${{ matrix.os }}
"#;
        let spec = prepare_workflow(content).unwrap();
        assert_eq!(spec.instances.len(), 2);
        assert_eq!(spec.workflow.jobs.len(), 1);
    }

    #[test]
    fn prepare_detects_unknown_dependency() {
        let content = r#"
on: push
jobs:
  a:
    needs: missing
    runs-on: ubuntu-latest
    steps:
      - run: echo a
"#;
        let err = prepare_workflow(content).unwrap_err();
        assert!(matches!(err, WorkflowError::UnknownDependency(_, _)));
    }
}
