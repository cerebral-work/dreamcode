# Phase 1.5e — Nested Subagent Streaming — Design

**Status:** Draft, pending user review.
**Scope:** Thread the top-level planner's `PlannerObserver` through reverie's subagent spawn functions so nested subagent runs fire their per-iteration callbacks on the same observer instance. Dreamcode's `ChannelObserver` begins receiving `on_action` / `on_spawn_complete` / `should_stop` events from nested planners; those events ship through the existing session-update pipeline to the agent panel. No new dreamcode code changes; small reverie upstream commit.

**Context:** Phase 1.5a added `PlannerObserver` and wired it into the top-level `run_planner_with_observer` entry point. But reverie's `subagent::spawn` and `subagent::spawn_parallel` internally call `run_planner_with_ctx`, which uses `NoopObserver` for its own child-planner loop. That means when a top-level planner emits `NextAction::Spawn`, the child's planner steps (`AddTodo`, `SetStatus`, `VfsWrite`, etc.) are invisible to the observer — only the final `on_spawn_complete` callback fires after the child terminates. For short subagents this is fine, but for longer-running children it creates a UX dead zone where the agent panel appears frozen.

**Cross-phase coordination:**
- Complements Phase 1.5d's mid-call cancel: now the should_stop signal propagates into nested subagent iteration-top checks too, so Cancel stops nested children within a loop iteration.
- No interaction with Phase 1.5a (memory retrieval) or Phase 1.5c (session persistence). Those concerns live at the top-level only.
- Reverie upstream change lands on the same `feat/planner-observer` branch as Phase 1.5a's prep, Phase 1.5c's run_planner_with_observer_and_todos, and Phase 1.5d's Cancelled override. Fourth commit on that branch.

---

## 1. Architecture

Reverie's `subagent::spawn` and `spawn_parallel` get observer-aware variants. The existing (no-observer) functions become one-line delegates that pass `&NoopObserver`, so any non-observer-aware callers keep working byte-identically. The planner's `Spawn` and `ParallelSpawn` match arms switch to the `_with_observer` variants and thread the current `observer` parameter through. The observer itself gets a `Send + Sync` supertrait bound so it can be shared across parallel-spawn thread boundaries.

```
Before (Phase 1.5d):
  Top planner's observer receives:  AddTodo, SetStatus, Spawn(req), on_spawn_complete(obs)
  Child planner (via subagent::spawn) runs with NoopObserver:
      AddTodo, SetStatus, VfsWrite, etc.  ← invisible to top observer

After (Phase 1.5e):
  Top planner's observer receives:  AddTodo, SetStatus, Spawn(req), AddTodo, SetStatus,
                                    VfsWrite, ..., on_spawn_complete(obs)
                                    └────── child events ───────────┘
```

Three reverie files touched, zero dreamcode code touched (docs only).

---

## 2. Components & Interfaces

### 2.1 `PlannerObserver` supertrait bound

Change in `crates/reverie-deepagent/src/planner.rs`:

```rust
pub trait PlannerObserver: Send + Sync {
    fn on_action(&self, _action: &NextAction) {}
    fn on_spawn_complete(&self, _observation: &SpawnObservation) {}
    fn should_stop(&self) -> bool {
        false
    }
}
```

`NoopObserver` and dreamcode's `ChannelObserver` already satisfy this (no-data + `smol::channel::Sender` + `Arc<AtomicBool>` are all `Send + Sync`). Reverie's test observers (`RecordingObserver`, `StopOnBackendError`, etc.) also already use Send + Sync types (`std::sync::Mutex`). No consumer code changes needed.

### 2.2 `subagent::spawn` split

In `crates/reverie-deepagent/src/subagent.rs`:

Current:
```rust
pub fn spawn(
    run: &Run,
    req: &SpawnRequest,
    parent_backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
) -> SpawnResponse {
    // … existing body with:
    //     run_planner_with_ctx(&child_run, ..., cfg.child_max_iterations, cfg)
}
```

After:
```rust
pub fn spawn(
    run: &Run,
    req: &SpawnRequest,
    parent_backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
) -> SpawnResponse {
    spawn_with_observer(run, req, parent_backend, cfg, &NoopObserver)
}

pub fn spawn_with_observer(
    run: &Run,
    req: &SpawnRequest,
    parent_backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
) -> SpawnResponse {
    // … existing spawn() body, but the single internal call to
    //     run_planner_with_ctx(&child_run, child_backend.as_mut(),
    //                           cfg.child_max_iterations, cfg)
    // becomes
    //     run_planner_with_observer(&child_run, child_backend.as_mut(),
    //                                cfg.child_max_iterations, cfg, observer)
}
```

### 2.3 `subagent::spawn_parallel` split

Same shape:

```rust
pub fn spawn_parallel(
    run: &Run,
    reqs: &[SpawnRequest],
    parent_backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
) -> Vec<SpawnResponse> {
    spawn_parallel_with_observer(run, reqs, parent_backend, cfg, &NoopObserver)
}

pub fn spawn_parallel_with_observer(
    run: &Run,
    reqs: &[SpawnRequest],
    parent_backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
) -> Vec<SpawnResponse> {
    // … existing spawn_parallel body with the inner per-sibling
    //     run_planner_with_ctx(&p.run, ..., cfg)
    // replaced with
    //     run_planner_with_observer(&p.run, ..., cfg, observer)
}
```

The `observer: &dyn PlannerObserver` is shared across all parallel siblings via the std::thread::scope scope — safe because `PlannerObserver: Send + Sync` and the function takes `&dyn PlannerObserver` which is `Copy` for shared reference across threads within a scope.

### 2.4 Planner loop call sites

In `crates/reverie-deepagent/src/planner.rs`, two match arms change:

```rust
NextAction::Spawn(req) => {
    // was: let resp = spawn_one(run, &req, backend as &dyn LlmBackend, spawn_cfg);
    let resp = spawn_one_with_observer(
        run, &req, backend as &dyn LlmBackend, spawn_cfg, observer,
    );
    let obs = SpawnObservation::from(&resp);
    observer.on_spawn_complete(&obs);
    pending_observations.push(obs);
    spawn_log.push(resp);
    tool_calls_made += 1;
}
NextAction::ParallelSpawn(reqs) => {
    // was: let resps = spawn_parallel(run, &reqs, backend as &dyn LlmBackend, spawn_cfg);
    let resps = spawn_parallel_with_observer(
        run, &reqs, backend as &dyn LlmBackend, spawn_cfg, observer,
    );
    for r in &resps {
        let obs = SpawnObservation::from(r);
        observer.on_spawn_complete(&obs);
        pending_observations.push(obs);
    }
    spawn_log.extend(resps);
    tool_calls_made += 1;
}
```

Imports at top of planner.rs updated to alias the new names:

```rust
use crate::subagent::{
    SpawnConfig, SpawnRequest, SpawnResponse, SpawnStatus,
    spawn_parallel_with_observer,
    spawn_with_observer as spawn_one_with_observer,
};
```

(Old `spawn as spawn_one, spawn_parallel` imports deleted — unused in the planner after the match-arm updates.)

### 2.5 Dreamcode

Zero code changes. `ChannelObserver` implements the existing `PlannerObserver` trait; adding `Send + Sync` to the trait doesn't break it (ChannelObserver's fields are `smol::channel::Sender<acp::SessionUpdate>` + `Arc<AtomicBool>`, both Send + Sync).

Docs update: `docs/reverie-agent.md` loses the "No live streaming within subagents" limitation bullet; gains a one-line note under "What you'll see" that planner-step chunks from subagents interleave with the parent's.

---

## 3. Error Handling

### 3.1 Observer panics inside a subagent's on_action
`std::thread::scope` in parallel spawn propagates panics on scope exit; the parent planner sees the panic via the scope's join semantics. Existing behavior, not a regression.

### 3.2 `should_stop` during parallel siblings
Each sibling checks `should_stop` at its own loop top. When `should_stop` flips true mid-parallel-spawn, each sibling terminates independently at its next iteration (within one iteration of its own loop — no coordination delay). The parent's spawn_parallel_with_observer waits for all siblings to finish before returning. Cancel during parallel spawn therefore stops all children within ~one LLM-call worth of time per sibling.

### 3.3 Send + Sync breaking change
Technically breaking for any external `!Send` `PlannerObserver` impl. In practice:
- `NoopObserver` (reverie) — Send + Sync ✓
- `ChannelObserver` (dreamcode) — Send + Sync ✓
- Reverie's test observers (`RecordingObserver` etc.) — all use `std::sync::Mutex`/`Arc<Mutex>` → Send + Sync ✓
- No known external consumers.

Accept as a non-semver-breaking bump (trait was introduced in the same `feat/planner-observer` branch; we're still iterating on it pre-merge).

### 3.4 Depth interaction
Nested subagents beyond depth 1 also thread the observer through (recursion via `spawn_with_observer` → `run_planner_with_observer` → `NextAction::Spawn` arm → `spawn_with_observer`). Depth limiting stays at `HARD_MAX_DEPTH = 4`; the observer propagation is orthogonal.

### 3.5 `spawn_stub` unchanged
`spawn_stub` (the Phase-1 compat stub) doesn't run a planner, so there's nothing to observe. No signature change.

### 3.6 External callers of `subagent::spawn` / `spawn_parallel`
Both functions keep their existing signatures. External callers see byte-identical behavior (`NoopObserver` is behaviorally a no-op, just like the previous `run_planner_with_ctx` with its built-in NoopObserver).

---

## 4. Testing

### 4.1 Reverie upstream — 2 new tests

In `crates/reverie-deepagent/src/subagent.rs` tests module:

1. **`spawn_with_observer_forwards_child_actions`** — scripted parent emits `Spawn(...)` that yields a child which does `AddTodo("child-alpha") + SetStatus(1, Completed)`. Wire a `RecordingObserver` (same shape as the one used in Phase 1.5a's planner tests); assert the recorded actions include strings like `"AddTodo(\"child-alpha\")"` and `"SetStatus(1, Completed)"`.

2. **`spawn_parallel_with_observer_forwards_all_siblings`** — scripted parent emits `ParallelSpawn(vec![req_a, req_b])` where sibling A does `AddTodo("alpha")` and sibling B does `AddTodo("beta")`. Assert the observer's recorded actions contain both `"alpha"` and `"beta"` substrings. Order is NOT asserted (parallel threads interleave nondeterministically).

Existing subagent tests (`spawn_success_runs_child_to_completion`, `spawn_parallel_runs_siblings_concurrently`, etc.) call the unchanged `spawn` / `spawn_parallel` signatures — they should pass byte-for-byte.

### 4.2 Dreamcode

No new tests. The behavior change is "the existing pipe carries more events"; no new control flow in the driver.

### 4.3 Manual smoke

After the reverie commit lands and dreamcode rebuilds:
1. Start Zed with Reverie.
2. Prompt something likely to spawn a subagent, e.g. "break this problem down: research the X library, implement a Y, and write tests."
3. The agent panel should show per-step chunks from the subagent's work interleaved between the `[spawn] …` and `[subagent …] Success: …` breadcrumbs. Before Phase 1.5e, there'd be a gap with only the breadcrumbs bracketing silence.

### 4.4 Not tested

- **Observer panic propagation across parallel thread boundaries.** The `std::thread::scope` behavior is standard library territory; not our test surface.
- **Deep nesting observer coverage (depth 3–4).** Recursion through the same code path; one-level test is sufficient.

---

## 5. Known Limitations (Phase 1.5e)

- **Events from subagents are not labeled with which child emitted them.** Interleaving is chronological; the `[spawn] researcher :: ...` and `[subagent researcher] Success: ...` breadcrumbs bracket each child's block. If this becomes confusing in practice, Phase 1.5e.2 can add `persona: Option<&str>` to the observer methods and render labeled chunks.
- **Parallel siblings' events interleave nondeterministically.** Cosmetic only — the actions themselves are correctly recorded and land in the right todos/vfs.
- **No grouping UI.** Subagent chunks appear in the same flat chat as parent chunks. A nested/collapsible view is future work.

## 6. Invoke-After

Per the brainstorming skill, after user approval the terminal state is invoking `writing-plans` to produce an implementation plan.

---

## Self-Review (inline)

**Placeholder scan:** no TBDs, TODOs, or vague language. Every method signature and import alias is spelled out. Test expectations name the string contents the observer should record.

**Internal consistency:**
- `PlannerObserver: Send + Sync` stated consistently in §2.1 and §3.3.
- `spawn_with_observer(run, req, parent_backend, cfg, observer)` signature matches between §2.2 (definition) and §2.4 (call site).
- `spawn_parallel_with_observer` same shape.
- Planner import aliases in §2.4 match the call-site names.

**Scope check:** one spec, one feature (nested observer propagation), ~40 LOC reverie + 2 tests + 1 docs tweak in dreamcode. Under half a day of work.

**Ambiguity check:**
- "Observer shared across parallel siblings" is explicit in §2.3 and §3.1.
- "Events from subagents are unlabeled" pre-empted in §5 to stop "should we add labels?" questions during implementation.
- "No dreamcode code changes" stated in §2.5 and §4.2 — no ambiguity about where the work lives.
