//! Contract tests: real responses must match the OpenAPI document.
//!
//! The docs tests already compare the spec's paths and methods against the
//! router. That catches a route that was never documented, but not a field
//! that was renamed, a type that changed from integer to string, or a
//! nested object that lost a property. Those drifts pass both the Rust
//! integration tests and the web tests because the web tests mock the API
//! module.
//!
//! These tests run representative requests and validate the JSON they get
//! against the schema the server publishes.

mod common;

use axum::http::Method;
use gita_api::docs::OPENAPI_JSON;
use serde_json::Value;

fn spec() -> Value {
    serde_json::from_str(OPENAPI_JSON).expect("the OpenAPI document must be valid JSON")
}

/// Validate a JSON value against an OpenAPI 3.0 schema fragment.
///
/// Supports the subset the document uses: `$ref`, `type`, `properties`,
/// `required`, `items`, `enum`, `nullable`, `allOf`, `anyOf` and `oneOf`.
/// It returns a dotted path in the error so a failure names the field that
/// drifted (`user.locale`, `0.full_name`, …).
fn validate(value: &Value, schema: &Value, root: &Value, path: &str) -> Result<(), String> {
    if value.is_null() && schema.get("nullable").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }

    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| format!("{path}: unsupported $ref {reference}"))?;
        let target = root
            .pointer(pointer)
            .ok_or_else(|| format!("{path}: dangling $ref {reference}"))?;
        return validate(value, target, root, path);
    }

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        for part in all_of {
            validate(value, part, root, path)?;
        }
    }

    for combinator in ["anyOf", "oneOf"] {
        if let Some(options) = schema.get(combinator).and_then(Value::as_array) {
            let mut errors = Vec::new();
            let matched = options
                .iter()
                .any(|option| match validate(value, option, root, path) {
                    Ok(()) => true,
                    Err(error) => {
                        errors.push(error);
                        false
                    }
                });
            if !matched {
                return Err(format!(
                    "{path}: matches none of {combinator}: {}",
                    errors.join("; ")
                ));
            }
        }
    }

    if let Some(expected) = schema.get("type") {
        let matches = match expected.as_str() {
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            Some("string") => value.is_string(),
            Some("integer") => value.is_i64() || value.is_u64(),
            Some("number") => value.is_number(),
            Some("boolean") => value.is_boolean(),
            Some("null") => value.is_null(),
            Some(other) => return Err(format!("{path}: unsupported type {other}")),
            None => true,
        };
        if !matches {
            return Err(format!("{path}: expected {}, got {}", expected, value));
        }
    }

    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if !values.contains(value) {
            return Err(format!("{path}: {value} is not one of {values:?}"));
        }
    }

    if let Some(object) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for field in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(field) {
                    return Err(format!("{path}: missing required field `{field}`"));
                }
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (field, property_schema) in properties {
                if let Some(field_value) = object.get(field) {
                    let child = if path.is_empty() {
                        field.clone()
                    } else {
                        format!("{path}.{field}")
                    };
                    validate(field_value, property_schema, root, &child)?;
                }
            }
        }
    }

    if let Some(items) = schema.get("items") {
        if let Some(array) = value.as_array() {
            for (index, item) in array.iter().enumerate() {
                validate(item, items, root, &format!("{path}[{index}]"))?;
            }
        }
    }

    Ok(())
}

/// Find the response schema the spec publishes for `method` on `path`.
///
/// The path in the spec is a *template* — `/api/repos/{owner}/{repo}` —
/// while the path a request carries is concrete
/// (`/api/repos/alice/widgets`). Looking the concrete path up verbatim
/// therefore finds nothing, and every endpoint with a path parameter
/// would be unreachable by this test.
///
/// So the lookup matches the concrete path against the templates:
///
/// - a literal segment must be equal;
/// - a `{parameter}` segment matches exactly one segment;
/// - a `{*parameter}` segment (axum's catch-all) matches one or more
///   trailing segments.
fn response_schema<'a>(spec: &'a Value, path: &str, method: &str) -> &'a Value {
    let paths = spec["paths"]
        .as_object()
        .expect("paths should be an object");
    let wanted: Vec<&str> = path.split('/').collect();

    // Several templates can match one concrete path: `/api/repos/{*path}`
    // matches everything under `/api/repos/`, and so does
    // `/api/repos/{owner}/{repo}/prs`. The most specific one is the
    // right answer — it is the one the server's own dispatcher picks,
    // and the only one that describes the response.
    let mut best: Option<(&str, usize)> = None;
    for (candidate, _) in paths {
        let Some(literals) = path_matches_template(candidate, &wanted) else {
            continue;
        };
        // More literal segments means a more specific template.
        if best.map(|(_, score)| literals > score).unwrap_or(true) {
            best = Some((candidate, literals));
        }
    }

    let (template, _) = best.unwrap_or_else(|| {
        panic!("{method} {path} is not documented in the OpenAPI document at all")
    });

    // A JSON pointer treats `/` as a separator, and the OpenAPI path keys
    // themselves contain slashes. Escape the key before interpolating it.
    let key = template.replace('~', "~0").replace('/', "~1");
    spec.pointer(&format!(
        "/paths/{key}/{method}/responses/200/content/application~1json/schema"
    ))
    .unwrap_or_else(|| {
        panic!("{method} {path} must document a 200 JSON response schema in the OpenAPI document")
    })
}

/// Whether the concrete path segments `wanted` fit the template path
/// `template`.
///
/// Returns the number of literal (non-parameter) segments in the
/// template, which is how specific the match is.
fn path_matches_template(template: &str, wanted: &[&str]) -> Option<usize> {
    let template_segments: Vec<&str> = template.split('/').collect();
    let mut literals = 0usize;
    let mut t = 0usize;
    let mut w = 0usize;

    while t < template_segments.len() {
        let segment = template_segments[t];
        if segment.starts_with("{*") && segment.ends_with('}') {
            // A catch-all takes every remaining segment, and there must
            // be at least one.
            if wanted.len() - w == 0 {
                return None;
            }
            return Some(literals);
        }
        if w >= wanted.len() {
            return None;
        }
        if segment.starts_with('{') && segment.ends_with('}') {
            // A named parameter takes exactly one segment.
        } else {
            if segment != wanted[w] {
                return None;
            }
            literals += 1;
        }
        t += 1;
        w += 1;
    }

    if w != wanted.len() {
        return None;
    }
    Some(literals)
}

/// Validate a response body and return it for further assertions.
async fn assert_response_matches(
    app: &gita_admin::App,
    method: Method,
    path: &str,
    body: Option<Value>,
    token: Option<&str>,
) -> Value {
    let method_name = match method.as_str() {
        "GET" => "get",
        "POST" => "post",
        "PATCH" => "patch",
        "PUT" => "put",
        "DELETE" => "delete",
        other => panic!("unsupported method {other}"),
    };
    let (status, body) = common::request(app, method, path, body, token).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{path}: {body}");

    let spec = spec();
    let schema = response_schema(&spec, path, method_name);
    validate(&body, schema, &spec, "").unwrap_or_else(|error| {
        panic!("{method_name} {path} drifted from the spec: {error}\nbody: {body}")
    });
    body
}

/// The auth and repository responses, the two shapes the web client relies
/// on most, match the published schemas end to end.
#[tokio::test]
async fn the_documented_shapes_are_the_ones_the_server_returns() {
    let (app, _guard) = common::test_app().await;

    let health = assert_response_matches(&app, Method::GET, "/api/health", None, None).await;
    assert_eq!(health["status"], "ok");
    assert_eq!(health["db"], "ok");
    assert!(health["git"].is_string());

    let registered = assert_response_matches(
        &app,
        Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "contractuser",
            "email": "contractuser@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    let token = registered["token"]
        .as_str()
        .expect("AuthResponse.token must be a string")
        .to_string();

    let me = assert_response_matches(&app, Method::GET, "/api/auth/me", None, Some(&token)).await;
    assert_eq!(me["username"], "contractuser");

    let created = assert_response_matches(
        &app,
        Method::POST,
        "/api/repos",
        Some(serde_json::json!({
            "name": "contract-repo",
            "description": "contract",
            "default_branch": "main",
        })),
        Some(&token),
    )
    .await;
    assert_eq!(created["full_name"], "contractuser/contract-repo");
    assert_eq!(created["owner"], "contractuser");
    assert_eq!(created["name"], "contract-repo");

    let listed = assert_response_matches(&app, Method::GET, "/api/repos", None, Some(&token)).await;
    let repos = listed.as_array().expect("the repository list is an array");
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0]["full_name"], "contractuser/contract-repo");
}

/// An endpoint whose path carries parameters is documented under a
/// template, and the request carries concrete values. The lookup has to
/// bridge the two, or every endpoint with a path parameter is out of
/// reach of this validator.
#[tokio::test]
async fn a_path_with_parameters_is_matched_against_its_template() {
    let (app, _guard) = common::test_app().await;

    let registered = assert_response_matches(
        &app,
        Method::POST,
        "/api/auth/register",
        Some(serde_json::json!({
            "username": "templated",
            "email": "templated@example.com",
            "password": "hunter22",
        })),
        None,
    )
    .await;
    let token = registered["token"].as_str().unwrap().to_string();

    let created = assert_response_matches(
        &app,
        Method::POST,
        "/api/repos",
        Some(serde_json::json!({
            "name": "templated-repo",
            "description": "template",
            "default_branch": "main",
        })),
        Some(&token),
    )
    .await;
    assert_eq!(created["full_name"], "templated/templated-repo");

    // The documented path is `/api/repos/{owner}/{repo}/prs`; the request
    // carries `templated` and `templated-repo`.
    let prs = assert_response_matches(
        &app,
        Method::GET,
        "/api/repos/templated/templated-repo/prs",
        None,
        Some(&token),
    )
    .await;
    assert!(
        prs["prs"].is_array(),
        "the PR list must be an array, got {prs}"
    );
}

/// The CI agent callbacks are documented, and their documented shapes are
/// the shapes they answer with.
///
/// These two endpoints were live long before they were documented: the
/// drift test read the route list out of a hand-maintained list of
/// modules, and `routes/ci_agent.rs` was not on it. Nothing could see
/// them — not the drift test, not this validator.
#[tokio::test]
async fn the_agent_callbacks_match_their_documented_shapes() {
    let (app, _guard) = common::test_app().await;

    // A job to report on.
    let user = common::register_user(&app, "contractagent").await;
    let full = common::create_repo(&app, &user.token, "contract-agent-repo").await;
    let (owner, repo) = full.split_once('/').unwrap();
    let repo_row = app
        .db
        .get_repository_by_name(&owner, &repo)
        .await
        .unwrap()
        .unwrap();

    let now = chrono::Utc::now().to_rfc3339();
    let pipeline_id: i64 = sqlx::query_scalar(
        "INSERT INTO pipelines (repo_id, path, ref, ref_sha, content, created_at, updated_at)
         VALUES ($1, '.gita-ci.yml', 'main', 'deadbeef', 'stages: [b]', $2, $2)
         RETURNING id",
    )
    .bind(repo_row.id)
    .bind(&now)
    .fetch_one(app.db.pool())
    .await
    .unwrap();
    let run_id: i64 = sqlx::query_scalar(
        "INSERT INTO pipeline_runs (pipeline_id, repo_id, ref, head_sha, status, queued_at)
         VALUES ($1, $2, 'main', 'deadbeef', 'running', $3)
         RETURNING id",
    )
    .bind(pipeline_id)
    .bind(repo_row.id)
    .bind(&now)
    .fetch_one(app.db.pool())
    .await
    .unwrap();
    let job_id = app
        .db
        .insert_job(run_id, "build", "b", &[], &["echo hi".to_string()])
        .await
        .unwrap();

    // The agent token, minted the way the runner mints it.
    let token = {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let plaintext = hex::encode(bytes);
        app.db
            .set_job_agent_token(job_id, &gita_db::hash_token(&plaintext))
            .await
            .unwrap();
        plaintext
    };

    let appended = assert_response_matches(
        &app,
        Method::POST,
        &format!("/api/ci/agent/jobs/{job_id}/logs"),
        Some(serde_json::json!({
            "seq": 1,
            "stream": "stdout",
            "line": "hello from the agent",
        })),
        Some(&token),
    )
    .await;
    assert_eq!(appended["ok"], true);

    let reported = assert_response_matches(
        &app,
        Method::POST,
        &format!("/api/ci/agent/jobs/{job_id}/status"),
        Some(serde_json::json!({ "status": "running" })),
        Some(&token),
    )
    .await;
    assert_eq!(reported["ok"], true);
    assert_eq!(reported["applied"], true);
}

/// The number of JSON 200 responses that currently publish a schema.
///
/// The hand-written spec still describes many endpoints with only a prose
/// summary. That is a visibility gap, not something this test can fix, but
/// the number must not go down: a change that deletes a response schema or
/// renames a path out from under one fails here.
const MIN_DOCUMENTED_JSON_RESPONSES: usize = 23;

/// No documented response schema may reference a component that does not
/// exist, and the number of schemas must not shrink.
#[test]
fn documented_response_schemas_do_not_disappear_or_dangle() {
    let spec = spec();
    let mut with_schema = 0usize;
    let mut dangling = Vec::new();

    let paths = spec["paths"].as_object().expect("paths must be an object");
    for (path, item) in paths {
        let Some(operations) = item.as_object() else {
            continue;
        };
        for (method, operation) in operations {
            if !gita_api::docs::HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            let Some(schema) = operation.pointer("/responses/200/content/application~1json/schema")
            else {
                continue;
            };
            with_schema += 1;
            for reference in schema_refs(schema) {
                let Some(pointer) = reference.strip_prefix('#') else {
                    dangling.push(format!("{method} {path}: non-local $ref {reference}"));
                    continue;
                };
                if spec.pointer(pointer).is_none() {
                    dangling.push(format!("{method} {path}: {reference}"));
                }
            }
        }
    }

    assert!(
        dangling.is_empty(),
        "these response schemas reference missing components: {dangling:?}"
    );
    assert!(
        with_schema >= MIN_DOCUMENTED_JSON_RESPONSES,
        "the spec used to document {MIN_DOCUMENTED_JSON_RESPONSES} JSON response schemas, \
         but now has {with_schema}; add the schema back or lower the ratchet deliberately"
    );
}

fn schema_refs(schema: &Value) -> Vec<String> {
    let mut refs = Vec::new();
    collect_refs(schema, &mut refs);
    refs
}

fn collect_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                out.push(reference.to_string());
            }
            for child in object.values() {
                collect_refs(child, out);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect_refs(child, out);
            }
        }
        _ => {}
    }
}
