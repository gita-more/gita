//! Trigger matching for workflows.
//!
//! A workflow's `on:` section says which events start it, and a push
//! event carries a branch and the paths that changed. This module decides
//! whether a given push activates a workflow.
//!
//! The glob conventions match gita's existing `.gita-ci.yml` filters: `*`
//! stops at a `/` while `**` crosses directory boundaries, so `release/*`
//! selects `release/1.x` but not `release/1.x/rc`.
//!
//! A filter set with no criteria matches everything. Otherwise a push
//! matches when **any** criterion matches — the same "any field passes"
//! rule the `.gita-ci.yml` filters use, so both configuration formats in
//! gita behave the same way.

use crate::types::{Trigger, TriggerEvent};
use globset::{GlobBuilder, GlobMatcher};
use std::collections::HashMap;

/// Whether a push activates a workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerDecision {
    /// The workflow should run.
    Run,
    /// The workflow should be skipped.
    Skip,
}

/// Match a glob pattern against a string.
///
/// `*` matches any non-`/` characters, `?` matches one character. Falls
/// back to plain equality when the pattern has no metacharacters.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') && !pattern.contains('[') {
        return pattern == text;
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher().is_match(text))
        .unwrap_or(false)
}

/// A compiled set of glob matchers, so a workflow's filters are not
/// recompiled for every push.
#[derive(Debug, Clone, Default)]
pub struct TriggerMatcher {
    branches: Vec<GlobMatcher>,
    paths: Vec<GlobMatcher>,
    tags: Vec<GlobMatcher>,
}

impl TriggerMatcher {
    /// Compile a trigger's filters.
    ///
    /// A pattern that is not a valid glob is skipped rather than
    /// failing the whole workflow: a typo in one filter should not stop
    /// the workflow from running.
    pub fn new(trigger: &Trigger) -> Self {
        let compile = |pats: &[String]| -> Vec<GlobMatcher> {
            pats.iter()
                .filter_map(|p| {
                    GlobBuilder::new(p)
                        .literal_separator(true)
                        .build()
                        .ok()
                        .map(|g| g.compile_matcher())
                })
                .collect()
        };
        TriggerMatcher {
            branches: compile(&trigger.branches),
            paths: compile(&trigger.paths),
            tags: compile(&trigger.tags),
        }
    }

    /// Whether a push to `branch` touching `changed_paths` matches.
    ///
    /// `branch` is the short branch name (`main`), which is what a user
    /// writes in `branches: [main]` — matching the full ref
    /// (`refs/heads/main`) would never match.
    pub fn matches_push(&self, branch: &str, changed_paths: &[&str]) -> TriggerDecision {
        // No criteria at all → every push matches.
        if self.branches.is_empty() && self.paths.is_empty() && self.tags.is_empty() {
            return TriggerDecision::Run;
        }

        // A branch filter that does not match disqualifies the push
        // outright — otherwise `branches: [main]` would also fire on
        // every feature branch whose path filter happened to match.
        if !self.branches.is_empty() && !self.branches.iter().any(|m| m.is_match(branch)) {
            return TriggerDecision::Skip;
        }

        // A path filter that does not match any changed path likewise
        // disqualifies the push.
        if !self.paths.is_empty()
            && !changed_paths
                .iter()
                .any(|p| self.paths.iter().any(|m| m.is_match(p)))
        {
            return TriggerDecision::Skip;
        }

        TriggerDecision::Run
    }
}

/// Decide whether a workflow's trigger configuration activates on a push.
///
/// Returns `Skip` when the workflow does not listen for push events at
/// all.
pub fn should_run_on_push(
    triggers: &crate::types::TriggerConfig,
    branch: &str,
    changed_paths: &[&str],
) -> TriggerDecision {
    let Some(trigger) = triggers.events.get(&TriggerEvent::Push) else {
        return TriggerDecision::Skip;
    };
    TriggerMatcher::new(trigger).matches_push(branch, changed_paths)
}

/// The identity of a trigger's filters, used as a cache key.
type FilterKey = (Vec<String>, Vec<String>, Vec<String>);

/// A cache of compiled matchers keyed by the filter values, so repeated
/// pushes do not recompile the same globs.
#[derive(Debug, Clone, Default)]
pub struct MatcherCache {
    entries: HashMap<FilterKey, TriggerMatcher>,
}

impl MatcherCache {
    /// Get (or build) the matcher for a trigger.
    pub fn get(&mut self, trigger: &Trigger) -> &TriggerMatcher {
        let key = (
            trigger.branches.clone(),
            trigger.paths.clone(),
            trigger.tags.clone(),
        );
        self.entries
            .entry(key)
            .or_insert_with(|| TriggerMatcher::new(trigger))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TriggerConfig;
    use indexmap::IndexMap;

    fn trigger(branches: &[&str], paths: &[&str]) -> Trigger {
        Trigger {
            branches: branches.iter().map(|s| s.to_string()).collect(),
            paths: paths.iter().map(|s| s.to_string()).collect(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn an_empty_filter_matches_everything() {
        let m = TriggerMatcher::new(&trigger(&[], &[]));
        assert_eq!(
            m.matches_push("main", &["anything.rs"]),
            TriggerDecision::Run
        );
    }

    #[test]
    fn a_branch_filter_selects_only_matching_branches() {
        let m = TriggerMatcher::new(&trigger(&["main"], &[]));
        assert_eq!(m.matches_push("main", &[]), TriggerDecision::Run);
        assert_eq!(m.matches_push("feature/x", &[]), TriggerDecision::Skip);
    }

    #[test]
    fn a_branch_glob_matches() {
        let m = TriggerMatcher::new(&trigger(&["release/*"], &[]));
        assert_eq!(m.matches_push("release/1.x", &[]), TriggerDecision::Run);
        assert_eq!(m.matches_push("release/1.x/rc", &[]), TriggerDecision::Skip);
    }

    #[test]
    fn a_path_filter_selects_only_matching_paths() {
        let m = TriggerMatcher::new(&trigger(&[], &["src/**"]));
        assert_eq!(
            m.matches_push("main", &["src/lib.rs"]),
            TriggerDecision::Run
        );
        assert_eq!(
            m.matches_push("main", &["README.md"]),
            TriggerDecision::Skip
        );
    }

    #[test]
    fn both_filters_must_match() {
        let m = TriggerMatcher::new(&trigger(&["main"], &["src/**"]));
        assert_eq!(
            m.matches_push("main", &["src/lib.rs"]),
            TriggerDecision::Run
        );
        // Right branch, wrong path.
        assert_eq!(
            m.matches_push("main", &["README.md"]),
            TriggerDecision::Skip
        );
        // Right path, wrong branch.
        assert_eq!(
            m.matches_push("other", &["src/lib.rs"]),
            TriggerDecision::Skip
        );
    }

    #[test]
    fn a_workflow_without_a_push_trigger_is_skipped() {
        let mut events = IndexMap::new();
        events.insert(TriggerEvent::PullRequest, trigger(&[], &[]));
        let config = TriggerConfig { events };
        assert_eq!(
            should_run_on_push(&config, "main", &[]),
            TriggerDecision::Skip
        );
    }

    #[test]
    fn a_workflow_with_a_push_trigger_is_evaluated() {
        let mut events = IndexMap::new();
        events.insert(TriggerEvent::Push, trigger(&["main"], &[]));
        let config = TriggerConfig { events };
        assert_eq!(
            should_run_on_push(&config, "main", &[]),
            TriggerDecision::Run
        );
        assert_eq!(
            should_run_on_push(&config, "other", &[]),
            TriggerDecision::Skip
        );
    }

    #[test]
    fn star_does_not_cross_a_directory_separator() {
        assert!(glob_match("release/*", "release/1.x"));
        assert!(!glob_match("release/*", "release/1.x/rc"));
        assert!(glob_match("release/**", "release/1.x/rc"));
        assert!(glob_match("src/**", "src/lib.rs"));
        assert!(glob_match("src/**", "src/deep/nested/file.rs"));
        assert!(!glob_match("src/*", "src/deep/nested/file.rs"));
        assert!(glob_match("*.md", "README.md"));
        assert!(!glob_match("*.md", "docs/README.md"));
        assert!(glob_match("**/*.md", "docs/README.md"));
    }

    #[test]
    fn a_pattern_without_metacharacters_is_plain_equality() {
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "mainline"));
    }

    #[test]
    fn the_matcher_cache_reuses_compiled_matchers() {
        let mut cache = MatcherCache::default();
        let t = trigger(&["main"], &["src/**"]);
        let a = cache.get(&t).matches_push("main", &["src/x.rs"]);
        let b = cache.get(&t).matches_push("main", &["src/x.rs"]);
        assert_eq!(a, TriggerDecision::Run);
        assert_eq!(b, TriggerDecision::Run);
    }
}
