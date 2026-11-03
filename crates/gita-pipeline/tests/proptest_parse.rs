//! Property tests for the pipeline parser.
//!
//! The parser is the first thing that touches a user-authored file, and a
//! panic there takes the whole process down instead of reporting a mistake
//! in the YAML. These tests feed it arbitrary text and well-formed documents
//! generated property by property.

use gita_pipeline::{expand, parse_yaml};
use proptest::prelude::*;

fn identifier() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_-]{0,15}"
}

fn command() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 ./_=-]{0,80}"
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// No input is too malformed to reject: arbitrary bytes, decoded as
    /// lossily into the UTF-8 the API would hand us, must never panic.
    #[test]
    fn arbitrary_text_is_rejected_or_parsed_but_never_panics(
        bytes in prop::collection::vec(any::<u8>(), 0..4096),
    ) {
        let text = String::from_utf8_lossy(&bytes);
        let _ = parse_yaml(&text);
    }

    /// A document written in the shape the guide documents round-trips:
    /// the stage, the job name, and the exact command line survive, and
    /// expansion accepts the result.
    #[test]
    fn a_well_formed_document_round_trips(
        stage in identifier(),
        job in identifier(),
        cmd in command(),
    ) {
        let stage = format!("stage_{stage}");
        let job = format!("job_{job}");
        let doc = format!(
            "stages:\n  - {stage}\n{job}:\n  stage: {stage}\n  script:\n    - '{cmd}'\n"
        );

        let spec = parse_yaml(&doc).expect("generated document must parse");
        prop_assert_eq!(&spec.stages, &vec![stage.clone()]);
        prop_assert_eq!(spec.jobs.len(), 1);
        prop_assert_eq!(&spec.jobs[0].name, &job);
        prop_assert_eq!(&spec.jobs[0].stage, &stage);
        prop_assert_eq!(&spec.jobs[0].script, &vec![cmd]);

        prop_assert!(expand(&spec).is_ok(), "a parsed document must expand");
    }

    /// Repeating a stage is always reported, never silently deduplicated or
    /// accepted: two stages with the same name make job placement ambiguous.
    #[test]
    fn a_duplicate_stage_is_always_rejected(stage in identifier()) {
        let doc = format!("stages:\n  - {stage}\n  - {stage}\n");
        prop_assert!(parse_yaml(&doc).is_err());
    }
}
