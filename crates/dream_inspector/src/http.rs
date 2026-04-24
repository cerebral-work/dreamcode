//! HTTP client for reveried `GET /events/recent`. Mirrors the resilience
//! pattern from `reverie_agent::http::ReverieHttpClient` — never propagates
//! transport errors; logs at info on first failure, debug thereafter.

use anyhow::{Result, anyhow};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient};
use serde::Deserialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_BASE_URL: &str = "http://localhost:7437";

#[derive(Debug, Clone, Deserialize)]
pub struct WireEvent {
    pub id: String,
    pub ts_ms: u64,
    pub category: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub summary: String,
    #[serde(default)]
    pub fields: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecentResponse {
    pub events: Vec<WireEvent>,
    #[serde(default)]
    pub next_after: Option<String>,
}

pub struct DreamHttpClient {
    base_url: String,
    http: Arc<dyn HttpClient>,
    first_fail_logged: AtomicBool,
}

impl DreamHttpClient {
    pub fn new(base_url: Option<String>, http: Arc<dyn HttpClient>) -> Arc<Self> {
        Arc::new(Self {
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            http,
            first_fail_logged: AtomicBool::new(false),
        })
    }

    fn note_first_fail(&self, err: &dyn std::fmt::Display) {
        if self
            .first_fail_logged
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            log::info!(
                "reverie events endpoint unreachable at {}: {}. Panel shows a banner; polling continues.",
                self.base_url,
                err,
            );
        } else {
            log::debug!("reverie /events/recent failed (suppressed after first): {err}");
        }
    }

    fn reset_fail_flag(&self) {
        self.first_fail_logged.store(false, Ordering::SeqCst);
    }

    pub async fn recent(
        &self,
        after: Option<&str>,
        limit: usize,
        categories_csv: &str,
    ) -> Result<RecentResponse, ClientError> {
        let mut uri = format!("{}/events/recent?limit={limit}", self.base_url);
        if let Some(a) = after
            && !a.is_empty()
        {
            uri.push_str(&format!("&after={}", urlencoding(a)));
        }
        if !categories_csv.is_empty() {
            uri.push_str(&format!("&categories={}", urlencoding(categories_csv)));
        }

        let mut response = match self.http.get(&uri, AsyncBody::empty(), false).await {
            Ok(r) => r,
            Err(e) => {
                self.note_first_fail(&e);
                return Err(ClientError::Transport);
            }
        };
        let status = response.status();
        if !status.is_success() {
            self.note_first_fail(&format!("HTTP {}", status));
            return Err(ClientError::Status(status.as_u16()));
        }
        let mut body = String::new();
        if let Err(e) = response.body_mut().read_to_string(&mut body).await {
            self.note_first_fail(&format!("body read: {e}"));
            return Err(ClientError::Transport);
        }
        let parsed: RecentResponse = serde_json::from_str(&body)
            .map_err(|e| anyhow!("parse /events/recent: {e}"))
            .map_err(|_| ClientError::Transport)?;
        self.reset_fail_flag();
        Ok(parsed)
    }
}

#[derive(Debug, Clone)]
pub enum ClientError {
    Transport,
    Status(u16),
}

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(&mut out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use http_client::{FakeHttpClient, Method, Response};
    use std::sync::{Arc as StdArc, Mutex};

    #[test]
    fn recent_sends_correct_query() {
        let captured: StdArc<Mutex<Option<String>>> = StdArc::new(Mutex::new(None));
        let cap2 = captured.clone();
        let http = FakeHttpClient::create(move |req| {
            let cap2 = cap2.clone();
            async move {
                assert_eq!(req.method(), Method::GET);
                *cap2.lock().unwrap() = Some(req.uri().to_string());
                Ok(Response::builder()
                    .status(200)
                    .body(AsyncBody::from(
                        r#"{"events":[],"next_after":null}"#.to_string(),
                    ))
                    .unwrap())
            }
        });
        let client = DreamHttpClient::new(Some("http://example.test".into()), http);
        block_on(client.recent(Some("1-0"), 50, "memory-io,dream")).unwrap();

        let uri = captured.lock().unwrap().clone().unwrap();
        assert!(uri.contains("/events/recent"), "{uri}");
        assert!(uri.contains("limit=50"), "{uri}");
        assert!(uri.contains("after=1-0"), "{uri}");
        assert!(uri.contains("categories=memory-io,dream"), "{uri}");
    }

    #[test]
    fn recent_parses_body() {
        let http = FakeHttpClient::create(|_req| async move {
            Ok(Response::builder()
                .status(200)
                .body(AsyncBody::from(
                    r###"{"events":[{"id":"1-0","ts_ms":1,"category":"memory-io","type":"obs.capture","summary":"x","fields":{}}],"next_after":"1-0"}"###.to_string(),
                ))
                .unwrap())
        });
        let client = DreamHttpClient::new(Some("http://example.test".into()), http);
        let r = block_on(client.recent(None, 10, "")).unwrap();
        assert_eq!(r.events.len(), 1);
        assert_eq!(r.events[0].type_, "obs.capture");
        assert_eq!(r.next_after.as_deref(), Some("1-0"));
    }

    #[test]
    fn recent_returns_transport_error_on_connection_failure() {
        let http = FakeHttpClient::create(|_req| async move {
            Err(anyhow::anyhow!("connection refused"))
        });
        let client = DreamHttpClient::new(Some("http://example.test".into()), http);
        let r = block_on(client.recent(None, 10, ""));
        assert!(matches!(r, Err(ClientError::Transport)));
    }

    #[test]
    fn recent_returns_status_error_on_5xx() {
        let http = FakeHttpClient::create(|_req| async move {
            Ok(Response::builder()
                .status(503)
                .body(AsyncBody::from(r#"{"error":"redis"}"#.to_string()))
                .unwrap())
        });
        let client = DreamHttpClient::new(Some("http://example.test".into()), http);
        let r = block_on(client.recent(None, 10, ""));
        assert!(matches!(r, Err(ClientError::Status(503))));
    }
}
