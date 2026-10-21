use anyhow::{Context, Result};
use serde_json::Value;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub timeout_secs: u64,
    pub retries: usize,
    pub backoff_base_millis: u64,
    pub user_agent: Option<String>,
}

impl RetryPolicy {
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            timeout_secs,
            retries: 3,
            backoff_base_millis: 1_000,
            user_agent: None,
        }
    }

    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    #[cfg(test)]
    fn fast(timeout_secs: u64) -> Self {
        Self {
            timeout_secs,
            retries: 3,
            backoff_base_millis: 1,
            user_agent: None,
        }
    }
}

pub fn get_json(url: &str, params: &[(&str, String)], timeout_secs: u64) -> Result<Value> {
    get_json_with_policy(url, params, &RetryPolicy::new(timeout_secs))
}

pub fn get_json_with_policy(
    url: &str,
    params: &[(&str, String)],
    policy: &RetryPolicy,
) -> Result<Value> {
    retry_get(url, params, policy, |response| {
        response
            .json()
            .with_context(|| format!("failed to decode JSON response from {url}"))
    })
}

pub fn get_text(url: &str, params: &[(&str, String)], timeout_secs: u64) -> Result<String> {
    get_text_with_policy(url, params, &RetryPolicy::new(timeout_secs))
}

pub fn get_text_with_policy(
    url: &str,
    params: &[(&str, String)],
    policy: &RetryPolicy,
) -> Result<String> {
    retry_get(url, params, policy, |response| {
        response
            .text()
            .with_context(|| format!("failed to decode text response from {url}"))
    })
}

fn retry_get<T>(
    url: &str,
    params: &[(&str, String)],
    policy: &RetryPolicy,
    decode: impl Fn(reqwest::blocking::Response) -> Result<T>,
) -> Result<T> {
    let retries = policy.retries.max(1);
    for attempt in 0..retries {
        let result = (|| -> Result<T> {
            let mut builder = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(policy.timeout_secs));
            if let Some(user_agent) = &policy.user_agent {
                builder = builder.user_agent(user_agent);
            }
            let response = builder
                .build()
                .context("failed to build HTTP client")?
                .get(url)
                .query(params)
                .send()
                .with_context(|| format!("GET {url} failed"))?
                .error_for_status()
                .with_context(|| format!("GET {url} returned an error status"))?;
            decode(response)
        })();
        match result {
            Ok(value) => return Ok(value),
            Err(err) if attempt + 1 == retries => return Err(err),
            Err(_) => thread::sleep(backoff(attempt, policy.backoff_base_millis)),
        }
    }
    unreachable!("retry loop always returns")
}

fn backoff(attempt: usize, base_millis: u64) -> Duration {
    Duration::from_millis(base_millis.saturating_mul(1_u64 << attempt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn retries_transient_json_failures() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_thread = attempts.clone();
        let handle = std::thread::spawn(move || {
            for mut stream in listener.incoming().take(2).flatten() {
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf);
                let attempt = attempts_for_thread.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    let _ = stream.write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n",
                    );
                } else {
                    let body = br#"{"ok":true}"#;
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(body);
                }
            }
        });
        let payload =
            get_json_with_policy(&format!("http://{addr}/test"), &[], &RetryPolicy::fast(2))
                .unwrap();
        handle.join().unwrap();
        assert_eq!(payload.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
