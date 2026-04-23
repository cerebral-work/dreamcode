//! Feed model — owns the ring buffer of events, the cursor, the active
//! category filter, and (later) the poll task.

use crate::categories::{Category, CategoryFilter};
use crate::http::{ClientError, DreamHttpClient, WireEvent};
use gpui::{Context, EventEmitter, Task};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

pub const RING_CAP: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedState {
    Idle,
    Connected { total: usize },
    Error(String),
}

pub struct FeedModel {
    http: Arc<DreamHttpClient>,
    events: VecDeque<WireEvent>,
    cursor: Option<String>,
    categories: CategoryFilter,
    state: FeedState,
    paused: bool,
    poll_task: Option<Task<()>>,
}

pub enum FeedEvent {
    Updated,
}

impl EventEmitter<FeedEvent> for FeedModel {}

impl FeedModel {
    pub fn new(http: Arc<DreamHttpClient>) -> Self {
        Self {
            http,
            events: VecDeque::with_capacity(RING_CAP),
            cursor: None,
            categories: CategoryFilter::default(),
            state: FeedState::Idle,
            paused: false,
            poll_task: None,
        }
    }

    pub fn events(&self) -> &VecDeque<WireEvent> {
        &self.events
    }

    pub fn categories(&self) -> &CategoryFilter {
        &self.categories
    }

    pub fn toggle_category(&mut self, c: Category, cx: &mut Context<Self>) {
        self.categories.toggle(c);
        cx.emit(FeedEvent::Updated);
        cx.notify();
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        self.paused = paused;
        cx.emit(FeedEvent::Updated);
        cx.notify();
    }

    pub fn state(&self) -> &FeedState {
        &self.state
    }

    /// Push a batch of events. Oldest drop when buffer exceeds RING_CAP.
    pub fn push_batch(
        &mut self,
        batch: Vec<WireEvent>,
        next_cursor: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !batch.is_empty() {
            for ev in batch {
                if self.events.len() >= RING_CAP {
                    self.events.pop_front();
                }
                self.events.push_back(ev);
            }
        }
        if let Some(c) = next_cursor {
            self.cursor = Some(c);
        }
        let total = self.events.len();
        self.state = FeedState::Connected { total };
        cx.emit(FeedEvent::Updated);
        cx.notify();
    }

    pub fn set_error(&mut self, msg: String, cx: &mut Context<Self>) {
        self.state = FeedState::Error(msg);
        cx.emit(FeedEvent::Updated);
        cx.notify();
    }

    /// Filter a snapshot of events by the current CategoryFilter.
    pub fn visible(&self) -> Vec<&WireEvent> {
        self.events
            .iter()
            .filter(|ev| match Category::from_wire(&ev.category) {
                Some(c) => self.categories.is_enabled(c),
                None => false,
            })
            .collect()
    }

    // ── polling ───────────────────────────────────────────────────────
    //
    // Adaptive cadence: 1s after a poll that returned events, 3s after an
    // empty poll. Lives for the lifetime of the entity; on drop, the task
    // is cancelled. Paused → skip the call but keep the task alive.

    pub fn start_polling(&mut self, cx: &mut Context<Self>) {
        // Installs the poll task on `self.poll_task`. Called from inside a
        // `feed.update(cx, |m, cx| m.start_polling(cx))` — so `cx` is the
        // FeedModel context that already owns the mutable lease on self,
        // and we set the field directly instead of re-entering via
        // `entity.update(...)` (which would double-lease-panic).
        let task = cx.spawn(async move |this, cx| {
            let mut interval = Duration::from_millis(1_000);
            loop {
                cx.background_executor().timer(interval).await;

                let Ok((http, after, cats, paused)) = this.read_with(cx, |m, _| {
                    (
                        m.http.clone(),
                        m.cursor.clone(),
                        m.categories.as_query(),
                        m.paused,
                    )
                }) else {
                    return; // entity gone
                };

                if paused {
                    interval = Duration::from_millis(3_000);
                    continue;
                }

                let result = http.recent(after.as_deref(), 100, &cats).await;
                let new_interval = match &result {
                    Ok(resp) if !resp.events.is_empty() => Duration::from_millis(1_000),
                    _ => Duration::from_millis(3_000),
                };
                if this
                    .update(cx, |m, cx| match result {
                        Ok(resp) => {
                            m.push_batch(resp.events, resp.next_after, cx);
                        }
                        Err(ClientError::Transport) => {
                            m.set_error(
                                "reverie daemon unreachable — retrying".to_string(),
                                cx,
                            );
                        }
                        Err(ClientError::Status(n)) => {
                            m.set_error(
                                format!("reverie returned HTTP {n} — retrying"),
                                cx,
                            );
                        }
                    })
                    .is_err()
                {
                    return; // entity gone
                }
                interval = new_interval;
            }
        });
        self.poll_task = Some(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::WireEvent;
    use gpui::{AppContext as _, TestAppContext};
    use http_client::FakeHttpClient;
    use serde_json::json;

    fn fake_client() -> Arc<DreamHttpClient> {
        let http = FakeHttpClient::create(|_req| async move {
            Ok(http_client::Response::builder()
                .status(200)
                .body(http_client::AsyncBody::from("{}".to_string()))
                .unwrap())
        });
        DreamHttpClient::new(Some("http://example.test".into()), http)
    }

    fn ev(id: &str, category: &str, type_: &str) -> WireEvent {
        WireEvent {
            id: id.into(),
            ts_ms: 0,
            category: category.into(),
            type_: type_.into(),
            summary: "x".into(),
            fields: json!({}),
        }
    }

    #[gpui::test]
    fn ring_buffer_caps_at_ring_cap(cx: &mut TestAppContext) {
        let http = fake_client();
        let feed = cx.new(|_| FeedModel::new(http));
        feed.update(cx, |m, cx| {
            let batch: Vec<WireEvent> = (0..(RING_CAP + 10))
                .map(|i| ev(&format!("{i}-0"), "memory-io", "obs.capture"))
                .collect();
            m.push_batch(batch, Some("X-0".into()), cx);
            assert_eq!(m.events().len(), RING_CAP);
            // Oldest should have been dropped
            assert_eq!(m.events().front().unwrap().id, format!("{}-0", 10));
        });
    }

    #[gpui::test]
    fn visible_filters_by_category(cx: &mut TestAppContext) {
        let http = fake_client();
        let feed = cx.new(|_| FeedModel::new(http));
        feed.update(cx, |m, cx| {
            m.push_batch(
                vec![
                    ev("1-0", "memory-io", "obs.capture"),
                    ev("2-0", "tx", "tx.commit"),
                    ev("3-0", "dream", "dream.phase"),
                ],
                None,
                cx,
            );
            // tx is off by default
            let ids: Vec<_> = m.visible().iter().map(|e| e.id.clone()).collect();
            assert_eq!(ids, vec!["1-0".to_string(), "3-0".to_string()]);
        });
    }

    #[gpui::test]
    fn toggle_category_updates_visible(cx: &mut TestAppContext) {
        let http = fake_client();
        let feed = cx.new(|_| FeedModel::new(http));
        feed.update(cx, |m, cx| {
            m.push_batch(vec![ev("1-0", "tx", "tx.commit")], None, cx);
            assert!(m.visible().is_empty());
            m.toggle_category(Category::Tx, cx);
            assert_eq!(m.visible().len(), 1);
        });
    }
}
