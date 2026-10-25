//! Authentication: password hashing, JWT, and personal access tokens.

use crate::error::{ApiError, ApiResult};
use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
};
use chrono::{DateTime, Duration, Utc};
use gita_core::User;
use gita_db::Db;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// JWT claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: i64,
    pub username: String,
    pub exp: usize,
    /// JWT ID for revocation.
    ///
    /// Optional because tokens issued before the revocation list existed
    /// have no `jti`. Those are not rejected — see
    /// [`Auth::decode_jwt`], which derives a stable id for them so they
    /// can still be revoked — but a token minted now always carries one.
    #[serde(default)]
    pub jti: Option<String>,
}

/// A password hasher wrapper.
#[derive(Debug, Clone)]
pub struct Auth {
    jwt_secret: String,
}

impl Auth {
    pub fn new(jwt_secret: String) -> Self {
        Self { jwt_secret }
    }

    /// Hash a plaintext password with Argon2.
    pub fn hash_password(password: &str) -> ApiResult<String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| ApiError::Internal(format!("password hash error: {e}")))
    }

    /// Verify a plaintext password against a stored hash.
    pub fn verify_password(password: &str, hash: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(hash) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }

    /// Generate a JWT for a user.
    pub fn sign_jwt(&self, user: &User) -> ApiResult<String> {
        let exp = (Utc::now() + Duration::days(7)).timestamp() as usize;
        let claims = Claims {
            sub: user.id,
            username: user.username.clone(),
            exp,
            jti: Some(Uuid::new_v4().to_string()),
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret.as_bytes()),
        )
        .map_err(|e| ApiError::Internal(format!("jwt encode error: {e}")))
    }

    /// Decode and validate a JWT, without consulting the revocation list.
    ///
    /// Use this when the caller only needs the claims (e.g. to read the
    /// `jti` of the token that was just presented). Use
    /// [`Auth::verify_jwt`] — or [`Auth::resolve_bearer`] — when the token
    /// must also be checked against the revocation list.
    ///
    /// Tokens minted before the revocation list existed carry no `jti`.
    /// Rather than reject them — which would log every existing user out
    /// the moment the server is upgraded — a stable id is derived from the
    /// token itself. It is deterministic, so the same legacy token always
    /// maps to the same revocation entry and can still be revoked.
    pub fn decode_jwt(&self, token: &str) -> ApiResult<Claims> {
        let mut validation = Validation::default();
        validation.required_spec_claims = std::collections::HashSet::new();
        validation.validate_exp = true;

        let mut claims = decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &validation,
        )
        .map(|d| d.claims)
        .map_err(|e| ApiError::Unauthorized(format!("invalid token: {e}")))?;

        if claims.jti.is_none() {
            claims.jti = Some(format!("legacy:{}", gita_db::hash_token(token)));
        }
        Ok(claims)
    }

    /// Decode and validate a JWT, and reject it if it was revoked.
    pub async fn verify_jwt(&self, db: &Db, token: &str) -> ApiResult<Claims> {
        let claims = self.decode_jwt(token)?;
        // `decode_jwt` always fills in a jti — deriving one for legacy
        // tokens — so this branch is taken for every usable token.
        if let Some(jti) = &claims.jti {
            if self.is_jwt_revoked(db, jti).await? {
                return Err(ApiError::Unauthorized("token has been revoked".into()));
            }
        }
        Ok(claims)
    }

    /// Check if a JWT is revoked.
    pub async fn is_jwt_revoked(&self, db: &Db, jti: &str) -> ApiResult<bool> {
        let row =
            sqlx::query("SELECT 1 FROM jwt_revocation_list WHERE jti = $1 AND expires_at > NOW()")
                .bind(jti)
                .fetch_optional(db.pool())
                .await?;
        Ok(row.is_some())
    }

    /// Revoke a JWT by its JTI.
    ///
    /// The expiry is stored alongside the JTI so the row can be pruned
    /// once the token would have expired anyway.
    pub async fn revoke_jwt(&self, db: &Db, jti: &str, expires_at: DateTime<Utc>) -> ApiResult<()> {
        sqlx::query(
            "INSERT INTO jwt_revocation_list (jti, expires_at) VALUES ($1, $2)
             ON CONFLICT (jti) DO NOTHING",
        )
        .bind(jti)
        .bind(expires_at)
        .execute(db.pool())
        .await?;
        Ok(())
    }

    /// Drop revocation rows for tokens that have expired anyway.
    ///
    /// A revoked token is only rejected while it would still have been
    /// valid, so rows past their expiry carry no information. Pruning them
    /// keeps the table from growing without bound. Failures are ignored:
    /// a stale row only costs a little space, and the caller is already on
    /// the success path.
    pub async fn prune_expired_revocations(&self, db: &Db) {
        let _ = sqlx::query("DELETE FROM jwt_revocation_list WHERE expires_at <= NOW()")
            .execute(db.pool())
            .await;
    }

    /// Resolve the authenticated user from a Bearer token (JWT or PAT).
    pub async fn resolve_bearer(&self, db: &Db, token: &str) -> ApiResult<User> {
        // First try PAT lookup.
        if let Some(user) = db.get_user_by_token(token).await? {
            return Ok(user);
        }
        // Then try JWT.
        let claims = self.verify_jwt(db, token).await?;
        db.get_user_by_id(claims.sub)
            .await?
            .ok_or_else(|| ApiError::Unauthorized("user no longer exists".to_string()))
    }
}

/// Hash a password (convenience for tests).
pub fn hash_password(password: &str) -> String {
    Auth::hash_password(password).expect("hash password")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_roundtrip() {
        let hash = Auth::hash_password("s3cret").unwrap();
        assert!(Auth::verify_password("s3cret", &hash));
        assert!(!Auth::verify_password("wrong", &hash));
    }

    #[test]
    fn password_verify_rejects_invalid_hash_format() {
        // Garbage strings must not panic; verify_password must return false.
        assert!(!Auth::verify_password("any", "not-a-real-hash"));
        assert!(!Auth::verify_password("any", ""));
    }

    #[test]
    fn jwt_roundtrip() {
        let auth = Auth::new("test-secret".to_string());
        let user = User {
            id: 1,
            username: "alice".to_string(),
            email: "a@b.c".to_string(),
            password_hash: "x".to_string(),
            created_at: Utc::now(),
            locale: "zh".to_string(),
            is_admin: false,
        };
        let token = auth.sign_jwt(&user).unwrap();
        let claims = auth.decode_jwt(&token).unwrap();
        assert_eq!(claims.sub, 1);
        assert_eq!(claims.username, "alice");
        // Every token carries its own JTI, so one can be revoked without
        // affecting the others.
        assert!(claims.jti.is_some());
    }

    #[test]
    fn jwt_claims_carry_a_unique_jti() {
        let auth = Auth::new("test-secret".to_string());
        let user = User {
            id: 1,
            username: "alice".to_string(),
            email: "a@b.c".to_string(),
            password_hash: "x".to_string(),
            created_at: Utc::now(),
            locale: "zh".to_string(),
            is_admin: false,
        };
        let a = auth.decode_jwt(&auth.sign_jwt(&user).unwrap()).unwrap();
        let b = auth.decode_jwt(&auth.sign_jwt(&user).unwrap()).unwrap();
        assert_ne!(a.jti, b.jti, "each JWT must have a distinct JTI");
    }

    /// A token minted before the revocation list existed has no `jti`.
    /// It must still decode — rejecting it would log every existing user
    /// out on upgrade — and it must get a *stable* id, so the same token
    /// always maps to the same revocation entry.
    #[test]
    fn a_token_without_a_jti_still_decodes_and_gets_a_stable_id() {
        let auth = Auth::new("test-secret".to_string());
        // Mint a token the old way: no `jti` in the claims.
        let legacy = Claims {
            sub: 1,
            username: "alice".to_string(),
            exp: (Utc::now() + Duration::days(7)).timestamp() as usize,
            jti: None,
        };
        let token = encode(
            &Header::default(),
            &legacy,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .unwrap();

        let first = auth.decode_jwt(&token).unwrap();
        let second = auth.decode_jwt(&token).unwrap();
        assert_eq!(first.jti, second.jti, "the derived id must be stable");
        assert!(
            first.jti.as_deref().unwrap().starts_with("legacy:"),
            "a token without a jti must get a derived id, got {:?}",
            first.jti
        );
    }

    #[test]
    fn jwt_with_wrong_secret_rejected() {
        let signer = Auth::new("secret-A".to_string());
        let user = User {
            id: 7,
            username: "x".to_string(),
            email: "y@z.io".to_string(),
            password_hash: "x".to_string(),
            created_at: Utc::now(),
            locale: "zh".to_string(),
            is_admin: false,
        };
        let token = signer.sign_jwt(&user).unwrap();
        let verifier = Auth::new("secret-B".to_string());
        let err = verifier.decode_jwt(&token).unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)), "got {err:?}");
    }

    #[test]
    fn jwt_garbage_returns_unauthorized() {
        let auth = Auth::new("test-secret".to_string());
        let err = auth.decode_jwt("not.a.jwt").unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }
}
