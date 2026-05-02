# Phase 0 — MCP Context Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand reverie's existing `/mcp` SSE server from one tool (`search_memory`) to six, adding `smart_context`, `add_observation`, `add_observation_passive`, `dream_status`, and `dream_last_report`.

**Architecture:** Refactor the monolithic `mcp_sse.rs` into a module with one file per tool, sharing a uniform `Ctx`-based call signature. Extract inner functions from existing axum handlers in `reverie-store` so the MCP layer can reuse business logic without going through extractors. Plumb `Arc<DreamScheduler>` into `McpState` so dream tools can read scheduler state.

**Tech Stack:** Rust, axum, tokio, serde_json, tower (for `oneshot` test fixtures), `reverie_store::http::AppState`, `crate::dream_scheduler::DreamScheduler`.

**Spec:** `docs/superpowers/specs/2026-04-29-phase-0-mcp-context-server-design.md`

**Repo location:** All code changes are in `/Users/dennis/programming projects/reverie/`. The spec and plan live in dreamcode for project tracking.

**Build commands** (run from reverie repo root):
- `cargo test -p reveried` — reveried test suite
- `cargo test -p reverie-store` — store test suite
- `./script/clippy` — lint (or `cargo clippy --workspace --all-features`)

---

## File Structure

| Path | Action | Responsibility |
|---|---|---|
| `crates/reveried/src/mcp_sse.rs` | Delete (renamed) | (was: monolithic MCP SSE server) |
| `crates/reveried/src/mcp.rs` | Create (renamed from above + slim down) | Module root: `McpState`, `Ctx`, `RpcRequest`, `RpcCallError`, auth middleware, dispatcher, SSE helpers |
| `crates/reveried/src/mcp/search_memory.rs` | Create | Existing `search_memory` tool moved here |
| `crates/reveried/src/mcp/smart_context.rs` | Create | New: wraps `format_smart_context*` |
| `crates/reveried/src/mcp/add_observation.rs` | Create | New: wraps `add_observation_inner` |
| `crates/reveried/src/mcp/add_observation_passive.rs` | Create | New: wraps `passive_capture_inner` |
| `crates/reveried/src/mcp/dream_status.rs` | Create | New: wraps `DreamScheduler::status_snapshot` |
| `crates/reveried/src/mcp/dream_last_report.rs` | Create | New: wraps `DreamScheduler::last_report` |
| `crates/reveried/src/main.rs` | Modify (one line) | `pub mod mcp_sse;` → `pub mod mcp;` |
| `crates/reveried/src/server/extra_routes.rs` | Modify | Update `mcp_sse::` → `mcp::` imports; pass `Arc<DreamScheduler>` into `maybe_mcp_router` |
| `crates/reverie-store/src/http/mod.rs` | Modify | Extract `add_observation_inner` and `passive_capture_inner` from existing handlers |
| `crates/reveried/tests/mcp_tools.rs` | Create | Integration tests (one per new tool + envelope checks) |
| `crates/reverie/CHANGELOG.md` | Modify | Add `Unreleased / Added` entry |

---

## Task 1: Rename `mcp_sse.rs` → `mcp.rs` (no behavior change)

**Goal:** File rename + module declaration update. Zero behavior changes; existing 13 unit tests still pass verbatim.

**Files:**
- Rename: `crates/reveried/src/mcp_sse.rs` → `crates/reveried/src/mcp.rs`
- Modify: `crates/reveried/src/main.rs:28`
- Modify: `crates/reveried/src/server/extra_routes.rs:21,139`

- [ ] **Step 1: Move the file**

```bash
cd "/Users/dennis/programming projects/reverie"
git mv crates/reveried/src/mcp_sse.rs crates/reveried/src/mcp.rs
```

- [ ] **Step 2: Update module declaration in `main.rs`**

In `crates/reveried/src/main.rs:28`, change:

```rust
pub mod mcp_sse;
```

to:

```rust
pub mod mcp;
```

- [ ] **Step 3: Update import + call sites in `extra_routes.rs`**

In `crates/reveried/src/server/extra_routes.rs:21`, change:

```rust
use crate::{agents_register, dream_journal, dream_routes, mcp_sse, webhook};
```

to:

```rust
use crate::{agents_register, dream_journal, dream_routes, mcp, webhook};
```

In `crates/reveried/src/server/extra_routes.rs:139`, change:

```rust
    Some(mcp_sse::router(mcp_sse::McpState::new(state, token)))
```

to:

```rust
    Some(mcp::router(mcp::McpState::new(state, token)))
```

- [ ] **Step 4: Verify tests still pass**

Run: `cargo test -p reveried mcp`
Expected: All existing `mcp::tests::*` tests pass (13 tests).

- [ ] **Step 5: Verify clippy clean**

Run: `cargo clippy -p reveried --all-targets`
Expected: No warnings introduced by the rename.

- [ ] **Step 6: Commit**

```bash
git add crates/reveried/src/mcp.rs crates/reveried/src/main.rs crates/reveried/src/server/extra_routes.rs
git rm crates/reveried/src/mcp_sse.rs 2>/dev/null || true
git commit -m "reveried: rename mcp_sse module to mcp

No behavior change. Preparation for splitting into a per-tool module
under TOD-phase-0 MCP expansion."
```

---

## Task 2: Introduce `Ctx` struct and extend `McpState` with `Arc<DreamScheduler>`

**Goal:** Add the dream scheduler handle to `McpState` and introduce a `Ctx` struct that bundles `Arc<AppState>` + `Arc<DreamScheduler>` for tool calls. Update the existing `search_memory` call site to use the new shape, but don't split into a sub-module yet.

**Files:**
- Modify: `crates/reveried/src/mcp.rs`
- Modify: `crates/reveried/src/server/extra_routes.rs`

- [ ] **Step 1: Update `McpState` struct in `mcp.rs`**

Replace the existing `McpState` struct (currently at the top of `mcp.rs`):

```rust
#[derive(Clone)]
pub struct McpState {
    pub app: Arc<reverie_store::http::AppState>,
    pub sched: Arc<crate::dream_scheduler::DreamScheduler>,
    pub token: Arc<String>,
}

impl McpState {
    pub fn new(
        app: Arc<reverie_store::http::AppState>,
        sched: Arc<crate::dream_scheduler::DreamScheduler>,
        token: String,
    ) -> Self {
        Self {
            app,
            sched,
            token: Arc::new(token),
        }
    }
}
```

- [ ] **Step 2: Add `Ctx` struct in `mcp.rs`**

Add directly below `McpState`:

```rust
pub(crate) struct Ctx {
    pub app: Arc<reverie_store::http::AppState>,
    pub sched: Arc<crate::dream_scheduler::DreamScheduler>,
}

impl Ctx {
    pub(crate) fn from_state(state: &McpState) -> Self {
        Self {
            app: state.app.clone(),
            sched: state.sched.clone(),
        }
    }
}
```

- [ ] **Step 3: Update `handle_tools_call` in `mcp.rs` to build a `Ctx`**

Find the `handle_tools_call` arm in `handle_mcp_post`. Currently it calls:

```rust
"tools/call" => match handle_tools_call(state.app, req.params).await { ... }
```

Change the dispatcher signature so it takes `&McpState`:

```rust
"tools/call" => match handle_tools_call(&state, req.params).await { ... }
```

Update the `handle_tools_call` function signature from:

```rust
async fn handle_tools_call(
    app: Arc<reverie_store::http::AppState>,
    params: Option<Value>,
) -> Result<Value, RpcCallError> {
```

to:

```rust
async fn handle_tools_call(
    state: &McpState,
    params: Option<Value>,
) -> Result<Value, RpcCallError> {
```

Inside the body, build `let ctx = Ctx::from_state(state);` near the top, and replace the existing call to `search_memory_content_blocks(app, ...)` with `search_memory_content_blocks(ctx.app.clone(), ...)`. (The function still takes `Arc<AppState>` for now; later tasks change its shape.)

- [ ] **Step 4: Update call sites in `extra_routes.rs`**

In `crates/reveried/src/server/extra_routes.rs`, change `maybe_mcp_router`'s signature:

```rust
fn maybe_mcp_router(
    state: Arc<AppState>,
    sched: Arc<DreamScheduler>,
    codepath: &str,
) -> Option<Router> {
```

Update its body:

```rust
    Some(mcp::router(mcp::McpState::new(state, sched, token)))
```

Update the two callers (`build_full` at line 74, `build_handoff` at line 106) to pass `sched.clone()`:

```rust
    if let Some(mcp_router) = maybe_mcp_router(state, sched.clone(), "serve") {
```

```rust
    if let Some(mcp_router) = maybe_mcp_router(state, sched.clone(), "handoff") {
```

(`sched: Arc<DreamScheduler>` is already in scope at both call sites — `build_full` line 51, `build_handoff` line 92.)

- [ ] **Step 5: Update existing tests in `mcp.rs` to construct `McpState` with sched**

Find `test_app()` in the `mod tests` block. Replace:

```rust
fn test_app() -> Router {
    router(McpState::new(
        reverie_store::http::AppState::new_with_noop(
            EngramCompatStore::open_in_memory().unwrap(),
        ),
        TEST_TOKEN.to_string(),
    ))
}
```

with:

```rust
fn test_sched() -> Arc<crate::dream_scheduler::DreamScheduler> {
    use crate::dream_scheduler::{DreamScheduler, DreamSchedulerConfig};
    use reverie_store::config::ReveriedConfig;
    use reverie_store::events::NoopEventManager;
    Arc::new(DreamScheduler::new(
        Arc::new(NoopEventManager),
        ReveriedConfig::default(),
        DreamSchedulerConfig::default(),
    ))
}

fn test_app() -> Router {
    router(McpState::new(
        Arc::new(reverie_store::http::AppState::new_with_noop(
            EngramCompatStore::open_in_memory().unwrap(),
        )),
        test_sched(),
        TEST_TOKEN.to_string(),
    ))
}
```

(Note: existing `AppState::new_with_noop` may already return `Arc<AppState>` directly — check the call site. If it returns a bare `AppState`, wrap with `Arc::new`. If it already returns `Arc<AppState>`, skip the wrapping.)

Update the second test that constructs `McpState::new` directly (`test_mcp_tools_call_search_memory_scopes_to_project`, around line 589) the same way:

```rust
let app = router(McpState::new(
    Arc::new(reverie_store::http::AppState::new_with_noop(store)),
    test_sched(),
    TEST_TOKEN.to_string(),
));
```

- [ ] **Step 6: Verify tests pass**

Run: `cargo test -p reveried mcp`
Expected: All 13 existing tests still pass.

- [ ] **Step 7: Verify clippy clean**

Run: `cargo clippy -p reveried --all-targets`
Expected: No new warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/reveried/src/mcp.rs crates/reveried/src/server/extra_routes.rs
git commit -m "reveried/mcp: thread DreamScheduler into McpState

Adds Arc<DreamScheduler> alongside Arc<AppState> in McpState and
introduces a Ctx { app, sched } bundle for tool dispatch. Uniform
signature lets the upcoming dream_status / dream_last_report tools
share the dispatcher table with the existing search_memory tool.

No new tools yet — search_memory behavior unchanged."
```

---

## Task 3: Split `search_memory` into `mcp/search_memory.rs`

**Goal:** Move the `search_memory` tool's per-tool logic (parameter parsing, content blocks, schema, FTS+chunker helpers) into a sub-module file with the uniform `call(ctx, args)` signature. The dispatcher in `mcp.rs` shrinks; `mcp.rs` keeps only envelope/auth/routing.

**Files:**
- Modify: `crates/reveried/src/mcp.rs`
- Create: `crates/reveried/src/mcp/search_memory.rs`

- [ ] **Step 1: Create the new module file**

Create `crates/reveried/src/mcp/search_memory.rs` with:

```rust
use std::sync::Arc;

use reverie_store::engram_types::Observation;
use reverie_store::engram_types::SearchOptions;
use serde_json::{Value, json};

use super::{Ctx, RpcCallError};

pub const TOOL_NAME: &str = "search_memory";
const DEFAULT_SEARCH_LIMIT: usize = 5;
const MAX_SEARCH_LIMIT: usize = 20;

pub fn schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Search Reverie's memory store, scoped to a single project.",
        "inputSchema": {
            "type": "object",
            "required": ["query", "project"],
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Natural-language memory search query.",
                },
                "project": {
                    "type": "string",
                    "description": "Project scope for the search. Only observations tagged with this project are returned.",
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of memory results to return.",
                    "minimum": 1,
                    "maximum": 20,
                    "default": 5,
                },
            },
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

pub async fn call(ctx: &Ctx, arguments: &Value) -> Result<Value, RpcCallError> {
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.query required"))?;
    let project = arguments
        .get("project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|project| !project.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.project required"))?;
    let limit = parse_search_limit(arguments)?;

    let content = search_memory_content_blocks(ctx.app.clone(), query, project, limit)
        .await
        .map_err(|error| RpcCallError::server_error(format!("search_memory failed: {error}")))?;

    Ok(json!({ "content": content }))
}

fn parse_search_limit(arguments: &Value) -> Result<usize, RpcCallError> {
    let Some(limit) = arguments.get("limit") else {
        return Ok(DEFAULT_SEARCH_LIMIT);
    };
    let limit = limit.as_u64().and_then(|limit| usize::try_from(limit).ok()).ok_or_else(|| {
        RpcCallError::invalid_params("arguments.limit must be a positive integer")
    })?;
    Ok(limit.clamp(1, MAX_SEARCH_LIMIT))
}

async fn search_memory_content_blocks(
    app: Arc<reverie_store::http::AppState>,
    query: &str,
    project: &str,
    limit: usize,
) -> anyhow::Result<Vec<Value>> {
    let mut blocks = search_memory_with_chunker(&app, query, project, limit).await?;
    if blocks.is_empty() {
        blocks = search_memory_with_fts(&app, query, project, limit).await?;
    }
    if blocks.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": format!("No memories found for: {query:?} (project={project})"),
        }));
    }
    Ok(blocks)
}

async fn search_memory_with_chunker(
    app: &Arc<reverie_store::http::AppState>,
    query: &str,
    project: &str,
    limit: usize,
) -> anyhow::Result<Vec<Value>> {
    let Some(hook) = app.chunker_hook.as_ref() else {
        return Ok(Vec::new());
    };

    let overfetch = limit.saturating_mul(4).max(limit);
    let hits = hook.search(query, overfetch)?;
    if hits.is_empty() {
        return Ok(Vec::new());
    }

    let store = app.store.lock().await;
    let mut blocks = Vec::with_capacity(limit);
    for (sync_id, score) in hits {
        if blocks.len() >= limit {
            break;
        }
        match store.get_observation_by_sync_id(&sync_id) {
            Ok(observation) => {
                if observation.project.as_deref() == Some(project) {
                    blocks.push(memory_text_block(&observation, Some(score)));
                }
            },
            Err(error) => {
                tracing::debug!(%sync_id, %error, "mcp search_memory: chunk hit missing observation");
            },
        }
    }
    Ok(blocks)
}

async fn search_memory_with_fts(
    app: &Arc<reverie_store::http::AppState>,
    query: &str,
    project: &str,
    limit: usize,
) -> anyhow::Result<Vec<Value>> {
    let store = app.store.lock().await;
    let results = store.search(
        query,
        SearchOptions {
            limit,
            query_mode: Some("or".to_string()),
            project: Some(project.to_string()),
            ..Default::default()
        },
    )?;
    Ok(results
        .into_iter()
        .map(|result| memory_text_block(&result.observation, Some(result.rank as f32)))
        .collect())
}

fn memory_text_block(observation: &Observation, score: Option<f32>) -> Value {
    let mut text = format!(
        "#{} [{}] {}\n{}",
        observation.id, observation.r#type, observation.title, observation.content
    );
    if let Some(project) = observation.project.as_deref().filter(|project| !project.is_empty()) {
        text.push_str(&format!("\nProject: {project}"));
    }
    if !observation.scope.is_empty() {
        text.push_str(&format!("\nScope: {}", observation.scope));
    }
    if let Some(topic_key) =
        observation.topic_key.as_deref().filter(|topic_key| !topic_key.is_empty())
    {
        text.push_str(&format!("\nTopic: {topic_key}"));
    }
    if let Some(score) = score {
        text.push_str(&format!("\nScore: {score:.4}"));
    }

    json!({
        "type": "text",
        "text": text,
    })
}
```

- [ ] **Step 2: Slim down `mcp.rs` — declare submodule + remove migrated code**

In `crates/reveried/src/mcp.rs`:

1. At the top of the file (after `use` statements), add:
   ```rust
   pub mod search_memory;
   ```

2. Remove the moved code from `mcp.rs`:
   - `parse_search_limit`
   - `search_memory_content_blocks`
   - `search_memory_with_chunker`
   - `search_memory_with_fts`
   - `memory_text_block`
   - The constants `DEFAULT_SEARCH_LIMIT`, `MAX_SEARCH_LIMIT`
   - The unused `use reverie_store::engram_types::{Observation, SearchOptions};` import

3. Replace the inline `tools_list_result()` body so it iterates a static array. Change:

   ```rust
   fn tools_list_result() -> Value {
       json!({
           "tools": [ /* search_memory inline */ ]
       })
   }
   ```

   to:

   ```rust
   fn tools_list_result() -> Value {
       json!({
           "tools": [
               search_memory::schema(),
           ]
       })
   }
   ```

4. Replace `handle_tools_call` body. Change:

   ```rust
   async fn handle_tools_call(
       state: &McpState,
       params: Option<Value>,
   ) -> Result<Value, RpcCallError> {
       let params = params.ok_or_else(|| RpcCallError::invalid_params("params required"))?;
       let name = params.get("name").and_then(Value::as_str).unwrap_or("");
       if name != "search_memory" {
           return Err(RpcCallError::method_not_found(format!("unknown tool: {name}")));
       }
       /* ... existing inline parse + call ... */
   }
   ```

   to:

   ```rust
   async fn handle_tools_call(
       state: &McpState,
       params: Option<Value>,
   ) -> Result<Value, RpcCallError> {
       let params = params.ok_or_else(|| RpcCallError::invalid_params("params required"))?;
       let name = params.get("name").and_then(Value::as_str).unwrap_or("");
       let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
       let ctx = Ctx::from_state(state);
       match name {
           search_memory::TOOL_NAME => search_memory::call(&ctx, &arguments).await,
           other => Err(RpcCallError::method_not_found(format!("unknown tool: {other}"))),
       }
   }
   ```

5. Make `RpcCallError`'s constructors `pub(crate)` so the sub-module can use them. Change:

   ```rust
   impl RpcCallError {
       fn invalid_params(message: impl Into<String>) -> Self { ... }
       fn method_not_found(message: impl Into<String>) -> Self { ... }
       fn server_error(message: impl Into<String>) -> Self { ... }
   }
   ```

   to:

   ```rust
   impl RpcCallError {
       pub(crate) fn invalid_params(message: impl Into<String>) -> Self { ... }
       pub(crate) fn method_not_found(message: impl Into<String>) -> Self { ... }
       pub(crate) fn server_error(message: impl Into<String>) -> Self { ... }
   }
   ```

   And make the struct + fields visible:

   ```rust
   pub(crate) struct RpcCallError {
       pub(crate) code: i64,
       pub(crate) message: String,
   }
   ```

- [ ] **Step 3: Verify tests still pass**

Run: `cargo test -p reveried mcp`
Expected: All 13 existing tests still pass — same behavior, same JSON-RPC outputs, same SSE framing.

- [ ] **Step 4: Verify clippy clean**

Run: `cargo clippy -p reveried --all-targets`
Expected: No new warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/reveried/src/mcp.rs crates/reveried/src/mcp/
git commit -m "reveried/mcp: split search_memory into per-tool module

Pulls the search_memory tool's schema, parameter parsing, and content-
block helpers out of the mcp dispatcher and into mcp/search_memory.rs.
Establishes the call(ctx, args) signature shared by all tools and a
dispatcher table mechanical enough to extend.

No behavior change — same JSON-RPC envelope, same FTS+chunker fallback,
same project-scoped output."
```

---

## Task 4: Extract `add_observation_inner` from `reverie-store`

**Goal:** Pull the body of `handle_add_observation` (`reverie-store/src/http/mod.rs:998-1142`) into a free function the MCP layer can call without going through axum extractors. Axum handler shrinks to a thin adapter; all behavior (write-gate, contradiction detector, chunker hook, event publish, tx scope, tag attach, supersedes, gate-warning header) is preserved.

**Files:**
- Modify: `crates/reverie-store/src/http/mod.rs`

- [ ] **Step 1: Define `AddObservationOutcome` and `AddObservationError` types**

Above `handle_add_observation` (around line 996), add:

```rust
/// Outcome of a successful `add_observation_inner` call. Carries the
/// new observation's row id and any non-fatal write-gate warnings that
/// the caller may surface (HTTP layer attaches them as a response
/// header; MCP layer includes them in the result body).
#[derive(Debug, Clone)]
pub struct AddObservationOutcome {
    pub id: i64,
    pub gate_warnings: Vec<crate::placement::Issue>,
}

#[derive(Debug)]
pub enum AddObservationError {
    InvalidParams(&'static str),
    GateRejected { reasons: Vec<String>, issues: Vec<crate::placement::Issue> },
    Store(anyhow::Error),
}
```

- [ ] **Step 2: Extract the inner function**

Add directly below the new error types (still in `mod.rs`):

```rust
pub async fn add_observation_inner(
    state: &Arc<AppState>,
    body: AddObservationParams,
) -> Result<AddObservationOutcome, AddObservationError> {
    if body.title.is_empty() || body.content.is_empty() {
        return Err(AddObservationError::InvalidParams(
            "title and content are required",
        ));
    }

    let gate_warnings: Vec<crate::placement::Issue> =
        if let Some(gate) = state.write_gate.as_ref().filter(|g| g.enabled) {
            let issues = gate.evaluate(&body, None);
            let (rule, verdict) = if issues.is_empty() {
                ("write-gate", crate::events::types::GateVerdict::Accept)
            } else {
                let rule_name = issues.first().map(|i| i.rule.as_label()).unwrap_or("write-gate");
                (rule_name, crate::events::types::GateVerdict::Reject)
            };
            let candidate_hash = crate::engram_quirks::hash_normalized(&body.content);
            state.event_manager.publish(crate::events::types::Event::GateDecision {
                rule: rule.to_string(),
                candidate_hash,
                verdict,
            });
            if !issues.is_empty() {
                let mode_label = if gate.strict { "strict" } else { "warn" };
                for issue in &issues {
                    crate::http::metrics::metrics()
                        .write_gate_rejections_total
                        .with_label_values(&[issue.rule.as_label(), mode_label])
                        .inc();
                }
                if gate.strict {
                    let reasons: Vec<String> =
                        issues.iter().map(|i| i.rule.as_label().to_string()).collect();
                    return Err(AddObservationError::GateRejected { reasons, issues });
                }
                issues
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

    let scope = match body.topic_key.clone() {
        Some(tk) if !tk.is_empty() => state.begin_tx_with_topic(TxKind::MemSave, tk),
        _ => state.begin_tx(TxKind::MemSave),
    };
    let tags: Vec<(String, String)> = body
        .tags
        .as_ref()
        .map(|ts| ts.iter().map(|t| (t.facet.clone(), t.value.clone())).collect())
        .unwrap_or_default();
    let supersedes_sync_id = body.supersedes.clone();
    let mut store = state.store.lock().await;
    match store.add_observation(body) {
        Ok(id) => {
            if let Some(ref old_sync_id) = supersedes_sync_id
                && let Err(e) = store.set_supersedes(id, old_sync_id)
            {
                tracing::warn!("supersedes wiring failed for obs {id}: {e}");
            }
            if !tags.is_empty()
                && let Ok(obs) = store.get_observation(id)
                && let Err(e) = store.add_tags(&obs.sync_id, &tags)
            {
                tracing::warn!("tag attach failed for obs {id}: {e}");
            }
            #[cfg(feature = "backend-sqlite")]
            let hook_payload =
                state.chunker_hook.as_ref().and_then(|_| store.get_observation(id).ok());
            if let Some(detector) = state.contradiction_detector.as_ref()
                && let Ok(obs) = store.get_observation(id)
            {
                detector.on_observation_added(&store, &obs);
            }
            let captured_type: String = {
                let t = store.get_observation(id).ok().map(|o| o.r#type).unwrap_or_default();
                if t.is_empty() { "manual".into() } else { t }
            };
            let captured_topic: Option<String> =
                store.get_observation(id).ok().and_then(|o| o.topic_key);
            drop(store);
            scope.commit();
            state.event_manager.publish(Event::ObservationCaptured {
                id,
                type_tag: captured_type,
                topic_key: captured_topic,
            });
            state.notify_write();
            #[cfg(feature = "backend-sqlite")]
            if let (Some(hook), Some(obs)) = (state.chunker_hook.as_ref(), hook_payload.as_ref()) {
                hook.on_observation_added(obs);
            }
            Ok(AddObservationOutcome { id, gate_warnings })
        },
        Err(e) => {
            let msg = e.to_string();
            scope.abort(msg);
            Err(AddObservationError::Store(e))
        },
    }
}
```

- [ ] **Step 3: Replace `handle_add_observation` body with a thin adapter**

Replace the entire body of `handle_add_observation` (currently `mod.rs:998-1142`) with:

```rust
#[tracing::instrument(skip(state))]
async fn handle_add_observation(
    State(state): State<SharedState>,
    Json(body): Json<AddObservationParams>,
) -> Response {
    match add_observation_inner(&state, body).await {
        Ok(outcome) => {
            let mut resp = (
                StatusCode::CREATED,
                Json(json!({ "id": outcome.id, "status": "saved" })),
            )
                .into_response();
            if !outcome.gate_warnings.is_empty() {
                let reasons = outcome
                    .gate_warnings
                    .iter()
                    .map(|i| i.rule.as_label())
                    .collect::<Vec<_>>()
                    .join(",");
                if let Ok(val) = axum::http::HeaderValue::from_str(&reasons) {
                    resp.headers_mut().insert("x-reverie-write-gate-warnings", val);
                }
            }
            resp
        },
        Err(AddObservationError::InvalidParams(msg)) => {
            json_error(StatusCode::BAD_REQUEST, msg)
        },
        Err(AddObservationError::GateRejected { reasons, issues }) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "write_gate_rejected",
                "reasons": reasons,
                "issues": issues,
            })),
        )
            .into_response(),
        Err(AddObservationError::Store(e)) => anyhow_to_500(e),
    }
}
```

- [ ] **Step 4: Verify the whole reverie-store test suite still passes**

Run: `cargo test -p reverie-store`
Expected: All existing tests pass — the extraction is behavior-preserving.

- [ ] **Step 5: Verify clippy clean**

Run: `cargo clippy -p reverie-store --all-targets`
Expected: No new warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/reverie-store/src/http/mod.rs
git commit -m "reverie-store: extract add_observation_inner

Pulls handle_add_observation's body into a free function returning
AddObservationOutcome / AddObservationError so non-axum callers (the
upcoming reveried MCP add_observation tool) can reuse the same write-
gate / contradiction-detector / event-publish / tag-attach pipeline.

Axum handler shrinks to a 30-line adapter that maps the outcome to an
HTTP response. No behavior change — gate-warning header, gate-strict
422, and validation errors all preserved."
```

---

## Task 5: Extract `passive_capture_inner` from `reverie-store`

**Goal:** Same shape as Task 4 but for `handle_passive_capture` (`reverie-store/src/http/mod.rs:1158-1202`).

**Files:**
- Modify: `crates/reverie-store/src/http/mod.rs`

- [ ] **Step 1: Define `PassiveCaptureError`**

Above `handle_passive_capture`, add:

```rust
#[derive(Debug)]
pub enum PassiveCaptureError {
    InvalidParams(&'static str),
    InvalidSession { status: StatusCode, msg: String },
    Store(anyhow::Error),
}
```

- [ ] **Step 2: Make `PassiveCaptureBody` and `passive_capture_inner` pub-visible**

Change the existing `PassiveCaptureBody` from:

```rust
#[derive(Debug, Deserialize)]
struct PassiveCaptureBody {
```

to:

```rust
#[derive(Debug, Deserialize)]
pub struct PassiveCaptureBody {
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}
```

(All fields go to `pub`.)

Add the inner function directly above `handle_passive_capture`:

```rust
pub async fn passive_capture_inner(
    state: &Arc<AppState>,
    body: PassiveCaptureBody,
) -> Result<crate::engram_types::PassiveCaptureResult, PassiveCaptureError> {
    if body.session_id.is_empty() || body.content.is_empty() {
        return Err(PassiveCaptureError::InvalidParams(
            "session_id and content are required",
        ));
    }
    if let Err((status, msg)) = validate_session_id(&body.session_id) {
        return Err(PassiveCaptureError::InvalidSession {
            status,
            msg: msg.to_string(),
        });
    }
    let scope = state.begin_tx(TxKind::MemSave);
    let mut store = state.store.lock().await;
    let params = crate::engram_types::PassiveCaptureParams {
        session_id: body.session_id,
        content: body.content,
        project: body.project,
        source: body.source,
    };
    match store.passive_capture(params) {
        Ok(r) => {
            let saved = r.saved;
            drop(store);
            scope.commit();
            if saved > 0 {
                state.event_manager.publish(Event::ObservationCaptured {
                    id: 0,
                    type_tag: "passive".into(),
                    topic_key: None,
                });
            }
            state.notify_write();
            Ok(r)
        },
        Err(e) => {
            let msg = e.to_string();
            scope.abort(msg);
            Err(PassiveCaptureError::Store(e))
        },
    }
}
```

- [ ] **Step 3: Replace `handle_passive_capture` body with a thin adapter**

Replace the entire body:

```rust
#[tracing::instrument(skip(state))]
async fn handle_passive_capture(
    State(state): State<SharedState>,
    Json(body): Json<PassiveCaptureBody>,
) -> Response {
    match passive_capture_inner(&state, body).await {
        Ok(r) => Json(r).into_response(),
        Err(PassiveCaptureError::InvalidParams(msg)) => {
            json_error(StatusCode::BAD_REQUEST, msg)
        },
        Err(PassiveCaptureError::InvalidSession { status, msg }) => {
            json_error(status, &msg)
        },
        Err(PassiveCaptureError::Store(e)) => anyhow_to_500(e),
    }
}
```

- [ ] **Step 4: Verify reverie-store tests still pass**

Run: `cargo test -p reverie-store`
Expected: All tests pass.

- [ ] **Step 5: Verify clippy clean**

Run: `cargo clippy -p reverie-store --all-targets`
Expected: No new warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/reverie-store/src/http/mod.rs
git commit -m "reverie-store: extract passive_capture_inner

Same shape as add_observation_inner — pulls handle_passive_capture's
body into a free function returning PassiveCaptureResult /
PassiveCaptureError so the reveried MCP add_observation_passive tool
can reuse the same Key-Learnings extraction + ObservationCaptured
event-publish pipeline.

Axum handler shrinks to an adapter. PassiveCaptureBody made pub.
No behavior change."
```

---

## Task 6: Add `smart_context` tool

**Goal:** First new tool. Read-only, project-required, wraps `format_smart_context_tokens` / `format_smart_context`.

**Files:**
- Create: `crates/reveried/src/mcp/smart_context.rs`
- Modify: `crates/reveried/src/mcp.rs` (declare module + register in dispatcher)
- Create: `crates/reveried/tests/mcp_tools.rs` (integration test file, first occupant)

- [ ] **Step 1: Write failing unit test**

Create `crates/reveried/src/mcp/smart_context.rs` with the test stub at the bottom:

```rust
use std::sync::Arc;

use serde_json::{Value, json};

use super::{Ctx, RpcCallError};

pub const TOOL_NAME: &str = "smart_context";
const DEFAULT_LIMIT_MIN: u64 = 1;
const DEFAULT_LIMIT_MAX: u64 = 50;

pub fn schema() -> Value {
    todo!("schema")
}

pub async fn call(_ctx: &Ctx, _arguments: &Value) -> Result<Value, RpcCallError> {
    todo!("call")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reverie_store::backends::engram_compat::EngramCompatStore;
    use reverie_store::http::AppState;

    fn test_ctx() -> Ctx {
        use crate::dream_scheduler::{DreamScheduler, DreamSchedulerConfig};
        use reverie_store::config::ReveriedConfig;
        use reverie_store::events::NoopEventManager;
        let app = Arc::new(AppState::new_with_noop(EngramCompatStore::open_in_memory().unwrap()));
        let sched = Arc::new(DreamScheduler::new(
            Arc::new(NoopEventManager),
            ReveriedConfig::default(),
            DreamSchedulerConfig::default(),
        ));
        Ctx { app, sched }
    }

    #[tokio::test]
    async fn rejects_missing_project() {
        let ctx = test_ctx();
        let err = call(&ctx, &json!({})).await.expect_err("should reject");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("project"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails (compile fail or panic)**

Run: `cargo test -p reveried mcp::smart_context::tests::rejects_missing_project`
Expected: FAIL — `todo!("call")` panics, OR the function isn't reachable yet because the module isn't registered.

(If "module not found", proceed to register the module first per Step 3, then re-run.)

- [ ] **Step 3: Register the new module in `mcp.rs`**

In `crates/reveried/src/mcp.rs`, below the existing `pub mod search_memory;`:

```rust
pub mod search_memory;
pub mod smart_context;
```

Update the dispatcher in `handle_tools_call`:

```rust
match name {
    search_memory::TOOL_NAME => search_memory::call(&ctx, &arguments).await,
    smart_context::TOOL_NAME => smart_context::call(&ctx, &arguments).await,
    other => Err(RpcCallError::method_not_found(format!("unknown tool: {other}"))),
}
```

Update `tools_list_result`:

```rust
fn tools_list_result() -> Value {
    json!({
        "tools": [
            search_memory::schema(),
            smart_context::schema(),
        ]
    })
}
```

- [ ] **Step 4: Implement `schema()` and `call()`**

Replace the bodies in `crates/reveried/src/mcp/smart_context.rs`:

```rust
pub fn schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Load Reverie's project-aware tiered context blob. Returns the formatted text consumed by upstream agents (TOD-257).",
        "inputSchema": {
            "type": "object",
            "required": ["project"],
            "properties": {
                "project": {
                    "type": "string",
                    "description": "Project scope. Tiers A/B/C are filtered to this project.",
                },
                "query": {
                    "type": "string",
                    "description": "Optional query string used by adaptive token-budget sizing.",
                },
                "limit": {
                    "type": "integer",
                    "description": "Max bullet rows when no token budget resolves.",
                    "minimum": 1,
                    "maximum": 50,
                },
                "token_budget": {
                    "type": "integer",
                    "description": "Explicit token budget. Wins over model-derived budget.",
                    "minimum": 1,
                },
                "model": {
                    "type": "string",
                    "description": "Model hint for token-budget resolution (e.g. \"claude-opus-4-7\").",
                },
                "adaptive": {
                    "type": "boolean",
                    "description": "Scale resolved budget by query complexity. Default false.",
                    "default": false,
                },
            },
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

pub async fn call(ctx: &Ctx, arguments: &Value) -> Result<Value, RpcCallError> {
    let project = arguments
        .get("project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.project required"))?;

    let limit = match arguments.get("limit") {
        None => None,
        Some(v) => Some(
            v.as_u64()
                .filter(|l| (DEFAULT_LIMIT_MIN..=DEFAULT_LIMIT_MAX).contains(l))
                .and_then(|l| usize::try_from(l).ok())
                .ok_or_else(|| {
                    RpcCallError::invalid_params(format!(
                        "arguments.limit must be a positive integer in [{DEFAULT_LIMIT_MIN}, {DEFAULT_LIMIT_MAX}]"
                    ))
                })?,
        ),
    };
    let token_budget = match arguments.get("token_budget") {
        None => None,
        Some(v) => Some(v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| {
            RpcCallError::invalid_params("arguments.token_budget must be a positive integer")
        })?),
    };
    let query = arguments.get("query").and_then(Value::as_str).map(str::to_string);
    let model = arguments.get("model").and_then(Value::as_str).map(str::to_string);
    let adaptive = arguments.get("adaptive").and_then(Value::as_bool).unwrap_or(false);

    let app = ctx.app.clone();
    let text = render_smart_context(app, project, query, model, limit, token_budget, adaptive)
        .await
        .map_err(|e| RpcCallError::server_error(format!("smart_context failed: {e}")))?;

    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": text,
            }
        ]
    }))
}

async fn render_smart_context(
    app: Arc<reverie_store::http::AppState>,
    project: &str,
    query: Option<String>,
    model: Option<String>,
    limit: Option<usize>,
    token_budget: Option<usize>,
    adaptive: bool,
) -> anyhow::Result<String> {
    use reverie_store::http::smart_context::{
        apply_adaptive_budget, format_smart_context, format_smart_context_tokens,
        resolve_token_budget,
    };

    app.wait_for_read().await;
    let store = app.store.lock().await;
    let mut cfg = store.smart_context_config();
    if let Some(l) = limit {
        cfg.smart_context_default_limit = l;
    }
    let resolved = resolve_token_budget(token_budget, limit, None, model.as_deref());
    let resolved =
        resolved.map(|base| apply_adaptive_budget(base, adaptive, token_budget, query.as_deref()));
    let text = match resolved {
        Some(tb) => format_smart_context_tokens(&store, Some(project), &cfg, tb)?,
        None => format_smart_context(&store, Some(project), &cfg)?,
    };
    Ok(text)
}
```

- [ ] **Step 5: Verify smart_context helpers compile**

`format_smart_context` (`smart_context.rs:567`), `format_smart_context_tokens` (`smart_context.rs:467`), `resolve_token_budget` (`smart_context.rs:182`), and `apply_adaptive_budget` (`smart_context.rs:91`) are already `pub fn`, and the module itself is `pub mod smart_context;` at `reverie-store/src/http/mod.rs:24`. Run:

```bash
cargo build -p reveried
```

Expected: clean build. If any helper turns out to be private, add `pub` to its declaration in `reverie-store/src/http/smart_context.rs`.

- [ ] **Step 6: Add unit test cases**

Append additional cases to the `mod tests` block in `mcp/smart_context.rs`:

```rust
#[tokio::test]
async fn rejects_empty_project() {
    let ctx = test_ctx();
    let err = call(&ctx, &json!({"project": "  "})).await.expect_err("empty project rejected");
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("project"));
}

#[tokio::test]
async fn rejects_invalid_limit() {
    let ctx = test_ctx();
    let err = call(&ctx, &json!({"project": "p", "limit": 0}))
        .await
        .expect_err("limit=0 rejected");
    assert_eq!(err.code, -32602);

    let err = call(&ctx, &json!({"project": "p", "limit": 999}))
        .await
        .expect_err("limit too large rejected");
    assert_eq!(err.code, -32602);
}

#[tokio::test]
async fn returns_content_block_on_empty_store() {
    let ctx = test_ctx();
    let result = call(&ctx, &json!({"project": "p"})).await.expect("empty-store call ok");
    let content = result.get("content").expect("content array");
    assert!(content.is_array());
    assert_eq!(content.as_array().unwrap().len(), 1);
    assert_eq!(content[0]["type"], "text");
}
```

- [ ] **Step 7: Run unit tests — should now pass**

Run: `cargo test -p reveried mcp::smart_context::tests`
Expected: 4/4 PASS.

- [ ] **Step 8: Create `tests/mcp_tools.rs` with integration test for smart_context**

Create `crates/reveried/tests/mcp_tools.rs`:

```rust
use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use reverie_store::backends::engram_compat::EngramCompatStore;
use reverie_store::http::AppState;
use reverie_store::config::ReveriedConfig;
use reverie_store::events::NoopEventManager;
use reveried::dream_scheduler::{DreamScheduler, DreamSchedulerConfig};
use reveried::mcp::{McpState, router};
use tower::ServiceExt;

const TEST_TOKEN: &str = "test-token-mcp-tools";

fn test_app_with_store(store: EngramCompatStore) -> Router {
    let app = Arc::new(AppState::new_with_noop(store));
    let sched = Arc::new(DreamScheduler::new(
        Arc::new(NoopEventManager),
        ReveriedConfig::default(),
        DreamSchedulerConfig::default(),
    ));
    router(McpState::new(app, sched, TEST_TOKEN.to_string()))
}

fn test_app() -> Router {
    test_app_with_store(EngramCompatStore::open_in_memory().unwrap())
}

fn authed_call(method: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/mcp")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_TOKEN}"))
        .header("Mcp-Method", method)
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_string(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn smart_context_returns_content_block() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/call",
            r#"{"jsonrpc":"2.0","method":"tools/call","id":"sc-1","params":{"name":"smart_context","arguments":{"project":"reverie"}}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains(r#""id":"sc-1""#));
    assert!(body.contains(r#""type":"text""#));
}

#[tokio::test]
async fn smart_context_rejects_missing_project() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/call",
            r#"{"jsonrpc":"2.0","method":"tools/call","id":"sc-2","params":{"name":"smart_context","arguments":{}}}"#,
        ))
        .await
        .unwrap();
    let body = body_string(response).await;
    assert!(body.contains(r#""code":-32602"#));
    assert!(body.contains("project"));
}
```

- [ ] **Step 9: Run integration tests**

Run: `cargo test -p reveried --test mcp_tools`
Expected: 2/2 PASS.

- [ ] **Step 10: Verify clippy clean**

Run: `cargo clippy -p reveried --all-targets`
Expected: No new warnings.

- [ ] **Step 11: Commit**

```bash
git add crates/reveried/src/mcp.rs crates/reveried/src/mcp/smart_context.rs crates/reveried/tests/mcp_tools.rs
git commit -m "reveried/mcp: add smart_context tool

Wraps reverie-store's format_smart_context / format_smart_context_tokens
under the MCP /mcp surface. Project required at the boundary even
though the HTTP route accepts it as optional — keeps the MCP retrieval
contract project-scoped.

4 unit tests cover required-arg, limit clamping, empty-store text
block. 2 integration tests cover the full /mcp JSON-RPC envelope."
```

---

## Task 7: Add `add_observation` tool

**Goal:** First write tool. Wraps `add_observation_inner` from Task 4.

**Files:**
- Create: `crates/reveried/src/mcp/add_observation.rs`
- Modify: `crates/reveried/src/mcp.rs`
- Modify: `crates/reveried/tests/mcp_tools.rs`

- [ ] **Step 1: Write the failing unit test stub**

Create `crates/reveried/src/mcp/add_observation.rs`:

```rust
use std::sync::Arc;

use reverie_store::engram_types::AddObservationParams;
use reverie_store::http::{AddObservationError, add_observation_inner};
use serde_json::{Value, json};

use super::{Ctx, RpcCallError};

pub const TOOL_NAME: &str = "add_observation";

pub fn schema() -> Value {
    todo!("schema")
}

pub async fn call(_ctx: &Ctx, _arguments: &Value) -> Result<Value, RpcCallError> {
    todo!("call")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reverie_store::backends::engram_compat::EngramCompatStore;
    use reverie_store::http::AppState;

    fn test_ctx() -> Ctx {
        use crate::dream_scheduler::{DreamScheduler, DreamSchedulerConfig};
        use reverie_store::config::ReveriedConfig;
        use reverie_store::events::NoopEventManager;
        let app = Arc::new(AppState::new_with_noop(EngramCompatStore::open_in_memory().unwrap()));
        let sched = Arc::new(DreamScheduler::new(
            Arc::new(NoopEventManager),
            ReveriedConfig::default(),
            DreamSchedulerConfig::default(),
        ));
        Ctx { app, sched }
    }

    #[tokio::test]
    async fn rejects_missing_project() {
        let ctx = test_ctx();
        let err = call(&ctx, &json!({"title": "t", "content": "c"}))
            .await
            .expect_err("project required");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("project"));
    }
}
```

- [ ] **Step 2: Register module + dispatcher arm in `mcp.rs`**

```rust
pub mod search_memory;
pub mod smart_context;
pub mod add_observation;
```

Dispatcher:

```rust
match name {
    search_memory::TOOL_NAME => search_memory::call(&ctx, &arguments).await,
    smart_context::TOOL_NAME => smart_context::call(&ctx, &arguments).await,
    add_observation::TOOL_NAME => add_observation::call(&ctx, &arguments).await,
    other => Err(RpcCallError::method_not_found(format!("unknown tool: {other}"))),
}
```

Tools list:

```rust
search_memory::schema(),
smart_context::schema(),
add_observation::schema(),
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p reveried mcp::add_observation::tests::rejects_missing_project`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 4: Implement `schema()` and `call()`**

```rust
pub fn schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Add a single observation (title + content) to Reverie's memory store, scoped to a project. Goes through the same write-gate, contradiction detector, and event-publish pipeline as POST /observations.",
        "inputSchema": {
            "type": "object",
            "required": ["project", "title", "content"],
            "properties": {
                "project": {"type": "string", "description": "Project scope. Required."},
                "title": {"type": "string", "description": "Short title; non-empty."},
                "content": {"type": "string", "description": "Full observation body; non-empty."},
                "session_id": {"type": "string", "description": "Optional session id; defaults to lazy 'manual-save'."},
                "type": {"type": "string", "description": "Observation type tag (e.g. 'discovery', 'decision')."},
                "tool_name": {"type": "string"},
                "topic_key": {"type": "string"},
                "tags": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["facet", "value"],
                        "properties": {
                            "facet": {"type": "string"},
                            "value": {"type": "string"},
                        },
                    },
                },
                "supersedes": {"type": "string", "description": "sync_id of the observation this replaces."},
                "event_id": {"type": "string", "description": "Idempotency key for replay dedup."},
                "confidence": {"type": "number", "minimum": 0.0, "maximum": 1.0},
                "source": {"type": "string", "description": "Originating system. Defaults to 'mcp'."},
                "discovery_tokens": {"type": "integer", "minimum": 0},
                "scope": {"type": "string"},
                "skip_content_dedupe": {"type": "boolean"},
            },
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    })
}

pub async fn call(ctx: &Ctx, arguments: &Value) -> Result<Value, RpcCallError> {
    let project = arguments
        .get("project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.project required"))?;
    let title = arguments
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.title required"))?;
    let content = arguments
        .get("content")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.content required"))?;

    let mut params = AddObservationParams {
        project: Some(project.to_string()),
        title: title.to_string(),
        content: content.to_string(),
        ..Default::default()
    };
    params.session_id = arguments
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    params.r#type = arguments
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    params.tool_name = arguments.get("tool_name").and_then(Value::as_str).map(str::to_string);
    params.topic_key = arguments.get("topic_key").and_then(Value::as_str).map(str::to_string);
    params.scope = arguments.get("scope").and_then(Value::as_str).map(str::to_string);
    params.discovery_tokens = arguments.get("discovery_tokens").and_then(Value::as_i64);
    params.supersedes = arguments.get("supersedes").and_then(Value::as_str).map(str::to_string);
    params.event_id = arguments.get("event_id").and_then(Value::as_str).map(str::to_string);
    params.confidence = arguments.get("confidence").and_then(Value::as_f64);
    params.skip_content_dedupe = arguments.get("skip_content_dedupe").and_then(Value::as_bool);

    let source = arguments.get("source").and_then(Value::as_str).filter(|s| !s.is_empty());
    params.source = Some(source.unwrap_or("mcp").to_string());

    if let Some(tags_val) = arguments.get("tags").and_then(Value::as_array) {
        let mut tags = Vec::with_capacity(tags_val.len());
        for tag in tags_val {
            let facet = tag
                .get("facet")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcCallError::invalid_params("tag.facet required"))?;
            let value = tag
                .get("value")
                .and_then(Value::as_str)
                .ok_or_else(|| RpcCallError::invalid_params("tag.value required"))?;
            tags.push(reverie_store::engram_types::TagParam {
                facet: facet.to_string(),
                value: value.to_string(),
            });
        }
        params.tags = Some(tags);
    }

    match add_observation_inner(&ctx.app, params).await {
        Ok(outcome) => {
            let warnings: Vec<&str> =
                outcome.gate_warnings.iter().map(|i| i.rule.as_label()).collect();
            let payload = json!({
                "id": outcome.id,
                "status": "saved",
                "gate_warnings": warnings,
            });
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": payload.to_string(),
                    }
                ]
            }))
        },
        Err(AddObservationError::InvalidParams(msg)) => {
            Err(RpcCallError::invalid_params(msg))
        },
        Err(AddObservationError::GateRejected { reasons, .. }) => Err(RpcCallError {
            code: -32000,
            message: format!("write_gate_rejected: {}", reasons.join(",")),
        }),
        Err(AddObservationError::Store(e)) => {
            Err(RpcCallError::server_error(format!("add_observation failed: {e}")))
        },
    }
}
```

(Note: this requires `RpcCallError`'s fields to be `pub(crate)` — already done in Task 3. If a `pub fn new(code, msg)` constructor is preferred, add it to `RpcCallError` in `mcp.rs` and use it here instead of struct-literal.)

`add_observation_inner` is already `pub` in `reverie_store::http` from Task 4. Confirm imports compile.

- [ ] **Step 5: Run test — should now pass**

Run: `cargo test -p reveried mcp::add_observation::tests::rejects_missing_project`
Expected: PASS.

- [ ] **Step 6: Add additional unit tests**

Append to `mod tests`:

```rust
#[tokio::test]
async fn rejects_missing_title() {
    let ctx = test_ctx();
    let err = call(&ctx, &json!({"project": "p", "content": "c"}))
        .await
        .expect_err("title required");
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("title"));
}

#[tokio::test]
async fn rejects_missing_content() {
    let ctx = test_ctx();
    let err = call(&ctx, &json!({"project": "p", "title": "t"}))
        .await
        .expect_err("content required");
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("content"));
}

#[tokio::test]
async fn happy_path_returns_id() {
    let ctx = test_ctx();
    let result = call(&ctx, &json!({
        "project": "test-project",
        "title": "MCP write test",
        "content": "Saved via add_observation tool.",
    })).await.expect("save ok");
    let content = &result["content"][0]["text"];
    let payload: Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert!(payload.get("id").and_then(Value::as_i64).unwrap_or(0) > 0);
    assert_eq!(payload["status"], "saved");
}

#[tokio::test]
async fn defaults_source_to_mcp() {
    let ctx = test_ctx();
    let result = call(&ctx, &json!({
        "project": "test-project",
        "title": "Source default test",
        "content": "Source field should default to mcp.",
    })).await.expect("save ok");
    let content = &result["content"][0]["text"];
    let payload: Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    let id = payload["id"].as_i64().unwrap();

    let store = ctx.app.store.lock().await;
    let obs = store.get_observation(id).expect("row present");
    assert_eq!(obs.source.as_deref(), Some("mcp"));
}

#[tokio::test]
async fn explicit_source_overrides_default() {
    let ctx = test_ctx();
    let result = call(&ctx, &json!({
        "project": "test-project",
        "title": "Source override",
        "content": "Source comes through.",
        "source": "claude-desktop",
    })).await.expect("save ok");
    let content = &result["content"][0]["text"];
    let payload: Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    let id = payload["id"].as_i64().unwrap();
    let store = ctx.app.store.lock().await;
    let obs = store.get_observation(id).expect("row present");
    assert_eq!(obs.source.as_deref(), Some("claude-desktop"));
}
```

(`reverie_store::engram_types::Observation` should already have a `source: Option<String>` field — check `engram_types.rs` and adjust `obs.source` access to match the actual field name; if it's stored differently, query via the store's API instead.)

- [ ] **Step 7: Run full unit-test set**

Run: `cargo test -p reveried mcp::add_observation::tests`
Expected: 5/5 PASS.

- [ ] **Step 8: Add integration test in `tests/mcp_tools.rs`**

Append:

```rust
#[tokio::test]
async fn add_observation_round_trip() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/call",
            r#"{"jsonrpc":"2.0","method":"tools/call","id":"add-1","params":{"name":"add_observation","arguments":{"project":"reverie","title":"MCP integration","content":"Round-trip via /mcp."}}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains(r#""id":"add-1""#));
    assert!(body.contains(r#""status":"saved""#));
}

#[tokio::test]
async fn add_observation_rejects_empty_title() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/call",
            r#"{"jsonrpc":"2.0","method":"tools/call","id":"add-2","params":{"name":"add_observation","arguments":{"project":"p","title":"","content":"c"}}}"#,
        ))
        .await
        .unwrap();
    let body = body_string(response).await;
    assert!(body.contains(r#""code":-32602"#));
    assert!(body.contains("title"));
}
```

- [ ] **Step 9: Run integration tests**

Run: `cargo test -p reveried --test mcp_tools`
Expected: 4/4 PASS (2 from smart_context + 2 new).

- [ ] **Step 10: Verify clippy clean**

Run: `cargo clippy -p reveried --all-targets`

- [ ] **Step 11: Commit**

```bash
git add crates/reveried/src/mcp.rs crates/reveried/src/mcp/add_observation.rs crates/reveried/tests/mcp_tools.rs
git commit -m "reveried/mcp: add add_observation tool

Wraps add_observation_inner — full write-gate, contradiction-detector,
event-publish pipeline reused via in-process AppState. Project / title
/ content all required at the MCP boundary. Source defaults to 'mcp'
for downstream telemetry; explicit source overrides.

Strict-mode write-gate rejection surfaces as JSON-RPC -32000 with the
joined rule labels in the message. 5 unit tests + 2 integration tests."
```

---

## Task 8: Add `add_observation_passive` tool

**Goal:** Same shape as Task 7 but for passive (Key-Learnings extraction) writes.

**Files:**
- Create: `crates/reveried/src/mcp/add_observation_passive.rs`
- Modify: `crates/reveried/src/mcp.rs`
- Modify: `crates/reveried/tests/mcp_tools.rs`

- [ ] **Step 1: Write failing test stub**

Create `crates/reveried/src/mcp/add_observation_passive.rs`:

```rust
use std::sync::Arc;

use reverie_store::http::{PassiveCaptureBody, PassiveCaptureError, passive_capture_inner};
use serde_json::{Value, json};

use super::{Ctx, RpcCallError};

pub const TOOL_NAME: &str = "add_observation_passive";

pub fn schema() -> Value {
    todo!("schema")
}

pub async fn call(_ctx: &Ctx, _arguments: &Value) -> Result<Value, RpcCallError> {
    todo!("call")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reverie_store::backends::engram_compat::EngramCompatStore;
    use reverie_store::http::AppState;

    fn test_ctx() -> Ctx {
        use crate::dream_scheduler::{DreamScheduler, DreamSchedulerConfig};
        use reverie_store::config::ReveriedConfig;
        use reverie_store::events::NoopEventManager;
        let app = Arc::new(AppState::new_with_noop(EngramCompatStore::open_in_memory().unwrap()));
        let sched = Arc::new(DreamScheduler::new(
            Arc::new(NoopEventManager),
            ReveriedConfig::default(),
            DreamSchedulerConfig::default(),
        ));
        Ctx { app, sched }
    }

    #[tokio::test]
    async fn rejects_missing_project() {
        let ctx = test_ctx();
        let err = call(
            &ctx,
            &json!({"session_id": "sess-1", "content": "## Key Learnings:\n- foo"}),
        )
        .await
        .expect_err("project required");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("project"));
    }
}
```

- [ ] **Step 2: Register module + dispatcher arm**

In `mcp.rs`:

```rust
pub mod add_observation_passive;
```

Dispatcher:

```rust
add_observation_passive::TOOL_NAME => add_observation_passive::call(&ctx, &arguments).await,
```

Tools list:

```rust
add_observation_passive::schema(),
```

- [ ] **Step 3: Run failing test**

Run: `cargo test -p reveried mcp::add_observation_passive::tests::rejects_missing_project`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 4: Implement `schema()` and `call()`**

```rust
pub fn schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Passive memory capture — extracts only `## Key Learnings:` bullets from the supplied content. Distinct from add_observation (which saves raw text).",
        "inputSchema": {
            "type": "object",
            "required": ["project", "session_id", "content"],
            "properties": {
                "project": {"type": "string"},
                "session_id": {"type": "string"},
                "content": {"type": "string", "description": "Markdown body. Only `## Key Learnings:` bullets are extracted and saved."},
                "source": {"type": "string", "description": "Originating system. Defaults to 'mcp'."},
            },
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": false,
            "idempotentHint": false,
            "openWorldHint": false,
        },
    })
}

pub async fn call(ctx: &Ctx, arguments: &Value) -> Result<Value, RpcCallError> {
    let project = arguments
        .get("project")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.project required"))?;
    let session_id = arguments
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.session_id required"))?;
    let content = arguments
        .get("content")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| RpcCallError::invalid_params("arguments.content required"))?;
    let source = arguments
        .get("source")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("mcp")
        .to_string();

    let body = PassiveCaptureBody {
        session_id: session_id.to_string(),
        content: content.to_string(),
        project: Some(project.to_string()),
        source: Some(source),
    };

    match passive_capture_inner(&ctx.app, body).await {
        Ok(result) => {
            let payload = serde_json::to_value(&result)
                .map_err(|e| RpcCallError::server_error(format!("serialize result: {e}")))?;
            Ok(json!({
                "content": [
                    {
                        "type": "text",
                        "text": payload.to_string(),
                    }
                ]
            }))
        },
        Err(PassiveCaptureError::InvalidParams(msg)) => Err(RpcCallError::invalid_params(msg)),
        Err(PassiveCaptureError::InvalidSession { msg, .. }) => {
            Err(RpcCallError::invalid_params(msg))
        },
        Err(PassiveCaptureError::Store(e)) => {
            Err(RpcCallError::server_error(format!("passive_capture failed: {e}")))
        },
    }
}
```

- [ ] **Step 5: Run test — should pass**

Run: `cargo test -p reveried mcp::add_observation_passive::tests::rejects_missing_project`
Expected: PASS.

- [ ] **Step 6: Add more unit tests**

```rust
#[tokio::test]
async fn rejects_missing_session_id() {
    let ctx = test_ctx();
    let err = call(&ctx, &json!({"project": "p", "content": "## Key Learnings:\n- x"}))
        .await
        .expect_err("session_id required");
    assert_eq!(err.code, -32602);
    assert!(err.message.contains("session_id"));
}

#[tokio::test]
async fn happy_path_with_key_learnings() {
    let ctx = test_ctx();
    let result = call(&ctx, &json!({
        "project": "test-project",
        "session_id": "passive-session-1",
        "content": "## Key Learnings:\n- The blue heron flies south in winter\n- Migration patterns shift with climate",
    })).await.expect("save ok");
    let content = &result["content"][0]["text"];
    let payload: Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert!(payload.get("saved").and_then(Value::as_u64).unwrap_or(0) >= 1);
}

#[tokio::test]
async fn no_op_when_no_key_learnings_section() {
    let ctx = test_ctx();
    let result = call(&ctx, &json!({
        "project": "test-project",
        "session_id": "passive-session-2",
        "content": "Just some prose with no Key Learnings section.",
    })).await.expect("call ok");
    let content = &result["content"][0]["text"];
    let payload: Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
    assert_eq!(payload.get("saved").and_then(Value::as_u64).unwrap_or(99), 0);
}
```

- [ ] **Step 7: Run unit tests**

Run: `cargo test -p reveried mcp::add_observation_passive::tests`
Expected: 4/4 PASS.

- [ ] **Step 8: Add integration test**

Append to `tests/mcp_tools.rs`:

```rust
#[tokio::test]
async fn add_observation_passive_extracts_key_learnings() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/call",
            r#"{"jsonrpc":"2.0","method":"tools/call","id":"passive-1","params":{"name":"add_observation_passive","arguments":{"project":"reverie","session_id":"sess-int","content":"## Key Learnings:\n- foo bar baz"}}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains(r#""id":"passive-1""#));
    assert!(body.contains(r#""saved""#));
}
```

- [ ] **Step 9: Run integration tests**

Run: `cargo test -p reveried --test mcp_tools`
Expected: 5/5 PASS.

- [ ] **Step 10: Clippy + commit**

Run: `cargo clippy -p reveried --all-targets`

```bash
git add crates/reveried/src/mcp.rs crates/reveried/src/mcp/add_observation_passive.rs crates/reveried/tests/mcp_tools.rs
git commit -m "reveried/mcp: add add_observation_passive tool

Wraps passive_capture_inner — extracts only ## Key Learnings: bullets
from the input content. Project / session_id / content all required at
the MCP boundary; source defaults to 'mcp'. 4 unit tests cover
required-arg, happy-path with bullets, and no-op when the section
header is absent. 1 integration test."
```

---

## Task 9: Add `dream_status` tool

**Goal:** First read tool that uses `ctx.sched`. Wraps `DreamScheduler::status_snapshot()`.

**Files:**
- Create: `crates/reveried/src/mcp/dream_status.rs`
- Modify: `crates/reveried/src/mcp.rs`
- Modify: `crates/reveried/tests/mcp_tools.rs`

- [ ] **Step 1: Write failing test stub**

Create `crates/reveried/src/mcp/dream_status.rs`:

```rust
use serde_json::{Value, json};

use super::{Ctx, RpcCallError};

pub const TOOL_NAME: &str = "dream_status";

pub fn schema() -> Value {
    todo!("schema")
}

pub async fn call(_ctx: &Ctx, _arguments: &Value) -> Result<Value, RpcCallError> {
    todo!("call")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use reverie_store::backends::engram_compat::EngramCompatStore;
    use reverie_store::http::AppState;

    fn test_ctx() -> Ctx {
        use crate::dream_scheduler::{DreamScheduler, DreamSchedulerConfig};
        use reverie_store::config::ReveriedConfig;
        use reverie_store::events::NoopEventManager;
        let app = Arc::new(AppState::new_with_noop(EngramCompatStore::open_in_memory().unwrap()));
        let sched = Arc::new(DreamScheduler::new(
            Arc::new(NoopEventManager),
            ReveriedConfig::default(),
            DreamSchedulerConfig::default(),
        ));
        Ctx { app, sched }
    }

    #[tokio::test]
    async fn returns_initial_status() {
        let ctx = test_ctx();
        let result = call(&ctx, &json!({})).await.expect("status ok");
        let content = &result["content"][0]["text"];
        let payload: Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
        assert_eq!(payload["in_progress"], false);
        assert_eq!(payload["pending"], false);
        assert_eq!(payload["has_last_report"], false);
        assert!(payload["last_completed_ms_ago"].is_null());
    }
}
```

- [ ] **Step 2: Register module + dispatcher**

In `mcp.rs`:

```rust
pub mod dream_status;
```

Dispatcher:

```rust
dream_status::TOOL_NAME => dream_status::call(&ctx, &arguments).await,
```

Tools list:

```rust
dream_status::schema(),
```

- [ ] **Step 3: Run failing test**

Run: `cargo test -p reveried mcp::dream_status::tests::returns_initial_status`
Expected: FAIL.

- [ ] **Step 4: Implement**

```rust
pub fn schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Daemon-wide dream-cycle scheduler status. Returns in_progress, pending, last_completed_ms_ago, and has_last_report.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

pub async fn call(ctx: &Ctx, _arguments: &Value) -> Result<Value, RpcCallError> {
    let snapshot = ctx.sched.status_snapshot().await;
    let payload = serde_json::to_value(&snapshot)
        .map_err(|e| RpcCallError::server_error(format!("serialize status: {e}")))?;
    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": payload.to_string(),
            }
        ]
    }))
}
```

- [ ] **Step 5: Run unit tests**

Run: `cargo test -p reveried mcp::dream_status::tests`
Expected: PASS.

- [ ] **Step 6: Add integration test**

Append to `tests/mcp_tools.rs`:

```rust
#[tokio::test]
async fn dream_status_returns_initial_state() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/call",
            r#"{"jsonrpc":"2.0","method":"tools/call","id":"ds-1","params":{"name":"dream_status","arguments":{}}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains(r#""id":"ds-1""#));
    assert!(body.contains("in_progress"));
    assert!(body.contains("has_last_report"));
}
```

- [ ] **Step 7: Run integration tests**

Run: `cargo test -p reveried --test mcp_tools`
Expected: 6/6 PASS.

- [ ] **Step 8: Clippy + commit**

```bash
git add crates/reveried/src/mcp.rs crates/reveried/src/mcp/dream_status.rs crates/reveried/tests/mcp_tools.rs
git commit -m "reveried/mcp: add dream_status tool

Daemon-wide read tool. Wraps DreamScheduler::status_snapshot —
returns in_progress / pending / last_completed_ms_ago / has_last_report.
No project arg: dream cycles are daemon-wide, not project-scoped.
1 unit test + 1 integration test."
```

---

## Task 10: Add `dream_last_report` tool

**Goal:** Second dream tool. Wraps `DreamScheduler::last_report()` → `Option<DreamReport>`.

**Files:**
- Create: `crates/reveried/src/mcp/dream_last_report.rs`
- Modify: `crates/reveried/src/mcp.rs`
- Modify: `crates/reveried/tests/mcp_tools.rs`

- [ ] **Step 1: Write failing test stub**

Create `crates/reveried/src/mcp/dream_last_report.rs`:

```rust
use serde_json::{Value, json};

use crate::dream_routes::DreamReportWire;

use super::{Ctx, RpcCallError};

pub const TOOL_NAME: &str = "dream_last_report";

pub fn schema() -> Value {
    todo!("schema")
}

pub async fn call(_ctx: &Ctx, _arguments: &Value) -> Result<Value, RpcCallError> {
    todo!("call")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use reverie_store::backends::engram_compat::EngramCompatStore;
    use reverie_store::http::AppState;

    fn test_ctx() -> Ctx {
        use crate::dream_scheduler::{DreamScheduler, DreamSchedulerConfig};
        use reverie_store::config::ReveriedConfig;
        use reverie_store::events::NoopEventManager;
        let app = Arc::new(AppState::new_with_noop(EngramCompatStore::open_in_memory().unwrap()));
        let sched = Arc::new(DreamScheduler::new(
            Arc::new(NoopEventManager),
            ReveriedConfig::default(),
            DreamSchedulerConfig::default(),
        ));
        Ctx { app, sched }
    }

    #[tokio::test]
    async fn returns_null_when_no_report() {
        let ctx = test_ctx();
        let result = call(&ctx, &json!({})).await.expect("call ok");
        let content = &result["content"][0]["text"];
        let payload: Value = serde_json::from_str(content.as_str().unwrap()).unwrap();
        assert!(payload.is_null(), "expected null payload, got {payload}");
    }
}
```

- [ ] **Step 2: Register module + dispatcher**

In `mcp.rs`:

```rust
pub mod dream_last_report;
```

Dispatcher:

```rust
dream_last_report::TOOL_NAME => dream_last_report::call(&ctx, &arguments).await,
```

Tools list:

```rust
dream_last_report::schema(),
```

- [ ] **Step 3: Run failing test**

Run: `cargo test -p reveried mcp::dream_last_report::tests::returns_null_when_no_report`
Expected: FAIL.

- [ ] **Step 4: Implement**

```rust
pub fn schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Most recent completed dream cycle's report. Returns phases (with counts and durations), total_duration_ms, and dry_run flag. Null content block when no cycle has run yet.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": true,
            "destructiveHint": false,
            "idempotentHint": true,
            "openWorldHint": false,
        },
    })
}

pub async fn call(ctx: &Ctx, _arguments: &Value) -> Result<Value, RpcCallError> {
    let report = ctx.sched.last_report().await;
    let text = match report.as_ref() {
        Some(r) => {
            let wire = DreamReportWire::from(r);
            serde_json::to_value(&wire)
                .map_err(|e| RpcCallError::server_error(format!("serialize report: {e}")))?
                .to_string()
        },
        None => "null".to_string(),
    };
    Ok(json!({
        "content": [
            {
                "type": "text",
                "text": text,
            }
        ]
    }))
}
```

- [ ] **Step 5: Run unit tests**

Run: `cargo test -p reveried mcp::dream_last_report::tests`
Expected: PASS.

- [ ] **Step 6: Add integration test**

Append to `tests/mcp_tools.rs`:

```rust
#[tokio::test]
async fn dream_last_report_returns_null_initially() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/call",
            r#"{"jsonrpc":"2.0","method":"tools/call","id":"dlr-1","params":{"name":"dream_last_report","arguments":{}}}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    assert!(body.contains(r#""id":"dlr-1""#));
    assert!(body.contains(r#""text":"null""#));
}

#[tokio::test]
async fn tools_list_returns_six_tools() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/list",
            r#"{"jsonrpc":"2.0","method":"tools/list","id":"list-1"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_string(response).await;
    for name in [
        "search_memory",
        "smart_context",
        "add_observation",
        "add_observation_passive",
        "dream_status",
        "dream_last_report",
    ] {
        assert!(body.contains(&format!(r#""name":"{name}""#)), "missing tool {name} in {body}");
    }
}

#[tokio::test]
async fn unknown_tool_returns_method_not_found() {
    let app = test_app();
    let response = app
        .oneshot(authed_call(
            "tools/call",
            r#"{"jsonrpc":"2.0","method":"tools/call","id":"unk-1","params":{"name":"nonexistent_tool","arguments":{}}}"#,
        ))
        .await
        .unwrap();
    let body = body_string(response).await;
    assert!(body.contains(r#""code":-32601"#));
    assert!(body.contains("nonexistent_tool"));
}
```

- [ ] **Step 7: Run integration tests**

Run: `cargo test -p reveried --test mcp_tools`
Expected: 9/9 PASS.

- [ ] **Step 8: Run full test suite**

Run: `cargo test -p reveried && cargo test -p reverie-store`
Expected: All green.

- [ ] **Step 9: Clippy clean**

Run: `cargo clippy -p reveried --all-targets && cargo clippy -p reverie-store --all-targets`
Expected: No new warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/reveried/src/mcp.rs crates/reveried/src/mcp/dream_last_report.rs crates/reveried/tests/mcp_tools.rs
git commit -m "reveried/mcp: add dream_last_report tool

Wraps DreamScheduler::last_report → DreamReportWire (phases, counts,
durations). Null content block when no cycle has run.

Phase 0 tool surface complete: search_memory, smart_context,
add_observation, add_observation_passive, dream_status,
dream_last_report. tools/list returns six entries; envelope-level
integration tests confirm tools/list shape and unknown-tool fallback."
```

---

## Task 11: CHANGELOG entry

**Goal:** Document the new MCP surface in reverie's CHANGELOG.

**Files:**
- Modify: `/Users/dennis/programming projects/reverie/CHANGELOG.md`

- [ ] **Step 1: Add an `Added` entry under `[Unreleased]`**

In `CHANGELOG.md` near the top of the existing `[Unreleased] / Added` block, prepend:

```markdown
- **reveried MCP context surface (Phase 0)**: `/mcp` JSON-RPC server expanded from one tool (`search_memory`) to six. New tools: `smart_context` (project-scoped tiered context loader; wraps `/context/smart`), `add_observation` (direct write through the existing write-gate / contradiction-detector / event-publish pipeline), `add_observation_passive` (`## Key Learnings:` extraction), `dream_status` (daemon-wide scheduler snapshot), `dream_last_report` (`DreamReportWire` of the most recent cycle; null when none). Project required at the MCP boundary on every memory-touching tool. `mcp_sse.rs` refactored into `mcp.rs` + `mcp/<tool>.rs` per-tool modules; `add_observation_inner` and `passive_capture_inner` extracted in `reverie-store::http` so the same business logic powers both HTTP and MCP. `McpState` now carries `Arc<DreamScheduler>` alongside `Arc<AppState>`. SSE transport, `Mcp-Method` header validation, and `REVERIE_MCP_TOKEN` bearer auth all unchanged. 18+ new unit tests across the per-tool modules + 9 integration tests in `tests/mcp_tools.rs` covering happy-path, validation, and envelope-level checks.
```

- [ ] **Step 2: Run a final full-suite check**

Run: `cargo test -p reveried && cargo test -p reverie-store && cargo clippy -p reveried --all-targets && cargo clippy -p reverie-store --all-targets`
Expected: all green.

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md
git commit -m "changelog: reveried MCP Phase 0

Documents the six-tool MCP surface and the supporting refactor."
```

---

## Acceptance check (end of Phase 0)

After Task 11 commits:

- [ ] `cargo test -p reveried --test mcp_tools` → 9/9 pass
- [ ] `cargo test -p reveried mcp::` → all per-tool unit tests pass
- [ ] `cargo test -p reverie-store` → all existing tests still pass
- [ ] `cargo clippy --workspace --all-features` → no new warnings
- [ ] Manual smoke: with `REVERIE_ENABLE_MCP=1` and `REVERIE_MCP_TOKEN=<token>`, run `reveried serve` and POST to `/mcp` with `tools/list` — expect 6 tools.
