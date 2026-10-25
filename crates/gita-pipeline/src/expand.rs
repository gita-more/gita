//! Job ordering: stages in declaration order, then the `needs` graph
//! within a stage.

use crate::types::{JobPlan, ParsedJob, PipelineParseError, PipelineSpec};

/// Order the parsed jobs the way they are actually run.
///
/// Two rules, in this order:
///
/// 1. **Stages run in the order they are declared.** A job in stage `test`
///    never starts before every job in stage `build` has finished, even if
///    the `test` job is written first in the file and has no `needs`.
/// 2. **Within a stage, `needs` decides the order.** Jobs whose
///    dependencies are all done come first; ties keep the file order.
///
/// A job may only depend on jobs in its own stage or an earlier one.
/// Depending on a *later* stage is rejected: it contradicts rule 1, and
/// the old code only noticed it as a bogus "cycle detected" error.
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

    // A dependency on a later stage contradicts the stage order, so say so
    // plainly instead of letting the topological sort report a cycle.
    for job in &spec.jobs {
        let job_stage_idx = stage_index.get(job.stage.as_str()).copied().unwrap_or(0);
        for dep in &job.needs {
            let Some(dep_job) = by_name.get(dep.as_str()) else {
                continue;
            };
            let dep_stage_idx = stage_index
                .get(dep_job.stage.as_str())
                .copied()
                .unwrap_or(0);
            if dep_stage_idx > job_stage_idx {
                return Err(PipelineParseError::ForwardDependency {
                    job: job.name.clone(),
                    dep: dep.clone(),
                    job_stage: job.stage.clone(),
                    dep_stage: dep_job.stage.clone(),
                });
            }
        }
    }

    // Walk the stages in declaration order. Within a stage, emit the jobs
    // whose `needs` are all satisfied, keeping the file order for jobs that
    // are ready at the same time.
    let mut produced: Vec<JobPlan> = Vec::with_capacity(spec.jobs.len());
    let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (idx, stage) in spec.stages.iter().enumerate() {
        let mut remaining: Vec<&ParsedJob> =
            spec.jobs.iter().filter(|j| &j.stage == stage).collect();

        while !remaining.is_empty() {
            let mut progress = false;
            let mut i = 0;
            while i < remaining.len() {
                let job = remaining[i];
                // A dependency in an earlier stage is already done; a
                // dependency in this same stage has to be done first.
                let ready = job.needs.iter().all(|n| done.contains(n));
                if ready {
                    produced.push(JobPlan {
                        job: job.clone(),
                        stage_index: idx,
                    });
                    done.insert(job.name.clone());
                    // `remove`, not `swap_remove`: swap_remove would move
                    // the last job into this slot and shuffle the order of
                    // the jobs that follow, breaking the file-order
                    // tie-break within a stage.
                    remaining.remove(i);
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
    }

    // Every job belongs to a declared stage (parse_yaml checks that), so
    // nothing can be left over — but be explicit rather than silently
    // dropping a job if that ever stops being true.
    if produced.len() != spec.jobs.len() {
        let names: Vec<String> = spec
            .jobs
            .iter()
            .filter(|j| !done.contains(&j.name))
            .map(|j| j.name.clone())
            .collect();
        return Err(PipelineParseError::Cycle(names));
    }

    Ok(produced)
}
