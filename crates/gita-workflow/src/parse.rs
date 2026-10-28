//! YAML parsing and validation for `.github/workflows/*.yml`.
//!
//! We deserialize into a raw shape first (mirroring the YAML document),
//! then convert to the typed [`Workflow`]. This lets us give precise
//! errors for unknown keys and malformed structures while keeping the
//! public model clean.

use indexmap::IndexMap;
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::types::{
    Matrix, MatrixAxis, Permission, RunOn, Step, Trigger, TriggerConfig, TriggerEvent, Workflow,
    WorkflowError, WorkflowJob,
};

/// Raw mirror of the workflow YAML document.
#[derive(Debug, Deserialize)]
struct RawWorkflow {
    /// A human-readable name.
    name: Option<String>,
    /// The trigger section. Serde can't deserialize a top-level key named
    /// `on` with the normal `#[serde(rename)]`, so we capture it with a
    /// rename.
    #[serde(rename = "on")]
    on: Option<serde_json::Value>,
    /// The jobs mapping.
    jobs: Option<IndexMap<String, serde_json::Value>>,
    /// Permissions are captured but not interpreted.
    #[allow(dead_code)]
    permissions: Option<serde_json::Value>,
}

/// Parse a workflow from YAML source.
pub fn parse_workflow(src: &str) -> Result<Workflow, WorkflowError> {
    let raw: RawWorkflow = serde_yaml::from_str(src)?;

    // ---- Trigger ----
    let trigger = parse_trigger(raw.on.as_ref())?;

    // ---- Jobs ----
    let jobs = match raw.jobs {
        Some(j) => j,
        None => return Err(WorkflowError::EmptyJobs),
    };
    if jobs.is_empty() {
        return Err(WorkflowError::EmptyJobs);
    }

    let mut parsed_jobs: IndexMap<String, WorkflowJob> = IndexMap::new();
    for (name, value) in jobs {
        let job = parse_job(&name, &value)?;
        parsed_jobs.insert(name, job);
    }

    Ok(Workflow {
        name: raw.name,
        trigger,
        jobs: parsed_jobs,
    })
}

fn parse_trigger(value: Option<&serde_json::Value>) -> Result<TriggerConfig, WorkflowError> {
    let value = value.ok_or(WorkflowError::MissingTrigger)?;

    // `on: push` (a bare string) is shorthand for `on: { push: {} }`.
    if let Some(s) = value.as_str() {
        let ev = event_from_name(s).ok_or(WorkflowError::MissingTrigger)?;
        return Ok(TriggerConfig {
            events: IndexMap::from_iter([(ev, Trigger::default())]),
        });
    }

    // `on: [push, pull_request]` is a list shorthand.
    if let Some(list) = value.as_array() {
        let mut events = IndexMap::new();
        for item in list {
            let name = item.as_str().ok_or(WorkflowError::MissingTrigger)?;
            if let Some(ev) = event_from_name(name) {
                events.insert(ev, Trigger::default());
            }
        }
        if events.is_empty() {
            return Err(WorkflowError::MissingTrigger);
        }
        return Ok(TriggerConfig { events });
    }

    // `on: { push: {...}, pull_request: {...} }` — the full form.
    let map = value.as_object().ok_or(WorkflowError::MissingTrigger)?;
    let mut events = IndexMap::new();
    for (name, cfg) in map {
        let Some(ev) = event_from_name(name) else {
            // Unknown event kinds are ignored for forward compatibility.
            continue;
        };
        let trigger = parse_trigger_rule(cfg)?;
        events.insert(ev, trigger);
    }
    if events.is_empty() {
        return Err(WorkflowError::MissingTrigger);
    }
    Ok(TriggerConfig { events })
}

fn parse_trigger_rule(value: &serde_json::Value) -> Result<Trigger, WorkflowError> {
    // `on.push: null` / `{}` → default trigger.
    if value.is_null() {
        return Ok(Trigger::default());
    }
    // `on.push: branches` (bare string shorthand is not standard, but be
    // lenient: treat it as branches).
    if let Some(s) = value.as_str() {
        return Ok(Trigger {
            branches: vec![s.to_string()],
            ..Default::default()
        });
    }
    let obj = value.as_object().ok_or(WorkflowError::MissingTrigger)?;
    let get_list = |key: &str| -> Vec<String> {
        obj.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(Trigger {
        branches: get_list("branches"),
        paths: get_list("paths"),
        tags: get_list("tags"),
    })
}

fn event_from_name(name: &str) -> Option<TriggerEvent> {
    match name {
        "push" => Some(TriggerEvent::Push),
        "pull_request" => Some(TriggerEvent::PullRequest),
        _ => None,
    }
}

fn parse_job(name: &str, value: &serde_json::Value) -> Result<WorkflowJob, WorkflowError> {
    let obj = value.as_object().ok_or_else(|| {
        WorkflowError::InvalidExpression(0, format!("job '{name}' is not a mapping"))
    })?;

    let runs_on = parse_runs_on(obj.get("runs-on"))?;
    let steps = parse_steps(obj.get("steps"))?;
    if steps.is_empty() {
        return Err(WorkflowError::EmptySteps(name.to_string()));
    }

    let needs = obj
        .get("needs")
        .map(|v| {
            if let Some(s) = v.as_str() {
                vec![s.to_string()]
            } else if let Some(a) = v.as_array() {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            } else {
                Vec::new()
            }
        })
        .unwrap_or_default();

    let env = parse_string_map(obj.get("env"))?;
    let matrix = parse_matrix(obj.get("strategy"))?;
    let shell = obj
        .get("shell")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(WorkflowJob {
        name: name.to_string(),
        runs_on,
        steps,
        needs,
        env,
        matrix,
        shell,
    })
}

fn parse_runs_on(value: Option<&serde_json::Value>) -> Result<RunOn, WorkflowError> {
    let value = value
        .ok_or_else(|| WorkflowError::InvalidExpression(0, "missing 'runs-on'".to_string()))?;
    if let Some(s) = value.as_str() {
        Ok(RunOn::Label(s.to_string()))
    } else if let Some(a) = value.as_array() {
        let labels: Vec<String> = a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect();
        if labels.is_empty() {
            Ok(RunOn::Label("".to_string()))
        } else {
            Ok(RunOn::Labels(labels))
        }
    } else {
        Err(WorkflowError::InvalidExpression(
            0,
            "invalid 'runs-on'".to_string(),
        ))
    }
}

fn parse_steps(value: Option<&serde_json::Value>) -> Result<Vec<Step>, WorkflowError> {
    let value =
        value.ok_or_else(|| WorkflowError::InvalidExpression(0, "missing 'steps'".to_string()))?;
    let arr = value.as_array().ok_or_else(|| {
        WorkflowError::InvalidExpression(0, "'steps' must be a list of step mappings".to_string())
    })?;

    let mut steps = Vec::with_capacity(arr.len());
    for (idx, item) in arr.iter().enumerate() {
        let step = parse_step(idx, item)?;
        steps.push(step);
    }
    Ok(steps)
}

fn parse_step(idx: usize, value: &serde_json::Value) -> Result<Step, WorkflowError> {
    let obj = value.as_object().ok_or_else(|| {
        WorkflowError::InvalidExpression(idx, "step must be a mapping".to_string())
    })?;

    let run = obj.get("run").and_then(|v| v.as_str()).map(str::to_string);
    let uses = obj.get("uses").and_then(|v| v.as_str()).map(str::to_string);

    match (run.as_ref(), uses.as_ref()) {
        (Some(_), Some(_)) => return Err(WorkflowError::StepAmbiguousCommand(idx)),
        (None, None) => return Err(WorkflowError::StepMissingCommand(idx)),
        _ => {}
    }

    let name = obj.get("name").and_then(|v| v.as_str()).map(str::to_string);
    let env = parse_string_map(obj.get("env"))?;
    let working_directory = obj
        .get("working-directory")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let if_condition = obj.get("if").and_then(|v| v.as_str()).map(str::to_string);

    Ok(Step {
        name,
        run,
        uses,
        env,
        working_directory,
        if_condition,
    })
}

fn parse_string_map(
    value: Option<&serde_json::Value>,
) -> Result<BTreeMap<String, String>, WorkflowError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let obj = value.as_object().ok_or_else(|| {
        WorkflowError::InvalidExpression(0, "expected a mapping of string values".to_string())
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        let s = v.as_str().ok_or_else(|| {
            WorkflowError::InvalidExpression(0, format!("env key '{k}' is not a string"))
        })?;
        out.insert(k.clone(), s.to_string());
    }
    Ok(out)
}

fn parse_matrix(value: Option<&serde_json::Value>) -> Result<Option<Matrix>, WorkflowError> {
    let Some(strategy) = value else {
        return Ok(None);
    };
    let strategy_obj = strategy.as_object().ok_or_else(|| {
        WorkflowError::InvalidExpression(0, "'strategy' must be a mapping".to_string())
    })?;
    let Some(matrix) = strategy_obj.get("matrix") else {
        return Ok(None);
    };
    let matrix_obj = matrix.as_object().ok_or_else(|| {
        WorkflowError::InvalidExpression(0, "'strategy.matrix' must be a mapping".to_string())
    })?;

    let mut axes = Vec::new();
    for (k, v) in matrix_obj {
        let values = match v {
            serde_json::Value::Array(a) => a.clone(),
            serde_json::Value::String(s) => vec![serde_json::Value::String(s.clone())],
            other => vec![other.clone()],
        };
        if values.is_empty() {
            return Err(WorkflowError::EmptyMatrixAxis(k.clone()));
        }
        axes.push(MatrixAxis {
            name: k.clone(),
            values,
        });
    }
    if axes.is_empty() {
        return Ok(None);
    }
    Ok(Some(Matrix { axes }))
}

// Keep the `Permission` type referenced so the lint doesn't complain about
// an unused type in this crate's public API surface.
#[allow(dead_code)]
fn _permissions(value: Option<&serde_json::Value>) -> Permission {
    let mut values = BTreeMap::new();
    if let Some(obj) = value.and_then(|v| v.as_object()) {
        for (k, v) in obj {
            values.insert(k.clone(), v.to_string());
        }
    }
    Permission { values }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TriggerEvent;

    fn parse(src: &str) -> Result<Workflow, WorkflowError> {
        parse_workflow(src)
    }

    #[test]
    fn bare_on_push() {
        let wf = parse(
            "on: push\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n",
        )
        .unwrap();
        assert_eq!(wf.trigger.events.len(), 1);
        assert!(wf.trigger.events.contains_key(&TriggerEvent::Push));
        assert_eq!(wf.jobs.len(), 1);
    }

    #[test]
    fn full_on_mapping() {
        let wf = parse(
            r#"
on:
  push:
    branches: [main]
  pull_request:
    paths: ["src/**"]
jobs:
  t:
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"#,
        )
        .unwrap();
        assert!(wf.trigger.events.contains_key(&TriggerEvent::Push));
        assert!(wf.trigger.events.contains_key(&TriggerEvent::PullRequest));
        assert_eq!(
            wf.trigger.events[&TriggerEvent::Push].branches,
            vec!["main"]
        );
        assert_eq!(
            wf.trigger.events[&TriggerEvent::PullRequest].paths,
            vec!["src/**"]
        );
    }

    #[test]
    fn list_on_shorthand() {
        let wf = parse(
            "on: [push, pull_request]\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n",
        )
        .unwrap();
        assert!(wf.trigger.events.contains_key(&TriggerEvent::Push));
        assert!(wf.trigger.events.contains_key(&TriggerEvent::PullRequest));
    }

    #[test]
    fn missing_on_is_an_error() {
        let err =
            parse("jobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo hi\n")
                .unwrap_err();
        assert!(matches!(err, WorkflowError::MissingTrigger));
    }

    #[test]
    fn missing_jobs_is_an_error() {
        let err = parse("on: push\n").unwrap_err();
        assert!(matches!(err, WorkflowError::EmptyJobs));
    }

    #[test]
    fn step_must_have_run_or_uses() {
        let err = parse(
            "on: push\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - name: x\n",
        )
        .unwrap_err();
        assert!(matches!(err, WorkflowError::StepMissingCommand(0)));
    }

    #[test]
    fn step_cannot_have_both() {
        let err = parse(
            "on: push\njobs:\n  t:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo\n        uses: actions/checkout@v4\n",
        )
        .unwrap_err();
        assert!(matches!(err, WorkflowError::StepAmbiguousCommand(0)));
    }

    #[test]
    fn parses_steps_env_and_working_directory() {
        let wf = parse(
            r#"
on: push
jobs:
  t:
    runs-on: ubuntu-latest
    env:
      JOB_VAR: jobvalue
    steps:
      - name: build
        run: cargo build
        working-directory: web
        env:
          STEP_VAR: stepvalue
"#,
        )
        .unwrap();
        let job = &wf.jobs["t"];
        assert_eq!(job.env.get("JOB_VAR").map(String::as_str), Some("jobvalue"));
        assert_eq!(job.steps.len(), 1);
        assert_eq!(job.steps[0].name.as_deref(), Some("build"));
        assert_eq!(job.steps[0].run.as_deref(), Some("cargo build"));
        assert_eq!(job.steps[0].working_directory.as_deref(), Some("web"));
        assert_eq!(
            job.steps[0].env.get("STEP_VAR").map(String::as_str),
            Some("stepvalue")
        );
    }

    #[test]
    fn parses_needs_single_and_list() {
        let wf = parse(
            r#"
on: push
jobs:
  a:
    runs-on: ubuntu-latest
    steps: [{run: echo a}]
  b:
    needs: a
    runs-on: ubuntu-latest
    steps: [{run: echo b}]
  c:
    needs: [a, b]
    runs-on: ubuntu-latest
    steps: [{run: echo c}]
"#,
        )
        .unwrap();
        assert_eq!(wf.jobs["b"].needs, vec!["a"]);
        assert_eq!(wf.jobs["c"].needs, vec!["a", "b"]);
    }

    #[test]
    fn runs_on_array_and_single() {
        let wf = parse(
            r#"
on: push
jobs:
  a:
    runs-on: [self-hosted, linux, x64]
    steps: [{run: echo a}]
  b:
    runs-on: ubuntu-latest
    steps: [{run: echo b}]
"#,
        )
        .unwrap();
        assert_eq!(
            wf.jobs["a"].runs_on,
            RunOn::Labels(vec!["self-hosted".into(), "linux".into(), "x64".into()])
        );
        assert_eq!(wf.jobs["b"].runs_on, RunOn::Label("ubuntu-latest".into()));
    }

    #[test]
    fn matrix_parsed() {
        let wf = parse(
            r#"
on: push
jobs:
  t:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu, windows]
        node: [18, 20]
    steps: [{run: echo hi}]
"#,
        )
        .unwrap();
        let m = wf.jobs["t"].matrix.as_ref().unwrap();
        assert_eq!(m.axes.len(), 2);
        let os_axis = m.axes.iter().find(|a| a.name == "os").expect("os axis");
        assert_eq!(
            os_axis.values,
            vec![serde_json::json!("ubuntu"), serde_json::json!("windows")]
        );
        let node_axis = m.axes.iter().find(|a| a.name == "node").expect("node axis");
        assert_eq!(
            node_axis.values,
            vec![serde_json::json!(18), serde_json::json!(20)]
        );
    }
}
