# Phase 1.5b — Universal Memory Middleware — Design

**Status:** Draft, pending user review.
**Scope:** A wrapper `AgentServer` + `AgentConnection` pair that auto-injects reverie memory retrieval into every non-Reverie agent's prompts. Claude, Gemini, Zed-native, and Custom agents all get memory through the same `/context/smart` path that Phase 1.5a wired up for Reverie. Save captures user intent only (no assistant-side capture) via `/observations/passive`.

**Context:** Phase 1.5a delivered memory auto-retrieval for the Reverie agent in-process. Every other agent in Zed's agent panel — Claude, Gemini, the built-in Zed agent, any Custom ACP servers — sees no memory. This spec generalizes by wrapping each agent's `AgentServer` in a `ReverieAugmentedAgentServer` that interposes `/context/smart` and `/observations/passive` calls around the inner connection's `prompt()` method. No changes to individual agent implementations; no changes to ACP; no new reverie endpoints.

**Cross-phase coordination:**
- Reuses `reverie_agent::ReverieHttpClient` from Phase 1.5a unchanged.
- Orthogonal to Phase 1.5c (persistence — that's Reverie-agent-specific).
- Orthogonal to Phase 1.5d (mid-call cancel — the inner `AgentConnection` handles its own cancel; the wrapper just delegates).
- Orthogonal to Phase 1.5e (subagent streaming — that's Reverie-agent-specific).

---

## 1. Architecture

Two new types in `reverie_agent`: `ReverieAugmentedAgentServer` (wraps `Rc<dyn AgentServer>`) and `ReverieAugmentedConnection` (wraps `Rc<dyn AgentConnection>`). Both delegate every method to their inner counterpart except `connect()` (which wraps the returned connection) and `prompt()` (which pre-pends retrieved memory and fires a user-intent save after the inner resolves successfully).

`Agent::server()` in `crates/agent_ui/src/agent_ui.rs` is changed to build the inner server as today, then wrap it via `reverie_agent::augment_with_memory(inner, project, cx)` unless the agent opts out via `env.REVERIE_AUGMENT == "0"`. The Reverie agent's own variant short-circuits before the wrap — it already has memory in-process.

```
User picks Claude in the agent panel
  ↓
Agent::NativeAgent or Agent::Custom{id} → Rc<dyn AgentServer>  (inner)
  ↓
reverie_agent::augment_with_memory(inner, project, cx):
  Build ReverieHttpClient from project worktrees + env.
  Return Rc<ReverieAugmentedAgentServer { inner, http_client }>.
  ↓
On connect(): inner.connect().await → Rc<dyn AgentConnection>
  Wrap in Rc<ReverieAugmentedConnection { inner, http_client }>.
  ↓
On prompt(id, params, cx):
  1. user_text = extract text blocks from params.prompt.
  2. memory = http_client.smart_context(user_text).await.ok().flatten();
  3. If memory: params.prompt.insert(0, Text("Relevant memory:\n<ctx>\n")).
  4. response = inner.prompt(id, params, cx).await?;
  5. If response.stop_reason == EndTurn:
         http_client.save_passive(session_id, user_text, "zed-augment-user-intent").await;
  6. Return response.
```

Retrieval failures degrade silently to pass-through (no memory, no UI noise). Save is fire-and-forget.

---

## 2. Components & Interfaces

### 2.1 New file: `crates/reverie_agent/src/augment.rs` (~170 lines)

```rust
use crate::ReverieHttpClient;
use acp_thread::{AgentConnection, UserMessageId};
use agent_client_protocol as acp;
use agent_servers::{AgentServer, AgentServerDelegate};
use anyhow::{Result, anyhow};
use gpui::{App, Entity, Task};
use project::{AgentId, Project};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;

pub struct ReverieAugmentedAgentServer {
    inner: Rc<dyn AgentServer>,
    http_client: Arc<ReverieHttpClient>,
}

impl ReverieAugmentedAgentServer {
    pub fn new(inner: Rc<dyn AgentServer>, http_client: Arc<ReverieHttpClient>) -> Self {
        Self { inner, http_client }
    }
}

impl AgentServer for ReverieAugmentedAgentServer {
    fn agent_id(&self) -> AgentId {
        self.inner.agent_id()
    }
    fn logo(&self) -> ui::IconName {
        self.inner.logo()
    }
    fn connect(
        &self,
        delegate: AgentServerDelegate,
        project: Entity<Project>,
        cx: &mut App,
    ) -> Task<Result<Rc<dyn AgentConnection>>> {
        let http = self.http_client.clone();
        let inner_task = self.inner.connect(delegate, project, cx);
        cx.spawn(async move |_cx| {
            let inner_conn = inner_task.await?;
            Ok(Rc::new(ReverieAugmentedConnection::new(inner_conn, http))
                as Rc<dyn AgentConnection>)
        })
    }
    // Delegate every other optional method on AgentServer to self.inner.
    // Full list enumerated at impl time — each is a one-liner like
    //     fn default_mode(&self, cx: &App) -> Option<acp::SessionModeId> {
    //         self.inner.default_mode(cx)
    //     }
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}

struct ReverieAugmentedConnection {
    inner: Rc<dyn AgentConnection>,
    http_client: Arc<ReverieHttpClient>,
}

impl ReverieAugmentedConnection {
    fn new(inner: Rc<dyn AgentConnection>, http_client: Arc<ReverieHttpClient>) -> Self {
        Self { inner, http_client }
    }
}

impl AgentConnection for ReverieAugmentedConnection {
    fn agent_id(&self) -> AgentId { self.inner.agent_id() }
    fn telemetry_id(&self) -> ui::SharedString { self.inner.telemetry_id() }
    fn auth_methods(&self) -> &[acp::AuthMethod] { self.inner.auth_methods() }
    fn authenticate(&self, method: acp::AuthMethodId, cx: &mut App) -> Task<Result<()>> {
        self.inner.clone().authenticate(method, cx)
    }
    fn new_session(
        self: Rc<Self>,
        project: Entity<Project>,
        work_dirs: util::path_list::PathList,
        cx: &mut App,
    ) -> Task<Result<Entity<acp_thread::AcpThread>>> {
        self.inner.clone().new_session(project, work_dirs, cx)
    }
    fn cancel(&self, session_id: &acp::SessionId, cx: &mut App) {
        self.inner.cancel(session_id, cx);
    }
    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
    // Delegate every optional method (load_session, resume_session,
    // supports_*, truncate, retry, set_title, model_selector, telemetry,
    // session_modes, session_config_options, session_list,
    // terminal_auth_task, close_session, etc.) — each is a one-liner
    // that calls self.inner's version, possibly via self.inner.clone()
    // for `self: Rc<Self>` methods.

    fn prompt(
        &self,
        id: UserMessageId,
        mut params: acp::PromptRequest,
        cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        let http = self.http_client.clone();
        let inner = self.inner.clone();
        let user_text = user_text_from_prompt(&params.prompt);
        let session_id = params.session_id.clone();

        cx.spawn(async move |cx| {
            let memory = http.smart_context(&user_text).await.ok().flatten();
            params.prompt = augment_prompt_blocks(params.prompt, memory);

            let response = cx
                .update(|cx| inner.prompt(id, params, cx))?
                .await?;

            if matches!(response.stop_reason, acp::StopReason::EndTurn) {
                let session_id_str = session_id.0.as_ref().to_string();
                let _ = http
                    .save_passive(
                        &session_id_str,
                        &user_text,
                        "zed-augment-user-intent",
                    )
                    .await;
            }
            Ok(response)
        })
    }
}

pub(crate) fn augment_prompt_blocks(
    mut blocks: Vec<acp::ContentBlock>,
    memory: Option<crate::http::SmartContext>,
) -> Vec<acp::ContentBlock> {
    if let Some(ctx) = memory {
        if !ctx.content.trim().is_empty() {
            let memory_block = acp::ContentBlock::Text(acp::TextContent::new(
                format!("Relevant memory:\n{}\n", ctx.content),
            ));
            blocks.insert(0, memory_block);
        }
    }
    blocks
}

fn user_text_from_prompt(blocks: &[acp::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            acp::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

### 2.2 Public factory

```rust
// In augment.rs or reverie_agent.rs:
pub fn augment_with_memory(
    inner: Rc<dyn AgentServer>,
    project: &Entity<Project>,
    cx: &App,
) -> Option<Rc<dyn AgentServer>> {
    // Skip wrapping if inner is the Reverie agent itself — it already
    // does retrieval in-process. Defense in depth; the caller in
    // agent_ui.rs already short-circuits before calling this.
    if inner.agent_id() == *crate::REVERIE_AGENT_ID {
        return None;
    }
    let base_url = std::env::var("REVERIE_URL").ok();
    let project_name = resolve_project_name_for_augment(project, cx);
    let http_client_arc: Arc<dyn http_client::HttpClient> =
        project.read(cx).client().http_client();
    let http_client =
        ReverieHttpClient::new(base_url, project_name, http_client_arc);
    Some(Rc::new(ReverieAugmentedAgentServer::new(inner, http_client))
        as Rc<dyn AgentServer>)
}

fn resolve_project_name_for_augment(project: &Entity<Project>, cx: &App) -> String {
    if let Ok(from_env) = std::env::var("REVERIE_PROJECT") {
        return from_env;
    }
    project
        .read(cx)
        .visible_worktrees(cx)
        .next()
        .map(|wt| wt.read(cx).root_name().as_unix_str().to_string())
        .unwrap_or_else(|| "unknown-project".to_string())
}
```

### 2.3 `Agent::server()` change in `crates/agent_ui/src/agent_ui.rs`

Update the signature to take `project: &Entity<Project>` and `cx: &App`. Before:

```rust
pub fn server(
    &self,
    fs: Arc<dyn fs::Fs>,
    thread_store: Entity<agent::ThreadStore>,
) -> Rc<dyn agent_servers::AgentServer> {
    match self {
        Self::NativeAgent => Rc::new(agent::NativeAgentServer::new(fs, thread_store)),
        Self::ReverieAgent => Rc::new(reverie_agent::ReverieAgentServer::new()),
        Self::Custom { id: name } => {
            Rc::new(agent_servers::CustomAgentServer::new(name.clone()))
        }
        #[cfg(any(test, feature = "test-support"))]
        Self::Stub => Rc::new(crate::test_support::StubAgentServer::default_response()),
    }
}
```

After:

```rust
pub fn server(
    &self,
    fs: Arc<dyn fs::Fs>,
    thread_store: Entity<agent::ThreadStore>,
    project: &Entity<Project>,
    cx: &App,
) -> Rc<dyn agent_servers::AgentServer> {
    let inner: Rc<dyn agent_servers::AgentServer> = match self {
        Self::NativeAgent => Rc::new(agent::NativeAgentServer::new(fs, thread_store)),
        // Reverie short-circuits — it already has memory in-process.
        Self::ReverieAgent => return Rc::new(reverie_agent::ReverieAgentServer::new()),
        Self::Custom { id: name } => {
            Rc::new(agent_servers::CustomAgentServer::new(name.clone()))
        }
        #[cfg(any(test, feature = "test-support"))]
        Self::Stub => return Rc::new(crate::test_support::StubAgentServer::default_response()),
    };
    if augment_disabled_for(self, cx) {
        return inner;
    }
    reverie_agent::augment_with_memory(inner.clone(), project, cx).unwrap_or(inner)
}

fn augment_disabled_for(agent: &Agent, cx: &App) -> bool {
    let settings = cx.read_global(|settings: &settings::SettingsStore, _| {
        settings
            .get::<project::agent_server_store::AllAgentServersSettings>(None)
            .get(agent.id().as_ref())
            .cloned()
    });
    let envs = match settings {
        Some(s) => agent_env_vars(&s),
        None => return false,
    };
    envs.get("REVERIE_AUGMENT").map(|v| v == "0").unwrap_or(false)
}
```

`agent_env_vars` reads the `env` map out of whichever `CustomAgentServerSettings` variant the user configured. If the pattern from Phase 1-2 `CustomAgentServerSettings::{Custom, Extension, Registry}` applies, the env map is on each of those variants.

Every caller of `Agent::server(fs, thread_store)` is updated to pass `project` and `cx`. A grep at implementation time locates call sites.

### 2.4 Dependencies

None new. `reverie_agent` already depends on `http_client`, `project`, `agent_servers`, `acp_thread`, `agent_client_protocol`.

Module wiring in `crates/reverie_agent/src/reverie_agent.rs`:

```rust
mod augment;
mod backend;
mod connection;
mod http;
mod observer;
mod server;

#[cfg(test)]
mod tests;

pub use augment::augment_with_memory;
pub use http::ReverieHttpClient;
pub use server::{REVERIE_AGENT_ID, ReverieAgentServer};
```

---

## 3. Error Handling

### 3.1 Retrieval failure
`smart_context` returns `Ok(None)` on transport/5xx/parse/empty. `augment_prompt_blocks` handles `None` as pass-through. Inner agent sees the original `params.prompt` unchanged; user sees no UI noise.

### 3.2 Inner agent's `prompt()` error
The `response = inner.prompt(id, params, cx).await?` propagates. Save doesn't fire. Wrapper returns the error. User sees the inner's error handling.

### 3.3 Save failure
Fire-and-forget per 1.5a semantics. Never propagates.

### 3.4 http_client construction failure
`augment_with_memory` returns `None`. `agent_ui.rs` falls back to the un-wrapped inner. Log at info so the user sees "augment not installed."

### 3.5 Reverie self-wrap
Short-circuited twice: the enum match in `Agent::server()` returns early on `Self::ReverieAgent`, AND `augment_with_memory` checks `agent_id() == REVERIE_AGENT_ID` and returns `None`. Both gates catch it.

### 3.6 Env opt-out race
`agent_env_vars` reads the current SettingsStore snapshot at `connect()` time. If the user toggles `REVERIE_AUGMENT` mid-session, the setting takes effect on the NEXT new_session from Zed's agent panel. Acceptable — no one changes this mid-prompt.

### 3.7 UI breadcrumb absence
Unlike Reverie's agent, the wrapper does NOT emit a `[memory] consulted reverie` chunk. Reason: the wrapper doesn't have an `AcpThread` Entity handle at hand during the sync setup path of `prompt()` (getting one would require peeking at the inner connection's session map, which we don't own). Skipping is the YAGNI default. If users need visibility, the agent panel's logs (`log::info!` lines emitted by `ReverieHttpClient::note_first_fail` and elsewhere) are the escape hatch.

### 3.8 Prompt block ordering
`params.prompt.insert(0, memory_block)` prepends the memory to the user's message, ensuring it reaches the LLM as the first thing in the user turn. Existing blocks (mentions, selections, etc.) follow in their original order. No other mutations.

---

## 4. Testing

### 4.1 Pure-Rust unit tests (no GPUI)

In `crates/reverie_agent/src/tests.rs`:

1. **`augment_prompt_blocks_prepends_memory_when_present`** — input: `[Text("hello")]` + `Some(SmartContext { content: "- item 1" })`. Expected: `[Text("Relevant memory:\n- item 1\n"), Text("hello")]`.

2. **`augment_prompt_blocks_passes_through_on_no_memory`** — input: `[Text("hello")]` + `None`. Expected: `[Text("hello")]` (identical).

3. **`augment_prompt_blocks_skips_empty_memory`** — input: `[Text("hello")]` + `Some(SmartContext { content: "   \n" })`. Expected: `[Text("hello")]` (trim-empty memory is skipped).

### 4.2 Integration tests

Deferred. The wrapper-around-inner interaction through GPUI is the same testability constraint as Phase 1.5a/c/d — needs a fully-wired connection harness. Manual smoke is the fallback.

### 4.3 Manual smoke steps (for docs)

1. Start reveried on `:7437`. Seed a few `/observations` tagged with your project name.
2. Start Zed, pick **Zed Agent** (or Claude, Gemini) from the agent panel. NOT Reverie.
3. Send a prompt that should benefit from prior context.
4. Expected: the agent's response reflects knowledge of the seeded observations.
5. `curl -s localhost:7437/observations/recent | jq '.[] | select(.source == "zed-augment-user-intent")'` — the just-sent prompt text appears.
6. Toggle off with `settings.json`:
   ```json
   { "agent_servers": { "claude-acp": { "env": { "REVERIE_AUGMENT": "0" } } } }
   ```
   Restart or click "New Thread"; verify subsequent prompts don't include memory and don't appear in passive saves.

### 4.4 Not tested

- **Per-connection-type dispatch delegation** (each of the ~15 optional AgentServer/AgentConnection methods). These are one-line delegates; they compile-check correctly or they don't.
- **Augmentation interacting with Phase 1.5d mid-call cancel.** Cancel is handled by the inner agent; the wrapper only intercepts `prompt()`. If inner supports mid-call cancel, wrapper transparently respects it.

---

## 5. Known Limitations (Phase 1.5b)

- **No UI breadcrumb.** Augmented agents don't show a "[memory] consulted reverie" chunk.
- **User intent only.** No assistant-side capture — the LLM's response text is not saved to reverie. Only the user's prompt.
- **Per-agent opt-out; no global switch.** A user who wants to disable augmentation everywhere has to set `REVERIE_AUGMENT=0` on each configured agent. A future phase could add a workspace-global setting.
- **Retrieval fires on every prompt.** No heuristics to skip short/"yes"/"continue"-style follow-ups. Low cost if reveried is local; callable via `REVERIE_AUGMENT=0` if annoying.
- **No per-project scoping beyond the existing first-worktree resolution.** Multi-worktree projects collapse to the first worktree's name, same as Phase 1.5a.

## 6. Invoke-After

Per the brainstorming skill, after user approval of this spec the terminal state is invoking `writing-plans`.

---

## Self-Review (inline)

**Placeholder scan:** No TBDs. Every type, method, and setting path is spelled out with real code blocks. One explicit "enumerated at impl time" note in §2.1 for the full list of delegate methods on `AgentServer` / `AgentConnection` — these are boilerplate one-liners; implementation finds them by grep. Noted but not a blocker.

**Internal consistency:**
- `ReverieAugmentedAgentServer::new(inner, http_client)` signature consistent across §2.1 (def) and §2.2 (factory).
- `augment_with_memory(inner, project, cx) -> Option<Rc<dyn AgentServer>>` consistent across §2.2 (def) and §2.3 (caller).
- `augment_prompt_blocks(blocks, memory) -> Vec<ContentBlock>` consistent across §2.1 (def) and §4.1 (tests).
- `REVERIE_AUGMENT=0` as opt-out key consistent across §2.3 (check) and §5 (docs).

**Scope check:** one spec, one file (`augment.rs`) + one signature change (`Agent::server`). ~200 LOC implementation, 3 unit tests. Under a day.

**Ambiguity check:**
- "Wrapper skipped" vs "wrapper installed but empty" — resolved: on augment_disabled OR on http_client failure, return un-wrapped inner (§2.3 code + §3.4).
- "Save on non-EndTurn terminations" — resolved: §2.1 uses `matches!(..., EndTurn)`, §5 limitation doesn't claim more.
- "What if `agent_id()` changes mid-session" — doesn't happen; AgentId is stable for the life of the connection.
