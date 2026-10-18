//! Authentication: password hashing, JWT, and personal access tokens.

use crate::error::{ApiError, ApiResult};
use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::{rand_core::OsRng, SaltString},
};
use chrono::{Duration, Utc};
use gita_core::User;
use gita_db::Db;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

/// JWT claims.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: i64,
    pub username: String,
    pub exp: usize,
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
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.jwt_secret.as_bytes()),
        )
        .map_err(|e| ApiError::Internal(format!("jwt encode error: {e}")))
    }

    /// Decode and validate a JWT.
    pub fn verify_jwt(&self, token: &str) -> ApiResult<Claims> {
        decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.jwt_secret.as_bytes()),
            &Validation::default(),
        )
        .map(|d| d.claims)
        .map_err(|e| ApiError::Unauthorized(format!("invalid token: {e}")))
    }

    /// Resolve the authenticated user from a Bearer token (JWT or PAT).
    pub async fn resolve_bearer(&self, db: &Db, token: &str) -> ApiResult<User> {
        // First try PAT lookup.
        if let Some(user) = db.get_user_by_token(token).await? {
            return Ok(user);
        }
        // Then try JWT.
        let claims = self.verify_jwt(token)?;
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
        };
        let token = auth.sign_jwt(&user).unwrap();
        let claims = auth.verify_jwt(&token).unwrap();
        assert_eq!(claims.sub, 1);
        assert_eq!(claims.username, "alice");
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
        };
        let token = signer.sign_jwt(&user).unwrap();
        let verifier = Auth::new("secret-B".to_string());
        let err = verifier.verify_jwt(&token).unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)), "got {err:?}");
    }

    #[test]
    fn jwt_garbage_returns_unauthorized() {
        let auth = Auth::new("test-secret".to_string());
        let err = auth.verify_jwt("not.a.jwt").unwrap_err();
        assert!(matches!(err, ApiError::Unauthorized(_)));
    }
}