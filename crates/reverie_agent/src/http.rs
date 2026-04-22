use anyhow::{Result, anyhow};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_BASE_URL: &str = "http://localhost:7437";

pub struct ReverieHttpClient {
    base_url: String,
    http: Arc<dyn HttpClient>,
    project: String,
    first_fail_logged: AtomicBool,
}

#[derive(Debug, Clone)]
pub struct SmartContext {
    pub content: String,
}

#[derive(Deserialize)]
struct SmartContextResponse {
    #[serde(default)]
    context: String,
}

#[derive(Serialize)]
struct PassiveCaptureBody<'a> {
    session_id: &'a str,
    content: &'a str,
    project: &'a str,
    source: &'a str,
}

impl ReverieHttpClient {
    pub fn new(
        base_url: Option<String>,
        project: String,
        http: Arc<dyn HttpClient>,
    ) -> Arc<Self> {
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Arc::new(Self {
            base_url,
            http,
            project,
            first_fail_logged: AtomicBool::new(false),
        })
    }

    pub fn project(&self) -> &str {
        &self.project
    }

    fn note_first_fail(&self, err: &dyn std::fmt::Display) {
        if self
            .first_fail_logged
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            log::info!(
                "reverie daemon unreachable at {}: {}. Continuing without memory. Start reveried or set REVERIE_URL.",
                self.base_url,
                err,
            );
        } else {
            log::debug!("reverie request failed (suppressed after first): {err}");
        }
    }

    pub async fn smart_context(&self, query: &str) -> Result<Option<SmartContext>> {
        let uri = build_smart_context_uri(&self.base_url, query, &self.project);
        let mut response = match self.http.get(&uri, AsyncBody::empty(), false).await {
            Ok(r) => r,
            Err(e) => {
                self.note_first_fail(&e);
                return Ok(None);
            }
        };
        if !response.status().is_success() {
            self.note_first_fail(&format!("HTTP {}", response.status()));
            return Ok(None);
        }
        let mut body_text = String::new();
        if let Err(e) = response.body_mut().read_to_string(&mut body_text).await {
            self.note_first_fail(&format!("body read error: {e}"));
            return Ok(None);
        }
        let body: SmartContextResponse = match serde_json::from_str(&body_text) {
            Ok(b) => b,
            Err(e) => {
                self.note_first_fail(&format!("parse error: {e}"));
                return Ok(None);
            }
        };
        let trimmed = body.context.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        // Reveried returns this sentinel when /context/smart has no hits
        // for the requested project. Treat it like an empty result so the
        // host doesn't burn a UI breadcrumb on a no-op retrieval.
        if trimmed == "No previous session memories found." {
            return Ok(None);
        }
        Ok(Some(SmartContext { content: body.context }))
    }

    pub async fn save_passive(
        &self,
        session_id: &str,
        content: &str,
        source: &str,
    ) -> Result<()> {
        let uri = format!("{}/observations/passive", self.base_url);
        let body_obj = PassiveCaptureBody {
            session_id,
            content,
            project: &self.project,
            source,
        };
        let body_json = serde_json::to_string(&body_obj)
            .map_err(|e| anyhow!("serialize passive capture body: {e}"))?;
        match self
            .http
            .post_json(&uri, AsyncBody::from(body_json))
            .await
        {
            Ok(response) if response.status().is_success() => Ok(()),
            Ok(response) => {
                self.note_first_fail(&format!("save_passive HTTP {}", response.status()));
                Ok(())
            }
            Err(e) => {
                self.note_first_fail(&e);
                Ok(())
            }
        }
    }
}

fn build_smart_context_uri(base_url: &str, query: &str, project: &str) -> String {
    format!(
        "{}/context/smart?q={}&project={}",
        base_url,
        urlencoding(query),
        urlencoding(project),
    )
}

// Minimal percent-encoding for query values — avoid pulling in a full url
// crate just for two params. We only need to escape characters that would
// break a URL (`&`, `=`, `#`, `%`, space, non-ASCII). Reverie's handlers
// are tolerant of raw UTF-8 but a safe encoder keeps the tests
// deterministic.
fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
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
