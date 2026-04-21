use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const DEFAULT_BASE_URL: &str = "http://localhost:7437";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct ReverieHttpClient {
    base_url: String,
    http: reqwest::Client,
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
    pub fn new(base_url: Option<String>, project: String) -> Result<Arc<Self>> {
        let base_url = base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let http = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()?;
        Ok(Arc::new(Self {
            base_url,
            http,
            project,
            first_fail_logged: AtomicBool::new(false),
        }))
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

    pub async fn smart_context(&self, _query: &str) -> Result<Option<SmartContext>> {
        Ok(None)
    }

    pub async fn save_passive(
        &self,
        _session_id: &str,
        _content: &str,
        _source: &str,
    ) -> Result<()> {
        Ok(())
    }
}
