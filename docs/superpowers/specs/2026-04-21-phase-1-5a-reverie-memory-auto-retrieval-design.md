# Phase 1.5a: Reverie Memory Auto-Retrieval (in-process, Reverie only) — Design

**Status:** Draft, pending user review.
**Scope:** Automatic memory retrieval and passive observation save, implemented in-process inside `ReverieAgentConnection::prompt()`. Only the Reverie agent benefits in this phase. Universal middleware wrapper (Phase 1.5b) and standalone MCP server (Phase 2+) are out of scope.

**Context:** Phase 1 shipped the Reverie agent as an in-process Zed `AgentServer` backed by `reverie-deepagent`, but each prompt creates a fresh `Run` with no memory of past sessions. Phase 0 (MCP context server) was deferred. This spec targets the highest-leverage slice of the memory integration — auto-retrieval and passive save for the Reverie agent — so the planner sees prior work from its very first iteration without waiting on either the MCP path or cross-prompt persistence work.

---

## 1. Architecture

Phase 1.5a adds two HTTP calls around the existing planner loop in `ReverieAgentConnection::prompt()`, plus one small observer-style UI notification:

```
prompt() (foreground, AsyncApp):
  ┌─ resolve project name
  │    (env REVERIE_PROJECT override → worktree root_name → "unknown-project")
  │
  ├─ GET {base}/context/smart?q=<user_text>&project=<project>
  │    [NEW, 5s timeout, graceful degradation to None on failure]
  │    ↓
  │    if hit: emit AgentMessageChunk "[memory] retrieved N items (project=foo)"
  │    if hit: prepend "Relevant memory:\n<block>\n\n" to user_text before seed
  │
  ├─ smol::unblock(run_planner_with_observer)                    [UNCHANGED]
  │    ↓
  │    planner runs with seeded user_text, observer pumps updates as before
  │
  ├─ planner_task.await → PlannerResult
  │
  ├─ IF termination == Completed:
  │    (fire-and-forget, 5s timeout, errors logged at debug)
  │    POST {base}/observations/passive
  │       { title: "user-intent", content: <original_prompt>,
  │         project, topic_key: "agent-session/<session_uuid>" }
  │    POST {base}/observations/passive
  │       { title: "run-summary",  content: <summary>,
  │         project, topic_key: "agent-session/<session_uuid>" }
  │
  └─ return PromptResponse
```

**Scope is a single file plus one new module.** All retrieval/save logic lives in `connection.rs::prompt()` and a new `http.rs`. No changes to `backend.rs`, `observer.rs`, `server.rs`'s flow beyond passing the `ReverieHttpClient` through, `agent_ui`, or the reverie repo.

---

## 2. Components & Interfaces

### 2.1 New file: `crates/reverie_agent/src/http.rs` (~120 lines)

```rust
pub struct ReverieHttpClient {
    base_url: String,       // resolved from REVERIE_URL or default
    http: reqwest::Client,  // 5s default timeout, rustls-tls
    project: String,        // resolved at construction from agent server
}

pub struct SmartContext {
    pub content: String,    // pre-formatted context block from reverie
    pub hit_count: usize,   // for logging and UI chunk
}

impl ReverieHttpClient {
    pub fn new(base_url: String, project: String) -> Result<Self>;

    /// Returns `Ok(None)` on any non-fatal failure (timeout, connection
    /// refused, 5xx, empty result). Caller proceeds without memory context.
    pub async fn smart_context(&self, query: &str) -> Result<Option<SmartContext>>;

    /// Fire-and-forget. Errors logged at debug, never propagated.
    pub async fn save_passive(
        &self,
        title: &str,
        content: &str,
        topic_key: &str,
    ) -> Result<()>;
}
```

### 2.2 Modified file: `crates/reverie_agent/src/connection.rs`

`ReverieAgentConnection` gains one field:

```rust
http_client: Arc<ReverieHttpClient>,
```

Constructor signature becomes `ReverieAgentConnection::new(model, http_client)`.

`prompt()` adds two awaits and one UI event:

```rust
// In the cx.spawn async block, BEFORE smol::unblock:
let original_prompt = user_text.clone();
let memory = http_client.smart_context(&user_text).await.ok().flatten();
if let Some(ref ctx) = memory {
    // UI breadcrumb so the user sees memory was consulted.
    let _ = thread_weak.update(cx, |thread, cx| {
        let msg = format!(
            "[memory] retrieved {} relevant item(s) (project={})",
            ctx.hit_count, http_client.project()
        );
        let chunk = acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(msg),
        ));
        let _ = thread.handle_session_update(
            acp::SessionUpdate::AgentMessageChunk(chunk), cx,
        );
    });
    user_text = format!("Relevant memory:\n{}\n\n{}", ctx.content, user_text);
}

// ... existing smol::unblock block, unchanged ...

// AFTER planner_task.await, if termination == Completed:
if matches!(planner_result.termination, TerminationReason::Completed) {
    let topic_key = format!("agent-session/{}", session_id.0.as_ref());
    let _ = http_client.save_passive(
        "user-intent", &original_prompt, &topic_key,
    ).await;
    let _ = http_client.save_passive(
        "run-summary", &summary, &topic_key,
    ).await;
}
```

### 2.3 Modified file: `crates/reverie_agent/src/server.rs`

`ReverieAgentServer::connect` now:
1. Resolves `base_url` from `env.REVERIE_URL` → shell `REVERIE_URL` → `"http://localhost:7437"`.
2. Resolves `project` from `env.REVERIE_PROJECT` → `project.read(cx).visible_worktrees(cx).next().root_name()` → `"unknown-project"`.
3. Builds `ReverieHttpClient` once; wraps in `Arc`.
4. Passes into `ReverieAgentConnection::new(model, Arc::clone(&http_client))`.

### 2.4 Dependencies

**Added to `crates/reverie_agent/Cargo.toml`:**

```toml
reqwest = { workspace = true, default-features = false, features = ["json", "rustls-tls"] }
```

`reqwest` is already in the workspace (used by many Zed crates). Setting `default-features = false` avoids double-building `native-tls` alongside the `rustls-tls` selection.

**Added to `[dev-dependencies]`:**

```toml
tiny_http = "0.12"   # only used in unit tests to stand up ad-hoc servers on 127.0.0.1:0
```

### 2.5 Settings surface

No new settings schema. Users configure via `agent_servers.reverie.env`, which already exists on `CustomAgentServer` and is honoured by the reverie registration in `agent_ui`:

```json
{
  "agent_servers": {
    "reverie": {
      "env": {
        "REVERIE_URL": "http://localhost:7437",
        "REVERIE_PROJECT": "my-project"
      }
    }
  }
}
```

Both keys are optional; sensible defaults apply.

---

## 3. Error Handling & UX

### 3.1 Retrieval (pre-planner)

- **5s timeout per call** enforced by `reqwest::Client`.
- **Graceful degradation:** on timeout, connection refused, 5xx, or deserialization error, `smart_context` returns `Ok(None)`. The prompt turn proceeds with no memory context, and the user sees no error.
- **Empty result** (reverie returned zero hits) also returns `Ok(None)` — no UI noise.
- The pre-planner retrieval await is awaited directly in the foreground `cx.spawn` block and counts against the time-to-first-token budget, so 5s is the hard ceiling.

### 3.2 Save (post-planner)

- **Fire-and-forget** semantics. Each `save_passive` call is awaited but its `Result` is discarded. The `prompt()` task completes regardless.
- **5s timeout per call** — prevents a hung daemon from delaying the `PromptResponse`.
- **Partial failure is fine.** If one of the two observations succeeds and the other fails, we accept the partial state. Reverie's dream cycles will consolidate; losing one record is low-signal.
- **Skipped on non-Completed termination.** `MaxIterations`, `GaveUp`, `Backend`, `EmptyCompletion`, and `Cancelled` runs do not save — the planner couldn't reach a clean endpoint, so saving the partial outcome would pollute the vault.

### 3.3 UI visibility

- **On successful retrieval with ≥1 hit:** one `AgentMessageChunk` chunk: `"[memory] retrieved N relevant item(s) (project=<name>)"`. Lets the user see that memory was consulted without dumping the full block into the chat.
- **On retrieval failure or empty result:** no UI message. Silent degradation is correct — memory is an enhancement, not a user-invoked operation.
- **On save success/failure:** no UI message. Save is background housekeeping.

### 3.4 First-fail logging

- On the first per-session request that fails with connection-refused, log once at `info`: `"reverie daemon unreachable at <url>, continuing without memory. Start reveried or set REVERIE_URL."`
- Subsequent failures in the same session log at `debug` only (no log spam).
- Tracked via an `AtomicBool` on `ReverieHttpClient`.

### 3.5 Topic key scheme

All observations from a single prompt share `topic_key = "agent-session/<session_uuid>"`, where `<session_uuid>` is the `acp::SessionId` created in `new_session`. Lets reverie's dream cycles group related records when consolidating.

### 3.6 Project name resolution order

1. `env["REVERIE_PROJECT"]` on the agent server config.
2. Shell `REVERIE_PROJECT` env var.
3. `project.visible_worktrees(cx).next().root_name()` — the first visible worktree's root directory name.
4. Fallback literal `"unknown-project"`.

### 3.7 Truncation

- `smart_context` response is injected as-is. Reverie's `/context/smart` already has adaptive server-side budgeting (tiktoken cl100k_base, model-aware) per reverie's own README, so we trust it.
- User prompt and summary are saved verbatim — reverie handles its own size caps.

### 3.8 Settings & env precedence

1. `agent_servers.reverie.env.REVERIE_URL` (settings.json).
2. Shell env `REVERIE_URL`.
3. Default `http://localhost:7437`.

Same order applies to `REVERIE_PROJECT`.

---

## 4. Testing

### 4.1 Unit tests (`crates/reverie_agent/src/tests.rs`, pure Rust)

All tests stand up a `tiny_http` server on `127.0.0.1:0` (kernel-picked port) to avoid coupling to a real reveried.

1. **`http_client_returns_none_on_connection_refused`** — point the client at a port that's guaranteed bound-and-closed; assert `smart_context` returns `Ok(None)` within the timeout window. Tightens against regressions where a transport error becomes an uncaught `Err`.

2. **`http_client_parses_smart_context_response`** — test server serves canned `/context/smart` JSON with a known `content` and `hit_count`; assert round-trip matches.

3. **`http_client_save_passive_serializes_correctly`** — test server captures the POST body and verifies `title`, `content`, `project`, and `topic_key` fields.

4. **`http_client_respects_base_url_override`** — construct client with a custom `base_url`; assert calls hit that host.

5. **`http_client_timeout_enforced`** — test server delays its response past 5s; assert the client returns `Ok(None)` (for retrieval) or logs and returns `Ok(())` (for save).

6. **`memory_context_prepended_to_user_text`** — given a mock server returning a known context block, unit-test the text composition (via a testable helper extracted from `prompt()`) and assert `user_text` starts with `"Relevant memory:\n<block>\n\n"`.

7. **`save_skipped_on_non_completed_termination`** — simulate a `PlannerResult { termination: MaxIterations, .. }` against a mock server; assert no POST hits the `/observations/passive` endpoint.

8. **`save_fires_both_observations_on_completed`** — simulate `Completed`; assert two POSTs land with identical topic_key and distinct titles (`user-intent`, `run-summary`).

### 4.2 Not tested in Phase 1.5a

- **Retrieval quality** — whether the returned context is useful is a reverie concern.
- **Dream cycle consolidation** of the passive observations — reverie's own test surface.
- **Concurrent prompts against the same session** — Phase 1.5a doesn't change concurrency semantics.
- **Full Zed + reveried integration** — deferred to a manual smoke step in `docs/reverie-agent.md`.

### 4.3 Manual smoke step

After Phase 1.5a lands, document in `docs/reverie-agent.md`:
1. Start reveried: `reveried serve`.
2. Start Zed: `./target/release/zed`.
3. Open the Reverie agent, prompt something related to prior work.
4. Verify the `[memory] retrieved N items` chunk appears before the planner starts.
5. After the turn finishes (Completed), run `curl localhost:7437/observations/recent | jq .` and confirm two records with `topic_key="agent-session/..."`.

---

## 5. Known Limitations (Phase 1.5a)

- **Reverie agent only.** Claude / Gemini / Zed-native agents see no memory. Phase 1.5b (universal wrapper middleware) addresses this.
- **One retrieval call per prompt.** No per-iteration or per-spawn retrieval. A follow-up (Phase 1.5a.2) can add the Spawn trigger from the original sequence.
- **Single worktree.** Multi-worktree projects collapse to the first visible worktree's name.
- **No retry.** A single-shot transport failure means no memory context for that turn. Acceptable given graceful degradation; simpler than backoff logic.

## 6. Invoke-After

Per the brainstorming skill, the terminal state after user approval is to invoke the `writing-plans` skill for the implementation plan.

---

## Self-Review (inline)

- **Placeholder scan:** No TBDs, TODOs, or vague requirements.
- **Internal consistency:** Architecture diagram (§1), component interfaces (§2), and error handling (§3) agree on: endpoint paths, timeout values, graceful-degradation semantics, topic_key scheme, save skip policy. Project name resolution documented identically in §1 and §3.6.
- **Scope check:** One spec, one agent, two endpoints, single file changed (plus one new file). Under 400 LOC of implementation work. Phase 1.5b and Phase 2 are explicitly out of scope and named as separate specs.
- **Ambiguity check:** Save timing is explicit (`if Completed`, two POSTs, topic_key scheme named). UI chunk wording is quoted verbatim. Env precedence listed as a numbered order. Truncation policy is "trust reverie's server-side budget."
