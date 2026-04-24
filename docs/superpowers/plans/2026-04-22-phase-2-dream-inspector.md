# Dream Inspector (Phase 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bottom-dock panel in dreamcode (Zed fork) that shows a live, filterable feed of reverie's memory-related events, by adding one new HTTP route to reveried and a new `dream_inspector` crate to dreamcode.

**Architecture:** Reveried gets a `GET /events/recent` route that does XRANGE on `events:all` (Redis stream) and returns a stable, category-tagged JSON shape. Dreamcode gets a new crate whose panel entity polls that route, maintains a 500-event ring buffer, and renders one-line rows filtered by category. No Redis dependency in Zed.

**Tech Stack:**
- **reverie side**: Rust, axum, redis 1.2 (streams feature), serde, tokio.
- **dreamcode side**: Rust, gpui, workspace (Panel trait), http_client, settings, serde.

**Repositories:**
- reverie: `/Users/dennis/programming projects/reverie` — tasks prefixed `A`.
- dreamcode: `/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend` — tasks prefixed `B`.

**Spec:** `docs/superpowers/specs/2026-04-22-phase-2-dream-inspector-design.md` (this repo).

---

## File Structure

### reverie

```
crates/reveried/src/
├── events_routes.rs          # NEW — route, handlers, wire types, mapping
└── server/extra_routes.rs    # MODIFY — merge new router in build_full + build_handoff
```

### dreamcode

```
crates/dream_inspector/
├── Cargo.toml                # NEW
└── src/
    ├── dream_inspector.rs    # NEW — crate root, re-exports, init()
    ├── categories.rs         # NEW — Category enum, defaults, settings key
    ├── http.rs               # NEW — DreamHttpClient + wire types
    ├── feed.rs               # NEW — FeedModel (ring buffer + poll task)
    └── panel.rs              # NEW — DreamInspectorPanel (Render + Panel)

crates/zed/src/zed.rs         # MODIFY — register panel in initialize_panels
Cargo.toml                    # MODIFY — add crate to workspace members + deps
```

---

# Part A — reverie upstream (new `/events/recent` route)

Work in the reverie repo: `/Users/dennis/programming projects/reverie`.

## Task A1: Scaffold `events_routes.rs` with wire types

**Files:**
- Create: `crates/reveried/src/events_routes.rs`

- [ ] **Step A1.1: Create the file with wire shapes and a placeholder handler**

```rust
// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Christian M. Todie

//! `GET /events/recent` — paginated read of the `events:all` Redis stream,
//! translated into a stable, category-tagged JSON shape for UI clients
//! (dreamcode Dream Inspector panel). See
//! `docs/superpowers/specs/2026-04-22-phase-2-dream-inspector-design.md`
//! in the dreamcode repo for the contract.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

const STREAM_KEY: &str = "events:all";
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 200;

/// Stable, category-tagged shape returned by `GET /events/recent`. Byte
/// layout is the contract with the dreamcode Dream Inspector panel.
#[derive(Debug, Clone, Serialize)]
pub struct WireEvent {
    pub id: String,
    pub ts_ms: u64,
    pub category: &'static str,
    #[serde(rename = "type")]
    pub type_: &'static str,
    pub summary: String,
    pub fields: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecentResponse {
    pub events: Vec<WireEvent>,
    pub next_after: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RecentParams {
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub categories: Option<String>,
}

#[derive(Clone)]
pub struct EventsRouteState {
    pub conn: Arc<Mutex<ConnectionManager>>,
}

/// Build the `/events` sub-router. Merge into the main reveried router
/// via `Router::merge`, same pattern as `dream_routes::router`.
pub fn router(state: EventsRouteState) -> Router {
    Router::new()
        .route("/events/recent", get(handle_recent))
        .with_state(state)
}

async fn handle_recent(
    State(_state): State<EventsRouteState>,
    Query(_params): Query<RecentParams>,
) -> Response {
    // Placeholder — filled in by later tasks.
    (StatusCode::NOT_IMPLEMENTED, "stub").into_response()
}
```

- [ ] **Step A1.2: Register the module**

Modify `crates/reveried/src/main.rs`. Find the `mod dream_routes;` line (currently line 19) and add immediately below:

```rust
mod events_routes;
```

- [ ] **Step A1.3: Compile check**

Run: `cargo check -p reveried`
Expected: compiles cleanly (`_state` and `_params` prefixed to silence unused warnings).

- [ ] **Step A1.4: Commit**

```bash
git add crates/reveried/src/events_routes.rs crates/reveried/src/main.rs
git commit -m "reveried: scaffold /events/recent route with wire types"
```

---

## Task A2: Event-kind → (category, type, summary) mapping

**Files:**
- Modify: `crates/reveried/src/events_routes.rs`

This is the pure, well-tested core. Input is an `event_kind` string (as stored by `redis_sink::StreamEntry`) plus a parsed `fields` JSON. Output is the `(category, type_, summary)` triple.

- [ ] **Step A2.1: Write failing tests first**

Add at the bottom of `crates/reveried/src/events_routes.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(event_kind: &str, actor: &str, target: &str, payload: serde_json::Value) -> Option<Mapped> {
        map_entry(event_kind, actor, target, &payload)
    }

    #[test]
    fn maps_observation_captured() {
        let got = m("observation_captured", "note", "42", json!({"topic_key":"arch/overview"})).unwrap();
        assert_eq!(got.category, "memory-io");
        assert_eq!(got.type_, "obs.capture");
        assert!(got.summary.contains("note"));
        assert!(got.summary.contains("arch/overview"));
    }

    #[test]
    fn maps_context_request_and_response() {
        let req = m("context_request", "context", "req-1", json!({})).unwrap();
        assert_eq!(req.category, "memory-io");
        assert_eq!(req.type_, "ctx.request");

        let resp = m("context_response", "context", "req-1", json!({"items":3,"latency_ms":12})).unwrap();
        assert_eq!(resp.category, "memory-io");
        assert_eq!(resp.type_, "ctx.response");
        assert!(resp.summary.contains("hits=3"));
    }

    #[test]
    fn maps_dream_phases() {
        assert_eq!(m("phase_start", "dream", "consolidate", json!({"run_id":"r-1"})).unwrap().category, "dream");
        assert_eq!(m("phase_end", "dream", "consolidate", json!({"run_id":"r-1","ok":true})).unwrap().category, "dream");
        let dpc = m("dream_phase_completed", "dream", "consolidate", json!({"run_id":"r-1","outcome":"Committed"})).unwrap();
        assert_eq!(dpc.category, "dream");
        assert!(dpc.summary.contains("Committed"));
    }

    #[test]
    fn maps_tx_family() {
        assert_eq!(m("tx_begin", "MemSave", "topic/x", json!({"tx_id":"t-1"})).unwrap().category, "tx");
        assert_eq!(m("tx_commit", "MemSave", "t-1", json!({"duration_ms":5})).unwrap().category, "tx");
        assert_eq!(m("tx_abort", "MemSave", "t-1", json!({"duration_ms":5,"reason":"boom"})).unwrap().category, "tx");
    }

    #[test]
    fn maps_coord_gate_permission() {
        assert_eq!(m("session_registered", "s-1", "worker", json!({})).unwrap().category, "coord");
        assert_eq!(m("lock_acquired", "h-1", "db", json!({"reason":"x"})).unwrap().category, "coord");
        assert_eq!(m("inbox_delivered", "a", "b", json!({"kind":"dispatch-order","subject":"go"})).unwrap().category, "coord");
        assert_eq!(m("dispatch_order_issued", "role", "o-1", json!({"subject":"go"})).unwrap().category, "coord");
        assert_eq!(m("peer_message", "a", "hello", json!({"subject":"hi"})).unwrap().category, "coord");
        assert_eq!(m("coord_order", "i", "k", json!({"subject":"x","resolved_by":null})).unwrap().category, "coord");
        assert_eq!(m("gate_decision", "rule", "hash", json!({"verdict":"Reject"})).unwrap().category, "gate");
        assert_eq!(m("permission_request", "req-1", "res", json!({})).unwrap().category, "permission");
        assert_eq!(m("permission_grant", "req-1", "AllowPersistent", json!({})).unwrap().category, "permission");
    }

    #[test]
    fn drops_unmapped_events() {
        // Per spec, these kinds are intentionally not surfaced.
        assert!(m("lock_released", "h", "db", json!({})).is_none());
        assert!(m("bio_frame", "src", "cycle", json!({})).is_none());
        assert!(m("work_request", "kind", "w-1", json!({})).is_none());
        assert!(m("work_completed", "kind", "w-1", json!({"success":true})).is_none());
    }

    #[test]
    fn unknown_event_kind_is_dropped() {
        assert!(m("xyzzy_future_event", "a", "b", json!({})).is_none());
    }
}
```

- [ ] **Step A2.2: Run the tests and confirm they fail**

Run: `cargo test -p reveried --test-threads=1 events_routes::tests`
Expected: FAIL — `map_entry` and `Mapped` don't exist yet.

- [ ] **Step A2.3: Implement the mapper**

Add this to `crates/reveried/src/events_routes.rs`, above the `#[cfg(test)] mod tests` block:

```rust
/// Output of the kind→wire mapping. Kept separate from `WireEvent` so
/// callers can fill in `id`, `ts_ms`, and `fields` from stream metadata
/// (they aren't a function of the kind alone).
pub(crate) struct Mapped {
    pub category: &'static str,
    pub type_: &'static str,
    pub summary: String,
}

fn s(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn map_entry(
    event_kind: &str,
    actor: &str,
    target: &str,
    payload: &serde_json::Value,
) -> Option<Mapped> {
    match event_kind {
        "observation_captured" => {
            let topic = s(&payload["topic_key"]);
            let summary = if topic.is_empty() {
                format!("{actor} · id={target}")
            } else {
                format!("{actor} · {topic}")
            };
            Some(Mapped { category: "memory-io", type_: "obs.capture", summary })
        }
        "context_request" => Some(Mapped {
            category: "memory-io",
            type_: "ctx.request",
            summary: format!("req={target}"),
        }),
        "context_response" => {
            let hits = s(&payload["items"]);
            let lat = s(&payload["latency_ms"]);
            Some(Mapped {
                category: "memory-io",
                type_: "ctx.response",
                summary: format!("hits={hits} · latency={lat}ms · req={target}"),
            })
        }
        "phase_start" => Some(Mapped {
            category: "dream",
            type_: "phase.start",
            summary: format!("{target} · run={}", s(&payload["run_id"])),
        }),
        "phase_end" => Some(Mapped {
            category: "dream",
            type_: "phase.end",
            summary: format!("{target} · ok={} · run={}", s(&payload["ok"]), s(&payload["run_id"])),
        }),
        "dream_phase_completed" => Some(Mapped {
            category: "dream",
            type_: "dream.phase",
            summary: format!("{target} → {}", s(&payload["outcome"])),
        }),
        "tx_begin" => Some(Mapped {
            category: "tx",
            type_: "tx.begin",
            summary: if target.is_empty() { actor.to_string() } else { format!("{actor} · {target}") },
        }),
        "tx_commit" => Some(Mapped {
            category: "tx",
            type_: "tx.commit",
            summary: format!("{actor} · {}ms", s(&payload["duration_ms"])),
        }),
        "tx_abort" => Some(Mapped {
            category: "tx",
            type_: "tx.abort",
            summary: format!("{actor} · reason={}", s(&payload["reason"])),
        }),
        "session_registered" => Some(Mapped {
            category: "coord",
            type_: "session.registered",
            summary: format!("{actor} · role={target}"),
        }),
        "session_deregistered" => Some(Mapped {
            category: "coord",
            type_: "session.deregistered",
            summary: actor.to_string(),
        }),
        "lock_acquired" => Some(Mapped {
            category: "coord",
            type_: "lock.acquired",
            summary: format!("{target} by {actor} ({})", s(&payload["reason"])),
        }),
        "inbox_delivered" => Some(Mapped {
            category: "coord",
            type_: "inbox.delivered",
            summary: format!("{actor} → {target} · {}: {}", s(&payload["kind"]), s(&payload["subject"])),
        }),
        "dispatch_order_issued" => Some(Mapped {
            category: "coord",
            type_: "dispatch.order",
            summary: format!("{target} · role={actor} · {}", s(&payload["subject"])),
        }),
        "peer_message" => Some(Mapped {
            category: "coord",
            type_: "peer.message",
            summary: format!("{actor} · {target}: {}", s(&payload["subject"])),
        }),
        "coord_order" => Some(Mapped {
            category: "coord",
            type_: "coord.order",
            summary: format!("{actor} · {target}: {}", s(&payload["subject"])),
        }),
        "gate_decision" => Some(Mapped {
            category: "gate",
            type_: "gate.decision",
            summary: format!("{actor} → {} (hash={})", s(&payload["verdict"]), target.chars().take(8).collect::<String>()),
        }),
        "permission_request" => Some(Mapped {
            category: "permission",
            type_: "permission.request",
            summary: format!("{target} · req={actor}"),
        }),
        "permission_grant" => Some(Mapped {
            category: "permission",
            type_: "permission.grant",
            summary: format!("{target} · req={actor}"),
        }),
        // Intentionally dropped per spec: lock_released, bio_frame,
        // work_request, work_completed, and unknown future kinds.
        _ => None,
    }
}
```

- [ ] **Step A2.4: Run tests and confirm they pass**

Run: `cargo test -p reveried events_routes::tests`
Expected: PASS — all mapper tests green.

- [ ] **Step A2.5: Commit**

```bash
git add crates/reveried/src/events_routes.rs
git commit -m "reveried: event_kind to wire-shape mapper with unit tests"
```

---

## Task A3: XRANGE helper + RFC3339 → ms timestamp parsing

**Files:**
- Modify: `crates/reveried/src/events_routes.rs`

- [ ] **Step A3.1: Write failing tests for the ts parser**

Add to the `tests` module in `events_routes.rs`:

```rust
    #[test]
    fn parses_rfc3339_to_ms() {
        let t = parse_ts_ms("2026-04-22T11:18:58.012Z");
        assert!(t > 1_000_000_000_000, "expected a reasonable unix-ms, got {t}");
    }

    #[test]
    fn parses_rfc3339_missing_fraction() {
        let t = parse_ts_ms("2026-04-22T11:18:58Z");
        assert!(t > 1_000_000_000_000);
    }

    #[test]
    fn parses_invalid_ts_returns_zero() {
        assert_eq!(parse_ts_ms("not-a-date"), 0);
    }
```

- [ ] **Step A3.2: Run, confirm FAIL (function missing)**

Run: `cargo test -p reveried events_routes::tests::parses`
Expected: FAIL — `parse_ts_ms` undefined.

- [ ] **Step A3.3: Implement `parse_ts_ms`**

Add above the tests module:

```rust
fn parse_ts_ms(rfc3339: &str) -> u64 {
    // StreamEntry stamps UTC RFC3339; falls back to 0 on parse error
    // (future-proof against schema drift).
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| dt.timestamp_millis().max(0) as u64)
        .unwrap_or(0)
}
```

- [ ] **Step A3.4: Verify chrono is already a dep**

Run: `cargo tree -p reveried -i chrono 2>&1 | head -5`
Expected: chrono appears (already pulled in via reverie-store).

If NOT present, add to `crates/reveried/Cargo.toml` under `[dependencies]`: `chrono = { workspace = true }`.

- [ ] **Step A3.5: Run parser tests**

Run: `cargo test -p reveried events_routes::tests::parses`
Expected: PASS.

- [ ] **Step A3.6: Commit**

```bash
git add crates/reveried/src/events_routes.rs
git commit -m "reveried: RFC3339 -> unix-ms parser for events wire shape"
```

---

## Task A4: Wire the real handler (XRANGE + mapping + category filter)

**Files:**
- Modify: `crates/reveried/src/events_routes.rs`

- [ ] **Step A4.1: Replace the placeholder handler**

Replace the `async fn handle_recent` stub from Task A1 with:

```rust
async fn handle_recent(
    State(state): State<EventsRouteState>,
    Query(params): Query<RecentParams>,
) -> Response {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);

    // Parse `categories=a,b,c` into a set. Absent → no filter.
    let filter: Option<std::collections::HashSet<String>> =
        params.categories.as_deref().map(|s| {
            s.split(',').filter(|t| !t.is_empty()).map(|t| t.trim().to_string()).collect()
        });

    // Redis XRANGE: use `(X` syntax for exclusive-after, `-` for none.
    let start = match params.after.as_deref() {
        Some(id) if !id.is_empty() => format!("({id}"),
        _ => "-".to_string(),
    };

    let entries: Vec<(String, HashMap<String, String>)> = {
        use redis::AsyncCommands as _;
        let mut conn = state.conn.lock().await;
        match conn.xrange_count(STREAM_KEY, &start, "+", limit).await {
            Ok(v) => v,
            Err(e) => {
                tracing::info!(error = %e, "events:all XRANGE failed");
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({"error":"events stream unavailable"})),
                )
                    .into_response();
            }
        }
    };

    let mut out: Vec<WireEvent> = Vec::with_capacity(entries.len());
    let mut last_id: Option<String> = None;
    for (id, fields) in entries {
        last_id = Some(id.clone());
        let event_kind = fields.get("event_kind").cloned().unwrap_or_default();
        let actor = fields.get("actor").cloned().unwrap_or_default();
        let target = fields.get("target").cloned().unwrap_or_default();
        let ts = fields.get("ts").cloned().unwrap_or_default();
        let payload: serde_json::Value = fields
            .get("payload")
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or(serde_json::Value::Null);

        let Some(mapped) = map_entry(&event_kind, &actor, &target, &payload) else {
            continue;
        };
        if let Some(f) = filter.as_ref()
            && !f.contains(mapped.category)
        {
            continue;
        }

        out.push(WireEvent {
            id,
            ts_ms: parse_ts_ms(&ts),
            category: mapped.category,
            type_: mapped.type_,
            summary: mapped.summary,
            fields: payload,
        });
    }

    Json(RecentResponse {
        events: out,
        next_after: last_id,
    })
    .into_response()
}
```

- [ ] **Step A4.2: Compile check**

Run: `cargo check -p reveried`
Expected: compiles cleanly. If `AsyncCommands::xrange_count` resolution fails, confirm `redis` workspace feature `streams` is enabled (see Cargo.toml:47 in `crates/reverie-store`).

If reveried's own `Cargo.toml` doesn't already pull in `redis`, add to `crates/reveried/Cargo.toml` under `[dependencies]`:
```toml
redis = { version = "1.2", features = ["tokio-comp", "connection-manager", "streams"] }
```

- [ ] **Step A4.3: Write a handler-level test with a mock-backed ConnectionManager**

At the bottom of the `tests` module in `events_routes.rs`:

```rust
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt as _;

    /// Ephemeral ConnectionManager against a real local Redis. Skipped
    /// gracefully when Redis is not reachable, so CI without Redis still
    /// passes the compile-level checks.
    async fn maybe_conn() -> Option<ConnectionManager> {
        let url = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
        let client = redis::Client::open(url).ok()?;
        ConnectionManager::new(client).await.ok()
    }

    async fn push_entry(conn: &mut ConnectionManager, fields: &[(&str, &str)]) -> String {
        use redis::AsyncCommands as _;
        conn.xadd(STREAM_KEY, "*", fields).await.unwrap()
    }

    #[tokio::test]
    async fn handler_returns_mapped_events() {
        let Some(mut conn) = maybe_conn().await else {
            eprintln!("redis not reachable; skipping integration test");
            return;
        };
        // Clear stream so cursor math is predictable.
        let _: Result<(), _> = redis::cmd("DEL").arg(STREAM_KEY).query_async(&mut conn).await;

        push_entry(&mut conn, &[
            ("v","v1"),("ts","2026-04-22T11:18:58.012Z"),("service","test"),
            ("event_kind","observation_captured"),("actor","note"),("target","42"),
            ("payload", r#"{"topic_key":"arch/overview"}"#),
        ]).await;

        let state = EventsRouteState { conn: Arc::new(Mutex::new(conn)) };
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/events/recent").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body_bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let parsed: RecentResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].category, "memory-io");
        assert_eq!(parsed.events[0].type_, "obs.capture");
        assert!(parsed.events[0].summary.contains("arch/overview"));
        assert!(parsed.next_after.is_some());
    }

    #[tokio::test]
    async fn handler_filters_by_category() {
        let Some(mut conn) = maybe_conn().await else { return };
        let _: Result<(), _> = redis::cmd("DEL").arg(STREAM_KEY).query_async(&mut conn).await;

        push_entry(&mut conn, &[
            ("v","v1"),("ts","2026-04-22T11:18:58Z"),("service","test"),
            ("event_kind","observation_captured"),("actor","note"),("target","1"),("payload","{}"),
        ]).await;
        push_entry(&mut conn, &[
            ("v","v1"),("ts","2026-04-22T11:18:59Z"),("service","test"),
            ("event_kind","tx_commit"),("actor","MemSave"),("target","t-1"),("payload",r#"{"duration_ms":5}"#),
        ]).await;

        let state = EventsRouteState { conn: Arc::new(Mutex::new(conn)) };
        let app = router(state);
        let resp = app
            .oneshot(Request::get("/events/recent?categories=memory-io").body(Body::empty()).unwrap())
            .await
            .unwrap();

        let body_bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let parsed: RecentResponse = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(parsed.events.len(), 1, "{parsed:?}");
        assert_eq!(parsed.events[0].category, "memory-io");
    }
```

- [ ] **Step A4.4: Start Redis if needed, then run tests**

Check: `redis-cli ping` should print `PONG`. If not, start Redis: `brew services start redis` (macOS) or equivalent.

Run: `cargo test -p reveried events_routes`
Expected: PASS, including both integration tests.

- [ ] **Step A4.5: Commit**

```bash
git add crates/reveried/src/events_routes.rs crates/reveried/Cargo.toml
git commit -m "reveried: implement /events/recent XRANGE handler with category filter"
```

---

## Task A5: Merge the events router into the live server

**Files:**
- Modify: `crates/reveried/src/server/extra_routes.rs`
- Modify: `crates/reveried/src/main.rs` (add connection plumbing)

- [ ] **Step A5.1: Find where the redis `ConnectionManager` is constructed in main.rs**

Run: `grep -n 'ConnectionManager::new\|redis::Client::open' "/Users/dennis/programming projects/reverie/crates/reveried/src/main.rs"`

Expected: find existing Redis connection setup. We want to share that same `ConnectionManager` (or create a sibling) with the events router.

- [ ] **Step A5.2: Thread a connection into `extra_routes::build_full`**

Update `crates/reveried/src/server/extra_routes.rs`. Add an arg:

```rust
pub fn build_full(
    sched: Arc<DreamScheduler>,
    redis_conn: Option<Arc<tokio::sync::Mutex<redis::aio::ConnectionManager>>>,
) -> (Router, ExtraRoutesHandles) {
    let (journal_router, journal) = dream_journal::router();
    let (agents_router, agents_registry) = agents_register::router();
    let agents_consumer = agents_register::maybe_spawn_consumer(agents_registry);
    let dream_router = dream_routes::router(sched);

    let mut extra = journal_router.merge(agents_router).merge(dream_router);

    if let Some(conn) = redis_conn {
        let events_router = crate::events_routes::router(
            crate::events_routes::EventsRouteState { conn },
        );
        extra = extra.merge(events_router);
        tracing::info!("events: /events/recent enabled");
    }

    if let Some(wh_state) = webhook::state_from_env() {
        extra = extra.merge(webhook::router(wh_state));
        tracing::info!("webhook: /webhooks/github enabled (TOD-833)");
    }

    let handles = ExtraRoutesHandles {
        _journal: journal,
        _agents_consumer: agents_consumer,
    };
    (extra, handles)
}
```

And similarly add the arg to `build_handoff`:

```rust
pub fn build_handoff(
    sched: Arc<DreamScheduler>,
    redis_conn: Option<Arc<tokio::sync::Mutex<redis::aio::ConnectionManager>>>,
) -> Router {
    let mut extra = dream_routes::router(sched);
    if let Some(conn) = redis_conn {
        extra = extra.merge(crate::events_routes::router(
            crate::events_routes::EventsRouteState { conn },
        ));
    }
    if let Some(wh_state) = webhook::state_from_env() {
        extra = extra.merge(webhook::router(wh_state));
        tracing::info!("webhook: /webhooks/github enabled on handoff path (TOD-833)");
    }
    extra
}
```

- [ ] **Step A5.3: Pass the connection from main.rs**

In `crates/reveried/src/main.rs`, locate every call site of `build_full(sched)` and `build_handoff(sched)`. For each:

1. Before the call, obtain the connection. If a `ConnectionManager` is already created nearby, wrap it: `let redis_conn = Some(Arc::new(tokio::sync::Mutex::new(conn.clone())));`.
2. If not, and only if the current function already has a `redis_url: String` in scope: `let redis_conn = redis::Client::open(redis_url.as_str()).ok().and_then(|c| futures::executor::block_on(async { redis::aio::ConnectionManager::new(c).await.ok() })).map(|c| Arc::new(tokio::sync::Mutex::new(c)));`
3. Otherwise pass `None` — the `/events/recent` route is then simply absent (matches the existing pattern for webhook and agents).

Update each call site: `build_full(sched, redis_conn)` / `build_handoff(sched, redis_conn)`.

- [ ] **Step A5.4: Compile check**

Run: `cargo check -p reveried`
Expected: clean build. If there are existing test call sites of `build_full`/`build_handoff`, extend them with `None` as the second arg.

- [ ] **Step A5.5: Manual end-to-end**

Stop any running reveried, then:
```bash
cargo run -p reveried -- serve &
sleep 2
curl -sS localhost:7437/events/recent | jq '.'
```
Expected: `{"events":[], "next_after": null}` — or a populated list if events have fired since startup. Not a 404.

- [ ] **Step A5.6: Commit**

```bash
git add crates/reveried/src/server/extra_routes.rs crates/reveried/src/main.rs
git commit -m "reveried: mount /events/recent on serve and handoff paths"
```

---

# Part B — dreamcode (`crates/dream_inspector` + registration)

Work in dreamcode: `/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend`.

## Task B1: Scaffold the new crate

**Files:**
- Create: `crates/dream_inspector/Cargo.toml`
- Create: `crates/dream_inspector/src/dream_inspector.rs`
- Modify: `Cargo.toml` (root — add to `members` and `[workspace.dependencies]`)

- [ ] **Step B1.1: Create Cargo.toml**

Write `crates/dream_inspector/Cargo.toml`:

```toml
[package]
name = "dream_inspector"
version = "0.1.0"
edition.workspace = true
publish.workspace = true
license = "GPL-3.0-or-later"

[lib]
path = "src/dream_inspector.rs"

[features]
test-support = []

[dependencies]
anyhow.workspace = true
collections.workspace = true
futures.workspace = true
gpui.workspace = true
http_client.workspace = true
log.workspace = true
parking_lot.workspace = true
project.workspace = true
serde.workspace = true
serde_json.workspace = true
settings.workspace = true
smol.workspace = true
ui.workspace = true
workspace.workspace = true

[dev-dependencies]
gpui = { workspace = true, features = ["test-support"] }
```

- [ ] **Step B1.2: Create crate root**

Write `crates/dream_inspector/src/dream_inspector.rs`:

```rust
//! Dream Inspector — bottom-dock panel that shows a live feed of
//! reverie-daemon events (observations, retrievals, dream phases) by
//! polling the reveried `GET /events/recent` endpoint.
//!
//! See `docs/superpowers/specs/2026-04-22-phase-2-dream-inspector-design.md`
//! for the contract this crate implements.

pub mod categories;
pub mod feed;
pub mod http;
pub mod panel;

pub use panel::DreamInspectorPanel;

use gpui::App;

/// Called from `crates/zed/src/zed.rs` during app init to register the
/// panel's toggle action. Actual panel construction happens per-workspace
/// via [`DreamInspectorPanel::load`].
pub fn init(_cx: &mut App) {
    // Action registration is handled by the `actions!` macro in panel.rs;
    // this hook exists so the zed.rs init site has a stable surface even
    // if more global wiring is added later.
}
```

- [ ] **Step B1.3: Register in workspace**

In the root `Cargo.toml`, insert `"crates/dream_inspector",` into the alphabetical `members = [...]` list — specifically between `"crates/diagnostics"` and `"crates/docs_preview"`. Find the location with:

```bash
grep -n '"crates/diagnostics"\|"crates/docs_preview"' Cargo.toml
```

Add `"crates/dream_inspector",` between those two entries.

Also add to `[workspace.dependencies]` (search for the `reverie_agent = { path = ... }` line and add an adjacent entry):

```toml
dream_inspector = { path = "crates/dream_inspector" }
```

- [ ] **Step B1.4: Compile check**

Run: `cargo check -p dream_inspector`
Expected: compile fails — categories/feed/http/panel modules don't exist yet.

To unblock step-by-step TDD, temporarily add empty stubs:

```bash
mkdir -p "crates/dream_inspector/src"
for m in categories feed http panel; do
  printf "%s" "//! $m module — implemented in subsequent tasks." > "crates/dream_inspector/src/${m}.rs"
done
```

Run: `cargo check -p dream_inspector`
Expected: clean compile (empty modules).

- [ ] **Step B1.5: Commit**

```bash
git add Cargo.toml crates/dream_inspector/
git commit -m "dream_inspector: scaffold new crate"
```

---

## Task B2: `categories` module

**Files:**
- Modify: `crates/dream_inspector/src/categories.rs`

- [ ] **Step B2.1: Write failing tests**

Replace the stub with:

```rust
//! Category enum + default filter + serde round-trip for settings.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    MemoryIo,
    Dream,
    Tx,
    Coord,
    Gate,
    Permission,
}

impl Category {
    pub const ALL: [Category; 6] = [
        Category::MemoryIo,
        Category::Dream,
        Category::Tx,
        Category::Coord,
        Category::Gate,
        Category::Permission,
    ];

    pub fn wire_name(&self) -> &'static str {
        match self {
            Category::MemoryIo => "memory-io",
            Category::Dream => "dream",
            Category::Tx => "tx",
            Category::Coord => "coord",
            Category::Gate => "gate",
            Category::Permission => "permission",
        }
    }

    pub fn display_name(&self) -> &'static str {
        self.wire_name()
    }

    pub fn from_wire(s: &str) -> Option<Category> {
        Category::ALL.iter().copied().find(|c| c.wire_name() == s)
    }
}

#[derive(Debug, Clone)]
pub struct CategoryFilter {
    enabled: HashSet<Category>,
}

impl Default for CategoryFilter {
    fn default() -> Self {
        let mut enabled = HashSet::new();
        enabled.insert(Category::MemoryIo);
        enabled.insert(Category::Dream);
        Self { enabled }
    }
}

impl CategoryFilter {
    pub fn is_enabled(&self, c: Category) -> bool {
        self.enabled.contains(&c)
    }

    pub fn toggle(&mut self, c: Category) {
        if !self.enabled.insert(c) {
            self.enabled.remove(&c);
        }
    }

    /// Serialize to the comma-separated `categories=` query-string form
    /// used by `GET /events/recent`.
    pub fn as_query(&self) -> String {
        let mut names: Vec<&'static str> =
            self.enabled.iter().map(|c| c.wire_name()).collect();
        names.sort();
        names.join(",")
    }

    pub fn enabled_set(&self) -> &HashSet<Category> {
        &self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_memory_io_and_dream() {
        let f = CategoryFilter::default();
        assert!(f.is_enabled(Category::MemoryIo));
        assert!(f.is_enabled(Category::Dream));
        assert!(!f.is_enabled(Category::Tx));
        assert!(!f.is_enabled(Category::Coord));
    }

    #[test]
    fn toggle_flips_membership() {
        let mut f = CategoryFilter::default();
        assert!(!f.is_enabled(Category::Tx));
        f.toggle(Category::Tx);
        assert!(f.is_enabled(Category::Tx));
        f.toggle(Category::Tx);
        assert!(!f.is_enabled(Category::Tx));
    }

    #[test]
    fn as_query_is_stable_alpha() {
        let mut f = CategoryFilter::default();
        f.toggle(Category::Coord);
        // memory-io + dream + coord → sorted: coord,dream,memory-io
        assert_eq!(f.as_query(), "coord,dream,memory-io");
    }

    #[test]
    fn wire_name_round_trip() {
        for c in Category::ALL {
            assert_eq!(Category::from_wire(c.wire_name()), Some(c));
        }
        assert_eq!(Category::from_wire("nonsense"), None);
    }
}
```

- [ ] **Step B2.2: Run**

Run: `cargo test -p dream_inspector categories`
Expected: PASS.

- [ ] **Step B2.3: Commit**

```bash
git add crates/dream_inspector/src/categories.rs
git commit -m "dream_inspector: Category + CategoryFilter with default on memory-io and dream"
```

---

## Task B3: HTTP client

**Files:**
- Modify: `crates/dream_inspector/src/http.rs`

- [ ] **Step B3.1: Write the client + failing tests**

Replace the stub with:

```rust
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
```

- [ ] **Step B3.2: Run**

Run: `cargo test -p dream_inspector http`
Expected: PASS (4 tests).

- [ ] **Step B3.3: Commit**

```bash
git add crates/dream_inspector/src/http.rs
git commit -m "dream_inspector: HTTP client for /events/recent with FakeHttpClient tests"
```

---

## Task B4: FeedModel — ring buffer + category filter

**Files:**
- Modify: `crates/dream_inspector/src/feed.rs`

- [ ] **Step B4.1: Write the model + failing tests**

Replace the stub with:

```rust
//! Feed model — owns the ring buffer of events, the cursor, the active
//! category filter, and (later) the poll task.

use crate::categories::{Category, CategoryFilter};
use crate::http::{ClientError, DreamHttpClient, WireEvent};
use gpui::{Context, Entity, EventEmitter, Task};
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
    pub fn push_batch(&mut self, batch: Vec<WireEvent>, next_cursor: Option<String>, cx: &mut Context<Self>) {
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
            .filter(|ev| {
                match Category::from_wire(&ev.category) {
                    Some(c) => self.categories.is_enabled(c),
                    None => false,
                }
            })
            .collect()
    }

    // ── polling ───────────────────────────────────────────────────────
    //
    // Adaptive cadence: 1s after a poll that returned events, 3s after an
    // empty poll. Lives for the lifetime of the entity; on drop, the task
    // is cancelled. Paused → skip the call but keep the task alive.

    pub fn start_polling(entity: &Entity<Self>, cx: &mut Context<Self>) {
        let weak = entity.downgrade();
        let task = cx.spawn(async move |_this, cx| {
            let mut interval = Duration::from_millis(1_000);
            loop {
                cx.background_executor().timer(interval).await;
                let Some(this) = weak.upgrade() else { return };

                let (http, after, cats, paused) = match this.read_with(cx, |m, _| {
                    (
                        m.http.clone(),
                        m.cursor.clone(),
                        m.categories.as_query(),
                        m.paused,
                    )
                }) {
                    Ok(v) => v,
                    Err(_) => return, // entity gone
                };

                if paused {
                    interval = Duration::from_millis(3_000);
                    continue;
                }

                let result = http.recent(after.as_deref(), 100, &cats).await;
                let _ = this.update(cx, |m, cx| {
                    match result {
                        Ok(resp) => {
                            let got = resp.events.len();
                            m.push_batch(resp.events, resp.next_after, cx);
                            interval = if got > 0 {
                                Duration::from_millis(1_000)
                            } else {
                                Duration::from_millis(3_000)
                            };
                        }
                        Err(ClientError::Transport) => {
                            m.set_error(
                                "reverie daemon unreachable — retrying".to_string(),
                                cx,
                            );
                            interval = Duration::from_millis(3_000);
                        }
                        Err(ClientError::Status(n)) => {
                            m.set_error(
                                format!("reverie returned HTTP {n} — retrying"),
                                cx,
                            );
                            interval = Duration::from_millis(3_000);
                        }
                    }
                });
            }
        });
        // Install the task on the entity. Must update via the entity handle
        // since we're inside a Context<Self>::spawn above.
        entity.update(cx, |m, _| m.poll_task = Some(task));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::WireEvent;
    use gpui::TestAppContext;
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
```

- [ ] **Step B4.2: Run**

Run: `cargo test -p dream_inspector feed`
Expected: PASS (3 gpui tests).

- [ ] **Step B4.3: Commit**

```bash
git add crates/dream_inspector/src/feed.rs
git commit -m "dream_inspector: FeedModel ring buffer + category filter + adaptive poll"
```

---

## Task B5: Panel skeleton + Render

**Files:**
- Modify: `crates/dream_inspector/src/panel.rs`

- [ ] **Step B5.1: Replace the stub with a minimal renderable panel**

```rust
//! `DreamInspectorPanel` — the bottom-dock panel.
//!
//! Render pipeline:
//!  top    — category pill bar + pause/follow controls
//!  middle — virtualized feed rows (compact one-liner: time | type | summary)
//!  bottom — status line (connected · N events · idle | error banner)

use crate::categories::Category;
use crate::feed::{FeedEvent, FeedModel, FeedState};
use crate::http::DreamHttpClient;
use gpui::{
    Action, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, SharedString, Styled, Subscription, Window, actions, div, prelude::*,
    uniform_list,
};
use project::Project;
use std::sync::Arc;
use ui::{
    Button, Color, Divider, FluentBuilder, Icon, IconButton, IconName, IconSize, Label, LabelSize,
    Tooltip, h_flex, v_flex,
};
use workspace::Workspace;
use workspace::dock::{DockPosition, Panel, PanelEvent};

actions!(
    dream_inspector,
    [
        /// Toggle visibility of the Dream Inspector panel.
        Toggle
    ]
);

pub const PANEL_KEY: &str = "DreamInspectorPanel";

pub struct DreamInspectorPanel {
    feed: Entity<FeedModel>,
    focus_handle: FocusHandle,
    follow: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for DreamInspectorPanel {}

impl Focusable for DreamInspectorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl DreamInspectorPanel {
    pub fn new(
        _workspace: &Workspace,
        _project: Entity<Project>,
        cx: &mut Context<Self>,
    ) -> Self {
        let http_client: Arc<dyn http_client::HttpClient> = _workspace
            .project()
            .read(cx)
            .client()
            .http_client();
        let base_url = std::env::var("REVERIE_URL").ok();
        let dream_http = DreamHttpClient::new(base_url, http_client);

        let feed = cx.new(|_| FeedModel::new(dream_http));
        FeedModel::start_polling(&feed, cx);

        let subscription = cx.subscribe(&feed, |_this, _feed, _event: &FeedEvent, cx| {
            cx.notify();
        });

        Self {
            feed,
            focus_handle: cx.focus_handle(),
            follow: true,
            _subscriptions: vec![subscription],
        }
    }

    fn render_pills(&self, cx: &Context<Self>) -> AnyElement {
        let filter = self.feed.read(cx).categories().clone();
        let feed = self.feed.clone();
        h_flex()
            .gap_1()
            .children(Category::ALL.iter().map(|&c| {
                let enabled = filter.is_enabled(c);
                let feed = feed.clone();
                Button::new(SharedString::new_static(c.display_name()), c.display_name())
                    .label_size(LabelSize::Small)
                    .color(if enabled { Color::Success } else { Color::Muted })
                    .on_click(move |_, _, cx| {
                        feed.update(cx, |m, cx| m.toggle_category(c, cx));
                    })
            }))
            .into_any_element()
    }

    fn render_controls(&self, cx: &Context<Self>) -> AnyElement {
        let paused = self.feed.read(cx).is_paused();
        let feed = self.feed.clone();
        h_flex()
            .gap_1()
            .child(
                IconButton::new("dream-pause", if paused { IconName::Play } else { IconName::Pause })
                    .tooltip(|_, cx| Tooltip::simple("Pause polling", cx))
                    .on_click(move |_, _, cx| {
                        feed.update(cx, |m, cx| m.set_paused(!m.is_paused(), cx));
                    }),
            )
            .into_any_element()
    }

    fn render_feed(&self, cx: &Context<Self>) -> AnyElement {
        let snap: Vec<_> = self
            .feed
            .read(cx)
            .visible()
            .into_iter()
            .cloned()
            .collect();
        let count = snap.len();
        if count == 0 {
            return div()
                .flex_1()
                .items_center()
                .justify_center()
                .child(Label::new("Waiting for events…").color(Color::Muted))
                .into_any_element();
        }
        uniform_list(
            "dream-feed",
            count,
            move |range, _window, _cx| {
                range
                    .map(|i| {
                        let ev = &snap[i];
                        h_flex()
                            .gap_3()
                            .font_family("mono")
                            .child(
                                Label::new(format_time(ev.ts_ms))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(SharedString::from(ev.type_.clone()))
                                    .size(LabelSize::Small)
                                    .color(color_for_category(&ev.category)),
                            )
                            .child(
                                Label::new(SharedString::from(ev.summary.clone()))
                                    .size(LabelSize::Small),
                            )
                            .into_any_element()
                    })
                    .collect()
            },
        )
        .flex_1()
        .into_any_element()
    }

    fn render_status(&self, cx: &Context<Self>) -> AnyElement {
        let state = self.feed.read(cx).state().clone();
        let text: SharedString = match state {
            FeedState::Idle => "connecting…".into(),
            FeedState::Connected { total } => format!("connected · {total} events").into(),
            FeedState::Error(ref msg) => msg.clone().into(),
        };
        let color = match self.feed.read(cx).state() {
            FeedState::Error(_) => Color::Warning,
            _ => Color::Muted,
        };
        div()
            .p_1()
            .child(Label::new(text).size(LabelSize::Small).color(color))
            .into_any_element()
    }
}

fn format_time(ms: u64) -> SharedString {
    // HH:MM:SS in local time.
    let secs = (ms / 1000) as i64;
    let dt = chrono::DateTime::from_timestamp(secs, 0)
        .unwrap_or_else(|| chrono::DateTime::from_timestamp(0, 0).unwrap());
    dt.with_timezone(&chrono::Local)
        .format("%H:%M:%S")
        .to_string()
        .into()
}

fn color_for_category(wire: &str) -> Color {
    match wire {
        "memory-io" => Color::Info,
        "dream" => Color::Accent,
        "tx" => Color::Muted,
        "coord" | "gate" | "permission" => Color::Muted,
        _ => Color::Default,
    }
}

impl Render for DreamInspectorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .p_1()
                    .gap_2()
                    .child(self.render_pills(cx))
                    .child(div().flex_1())
                    .child(self.render_controls(cx)),
            )
            .child(Divider::horizontal())
            .child(self.render_feed(cx))
            .child(Divider::horizontal())
            .child(self.render_status(cx))
    }
}
```

Note: `chrono` is already a workspace dep (used elsewhere in Zed) and does not need to be added to this crate's Cargo.toml — it's transitively available. If the build complains, add `chrono.workspace = true` to `crates/dream_inspector/Cargo.toml`.

- [ ] **Step B5.2: Compile check (no panel-registration yet; impl Panel follows)**

Run: `cargo check -p dream_inspector`
Expected: compile may fail because `Render` + Panel aren't yet tied together — that's fine, next task adds `impl Panel`. If it complains about unused `Category` imports etc, leave them — B6 will use them.

Actually: `DreamInspectorPanel` already impls `Render`, `Focusable`, `EventEmitter<PanelEvent>`. Panel's supertraits are `Focusable + EventEmitter<PanelEvent> + Render + Sized`, all satisfied. The remaining required methods (position, icon, etc.) are added in B6. Until then, `impl Panel for …` simply doesn't exist — that's fine, this task compiles without it.

If `cargo check` fails, read the exact error before adding dependencies blindly. Common fix: add `chrono.workspace = true` to the crate Cargo.toml.

- [ ] **Step B5.3: Commit**

```bash
git add crates/dream_inspector/src/panel.rs crates/dream_inspector/Cargo.toml
git commit -m "dream_inspector: panel skeleton with feed rendering"
```

---

## Task B6: `impl Panel for DreamInspectorPanel` + load

**Files:**
- Modify: `crates/dream_inspector/src/panel.rs`

- [ ] **Step B6.1: Add the Panel impl and load()**

Append to `crates/dream_inspector/src/panel.rs`:

```rust
impl Panel for DreamInspectorPanel {
    fn persistent_name() -> &'static str {
        "DreamInspectorPanel"
    }

    fn panel_key() -> &'static str {
        PANEL_KEY
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Bottom)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        gpui::px(260.0)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Sparkle)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Dream Inspector")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(Toggle)
    }

    fn activation_priority(&self) -> u32 {
        5
    }
}

impl DreamInspectorPanel {
    pub async fn load(
        workspace: gpui::WeakEntity<Workspace>,
        mut cx: gpui::AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, _window, cx| {
            let project = workspace.project().clone();
            cx.new(|cx| DreamInspectorPanel::new(workspace, project, cx))
        })
    }
}
```

- [ ] **Step B6.2: Compile check**

Run: `cargo check -p dream_inspector`
Expected: clean compile.

- [ ] **Step B6.3: Commit**

```bash
git add crates/dream_inspector/src/panel.rs
git commit -m "dream_inspector: impl Panel + load() for bottom-dock registration"
```

---

## Task B7: Register in Zed startup

**Files:**
- Modify: `crates/zed/src/zed.rs`

- [ ] **Step B7.1: Add the import**

Near the top of `crates/zed/src/zed.rs`, where other panel crates are imported (e.g. `use terminal_view::terminal_panel::{self, TerminalPanel};`), add:

```rust
use dream_inspector::DreamInspectorPanel;
```

- [ ] **Step B7.2: Add dep**

Modify `crates/zed/Cargo.toml` — in `[dependencies]`, alphabetical, add:

```toml
dream_inspector.workspace = true
```

- [ ] **Step B7.3: Register in `initialize_panels`**

In `crates/zed/src/zed.rs`, find `fn initialize_panels` (around line 695). Add a new panel alongside the others:

```rust
let dream_inspector_panel = DreamInspectorPanel::load(workspace_handle.clone(), cx.clone());
```

And in the `futures::join!` block near the bottom of the function, add a line:

```rust
add_panel_when_ready(dream_inspector_panel, workspace_handle.clone(), cx.clone()),
```

- [ ] **Step B7.4: Register the Toggle action handler**

Find the block registering `terminal_panel::ToggleFocus` (around line 1121). Add a sibling handler:

```rust
workspace.register_action(
    |workspace, _: &dream_inspector::panel::Toggle, window, cx| {
        workspace.toggle_panel_focus::<DreamInspectorPanel>(window, cx);
    },
);
```

(Exact context: look for where `terminal_panel::ToggleFocus` is registered with `workspace.register_action(...)`. Mirror the pattern.)

- [ ] **Step B7.5: Build zed**

Run: `cargo build -p zed`
Expected: clean build (may take a few minutes).

- [ ] **Step B7.6: Commit**

```bash
git add Cargo.toml crates/zed/Cargo.toml crates/zed/src/zed.rs
git commit -m "zed: register DreamInspectorPanel in workspace init"
```

---

## Task B8: End-to-end smoke test

**Files:** none (runtime verification only).

- [ ] **Step B8.1: Ensure reveried is running with the new route**

From the **reverie** repo, if not already done:

```bash
cd "/Users/dennis/programming projects/reverie"
cargo build -p reveried
pkill -x reveried || true
./target/debug/reveried serve > /tmp/reveried.out 2>&1 &
sleep 2
curl -sS localhost:7437/events/recent | jq '.'
```

Expected: `{"events":[...],"next_after":...}` (even if empty).

- [ ] **Step B8.2: Trigger a memory event so the feed has content**

```bash
curl -sS -X POST http://localhost:7437/observations \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"smoke","type":"note","title":"smoke","content":"phase 2 smoke trigger","source":"smoke"}'
curl -sS localhost:7437/events/recent | jq '.events[] | {type, summary}'
```

Expected: at least one `obs.capture` event visible.

- [ ] **Step B8.3: Launch the freshly-built Zed**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
pkill -x zed || true
./target/debug/zed . &
```

- [ ] **Step B8.4: Verify the panel**

1. In Zed's command palette (`cmd-shift-p`), run `dream_inspector: Toggle`. The panel opens at the bottom.
2. See the smoke-trigger observation in the feed (category `memory-io`, type `obs.capture`).
3. Trigger another observation via curl (step B8.2 again); within ~1-3s the new row appears.
4. Click the `memory-io` pill; rows disappear. Click again; they come back.
5. Click the pause icon; no new rows appear on fresh curls. Unpause; catch-up happens on the next poll.
6. Kill reveried (`pkill -x reveried`); within 3s the banner appears: "reverie daemon unreachable — retrying". Restart reveried; banner clears on next successful poll.

- [ ] **Step B8.5: If all six checks pass, commit a smoke marker**

```bash
git commit --allow-empty -m "phase 2 smoke: all checks pass"
```

---

## Self-review checklist (for the plan author, run before handoff)

1. **Spec coverage**:
   - Upstream route (§2 of spec) → Tasks A1–A5. ✓
   - Panel crate structure (§3) → Task B1. ✓
   - Panel UI regions (§4) → Tasks B5 (render), B6 (Panel impl). ✓
   - Adaptive polling (§5) → Task B4. ✓
   - Error/empty/down states (§6) → Task B4 (state), B5 (render_status). ✓
   - Settings persistence — **gap**: the spec promised `agent.reverie.dream_inspector.categories` persistence; the plan leaves the filter as per-session-only. Mark as deferred post-smoke (v1.0 ships with ephemeral filter). Note it in the spec's "Deferred" section as a follow-up ticket after implementation.

2. **Placeholder scan**: no "TODO", "TBD", or vague instructions in task steps. Every code step shows the code.

3. **Type consistency**:
   - `WireEvent` shape matches between reverie (A1/A4) and dreamcode (B3). ✓
   - `RecentResponse` field names (`events`, `next_after`) match both sides. ✓
   - `Category::wire_name` strings match reverie's `Mapped.category` strings ("memory-io", "dream", "tx", "coord", "gate", "permission"). ✓
   - `CategoryFilter::as_query()` produces the CSV form the handler parses. ✓
   - `FeedModel::start_polling` signature (`entity: &Entity<Self>, cx: &mut Context<Self>`) matches the call site in `DreamInspectorPanel::new`. ✓

## Known deferrals (intentionally out of scope)

- SSE streaming (γ upgrade path) — entire effort.
- Event-detail drill-down (click a row → side sheet).
- Settings persistence of filter toggles — **revisit after smoke passes**.
- `uniform_list` may not give you perfect auto-scroll-to-latest; the spec's "follow mode" UX is sketched but the plan doesn't include a scroll-to-bottom button yet. Add in a follow-up if the panel feels off.
- Workspace state restore across Zed restarts — panel opens fresh each time.
