# Phase 1.5b — Universal Memory Middleware Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wrap every non-Reverie `AgentServer` returned by `Agent::server()` in a `ReverieAugmentedAgentServer` that pre-pends reverie memory into each `prompt()` call and fires a fire-and-forget user-intent save on successful completion. Opt out per agent via `env.REVERIE_AUGMENT=0` in `settings.json`.

**Architecture:** New file `crates/reverie_agent/src/augment.rs` defines `ReverieAugmentedAgentServer` and `ReverieAugmentedConnection` as thin wrappers around `Rc<dyn AgentServer>` / `Rc<dyn AgentConnection>`. A pure `augment_prompt_blocks` helper handles the text insertion deterministically and is unit-tested. `agent_ui::Agent::server()` signature gains `&Entity<Project>` + `&App` parameters so the factory can build a `ReverieHttpClient` per connection; four call sites updated to pass those through.

**Tech Stack:** Rust, existing `ReverieHttpClient` (Phase 1.5a), `Rc<dyn AgentServer>` / `Rc<dyn AgentConnection>` from Zed's agent infrastructure, `agent_client_protocol` for `PromptRequest` / `ContentBlock`.

**Spec reference:** `docs/superpowers/specs/2026-04-21-phase-1-5b-universal-memory-middleware-design.md`.

**Working directory:** `/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/` on branch `feat/reverie-agent-backend`. No reverie-repo changes in Phase 1.5b.

---

## File Structure

**New files (dreamcode worktree):**
- `crates/reverie_agent/src/augment.rs` — `ReverieAugmentedAgentServer`, `ReverieAugmentedConnection`, `augment_with_memory` factory, `augment_prompt_blocks` pure helper.

**Modified files:**
- `crates/reverie_agent/src/reverie_agent.rs` — `mod augment;` + `pub use augment::augment_with_memory;`.
- `crates/reverie_agent/src/http.rs` — make `SmartContext` and its `content` field `pub(crate)` visible for the `augment` module's tests. (If already `pub(crate)`, no change.)
- `crates/reverie_agent/src/tests.rs` — 3 new unit tests for `augment_prompt_blocks`.
- `crates/agent_ui/src/agent_ui.rs` — `Agent::server()` signature changes; body wraps non-Reverie results.
- `crates/agent_ui/src/agent_panel.rs` — update 2 callers to pass `project` + `cx`.
- `crates/agent_ui/src/thread_import.rs` — update 1 caller.
- `crates/agent_ui/src/threads_archive_view.rs` — update 1 caller.
- `docs/reverie-agent.md` — new section "Memory for non-Reverie agents" + updated Known limitations.

---

## Task 1: Pure helper `augment_prompt_blocks` (TDD)

**Files:**
- Create: `crates/reverie_agent/src/augment.rs`
- Modify: `crates/reverie_agent/src/reverie_agent.rs`
- Modify: `crates/reverie_agent/src/tests.rs`

- [ ] **Step 1: Create `augment.rs` skeleton with the pure helper**

Create `crates/reverie_agent/src/augment.rs`:

```rust
use agent_client_protocol as acp;

use crate::http::SmartContext;

/// Insert a "Relevant memory:\n<ctx>\n" text ContentBlock at position 0 of
/// the prompt's block list when memory is present and non-empty. Otherwise
/// return the blocks unchanged.
pub(crate) fn augment_prompt_blocks(
    mut blocks: Vec<acp::ContentBlock>,
    memory: Option<SmartContext>,
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
```

- [ ] **Step 2: Register the module**

In `crates/reverie_agent/src/reverie_agent.rs`, update the mod block:

Before:
```rust
mod backend;
mod connection;
mod http;
mod observer;
mod server;
```

After:
```rust
mod augment;
mod backend;
mod connection;
mod http;
mod observer;
mod server;
```

(Later tasks add a `pub use augment::augment_with_memory;` line — skip for now.)

- [ ] **Step 3: Expose `SmartContext.content` at pub(crate)**

In `crates/reverie_agent/src/http.rs`, verify the existing `SmartContext`:

```rust
#[derive(Debug, Clone)]
pub struct SmartContext {
    pub content: String,
}
```

The `content` field is already `pub` so the `augment` sibling module can read it. No change needed. Proceed.

- [ ] **Step 4: Write three failing unit tests**

Append to `crates/reverie_agent/src/tests.rs` at the end of the file:

```rust
mod augment_tests {
    use crate::augment::augment_prompt_blocks;
    use crate::http::SmartContext;
    use agent_client_protocol as acp;

    fn text_block(s: &str) -> acp::ContentBlock {
        acp::ContentBlock::Text(acp::TextContent::new(s.to_string()))
    }

    fn block_text(b: &acp::ContentBlock) -> Option<&str> {
        match b {
            acp::ContentBlock::Text(t) => Some(t.text.as_ref()),
            _ => None,
        }
    }

    #[test]
    fn augment_prompt_blocks_prepends_memory_when_present() {
        let blocks = vec![text_block("hello")];
        let memory = Some(SmartContext { content: "- item 1".into() });
        let out = augment_prompt_blocks(blocks, memory);
        assert_eq!(out.len(), 2);
        assert_eq!(
            block_text(&out[0]).unwrap(),
            "Relevant memory:\n- item 1\n"
        );
        assert_eq!(block_text(&out[1]).unwrap(), "hello");
    }

    #[test]
    fn augment_prompt_blocks_passes_through_on_no_memory() {
        let blocks = vec![text_block("hello")];
        let out = augment_prompt_blocks(blocks, None);
        assert_eq!(out.len(), 1);
        assert_eq!(block_text(&out[0]).unwrap(), "hello");
    }

    #[test]
    fn augment_prompt_blocks_skips_empty_memory() {
        let blocks = vec![text_block("hello")];
        let memory = Some(SmartContext { content: "   \n".into() });
        let out = augment_prompt_blocks(blocks, memory);
        assert_eq!(out.len(), 1, "whitespace-only memory should be skipped");
        assert_eq!(block_text(&out[0]).unwrap(), "hello");
    }
}
```

Note: `acp::TextContent::new` returns a struct with a `text` field. The exact accessor (`.text`, `.as_ref()`, `.text.as_ref()`, etc.) depends on the `TextContent` shape — inspect at run time. If `.text` is `Arc<str>` or `SharedString`, the `.as_ref()` call coerces to `&str`. If it's `String`, drop the `.as_ref()`. Fix at the one call site in the helper.

- [ ] **Step 5: Run the tests — expect PASS**

Run: `cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend" && cargo test -p reverie_agent augment_tests`

Expected: 3/3 pass. The helper is already complete and the tests exercise exactly its logic.

If compile errors about `block_text` — the `.text` field access may need adjustment based on `acp::TextContent`'s shape. Inspect `crates/agent_ui/src/conversation_view/thread_view.rs` for the reference pattern that already reads `TextContent.text`.

- [ ] **Step 6: Run full reverie_agent suite**

Run: `cargo test -p reverie_agent`

Expected: 20/20 pass (17 pre-existing + 3 new).

- [ ] **Step 7: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: pure helper augment_prompt_blocks + unit tests

Adds crates/reverie_agent/src/augment.rs with a single pure function
that prepends a "Relevant memory:\n<ctx>\n" text block at position
0 of a Vec<acp::ContentBlock> when memory is Some and non-empty.

Three unit tests cover: memory present, memory None (pass-through),
and whitespace-only memory (skipped).

Task 2 wraps this helper in ReverieAugmentedConnection::prompt;
Task 3 adds the AgentServer wrapper that installs the connection
wrapper; Task 4 wires agent_ui to use the factory.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `ReverieAugmentedConnection` implementing `AgentConnection`

**Files:**
- Modify: `crates/reverie_agent/src/augment.rs`

- [ ] **Step 1: Read AgentConnection's full trait surface**

Run: `sed -n '47,190p' "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/crates/acp_thread/src/connection.rs"`

Expected: see every method on `AgentConnection`. Count them. Most are default-method optional, which means `ReverieAugmentedConnection` can omit them and get the trait default (the usual "not supported" Task::ready(Err(...))).

However, the wrapper MUST override every method that doesn't have a sensible default — otherwise a non-supported default overrides whatever the inner agent offered. The required overrides are:
- `agent_id`, `telemetry_id`, `auth_methods`, `authenticate` — no default, must override.
- `new_session`, `prompt`, `cancel`, `into_any` — no default, must override.
- `supports_load_session` / `load_session` / `supports_close_session` / `close_session` / `supports_resume_session` / `resume_session` / `supports_session_history` — have defaults saying "not supported", but if the inner agent supports them we must NOT hide that. Override as delegates.
- `retry`, `truncate`, `set_title`, `model_selector`, `telemetry`, `session_modes`, `session_config_options`, `session_list`, `terminal_auth_task` — have defaults returning None. Same concern: override as delegates so inner's capabilities surface.

In short: override everything. Every override is a one-liner.

- [ ] **Step 2: Write the connection wrapper**

Append to `crates/reverie_agent/src/augment.rs`:

```rust
use acp_thread::{
    AgentConnection, AgentModelSelector, AgentSessionConfigOptions, AgentSessionList,
    AgentSessionModes, AgentSessionRetry, AgentSessionSetTitle, AgentSessionTruncate,
    AgentTelemetry, UserMessageId,
};
use anyhow::Result;
use gpui::{App, Entity, SharedString, Task};
use project::{AgentId, Project};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use task::SpawnInTerminal;
use util::path_list::PathList;

use crate::ReverieHttpClient;

pub(crate) struct ReverieAugmentedConnection {
    inner: Rc<dyn AgentConnection>,
    http_client: Arc<ReverieHttpClient>,
}

impl ReverieAugmentedConnection {
    pub(crate) fn new(
        inner: Rc<dyn AgentConnection>,
        http_client: Arc<ReverieHttpClient>,
    ) -> Self {
        Self { inner, http_client }
    }
}

impl AgentConnection for ReverieAugmentedConnection {
    fn agent_id(&self) -> AgentId {
        self.inner.agent_id()
    }

    fn telemetry_id(&self) -> SharedString {
        self.inner.telemetry_id()
    }

    fn auth_methods(&self) -> &[acp::AuthMethod] {
        self.inner.auth_methods()
    }

    fn authenticate(&self, method: acp::AuthMethodId, cx: &mut App) -> Task<Result<()>> {
        self.inner.authenticate(method, cx)
    }

    fn new_session(
        self: Rc<Self>,
        project: Entity<Project>,
        work_dirs: PathList,
        cx: &mut App,
    ) -> Task<Result<Entity<acp_thread::AcpThread>>> {
        self.inner.clone().new_session(project, work_dirs, cx)
    }

    fn supports_load_session(&self) -> bool {
        self.inner.supports_load_session()
    }

    fn load_session(
        self: Rc<Self>,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        title: Option<SharedString>,
        cx: &mut App,
    ) -> Task<Result<Entity<acp_thread::AcpThread>>> {
        self.inner.clone().load_session(session_id, project, work_dirs, title, cx)
    }

    fn supports_close_session(&self) -> bool {
        self.inner.supports_close_session()
    }

    fn close_session(
        self: Rc<Self>,
        session_id: &acp::SessionId,
        cx: &mut App,
    ) -> Task<Result<()>> {
        self.inner.clone().close_session(session_id, cx)
    }

    fn supports_resume_session(&self) -> bool {
        self.inner.supports_resume_session()
    }

    fn resume_session(
        self: Rc<Self>,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        title: Option<SharedString>,
        cx: &mut App,
    ) -> Task<Result<Entity<acp_thread::AcpThread>>> {
        self.inner
            .clone()
            .resume_session(session_id, project, work_dirs, title, cx)
    }

    fn supports_session_history(&self) -> bool {
        self.inner.supports_session_history()
    }

    fn terminal_auth_task(
        &self,
        method: &acp::AuthMethodId,
        cx: &App,
    ) -> Option<Task<Result<SpawnInTerminal>>> {
        self.inner.terminal_auth_task(method, cx)
    }

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
            let blocks = std::mem::take(&mut params.prompt);
            params.prompt = augment_prompt_blocks(blocks, memory);

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

    fn retry(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionRetry>> {
        self.inner.retry(session_id, cx)
    }

    fn cancel(&self, session_id: &acp::SessionId, cx: &mut App) {
        self.inner.cancel(session_id, cx)
    }

    fn truncate(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionTruncate>> {
        self.inner.truncate(session_id, cx)
    }

    fn set_title(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionSetTitle>> {
        self.inner.set_title(session_id, cx)
    }

    fn model_selector(
        &self,
        session_id: &acp::SessionId,
    ) -> Option<Rc<dyn AgentModelSelector>> {
        self.inner.model_selector(session_id)
    }

    fn telemetry(&self) -> Option<Rc<dyn AgentTelemetry>> {
        self.inner.telemetry()
    }

    fn session_modes(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionModes>> {
        self.inner.session_modes(session_id, cx)
    }

    fn session_config_options(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionConfigOptions>> {
        self.inner.session_config_options(session_id, cx)
    }

    fn session_list(&self, cx: &mut App) -> Option<Rc<dyn AgentSessionList>> {
        self.inner.session_list(cx)
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}

fn user_text_from_prompt(blocks: &[acp::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            acp::ContentBlock::Text(t) => Some(t.text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
```

Imports at top of `augment.rs` need `acp` alias. Update the beginning:

```rust
use agent_client_protocol::{self as acp};

// ... existing augment_prompt_blocks
```

And SmartContext import may need to reference `crate::http::SmartContext` rather than `crate::SmartContext` — adjust based on the `pub` exposure in `reverie_agent.rs`.

- [ ] **Step 3: Verify compile**

Run: `cargo check -p reverie_agent`

Expected: clean compile. Warnings for unused `ReverieAugmentedConnection` are acceptable — Task 3 consumes it.

If any method's signature doesn't match what the trait declares — e.g., if `model_selector` actually takes different args — read the current trait in `crates/acp_thread/src/connection.rs` and adjust. The exact arg list is what matters; each delegate body is always `self.inner.method_name(args).method_kind()`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p reverie_agent`

Expected: 20/20 still pass.

- [ ] **Step 5: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: ReverieAugmentedConnection wrapping any AgentConnection

Middleware type that delegates every AgentConnection trait method
to an inner Rc<dyn AgentConnection>, except prompt(), which:

  1. extracts user text from params.prompt,
  2. await http_client.smart_context(user_text) → Option<SmartContext>,
  3. prepends a "Relevant memory:" text block via augment_prompt_blocks,
  4. await inner.prompt(id, modified_params, cx),
  5. on StopReason::EndTurn, fire-and-forget save_passive("zed-augment-
     user-intent") with the original user_text.

Task 3 adds ReverieAugmentedAgentServer that installs this connection
from AgentServer::connect.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `ReverieAugmentedAgentServer` + `augment_with_memory` factory

**Files:**
- Modify: `crates/reverie_agent/src/augment.rs`
- Modify: `crates/reverie_agent/src/reverie_agent.rs`

- [ ] **Step 1: Append the server wrapper**

In `crates/reverie_agent/src/augment.rs`, append:

```rust
use agent_servers::{AgentServer, AgentServerDelegate};
use http_client::HttpClient as _;
use language_model::LanguageModel;
use ui::IconName;

pub struct ReverieAugmentedAgentServer {
    inner: Rc<dyn AgentServer>,
    http_client: Arc<ReverieHttpClient>,
}

impl ReverieAugmentedAgentServer {
    pub fn new(
        inner: Rc<dyn AgentServer>,
        http_client: Arc<ReverieHttpClient>,
    ) -> Self {
        Self { inner, http_client }
    }
}

impl AgentServer for ReverieAugmentedAgentServer {
    fn agent_id(&self) -> AgentId {
        self.inner.agent_id()
    }

    fn logo(&self) -> IconName {
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
            let wrapped: Rc<dyn AgentConnection> =
                Rc::new(ReverieAugmentedConnection::new(inner_conn, http));
            Ok(wrapped)
        })
    }

    fn default_mode(&self, cx: &App) -> Option<acp::SessionModeId> {
        self.inner.default_mode(cx)
    }

    fn set_default_mode(
        &self,
        mode_id: Option<acp::SessionModeId>,
        fs: Arc<dyn fs::Fs>,
        cx: &mut App,
    ) {
        self.inner.set_default_mode(mode_id, fs, cx)
    }

    fn default_model(&self, cx: &App) -> Option<acp::ModelId> {
        self.inner.default_model(cx)
    }

    fn set_default_model(
        &self,
        model_id: Option<acp::ModelId>,
        fs: Arc<dyn fs::Fs>,
        cx: &mut App,
    ) {
        self.inner.set_default_model(model_id, fs, cx)
    }

    fn favorite_model_ids(&self, cx: &mut App) -> collections::HashSet<acp::ModelId> {
        self.inner.favorite_model_ids(cx)
    }

    fn default_config_option(&self, config_id: &str, cx: &App) -> Option<String> {
        self.inner.default_config_option(config_id, cx)
    }

    fn set_default_config_option(
        &self,
        config_id: &str,
        value_id: Option<&str>,
        fs: Arc<dyn fs::Fs>,
        cx: &mut App,
    ) {
        self.inner
            .set_default_config_option(config_id, value_id, fs, cx)
    }

    fn favorite_config_option_value_ids(
        &self,
        config_id: &acp::SessionConfigId,
        cx: &mut App,
    ) -> collections::HashSet<acp::SessionConfigValueId> {
        self.inner.favorite_config_option_value_ids(config_id, cx)
    }

    fn toggle_favorite_config_option_value(
        &self,
        config_id: acp::SessionConfigId,
        value_id: acp::SessionConfigValueId,
        should_be_favorite: bool,
        fs: Arc<dyn fs::Fs>,
        cx: &App,
    ) {
        self.inner.toggle_favorite_config_option_value(
            config_id,
            value_id,
            should_be_favorite,
            fs,
            cx,
        )
    }

    fn toggle_favorite_model(
        &self,
        model_id: acp::ModelId,
        should_be_favorite: bool,
        fs: Arc<dyn fs::Fs>,
        cx: &App,
    ) {
        self.inner
            .toggle_favorite_model(model_id, should_be_favorite, fs, cx)
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}
```

Check `agent_servers::AgentServer` trait for the complete method list — the above covers what's in the trait definition at `crates/agent_servers/src/agent_servers.rs:43-122`. If any method was added after that snapshot (new Zed versions), the compiler will refuse the `impl` with "must implement" or warn about trait method being missed. Fix by adding a one-liner delegate for each.

- [ ] **Step 2: Add the `augment_with_memory` factory**

Append to the same file:

```rust
/// Wrap `inner` in a `ReverieAugmentedAgentServer` that injects reverie
/// memory into every prompt() call routed through the inner server's
/// connection. Returns `None` if http_client construction fails — callers
/// should fall back to the un-wrapped inner in that case.
pub fn augment_with_memory(
    inner: Rc<dyn AgentServer>,
    project: &Entity<Project>,
    cx: &App,
) -> Option<Rc<dyn AgentServer>> {
    // Defense-in-depth: never wrap the Reverie agent itself — it already
    // does retrieval in-process, and wrapping would cause double-retrieval.
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

fn resolve_project_name_for_augment(
    project: &Entity<Project>,
    cx: &App,
) -> String {
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

- [ ] **Step 3: Re-export the factory from the crate root**

In `crates/reverie_agent/src/reverie_agent.rs`:

Before:
```rust
pub use http::ReverieHttpClient;
pub use server::{REVERIE_AGENT_ID, ReverieAgentServer};
```

After:
```rust
pub use augment::augment_with_memory;
pub use http::ReverieHttpClient;
pub use server::{REVERIE_AGENT_ID, ReverieAgentServer};
```

- [ ] **Step 4: Verify compile**

Run: `cargo check -p reverie_agent`

Expected: clean. Any unresolved imports — fix at the call site (likely `collections::HashSet`, `fs::Fs`, `task::SpawnInTerminal` — all workspace deps).

- [ ] **Step 5: Run tests**

Run: `cargo test -p reverie_agent`

Expected: 20/20 pass.

- [ ] **Step 6: Verify zed-ui still compiles (no callers yet but make sure we haven't broken anything upstream)**

Run: `cargo check -p agent_ui`

Expected: clean.

- [ ] **Step 7: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: ReverieAugmentedAgentServer + augment_with_memory factory

ReverieAugmentedAgentServer delegates every AgentServer trait method
to an inner Rc<dyn AgentServer>. connect() calls inner.connect() then
wraps the returned connection in ReverieAugmentedConnection (Task 2)
so prompt() auto-retrieves memory.

augment_with_memory(inner, project, cx) -> Option<Rc<dyn AgentServer>>:
  - Defense-in-depth: returns None if inner IS the Reverie agent.
  - Resolves base_url from REVERIE_URL env (default localhost:7437).
  - Resolves project name from REVERIE_PROJECT env or first worktree.
  - Builds ReverieHttpClient and wraps.

Task 4 wires Agent::server() to call this factory for every non-
Reverie variant.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Update `Agent::server()` signature and body in `agent_ui.rs`

**Files:**
- Modify: `crates/agent_ui/src/agent_ui.rs`

- [ ] **Step 1: Inspect current implementation**

Run: `sed -n '330,350p' "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/crates/agent_ui/src/agent_ui.rs"`

Expected: the existing `server(&self, fs, thread_store) -> Rc<dyn AgentServer>` method body.

- [ ] **Step 2: Change the signature and body**

In `crates/agent_ui/src/agent_ui.rs`, find the `server` method on `impl Agent`:

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

Replace with:

```rust
    pub fn server(
        &self,
        fs: Arc<dyn fs::Fs>,
        thread_store: Entity<agent::ThreadStore>,
        project: &Entity<Project>,
        cx: &App,
    ) -> Rc<dyn agent_servers::AgentServer> {
        // Reverie's own variant short-circuits — it already has
        // retrieval in-process, wrapping would double-retrieve.
        if let Self::ReverieAgent = self {
            return Rc::new(reverie_agent::ReverieAgentServer::new());
        }
        #[cfg(any(test, feature = "test-support"))]
        if let Self::Stub = self {
            return Rc::new(crate::test_support::StubAgentServer::default_response());
        }

        let inner: Rc<dyn agent_servers::AgentServer> = match self {
            Self::NativeAgent => Rc::new(agent::NativeAgentServer::new(fs, thread_store)),
            Self::Custom { id: name } => {
                Rc::new(agent_servers::CustomAgentServer::new(name.clone()))
            }
            // Unreachable — short-circuited above, but the match must be exhaustive.
            Self::ReverieAgent => unreachable!("short-circuited above"),
            #[cfg(any(test, feature = "test-support"))]
            Self::Stub => unreachable!("short-circuited above"),
        };

        if augment_disabled_for_agent(self, cx) {
            return inner;
        }
        reverie_agent::augment_with_memory(inner.clone(), project, cx).unwrap_or(inner)
    }
```

- [ ] **Step 3: Add the `augment_disabled_for_agent` helper**

In the same file, place a free function below the `impl Agent` block (or add it as a private item):

```rust
fn augment_disabled_for_agent(agent: &Agent, cx: &App) -> bool {
    let agent_id = agent.id();
    let settings = cx.read_global(|settings: &settings::SettingsStore, _| {
        settings
            .get::<project::agent_server_store::AllAgentServersSettings>(None)
            .get(agent_id.as_ref())
            .cloned()
    });
    let Some(settings) = settings else { return false; };
    let env = match &settings {
        project::agent_server_store::CustomAgentServerSettings::Custom { env, .. } => env,
        project::agent_server_store::CustomAgentServerSettings::Extension { env, .. } => env,
        project::agent_server_store::CustomAgentServerSettings::Registry { env, .. } => env,
    };
    env.get("REVERIE_AUGMENT")
        .map(|v| v == "0")
        .unwrap_or(false)
}
```

(If `CustomAgentServerSettings` has a different variant list or field names, adjust. The Phase 1 `custom.rs` already reads the same shape, so mirror what it does — see `crates/agent_servers/src/custom.rs` for the canonical pattern.)

- [ ] **Step 4: Verify the agent_ui crate compiles**

Run: `cargo check -p agent_ui`

Expected: FAIL at call sites — four callers of the old `server(fs, thread_store)` signature don't pass `project` + `cx` yet. Task 5 fixes them.

Specifically:
- `crates/agent_ui/src/agent_panel.rs:1053`
- `crates/agent_ui/src/agent_panel.rs:2683`
- `crates/agent_ui/src/thread_import.rs:494`
- `crates/agent_ui/src/threads_archive_view.rs:800`

These four expected-failures are OK for the staging commit. Do NOT commit until Task 5 lands call sites.

- [ ] **Step 5: (no commit — wait for Task 5)**

---

## Task 5: Update 4 call sites to pass `project` + `cx`

**Files:**
- Modify: `crates/agent_ui/src/agent_panel.rs` (2 call sites)
- Modify: `crates/agent_ui/src/thread_import.rs` (1 call site)
- Modify: `crates/agent_ui/src/threads_archive_view.rs` (1 call site)

For each call site, find what `project` is available in scope and pass it. Each call is a two-argument addition.

- [ ] **Step 1: Fix `crates/agent_ui/src/agent_panel.rs` line ~1053**

Inspect context. Run:
```
sed -n '1045,1060p' "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/crates/agent_ui/src/agent_panel.rs"
```

Find the line:
```rust
Agent::NativeAgent.server(fs.clone(), thread_store.clone()),
```

Modify to pass `project` and `cx`. What `project` is in scope? The `AgentPanel` has a `self.project` field (or similar) — inspect the surrounding method. Replace with:

```rust
Agent::NativeAgent.server(fs.clone(), thread_store.clone(), &self.project, cx),
```

If `self.project` isn't accessible from the current scope, use whichever `project` binding IS in scope (search upwards). For the `&mut App` / `&App` issue: `server` takes `&App`, the caller's `cx` is usually `&mut Context<_>` which derefs to `&App` freely.

- [ ] **Step 2: Fix `crates/agent_ui/src/agent_panel.rs` line ~2683**

Inspect context. Run:
```
sed -n '2675,2690p' "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/crates/agent_ui/src/agent_panel.rs"
```

Find:
```rust
.unwrap_or_else(|| agent.server(self.fs.clone(), self.thread_store.clone()));
```

Modify:
```rust
.unwrap_or_else(|| agent.server(self.fs.clone(), self.thread_store.clone(), &self.project, cx));
```

(Adjust `self.project` to match the actual field name — the agent_panel struct is the reference.)

- [ ] **Step 3: Fix `crates/agent_ui/src/thread_import.rs` line ~494**

Inspect context. Run:
```
sed -n '485,500p' "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/crates/agent_ui/src/thread_import.rs"
```

Find:
```rust
let server = agent.server(<dyn Fs>::global(cx), ThreadStore::global(cx));
```

Modify:
```rust
let server = agent.server(<dyn Fs>::global(cx), ThreadStore::global(cx), &project, cx);
```

Where `project` — look at the surrounding function for the available project binding. If none is in scope, the function signature may need a `project: &Entity<Project>` added; trace back up to the caller. Note: this change has a potential for cascade. If thread_import is called from a GPUI action that doesn't carry a project, plumbing one through may be substantial. Inspect at implementation time and decide whether to:
- Plumb project down through thread_import's caller chain, OR
- Short-circuit: if we can't get a project, skip augmentation for that call site (use `agent.server_without_augment(fs, thread_store)` — a separate method). **RECOMMENDED:** add a `server_without_augment` overload used by thread_import ONLY, and keep `server` as the augmented path.

Let me define that now in the same pass so the plan accommodates it.

**Add a `server_without_augment` method** in `crates/agent_ui/src/agent_ui.rs`:

```rust
    /// Build an AgentServer WITHOUT reverie memory augmentation. Used
    /// by callers that don't have a project handle in scope (e.g., thread
    /// import). These callers opt out of Phase 1.5b automatically.
    pub fn server_without_augment(
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

Then in `thread_import.rs:494`:
```rust
let server = agent.server_without_augment(<dyn Fs>::global(cx), ThreadStore::global(cx));
```

Add a comment at the call site: `// thread_import doesn't have a project handle; augment is skipped here.`

- [ ] **Step 4: Fix `crates/agent_ui/src/threads_archive_view.rs` line ~800**

Inspect context. Run:
```
sed -n '795,805p' "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/crates/agent_ui/src/threads_archive_view.rs"
```

Find:
```rust
.request_connection(agent.clone(), agent.server(fs, ThreadStore::global(cx)), cx)
```

If `project` is in scope via `self.project` or a local, pass it. If not, use `server_without_augment` like thread_import. Archive-view rehydration of an existing thread often has the project cached; trace it.

Likely form:
```rust
.request_connection(agent.clone(), agent.server(fs, ThreadStore::global(cx), &project, cx), cx)
```

OR:
```rust
.request_connection(agent.clone(), agent.server_without_augment(fs, ThreadStore::global(cx)), cx)
```

Pick based on what's available.

- [ ] **Step 5: Verify agent_ui compiles**

Run: `cargo check -p agent_ui`

Expected: clean compile.

- [ ] **Step 6: Verify the full zed binary still compiles**

Run: `cargo check -p zed`

Expected: clean (takes several minutes cold).

- [ ] **Step 7: Commit Task 4 + Task 5 together**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/agent_ui
git commit -m "$(cat <<'EOF'
agent_ui: route non-Reverie AgentServer builds through augment_with_memory

Agent::server() now takes project + cx and calls
reverie_agent::augment_with_memory on non-Reverie variants. Reverie
short-circuits (already has retrieval in-process). Opt-out per agent
via env.REVERIE_AUGMENT=0 in settings.json.

Call sites updated:
  - agent_panel.rs:1053  → passes &self.project, cx
  - agent_panel.rs:2683  → passes &self.project, cx
  - thread_import.rs:494 → uses new server_without_augment (no project
                            handle in scope)
  - threads_archive_view.rs:800 → passes &project, cx

server_without_augment is a new overload for callers that can't (or
don't need to) augment. It builds the inner AgentServer identically
but skips the wrapper.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Documentation update

**Files:**
- Modify: `docs/reverie-agent.md`

- [ ] **Step 1: Add a "Memory for non-Reverie agents" section**

Read the current `docs/reverie-agent.md`. After the "## Memory (auto-retrieval)" section (which describes the Reverie agent's retrieval), insert a new section:

```markdown
## Memory for non-Reverie agents (Phase 1.5b)

When you pick Claude, Gemini, Zed Agent, or any Custom ACP agent in the agent panel, Zed wraps that agent in a memory-augmented connection:

1. **Before each prompt** — calls `GET /context/smart?q=<your prompt>&project=<project>` with a 5s timeout. On hit, a `Relevant memory:\n<block>\n` text block is prepended to the outgoing prompt (position 0), before any user text or mentions.
2. **After each successful prompt** (StopReason = EndTurn) — fires a fire-and-forget POST to `/observations/passive`:
   - `{ session_id, content: <your prompt>, project, source: "zed-augment-user-intent" }`

Failed prompts (StopReason = MaxTurnRequests / Refusal / Cancelled / etc.) do NOT save.

### Opt-out per agent

Set `REVERIE_AUGMENT=0` in the agent's env:

```json
{
  "agent_servers": {
    "claude-acp": {
      "env": {
        "REVERIE_AUGMENT": "0"
      }
    }
  }
}
```

### What's different from the Reverie agent

- **No UI breadcrumb.** The wrapped agent's chat doesn't show a "[memory] consulted reverie" chunk. Check `REVERIE_URL`'s reveried logs if you want to confirm retrieval happened.
- **User intent only; no assistant-side capture.** The wrapper can't see the LLM's response text through `PromptResponse`, so only the user's prompt is saved to reverie. Switch to the Reverie agent for fuller capture.
- **Thread import bypasses augmentation.** Opening an older thread's history rebuilds connections without retrieval.
```

- [ ] **Step 2: Update the "Known limitations" section**

Replace the existing "Memory" bullet (or add a new one) with:

```markdown
- **Non-Reverie agents get user-intent capture only.** See "Memory for non-Reverie agents" above. Assistant responses are not saved to reverie when using Claude/Gemini/Zed-native; use the Reverie agent for assistant-side capture.
```

If there's an existing bullet saying "No memory available to non-Reverie agents" (from Phase 1.5a), REMOVE it — Phase 1.5b resolves that.

- [ ] **Step 3: Verify doc**

Run: `grep -c "Memory for non-Reverie agents" "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/docs/reverie-agent.md"`

Expected: 1.

- [ ] **Step 4: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add docs/reverie-agent.md
git commit -m "$(cat <<'EOF'
docs(reverie-agent): Phase 1.5b — memory for non-Reverie agents

Adds a section describing the universal memory wrapper: Claude /
Gemini / Zed-native / Custom agents all get automatic /context/smart
retrieval and /observations/passive save (user intent only) when
reveried is reachable. Per-agent opt-out via env.REVERIE_AUGMENT=0.

Removes the "no memory for non-Reverie agents" limitation bullet
from the Known limitations list.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**1. Spec coverage:**

| Spec section | Tasks |
|---|---|
| §1 Architecture — wrapper types + Agent::server dispatch | Tasks 2, 3, 4 |
| §2.1 augment.rs pure helper + trait impls | Tasks 1, 2, 3 |
| §2.2 augment_with_memory factory | Task 3 |
| §2.3 Agent::server signature + body | Task 4 |
| §2.3 Caller updates | Task 5 |
| §2.4 reverie_agent.rs mod + re-export | Tasks 1, 3 |
| §3.1–3.7 Error handling flows | Behaviour documented; implementation spreads across Tasks 2-4 |
| §4.1 3 unit tests for augment_prompt_blocks | Task 1 |
| §5 Known limitations | Task 6 (docs) |

**2. Placeholder scan:** No TBDs. Task 5 Step 3 flags a potential plumbing cascade in thread_import and offers a concrete fallback (`server_without_augment`); this is a real decision branch, not a placeholder. The implementer inspects surrounding scope and picks one. Same pattern in Task 5 Step 4 for threads_archive_view.

**3. Type consistency:**
- `ReverieAugmentedAgentServer::new(inner, http_client)` consistent across Tasks 3, §2.1.
- `ReverieAugmentedConnection::new(inner, http_client)` consistent across Task 2 (def) and Task 3 (use at server's connect()).
- `augment_with_memory(inner, project, cx) -> Option<Rc<dyn AgentServer>>` consistent across Task 3 (def) and Task 4 (call site).
- `augment_prompt_blocks(blocks, memory) -> Vec<ContentBlock>` consistent across Task 1 (def) and Task 2 (use in prompt()).
- `Agent::server(fs, thread_store, project, cx)` consistent across Task 4 (new signature) and Task 5 (all call sites).
- `server_without_augment(fs, thread_store)` has the original signature; consistent in Task 5 Step 3's definition and use.
- `REVERIE_AUGMENT` setting key quoted identically in Task 4 Step 3 code and Task 6 Step 1 docs.
- `zed-augment-user-intent` save source string consistent across Task 2 Step 2 code and Task 6 Step 1 docs.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-21-phase-1-5b-universal-memory-middleware.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
