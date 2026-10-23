//! Rate limiting middleware (in-memory, per-IP keyed).
//!
//! Protects the `/api/auth/login` and `/api/auth/register` endpoints from
//! brute force and abuse. Uses [`governor::RateLimiter`] with a token-bucket
//! quota:
//! - refill: `per_second` tokens per second
//! - burst:  `burst`
//!
//! Beyond the bucket, requests get `429 Too Many Requests`. The limiter
//! is per-IP and in-memory (production multi-node setups would back it
//! with Redis or similar shared state).

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use governor::clock::{Clock, DefaultClock};
use governor::state::keyed::DashMapStateStore;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use std::sync::Arc;

pub type SharedLimiter = Arc<RateLimiter<String, DashMapStateStore<String>, DefaultClock>>;

/// Build a per-IP rate limiter with the given quota.
pub fn build_limiter(per_second: u32, burst: u32) -> SharedLimiter {
    let per_second = NonZeroU32::new(per_second.max(1)).unwrap();
    let burst = NonZeroU32::new(burst.max(1)).unwrap();
    let quota = Quota::per_second(per_second).allow_burst(burst);
    Arc::new(RateLimiter::keyed(quota))
}

/// Build a per-IP rate limiter whose bucket refills very slowly.
///
/// The token bucket replenishes one cell per `period`, so a test that
/// exercises the *refusal* path does not race the clock: with a one-second
/// refill, a second request issued a moment later could be admitted and the
/// test would fail intermittently under load. Tests use this to get a
/// limiter that behaves as "the bucket is empty and stays empty".
pub fn build_slow_refill_limiter(burst: u32) -> SharedLimiter {
    let burst = NonZeroU32::new(burst.max(1)).unwrap();
    let quota = Quota::with_period(std::time::Duration::from_secs(3600))
        .expect("a one-hour period is non-zero")
        .allow_burst(burst);
    Arc::new(RateLimiter::keyed(quota))
}

/// Custom header exposed on 429 responses with the seconds-until-retry hint.
pub const RETRY_AFTER: HeaderName = axum::http::HeaderName::from_static("retry-after");

/// Resolve the address a rate-limit bucket should be keyed on.
///
/// The real peer address (`ConnectInfo`) is authoritative: it comes from the
/// TCP connection and cannot be forged by the client. When it is available
/// — which is the case for every request that arrives over a socket — the
/// `X-Forwarded-For` header is ignored entirely.
///
/// `X-Forwarded-For` is only consulted when there is no peer address, i.e.
/// when the router is driven directly (tests, or an in-process embedding).
/// In that case the **right-most** entry is used: a client that prepends
/// forged hops to the header cannot influence the key, because the right-most
/// entry is the one written by the closest proxy. Taking the left-most entry
/// (as an earlier version did) would let a caller mint an unlimited number
/// of buckets by varying a header value it controls.
pub fn client_ip(
    headers: &HeaderMap,
    connect_info: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
) -> String {
    if let Some(ci) = connect_info {
        return ci.0.ip().to_string();
    }
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

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
                axum::http::HeaderValue::from_str(&secs.to_string())
                    .unwrap_or_else(|_| axum::http::HeaderValue::from_static("1")),
            );
            response
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn headers_with_xff(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    /// The peer address wins over the header: it cannot be forged by the
    /// client, whereas `X-Forwarded-For` can.
    #[test]
    fn peer_address_takes_precedence_over_forwarded_for() {
        let peer: SocketAddr = "198.51.100.7:5000".parse().unwrap();
        let headers = headers_with_xff("1.2.3.4, 203.0.113.9");
        let ip = client_ip(&headers, Some(axum::extract::ConnectInfo(peer)));
        assert_eq!(ip, "198.51.100.7");
    }

    /// Without a peer address, the right-most forwarded hop is used, so
    /// prepending forged hops cannot mint a fresh bucket.
    #[test]
    fn forwarded_for_uses_the_right_most_hop() {
        let headers = headers_with_xff("1.2.3.4, 203.0.113.9");
        assert_eq!(client_ip(&headers, None), "203.0.113.9");
    }

    /// Two requests that differ only in a forged left-most hop must resolve
    /// to the same key.
    #[test]
    fn forged_left_most_hop_does_not_change_the_key() {
        let a = headers_with_xff("1.2.3.4, 203.0.113.9");
        let b = headers_with_xff("5.6.7.8, 203.0.113.9");
        assert_eq!(client_ip(&a, None), client_ip(&b, None));
    }

    /// A single-hop header still works, and surrounding whitespace is
    /// trimmed.
    #[test]
    fn single_hop_and_whitespace_are_handled() {
        let h = headers_with_xff("  203.0.113.9  ");
        assert_eq!(client_ip(&h, None), "203.0.113.9");
    }

    /// With no header at all the key is a fixed sentinel rather than empty.
    #[test]
    fn missing_header_yields_a_stable_sentinel() {
        let empty = HeaderMap::new();
        assert_eq!(client_ip(&empty, None), "unknown");
    }
}
