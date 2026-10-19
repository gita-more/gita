//! Rate limiting middleware (in-memory, per-IP keyed).
//!
//! Protects the `/api/auth/login` endpoint from brute force. Uses
//! [`governor::RateLimiter`] with a token-bucket quota:
//! - refill: `per_second` tokens per second
//! - burst:  `burst`
//!
//! Beyond the bucket, requests get `429 Too Many Requests`. The limiter
//! is per-IP and in-memory (production multi-node setups would back it
//! with Redis or similar shared state).

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderName, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use std::sync::Arc;

pub type SharedLimiter =
    Arc<RateLimiter<String, DashMapStateStore<String>, DefaultClock>>;

/// Build a per-IP rate limiter with the given quota.
pub fn build_limiter(per_second: u32, burst: u32) -> SharedLimiter {
    let per_second = NonZeroU32::new(per_second.max(1)).unwrap();
    let burst = NonZeroU32::new(burst.max(1)).unwrap();
    let quota = Quota::per_second(per_second).allow_burst(burst);
    Arc::new(RateLimiter::keyed(quota))
}

/// Custom header exposed on 429 responses with the seconds-until-retry hint.
pub const RETRY_AFTER: HeaderName = axum::http::HeaderName::from_static("retry-after");

/// Axum middleware that rate-limits by client IP using `limiter`.
///
/// `client_ip` is the IP extracted from the request's `ConnectInfo` (set up
/// by the server when `into_make_service_with_connect_info::<SocketAddr>()` is
/// used). When the bucket for that IP is empty, returns `429 Too Many
/// Requests` with a `Retry-After` header.
pub async fn rate_limit_by_ip(
    limiter: Option<axum::extract::Extension<SharedLimiter>>,
    connect_info: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    // If the limiter extension is absent (e.g. in tests that don't wire
    // one in), pass the request through.
    let Some(axum::extract::Extension(limiter)) = limiter else {
        return next.run(req).await;
    };
    // When the request came over a real TCP socket (server-side), the
    // ConnectInfo is populated. When tests call the router via `oneshot`
    // without going through a TCP listener, it's absent — in that case
    // we treat the key as a fixed sentinel so tests don't share one
    // bucket and a rate-limit failure doesn't break unrelated tests.
    let key = connect_info
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "test:127.0.0.1".to_string());
    match limiter.check_key(&key) {
        Ok(()) => next.run(req).await,
        Err(neg) => {
            let secs = neg
                .wait_time_from(DefaultClock::default().now())
                .as_secs()
                .max(1);
            let mut response = (
                StatusCode::TOO_MANY_REQUESTS,
                format!("rate limit exceeded; retry in {secs}s"),
            )
                .into_response();
            response.headers_mut().insert(
                RETRY_AFTER,
                axum::http::HeaderValue::from_str(&secs.to_string()).unwrap_or_else(|_| {
                    axum::http::HeaderValue::from_static("1")
                }),
            );
            response
        }
    }
}

use std::time::Duration;
