//! Expression evaluation for workflow commands.
//!
//! GitHub Actions substitutes `${{ ... }}` expressions in a step's
//! `run:` script before handing it to the shell. Without that
//! substitution the raw `${{ matrix.os }}` reaches bash, where `${{` is a
//! syntax error — so a matrix job would fail on its very first step.
//!
//! This module implements the subset gita needs today:
//!
//! * `matrix.<key>` — the current matrix combination's value.
//! * `env.<NAME>` — an environment variable in scope for the step.
//!
//! An unknown expression is left untouched rather than replaced with an
//! empty string: a silently-blank substitution would run the wrong
//! command, which is harder to notice than a visible placeholder.

use serde_json::Value;
use std::collections::BTreeMap;

/// The values an expression can refer to.
#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    /// The current matrix combination.
    pub matrix: BTreeMap<String, Value>,
    /// Environment variables in scope for the step.
    pub env: BTreeMap<String, String>,
}

/// The outcome of evaluating one expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evaluated {
    /// The expression resolved to a value.
    Value(String),
    /// The expression could not be resolved.
    Unknown(String),
}

/// Substitute every `${{ ... }}` expression in `input`.
///
/// Whole-string expressions are replaced by the bare value (`${{ matrix.os }}`
/// → `linux`); expressions embedded in a larger string are replaced by
/// their string form. Unknown expressions are left in place.
pub fn evaluate(input: &str, ctx: &EvalContext) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else {
            // Unterminated expression — emit the remainder verbatim.
            out.push_str(&rest[start..]);
            return out;
        };
        let expr = after[..end].trim();
        match resolve(expr, ctx) {
            Evaluated::Value(v) => out.push_str(&v),
            Evaluated::Unknown(_) => {
                // Keep the original text so the problem is visible.
                out.push_str(&rest[start..start + 3 + end + 2]);
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Resolve a single expression body (the text between `${{` and `}}`).
pub fn resolve(expr: &str, ctx: &EvalContext) -> Evaluated {
    let expr = expr.trim();
    if let Some(key) = expr.strip_prefix("matrix.") {
        return match ctx.matrix.get(key) {
            Some(v) => Evaluated::Value(value_to_string(v)),
            None => Evaluated::Unknown(expr.to_string()),
        };
    }
    if let Some(name) = expr.strip_prefix("env.") {
        return match ctx.env.get(name) {
            Some(v) => Evaluated::Value(v.clone()),
            None => Evaluated::Unknown(expr.to_string()),
        };
    }
    // `matrix` or `env` on their own render as JSON, like GitHub does.
    if expr == "matrix" {
        return Evaluated::Value(
            serde_json::to_string(&ctx.matrix).unwrap_or_else(|_| "{}".to_string()),
        );
    }
    if expr == "env" {
        return Evaluated::Value(
            serde_json::to_string(&ctx.env).unwrap_or_else(|_| "{}".to_string()),
        );
    }
    Evaluated::Unknown(expr.to_string())
}

/// Render a JSON value the way GitHub renders it in a string context.
fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> EvalContext {
        let mut matrix = BTreeMap::new();
        matrix.insert("os".to_string(), Value::String("ubuntu".to_string()));
        matrix.insert("node".to_string(), Value::from(20));
        let mut env = BTreeMap::new();
        env.insert("HOME".to_string(), "/home/ci".to_string());
        EvalContext { matrix, env }
    }

    #[test]
    fn a_whole_string_expression_yields_the_bare_value() {
        assert_eq!(evaluate("${{ matrix.os }}", &ctx()), "ubuntu");
        assert_eq!(evaluate("${{ matrix.node }}", &ctx()), "20");
    }

    #[test]
    fn an_embedded_expression_is_substituted_in_place() {
        assert_eq!(
            evaluate("echo running on ${{ matrix.os }} now", &ctx()),
            "echo running on ubuntu now"
        );
    }

    #[test]
    fn several_expressions_in_one_string_are_all_substituted() {
        assert_eq!(
            evaluate("${{ matrix.os }}-${{ matrix.node }}", &ctx()),
            "ubuntu-20"
        );
    }

    #[test]
    fn env_expressions_are_resolved() {
        assert_eq!(evaluate("${{ env.HOME }}", &ctx()), "/home/ci");
    }

    #[test]
    fn an_unknown_expression_is_left_in_place() {
        // Leaving it visible is deliberate: an empty substitution would
        // silently run a different command.
        assert_eq!(evaluate("${{ matrix.nope }}", &ctx()), "${{ matrix.nope }}");
        assert_eq!(
            evaluate("value=${{ secrets.TOKEN }}", &ctx()),
            "value=${{ secrets.TOKEN }}"
        );
    }

    #[test]
    fn a_string_without_expressions_is_unchanged() {
        assert_eq!(
            evaluate("cargo test --workspace", &ctx()),
            "cargo test --workspace"
        );
    }

    #[test]
    fn an_unterminated_expression_is_left_alone() {
        assert_eq!(evaluate("echo ${{ matrix.os", &ctx()), "echo ${{ matrix.os");
    }

    #[test]
    fn whitespace_inside_the_expression_is_tolerated() {
        assert_eq!(evaluate("${{  matrix.os  }}", &ctx()), "ubuntu");
    }

    #[test]
    fn the_matrix_object_renders_as_json() {
        let rendered = evaluate("${{ matrix }}", &ctx());
        assert!(rendered.contains("ubuntu"), "rendered: {rendered}");
    }

    #[test]
    fn resolving_an_unknown_expression_is_reported() {
        assert_eq!(
            resolve("matrix.nope", &ctx()),
            Evaluated::Unknown("matrix.nope".to_string())
        );
        assert_eq!(
            resolve("matrix.os", &ctx()),
            Evaluated::Value("ubuntu".to_string())
        );
    }
}
