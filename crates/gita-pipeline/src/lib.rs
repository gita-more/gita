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
//! Jobs may also be nested under a `jobs:` mapping, which is equivalent:
//!
//! ```yaml
//! stages: [build]
//! jobs:
//!   build:
//!     stage: build
//!     script: [cargo build --release]
//! ```
//!
//! Both spellings are accepted so a file can be written either way. The
//! reserved top-level keys are `stages`, `jobs`, `only` and `except`; every
//! other top-level mapping is treated as a job. A top-level key that is
//! *not* a job (a typo such as `stges:`, or a key belonging to a later
//! phase such as `image:`) is rejected by name rather than being
//! mistaken for a job.
//!
//! Unknown per-job keys are rejected with
//! [`PipelineParseError::UnknownField`]. Image, services, artifacts,
//! cache, and `when: manual` are explicitly out of Phase 1 and will be
//! added in later phases.
//!
//! `expand()` orders the jobs the way they run: stages in the order they
//! are declared, and within a stage the `needs` graph decides. A job may
//! not depend on a job in a later stage.

pub mod expand;
mod glob;
pub mod parse;
pub mod types;

pub use types::{
    JobPlan, ParsedJob, PipelineParseError, PipelineSpec, TriggerDecision, TriggerRule, Triggers,
};

pub use expand::expand;
pub use parse::parse_yaml;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glob::glob_match;

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
        // The jobs keep the order they are written in, in both spellings.
        let names: Vec<&str> = spec.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["compile", "unit", "lint"]);
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

    /// The writing guide documents jobs as top-level keys. That form was
    /// rejected outright — the parser only understood a `jobs:` mapping — so
    /// every example in the guide, including its Quick start, produced a
    /// pipeline that never ran.
    #[test]
    fn parse_accepts_top_level_jobs() {
        let yaml = r#"
stages: [build, test, deploy]

build:
  stage: build
  script:
    - cargo build --release

unit:
  stage: test
  script: cargo test --lib
  needs: [build]

deploy:
  stage: deploy
  script: ./scripts/deploy.sh
  needs: [unit]
"#;
        let spec = parse_yaml(yaml).expect("top-level job form should parse");
        assert_eq!(spec.stages, vec!["build", "test", "deploy"]);
        let names: Vec<&str> = spec.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["build", "unit", "deploy"]);

        // The job fields carry through exactly as in the nested form.
        let unit = spec.jobs.iter().find(|j| j.name == "unit").unwrap();
        assert_eq!(unit.stage, "test");
        assert_eq!(unit.script, vec!["cargo test --lib".to_string()]);
        assert_eq!(unit.needs, vec!["build".to_string()]);
    }

    /// Both spellings may appear in one file; a name used twice is an error
    /// rather than a silent overwrite.
    #[test]
    fn parse_merges_top_level_and_nested_jobs() {
        let yaml = r#"
stages: [build, test]
build:
  stage: build
  script: [cargo build]
jobs:
  unit:
    stage: test
    script: [cargo test]
"#;
        let spec = parse_yaml(yaml).expect("mixed form should parse");
        let names: Vec<&str> = spec.jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, vec!["build", "unit"]);
    }

    #[test]
    fn parse_rejects_duplicate_job_across_both_spellings() {
        let yaml = r#"
stages: [build]
build:
  stage: build
  script: [cargo build]
jobs:
  build:
    stage: build
    script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::DuplicateJob(_)),
            "got: {err:?}"
        );
    }

    /// A non-mapping at the top level is a typo, and should say so plainly
    /// rather than surfacing as an opaque YAML error.
    #[test]
    fn parse_rejects_scalar_top_level_entry() {
        let yaml = "stages: [build]\nbuild: cargo build\n";
        let err = parse_yaml(yaml).unwrap_err();
        match err {
            PipelineParseError::UnknownTopLevelKey { key, .. } => {
                assert_eq!(key, "build");
            }
            other => panic!("got: {other:?}"),
        }
    }

    /// Unknown keys inside a job are still rejected, in either spelling.
    #[test]
    fn parse_rejects_unknown_field_in_top_level_job() {
        let yaml = r#"
stages: [build]
build:
  stage: build
  script: [cargo build]
  image: "rust:1.78"
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::UnknownField(_)),
            "got: {err:?}"
        );
    }

    /// A top-level key that is not a job is reported by name. Treating it
    /// as a job used to produce "job 'stges' references unknown stage ''",
    /// which points at the wrong problem: the writer mistyped a keyword.
    #[test]
    fn parse_rejects_typo_in_top_level_keyword() {
        let yaml = r#"
stages: [build]
stges: [build]
build:
  stage: build
  script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        match err {
            PipelineParseError::UnknownTopLevelKey { key, .. } => {
                assert_eq!(key, "stges");
            }
            other => panic!("got: {other:?}"),
        }
    }

    /// A top-level key from a later phase (`image:`) is rejected by name
    /// too, rather than being mistaken for a job.
    #[test]
    fn parse_rejects_phase_two_top_level_key() {
        let yaml = r#"
stages: [build]
image: "rust:1.78"
build:
  stage: build
  script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::UnknownTopLevelKey { .. }),
            "got: {err:?}"
        );
    }

    /// A job without a `stage` says so, instead of claiming it references
    /// an unknown stage named "".
    #[test]
    fn parse_rejects_job_without_stage() {
        let yaml = r#"
stages: [build]
build:
  script: [cargo build]
"#;
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::MissingStage { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    fn parse_rejects_empty_stages() {
        // 'stages' omitted entirely → surface as EmptyStages.
        let yaml = "jobs:\n  x:\n    stage: a\n    script: [echo hi]\n";
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::EmptyStages),
            "got: {err:?}"
        );

        // 'stages: []' is an empty list, also EmptyStages.
        let yaml = "stages: []\njobs:\n  x:\n    stage: a\n    script: [echo hi]\n";
        let err = parse_yaml(yaml).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::EmptyStages),
            "got: {err:?}"
        );
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
        // A duplicate job name is a mistake either way: the YAML parser may
        // reject it outright, or the map may keep only one of the two. What
        // must not happen is two jobs with the same name surviving.
        match parse_yaml(yaml) {
            Err(_) => {}
            Ok(spec) => {
                let count = spec.jobs.iter().filter(|j| j.name == "build").count();
                assert!(count <= 1, "duplicate jobs must not survive parse_yaml");
            }
        }
    }

    /// Stages are the primary ordering mechanism: a job in a later stage
    /// never starts before every job in an earlier stage has finished,
    /// even when it is written first in the file and has no `needs`.
    ///
    /// The old implementation sorted by `needs` alone, so the `test` job
    /// below ran *before* the `build` job — the stage list was decorative.
    #[test]
    fn expand_orders_by_stage_even_without_needs() {
        let spec = parse_yaml(
            r#"
stages: [build, test]
jobs:
  test:
    stage: test
    script: [echo t]
  build:
    stage: build
    script: [echo b]
"#,
        )
        .unwrap();
        let plan = expand(&spec).unwrap();
        let names: Vec<&str> = plan.iter().map(|p| p.job.name.as_str()).collect();
        assert_eq!(names, vec!["build", "test"]);
        // The stage index is the position in the declared stage list.
        assert_eq!(plan[0].stage_index, 0);
        assert_eq!(plan[1].stage_index, 1);
    }

    /// Within one stage, the file order is the tie-break, and `needs`
    /// still pulls a job ahead of the ones that depend on it.
    #[test]
    fn expand_keeps_file_order_within_a_stage() {
        let spec = parse_yaml(
            r#"
stages: [test]
jobs:
  first:
    stage: test
    script: [echo 1]
  second:
    stage: test
    script: [echo 2]
    needs: [first]
  third:
    stage: test
    script: [echo 3]
"#,
        )
        .unwrap();
        let plan = expand(&spec).unwrap();
        let names: Vec<&str> = plan.iter().map(|p| p.job.name.as_str()).collect();
        assert_eq!(names, vec!["first", "second", "third"]);
    }

    /// A job may not depend on a job in a later stage: that contradicts the
    /// stage order. The old code reported it as a "cycle", which pointed at
    /// the wrong problem.
    #[test]
    fn expand_rejects_dependency_on_a_later_stage() {
        let spec = parse_yaml(
            r#"
stages: [build, test]
jobs:
  build:
    stage: build
    script: [echo b]
    needs: [test]
  test:
    stage: test
    script: [echo t]
"#,
        )
        .unwrap();
        let err = expand(&spec).unwrap_err();
        assert!(
            matches!(err, PipelineParseError::ForwardDependency { .. }),
            "got: {err:?}"
        );
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

    #[test]
    fn parse_only_branches_globs() {
        let yaml = r#"
stages: [build]
only:
  branches: [main, release/*]
jobs:
  b:
    stage: build
    script: [echo b]
"#;
        let spec = parse_yaml(yaml).unwrap();
        assert_eq!(spec.triggers.only.branches, vec!["main", "release/*"]);
        assert!(spec.triggers.only.paths.is_empty());
    }

    #[test]
    fn should_run_logic() {
        let spec_yaml = r#"
stages: [build]
only:
  branches: [main, release/*]
  paths: ["src/**"]
except:
  paths: ["docs/**"]
jobs:
  b:
    stage: build
    script: [echo b]
"#;
        let spec = parse_yaml(spec_yaml).unwrap();

        // Branch matches main → run.
        assert_eq!(
            spec.triggers.should_run("main", &["README.md"]),
            TriggerDecision::Run
        );
        // Branch matches release/1.x via glob, path matches src/** → run.
        assert_eq!(
            spec.triggers.should_run("release/1.x", &["src/lib.rs"]),
            TriggerDecision::Run
        );
        // Branch is feature/foo (no `only` branch match), path matches
        // src/** → still run because `only` is OR-of-branches-and-paths.
        assert_eq!(
            spec.triggers.should_run("feature/foo", &["src/main.rs"]),
            TriggerDecision::Run
        );

        // Branch doesn't match `only.branches`, path doesn't match
        // `only.paths` → skip.
        assert_eq!(
            spec.triggers.should_run("feature/foo", &["README.md"]),
            TriggerDecision::Skip
        );

        // Branch matches, but path matches `except.paths` → skip.
        assert_eq!(
            spec.triggers.should_run("main", &["docs/intro.md"]),
            TriggerDecision::Skip
        );
    }

    /// `*` stops at a `/`; `**` crosses directories. This is the
    /// convention every other CI platform uses, and what the writing
    /// guide documents.
    ///
    /// The matcher used to be globset's default, where `*` also matches
    /// `/` — so `release/*` selected the branch `release/a/b`, which the
    /// author never meant to select.
    #[test]
    fn glob_star_does_not_cross_a_directory_separator() {
        // Branch globs.
        assert!(glob_match("release/*", "release/1.x"));
        assert!(!glob_match("release/*", "release/1.x/rc"));
        assert!(glob_match("release/**", "release/1.x/rc"));

        // Path globs.
        assert!(glob_match("src/**", "src/lib.rs"));
        assert!(glob_match("src/**", "src/deep/nested/file.rs"));
        assert!(!glob_match("src/*", "src/deep/nested/file.rs"));
        assert!(glob_match("*.md", "README.md"));
        assert!(!glob_match("*.md", "docs/README.md"));
        assert!(glob_match("**/*.md", "docs/README.md"));

        // A pattern without metachars is plain equality.
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "mainline"));
    }

    #[test]
    fn should_run_default_when_triggers_empty() {
        // No `only` and no `except` → always run.
        let spec = parse_yaml(
            r#"
stages: [build]
jobs:
  b:
    stage: build
    script: [echo b]
"#,
        )
        .unwrap();
        for branch in ["main", "feature/x", "release/1.x"] {
            assert_eq!(
                spec.triggers.should_run(branch, &["anything.md"]),
                TriggerDecision::Run
            );
        }
    }
}
