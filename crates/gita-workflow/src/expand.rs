//! Job expansion: matrix explosion and dependency ordering.
//!
//! GitHub Actions expands a job with `strategy.matrix` into one instance
//! per combination, and orders jobs topologically by `needs`. We model
//! both here.

use indexmap::IndexMap;
use std::collections::BTreeMap;

use crate::types::{
    Matrix, Workflow, WorkflowError, WorkflowJob, WorkflowJobInstance, WorkflowSpec,
};

/// Expand a parsed [`Workflow`] into a fully-resolved [`WorkflowSpec`].
///
/// Jobs without a matrix produce a single instance each. The instances
/// are returned in dependency order: every job's `needs` are satisfied
/// before it appears.
pub fn expand(workflow: Workflow) -> Result<WorkflowSpec, WorkflowError> {
    let mut instances: Vec<WorkflowJobInstance> = Vec::new();

    // Validate `needs` references before ordering: a dependency on an
    // unknown job would otherwise surface as a misleading "cycle".
    for (name, job) in &workflow.jobs {
        for dep in &job.needs {
            if !workflow.jobs.contains_key(dep) {
                return Err(WorkflowError::UnknownDependency(name.clone(), dep.clone()));
            }
        }
    }

    // Expand each job into its matrix instances.
    let mut by_name: IndexMap<String, Vec<WorkflowJobInstance>> = IndexMap::new();
    for (name, job) in &workflow.jobs {
        let job_instances = expand_job(name, job)?;
        by_name.insert(name.clone(), job_instances);
    }

    // Topologically order jobs by `needs`. We use a simple Kahn's
    // algorithm over the job names.
    let mut produced: Vec<String> = Vec::new();
    let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut remaining: Vec<&String> = workflow.jobs.keys().collect();

    while !remaining.is_empty() {
        let mut progress = false;
        let mut i = 0;
        while i < remaining.len() {
            let name = remaining[i];
            let job = &workflow.jobs[name];
            let ready = job.needs.iter().all(|n| done.contains(n));
            if ready {
                produced.push(name.clone());
                done.insert(name.clone());
                remaining.remove(i);
                progress = true;
            } else {
                i += 1;
            }
        }
        if !progress {
            let names: Vec<String> = remaining.iter().map(|n| n.to_string()).collect();
            return Err(WorkflowError::Cycle(names));
        }
    }

    // Append instances in the topological job order.
    for name in produced {
        if let Some(insts) = by_name.get(&name) {
            instances.extend(insts.iter().cloned());
        }
    }

    Ok(WorkflowSpec {
        workflow,
        instances,
    })
}

fn expand_job(name: &str, job: &WorkflowJob) -> Result<Vec<WorkflowJobInstance>, WorkflowError> {
    match &job.matrix {
        Some(matrix) => expand_matrix(name, job, matrix),
        None => Ok(vec![WorkflowJobInstance {
            job: job.clone(),
            matrix: BTreeMap::new(),
            matrix_label: String::new(),
        }]),
    }
}

fn expand_matrix(
    name: &str,
    job: &WorkflowJob,
    matrix: &Matrix,
) -> Result<Vec<WorkflowJobInstance>, WorkflowError> {
    // Build all combinations via Cartesian product.
    let mut combos: Vec<BTreeMap<String, serde_json::Value>> = vec![BTreeMap::new()];
    for axis in &matrix.axes {
        let mut next: Vec<BTreeMap<String, serde_json::Value>> = Vec::new();
        for combo in &combos {
            for value in &axis.values {
                let mut c = combo.clone();
                c.insert(axis.name.clone(), value.clone());
                next.push(c);
            }
        }
        combos = next;
    }

    let instances = combos
        .into_iter()
        .map(|combo| {
            let label = matrix_label(&combo);
            WorkflowJobInstance {
                job: job.clone(),
                matrix: combo,
                matrix_label: label,
            }
        })
        .collect();
    let _ = name;
    Ok(instances)
}

/// Build a human-readable matrix label like `(os: ubuntu, node: 20)`.
fn matrix_label(combo: &BTreeMap<String, serde_json::Value>) -> String {
    let parts: Vec<String> = combo
        .iter()
        .map(|(k, v)| format!("{k}: {}", serde_json::to_string(v).unwrap_or_default()))
        .collect();
    if parts.is_empty() {
        String::new()
    } else {
        format!("({})", parts.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_workflow;

    fn parse(src: &str) -> Workflow {
        parse_workflow(src).expect("valid yaml")
    }

    #[test]
    fn no_matrix_produces_one_instance() {
        let wf = parse(
            r#"
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"#,
        );
        let spec = expand(wf).unwrap();
        assert_eq!(spec.instances.len(), 1);
        assert!(spec.instances[0].matrix.is_empty());
        assert_eq!(spec.instances[0].matrix_label, "");
    }

    #[test]
    fn single_axis_matrix_expands() {
        let wf = parse(
            r#"
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu, windows]
    steps:
      - run: echo ${{ matrix.os }}
"#,
        );
        let spec = expand(wf).unwrap();
        assert_eq!(spec.instances.len(), 2);
        assert_eq!(
            spec.instances[0].matrix.get("os"),
            Some(&serde_json::json!("ubuntu"))
        );
        assert_eq!(
            spec.instances[1].matrix.get("os"),
            Some(&serde_json::json!("windows"))
        );
        assert_eq!(spec.instances[0].matrix_label, "(os: \"ubuntu\")");
    }

    #[test]
    fn multi_axis_matrix_cartesian() {
        let wf = parse(
            r#"
on: push
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu, windows]
        node: [18, 20]
    steps:
      - run: echo hi
"#,
        );
        let spec = expand(wf).unwrap();
        assert_eq!(spec.instances.len(), 4);
        let combos: Vec<_> = spec
            .instances
            .iter()
            .map(|i| (i.matrix.get("os").cloned(), i.matrix.get("node").cloned()))
            .collect();
        assert!(combos.contains(&(
            Some(serde_json::json!("ubuntu")),
            Some(serde_json::json!(18))
        )));
        assert!(combos.contains(&(
            Some(serde_json::json!("ubuntu")),
            Some(serde_json::json!(20))
        )));
        assert!(combos.contains(&(
            Some(serde_json::json!("windows")),
            Some(serde_json::json!(18))
        )));
        assert!(combos.contains(&(
            Some(serde_json::json!("windows")),
            Some(serde_json::json!(20))
        )));
    }

    #[test]
    fn needs_orders_jobs() {
        let wf = parse(
            r#"
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - run: echo build
  test:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - run: echo test
  deploy:
    needs: [build, test]
    runs-on: ubuntu-latest
    steps:
      - run: echo deploy
"#,
        );
        let spec = expand(wf).unwrap();
        let names: Vec<&str> = spec.instances.iter().map(|i| i.job.name.as_str()).collect();
        assert_eq!(names, vec!["build", "test", "deploy"]);
    }

    #[test]
    fn cycle_detected() {
        let wf = parse(
            r#"
on: push
jobs:
  a:
    needs: b
    runs-on: ubuntu-latest
    steps:
      - run: echo a
  b:
    needs: a
    runs-on: ubuntu-latest
    steps:
      - run: echo b
"#,
        );
        let err = expand(wf).unwrap_err();
        assert!(matches!(err, WorkflowError::Cycle(_)));
    }

    #[test]
    fn unknown_dependency_detected() {
        let wf = parse(
            r#"
on: push
jobs:
  a:
    needs: missing
    runs-on: ubuntu-latest
    steps:
      - run: echo a
"#,
        );
        let err = expand(wf).unwrap_err();
        assert!(matches!(err, WorkflowError::UnknownDependency(_, _)));
    }
}
