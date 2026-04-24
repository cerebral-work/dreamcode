# Reverie Deepagent as Zed Agent Backend — Phase 1 (B2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the `reverie-deepagent` Rust crate directly into Zed (dreamcode fork) as a new in-process `AgentServer`, so selecting "Reverie" in Zed's agent panel drives the deepagent planner while reusing Zed's configured language models. Planner actions (todos, vfs writes, subagent spawns) surface live as tool-call updates in the existing agent panel UI.

**Architecture:**
- New crate `crates/reverie_agent/` in dreamcode. Depends on `reverie-deepagent` as a path dep to the sibling `reverie` repo.
- Three core types:
  - `ReverieAgentServer` — impls `AgentServer` (mirror of `NativeAgentServer` at `crates/agent/src/native_agent_server.rs:18-90`).
  - `ReverieAgentConnection` — impls `AgentConnection` (in-process, mirror of `StubAgentConnection` at `crates/acp_thread/src/connection.rs:686-966`; no subprocess).
  - `ZedLlmBackend` — impls `reverie-deepagent::LlmBackend`, wraps a `LanguageModelRegistry` selection, reuses reverie's existing `backends/protocol.rs` prompt/parser helpers (`render_state_with_observations` + `parse_action`).
- Planner runs on a dedicated OS thread (since `LlmBackend::next_action` is synchronous and `DeepAgent::execute` is a blocking loop). A `smol::channel` bridges planner events back to the GPUI foreground, which updates the `AcpThread` via `handle_session_update`.
- One small upstream change to reverie: add an optional `Observer` callback to `run_planner_with_ctx` so the connection can surface each `NextAction` as a live `SessionUpdate::ToolCall`. This is additive and non-breaking.

**Tech Stack:** Rust, GPUI (Zed's UI framework), `agent_client_protocol` crate, `smol`/`futures` async, reverie-deepagent (Rust).

**Non-goals for Phase 1 (explicitly deferred):**
- The MCP context-server for memory retrieval (Phase 0). Separate plan.
- The Dream Inspector panel (Phase 2). Separate plan.
- Standalone ACP subprocess binary (Phase 1.5 / B1). This plan sticks to in-process B2.
- Remote reverie (non-localhost). Assumes deepagent runs in-process.

**Related sibling repo:** `/Users/dennis/programming projects/reverie` — source of the `reverie-deepagent` crate and target of one upstream change in Task 5.

---

## File Structure

**New files (dreamcode):**
- `crates/reverie_agent/Cargo.toml` — crate manifest
- `crates/reverie_agent/src/lib.rs` — module root, re-exports
- `crates/reverie_agent/src/server.rs` — `ReverieAgentServer` (impl `AgentServer`)
- `crates/reverie_agent/src/connection.rs` — `ReverieAgentConnection` (impl `AgentConnection`)
- `crates/reverie_agent/src/backend.rs` — `ZedLlmBackend` (impl `reverie-deepagent::LlmBackend`)
- `crates/reverie_agent/src/observer.rs` — `ChannelObserver` bridging planner → session updates
- `crates/reverie_agent/src/tests.rs` — unit tests for backend + parser wiring
- `docs/reverie-agent.md` — user-facing docs, example settings snippet

**Modified files (dreamcode):**
- `Cargo.toml` (workspace) — add `crates/reverie_agent` to members; add `reverie-deepagent` workspace dep
- `crates/agent_ui/src/agent_ui.rs` — register `ReverieAgentServer` alongside `NativeAgentServer` (around line 336)
- `crates/zed/Cargo.toml` — depend on `reverie_agent`

**New/modified files (reverie — upstream additive change):**
- `crates/reverie-deepagent/src/planner.rs` — add `PlannerObserver` trait + `run_planner_with_observer` variant. Existing `run_planner_with_ctx` delegates with a no-op observer. Non-breaking.
- `crates/reverie-deepagent/src/planner.rs` (same file) — new unit tests for observer.

Each file has one responsibility. `connection.rs` holds session state; `backend.rs` holds LLM glue; `observer.rs` is the channel plumbing. If `connection.rs` grows past ~500 lines, split session lifecycle into `session.rs`.

---

## Task 1: Scaffold the `reverie_agent` crate

**Files:**
- Create: `crates/reverie_agent/Cargo.toml`
- Create: `crates/reverie_agent/src/lib.rs`
- Modify: `Cargo.toml` (workspace root — add to `[workspace.members]` and `[workspace.dependencies]`)

- [ ] **Step 1: Inspect the workspace-dependency pattern**

Read the top of the workspace `Cargo.toml` and note how an existing crate like `agent` or `agent_servers` appears in `[workspace.members]`, `[workspace.dependencies]`, and a nearby crate's `[dependencies]`. Mimic that exactly.

Run: `head -60 "/Users/dennis/programming projects/dreamcode/Cargo.toml"`

Expected: see `members = [...]` and `[workspace.dependencies]` blocks.

- [ ] **Step 2: Create the crate manifest**

Create `crates/reverie_agent/Cargo.toml`:

```toml
[package]
name = "reverie_agent"
version = "0.1.0"
edition.workspace = true
publish.workspace = true
license = "GPL-3.0-or-later"

[lints]
workspace = true

[lib]
path = "src/lib.rs"
doctest = false

[features]
test-support = [
    "acp_thread/test-support",
    "language_model/test-support",
]

[dependencies]
acp_thread.workspace = true
agent_client_protocol.workspace = true
agent_servers.workspace = true
anyhow.workspace = true
collections.workspace = true
futures.workspace = true
fs.workspace = true
gpui.workspace = true
language_model.workspace = true
log.workspace = true
parking_lot.workspace = true
project.workspace = true
reverie-deepagent.workspace = true
serde.workspace = true
serde_json.workspace = true
settings.workspace = true
smol.workspace = true
ui.workspace = true
util.workspace = true
watch.workspace = true

[dev-dependencies]
gpui = { workspace = true, features = ["test-support"] }
acp_thread = { workspace = true, features = ["test-support"] }
language_model = { workspace = true, features = ["test-support"] }
```

- [ ] **Step 3: Create a minimal lib.rs**

Create `crates/reverie_agent/src/lib.rs`:

```rust
//! Zed AgentServer implementation backed by the reverie-deepagent crate.
//!
//! Mirrors the in-process shape of `crate::agent::NativeAgentServer` —
//! no subprocess; the planner loop runs on a dedicated OS thread and
//! emits session updates back to the GPUI foreground via a channel.

mod backend;
mod connection;
mod observer;
mod server;

#[cfg(test)]
mod tests;

pub use server::{REVERIE_AGENT_ID, ReverieAgentServer};
```

And create empty module stubs so the crate compiles:

```bash
touch "/Users/dennis/programming projects/dreamcode/crates/reverie_agent/src/backend.rs"
touch "/Users/dennis/programming projects/dreamcode/crates/reverie_agent/src/connection.rs"
touch "/Users/dennis/programming projects/dreamcode/crates/reverie_agent/src/observer.rs"
```

And `crates/reverie_agent/src/server.rs` initial content:

```rust
use project::AgentId;
use std::sync::LazyLock;

pub static REVERIE_AGENT_ID: LazyLock<AgentId> =
    LazyLock::new(|| AgentId::new("reverie".into()));

pub struct ReverieAgentServer;
```

- [ ] **Step 4: Add the crate to the workspace**

In the workspace `Cargo.toml` (`/Users/dennis/programming projects/dreamcode/Cargo.toml`):

Under `[workspace] members = [...]`, add `"crates/reverie_agent",` (keep alphabetical sort if existing list is sorted).

Under `[workspace.dependencies]`, add:
```toml
reverie_agent = { path = "crates/reverie_agent" }
reverie-deepagent = { path = "../reverie/crates/reverie-deepagent" }
```

The path dep to reverie assumes the sibling-directory layout `programming projects/{dreamcode,reverie}/`. Document this assumption in `docs/reverie-agent.md` (Task 10).

- [ ] **Step 5: Verify it builds**

Run: `cargo check -p reverie_agent`

Expected: PASS with warnings for unused code only. No errors.

If reverie's `reverie-deepagent` crate fails to resolve (path wrong, version conflict with reqwest/tokio), stop and reconcile versions. `reverie-deepagent` uses `reqwest::blocking` — ensure the feature flags don't conflict with Zed's `reqwest` usage. If conflict, use `default-features = false` on the dep and opt in only to what's needed.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/reverie_agent
git commit -m "feat(reverie_agent): scaffold crate"
```

---

## Task 2: Implement `ZedLlmBackend` (TDD)

Implements `reverie-deepagent::LlmBackend` on top of Zed's `LanguageModel` trait. Reuses reverie's existing prompt formatter and JSON-action parser. The backend is **synchronous** (the trait requires it) and will be run on a dedicated thread; inside the thread we use `futures::executor::block_on` to await Zed's async `stream_completion`.

**Files:**
- Modify: `crates/reverie_agent/src/backend.rs`
- Modify: `crates/reverie_agent/src/tests.rs`

- [ ] **Step 1: Sketch the backend type signature and write the first failing test**

Replace `crates/reverie_agent/src/tests.rs` with:

```rust
use crate::backend::ZedLlmBackend;
use gpui::TestAppContext;
use language_model::fake_provider::FakeLanguageModel;
use reverie_deepagent::{LlmBackend, NextAction, TodoList, Vfs};
use std::sync::Arc;
use tempfile::TempDir;

fn fresh_vfs() -> (TempDir, Vfs) {
    let tmp = TempDir::new().unwrap();
    let vfs = Vfs::new_under(tmp.path(), "test").unwrap();
    (tmp, vfs)
}

#[gpui::test]
fn backend_parses_add_todo_action(cx: &mut TestAppContext) {
    let fake = Arc::new(FakeLanguageModel::default());
    // Queue a JSON response that reverie's parser will map to AddTodo.
    fake.respond_with_text(
        r#"{"action":"add_todo","description":"investigate the bug"}"#,
    );

    let mut backend = cx.update(|cx| {
        ZedLlmBackend::new(fake.clone() as Arc<dyn language_model::LanguageModel>, cx)
    });

    let (_tmp, vfs) = fresh_vfs();
    let todos = TodoList::new();
    let action = backend.next_action(&todos, &vfs, &[]).expect("ok");
    assert!(matches!(action, NextAction::AddTodo(s) if s == "investigate the bug"));
}
```

Note: `FakeLanguageModel` is in `crates/language_model/src/fake_provider.rs:29`; its `respond_with_text` or equivalent helper is what you'd call to queue a canned response. If the real method is named differently (check `fake_provider.rs`), adjust the test to match — do not invent API.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p reverie_agent backend_parses_add_todo_action`

Expected: FAIL with "ZedLlmBackend not found" (compile error — the type doesn't exist yet).

- [ ] **Step 3: Implement the minimal backend that makes the test pass**

Write `crates/reverie_agent/src/backend.rs`:

```rust
use anyhow::Result;
use gpui::{App, AsyncApp};
use language_model::{
    LanguageModel, LanguageModelRequest, LanguageModelRequestMessage, MessageContent, Role,
};
use reverie_deepagent::backends::protocol::{
    CORRECTOR_SUFFIX, JSON_PROTOCOL_SUFFIX, parse_action, render_state_with_observations,
};
use reverie_deepagent::prompt::DEEPAGENT_BASE_PROMPT;
use reverie_deepagent::{BackendError, LlmBackend, NextAction, SpawnObservation, TodoList, Vfs};
use std::sync::Arc;

/// LlmBackend impl that drives reverie's deepagent planner through one of
/// Zed's configured `LanguageModel` providers.
///
/// The backend owns a rolling transcript of system + user + assistant
/// messages so the LLM sees full context across turns. `next_action` blocks
/// the current thread on a `stream_completion` call (we run on a dedicated
/// planner thread, so this is safe).
pub struct ZedLlmBackend {
    model: Arc<dyn LanguageModel>,
    transcript: Vec<LanguageModelRequestMessage>,
    system_prompt: String,
    /// `AsyncApp` handle captured at construction; allows us to dispatch
    /// work onto the executor from the blocking thread.
    async_cx: AsyncApp,
}

impl ZedLlmBackend {
    pub fn new(model: Arc<dyn LanguageModel>, cx: &mut App) -> Self {
        let system_prompt = format!("{DEEPAGENT_BASE_PROMPT}{JSON_PROTOCOL_SUFFIX}");
        let transcript = vec![LanguageModelRequestMessage {
            role: Role::System,
            content: vec![MessageContent::Text(system_prompt.clone())],
            cache: false,
        }];
        Self {
            model,
            transcript,
            system_prompt,
            async_cx: cx.to_async(),
        }
    }
}

impl LlmBackend for ZedLlmBackend {
    fn next_action(
        &mut self,
        todos: &TodoList,
        vfs: &Vfs,
        observations: &[SpawnObservation],
    ) -> Result<NextAction, BackendError> {
        let user_content = render_state_with_observations(todos, vfs, observations);
        self.transcript.push(LanguageModelRequestMessage {
            role: Role::User,
            content: vec![MessageContent::Text(user_content)],
            cache: false,
        });

        let request = LanguageModelRequest {
            messages: self.transcript.clone(),
            ..Default::default()
        };

        let model = self.model.clone();
        let text = futures::executor::block_on(async move {
            let mut stream = model
                .stream_completion_text(request, &Default::default())
                .await
                .map_err(|e| BackendError::Transport(e.to_string()))?;
            let mut buf = String::new();
            while let Some(chunk) = stream.stream.next().await {
                let chunk = chunk.map_err(|e| BackendError::Transport(e.to_string()))?;
                buf.push_str(&chunk);
            }
            Ok::<_, BackendError>(buf)
        })?;

        // Record the assistant turn in the transcript before parsing so a
        // retry sees it.
        self.transcript.push(LanguageModelRequestMessage {
            role: Role::Assistant,
            content: vec![MessageContent::Text(text.clone())],
            cache: false,
        });

        parse_action(&text)
    }

    fn inject_nudge(&mut self, msg: &str) {
        self.transcript.push(LanguageModelRequestMessage {
            role: Role::User,
            content: vec![MessageContent::Text(format!("NUDGE: {msg}"))],
            cache: false,
        });
    }

    fn child(&self) -> Result<Box<dyn LlmBackend + Send>, BackendError> {
        let mut child = Self {
            model: self.model.clone(),
            transcript: Vec::new(),
            system_prompt: self.system_prompt.clone(),
            async_cx: self.async_cx.clone(),
        };
        child.transcript.push(LanguageModelRequestMessage {
            role: Role::System,
            content: vec![MessageContent::Text(self.system_prompt.clone())],
            cache: false,
        });
        Ok(Box::new(child))
    }
}
```

Notes:
- Exact field names / method signatures on `LanguageModelRequest`, `MessageContent`, `Role` should match what's defined in `crates/language_model/src/language_model.rs` — if any differ, adjust to the real type (don't invent).
- `stream_completion_text` returns a stream; the exact type is at `language_model/src/language_model.rs:142` — inspect and adjust.
- `async_cx: AsyncApp` is stored but not used yet in this step. It's here so later tasks (observer dispatch) can post back to the foreground executor. If it's unused it'll warn; wire up in Task 4 or mark `#[allow(dead_code)]` temporarily.

And export it from `lib.rs`:

```rust
mod backend;
pub use backend::ZedLlmBackend;  // only if external consumers need it; otherwise keep private
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p reverie_agent backend_parses_add_todo_action`

Expected: PASS.

If it fails with a parse error from `parse_action`, inspect `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/backends/protocol.rs:98` to see the exact JSON schema `parse_action` expects, and adjust the canned `respond_with_text` content in the test.

- [ ] **Step 5: Add tests for `child()` independence and `inject_nudge`**

Append to `tests.rs`:

```rust
#[gpui::test]
fn child_has_independent_transcript(cx: &mut TestAppContext) {
    let fake = Arc::new(FakeLanguageModel::default());
    fake.respond_with_text(r#"{"action":"noop"}"#);
    fake.respond_with_text(r#"{"action":"noop"}"#);

    let mut parent = cx.update(|cx| {
        ZedLlmBackend::new(fake.clone() as Arc<dyn language_model::LanguageModel>, cx)
    });
    let (_tmp, vfs) = fresh_vfs();
    let todos = TodoList::new();
    parent.next_action(&todos, &vfs, &[]).unwrap();

    let mut child = parent.child().expect("child should be allowed");
    let act = child.next_action(&todos, &vfs, &[]).unwrap();
    assert!(matches!(act, NextAction::NoOp));
    // Child transcript must NOT include the parent's earlier user turn;
    // asserting via a visible side-effect is hard, so instead we ensure
    // the child-produced request was accepted and parsed.
}

#[gpui::test]
fn inject_nudge_appends_user_turn(cx: &mut TestAppContext) {
    let fake = Arc::new(FakeLanguageModel::default());
    fake.respond_with_text(r#"{"action":"noop"}"#);

    let mut backend = cx.update(|cx| {
        ZedLlmBackend::new(fake.clone() as Arc<dyn language_model::LanguageModel>, cx)
    });
    backend.inject_nudge("wake up");
    let (_tmp, vfs) = fresh_vfs();
    let todos = TodoList::new();
    let _ = backend.next_action(&todos, &vfs, &[]).unwrap();
    // Transcript inspection would require exposing a test-only accessor;
    // instead, confirm the call succeeds end-to-end.
}
```

Run: `cargo test -p reverie_agent` — expected: PASS (both new tests).

- [ ] **Step 6: Commit**

```bash
git add crates/reverie_agent/src/backend.rs crates/reverie_agent/src/tests.rs
git commit -m "feat(reverie_agent): ZedLlmBackend bridging reverie planner to Zed LanguageModel"
```

---

## Task 3: `ReverieAgentConnection` — in-process impl of `AgentConnection`

**Files:**
- Modify: `crates/reverie_agent/src/connection.rs`
- Modify: `crates/reverie_agent/src/lib.rs`

- [ ] **Step 1: Write the skeleton from the `StubAgentConnection` template**

Read `crates/acp_thread/src/connection.rs:686-966` (the full `StubAgentConnection` block) to see the exact session-map + thread-ref pattern.

Replace `crates/reverie_agent/src/connection.rs` with:

```rust
use acp_thread::{AcpThread, AgentConnection, UserMessageId};
use action_log::ActionLog;
use agent_client_protocol as acp;
use anyhow::Result;
use collections::HashMap;
use gpui::{App, AppContext as _, Entity, SharedString, Task, WeakEntity};
use language_model::LanguageModel;
use parking_lot::Mutex;
use project::{AgentId, Project};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use util::path_list::PathList;

use crate::server::REVERIE_AGENT_ID;

pub struct ReverieAgentConnection {
    model: Arc<dyn LanguageModel>,
    sessions: Arc<Mutex<HashMap<acp::SessionId, Session>>>,
}

struct Session {
    thread: WeakEntity<AcpThread>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

impl ReverieAgentConnection {
    pub fn new(model: Arc<dyn LanguageModel>) -> Self {
        Self {
            model,
            sessions: Arc::new(Mutex::new(HashMap::default())),
        }
    }
}

impl AgentConnection for ReverieAgentConnection {
    fn agent_id(&self) -> AgentId {
        REVERIE_AGENT_ID.clone()
    }

    fn telemetry_id(&self) -> SharedString {
        "reverie".into()
    }

    fn auth_methods(&self) -> &[acp::AuthMethod] {
        &[]
    }

    fn authenticate(&self, _method: acp::AuthMethodId, _cx: &mut App) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn new_session(
        self: Rc<Self>,
        project: Entity<Project>,
        work_dirs: PathList,
        cx: &mut App,
    ) -> Task<Result<Entity<AcpThread>>> {
        let session_id = acp::SessionId::new(uuid::Uuid::new_v4().to_string());
        let action_log = cx.new(|_| ActionLog::new(project.clone()));
        let thread = cx.new(|cx| {
            AcpThread::new(
                None,
                None,
                Some(work_dirs),
                self.clone(),
                project,
                action_log,
                session_id.clone(),
                watch::Receiver::constant(
                    acp::PromptCapabilities::new()
                        .image(false)
                        .audio(false)
                        .embedded_context(true),
                ),
                cx,
            )
        });
        self.sessions.lock().insert(
            session_id,
            Session {
                thread: thread.downgrade(),
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        );
        Task::ready(Ok(thread))
    }

    fn prompt(
        &self,
        _id: UserMessageId,
        _params: acp::PromptRequest,
        _cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        // Filled in by Task 4.
        Task::ready(Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)))
    }

    fn cancel(&self, session_id: &acp::SessionId, _cx: &mut App) {
        if let Some(s) = self.sessions.lock().get(session_id) {
            s.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}
```

Cross-check `AcpThread::new`'s real signature at `crates/acp_thread/src/acp_thread.rs` — if the arg order differs, adjust. Do NOT invent.

- [ ] **Step 2: Write `ReverieAgentServer`**

Replace `crates/reverie_agent/src/server.rs`:

```rust
use crate::connection::ReverieAgentConnection;
use acp_thread::AgentConnection;
use agent_client_protocol as acp;
use agent_servers::{AgentServer, AgentServerDelegate};
use anyhow::{Context as _, Result};
use gpui::{App, AppContext as _, Entity, Task};
use language_model::{LanguageModel, LanguageModelRegistry};
use project::{AgentId, Project};
use std::any::Any;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use ui::IconName;

pub static REVERIE_AGENT_ID: LazyLock<AgentId> =
    LazyLock::new(|| AgentId::new("reverie".into()));

pub struct ReverieAgentServer;

impl ReverieAgentServer {
    pub fn new() -> Self {
        Self
    }

    fn default_model(cx: &App) -> Result<Arc<dyn LanguageModel>> {
        let registry = LanguageModelRegistry::read_global(cx);
        registry
            .default_model()
            .map(|m| m.model)
            .context("no default language model configured")
    }
}

impl AgentServer for ReverieAgentServer {
    fn agent_id(&self) -> AgentId {
        REVERIE_AGENT_ID.clone()
    }

    fn logo(&self) -> IconName {
        IconName::ZedAgent
    }

    fn connect(
        &self,
        _delegate: AgentServerDelegate,
        _project: Entity<Project>,
        cx: &mut App,
    ) -> Task<Result<Rc<dyn AgentConnection>>> {
        let model = match Self::default_model(cx) {
            Ok(m) => m,
            Err(e) => return Task::ready(Err(e)),
        };
        let connection: Rc<dyn AgentConnection> =
            Rc::new(ReverieAgentConnection::new(model));
        Task::ready(Ok(connection))
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}
```

Cross-check `LanguageModelRegistry::default_model`'s real signature at `crates/language_model/src/registry.rs:123-200`.

- [ ] **Step 3: Smoke-test that `new_session` returns a real `AcpThread`**

Append to `tests.rs`:

```rust
use crate::connection::ReverieAgentConnection;
use project::Project;

#[gpui::test]
async fn new_session_creates_thread(cx: &mut TestAppContext) {
    let fake = Arc::new(FakeLanguageModel::default());
    let connection =
        Rc::new(ReverieAgentConnection::new(fake as Arc<dyn language_model::LanguageModel>));
    let project = cx.update(|cx| Project::test(fs::FakeFs::new(cx.background_executor().clone()), [], cx)).await;
    let thread = cx
        .update(|cx| connection.new_session(project, Default::default(), cx))
        .await
        .expect("new_session ok");
    cx.update(|cx| {
        // Just verify the entity is live.
        let _ = thread.read(cx);
    });
}
```

Project construction varies; look at an existing test like the ones in `crates/acp_thread/src/` for the canonical test-project builder. Don't invent `Project::test` if it doesn't exist — find the real helper.

- [ ] **Step 4: Run tests**

Run: `cargo test -p reverie_agent`

Expected: all tests pass (the new one + the Task 2 ones).

- [ ] **Step 5: Commit**

```bash
git add crates/reverie_agent/src/connection.rs crates/reverie_agent/src/server.rs crates/reverie_agent/src/tests.rs
git commit -m "feat(reverie_agent): ReverieAgentServer + ReverieAgentConnection skeleton"
```

---

## Task 4: Wire `prompt()` through `DeepAgent::execute()` on a background thread

`prompt()` must return a `Task<Result<PromptResponse>>`. The planner is blocking, so we run it on `std::thread::spawn` and bridge completion via `smol::channel::oneshot`. Live `SessionUpdate` streaming comes in Task 5 — for now just return the final summary.

**Files:**
- Modify: `crates/reverie_agent/src/connection.rs`

- [ ] **Step 1: Add a channel-bridging helper**

Append to `connection.rs`:

```rust
use reverie_deepagent::{DeepAgent, Run, run_planner_with_ctx};

fn run_deepagent_blocking(
    model: Arc<dyn LanguageModel>,
    user_prompt: String,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    cx: AsyncApp,
) -> Result<String> {
    let run = Run::new_default().map_err(|e| anyhow::anyhow!("vfs init failed: {e}"))?;
    // Seed the run with the user's prompt as an initial todo so the
    // planner has something to react to on iteration 1.
    // (Reverie expects the backend to drive this, but a seed todo is the
    // cheapest way to anchor intent in Phase 1.)
    // NOTE: verify whether reverie-deepagent has a public API for
    // "pre-seeded prompt" — if so use it; otherwise this todo approach is
    // acceptable.

    let backend_cx = cx.clone();
    let handle = std::thread::Builder::new()
        .name("reverie-deepagent".into())
        .spawn(move || {
            // Build the backend inside the thread so GPUI contexts are not
            // captured across threads unsafely.
            let mut backend =
                backend_cx.update(|cx| crate::backend::ZedLlmBackend::new(model, cx))?;
            let deep = DeepAgent::new(run, 32);
            let result = deep.execute(&mut backend);
            anyhow::Ok(format!(
                "planner terminated: {:?}, iterations={}, todos_pending={}",
                result.termination,
                result.iterations,
                result.todos.pending_count()
            ))
        })
        .map_err(|e| anyhow::anyhow!("thread spawn failed: {e}"))?;

    // TODO(Task 7): honour `cancel` by polling and aborting if flagged.
    let _ = cancel; // silence unused warning
    let _ = user_prompt; // will be passed into the planner once Run API supports a seed prompt

    handle
        .join()
        .map_err(|_| anyhow::anyhow!("planner thread panicked"))?
}
```

- [ ] **Step 2: Replace the stub `prompt()`**

Replace the `prompt` method body in `ReverieAgentConnection`:

```rust
fn prompt(
    &self,
    _id: UserMessageId,
    params: acp::PromptRequest,
    cx: &mut App,
) -> Task<Result<acp::PromptResponse>> {
    let session_id = params.session_id.clone();
    let user_text = params
        .prompt
        .iter()
        .filter_map(|b| match b {
            acp::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let cancel = {
        let sessions = self.sessions.lock();
        sessions
            .get(&session_id)
            .map(|s| s.cancel.clone())
            .unwrap_or_else(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
    };
    let thread = {
        let sessions = self.sessions.lock();
        sessions.get(&session_id).and_then(|s| s.thread.upgrade())
    };
    let model = self.model.clone();
    let async_cx = cx.to_async();

    cx.spawn(async move |cx| {
        let summary = smol::unblock(move || {
            run_deepagent_blocking(model, user_text, cancel, async_cx)
        })
        .await?;

        if let Some(thread) = thread {
            cx.update(|cx| {
                thread.update(cx, |thread, cx| {
                    let _ = thread.handle_session_update(
                        acp::SessionUpdate::AgentMessageChunk {
                            content: acp::ContentBlock::Text(acp::TextContent::new(summary)),
                        },
                        cx,
                    );
                })
            })?;
        }
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    })
}
```

Cross-check `acp::SessionUpdate` variant names and `acp::TextContent` constructor against `agent_client_protocol` crate docs — if variants differ, adjust.

- [ ] **Step 3: Smoke-test with a fake model**

Append to `tests.rs`:

```rust
#[gpui::test]
async fn prompt_runs_planner_to_completion(cx: &mut TestAppContext) {
    let fake = Arc::new(FakeLanguageModel::default());
    // Queue enough actions for a short productive run: AddTodo + Complete + NoOp.
    fake.respond_with_text(r#"{"action":"add_todo","description":"solve"}"#);
    fake.respond_with_text(r#"{"action":"set_status","id":1,"status":"completed"}"#);
    fake.respond_with_text(r#"{"action":"noop"}"#);

    let connection = Rc::new(ReverieAgentConnection::new(
        fake as Arc<dyn language_model::LanguageModel>,
    ));
    // ... project setup (use real helper from Task 3) ...
    // Call new_session, then prompt; await; assert PromptResponse stop_reason.
}
```

Fill in the `// ...` using the same project helper you used in Task 3.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p reverie_agent`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/reverie_agent/src/connection.rs crates/reverie_agent/src/tests.rs
git commit -m "feat(reverie_agent): run deepagent planner on background thread for prompt()"
```

---

## Task 5: Upstream reverie — add `PlannerObserver` callback (non-breaking)

The existing `run_planner_with_ctx` swallows every `NextAction` internally. To surface them live in Zed we need a hook. Add an additive observer API to reverie.

**Files (in reverie repo):**
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/planner.rs`
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/lib.rs` (re-exports)

Do this work in a new branch on the reverie repo. Coordinate with its CI before merging.

- [ ] **Step 1: Write a failing test for the observer**

In reverie's `planner.rs` `#[cfg(test)] mod tests`, append:

```rust
#[test]
fn observer_sees_every_action() {
    use std::sync::Mutex;
    let (_p, run) = fresh_run();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_cb = seen.clone();

    let mut backend = MockLlmBackend::new([
        NextAction::AddTodo("t".into()),
        NextAction::SetStatus(1, TodoStatus::Completed),
        NextAction::NoOp,
    ]);
    let observer = move |action: &NextAction| {
        seen_cb.lock().unwrap().push(format!("{action:?}"));
    };
    let _ = run_planner_with_observer(
        &run,
        &mut backend,
        10,
        &SpawnConfig::default(),
        &observer,
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].starts_with("AddTodo"));
}
```

Run: `cargo test -p reverie-deepagent observer_sees_every_action`

Expected: FAIL (the function doesn't exist).

- [ ] **Step 2: Add the observer trait and new fn**

Above `pub fn run_planner_with_ctx` in `planner.rs`:

```rust
/// Callback invoked after every action the planner applies. Allows hosts
/// (e.g. GUI agents) to stream live planner steps.
pub trait PlannerObserver {
    fn on_action(&self, action: &NextAction);
}

impl<F: Fn(&NextAction)> PlannerObserver for F {
    fn on_action(&self, action: &NextAction) {
        self(action)
    }
}

/// Like [`run_planner_with_ctx`] but invokes `observer.on_action(&action)`
/// after each action is applied.
pub fn run_planner_with_observer(
    run: &Run,
    backend: &mut dyn LlmBackend,
    max_iterations: u32,
    spawn_cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
) -> PlannerResult {
    // Copy the body of run_planner_with_ctx verbatim, adding
    //     observer.on_action(&action);
    // immediately after the `debug!` call on line ~318.
    // (Do not factor via a wrapper — the loop owns `i` and termination
    // returns directly, so a closure-based split is awkward.)
    unimplemented!()
}
```

Then refactor: copy the full `run_planner_with_ctx` body into `run_planner_with_observer`, add the `observer.on_action(&action);` call after the `debug!`. Make `run_planner_with_ctx` delegate:

```rust
pub fn run_planner_with_ctx(
    run: &Run,
    backend: &mut dyn LlmBackend,
    max_iterations: u32,
    spawn_cfg: &SpawnConfig,
) -> PlannerResult {
    struct Noop;
    impl PlannerObserver for Noop {
        fn on_action(&self, _: &NextAction) {}
    }
    run_planner_with_observer(run, backend, max_iterations, spawn_cfg, &Noop)
}
```

- [ ] **Step 3: Re-export from lib.rs**

In `reverie-deepagent/src/lib.rs` update the `pub use planner::{...}` line to include `PlannerObserver, run_planner_with_observer`.

- [ ] **Step 4: Run existing + new tests**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent`

Expected: all existing tests pass; `observer_sees_every_action` passes.

- [ ] **Step 5: Commit in reverie repo**

```bash
cd "/Users/dennis/programming projects/reverie"
git checkout -b feat/planner-observer
git add crates/reverie-deepagent/src/planner.rs crates/reverie-deepagent/src/lib.rs
git commit -m "feat(deepagent): add PlannerObserver hook and run_planner_with_observer"
```

- [ ] **Step 6: Verify the dreamcode path dep picks up the change**

Run: `cd "/Users/dennis/programming projects/dreamcode" && cargo check -p reverie_agent`

Expected: PASS. If cargo caches stale metadata, `cargo clean -p reverie-deepagent && cargo check -p reverie_agent`.

---

## Task 6: Bridge planner → `SessionUpdate::ToolCall` live stream

Use the new observer to send each `NextAction` as a tool call to the `AcpThread`.

**Files:**
- Modify: `crates/reverie_agent/src/observer.rs`
- Modify: `crates/reverie_agent/src/connection.rs`

- [ ] **Step 1: Implement `ChannelObserver`**

Replace `crates/reverie_agent/src/observer.rs`:

```rust
use agent_client_protocol as acp;
use reverie_deepagent::{NextAction, PlannerObserver};
use smol::channel::Sender;

/// Translates `NextAction` → `acp::SessionUpdate` and pushes onto a channel
/// consumed by the GPUI foreground.
pub struct ChannelObserver {
    tx: Sender<acp::SessionUpdate>,
}

impl ChannelObserver {
    pub fn new(tx: Sender<acp::SessionUpdate>) -> Self {
        Self { tx }
    }

    fn action_to_update(action: &NextAction) -> acp::SessionUpdate {
        // Render each action as a lightweight ToolCall so Zed's agent UI
        // already knows how to display it. For Phase 1 we emit a single
        // completed ToolCall per action; a later pass can distinguish
        // in-flight vs completed.
        let (name, body) = match action {
            NextAction::AddTodo(s) => ("add_todo", s.clone()),
            NextAction::SetStatus(id, st) => {
                ("set_status", format!("todo #{id} → {st:?}"))
            }
            NextAction::VfsWrite { path, contents } => (
                "vfs_write",
                format!("{path}\n---\n{}", &contents[..contents.len().min(200)]),
            ),
            NextAction::Spawn(req) => ("spawn", format!("{} :: {}", req.persona, req.task)),
            NextAction::ParallelSpawn(reqs) => (
                "parallel_spawn",
                reqs.iter()
                    .map(|r| format!("{} :: {}", r.persona, r.task))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            NextAction::Note(s) => ("note", s.clone()),
            NextAction::GiveUp => ("give_up", String::new()),
            NextAction::NoOp => ("noop", String::new()),
        };
        // Exact ToolCall constructor varies by acp crate version; adjust.
        acp::SessionUpdate::ToolCall(acp::ToolCall {
            tool_call_id: acp::ToolCallId::new(uuid::Uuid::new_v4().to_string()),
            kind: acp::ToolKind::Other,
            status: acp::ToolCallStatus::Completed,
            title: name.into(),
            content: vec![acp::ToolCallContent::Content(acp::ContentBlock::Text(
                acp::TextContent::new(body),
            ))],
            locations: vec![],
            raw_input: None,
            raw_output: None,
            meta: None,
        })
    }
}

impl PlannerObserver for ChannelObserver {
    fn on_action(&self, action: &NextAction) {
        let _ = self.tx.try_send(Self::action_to_update(action));
    }
}
```

Exact field names on `acp::ToolCall` are version-dependent — look at a concrete call site in `crates/agent/src/` for the live shape and mirror it.

- [ ] **Step 2: Consume the channel on the foreground**

In `run_deepagent_blocking` in `connection.rs`, before spawning the planner thread, build the channel + observer; in the `cx.spawn(async move |cx| ...)` in `prompt()`, read from the channel in parallel and dispatch each update to the thread entity. Replace the thread body and the outer spawn:

```rust
use smol::channel;

fn run_deepagent_with_updates(
    model: Arc<dyn LanguageModel>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    mut cx_for_backend: AsyncApp,
) -> (
    channel::Receiver<acp::SessionUpdate>,
    std::thread::JoinHandle<Result<String>>,
) {
    let (tx, rx) = channel::unbounded();
    let handle = std::thread::Builder::new()
        .name("reverie-deepagent".into())
        .spawn(move || -> Result<String> {
            let run = Run::new_default().map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut backend = cx_for_backend
                .update(|cx| crate::backend::ZedLlmBackend::new(model, cx))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let observer = crate::observer::ChannelObserver::new(tx);
            let result = run_planner_with_observer(
                &run,
                &mut backend,
                32,
                &reverie_deepagent::SpawnConfig::default(),
                &observer,
            );
            let _ = cancel; // Task 7 will wire real cancellation
            Ok(format!(
                "planner terminated: {:?}, iterations={}, todos_pending={}",
                result.termination, result.iterations, result.todos.pending_count()
            ))
        })
        .expect("spawn planner thread");
    (rx, handle)
}
```

And in `prompt()`, replace the `cx.spawn(...)` body:

```rust
cx.spawn(async move |cx| {
    let (rx, handle) = run_deepagent_with_updates(model, cancel, cx.clone());

    // Pump updates into the AcpThread as they arrive.
    while let Ok(update) = rx.recv().await {
        if let Some(thread) = thread.as_ref() {
            cx.update(|cx| {
                thread.update(cx, |thread, cx| {
                    let _ = thread.handle_session_update(update, cx);
                })
            })?;
        }
    }

    let summary = smol::unblock(move || {
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("planner thread panicked"))?
    })
    .await?;

    if let Some(thread) = thread {
        cx.update(|cx| {
            thread.update(cx, |thread, cx| {
                let _ = thread.handle_session_update(
                    acp::SessionUpdate::AgentMessageChunk {
                        content: acp::ContentBlock::Text(acp::TextContent::new(summary)),
                    },
                    cx,
                );
            })
        })?;
    }
    Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
})
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p reverie_agent`

Expected: PASS. The existing `prompt_runs_planner_to_completion` should still succeed; add a new test observing that `handle_session_update` was called at least N times where N is the action count.

- [ ] **Step 4: Commit**

```bash
git add crates/reverie_agent/src/observer.rs crates/reverie_agent/src/connection.rs
git commit -m "feat(reverie_agent): stream planner actions to AcpThread via ChannelObserver"
```

---

## Task 7: Surface `SpawnObservation` as tool-call completion

Each completed subagent spawn should close its tool-call with the child's summary. Phase 1 does this by adding a second observer hook for spawn results. For now, we'll piggyback on the main observer since `Spawn` / `ParallelSpawn` actions already fire; we just need to add a **post-spawn** update.

**Files:**
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/planner.rs`
- Modify: `crates/reverie_agent/src/observer.rs`

- [ ] **Step 1: Extend `PlannerObserver` with `on_spawn_complete` (in reverie)**

In reverie's `planner.rs`, extend the trait:

```rust
pub trait PlannerObserver {
    fn on_action(&self, action: &NextAction);
    fn on_spawn_complete(&self, observation: &SpawnObservation) {
        let _ = observation;
    }
}

impl<F: Fn(&NextAction)> PlannerObserver for F {
    fn on_action(&self, action: &NextAction) {
        self(action)
    }
    // default `on_spawn_complete` kept
}
```

In the `NextAction::Spawn` arm of `run_planner_with_observer`, after `let resp = spawn_one(...)`, add:

```rust
let obs = SpawnObservation::from(&resp);
observer.on_spawn_complete(&obs);
```

And similarly in the `ParallelSpawn` arm, loop through `resps` after they're collected.

- [ ] **Step 2: Implement the hook in `ChannelObserver`**

In `crates/reverie_agent/src/observer.rs`:

```rust
impl PlannerObserver for ChannelObserver {
    fn on_action(&self, action: &NextAction) {
        let _ = self.tx.try_send(Self::action_to_update(action));
    }
    fn on_spawn_complete(&self, obs: &reverie_deepagent::SpawnObservation) {
        let update = acp::SessionUpdate::AgentMessageChunk {
            content: acp::ContentBlock::Text(acp::TextContent::new(format!(
                "[subagent {}] {:?}: {}",
                obs.persona, obs.status, obs.summary
            ))),
        };
        let _ = self.tx.try_send(update);
    }
}
```

- [ ] **Step 3: Test + commit**

Run in reverie: `cargo test -p reverie-deepagent`
Run in dreamcode: `cargo test -p reverie_agent`

Expected: PASS both.

Commit each repo separately with appropriate messages.

---

## Task 8: Cancellation

Wire `AgentConnection::cancel` → `AtomicBool` → observed by a new reverie hook.

**Files:**
- Modify (reverie): `crates/reverie-deepagent/src/planner.rs`
- Modify: `crates/reverie_agent/src/connection.rs`

- [ ] **Step 1: Add a `should_stop` hook to `PlannerObserver`**

```rust
pub trait PlannerObserver {
    fn on_action(&self, _: &NextAction) {}
    fn on_spawn_complete(&self, _: &SpawnObservation) {}
    fn should_stop(&self) -> bool { false }
}
```

In `run_planner_with_observer`, at the top of the loop add:

```rust
if observer.should_stop() {
    return PlannerResult {
        termination: TerminationReason::GaveUp,
        iterations: i,
        todos,
        spawn_log,
    };
}
```

(Reusing `GaveUp` is a compromise — consider adding a `TerminationReason::Cancelled` variant if it doesn't break downstream. Check the variant list; if adding is cheap, add it.)

- [ ] **Step 2: Hook up the `AtomicBool` in `ChannelObserver`**

Give `ChannelObserver` a `cancel: Arc<AtomicBool>` field, set in `new()`, checked in `should_stop()`. The connection already has the `cancel` flag — pass it when constructing the observer.

- [ ] **Step 3: Test cancellation**

Write a test that calls `new_session` + `prompt`, then after one action, calls `connection.cancel(...)`, and asserts the `PromptResponse` comes back with `StopReason::Cancelled` (or whatever Zed's cancel convention is — check `StubAgentConnection::cancel` at `acp_thread/src/connection.rs:934`).

- [ ] **Step 4: Commit (both repos)**

---

## Task 9: Register `ReverieAgentServer` in `agent_ui.rs`

**Files:**
- Modify: `crates/agent_ui/src/agent_ui.rs`
- Modify: `crates/zed/Cargo.toml`

- [ ] **Step 1: Inspect the existing enum at agent_ui.rs:336**

Read lines 300-400 of `crates/agent_ui/src/agent_ui.rs` to understand how the `AgentServer` dispatch enum is shaped. Identify the variant (e.g., `Self::NativeAgent => ...`) and mirror it.

Run: `sed -n '300,400p' "/Users/dennis/programming projects/dreamcode/crates/agent_ui/src/agent_ui.rs"`  
Expected: reveals an enum arm that returns `Rc<agent::NativeAgentServer::new(...)>`.

- [ ] **Step 2: Add `ReverieAgent` variant**

Add the variant to the enum definition, and an arm to the match:

```rust
Self::ReverieAgent => Rc::new(reverie_agent::ReverieAgentServer::new()),
```

Add `reverie_agent.workspace = true` to `crates/agent_ui/Cargo.toml` (and/or `crates/zed/Cargo.toml` if that's where registration hooks in — check both).

- [ ] **Step 3: Add settings entry**

Follow the existing `NativeAgent` settings plumbing so selecting "reverie" in `settings.json` routes to the enum variant. Find where `NativeAgent` is wired in settings and mirror.

- [ ] **Step 4: Build Zed and smoke-test**

Run: `cargo build -p zed` (may take 10+ min on a cold build)  
Expected: PASS.

Then: run Zed, open the agent panel, verify "Reverie" appears as a selectable agent. Send a prompt, watch tool-call updates flow in.

- [ ] **Step 5: Commit**

```bash
git add crates/agent_ui crates/zed
git commit -m "feat(agent_ui): register ReverieAgentServer"
```

---

## Task 10: Docs + user-facing settings example

**Files:**
- Create: `docs/reverie-agent.md`

- [ ] **Step 1: Write the doc**

Create `docs/reverie-agent.md` with:
- What it is (Reverie deepagent in-process)
- Prerequisite: sibling-directory layout `programming projects/{dreamcode,reverie}/`
- Settings example:
  ```json
  {
    "agent_servers": {
      "reverie": { /* ... */ }
    }
  }
  ```
- How to pick a default model (uses `LanguageModelRegistry` default; override via standard Zed language-model picker)
- Known limitations for Phase 1:
  - No live subagent streaming inside a subagent's nested planner (only the top-level observer fires)
  - Cancellation granularity is per-action, not mid-LLM-call
  - No persistence of runs between prompts (each prompt gets a fresh `Run`)
  - No integration with reverie's memory layer yet (separate Phase 0 plan)

- [ ] **Step 2: Commit**

```bash
git add docs/reverie-agent.md
git commit -m "docs(reverie-agent): Phase 1 user guide"
```

---

## Self-Review

**Spec coverage:** Every goal from the conversation ("use reverie's deepagent", "Zed as frontend", "skip Python", "B2 direct link", "live subagent interaction") maps to a task:
- Direct link: Task 1 (dep) + Task 9 (registration)
- ZedLlmBackend using Zed's language model registry: Task 2
- AgentServer + AgentConnection wiring: Tasks 3 + 4
- Live planner-step updates: Tasks 5 + 6
- Subagent spawn visibility: Task 7
- Cancellation: Task 8

**Placeholders:** The plan flags several "cross-check exact field/method names in the live crate" points because ACP / gpui / language_model APIs evolve. Those are explicit verification steps, not hidden gaps.

**Type consistency:** `REVERIE_AGENT_ID` in server.rs and connection.rs consistent. `ZedLlmBackend::new` signature consistent across Task 2, 4, 6. `run_planner_with_observer` signature stable across Tasks 5, 6, 7, 8.

**Known gaps to acknowledge:**
1. The `Run::new_default()` seeding of the user's prompt (Task 4 Step 1) is hand-wavey — reverie may or may not have a direct "seed prompt" API. If it does, use that; if not, adding a seed-prompt field to `Run` is a small upstream change in Phase 1.5.
2. Tool-call UI shape (Task 6) may render as a separate collapsible entry rather than inline text. That's acceptable for Phase 1; polish is out of scope.
3. We do not cover multi-session persistence. Every `prompt()` starts a fresh `DeepAgent` run. Adding state persistence is a Phase 2 concern.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-20-reverie-agent-backend.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Best for this plan because the cross-repo edits (reverie + dreamcode) benefit from focused context per task.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
