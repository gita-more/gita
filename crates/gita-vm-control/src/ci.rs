//! CI VM endpoints for the runner.
//!
//! A job with a `vm:` block runs inside a cluster virtual machine. The
//! runner used to build that VM itself: it took the server's database
//! pool, constructed a `VmStore`, and wrote the VM row directly. That
//! put the schema — and the control plane's invariants — in the runner's
//! hands.
//!
//! These endpoints move that work back to the control plane, where it
//! belongs:
//!
//! * `POST /ci/create` — instantiate a template into a VM, linked to the
//!   job, with the job's agent token hashed onto the job row.
//! * `POST /ci/reclaim` — destroy a VM that belongs to a job.
//!
//! Both are authenticated with a **runner token** (a personal access
//! token named `runner/…`), not with an administrator's token: a runner
//! is a CI worker, and it has no business managing the cluster.

use crate::auth::VmUser;
use crate::state::AppState;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use gita_vm_api::ids::OwnerRef;
use gita_vm_api::template::VmSpecOverrides;
use gita_vm_api::vm::{Vm, VmLinkage, VmStatus};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::handlers::{ApiError, ApiResult};

pub fn ci_routes() -> Router<AppState> {
    Router::new()
        .route("/ci/create", post(create_ci_vm))
        .route("/ci/reclaim", post(reclaim_ci_vm))
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

/// Require a runner token: a personal access token named `runner/…`.
///
/// The middleware has already resolved the bearer token into a user; this
/// checks the *kind* of token. A user's everyday PAT must not be able to
/// create and destroy cluster VMs.
async fn require_runner_token(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    let Some(token) = bearer_token_from(headers) else {
        return Err(ApiError(
            axum::http::StatusCode::UNAUTHORIZED,
            "missing bearer token".into(),
        ));
    };
    let name = state
        .auth
        .db()
        .get_token_name(&token)
        .await
        .map_err(|e| ApiError(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| ApiError(axum::http::StatusCode::UNAUTHORIZED, "invalid token".into()))?;
    if !name.starts_with("runner/") {
        return Err(ApiError(
            axum::http::StatusCode::FORBIDDEN,
            "this endpoint requires a runner token (one named runner/…)".into(),
        ));
    }
    Ok(())
}

/// Pull the bearer token out of the headers.
///
/// `bearer_token` takes a `Request`, which cannot be borrowed across an
/// `await`; this works on the header map the handler already has.
fn bearer_token_from(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get("authorization")?.to_str().ok()?;
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateCiVmRequest {
    /// The pipeline run the job belongs to.
    pub run_id: i64,
    /// The job the VM executes.
    pub job_id: i64,
    /// The repository the job runs against.
    pub repo_id: i64,
    /// The cluster template to instantiate, e.g. `ci-runner`.
    pub template: String,
    /// Override the template's CPU, in millicores.
    #[serde(default)]
    pub cpu_millis: Option<u64>,
    /// Override the template's memory, in bytes.
    #[serde(default)]
    pub memory_bytes: Option<u64>,
    /// Override the template's disk, in bytes.
    #[serde(default)]
    pub disk_bytes: Option<u64>,
    /// When the VM may be reclaimed (unix milliseconds).
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
    /// cloud-init user data: the job's script, its environment and the
    /// agent token. The runner composes it — it is the only side that
    /// knows the job — and the plaintext agent token travels only here.
    #[serde(default)]
    pub user_data: Option<String>,
    /// The SHA-256 hash of the agent token, stored on the job row.
    pub agent_token_hash: String,
}

#[derive(Debug, Serialize)]
pub struct CreateCiVmResponse {
    pub vm_id: i64,
}

/// POST /ci/create
async fn create_ci_vm(
    State(state): State<AppState>,
    _user: VmUser,
    headers: HeaderMap,
    Json(req): Json<CreateCiVmRequest>,
) -> ApiResult<Json<CreateCiVmResponse>> {
    require_runner_token(&state, &headers).await?;

    // Resolve the template by name.
    let templates = state
        .repo
        .list_templates(None)
        .await
        .map_err(ApiError::from)?;
    let template = templates
        .iter()
        .find(|t| t.name == req.template)
        .ok_or_else(|| {
            ApiError(
                axum::http::StatusCode::NOT_FOUND,
                format!("vm template {:?} not found", req.template),
            )
        })?;

    // Apply the job's resource overrides. Sizes are validated by the
    // pipeline parser; a bad one here is a client bug.
    let overrides = VmSpecOverrides {
        cpu_millis: req.cpu_millis,
        memory_bytes: req.memory_bytes,
        disk_bytes: req.disk_bytes,
        user_data: req.user_data.clone(),
        ..Default::default()
    };

    let mut spec = template.instantiate(Some(&overrides));
    // `instantiate` copies the template's spec, whose `template` field is
    // normally empty. The VM row's `template_id` is taken from this field,
    // so without it a CI VM would be recorded with no template at all.
    spec.template = Some(template.id);
    // The job's user data wins over the template's: it carries the
    // script the job actually has to run.
    if let Some(user_data) = req.user_data.clone() {
        spec.user_data = Some(user_data);
    }

    let now = gita_vm_core::now_ms();
    let vm = Vm {
        id: gita_vm_api::ids::VmId(0),
        name: format!("ci-{}-{}", req.repo_id, now),
        owner: OwnerRef::system(),
        cell: None,
        node: None,
        spec,
        status: VmStatus::Pending,
        generation: 1,
        observed_generation: 0,
        conditions: vec![],
        rootfs_path: None,
        ip_address: None,
        console_url: None,
        created_at_ms: now,
        message: None,
        expires_at_ms: req.expires_at_ms,
    };

    let linkage = VmLinkage {
        repo_id: Some(req.repo_id),
        pipeline_run_id: Some(req.run_id),
        job_id: Some(req.job_id),
    };
    let vm_id = state
        .repo
        .create_linked_vm(&vm, linkage)
        .await
        .map_err(ApiError::from)?;

    // The agent inside the VM authenticates with this token; only its
    // hash is stored, and the plaintext lives in the user data above.
    state
        .auth
        .db()
        .set_job_agent_token(req.job_id, &req.agent_token_hash)
        .await
        .map_err(|e| ApiError(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(CreateCiVmResponse { vm_id: vm_id.0 }))
}

// ---------------------------------------------------------------------------
// Reclaim
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ReclaimCiVmRequest {
    pub vm_id: i64,
    /// The job that owns the VM. The server checks the linkage, so a
    /// runner cannot destroy a VM that belongs to another job.
    pub job_id: i64,
}

/// POST /ci/reclaim
async fn reclaim_ci_vm(
    State(state): State<AppState>,
    _user: VmUser,
    headers: HeaderMap,
    Json(req): Json<ReclaimCiVmRequest>,
) -> ApiResult<impl IntoResponse> {
    require_runner_token(&state, &headers).await?;

    let vm_id = gita_vm_api::ids::VmId(req.vm_id);
    let linkage = state
        .repo
        .get_vm_linkage(vm_id)
        .await
        .map_err(ApiError::from)?;
    match linkage {
        Some(linkage) if linkage.job_id == Some(req.job_id) => {}
        Some(_) => {
            return Err(ApiError(
                axum::http::StatusCode::FORBIDDEN,
                "that VM belongs to a different job".into(),
            ))
        }
        None => {
            return Err(ApiError(
                axum::http::StatusCode::NOT_FOUND,
                "vm not found".into(),
            ))
        }
    }

    state.repo.delete_vm(vm_id).await.map_err(ApiError::from)?;
    Ok(Json(json!({"ok": true})))
}
