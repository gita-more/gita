//! Authentication routes: register, login, personal access tokens.

use crate::auth::Auth;
use crate::error::{ApiError, ApiResult};
use crate::extract::AuthUserExtractor;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use gita_core::EventType;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    /// Optional UI language preference ("zh" | "en"); defaults to "zh".
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: PublicUser,
}

#[derive(Debug, Serialize)]
pub struct PublicUser {
    pub id: i64,
    pub username: String,
    pub email: String,
    pub locale: String,
    pub is_admin: bool,
}

impl From<gita_core::User> for PublicUser {
    fn from(u: gita_core::User) -> Self {
        Self {
            id: u.id,
            username: u.username,
            email: u.email,
            locale: u.locale,
            is_admin: u.is_admin,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct UpdateMeRequest {
    pub locale: Option<String>,
    pub email: Option<String>,
    pub current_password: Option<String>,
    pub new_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Payload for initializing the system on first-ever use.
///
/// Only accepted when the users table is completely empty. The created
/// account becomes the first administrator (owner of everything it owns).
#[derive(Debug, Deserialize)]
pub struct InitRequest {
    pub username: String,
    pub email: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub id: i64,
    /// The token's secret. Only populated when the token was *just*
    /// created — this is the one and only time the user gets to see it.
    /// Later listings carry an empty string.
    pub token: String,
    /// A short prefix of the secret, so the user can tell their tokens
    /// apart without the secret being recoverable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    pub name: String,
}

/// The first few characters of a token, for display purposes.
fn token_prefix(token: &str) -> String {
    token.chars().take(8).collect()
}

pub fn auth_routes() -> Router<AppState> {
    // Whether the system has been initialized yet (any user exists).
    // Unauthenticated — needed so the frontend can decide to show the
    // init page before any account exists.
    // Create the very first admin account. Rejected once any user
    // exists, so it can only ever run once.
    Router::new()
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/initialized", get(initialized))
        .route("/api/auth/init", post(init))
        .route("/api/auth/tokens", post(create_token).get(list_tokens))
        .route("/api/auth/tokens/{id}", axum::routing::delete(delete_token))
        .route("/api/auth/me", axum::routing::get(me).patch(update_me))
        .route(
            "/api/auth/password-reset/request",
            axum::routing::post(password_reset_request),
        )
        .route(
            "/api/auth/password-reset/confirm",
            axum::routing::post(password_reset_confirm),
        )
        .route("/api/auth/logout", axum::routing::post(logout))
}

async fn register(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    connect_info: crate::extract::OptionalConnectInfo,
    Json(req): Json<RegisterRequest>,
) -> ApiResult<Json<AuthResponse>> {
    // Per-IP rate limiting. Registration is the more expensive of the two:
    // it performs an Argon2 hash on every call, so leaving it unthrottled
    // lets a script fill the users table at full request rate — and
    // brute-forcing a *known* account is limited while creating unlimited
    // new ones is not. It gets its own bucket so that login traffic and
    // registration traffic cannot starve one another.
    if let Some(limiter) = state.register_limiter.as_ref() {
        let key = crate::middleware::client_ip(&headers, connect_info.0);
        if limiter.check_key(&key).is_err() {
            return Err(ApiError::RateLimited(
                "too many registration attempts; try again later".into(),
            ));
        }
    }
    let username = req.username.trim().to_string();
    let email = req.email.trim().to_string();
    if username.is_empty() || username.len() < 2 {
        return Err(ApiError::BadRequest(
            "username must be at least 2 characters".into(),
        ));
    }
    // The username becomes a directory name under the repositories root
    // (`<data_dir>/repos/<username>/`), so it must not be able to escape it.
    // Same character rule as repository names.
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ApiError::BadRequest(
            "username may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    // "." and ".." are valid character-wise but are directory traversal.
    if username == "." || username == ".." {
        return Err(ApiError::BadRequest("invalid username".into()));
    }
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::BadRequest("invalid email".into()));
    }
    if req.password.len() < 6 {
        return Err(ApiError::BadRequest(
            "password must be at least 6 characters".into(),
        ));
    }

    // Check uniqueness.
    //
    // The two conflicts share one message on purpose. Telling the
    // caller *which* of the two collided tells an anonymous caller
    // whether an account exists — and the two cases have different
    // searchable spaces (usernames are public identifiers, e-mail
    // addresses are not) — so a distinct message for each would leak
    // the registered e-mail addresses.
    if let Some(_) = state.db.get_user_by_username(&username).await? {
        // Hash a dummy password to keep timing constant (mitigate
        // username enumeration via response time).
        let _ = Auth::hash_password("dummy-constant-time-mitigation");
        return Err(ApiError::Conflict(
            "username or email already in use".into(),
        ));
    }
    if let Some(_) = state.db.get_user_by_email(&email).await? {
        // Hash a dummy password to keep timing constant.
        let _ = Auth::hash_password("dummy-constant-time-mitigation");
        return Err(ApiError::Conflict(
            "username or email already in use".into(),
        ));
    }

    // Validate locale if provided (default "zh").
    let locale = req.locale.as_deref().unwrap_or("zh");
    if locale != "zh" && locale != "en" {
        return Err(ApiError::BadRequest("invalid locale".into()));
    }

    let hash = Auth::hash_password(&req.password)?;
    let user = state
        .db
        .create_user_with_locale(&username, &email, &hash, locale)
        .await?;
    let token = state.auth.sign_jwt(&user)?;

    state.bus.publish(
        EventType::UserRegistered,
        None,
        Some(user.id),
        serde_json::json!({"username": username}),
    );

    Ok(Json(AuthResponse {
        token,
        user: PublicUser::from(user),
    }))
}

pub async fn login(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    connect_info: crate::extract::OptionalConnectInfo,
    Json(req): Json<LoginRequest>,
) -> ApiResult<Json<AuthResponse>> {
    // Per-IP rate limiting (P1). The bucket is per-IP and keyed on the
    // real peer address when the request arrived over a socket; the
    // X-Forwarded-For header is only consulted as a fallback (see
    // `middleware::client_ip`).
    if let Some(limiter) = state.login_limiter.as_ref() {
        let key = crate::middleware::client_ip(&headers, connect_info.0);
        if limiter.check_key(&key).is_err() {
            return Err(ApiError::RateLimited(
                "too many login attempts; try again later".into(),
            ));
        }
    }
    let user = state
        .db
        .get_user_by_username(&req.username)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("invalid credentials".into()))?;
    if !Auth::verify_password(&req.password, &user.password_hash) {
        return Err(ApiError::Unauthorized("invalid credentials".into()));
    }
    let token = state.auth.sign_jwt(&user)?;
    Ok(Json(AuthResponse {
        token,
        user: PublicUser::from(user),
    }))
}

/// Whether the system has been initialized yet — true once any user
/// exists. Unauthenticated: the frontend checks this before any account
/// exists to decide whether to show the init page.
async fn initialized(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let count = state.db.count_users().await?;
    Ok(Json(json!({ "initialized": count > 0 })))
}

/// Create the very first admin account.
///
/// Only accepted when the users table is completely empty, so this can
/// only ever run once — the first person to initialize the system becomes
/// its first administrator.
async fn init(
    State(state): State<AppState>,
    Json(req): Json<InitRequest>,
) -> ApiResult<Json<AuthResponse>> {
    let username = req.username.trim().to_string();
    let email = req.email.trim().to_string();
    if username.is_empty() || username.len() < 2 {
        return Err(ApiError::BadRequest(
            "username must be at least 2 characters".into(),
        ));
    }
    // The username becomes a directory name under the repositories root
    // (`<data_dir>/repos/<username>/`), so it must not be able to escape it.
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(ApiError::BadRequest(
            "username may only contain letters, numbers, '-', '_', '.'".into(),
        ));
    }
    if username == "." || username == ".." {
        return Err(ApiError::BadRequest("invalid username".into()));
    }
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::BadRequest("invalid email".into()));
    }
    if req.password.len() < 6 {
        return Err(ApiError::BadRequest(
            "password must be at least 6 characters".into(),
        ));
    }

    // Guard: only initialize an empty system. This check and the insert are
    // two statements, so a race could theoretically create two users — but
    // the unique constraints on username/email make the second insert fail,
    // which we surface as a conflict. The first writer wins.
    let count = state.db.count_users().await?;
    if count > 0 {
        return Err(ApiError::BadRequest("system already initialized".into()));
    }

    let hash = Auth::hash_password(&req.password)?;
    let user = state
        .db
        .create_user_with_role(&username, &email, &hash, "zh", true)
        .await?;
    let token = state.auth.sign_jwt(&user)?;

    state.bus.publish(
        EventType::UserRegistered,
        None,
        Some(user.id),
        serde_json::json!({"username": username}),
    );

    Ok(Json(AuthResponse {
        token,
        user: PublicUser::from(user),
    }))
}

async fn create_token(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    // `Option<Json<T>>` accepts a request without a body — the token name
    // is optional, so a body-less POST is a valid "create a default token".
    req: Option<Json<CreateTokenRequest>>,
) -> ApiResult<Json<TokenResponse>> {
    let name = req
        .map(|Json(r)| r.name)
        .flatten()
        .unwrap_or_else(|| "default".to_string());
    // Generate a random 32-byte token, hex-encoded.
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);

    let created = state.db.create_access_token(user.id, &token, &name).await?;
    let prefix = token_prefix(&token);
    Ok(Json(TokenResponse {
        id: created.id,
        // The plaintext is shown exactly once, here.
        token,
        prefix: Some(prefix),
        name: created.name,
    }))
}

async fn list_tokens(
    State(state): State<AppState>,
    user: AuthUserExtractor,
) -> ApiResult<Json<Vec<TokenResponse>>> {
    let tokens = state.db.list_tokens(user.id).await?;
    Ok(Json(
        tokens
            .into_iter()
            .map(|t| TokenResponse {
                id: t.id,
                // The secret is not recoverable from the database, so it
                // cannot be shown again.
                token: String::new(),
                prefix: None,
                name: t.name,
            })
            .collect(),
    ))
}

async fn delete_token(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    Path(id): Path<i64>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let deleted = state.db.delete_token(id, user.id).await?;
    if !deleted {
        return Err(ApiError::NotFound("token not found".into()));
    }
    Ok(Json(crate::error::MessageResponse {
        message: "token deleted".to_string(),
    }))
}

async fn me(State(state): State<AppState>, user: AuthUserExtractor) -> ApiResult<Json<PublicUser>> {
    let db_user = state
        .db
        .get_user_by_id(user.id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    Ok(Json(PublicUser::from(db_user)))
}

async fn update_me(
    State(state): State<AppState>,
    user: AuthUserExtractor,
    // Every field is optional, so a body-less PATCH is a no-op rather
    // than a 415.
    req: Option<Json<UpdateMeRequest>>,
) -> ApiResult<Json<PublicUser>> {
    let req = req.map(|Json(r)| r).unwrap_or_default();
    let mut db_user = state
        .db
        .get_user_by_id(user.id)
        .await?
        .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;

    // Email change.
    if let Some(email) = req.email {
        let email = email.trim().to_string();
        if email.is_empty() || !email.contains('@') {
            return Err(ApiError::BadRequest("invalid email".into()));
        }
        // Verify current password for sensitive changes.
        let current = req.current_password.as_deref().ok_or_else(|| {
            ApiError::BadRequest("current_password required to change email".into())
        })?;
        if !Auth::verify_password(current, &db_user.password_hash) {
            return Err(ApiError::Unauthorized("invalid credentials".into()));
        }
        // Ensure the new email is unique.
        if let Some(other) = state.db.get_user_by_email(&email).await? {
            if other.id != user.id {
                return Err(ApiError::Conflict("email already registered".into()));
            }
        }
        db_user = state
            .db
            .update_user_email(user.id, &email)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    }

    // Password change.
    if let Some(new_password) = req.new_password {
        if new_password.len() < 6 {
            return Err(ApiError::BadRequest(
                "password must be at least 6 characters".into(),
            ));
        }
        let current = req.current_password.as_deref().ok_or_else(|| {
            ApiError::BadRequest("current_password required to change password".into())
        })?;
        if !Auth::verify_password(current, &db_user.password_hash) {
            return Err(ApiError::Unauthorized("invalid credentials".into()));
        }
        let hash = Auth::hash_password(&new_password)?;
        state.db.update_user_password_hash(user.id, &hash).await?;
        // Refresh user record so the response reflects the new hash.
        db_user = state
            .db
            .get_user_by_id(user.id)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    }

    // Locale change.
    if let Some(locale) = req.locale {
        let locale = locale.trim().to_string();
        if locale != "zh" && locale != "en" {
            return Err(ApiError::BadRequest("invalid locale".into()));
        }
        db_user = state
            .db
            .update_user_locale(user.id, &locale)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user not found".into()))?;
    }

    Ok(Json(PublicUser::from(db_user)))
}
// ---------------------------------------------------------------------------
// Password reset
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PasswordResetRequestBody {
    /// Either the username or the email of the account.
    pub identifier: String,
}

#[derive(Debug, Deserialize)]
pub struct PasswordResetConfirmBody {
    pub token: String,
    pub new_password: String,
}

/// Start a password-reset flow. Always returns the same 200 response
/// regardless of whether the user exists, to avoid account enumeration.
///
/// The reset token has to reach the user somehow. A deployment with a mail
/// transport delivers it by mail; one without has no way to do so, so
/// `server.allow_insecure_password_reset` puts the token in the response
/// body instead. That switch is off by default: the response is uniform
/// for existing and non-existent accounts, so returning the token would
/// let an anonymous caller enumerate accounts *and* take them over.
async fn password_reset_request(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    connect_info: crate::extract::OptionalConnectInfo,
    Json(req): Json<PasswordResetRequestBody>,
) -> ApiResult<Json<serde_json::Value>> {
    // Per-IP rate limiting. The endpoint is unauthenticated, and every
    // request for an existing account writes a token row and a log line —
    // an unthrottled endpoint would let a script grow both at request rate.
    // The bucket is separate from login and registration so that reset
    // traffic cannot starve them (or the reverse).
    if let Some(limiter) = state.password_reset_limiter.as_ref() {
        let key = crate::middleware::client_ip(&headers, connect_info.0);
        if limiter.check_key(&key).is_err() {
            return Err(ApiError::RateLimited(
                "too many password reset attempts; try again later".into(),
            ));
        }
    }
    let identifier = req.identifier.trim().to_string();
    let mut reset_token: Option<String> = None;
    if let Some(user) = state.db.get_user_by_username(&identifier).await? {
        reset_token = Some(create_reset_token(&state, user.id, &user.username).await?);
    } else if let Some(user) = state.db.get_user_by_email(&identifier).await? {
        reset_token = Some(create_reset_token(&state, user.id, &user.username).await?);
    }
    let mut body = serde_json::json!({
        "message": "if the account exists, a reset link has been sent",
    });
    // The key is absent — not null — when the switch is off, so that its
    // presence cannot be used to detect an existing account.
    if state.allow_insecure_password_reset {
        if let Some(token) = reset_token {
            body["reset_token"] = serde_json::Value::String(token);
        }
    }
    Ok(Json(body))
}

/// Issue a reset token and return it.
///
/// The token is a 32-byte random value, hex-encoded — 64 hex characters of
/// entropy, so it cannot be guessed.
async fn create_reset_token(state: &AppState, user_id: i64, username: &str) -> ApiResult<String> {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let token = hex::encode(bytes);
    let now = chrono::Utc::now();
    let expires_at = (now + chrono::Duration::hours(1)).to_rfc3339();
    // Tokens that were already used or have expired can never be redeemed;
    // drop them now so the table does not grow without bound. Tokens that
    // are still valid survive — requesting a second link must not silently
    // invalidate the first one.
    state
        .db
        .purge_stale_password_reset_tokens(user_id, &now.to_rfc3339())
        .await?;
    state
        .db
        .create_password_reset_token(user_id, &token, &expires_at)
        .await?;
    // No mail transport is wired up, so the token is only reachable through
    // the server log. The hint shows the exact invocation the endpoint
    // expects — the confirm endpoint takes a JSON body, it is not a GET
    // with a query parameter.
    tracing::info!(
        "password reset for {username}: POST /api/auth/password-reset/confirm \
         with body {{\"token\":\"{token}\",\"new_password\":\"<new password>\"}}"
    );
    Ok(token)
}

async fn password_reset_confirm(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    connect_info: crate::extract::OptionalConnectInfo,
    Json(req): Json<PasswordResetConfirmBody>,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    // Same bucket as the request endpoint. Confirming a valid token runs an
    // Argon2 hash, which is deliberately expensive — it must not be
    // reachable at request rate.
    if let Some(limiter) = state.password_reset_limiter.as_ref() {
        let key = crate::middleware::client_ip(&headers, connect_info.0);
        if limiter.check_key(&key).is_err() {
            return Err(ApiError::RateLimited(
                "too many password reset attempts; try again later".into(),
            ));
        }
    }
    if req.new_password.len() < 6 {
        return Err(ApiError::BadRequest(
            "password must be at least 6 characters".into(),
        ));
    }
    // Claim the token atomically: the same statement that reads it marks it
    // used, so a token can only ever be redeemed once. Reading it and
    // marking it afterwards would let two concurrent requests both pass the
    // read and both set a new password — the second silently overwriting
    // the first.
    let user_id = state
        .db
        .claim_password_reset_token(&req.token, &chrono::Utc::now().to_rfc3339())
        .await?
        .ok_or_else(|| ApiError::NotFound("invalid or expired token".into()))?;
    // The token is spent now. If hashing or the update fails, the user has
    // to request a new link — better than leaving a live token behind.
    let hash = Auth::hash_password(&req.new_password)?;
    state.db.update_user_password_hash(user_id, &hash).await?;
    Ok(Json(crate::error::MessageResponse {
        message: "password reset".to_string(),
    }))
}

/// POST /api/auth/logout — revoke the current session.
///
/// The client sends the Bearer token in the Authorization header.
///
/// * With a **JWT**, the token's `jti` is added to the revocation list
///   (together with its expiry), so the token can no longer authenticate.
///   This is the only thing logout can revoke: the token stays dead until
///   it would have expired anyway.
///
/// * With a **personal access token**, there is no session to end — a PAT
///   is a long-lived credential that outlives any login. Logging out does
///   *not* revoke it, and the response says so; a PAT is revoked
///   explicitly with `DELETE /api/auth/tokens/{id}`.
async fn logout(
    State(state): State<AppState>,
    _user: crate::extract::AuthUserExtractor,
    headers: axum::http::HeaderMap,
) -> ApiResult<Json<crate::error::MessageResponse>> {
    let header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or_else(|| ApiError::Unauthorized("expected Bearer token".into()))?;

    // The extractor already authenticated this token, so it is either a
    // valid JWT or a valid PAT. Anything that does not decode as a JWT is
    // therefore a PAT.
    let message = match state.auth.decode_jwt(token) {
        Ok(claims) => {
            let expires_at = chrono::DateTime::from_timestamp(claims.exp as i64, 0)
                .unwrap_or_else(chrono::Utc::now);
            if let Some(jti) = &claims.jti {
                state.auth.revoke_jwt(&state.db, jti, expires_at).await?;
            }
            // Opportunistic pruning: rows past their expiry are dead
            // weight. A failure here must not fail the logout.
            state.auth.prune_expired_revocations(&state.db).await;
            "logged out".to_string()
        }
        Err(_) => "logged out (this is a personal access token; it is not revoked by \
             logout — use DELETE /api/auth/tokens/{id} to revoke it)"
            .to_string(),
    };

    Ok(Json(crate::error::MessageResponse { message }))
}
