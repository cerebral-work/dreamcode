# Phase 1.5d — Mid-Call Cancel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `AgentConnection::cancel` interrupt an in-flight `stream_completion_text` within a chunk boundary instead of waiting for the current LLM turn to finish. Cancelled mid-calls report as `TerminationReason::Cancelled` (not `Backend`).

**Architecture:** Add a per-prompt `smol::channel::bounded::<()>(1)` cancel notifier held on the `Session`. `AgentConnection::cancel` fires `try_send(())` on it (in addition to flipping the existing AtomicBool). A new pure-control-flow helper `race_with_cancel` is used by `stream_to_string_cancellable` to race both the initial `stream_completion_text` await and each subsequent `stream.next()` against `cancel_rx.recv()`. Reverie's planner backend-error arm checks `observer.should_stop()` and reports `Cancelled` when true.

**Tech Stack:** Rust, `smol::channel` (already a dep), `futures::future::select` / `FutureExt::fuse`, `parking_lot::Mutex`.

**Spec reference:** `docs/superpowers/specs/2026-04-21-phase-1-5d-mid-call-cancel-design.md`.

**Working directory:** `/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/` on branch `feat/reverie-agent-backend`. Reverie at `/Users/dennis/programming projects/reverie` on branch `feat/planner-observer`.

---

## File Structure

**Modified files (reverie repo):**
- `crates/reverie-deepagent/src/planner.rs` — inside `run_planner_with_observer_and_todos`, change the Err arm of the `backend.next_action` match to pick `Cancelled` when `observer.should_stop()`. Add 1 unit test.

**Modified files (dreamcode worktree):**
- `crates/reverie_agent/src/connection.rs` — `Session` field `cancel_notify`, `new_session` init, `cancel()` fires try_send, `prompt()` installs per-prompt channel and uses cancellable helper, new helpers `race_with_cancel` and `stream_to_string_cancellable`, delete `stream_to_string`.
- `crates/reverie_agent/src/tests.rs` — 2 new tests for `race_with_cancel`.
- `docs/reverie-agent.md` — one-paragraph update to the "Canceling a run" / "Phase 1.5c limitations" sections to reflect responsive cancel.

---

## Task 1: Reverie upstream — `Cancelled` on backend-error when `should_stop` (TDD)

**Files (reverie repo):**
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/planner.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `planner.rs`, immediately after `empty_seed_matches_legacy_entry_point`:

```rust
#[test]
fn backend_error_with_should_stop_reports_cancelled() {
    use std::cell::RefCell;

    struct ScriptedThenErrBackend {
        step: RefCell<u32>,
        fail_on: u32,
    }
    impl LlmBackend for ScriptedThenErrBackend {
        fn next_action(
            &mut self,
            _todos: &TodoList,
            _vfs: &Vfs,
            _obs: &[SpawnObservation],
        ) -> Result<NextAction, BackendError> {
            let mut n = self.step.borrow_mut();
            *n += 1;
            if *n >= self.fail_on {
                Err(BackendError::Transport("simulated cancel".into()))
            } else {
                Ok(NextAction::AddTodo("keeps-planner-running".into()))
            }
        }
    }

    struct StopOnBackendError {
        hits: std::sync::Mutex<u32>,
    }
    impl PlannerObserver for StopOnBackendError {
        fn on_action(&self, _: &NextAction) {
            *self.hits.lock().unwrap() += 1;
        }
        // Report stop after at least one productive action so the Err-arm
        // branch gets exercised while should_stop already returns true.
        fn should_stop(&self) -> bool {
            *self.hits.lock().unwrap() >= 1
        }
    }

    let (_p, run) = fresh_run();
    let mut backend = ScriptedThenErrBackend {
        step: RefCell::new(0),
        fail_on: 2,
    };
    let observer = StopOnBackendError {
        hits: std::sync::Mutex::new(0),
    };
    let result = run_planner_with_observer(
        &run,
        &mut backend,
        10,
        &SpawnConfig::default(),
        &observer,
    );
    assert_eq!(
        result.termination,
        TerminationReason::Cancelled,
        "backend error + should_stop=true must report Cancelled, not Backend"
    );
}
```

Note: the test uses `RefCell` for `step` because `next_action` takes `&mut self` but we want interior mutability without needing `Send` + `Sync` bounds the trait doesn't force. The observer's `hits` is a `std::sync::Mutex` because `PlannerObserver::should_stop` takes `&self` and must be Send + Sync.

- [ ] **Step 2: Run the test; verify it fails**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent backend_error_with_should_stop_reports_cancelled`

Expected: FAIL with `assertion \`left == right\` failed` — the planner currently returns `Backend` regardless of should_stop.

- [ ] **Step 3: Update the backend-error arm in the planner loop**

In `crates/reverie-deepagent/src/planner.rs`, inside `run_planner_with_observer_and_todos`, find the `Err(e) =>` arm of the `match backend.next_action(...)`:

```rust
        let action = match backend.next_action(&todos, &run.vfs, &obs_for_turn) {
            Ok(a) => a,
            Err(e) => {
                warn!(iter = i, error = %e, "deepagent: backend error, terminating");
                return PlannerResult {
                    termination: TerminationReason::Backend,
                    iterations: i,
                    todos,
                    spawn_log,
                };
            },
        };
```

Replace the `Err(e) =>` arm with:

```rust
            Err(e) => {
                // If cancellation was requested, the error is almost
                // certainly a symptom of the host tearing down an in-flight
                // LLM call — report as Cancelled so the UX is truthful.
                let termination = if observer.should_stop() {
                    TerminationReason::Cancelled
                } else {
                    TerminationReason::Backend
                };
                warn!(
                    iter = i,
                    error = %e,
                    ?termination,
                    "deepagent: backend error, terminating"
                );
                return PlannerResult {
                    termination,
                    iterations: i,
                    todos,
                    spawn_log,
                };
            },
```

- [ ] **Step 4: Run the new test plus the full suite; verify everything passes**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent`

Expected: 79/79 pass (78 pre-existing + 1 new).

If any pre-existing test regressed: the most likely cause is that a test fixture has an observer that returns true from `should_stop` earlier than expected. Re-read the failing test — the planner's Err handling should still pick `Backend` when `should_stop() == false`.

- [ ] **Step 5: Commit**

```bash
cd "/Users/dennis/programming projects/reverie"
git add crates/reverie-deepagent/src/planner.rs
git commit -m "$(cat <<'EOF'
deepagent: report Cancelled on backend error when should_stop is set

When the planner's call to backend.next_action returns Err and
observer.should_stop() is true at that moment, treat the termination
as Cancelled rather than Backend. The error is almost always a
symptom of the host tearing down an in-flight LLM call in response
to user cancellation; reporting Backend gives the wrong signal to
downstream consumers (meshctl, UI summaries).

Pre-existing behaviour when should_stop is false (real transport
errors) is unchanged — still terminates as Backend.

Enables the Phase 1.5d dreamcode mid-call cancel to surface truthfully
in the Zed agent panel.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: Verify dreamcode still compiles on the updated reverie**

Run: `cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend" && cargo check -p reverie_agent`

Expected: PASS. No symbol changes so no cascading effects.

---

## Task 2: `race_with_cancel` helper + tests (dreamcode, TDD)

**Files:**
- Modify: `crates/reverie_agent/src/connection.rs`
- Modify: `crates/reverie_agent/src/tests.rs`

- [ ] **Step 1: Write failing tests for `race_with_cancel`**

Append to `crates/reverie_agent/src/tests.rs` at the end of the file:

```rust
mod cancel_race_tests {
    use crate::connection::race_with_cancel;

    #[test]
    fn race_with_cancel_fires_err_when_cancel_pre_seeded() {
        let (tx, rx) = smol::channel::bounded::<()>(1);
        tx.try_send(()).unwrap();

        let result: anyhow::Result<()> = futures::executor::block_on(race_with_cancel(
            futures::future::pending::<anyhow::Result<()>>(),
            &rx,
            "cancelled",
        ));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("cancelled"),
            "expected error message to contain 'cancelled'"
        );
    }

    #[test]
    fn race_with_cancel_lets_work_win_when_no_signal() {
        let (_tx, rx) = smol::channel::bounded::<()>(1);

        let result: anyhow::Result<u32> = futures::executor::block_on(race_with_cancel(
            async { Ok(42u32) },
            &rx,
            "cancelled",
        ));
        assert_eq!(result.unwrap(), 42);
    }
}
```

- [ ] **Step 2: Run the tests; verify they fail to compile**

Run: `cargo test -p reverie_agent cancel_race_tests`

Expected: FAIL with `unresolved import 'crate::connection::race_with_cancel'`.

- [ ] **Step 3: Implement `race_with_cancel` in connection.rs**

In `crates/reverie_agent/src/connection.rs`, add near the bottom of the file (after `build_persistent_run` and before `user_text_from_prompt`):

```rust
pub(crate) async fn race_with_cancel<T, F>(
    work: F,
    cancel_rx: &smol::channel::Receiver<()>,
    cancel_err: &'static str,
) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    use futures::future::Either;

    let work = Box::pin(work);
    let cancel = Box::pin(cancel_rx.recv());
    match futures::future::select(work, cancel).await {
        Either::Left((Ok(t), _)) => Ok(t),
        Either::Left((Err(e), _)) => Err(e),
        Either::Right(_) => Err(anyhow!(cancel_err)),
    }
}
```

- [ ] **Step 4: Run the tests; verify they pass**

Run: `cargo test -p reverie_agent cancel_race_tests`

Expected: both tests PASS.

Also run the full suite to make sure nothing else regressed:

Run: `cargo test -p reverie_agent`

Expected: 17/17 pass (15 pre-existing + 2 new).

- [ ] **Step 5: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: add race_with_cancel helper for mid-await cancellation

Pure async control-flow primitive: races a work future against a
cancel channel Receiver. On cancel-wins, returns Err with a
caller-supplied message; on work-wins, returns the work's Result.
The work future is dropped (cancelled) when the cancel arm wins,
which for HTTP streams means the underlying request is torn down.

Task 3 uses this inside stream_to_string_cancellable to race both
the stream_completion_text future and each stream.next() chunk
against the session's per-prompt cancel_rx.

Two tests with futures::future::pending() as the work verify the
cancel-first path errors and the work-only path returns the real
value.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `stream_to_string_cancellable` replaces `stream_to_string`

**Files:**
- Modify: `crates/reverie_agent/src/connection.rs`

- [ ] **Step 1: Add `stream_to_string_cancellable`, delete `stream_to_string`**

In `crates/reverie_agent/src/connection.rs`, find the existing `stream_to_string` helper near the bottom of the file:

```rust
async fn stream_to_string(
    model: &Arc<dyn LanguageModel>,
    request: LanguageModelRequest,
    cx: &AsyncApp,
) -> Result<String> {
    let mut text_stream = model
        .stream_completion_text(request, cx)
        .await
        .map_err(|e| anyhow!("stream_completion_text failed: {e}"))?;
    let mut text = String::new();
    while let Some(chunk) = text_stream.stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("stream chunk error: {e}"))?;
        text.push_str(&chunk);
    }
    Ok(text)
}
```

Replace it entirely with:

```rust
async fn stream_to_string_cancellable(
    model: &Arc<dyn LanguageModel>,
    request: LanguageModelRequest,
    cx: &AsyncApp,
    cancel_rx: &smol::channel::Receiver<()>,
) -> Result<String> {
    let text_stream_fut = async {
        model
            .stream_completion_text(request, cx)
            .await
            .map_err(|e| anyhow!("stream_completion_text failed: {e}"))
    };
    let mut text_stream = race_with_cancel(
        text_stream_fut,
        cancel_rx,
        "cancelled before stream started",
    )
    .await?;

    let mut text = String::new();
    loop {
        let next_fut = async { Ok::<_, anyhow::Error>(text_stream.stream.next().await) };
        let next = race_with_cancel(next_fut, cancel_rx, "cancelled mid-stream").await?;
        match next {
            None => break,
            Some(Ok(chunk)) => text.push_str(&chunk),
            Some(Err(e)) => return Err(anyhow!("stream chunk error: {e}")),
        }
    }
    Ok(text)
}
```

Note: the inner `async` blocks produce `Result<T>` so they slot into `race_with_cancel`'s signature. The chunk-level loop wraps `text_stream.stream.next()` (which returns `Option<Result<String, ...>>`) in `Ok(...)` so the Option-layered result can be raced.

- [ ] **Step 2: Update the call site in the foreground driver loop**

In the same file, inside the `cx.spawn(async move |cx| { ... })` block in `prompt()`, find:

```rust
            while let Ok(req) = req_rx.recv().await {
                let request = build_language_model_request(req.messages);
                let text_result = stream_to_string(&model, request, cx).await;
                let reply_payload = match text_result {
                    Ok(text) => Ok(text),
                    Err(e) => Err(e.to_string()),
                };
                if req.reply.send(reply_payload).is_err() {
                    log::warn!(
                        "reverie planner dropped its reply channel while the driver still held a request"
                    );
                }
            }
```

Task 4 will replace `stream_to_string` with `stream_to_string_cancellable` once the `cancel_rx` is in scope. For this task, leave the call site alone — compilation should still succeed because `stream_to_string_cancellable` is defined but unused. If the compiler complains about `stream_to_string` being an unresolved name (because we deleted it), then we DO need to change the call site here. Inspect the error and either:

(a) If `stream_to_string` is still referenced: replace its call with a temporary shim that passes a never-firing cancel_rx:

```rust
let (_never_tx, never_rx) = smol::channel::bounded::<()>(1);
let text_result = stream_to_string_cancellable(&model, request, cx, &never_rx).await;
```

…and mark this as explicitly-temporary with a comment: `// Task 4 wires the session's cancel_rx here.`

(b) If `stream_to_string` is already unused (compile succeeds), skip to Step 3.

- [ ] **Step 3: Verify compile**

Run: `cargo check -p reverie_agent`

Expected: clean compile. Warnings permitted for unused-variables (the temporary shim in Step 2a's case triggers one).

- [ ] **Step 4: Run all tests**

Run: `cargo test -p reverie_agent`

Expected: 17/17 pass.

- [ ] **Step 5: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: replace stream_to_string with cancellable variant

stream_to_string_cancellable races both the initial
stream_completion_text await and each subsequent stream.next() chunk
against a smol::channel::Receiver<()>. When cancel fires on either
await, the work future drops (which tears down the HTTP transport
via AsyncBody's Drop impl) and the helper returns Err with a
cancel-specific message.

Task 4 wires the session's per-prompt cancel_rx into the call site.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Wire per-prompt cancel channel through `Session`, `cancel()`, and `prompt()`

**Files:**
- Modify: `crates/reverie_agent/src/connection.rs`

- [ ] **Step 1: Add `cancel_notify` to the `Session` struct**

In `crates/reverie_agent/src/connection.rs`, find:

```rust
struct Session {
    thread: WeakEntity<AcpThread>,
    cancel: Arc<AtomicBool>,
    state: Arc<Mutex<SessionState>>,
}
```

Replace with:

```rust
struct Session {
    thread: WeakEntity<AcpThread>,
    cancel: Arc<AtomicBool>,
    state: Arc<Mutex<SessionState>>,
    // Phase 1.5d: holds the current (or most recent) prompt's cancel
    // notifier. cancel() fires try_send(()) on it so any pending await
    // in the foreground driver wakes immediately instead of after the
    // current LLM chunk resolves.
    cancel_notify: Arc<Mutex<Option<smol::channel::Sender<()>>>>,
}
```

- [ ] **Step 2: Initialize `cancel_notify` in `new_session`**

In `new_session`'s body, find the session insertion:

```rust
        self.sessions.lock().insert(
            session_id,
            Session {
                thread: thread.downgrade(),
                cancel: Arc::new(AtomicBool::new(false)),
                state: session_state,
            },
        );
```

Replace with:

```rust
        self.sessions.lock().insert(
            session_id,
            Session {
                thread: thread.downgrade(),
                cancel: Arc::new(AtomicBool::new(false)),
                state: session_state,
                cancel_notify: Arc::new(Mutex::new(None)),
            },
        );
```

- [ ] **Step 3: Update `cancel()` to fire the notifier**

In the same file, find:

```rust
    fn cancel(&self, session_id: &acp::SessionId, _cx: &mut App) {
        if let Some(session) = self.sessions.lock().get(session_id) {
            session
                .cancel
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
```

Replace with:

```rust
    fn cancel(&self, session_id: &acp::SessionId, _cx: &mut App) {
        if let Some(session) = self.sessions.lock().get(session_id) {
            session
                .cancel
                .store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(tx) = session.cancel_notify.lock().as_ref() {
                // Full → cancel already pending; Closed → driver already
                // torn down. Both are safe no-ops.
                let _ = tx.try_send(());
            }
        }
    }
```

- [ ] **Step 4: Install per-prompt cancel channel in `prompt()`**

In `prompt()`, find:

```rust
        let (thread_weak, cancel, state) = {
            let sessions = self.sessions.lock();
            match sessions.get(&session_id) {
                Some(s) => (s.thread.clone(), s.cancel.clone(), s.state.clone()),
                None => {
                    return Task::ready(Err(anyhow!(
                        "unknown session {:?}",
                        session_id.0.as_ref()
                    )));
                }
            }
        };
```

Replace with:

```rust
        let (thread_weak, cancel, state, cancel_notify) = {
            let sessions = self.sessions.lock();
            match sessions.get(&session_id) {
                Some(s) => (
                    s.thread.clone(),
                    s.cancel.clone(),
                    s.state.clone(),
                    s.cancel_notify.clone(),
                ),
                None => {
                    return Task::ready(Err(anyhow!(
                        "unknown session {:?}",
                        session_id.0.as_ref()
                    )));
                }
            }
        };

        // Phase 1.5d: install a fresh cancel channel for this prompt. Order:
        // 1. Install cancel_tx into session (cancel() can now reach it).
        // 2. Clear the AtomicBool so a cancel from the PRIOR prompt doesn't
        //    immediately kill this new run.
        // A cancel arriving between (1) and (2) lands on the new cancel_rx
        // (correct — we'll honour it). A cancel arriving BEFORE (1) is lost
        // unless it flipped the bool, in which case (2) wipes it — this is
        // a known sub-millisecond race documented in the Phase 1.5d spec.
        let (cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);
        *cancel_notify.lock() = Some(cancel_tx);
        cancel.store(false, std::sync::atomic::Ordering::SeqCst);
```

- [ ] **Step 5: Capture `cancel_rx` into the spawn closure and wire into the driver loop**

Still inside `prompt()`, find the `cx.spawn(async move |cx| { ... })` block. The current driver loop is:

```rust
            while let Ok(req) = req_rx.recv().await {
                let request = build_language_model_request(req.messages);
                let text_result = stream_to_string(&model, request, cx).await;
                // ...or if Task 3 Step 2 left a shim:
                // let text_result = stream_to_string_cancellable(&model, request, cx, &never_rx).await;
                let reply_payload = match text_result {
                    Ok(text) => Ok(text),
                    Err(e) => Err(e.to_string()),
                };
                if req.reply.send(reply_payload).is_err() {
                    log::warn!(
                        "reverie planner dropped its reply channel while the driver still held a request"
                    );
                }
            }
```

Replace the inner call site with the real `cancel_rx`:

```rust
            while let Ok(req) = req_rx.recv().await {
                let request = build_language_model_request(req.messages);
                let text_result =
                    stream_to_string_cancellable(&model, request, cx, &cancel_rx).await;
                let reply_payload = match text_result {
                    Ok(text) => Ok(text),
                    Err(e) => Err(e.to_string()),
                };
                if req.reply.send(reply_payload).is_err() {
                    log::warn!(
                        "reverie planner dropped its reply channel while the driver still held a request"
                    );
                }
            }
```

The `cancel_rx` variable must already be captured by the outer `cx.spawn(async move |cx| { ... })` because it's referenced inside. Rust's `move` closure captures it automatically; we don't need an explicit bind.

Also remove the temporary `let (_never_tx, never_rx) = ...` shim from Task 3 Step 2 if you added one.

- [ ] **Step 6: Verify compile**

Run: `cargo check -p reverie_agent`

Expected: clean compile, no new warnings.

- [ ] **Step 7: Run all tests**

Run: `cargo test -p reverie_agent`

Expected: 17/17 pass.

- [ ] **Step 8: Verify zed still builds**

Run: `cargo check -p agent_ui`

Expected: clean compile.

- [ ] **Step 9: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: wire per-prompt cancel channel through session + prompt()

Session gains a cancel_notify: Arc<Mutex<Option<Sender<()>>>> field.
cancel() fires try_send(()) on it (in addition to flipping the
AtomicBool). prompt() installs a fresh bounded(1) channel on each
invocation, clears the bool to avoid honouring a stale cancel from a
prior prompt, and passes the receiver into the driver loop's
stream_to_string_cancellable call.

Net effect: clicking Cancel during an in-flight LLM stream returns
from the chunk-level await within milliseconds instead of waiting
for the current turn to finish. The planner's subsequent
next_action call sees BackendError::Transport("cancelled ..."),
and reverie's Err arm (Task 1) reports TerminationReason::Cancelled
because observer.should_stop() is true from the AtomicBool.

Known small race (spec §3.4): a cancel() that arrives AFTER the old
session's cancel_tx is dropped but BEFORE the new one is installed
is silently discarded. Sub-millisecond window.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Update `docs/reverie-agent.md`

**Files:**
- Modify: `docs/reverie-agent.md`

- [ ] **Step 1: Update the "Canceling a run" section**

In `docs/reverie-agent.md`, find the existing "## Canceling a run" section:

```markdown
## Canceling a run

Clicking **Stop** in the agent panel flips a per-session `AtomicBool` that the planner checks at the top of every iteration. The run terminates with `TerminationReason::Cancelled` on the next loop top. In-flight LLM calls continue until their `stream_completion_text` future resolves; cancellation granularity is per-iteration, not mid-call.
```

Replace with:

```markdown
## Canceling a run

Clicking **Stop** in the agent panel interrupts the run within milliseconds, including while an LLM call is streaming. Internally: a per-session `AtomicBool` is flipped (so the planner's iteration-top `should_stop` check catches it) AND a per-prompt `smol::channel` notifier fires so any pending await in the foreground driver (the `stream_completion_text` call, or a `stream.next()` chunk await) wakes immediately. The in-flight HTTP request is torn down via async drop; the planner's next call to the backend sees a transport error, and because `should_stop` is true at that moment, the run terminates with `TerminationReason::Cancelled` (not `Backend`).

Partial state is preserved (Phase 1.5c): whatever the `TodoList` had accumulated before cancel stays, and the scratch `Vfs` is untouched. Send a follow-up prompt in the same thread to continue.

### Subagent limit

The cancel notifier is installed only for the top-level prompt's foreground driver. A subagent's own nested LLM calls aren't interrupted mid-stream — the parent-level `should_stop` catches them at the next planner iteration boundary. Deep cancellation through subagents is future work.
```

- [ ] **Step 2: Update the "Known limitations" section**

In the same file, find the "## Known limitations" section. Delete the bullet:

```markdown
- **Cancel is coarse.** It stops the *next* iteration, not the in-flight LLM call.
```

Replace it with:

```markdown
- **Cancel interrupts the top-level run but not nested subagent streams.** Subagent LLM calls finish their current chunk before noticing cancellation. See "Canceling a run" above.
```

- [ ] **Step 3: Verify the doc looks right**

Run: `sed -n '/^## Canceling a run/,/^## /p' docs/reverie-agent.md | head -20`

Expected: the new "Canceling a run" section appears with the milliseconds wording and the subagent note.

- [ ] **Step 4: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add docs/reverie-agent.md
git commit -m "$(cat <<'EOF'
docs(reverie-agent): Phase 1.5d mid-call cancel responsiveness

Rewrites "Canceling a run" to reflect the new mid-await cancellation
behaviour: both the AtomicBool and a per-prompt smol::channel notifier
fire on cancel, so in-flight streams drop within milliseconds and the
run terminates with TerminationReason::Cancelled (not Backend).

Flags the subagent-streaming limit inline and in the Known
limitations bullet.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**1. Spec coverage:**

| Spec section | Tasks |
|---|---|
| §1 Architecture — per-prompt cancel notifier + reverie Err-arm tweak | Tasks 1, 4 |
| §2.1 Session struct with `cancel_notify` field | Task 4 |
| §2.2 new_session init | Task 4 |
| §2.3 `cancel()` fires try_send | Task 4 |
| §2.4 `prompt()` installs per-prompt channel, clears bool | Task 4 |
| §2.5 `stream_to_string_cancellable` helper | Task 3 |
| §2.6 Reverie Err-arm checks `should_stop` | Task 1 |
| §3.1–3.6 Error handling flows | Implementation in Tasks 3 & 4; behaviour described in Task 4 Step 4 comment |
| §4.1 Reverie test | Task 1 |
| §4.2 Dreamcode tests via `race_with_cancel` | Task 2 |
| §5 Known limitations | Task 5 (docs) |

**2. Placeholder scan:** No "TBD", "add error handling", "similar to", or unattached "write tests" language. Every edit shows the before/after text.

**3. Type consistency:**
- `Session.cancel_notify: Arc<Mutex<Option<smol::channel::Sender<()>>>>` consistent across Tasks 4 Step 1, Step 2, Step 3, Step 4.
- `race_with_cancel<T, F>(work: F, cancel_rx: &Receiver<()>, cancel_err: &'static str) -> Result<T>` consistent across Task 2 Step 3 (def) and Task 3 Step 1 (use).
- `stream_to_string_cancellable(&Arc<dyn LanguageModel>, LanguageModelRequest, &AsyncApp, &Receiver<()>) -> Result<String>` consistent across Task 3 Step 1 (def) and Task 4 Step 5 (use).
- `cancel_notify.clone()` at the caller (Task 4 Step 4) matches `Arc<_>`'s Clone.
- Reverie's `TerminationReason::Cancelled` (defined before Phase 1.5d) is reused, not added anew.

**4. Spec-to-plan drift notes (inline fixes):**
- Spec §3.3 "Cancel between prompts" specifies the ordering (install cancel_tx FIRST, then clear bool). Task 4 Step 4 spells this out in the exact order with a code comment — matches.
- Spec §4.2 suggested extracting `race_with_cancel` as a helper. Task 2 does this explicitly; `stream_to_string_cancellable` (Task 3) is a thin wrapper on top.
- The `cancel_rx` is captured BY REFERENCE in the driver loop (each iteration passes `&cancel_rx`); `smol::channel::Receiver::recv()` takes `&self`, so no cloning is required. Matches spec §2.4.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-21-phase-1-5d-mid-call-cancel.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
