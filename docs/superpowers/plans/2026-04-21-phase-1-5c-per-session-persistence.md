# Phase 1.5c — Per-Session Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist `Run + TodoList + scratch Vfs` across prompts within one `AcpThread` session, so the Reverie agent's planner state survives between prompts in the same thread. Shared workspace (fresh LLM transcript per prompt, persistent planner state).

**Architecture:** Move `Run` and `TodoList` ownership out of `prompt()`'s `smol::unblock` closure into the connection's `Session.state: Arc<Mutex<SessionState>>`. `new_session` constructs a stable scratch dir rooted at `paths::data_dir()/reverie-runs/<session_id>/`. `prompt()` acquires the run slot via a short lock, rejects concurrent prompts in the same session, runs the planner against the existing `Arc<Run>` with the current `TodoList` as `initial_todos`, and writes the planner's returned `TodoList` back to the session. An `InProgressGuard` RAII type clears the in-progress flag on scope exit including panics.

**Tech Stack:** Rust, `parking_lot::Mutex` (already a workspace dep), Zed's `paths` crate, reverie-deepagent's `Run/Vfs/TodoList` types with pub-field construction.

**Spec reference:** `docs/superpowers/specs/2026-04-21-phase-1-5c-per-session-persistence-design.md`.

**Working directory:** `/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/` on branch `feat/reverie-agent-backend`. The reverie repo at `/Users/dennis/programming projects/reverie` is on branch `feat/planner-observer`; this plan adds one more commit there.

---

## File Structure

**Modified files (reverie repo):**
- `crates/reverie-deepagent/src/planner.rs` — add `run_planner_with_observer_and_todos`; existing `run_planner_with_observer` delegates. Add 2 unit tests.
- `crates/reverie-deepagent/src/lib.rs` — re-export `run_planner_with_observer_and_todos`.

**Modified files (dreamcode worktree):**
- `crates/reverie_agent/Cargo.toml` — add `paths.workspace = true`.
- `crates/reverie_agent/src/connection.rs` — `SessionState` struct, `InProgressGuard`, `acquire_run_slot` helper, rewritten `new_session` (stable scratch dir), rewritten `prompt()` (state acquire + initial_todos + put-back).
- `crates/reverie_agent/src/tests.rs` — add 4 unit tests targeting `acquire_run_slot` + `InProgressGuard`.
- `docs/reverie-agent.md` — add "Persistent session state" section; adjust the limitations list.

---

## Task 1: Reverie upstream — `run_planner_with_observer_and_todos` (TDD)

**Files (reverie repo):**
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/planner.rs`
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/lib.rs`

- [ ] **Step 1: Write a failing test for the seeded-todos entry point**

Append to the `#[cfg(test)] mod tests` block in `planner.rs`, after the existing `observer_receives_spawn_completion` test:

```rust
#[test]
fn seeded_initial_todos_visible_on_first_iteration() {
    use std::sync::{Arc, Mutex};

    struct RecordingBackend {
        seen: Arc<Mutex<Vec<usize>>>,
    }
    impl LlmBackend for RecordingBackend {
        fn next_action(
            &mut self,
            todos: &TodoList,
            _vfs: &Vfs,
            _observations: &[SpawnObservation],
        ) -> Result<NextAction, BackendError> {
            self.seen.lock().unwrap().push(todos.entries().len());
            Ok(NextAction::SetStatus(1, TodoStatus::Completed))
        }
    }

    let (_p, run) = fresh_run();
    let seen = Arc::new(Mutex::new(Vec::<usize>::new()));
    let mut backend = RecordingBackend { seen: seen.clone() };

    let mut seeded = TodoList::new();
    seeded.add("alpha".into());
    seeded.add("beta".into());

    let _ = run_planner_with_observer_and_todos(
        &run,
        &mut backend,
        5,
        &SpawnConfig::default(),
        &NoopObserver,
        seeded,
    );

    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.first().copied(),
        Some(2),
        "backend's first next_action must see the seeded 2-entry todo list, got {seen:?}"
    );
}
```

- [ ] **Step 2: Write a parity test with empty seed**

Append in the same module:

```rust
#[test]
fn empty_seed_matches_legacy_entry_point() {
    // Script a short productive run and run it through both entry points;
    // assert PlannerResult fields match on every axis the planner exposes.
    let actions = || {
        vec![
            NextAction::AddTodo("one".into()),
            NextAction::SetStatus(1, TodoStatus::Completed),
            NextAction::NoOp,
        ]
    };
    let (_p1, r1) = fresh_run();
    let mut b1 = MockLlmBackend::new(actions());
    let legacy = run_planner_with_observer(
        &r1,
        &mut b1,
        10,
        &SpawnConfig::default(),
        &NoopObserver,
    );

    let (_p2, r2) = fresh_run();
    let mut b2 = MockLlmBackend::new(actions());
    let seeded_empty = run_planner_with_observer_and_todos(
        &r2,
        &mut b2,
        10,
        &SpawnConfig::default(),
        &NoopObserver,
        TodoList::new(),
    );

    assert_eq!(legacy.termination, seeded_empty.termination);
    assert_eq!(legacy.iterations, seeded_empty.iterations);
    assert_eq!(legacy.todos.entries().len(), seeded_empty.todos.entries().len());
    assert_eq!(legacy.spawn_log.len(), seeded_empty.spawn_log.len());
}
```

- [ ] **Step 3: Run the tests; verify they fail**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent seeded_initial_todos_visible_on_first_iteration empty_seed_matches_legacy_entry_point`

Expected: FAIL — `run_planner_with_observer_and_todos` doesn't exist yet.

- [ ] **Step 4: Implement the new entry point**

In `crates/reverie-deepagent/src/planner.rs`, find the body of `run_planner_with_observer` (currently the canonical loop). Rename it internally so we can add the seeded variant without duplicating the loop. Structure:

Replace the current `pub fn run_planner_with_observer(...) -> PlannerResult { ... big body ... }` with:

```rust
pub fn run_planner_with_observer(
    run: &Run,
    backend: &mut dyn LlmBackend,
    max_iterations: u32,
    spawn_cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
) -> PlannerResult {
    run_planner_with_observer_and_todos(
        run,
        backend,
        max_iterations,
        spawn_cfg,
        observer,
        TodoList::new(),
    )
}

pub fn run_planner_with_observer_and_todos(
    run: &Run,
    backend: &mut dyn LlmBackend,
    max_iterations: u32,
    spawn_cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
    initial_todos: TodoList,
) -> PlannerResult {
    // --- paste the entire existing body of run_planner_with_observer here ---
    // Then change the initializer:
    //     let mut todos = TodoList::new();
    // to:
    //     let mut todos = initial_todos;
    // Nothing else in the loop body changes — `todos` is already mutably
    // threaded through every match arm.
}
```

The mechanical move: open `planner.rs`, locate the `let mut todos = TodoList::new();` line near the top of `run_planner_with_observer`'s body, and transplant the entire function body under the new `_and_todos` signature with the one initializer change.

- [ ] **Step 5: Run the tests; verify they pass**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent`

Expected: all 76 pre-existing tests still pass, plus the 2 new tests pass. Total 78/78.

If `empty_seed_matches_legacy_entry_point` fails on `termination` equality, the most likely cause is that the legacy path had a subtle side-effect at entry (e.g. a log line) that doesn't reproduce. Re-read the diff: the ONLY intended behavioral change is the todos initializer.

- [ ] **Step 6: Re-export from `lib.rs`**

In `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/lib.rs`, find the existing `pub use planner::{...}` block. Add `run_planner_with_observer_and_todos` to the identifier list. Keep alphabetical order among the `run_*` functions.

Before:
```rust
pub use planner::{
    BackendError, LlmBackend, MockLlmBackend, NextAction, NoopObserver, PlannerObserver,
    PlannerResult, SpawnObservation, TerminationReason, run_planner, run_planner_with_ctx,
    run_planner_with_observer,
};
```

After:
```rust
pub use planner::{
    BackendError, LlmBackend, MockLlmBackend, NextAction, NoopObserver, PlannerObserver,
    PlannerResult, SpawnObservation, TerminationReason, run_planner, run_planner_with_ctx,
    run_planner_with_observer, run_planner_with_observer_and_todos,
};
```

- [ ] **Step 7: Run the full test suite one more time**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent`

Expected: 78/78 pass.

- [ ] **Step 8: Commit**

```bash
cd "/Users/dennis/programming projects/reverie"
git add crates/reverie-deepagent/src/planner.rs crates/reverie-deepagent/src/lib.rs
git commit -m "$(cat <<'EOF'
deepagent: add run_planner_with_observer_and_todos entry point

Allows callers to seed the planner's TodoList instead of always
starting fresh. The existing run_planner_with_observer delegates to
the new function with TodoList::new(), so every current caller is
unchanged.

Enables the reverie_agent frontend (dreamcode) to carry TodoList
state across prompts within an AcpThread session — shared-workspace
persistence per the Phase 1.5c spec.

Tests: seeded_initial_todos_visible_on_first_iteration verifies the
backend's first next_action sees the seeded list; empty_seed_matches_
legacy_entry_point asserts termination/iterations/todos/spawn_log
parity with the legacy entry point when the seed is empty.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 9: Verify dreamcode picks up the change**

Run: `cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend" && cargo check -p reverie_agent`

Expected: PASS. Cargo's path dep will have picked up the new symbol as an available import. (Nothing currently *uses* the new symbol from dreamcode — Task 4 does that — but the re-export must be visible.)

---

## Task 2: Introduce `SessionState`, `InProgressGuard`, and `acquire_run_slot` (TDD, dreamcode)

**Files:**
- Modify: `crates/reverie_agent/src/connection.rs`
- Modify: `crates/reverie_agent/src/tests.rs`

- [ ] **Step 1: Write failing tests for the lock semantics**

Append to `crates/reverie_agent/src/tests.rs` (after the `http_tests` module, at the end of the file):

```rust
mod session_slot_tests {
    use crate::connection::{InProgressGuard, SessionState, acquire_run_slot};
    use parking_lot::Mutex;
    use reverie_deepagent::{Run, TodoList};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn fresh_state() -> Arc<Mutex<SessionState>> {
        let parent = TempDir::new().unwrap();
        // Leak the TempDir on purpose — the Run's scratch_root needs to outlive
        // this helper. Tests get a fresh process each `cargo test` invocation.
        let parent_path = parent.keep();
        let root = parent_path.join("session-test");
        std::fs::create_dir_all(&root).unwrap();
        let vfs = reverie_deepagent::Vfs::new(&root).unwrap();
        let run = Run {
            id: "session-test".into(),
            scratch_root: root,
            vfs,
            depth: 0,
        };
        Arc::new(Mutex::new(SessionState {
            run: Arc::new(run),
            todos: TodoList::new(),
            in_progress: false,
        }))
    }

    #[test]
    fn rejects_when_in_progress() {
        let state = fresh_state();
        state.lock().in_progress = true;
        let result = acquire_run_slot(&state);
        assert!(result.is_err(), "should reject when in_progress");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("already in progress"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn returns_current_todos_snapshot() {
        let state = fresh_state();
        state.lock().todos.add("alpha".into());

        let (_run, initial, _guard) = acquire_run_slot(&state).unwrap();
        assert_eq!(initial.entries().len(), 1);
        assert_eq!(initial.entries()[0].description, "alpha");

        // Mutating state.todos after acquire must NOT reflect in the snapshot.
        state.lock().todos.add("beta".into());
        assert_eq!(initial.entries().len(), 1, "snapshot should be a clone");
    }

    #[test]
    fn guard_clears_in_progress_on_drop() {
        let state = fresh_state();
        let (_run, _todos, guard) = acquire_run_slot(&state).unwrap();
        assert!(state.lock().in_progress, "acquire sets in_progress");
        drop(guard);
        assert!(
            !state.lock().in_progress,
            "dropping the guard must clear in_progress"
        );
    }

    #[test]
    fn guard_clears_in_progress_on_panic() {
        let state = fresh_state();
        let state_for_panic = state.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (_run, _todos, _guard) = acquire_run_slot(&state_for_panic).unwrap();
            panic!("simulated failure while holding the slot");
        }));
        assert!(result.is_err(), "the panic should propagate out of catch_unwind");
        assert!(
            !state.lock().in_progress,
            "in_progress must be cleared even when the guard is dropped via panic"
        );
    }
}
```

Note on `TempDir::keep()`: tempfile's `keep()` returns a `PathBuf` and leaks the directory. Since the test process is short-lived, this is fine; we don't need to clean up.

Also note: `TodoList::add(&mut self, desc: String)` — a `&mut self` method returning the new id (u32). The tests use it via `state.lock().todos.add(...)`; the return value is unused.

- [ ] **Step 2: Run the tests; verify they fail to compile**

Run: `cargo test -p reverie_agent session_slot_tests`

Expected: FAIL with compile errors: `SessionState`, `InProgressGuard`, `acquire_run_slot` not in `crate::connection`.

- [ ] **Step 3: Add `SessionState`, `InProgressGuard`, and `acquire_run_slot` to `connection.rs`**

In `crates/reverie_agent/src/connection.rs`, find the existing `struct Session` block. Replace it with:

```rust
struct Session {
    thread: WeakEntity<AcpThread>,
    cancel: Arc<AtomicBool>,
    state: Arc<parking_lot::Mutex<SessionState>>,
}

pub(crate) struct SessionState {
    pub(crate) run: Arc<reverie_deepagent::Run>,
    pub(crate) todos: reverie_deepagent::TodoList,
    pub(crate) in_progress: bool,
}

/// RAII guard that clears `in_progress` when dropped, including on panic
/// unwind. Returned by `acquire_run_slot` so the caller's normal control
/// flow (success, early return, or panic) all converge on "slot released".
pub(crate) struct InProgressGuard {
    state: Arc<parking_lot::Mutex<SessionState>>,
}

impl Drop for InProgressGuard {
    fn drop(&mut self) {
        self.state.lock().in_progress = false;
    }
}

pub(crate) fn acquire_run_slot(
    state: &Arc<parking_lot::Mutex<SessionState>>,
) -> Result<(
    Arc<reverie_deepagent::Run>,
    reverie_deepagent::TodoList,
    InProgressGuard,
)> {
    let mut st = state.lock();
    if st.in_progress {
        return Err(anyhow!(
            "a run is already in progress for this session; cancel it first"
        ));
    }
    st.in_progress = true;
    let run = st.run.clone();
    let initial_todos = st.todos.clone();
    Ok((
        run,
        initial_todos,
        InProgressGuard {
            state: state.clone(),
        },
    ))
}
```

Note: `pub(crate)` visibility on `SessionState` and its fields is deliberate — the test module is a child of the crate, so it needs crate-level visibility. External users of `reverie_agent` do NOT see these types.

You will also need to fix the existing `new_session` signature usage — it currently constructs the old `Session { thread, cancel }` struct. For this task we'll leave `new_session` alone; Task 3 rewrites it. Instead, temporarily initialize the `state` field with a placeholder so the crate still compiles:

In `new_session()`, right before the `self.sessions.lock().insert(...)` call, construct:

```rust
let placeholder_state = {
    // Task 3 replaces this with a stable-scratch-dir Run. For now it's
    // the same behaviour as Phase 1 / 1.5a: Run::new_default per session.
    let run = Arc::new(reverie_deepagent::Run::new_default()
        .map_err(|e| anyhow::anyhow!("Run::new_default failed: {e}"))?);
    Arc::new(parking_lot::Mutex::new(SessionState {
        run,
        todos: reverie_deepagent::TodoList::new(),
        in_progress: false,
    }))
};
```

And the insertion becomes:

```rust
self.sessions.lock().insert(
    session_id,
    Session {
        thread: thread.downgrade(),
        cancel: Arc::new(AtomicBool::new(false)),
        state: placeholder_state,
    },
);
```

But the current `new_session` returns `Task<Result<Entity<AcpThread>>>` synchronously. Wrapping the `Run::new_default` error in the method body requires either propagating the `?` or returning a ready-task. Use:

```rust
let run_result = reverie_deepagent::Run::new_default();
let run = match run_result {
    Ok(r) => Arc::new(r),
    Err(e) => {
        return Task::ready(Err(anyhow::anyhow!(
            "Run::new_default failed: {e}"
        )));
    }
};
let placeholder_state = Arc::new(parking_lot::Mutex::new(SessionState {
    run,
    todos: reverie_deepagent::TodoList::new(),
    in_progress: false,
}));
```

Place this block right before the `let action_log = ...` line. Then replace the insertion as shown above.

- [ ] **Step 4: Run the compile check**

Run: `cargo check -p reverie_agent`

Expected: compiles with warnings (unused `cancel` field on `Session` is inherited; no new warnings from this task). The `prompt()` body still references the old `state`-less `Session` struct indirectly, so if compile errors mention `s.state` not existing, we need to verify the snapshot we took of the sessions map is of the new field. Re-read `prompt()`'s session lookup:

```rust
let (thread_weak, cancel) = {
    let sessions = self.sessions.lock();
    match sessions.get(&session_id) {
        Some(s) => (s.thread.clone(), s.cancel.clone()),
        None => { return Task::ready(Err(anyhow!(...))); }
    }
};
```

That still compiles — we added a `state` field to `Session` but didn't remove anything. Good.

- [ ] **Step 5: Run the tests; verify the 4 new ones pass**

Run: `cargo test -p reverie_agent session_slot_tests`

Expected:
```
test session_slot_tests::rejects_when_in_progress ... ok
test session_slot_tests::returns_current_todos_snapshot ... ok
test session_slot_tests::guard_clears_in_progress_on_drop ... ok
test session_slot_tests::guard_clears_in_progress_on_panic ... ok
```

Also run the full test suite to make sure nothing else regressed:

Run: `cargo test -p reverie_agent`

Expected: 15/15 pass (11 pre-existing + 4 new).

- [ ] **Step 6: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: add SessionState + InProgressGuard + acquire_run_slot

Introduces the shape that Task 3 and Task 4 will populate and use.
SessionState bundles the persistent Arc<Run> + TodoList + in_progress
flag; InProgressGuard clears in_progress on drop including panic
unwind; acquire_run_slot takes the state under lock, rejects when a
run is already in flight, and returns cloned Run + TodoList + guard.

new_session wires in the new field with a placeholder that still uses
Run::new_default (same behaviour as Phase 1). Task 3 swaps it for a
stable scratch dir.

Four pure-Rust unit tests cover reject-when-in-progress, clone-not-
reference snapshot, Drop clears, and catch_unwind clears.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Stable scratch dir for the Run in `new_session`

**Files:**
- Modify: `crates/reverie_agent/Cargo.toml`
- Modify: `crates/reverie_agent/src/connection.rs`

- [ ] **Step 1: Verify `paths` is a workspace dep**

Run: `grep -E "^paths = " /Users/dennis/programming\ projects/dreamcode/.worktrees/reverie-agent-backend/Cargo.toml`

Expected: a single line `paths = { path = "crates/paths" }` or similar. This confirms `paths.workspace = true` will resolve.

- [ ] **Step 2: Add `paths` to `reverie_agent/Cargo.toml`**

Read `crates/reverie_agent/Cargo.toml` first. In the `[dependencies]` block, add after the existing `parking_lot.workspace = true`:

```toml
paths.workspace = true
```

Keep alphabetical order with neighbouring entries.

- [ ] **Step 3: Rewrite the placeholder in `new_session` to use a stable scratch dir**

In `crates/reverie_agent/src/connection.rs`, replace the placeholder block added in Task 2 Step 3. Find:

```rust
let run_result = reverie_deepagent::Run::new_default();
let run = match run_result {
    Ok(r) => Arc::new(r),
    Err(e) => {
        return Task::ready(Err(anyhow::anyhow!(
            "Run::new_default failed: {e}"
        )));
    }
};
```

Replace with:

```rust
let scratch_root = paths::data_dir()
    .join("reverie-runs")
    .join(session_id.0.as_ref());
let run = match build_persistent_run(&scratch_root, &session_id) {
    Ok(r) => Arc::new(r),
    Err(e) => {
        log::warn!(
            "reverie: failed to create stable scratch dir at {}: {e}. Falling back to ephemeral Run.",
            scratch_root.display()
        );
        match reverie_deepagent::Run::new_default() {
            Ok(r) => Arc::new(r),
            Err(fallback_err) => {
                return Task::ready(Err(anyhow::anyhow!(
                    "reverie: even temp-dir fallback failed: {fallback_err}"
                )));
            }
        }
    }
};
```

At the bottom of the file, or above `fn user_text_from_prompt`, add:

```rust
fn build_persistent_run(
    scratch_root: &std::path::Path,
    session_id: &acp::SessionId,
) -> Result<reverie_deepagent::Run> {
    std::fs::create_dir_all(scratch_root)
        .with_context(|| format!("mkdir {}", scratch_root.display()))?;
    let vfs = reverie_deepagent::Vfs::new(scratch_root)
        .map_err(|e| anyhow!("Vfs::new failed: {e}"))?;
    Ok(reverie_deepagent::Run {
        id: session_id.0.as_ref().to_string(),
        scratch_root: scratch_root.to_path_buf(),
        vfs,
        depth: 0,
    })
}
```

The `Context` import (from `anyhow::Context`) is already at the top of `connection.rs` (used by `.context("reverie planner thread failed")?` later). No new imports beyond `std::fs` and `std::path::Path`, both already brought in elsewhere — but verify by running cargo check.

- [ ] **Step 4: Verify compile**

Run: `cargo check -p reverie_agent`

Expected: clean compile, warnings only. If you see `unresolved import std::fs`, add `use std::fs;` near the other `std` imports at the top of `connection.rs`.

- [ ] **Step 5: Run existing tests**

Run: `cargo test -p reverie_agent`

Expected: 15/15 still pass. The `session_slot_tests` use their own local `fresh_state()` helper that doesn't touch `new_session`, so they're isolated from this change.

- [ ] **Step 6: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: stable scratch dir for session Runs

new_session now constructs the Run at
<paths::data_dir()>/reverie-runs/<session_id>/ instead of a fresh
temp dir per prompt. Direct field construction (Run fields are all
pub) avoids needing a new Run constructor upstream. On filesystem
failure, logs a warn and falls back to Run::new_default so prompts
still work — session state is just not durable in that case.

Unblocks Task 4's cross-prompt reuse of the same Arc<Run>: because
the scratch dir is keyed by session_id rather than a per-prompt
UUID, the same Run can be handed to successive planner invocations
without the vfs paths drifting.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Wire `acquire_run_slot` + `initial_todos` + put-back into `prompt()`

**Files:**
- Modify: `crates/reverie_agent/src/connection.rs`

- [ ] **Step 1: Replace the prompt() body's state-acquisition code**

In `crates/reverie_agent/src/connection.rs`, find the existing top of `prompt()`:

```rust
let (thread_weak, cancel) = {
    let sessions = self.sessions.lock();
    match sessions.get(&session_id) {
        Some(s) => (s.thread.clone(), s.cancel.clone()),
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

let (run, initial_todos, _guard) = match acquire_run_slot(&state) {
    Ok(x) => x,
    Err(e) => return Task::ready(Err(e)),
};
```

- [ ] **Step 2: Change the `smol::unblock` closure to use the acquired `run` and `initial_todos`**

Inside the `cx.spawn(async move |cx| { ... })` block in `prompt()`, find the current `planner_task` block:

```rust
let cancel_for_planner = cancel.clone();
let user_text_for_planner = user_text.clone();
let planner_task = smol::unblock(move || -> Result<reverie_deepagent::PlannerResult> {
    let mut backend = ZedLlmBackend::new(req_tx);
    // The user's prompt is bolted onto the system transcript as an
    // extra user turn so the model sees intent on iteration 1.
    // Run::new_default creates a fresh scratch dir per prompt —
    // Phase 1 has no cross-prompt persistence.
    backend.seed_user_message(&user_text_for_planner);

    let observer = ChannelObserver::new(event_tx, cancel_for_planner);
    let run =
        Run::new_default().map_err(|e| anyhow!("vfs init failed: {e}"))?;
    Ok(run_planner_with_observer(
        &run,
        &mut backend,
        DEFAULT_MAX_ITERATIONS,
        &SpawnConfig::default(),
        &observer,
    ))
});
```

Replace with:

```rust
let cancel_for_planner = cancel.clone();
let user_text_for_planner = user_text.clone();
let run_for_planner = run.clone();
let initial_todos_for_planner = initial_todos;
let planner_task = smol::unblock(move || -> Result<reverie_deepagent::PlannerResult> {
    let mut backend = ZedLlmBackend::new(req_tx);
    // Shared-workspace persistence: the LLM gets a fresh transcript per
    // prompt; the Run/Vfs are the same across prompts in this session
    // (see Phase 1.5c spec); the TodoList seed is whatever the last
    // planner run ended with.
    backend.seed_user_message(&user_text_for_planner);

    let observer = ChannelObserver::new(event_tx, cancel_for_planner);
    Ok(run_planner_with_observer_and_todos(
        &run_for_planner,
        &mut backend,
        DEFAULT_MAX_ITERATIONS,
        &SpawnConfig::default(),
        &observer,
        initial_todos_for_planner,
    ))
});
```

- [ ] **Step 3: Update the import line**

At the top of `crates/reverie_agent/src/connection.rs`, find:

```rust
use reverie_deepagent::{Run, SpawnConfig, run_planner_with_observer};
```

Replace with:

```rust
use reverie_deepagent::{Run, SpawnConfig, run_planner_with_observer_and_todos};
```

(`run_planner_with_observer` is no longer used inside this file.)

- [ ] **Step 4: Put the new TodoList back into session state after the planner finishes**

Further down in `prompt()`, after `event_drain.await;` and before the `let summary = format!(...)` line, insert:

```rust
// Phase 1.5c: carry planner's final TodoList back to the session so
// the next prompt can pick up from here. The InProgressGuard in scope
// (bound as `_guard` above) clears in_progress when this closure exits.
{
    let mut st = state.lock();
    st.todos = planner_result.todos.clone();
}
```

Note: `planner_result.todos` is already used in the summary formatting (`planner_result.todos.pending_count()`), so it must stay in scope. `.clone()` is fine — `TodoList: Clone`.

Alternative placement: the clone-then-read-original pattern wastes an allocation. To avoid it, reorder so the summary is computed first (it reads `pending_count` on the todos), then the todos are moved into the session. But the `clone()` of a TodoList with a handful of entries is negligible; preferring clarity.

- [ ] **Step 5: Verify compile**

Run: `cargo check -p reverie_agent`

Expected: clean compile, no new warnings.

- [ ] **Step 6: Run all tests**

Run: `cargo test -p reverie_agent`

Expected: 15/15 pass.

- [ ] **Step 7: Smoke-check that the full Zed dependency graph still compiles**

Run: `cargo check -p agent_ui`

Expected: clean compile, no warnings beyond the existing ones.

- [ ] **Step 8: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add crates/reverie_agent
git commit -m "$(cat <<'EOF'
reverie_agent: carry Run + TodoList across prompts in the same session

Each prompt on the same AcpThread now reuses the Arc<Run> and seeds
the planner with the TodoList the last run ended on. Implementation:

  1. prompt() acquires the session's Arc<Mutex<SessionState>>.
  2. acquire_run_slot rejects if in_progress (concurrent prompt on
     same session), else clones the Run, clones the TodoList as
     initial_todos, returns an InProgressGuard.
  3. The smol::unblock closure receives both by move and calls
     run_planner_with_observer_and_todos (reverie upstream, Task 1).
  4. After event_drain.await, the planner's final TodoList replaces
     state.todos so the next prompt sees it.
  5. The InProgressGuard clears in_progress on scope exit including
     panic unwind, so concurrent-prompt rejection can't get stuck.

LLM transcript still resets each prompt — shared-workspace, not
continuation.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Document the new behaviour in `docs/reverie-agent.md`

**Files:**
- Modify: `docs/reverie-agent.md`

- [ ] **Step 1: Add a "Persistent session state" section**

In `docs/reverie-agent.md`, after the "## Memory (auto-retrieval)" section and before the "## What you'll see" section, insert:

```markdown
## Persistent session state

Each agent panel thread keeps its own persistent planner state for the lifetime of the Zed session:

- **TodoList carries over.** When you send prompt 2 in the same thread, the planner sees the todos from prompt 1's final state. Pending, in-progress, and completed entries are all preserved. Phrase follow-ups like "keep going" or "update the status on todo 3" and it'll work.
- **Scratch Vfs is stable.** Every session has its own scratch dir at `<zed-data-dir>/reverie-runs/<session_id>/`. Files the planner wrote in prompt 1 (via `vfs_write`) are readable in prompt 2.
- **LLM transcript does NOT carry over.** Each prompt gets a fresh transcript; the LLM is told "here are your current todos" and "here's your scratch" via reverie's state-rendering. This is deliberate — it keeps token cost stable across long threads and matches how the deepagent is designed to operate.

### New thread = fresh state

Click the **+** button in the agent panel to start a new thread. The new thread gets its own `session_id`, its own scratch dir, and an empty TodoList.

### Known Phase 1.5c limitations

- **No cross-Zed-restart persistence.** Closing Zed throws away the in-memory TodoList; the scratch dir stays on disk but nothing points at it anymore. Phase B will serialize session state across restarts.
- **No cleanup of old scratch dirs.** `<zed-data-dir>/reverie-runs/` accumulates a directory per session indefinitely. If you need to reclaim space, delete the directory manually. Phase B adds retention.
- **No concurrent prompts on the same thread.** If you try to send a second prompt while the first is still running, you'll get `"a run is already in progress for this session; cancel it first"`. Use the panel's Cancel button, or start a new thread.
- **No UI to list or resume past sessions.** Each thread's scratch dir is named by `session_id`, which isn't shown in Zed's history view today. Phase C will surface past sessions.
```

- [ ] **Step 2: Update the "Known limitations" section at the bottom**

In the same file, find the "## Known limitations" section. Remove the bullet:

```markdown
- **No persistence between prompts.** Each prompt builds a fresh `Run` with an empty todo list and empty scratch vfs.
```

It's no longer true. Replace it with:

```markdown
- **Session state is in-memory only.** See "Persistent session state" above for what carries across prompts (TodoList + Vfs) and what doesn't (LLM transcript, cross-restart state).
```

- [ ] **Step 3: Verify the doc renders cleanly**

Run: `head -130 docs/reverie-agent.md`

Expected: the new "Persistent session state" section appears between Memory and "What you'll see"; the updated limitations bullet replaces the old "No persistence" one.

- [ ] **Step 4: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add docs/reverie-agent.md
git commit -m "$(cat <<'EOF'
docs(reverie-agent): document Phase 1.5c persistent session state

Adds a "Persistent session state" section covering what carries
across prompts in the same thread (TodoList + scratch Vfs) and what
doesn't (LLM transcript), plus the four Phase 1.5c limitations
(no cross-restart, no scratch cleanup, no concurrent prompts,
no session-resume UI).

Removes the now-outdated "No persistence between prompts" bullet
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
| §1 Architecture (Arc<Run> per session, TodoList carryover, shared-workspace) | Tasks 3, 4 |
| §2.1 SessionState + InProgressGuard struct definitions | Task 2 |
| §2.2 new_session changes (stable scratch dir, build_persistent_run) | Task 3 |
| §2.3 prompt() changes (acquire_run_slot, initial_todos, put-back) | Task 4 |
| §2.4 Reverie upstream `run_planner_with_observer_and_todos` | Task 1 |
| §2.5 `paths.workspace = true` dep | Task 3 |
| §3.1 Concurrent-prompt rejection | Task 2 (acquire_run_slot) |
| §3.2 Cancel preserves partial TodoList | Task 4 (state.todos = planner_result.todos always, regardless of termination) |
| §3.3 Scratch dir creation failure fallback | Task 3 |
| §3.4 Planner-thread panic doesn't lock the session | Task 2 (InProgressGuard::Drop) |
| §3.5 Session-dropped cleanup (deferred to Phase B) | Flagged in Task 5 docs |
| §3.6 TodoList wholesale replacement | Task 4 Step 4 |
| §4.1 Reverie unit tests (2) | Task 1 Steps 1, 2, 5 |
| §4.2 Dreamcode unit tests (4) | Task 2 Step 1 |
| §4.3 Manual smoke steps | Task 5 (docs) |
| §5 Known limitations | Task 5 (docs) |

**2. Placeholder scan:** No "TBD", "TODO", "implement later", or vague error-handling language. Every edit shows exact code.

**3. Type consistency:**
- `SessionState { run: Arc<Run>, todos: TodoList, in_progress: bool }` consistent across Tasks 2, 3, 4.
- `InProgressGuard` consistent across Tasks 2 and 4 (`_guard` binding).
- `acquire_run_slot(&Arc<Mutex<SessionState>>) -> Result<(Arc<Run>, TodoList, InProgressGuard)>` stable across Tasks 2 and 4.
- `build_persistent_run(&Path, &SessionId) -> Result<Run>` introduced in Task 3 Step 3, not referenced by other tasks.
- `run_planner_with_observer_and_todos(&Run, &mut dyn LlmBackend, u32, &SpawnConfig, &dyn PlannerObserver, TodoList) -> PlannerResult` matches between Task 1 (definition) and Task 4 (call site).

**4. Spec-to-plan drift notes (inline fixes):**
- Spec §3.2 says "Partial TodoList state is captured on cancel." Task 4 Step 4 unconditionally writes `state.todos = planner_result.todos` regardless of `planner_result.termination`, which matches the spec — Cancelled runs still return a TodoList (the current state), it's just not further advanced.
- Spec §2.2 shows a `build_persistent_run` helper; Task 3 Step 3 adds it with the same name and signature.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-21-phase-1-5c-per-session-persistence.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
