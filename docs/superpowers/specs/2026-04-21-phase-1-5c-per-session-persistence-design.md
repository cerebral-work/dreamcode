# Phase 1.5c — Per-Session Persistence (shared workspace, in-memory) — Design

**Status:** Draft, pending user review.
**Scope:** Per-AcpThread persistence of `Run + TodoList + scratch Vfs` across prompts, inside the existing `ReverieAgentConnection`. Shared-workspace semantics (fresh LLM transcript per prompt; persistent planner state). No on-disk serialization, no UI history view — those are Phase B and Phase C respectively. Only the Reverie agent benefits.

**Context:** After Phase 1 and Phase 1.5a, every prompt still creates a fresh `Run` with an empty `TodoList` and a brand-new scratch directory, throwing away any planner progress between prompts. Users report feeling like the agent has amnesia within a single thread: prompt 2 can't see the todos the planner generated in prompt 1. This spec targets the smallest useful fix: keep per-thread session state in memory across prompts, shared-workspace style.

**Cross-phase coordination:**
- Complementary to Phase 1.5a's external memory retrieval (that pulls *cross-session* memory from reveried; this keeps *within-session* planner state).
- Non-conflicting with the still-future Phase 1.5b (universal middleware for non-Reverie agents).
- Phase B (disk persistence) is an additive layer: serialize the `SessionState` fields we're introducing here. This spec's choice of stable scratch-dir paths unblocks B without requiring path migration.

---

## 1. Architecture

Move `Run` and `TodoList` ownership out of `prompt()` and into the connection's `Session` entry. Each `AcpThread` gets:
- one persistent `Arc<Run>` (stable scratch dir rooted at Zed's `paths::data_dir()/reverie-runs/<session_id>/`),
- one mutable `TodoList` that carries across prompts,
- one `in_progress` flag so concurrent prompts on the same session are rejected.

The LLM transcript is **not** persisted. Each prompt gets a fresh `ZedLlmBackend`; reverie's existing `render_state_with_observations` tells the LLM "here are your current todos" and "here's what's in your vfs" on turn one. The human user sees prior responses in the agent panel (AcpThread keeps its own render); the LLM sees the planner's structured state. Different audiences, different representations — shared workspace, not continuation.

```
First prompt in a session (session_id = S1):
  new_session → mkdir -p <data_dir>/reverie-runs/S1/
             → Run { id: S1, scratch_root: <...>/S1, vfs: Vfs::new(<...>/S1), depth: 0 }
             → SessionState { run: Arc<Run>, todos: empty, in_progress: false }
  prompt()    → lock(state) → check !in_progress → set in_progress = true
             → clone Arc<Run>, clone TodoList
             → smol::unblock: run_planner_with_observer_and_todos(&run, …, initial_todos: empty)
             → planner_result.todos → state.todos
             → drop InProgressGuard → in_progress = false

Second prompt in same session:
  prompt()    → lock(state) → check !in_progress → set in_progress = true
             → clone Arc<Run>, clone state.todos (now populated)
             → smol::unblock: run_planner_with_observer_and_todos(&run, …, initial_todos: prior todos)
             → planner_result.todos → state.todos (replaced with latest)
             → drop InProgressGuard → in_progress = false
```

One reverie upstream change: `run_planner_with_observer_and_todos(..., initial_todos: TodoList)`. The existing `run_planner_with_observer` delegates with `TodoList::new()`. Non-breaking. Same pattern as the `PlannerObserver` hook landed in Phase 1.5a's prep.

No Run constructor change needed upstream — all fields (`id`, `scratch_root`, `vfs`, `depth`) are already `pub`, so we construct directly in dreamcode: `Run { id: session_id.to_string(), scratch_root, vfs: Vfs::new(&scratch_root)?, depth: 0 }`.

---

## 2. Components & Interfaces

### 2.1 Modified: `crates/reverie_agent/src/connection.rs`

Session struct grows:

```rust
struct Session {
    thread: WeakEntity<AcpThread>,
    cancel: Arc<AtomicBool>,
    state: Arc<parking_lot::Mutex<SessionState>>,
}

struct SessionState {
    run: Arc<Run>,
    todos: TodoList,
    in_progress: bool,
}

/// RAII guard that clears `in_progress` when dropped, including via panic.
struct InProgressGuard {
    state: Arc<parking_lot::Mutex<SessionState>>,
}
impl Drop for InProgressGuard {
    fn drop(&mut self) {
        self.state.lock().in_progress = false;
    }
}
```

### 2.2 `new_session()` changes

Before constructing the `AcpThread`, build the SessionState:

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
        Arc::new(reverie_deepagent::Run::new_default()
            .map_err(|e| anyhow!("even temp-dir fallback failed: {e}"))?)
    }
};
let session_state = Arc::new(parking_lot::Mutex::new(SessionState {
    run,
    todos: TodoList::new(),
    in_progress: false,
}));
```

Helper:

```rust
fn build_persistent_run(
    scratch_root: &Path,
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

### 2.3 `prompt()` changes

At the top, acquire the run slot with rejection on concurrency:

```rust
let state = {
    let sessions = self.sessions.lock();
    sessions.get(&session_id).map(|s| s.state.clone())
};
let state = match state {
    Some(s) => s,
    None => return Task::ready(Err(anyhow!("unknown session {:?}", session_id.0.as_ref()))),
};
let (run, initial_todos, guard) = match acquire_run_slot(&state) {
    Ok(x) => x,
    Err(e) => return Task::ready(Err(e)),
};
```

Where:

```rust
fn acquire_run_slot(
    state: &Arc<parking_lot::Mutex<SessionState>>,
) -> Result<(Arc<Run>, TodoList, InProgressGuard)> {
    let mut st = state.lock();
    if st.in_progress {
        return Err(anyhow!(
            "a run is already in progress for this session; cancel it first"
        ));
    }
    st.in_progress = true;
    let run = st.run.clone();
    let initial_todos = st.todos.clone();
    Ok((run, initial_todos, InProgressGuard { state: state.clone() }))
}
```

In the `smol::unblock` closure, replace the current `Run::new_default()` with the cloned-in `Arc<Run>` plus the initial_todos:

```rust
let planner_task = smol::unblock(move || -> Result<PlannerResult> {
    let mut backend = ZedLlmBackend::new(req_tx);
    backend.seed_user_message(&user_text_for_planner);
    let observer = ChannelObserver::new(event_tx, cancel_for_planner);
    Ok(run_planner_with_observer_and_todos(
        &run,  // <- Arc<Run>, dereffed to &Run
        &mut backend,
        DEFAULT_MAX_ITERATIONS,
        &SpawnConfig::default(),
        &observer,
        initial_todos,
    ))
});
```

After `planner_task.await` and `event_drain.await`, write back the new TodoList:

```rust
{
    let mut st = state.lock();
    st.todos = planner_result.todos.clone();
}
// guard is dropped at end of scope → in_progress = false.
```

### 2.4 Reverie upstream (`crates/reverie-deepagent/src/planner.rs`)

Add:

```rust
pub fn run_planner_with_observer_and_todos(
    run: &Run,
    backend: &mut dyn LlmBackend,
    max_iterations: u32,
    spawn_cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
    initial_todos: TodoList,
) -> PlannerResult {
    // same body as run_planner_with_observer, but initialize `todos` to
    // `initial_todos` instead of `TodoList::new()`.
}

pub fn run_planner_with_observer(
    run: &Run,
    backend: &mut dyn LlmBackend,
    max_iterations: u32,
    spawn_cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
) -> PlannerResult {
    run_planner_with_observer_and_todos(
        run, backend, max_iterations, spawn_cfg, observer, TodoList::new(),
    )
}
```

Existing `run_planner_with_ctx` and `run_planner` keep delegating through `run_planner_with_observer` as they do today; behavior unchanged for all pre-Phase-1.5c callers.

### 2.5 Dependency additions

`crates/reverie_agent/Cargo.toml` gains one workspace dep:

```toml
paths.workspace = true
```

No new crates.io deps. `parking_lot` is already present. `paths::data_dir()` is the Zed-supplied per-platform data dir (macOS: `~/Library/Application Support/Zed/`, Linux: `~/.local/share/zed/`, Windows: `%APPDATA%\Zed\`).

---

## 3. Error Handling

### 3.1 Concurrent prompt on same session
`acquire_run_slot` returns `Err("a run is already in progress for this session; cancel it first")`. The caller sees a normal `Task<Result>` error and surfaces it as a conversation-level failure. Zed's UI typically disables the send button while a run is in flight, so this error is a belt-and-suspenders backstop.

### 3.2 Cancel mid-run
`cancel` AtomicBool flips true; planner's `should_stop` hook returns it; planner returns `TerminationReason::Cancelled`. **Partial TodoList state is captured** — whatever the planner has accumulated before cancellation is written back via `state.todos = planner_result.todos`. Next prompt resumes from that partial state. No rewind.

### 3.3 Scratch dir creation failure
On any filesystem error during `new_session`'s `mkdir_all` or `Vfs::new`, log at `warn` with the path and error, then fall back to `Run::new_default()` (a fresh temp-dir `Run`). The session still works, but its state is effectively ephemeral (temp dir may be cleaned up by the OS). Phase B will make this failure more visible.

### 3.4 Planner thread panic
`smol::unblock`'s future propagates panics via the existing `context("reverie planner thread failed")?` path. `InProgressGuard`'s `Drop` impl clears `in_progress` during unwind so subsequent prompts aren't permanently blocked. Verified by an explicit test (§4 test #6).

### 3.5 Session dropped (user closes the AcpThread)
The `Session` entry in the sessions map is keyed by `acp::SessionId`; its `WeakEntity<AcpThread>` goes dead, but the entry itself is not removed in Phase 1.5c. In-memory `SessionState` and the on-disk scratch dir both leak until Zed exits. Phase B's session-index + GC work addresses this. Not a correctness issue; just accumulating state.

### 3.6 TodoList replacement semantics
`planner_result.todos` is the *complete* final list from the run (includes pending, in-progress, completed entries). We replace `state.todos` wholesale with it — we don't merge. This matches how the planner internally treats `todos`: it's the canonical source of truth for the run. The next prompt sees exactly what the last prompt ended with.

---

## 4. Testing

### 4.1 Reverie upstream unit tests (in `planner.rs`, pure Rust)

1. **`seeded_initial_todos_visible_on_first_iteration`** — build a `TodoList` with two entries via `.add(...)`, pass as `initial_todos`, use a `MockLlmBackend` that records the `todos` argument on its first `next_action` call. Assert the first call sees `len == 2`.

2. **`empty_initial_todos_matches_old_behavior`** — run one scripted scenario through `run_planner_with_observer` (old entry point) and the same scenario through `run_planner_with_observer_and_todos(..., TodoList::new())`; assert identical `PlannerResult` (termination reason, iteration count, final todos, spawn_log length).

### 4.2 Dreamcode unit tests (in `tests.rs`, pure Rust)

Extract `acquire_run_slot` as a standalone `pub(crate)` function taking `&Arc<Mutex<SessionState>>`. Tests target it directly — no GPUI needed.

3. **`acquire_run_slot_rejects_when_in_progress`** — pre-seed `SessionState { in_progress: true, ... }`, call `acquire_run_slot`, assert `Err` matching `"already in progress"`.

4. **`acquire_run_slot_returns_current_todos_snapshot`** — seed `SessionState` with `todos = <list with one entry "alpha">`, call `acquire_run_slot`, assert the returned `TodoList` contains `"alpha"`. Mutating `state.todos` after acquire must NOT reflect in the returned snapshot (proves it's a clone, not a reference).

5. **`in_progress_guard_clears_in_progress_on_drop`** — call `acquire_run_slot`, assert `state.lock().in_progress == true`; drop the returned guard; assert `state.lock().in_progress == false`.

6. **`in_progress_guard_clears_in_progress_on_panic`** — `std::panic::catch_unwind` around a closure that calls `acquire_run_slot` then panics; assert `state.lock().in_progress == false` after `catch_unwind` returns the panic.

### 4.3 Deferred to integration / manual smoke

End-to-end prompt sequencing (prompt 1 seeds todo, prompt 2 sees it) needs a fully wired `ReverieAgentConnection` inside a GPUI test harness, which the existing Phase 1 / 1.5a code also defers. Keep that consistency.

**Manual smoke** (documented in `docs/reverie-agent.md`):
- Start Zed with Reverie selected.
- Prompt 1: "add a todo called investigate-project-structure."
- Expected: `[add_todo] investigate-project-structure` breadcrumb, then planner summary.
- Prompt 2 (same thread): "what todos do you have?"
- Expected (pre-1.5c): planner sees empty list → "I don't have any todos." 
- Expected (post-1.5c): planner sees `investigate-project-structure` in its initial state → references it.
- Prompt 3 (new thread via the `+` button): fresh state, confirms session scoping.
- Cancel test: kick a long prompt ("investigate the whole codebase and write notes"), cancel partway, send follow-up ("continue from where you left off"), verify partial todos + scratch files are visible.

### 4.4 Not tested in Phase 1.5c

- **Cross-Zed-restart persistence.** That's Phase B by design.
- **Scratch dir size caps / cleanup.** Phase B.
- **Session list UI / history view.** Phase C.
- **Multiple worktrees per session.** The scratch dir is keyed on `session_id`, not project, so it's orthogonal.

---

## 5. Known Limitations (Phase 1.5c)

- **Session state lost on Zed restart.** By design — Phase B adds persistence.
- **Accumulating scratch dirs under `<data_dir>/reverie-runs/`.** No cleanup yet. Power users will see this grow. Phase B adds retention.
- **No UI to list or resume past sessions.** Each thread's session state is opaque. Phase C.
- **No concurrent prompts on one session.** Rejected with a clear error; future work could serialize instead.
- **Scratch dir path collision.** If a session_id is reused (shouldn't happen, UUIDs), we'd reuse its dir. Not a real risk but worth noting.

## 6. Invoke-After

Per the brainstorming skill, after user approval of this spec the terminal state is invoking `writing-plans` to produce an implementation plan.

---

## Self-Review (inline)

**Placeholder scan:** No TBDs, TODOs, or vague requirements. Every field type, function signature, and error message is spelled out.

**Internal consistency:** §1 architecture diagram matches §2.3 `prompt()` flow matches §3 error handling. `InProgressGuard` defined in §2.1, its behavior documented in §3.4, its tests listed in §4.2 #5 and #6. `TodoList` snapshot semantics stated identically in §1, §2.3, and §3.6.

**Scope check:** One spec, one agent (Reverie), one connection file, one reverie upstream change. Under 200 LOC of implementation work estimated. Phase B and Phase C are explicitly out of scope and named as separate specs.

**Ambiguity check:**
- "TodoList replacement is wholesale, not merge" — stated in §3.6 to preempt the "maybe we should diff and merge" line of thinking.
- "Partial TodoList state is captured on cancel" — §3.2 makes this explicit, including that there's no rewind.
- "Scratch dir leaks on session drop" — §3.5 flags this explicitly and attributes cleanup to Phase B.
- "LLM transcript is NOT persisted" — stated in §1 twice and in the brainstorming decisions above; no room to re-open "wait, do we carry transcript?" during implementation.
