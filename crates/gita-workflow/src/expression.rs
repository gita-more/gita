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

/// Evaluate a GitHub-Actions-style `if:` condition.
///
/// The condition may be written with or without the surrounding
/// `${{ ... }}`. An empty condition is treated as true. An expression
/// that cannot be parsed is treated as false, so a malformed
/// condition skips the step rather than running it.
pub fn evaluate_condition(input: &str, ctx: &EvalContext) -> bool {
    let mut s = input.trim();
    if s.is_empty() {
        return true;
    }
    if let Some(rest) = s.strip_prefix("${{") {
        s = rest.trim();
        if let Some(inner) = s.strip_suffix("}}") {
            s = inner.trim();
        }
    }
    let mut parser = CondParser {
        chars: s.chars().collect(),
        pos: 0,
        ctx,
    };
    let value = parser.parse_expr();
    truthy(&value)
}

#[derive(Clone)]
enum CValue {
    Bool(bool),
    Null,
    Number(f64),
    Str(String),
    Other(String),
}

fn truthy(v: &CValue) -> bool {
    match v {
        CValue::Bool(b) => *b,
        CValue::Null => false,
        CValue::Str(s) => !s.is_empty(),
        CValue::Number(n) => *n != 0.0,
        CValue::Other(s) => !s.is_empty() && s != "null" && s != "false",
    }
}

struct CondParser<'a> {
    chars: Vec<char>,
    pos: usize,
    ctx: &'a EvalContext,
}

impl<'a> CondParser<'a> {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn parse_expr(&mut self) -> CValue {
        self.parse_or()
    }

    fn parse_or(&mut self) -> CValue {
        let mut left = self.parse_and();
        loop {
            self.skip_ws();
            if self.peek_str("||") {
                self.pos += 2;
                let right = self.parse_and();
                let res = truthy(&left) || truthy(&right);
                left = CValue::Bool(res);
            } else {
                break;
            }
        }
        left
    }

    fn parse_and(&mut self) -> CValue {
        let mut left = self.parse_not();
        loop {
            self.skip_ws();
            if self.peek_str("&&") {
                self.pos += 2;
                let right = self.parse_not();
                let res = truthy(&left) && truthy(&right);
                left = CValue::Bool(res);
            } else {
                break;
            }
        }
        left
    }

    fn parse_not(&mut self) -> CValue {
        self.skip_ws();
        if self.pos < self.chars.len() && self.chars[self.pos] == '!' {
            self.pos += 1;
            let v = self.parse_not();
            return CValue::Bool(!truthy(&v));
        }
        self.parse_cmp()
    }

    fn parse_cmp(&mut self) -> CValue {
        let left = self.parse_primary();
        self.skip_ws();
        let op = if self.peek_str("==") {
            "=="
        } else if self.peek_str("!=") {
            "!="
        } else if self.peek_str(">=") {
            ">="
        } else if self.peek_str("<=") {
            "<="
        } else if self.pos < self.chars.len() && self.chars[self.pos] == '>' {
            ">"
        } else if self.pos < self.chars.len() && self.chars[self.pos] == '<' {
            "<"
        } else if self.peek_str("=") {
            "=="
        } else {
            return left;
        };
        // consume op
        // handle two-char ops
        let consume = match op {
            ">=" | "<=" | "==" | "!=" => 2,
            _ => 1,
        };
        self.pos += consume;
        let right = self.parse_primary();
        let res = match op {
            "==" | "=" => values_equal(&left, &right),
            "!=" => !values_equal(&left, &right),
            ">" => cmp_numbers(&left, &right, |a, b| a > b),
            "<" => cmp_numbers(&left, &right, |a, b| a < b),
            ">=" => cmp_numbers(&left, &right, |a, b| a >= b),
            "<=" => cmp_numbers(&left, &right, |a, b| a <= b),
            _ => false,
        };
        CValue::Bool(res)
    }

    fn parse_primary(&mut self) -> CValue {
        self.skip_ws();
        if self.pos >= self.chars.len() {
            return CValue::Null;
        }
        // parentheses
        if self.chars[self.pos] == '(' {
            self.pos += 1;
            let v = self.parse_expr();
            self.skip_ws();
            if self.pos < self.chars.len() && self.chars[self.pos] == ')' {
                self.pos += 1;
            }
            return v;
        }
        // string literal
        if self.chars[self.pos] == '\'' || self.chars[self.pos] == '"' {
            let quote = self.chars[self.pos];
            self.pos += 1;
            let mut out = String::new();
            while self.pos < self.chars.len() && self.chars[self.pos] != quote {
                out.push(self.chars[self.pos]);
                self.pos += 1;
            }
            if self.pos < self.chars.len() {
                self.pos += 1;
            }
            return CValue::Str(out);
        }
        // function call or identifier path
        if self.chars[self.pos].is_alphabetic() || self.chars[self.pos] == '_' {
            let start = self.pos;
            while self.pos < self.chars.len()
                && (self.chars[self.pos].is_alphanumeric() || self.chars[self.pos] == '_' || self.chars[self.pos] == '.')
            {
                self.pos += 1;
            }
            let ident = self.chars[start..self.pos].iter().collect::<String>();
            self.skip_ws();
            // function call
            if self.pos < self.chars.len() && self.chars[self.pos] == '(' {
                self.pos += 1;
                let mut args: Vec<CValue> = Vec::new();
                self.skip_ws();
                if self.pos < self.chars.len() && self.chars[self.pos] != ')' {
                    args.push(self.parse_expr());
                    loop {
                        self.skip_ws();
                        if self.pos < self.chars.len() && self.chars[self.pos] == ',' {
                            self.pos += 1;
                            args.push(self.parse_expr());
                        } else {
                            break;
                        }
                    }
                }
                self.skip_ws();
                if self.pos < self.chars.len() && self.chars[self.pos] == ')' {
                    self.pos += 1;
                }
                return self.call_function(&ident, &args);
            }
            // property path
            return self.resolve_path(&ident);
        }
        // number
        if self.chars[self.pos].is_ascii_digit() {
            let start = self.pos;
            while self.pos < self.chars.len()
                && (self.chars[self.pos].is_ascii_digit() || self.chars[self.pos] == '.')
            {
                self.pos += 1;
            }
            let num: f64 = self.chars[start..self.pos].iter().collect::<String>().parse().unwrap_or(0.0);
            return CValue::Number(num);
        }
        // unknown token: consume one char and return null
        self.pos += 1;
        CValue::Null
    }

    fn peek_str(&self, pat: &str) -> bool {
        let chars: Vec<char> = pat.chars().collect();
        if self.pos + chars.len() > self.chars.len() {
            return false;
        }
        self.chars[self.pos..self.pos + chars.len()] == chars
    }

    fn resolve_path(&self, path: &str) -> CValue {
        let parts: Vec<&str> = path.split('.').collect();
        if parts.is_empty() {
            return CValue::Null;
        }
        match parts[0] {
            "matrix" => {
                if parts.len() == 1 {
                    CValue::Other(serde_json::to_string(&self.ctx.matrix).unwrap_or_default())
                } else {
                    match self.ctx.matrix.get(parts[1]) {
                        Some(v) => json_to_cvalue(v),
                        None => CValue::Null,
                    }
                }
            }
            "env" => {
                if parts.len() == 1 {
                    CValue::Other(serde_json::to_string(&self.ctx.env).unwrap_or_default())
                } else {
                    match self.ctx.env.get(parts[1]) {
                        Some(v) => CValue::Str(v.clone()),
                        None => CValue::Null,
                    }
                }
            }
            "true" => CValue::Bool(true),
            "false" => CValue::Bool(false),
            "null" => CValue::Null,
            _ => CValue::Null,
        }
    }

    fn call_function(&self, name: &str, args: &[CValue]) -> CValue {
        match name {
            "always" | "success" => CValue::Bool(true),
            "failure" | "cancelled" => CValue::Bool(false),
            "contains" => {
                if args.len() >= 2 {
                    let hay = cvalue_to_string(&args[0]);
                    let needle = cvalue_to_string(&args[1]);
                    CValue::Bool(hay.contains(&needle))
                } else {
                    CValue::Bool(false)
                }
            }
            "startsWith" => {
                if args.len() >= 2 {
                    let s = cvalue_to_string(&args[0]);
                    let prefix = cvalue_to_string(&args[1]);
                    CValue::Bool(s.starts_with(&prefix))
                } else {
                    CValue::Bool(false)
                }
            }
            "endsWith" => {
                if args.len() >= 2 {
                    let s = cvalue_to_string(&args[0]);
                    let suffix = cvalue_to_string(&args[1]);
                    CValue::Bool(s.ends_with(&suffix))
                } else {
                    CValue::Bool(false)
                }
            }
            "toJSON" => {
                if let Some(a) = args.first() {
                    CValue::Str(cvalue_to_string(a))
                } else {
                    CValue::Null
                }
            }
            _ => CValue::Null,
        }
    }
}

fn values_equal(a: &CValue, b: &CValue) -> bool {
    match (a, b) {
        (CValue::Bool(x), CValue::Bool(y)) => x == y,
        (CValue::Null, CValue::Null) => true,
        (CValue::Number(x), CValue::Number(y)) => (x - y).abs() < f64::EPSILON,
        (CValue::Str(x), CValue::Str(y)) => x == y,
        (CValue::Str(x), CValue::Number(y)) => x.parse::<f64>().map(|n| (n - y).abs() < f64::EPSILON).unwrap_or(false),
        (CValue::Number(x), CValue::Str(y)) => y.parse::<f64>().map(|n| (n - x).abs() < f64::EPSILON).unwrap_or(false),
        _ => false,
    }
}

/// The numeric value of a comparison operand.
///
/// A boolean counts as 1 or 0 so that `true == 1` behaves the way a
/// spreadsheet would read it; anything else is 0.
fn cvalue_to_number(v: &CValue) -> f64 {
    match v {
        CValue::Number(n) => *n,
        CValue::Str(s) => s.parse().unwrap_or(0.0),
        CValue::Bool(true) => 1.0,
        CValue::Bool(false) => 0.0,
        _ => 0.0,
    }
}

fn cmp_numbers(a: &CValue, b: &CValue, f: fn(f64, f64) -> bool) -> bool {
    f(cvalue_to_number(a), cvalue_to_number(b))
}

fn cvalue_to_string(v: &CValue) -> String {
    match v {
        CValue::Str(s) => s.clone(),
        CValue::Bool(b) => b.to_string(),
        CValue::Null => String::new(),
        CValue::Number(n) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        CValue::Other(s) => s.clone(),
    }
}

fn json_to_cvalue(v: &serde_json::Value) -> CValue {
    match v {
        serde_json::Value::Bool(b) => CValue::Bool(*b),
        serde_json::Value::Null => CValue::Null,
        serde_json::Value::Number(n) => CValue::Number(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => CValue::Str(s.clone()),
        other => CValue::Other(other.to_string()),
    }
}
