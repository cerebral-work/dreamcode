# Phase 0 — Reverie MCP Context Server (Design)

**Status:** Design — pending implementation plan
**Date:** 2026-04-29
**Phase:** 0 of the dreamcode ↔ reverie integration
**Touches:** reverie only (no dreamcode changes)

## Background

The dreamcode ↔ reverie integration was originally scoped in three phases:

- **Phase 0** — MCP context server wrapping reverie's retrieval and write surface, so any MCP host (Claude Desktop, Cursor, the Zed agent panel) can reach reverie without forking Zed.
- **Phase 1** — In-process `AgentServer` linking `reverie-deepagent` directly into dreamcode (B2). Plan: `2026-04-20-reverie-agent-backend.md`.
- **Phase 2** — Dream Inspector panel in dreamcode. Plan: `2026-04-22-phase-2-dream-inspector.md`.

Phase 1 (and the 1.5a–e follow-ups) and Phase 2 are already specced and partly implemented. Phase 0 was deferred until reverie had its own MCP scaffolding to extend rather than build from scratch — that scaffolding now exists.

## Current state

`crates/reveried/src/mcp_sse.rs` (629 LOC) implements an MCP SSE server at `POST /mcp` with:

- JSON-RPC 2.0 envelope, `Mcp-Method` header validation, `text/event-stream` framing.
- `require_bearer_auth` middleware against `REVERIE_MCP_TOKEN` (constant-time compare).
- `initialize`, `tools/list`, `tools/call` methods.
- One tool: `search_memory` — project-scoped FTS+vector RRF over `/search/v2`.

13 unit tests at the bottom of the file cover the existing surface.

## Goals

Expand the MCP tool surface from one tool to six. After this phase:

- **`search_memory`** — existing; unchanged.
- **`smart_context`** — wraps `/context/smart` (project-aware tiered context loader, TOD-257).
- **`add_observation`** — wraps `POST /observations` (direct-add, raw text + tags + write-gate).
- **`add_observation_passive`** — wraps `POST /observations/passive` (extracts `## Key Learnings:` bullets only).
- **`dream_status`** — wraps `GET /dream/status` (daemon-wide consolidation state).
- **`dream_last_report`** — wraps `GET /dream/last-report` (most recent dream cycle).

## Non-goals

- **Stdio transport.** Today's MCP is SSE-only. Stdio (for Claude Desktop) would be a separate sidecar binary (`reverie-mcp` crate). Tracked but deferred.
- **MCP-side rate limiting.** `/search`, `/search/v2`, and `/context/smart` are HTTP-rate-limited via `SEARCH_RATE_LIMITED_ROUTES`. The MCP path calls `format_smart_context` directly via `AppState` and bypasses that limiter. Documented gap; not blocking Phase 0.
- **Streaming tools.** None of the five new tools need streaming (no long-running progress events).
- **Project-scoped dream tools.** Reverie's dream cycles are daemon-wide today (no per-project phase counts); forcing a `project` arg on dream tools would be a fiction.
- **Dreamcode changes.** Zero. Phase 0 is entirely reverie-internal.

## Architecture

### File layout

The existing `mcp_sse.rs` becomes a module:

```
crates/reveried/src/
  mcp.rs                         # module root
  mcp/
    search_memory.rs             # moved from mcp_sse.rs
    smart_context.rs             # new
    add_observation.rs           # new
    add_observation_passive.rs   # new
    dream_status.rs              # new
    dream_last_report.rs         # new
```

Renames `mcp_sse.rs` → `mcp.rs`. Justification: every tool still rides SSE; the `_sse` suffix added clarity when there was one transport-flavored file, but with a module of tools it's noise.

### Per-tool module shape

Each file exports two free functions and one constant:

```rust
pub const TOOL_NAME: &str = "smart_context";
pub fn schema() -> serde_json::Value { /* tools/list entry */ }
pub async fn call(
    app: Arc<reverie_store::http::AppState>,
    sched: Arc<crate::dream_scheduler::DreamScheduler>,
    args: &serde_json::Value,
) -> Result<serde_json::Value, RpcCallError>;
```

Tools that don't need the dream scheduler ignore the `sched` parameter — keeping the signature uniform makes the dispatcher table mechanical. (If the unused parameter feels noisy, a thin `Ctx { app, sched }` struct is a clean alternative — decide during implementation.)

### Dispatcher in `mcp.rs`

`tools_list_result()` iterates a static array `&[(name, schema_fn)]`. `handle_tools_call` matches on tool name and dispatches to the right `call`. Adding a 7th tool later = one new module + two lines in `mcp.rs`.

### State plumbing

`McpState` extends to carry `Arc<DreamScheduler>` alongside `Arc<AppState>` and `token`:

```rust
pub struct McpState {
    pub app: Arc<reverie_store::http::AppState>,
    pub sched: Arc<DreamScheduler>,
    pub token: Arc<String>,
}
```

The `mcp::router(state)` call site in `main.rs` and `upgrade.rs` (both the `serve` and SIGUSR2 handoff paths) updates to pass the scheduler handle that's already constructed there for `dream_routes::router(...)`.

### Auth, transport, JSON-RPC envelope

Unchanged. `require_bearer_auth` middleware, `Mcp-Method` header validation, `text/event-stream` response framing — all reused from today's `mcp_sse.rs`.

### Inner-function refactor in `reverie-store`

The MCP layer needs to reuse the business logic of `handle_add_observation` and `handle_passive_capture` without going through axum's extractor machinery. Each handler's body is extracted into a free function:

```rust
// Before
async fn handle_add_observation(State(state): State<SharedState>, Json(body): Json<AddObservationParams>) -> Response { /* 140 lines */ }

// After
pub(crate) async fn add_observation_inner(
    state: &SharedState,
    body: AddObservationParams,
) -> Result<AddObservationOutcome, AddObservationError>;

async fn handle_add_observation(State(state): State<SharedState>, Json(body): Json<AddObservationParams>) -> Response {
    match add_observation_inner(&state, body).await { /* 5-line adapter */ }
}
```

`AddObservationOutcome { id: i64, gate_warnings: Vec<Issue> }`. `AddObservationError` is an enum covering missing-required-fields, write-gate-strict-rejection (with `reasons`), and underlying store errors. Same shape for `passive_capture_inner`.

`handle_smart_context` already separates formatting (`format_smart_context_tokens`, `format_smart_context`) from the axum wrapper. The MCP `smart_context` tool calls those formatters directly — no refactor needed in `reverie-store`.

`DreamScheduler::status_snapshot()` and `DreamScheduler::last_report()` are already pub async methods returning serializable types — no refactor needed in `reveried`.

## Per-tool schemas

### Output shape (uniform across all tools)

Every tool returns `{"content": [...]}` per MCP `tools/call` result spec, where each content entry is a text block `{"type": "text", "text": "..."}`. This matches the existing `search_memory` convention (`mcp_sse.rs:search_memory_content_blocks`).

- Tools that return human-readable prose (`smart_context`, `search_memory`) emit one text block with the formatted text.
- Tools that return structured data (`add_observation`, `add_observation_passive`, `dream_status`, `dream_last_report`) emit one text block containing the JSON-stringified result. MCP hosts that want structured data can parse the text; hosts that want a quick glance read it as-is.
- Empty / null results emit one text block with a sentinel message (matches `search_memory`'s `"No memories found for: ..."` pattern).

### `search_memory` (existing — unchanged)

Kept verbatim during the module split. Tests move with the file.

### `smart_context` *(read, project-scoped)*

- **Required:** `project` (string, non-empty)
- **Optional:** `query` (string), `limit` (int 1–50), `token_budget` (int), `model` (string), `adaptive` (bool, default false)
- **Calls:** `format_smart_context_tokens(&store, Some(project), &cfg, tb)` when a token budget resolves; else `format_smart_context(&store, Some(project), &cfg)`.
- **Output:** MCP content block with the formatted text blob.
- **Annotations:** `readOnlyHint: true, idempotentHint: true, openWorldHint: false`.

### `add_observation` *(write, project-required)*

- **Required:** `project`, `title`, `content` (all non-empty strings).
- **Optional:** `session_id`, `type`, `tool_name`, `topic_key`, `tags` (`[{facet, value}]`), `supersedes`, `event_id`, `confidence` (0.0–1.0), `source` (default `"mcp"`), `discovery_tokens`, `scope`, `skip_content_dedupe`.
- **Calls:** `add_observation_inner(&state, params)`.
- **Output:** one text block with JSON `{"id": <i64>, "status": "saved", "gate_warnings": [...]}`.
- **Strict-mode write-gate rejection:** MCP error `-32000` with `reasons` array.
- **Annotations:** `readOnlyHint: false, destructiveHint: false, idempotentHint: false`.
- **Hardening:** `project` required at the MCP boundary even though HTTP is permissive; matches `search_memory` precedent and the project-tagged-memory principle.

### `add_observation_passive` *(write, project-required)*

- **Required:** `project`, `session_id`, `content`.
- **Optional:** `source`.
- **Calls:** `passive_capture_inner(&state, params)`.
- **Output:** one text block with JSON `PassiveCaptureResult` (saved count + per-bullet detail).
- **Annotations:** same as `add_observation`.
- **Hardening:** `project` required even though the HTTP route accepts it as optional — keeps the MCP write surface consistent.

### `dream_status` *(read, daemon-wide)*

- **Inputs:** none.
- **Calls:** `DreamScheduler::status_snapshot()`.
- **Output:** `{in_progress, pending, last_completed_ms_ago, has_last_report}`.
- **Annotations:** `readOnlyHint: true, idempotentHint: true`.

### `dream_last_report` *(read, daemon-wide)*

- **Inputs:** none.
- **Calls:** `DreamScheduler::last_report()` → `Option<DreamReport>`, mapped via the existing `DreamReportWire` (`{phases, total_duration_ms, dry_run}`).
- **Output:** the report, or a `null` content block when no report is cached.
- **Annotations:** `readOnlyHint: true, idempotentHint: true`.

## Errors

Uniform across all tools:

- Missing required field → `-32602 invalid_params` with field name.
- Empty string for required field → `-32602` (matches HTTP behavior).
- Project not found / no observations → empty result, not error (matches `search_memory`).
- Underlying store error → `-32000 server_error` with sanitized message (no internal paths).
- Auth failure handled by `require_bearer_auth` middleware before reaching the dispatcher (HTTP 401 — never enters JSON-RPC).
- Strict-mode write-gate rejection on `add_observation` → `-32000` with `reasons` array.

## Testing

### Unit tests (per-tool, in `#[cfg(test)] mod tests` blocks)

| Tool | Cases |
|---|---|
| `smart_context` | required-arg validation; empty `project` rejected; `limit` clamping; happy-path text block; store-error → server_error |
| `add_observation` | required-arg validation; strict-gate rejection → -32000 with reasons; warn-mode → result with `gate_warnings`; `source` default `"mcp"` applied; tag attach; supersedes wiring |
| `add_observation_passive` | session_id+content+project required; happy-path with `saved > 0`; no-op batch (`saved == 0`) |
| `dream_status` | snapshot fields wired; no-args path |
| `dream_last_report` | `null` content block when no cached report; populated `DreamReportWire` shape |
| `search_memory` | existing 13 tests retained verbatim from `mcp_sse.rs` (move, don't rewrite) |

Each unit test bypasses the MCP envelope and exercises the per-tool `call(...)` directly. Test fixtures use the in-memory `AppState` builder already present in `reverie-store` test helpers (verify exact path during implementation).

### Integration tests (`crates/reveried/tests/mcp_tools.rs`, new file)

One test per tool against the live `/mcp` route via `axum::Router::oneshot`:

1. Build `Router` with test `McpState` (in-memory store, ephemeral DreamScheduler).
2. Send `POST /mcp` with `Authorization: Bearer ...`, `Mcp-Method: tools/call`, JSON-RPC body.
3. Assert SSE response framing (`event: message\ndata: ...`) parses to expected JSON-RPC result shape.

Plus three envelope-level tests:

- `tools/list` returns 6 tools with stable schema.
- Missing bearer token → 401, never reaches dispatcher.
- Unknown tool name → method_not_found.

### Out of scope for testing

- Cross-tool flows (write then search) — covered by reverie's broader suite.
- SSE long-lived streaming — single-response per request; no streaming tools.
- Rate-limit-bypass — separate followup.

## Implementation order

1. **Refactor `mcp_sse.rs` → `mcp.rs` + `mcp/search_memory.rs`**, no behavior change. Existing 13 unit tests move with the module. Run `./script/clippy` + tests — must pass before any new tool lands.
2. **Extract inner functions** in `reverie-store/src/http/mod.rs`: `add_observation_inner` and `passive_capture_inner`. Axum handlers become 5-line adapters. Run reverie's full test suite to confirm no regression.
3. **`smart_context` tool** — read-only, simplest plumbing. Lands first to validate the new module shape.
4. **`add_observation` tool** — first write tool. Exercises gate warnings, tag attach, `source` defaulting.
5. **`add_observation_passive` tool**.
6. **`McpState` extension** to carry `Arc<DreamScheduler>` — touches `mcp.rs::router(...)` signature and the call site in `main.rs`/`upgrade.rs`. One commit, zero new tools yet.
7. **`dream_status` tool**.
8. **`dream_last_report` tool**.
9. **CHANGELOG entry** + integration test pass.

## Acceptance

Phase 0 is done when:

- `tools/list` returns 6 tools (existing 1 + 5 new).
- All 6 callable end-to-end via JSON-RPC over `/mcp`.
- New tools have ≥1 unit test each + ≥1 integration test each.
- `./script/clippy` clean, `cargo test -p reveried` green, `cargo test -p reverie-store` green.
- One sanity check from a real MCP host (Claude Desktop or equivalent) — manual, not in CI.

## Followups (not Phase 0)

- **Stdio transport** — separate `reverie-mcp` sidecar crate; revisit if Claude Desktop integration becomes a goal.
- **MCP-side rate limiting** — per-bearer-token counter in `mcp.rs`; covers the bypass gap.
- **Project-scoped dream tools** — depends on reverie supporting per-project dream cycles, which it doesn't today.
- **Streaming tools** — none of the current 5 need it; revisit if a long-running tool (e.g. `dream_trigger`) is added.
