//! Delivering a webhook to its receiver.
//!
//! A webhook is a promise the server makes to a third party: "when this
//! happens, I will tell you". The delivery is the one part of that
//! promise the server controls, and it is where the failures live:
//!
//! * the receiver is down, or the network between is broken;
//! * the receiver answers, but slowly — a client with no timeout waits
//!   forever;
//! * the receiver answers with an error.
//!
//! None of those may block the server. A webhook delivery runs in a
//! background task, and a background task that never finishes is a task
//! that a shutdown waits for forever — one unreachable webhook URL
//! would keep the process from stopping.
//!
//! So a delivery has a per-attempt timeout, and a bounded number of
//! attempts. The retry is what makes the promise worth something: a
//! receiver that was briefly down usually comes back.

use crate::sign_webhook_payload;

/// How long one delivery attempt may take before it is abandoned.
///
/// Without this, a receiver that accepts the connection and then never
/// answers holds the task open forever.
const ATTEMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How many attempts a delivery gets in total.
const MAX_ATTEMPTS: u32 = 3;

/// How long to wait between attempts.
///
/// Doubles per attempt, so the three attempts are spread over roughly
/// three seconds. Long enough for a receiver that is restarting, short
/// enough that a broken hook does not hold up the shutdown.
const BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

/// The outcome of a delivery attempt.
#[derive(Debug)]
pub enum DeliveryError {
    /// The request could not be made at all: the receiver refused the
    /// connection, the name did not resolve, the TLS handshake failed.
    Transport(String),
    /// The receiver answered, but with a status that is not a success.
    Status(u16),
    /// The attempt took longer than [`ATTEMPT_TIMEOUT`].
    Timeout,
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeliveryError::Transport(e) => write!(f, "transport error: {e}"),
            DeliveryError::Status(code) => write!(f, "the receiver answered {code}"),
            DeliveryError::Timeout => write!(f, "the receiver did not answer in time"),
        }
    }
}

impl std::error::Error for DeliveryError {}

/// Deliver one webhook payload, retrying a bounded number of times.
///
/// The body is signed with the hook's secret when there is one, and the
/// signature travels in `x-gita-signature-256`. A hook without a secret
/// is still delivered, but unsigned.
pub async fn deliver(
    url: &str,
    event: &str,
    secret: &str,
    body: &str,
) -> Result<(), DeliveryError> {
    let client = reqwest::Client::builder()
        // The client-level timeout bounds the whole request, connection
        // setup included; the per-attempt timeout below bounds the wait
        // for the response.
        .timeout(ATTEMPT_TIMEOUT)
        .build()
        .map_err(|e| DeliveryError::Transport(e.to_string()))?;

    let mut backoff = BACKOFF;
    let mut last: Option<DeliveryError> = None;

    for attempt in 1..=MAX_ATTEMPTS {
        let outcome = attempt_once(&client, url, event, secret, body).await;
        match outcome {
            Ok(()) => return Ok(()),
            Err(e) => {
                // A 4xx is the receiver telling us the request itself is
                // wrong. Repeating it unchanged would produce the same
                // answer, so only the last attempt is worth making.
                let is_client_error =
                    matches!(e, DeliveryError::Status(code) if (400..500).contains(&code));
                last = Some(e);
                if is_client_error || attempt == MAX_ATTEMPTS {
                    break;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF * 8);
            }
        }
    }

    Err(last.expect("at least one attempt was made"))
}

/// Make one delivery attempt.
async fn attempt_once(
    client: &reqwest::Client,
    url: &str,
    event: &str,
    secret: &str,
    body: &str,
) -> Result<(), DeliveryError> {
    let mut req = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-gita-event", event);

    // Sign the body with the hook's secret so the receiver can verify it
    // came from us. A hook without a secret is still delivered, but
    // unsigned.
    if !secret.is_empty() {
        let signature = sign_webhook_payload(secret, body);
        req = req.header("x-gita-signature-256", format!("sha256={signature}"));
    }

    let response = tokio::time::timeout(ATTEMPT_TIMEOUT, req.body(body.to_string()).send())
        .await
        .map_err(|_| DeliveryError::Timeout)?
        .map_err(|e| {
            if e.is_timeout() {
                DeliveryError::Timeout
            } else {
                DeliveryError::Transport(e.to_string())
            }
        })?;

    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(DeliveryError::Status(status.as_u16()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Start a receiver on a port the OS picks, returning its URL and
    /// the counter of requests it received.
    ///
    /// The request is read but its body is not needed: the tests are
    /// about the delivery's behaviour, not its payload.
    async fn start_receiver(status: u16) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();

        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = vec![0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let _ = socket
                        .write_all(
                            format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\n\r\n")
                                .as_bytes(),
                        )
                        .await;
                });
            }
        });

        (format!("http://{addr}/hook"), calls)
    }

    #[tokio::test]
    async fn a_successful_delivery_is_accepted() {
        let (url, calls) = start_receiver(200).await;
        deliver(&url, "push", "", "{}")
            .await
            .expect("the delivery must succeed");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one attempt, no retry");
    }

    #[tokio::test]
    async fn a_server_error_is_retried() {
        let (url, calls) = start_receiver(500).await;
        let err = deliver(&url, "push", "", "{}")
            .await
            .expect_err("a 500 is not a success");
        assert!(matches!(err, DeliveryError::Status(500)), "got {err:?}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            MAX_ATTEMPTS as usize,
            "a 5xx is worth retrying, and the delivery gave up after {} attempts",
            MAX_ATTEMPTS
        );
    }

    #[tokio::test]
    async fn a_client_error_is_not_retried() {
        let (url, calls) = start_receiver(404).await;
        let err = deliver(&url, "push", "", "{}")
            .await
            .expect_err("a 404 is not a success");
        assert!(matches!(err, DeliveryError::Status(404)), "got {err:?}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a 4xx means the request itself is wrong; repeating it changes nothing"
        );
    }

    #[tokio::test]
    async fn a_refused_connection_is_reported() {
        // Bind and immediately drop the listener, so the port is almost
        // certainly closed.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let err = deliver(&format!("http://{addr}/hook"), "push", "", "{}")
            .await
            .expect_err("nothing is listening");
        assert!(
            matches!(err, DeliveryError::Transport(_)),
            "a refused connection is a transport error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn a_hook_without_a_secret_is_delivered_unsigned() {
        let (url, calls) = start_receiver(200).await;
        deliver(&url, "push", "", "{}")
            .await
            .expect("the delivery must succeed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
