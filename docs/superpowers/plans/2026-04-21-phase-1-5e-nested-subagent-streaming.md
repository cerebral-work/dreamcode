# Phase 1.5e — Nested Subagent Streaming Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Thread the top-level `PlannerObserver` through reverie's `subagent::spawn` / `spawn_parallel` so nested subagent planners fire their per-iteration `on_action` / `on_spawn_complete` / `should_stop` callbacks on the same observer instance used by the top-level planner.

**Architecture:** Add `Send + Sync` supertrait bound to `PlannerObserver`. Add `spawn_with_observer` and `spawn_parallel_with_observer` public functions in `crates/reverie-deepagent/src/subagent.rs`; existing `spawn` / `spawn_parallel` delegate to these new variants with `&NoopObserver`. Update `drive_prepared_child` (private helper) to accept an observer. Planner loop's `Spawn` / `ParallelSpawn` match arms call the `_with_observer` variants and pass the loop's `observer` parameter through.

**Tech Stack:** Rust, `PlannerObserver` trait + `run_planner_with_observer`, `std::thread::scope` (sync parallel path), `tokio::task::JoinSet` (async parallel path — left on `NoopObserver` for this phase).

**Spec reference:** `docs/superpowers/specs/2026-04-21-phase-1-5e-nested-subagent-streaming-design.md`.

**Working directory:** `/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend/` on branch `feat/reverie-agent-backend`. Reverie at `/Users/dennis/programming projects/reverie` on branch `feat/planner-observer` (4th commit in this series).

---

## File Structure

**Modified files (reverie repo):**
- `crates/reverie-deepagent/src/planner.rs` — add `Send + Sync` bound to `PlannerObserver`; update `Spawn` / `ParallelSpawn` match arms to call `_with_observer` spawn variants; update imports.
- `crates/reverie-deepagent/src/subagent.rs` — add `spawn_with_observer` + `spawn_parallel_with_observer`; existing `spawn` / `spawn_parallel` delegate with `&NoopObserver`; `drive_prepared_child` gains an observer parameter; async variant passes `&NoopObserver`. Import `run_planner_with_observer` and `NoopObserver`. Add 2 tests.

**Modified files (dreamcode worktree):**
- `docs/reverie-agent.md` — remove the "No live streaming within subagents" limitation bullet; add a one-line note under "What you'll see" that child planner events interleave with the parent's.

**No code changes in `crates/reverie_agent/`.** `ChannelObserver` already satisfies `Send + Sync` (smol::channel::Sender + Arc<AtomicBool>).

---

## Task 1: Reverie — `PlannerObserver: Send + Sync` supertrait bound

**Files (reverie repo):**
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/planner.rs`

- [ ] **Step 1: Add the Send + Sync supertrait bound**

In `crates/reverie-deepagent/src/planner.rs`, find the existing trait definition:

```rust
pub trait PlannerObserver {
    fn on_action(&self, _action: &NextAction) {}
    fn on_spawn_complete(&self, _observation: &SpawnObservation) {}
    fn should_stop(&self) -> bool {
        false
    }
}
```

Replace with:

```rust
pub trait PlannerObserver: Send + Sync {
    fn on_action(&self, _action: &NextAction) {}
    fn on_spawn_complete(&self, _observation: &SpawnObservation) {}
    fn should_stop(&self) -> bool {
        false
    }
}
```

- [ ] **Step 2: Run the full reverie suite to confirm no existing impl regresses**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent`

Expected: 79/79 pass (same as after Phase 1.5d). NoopObserver, RecordingObserver, StopOnBackendError, ObserverStoppingBeforeNth, NudgeTrackingBackend — all already use `Send + Sync` types internally, so the bound is satisfied without changes.

If any test fails with a "PlannerObserver is not Send/Sync" error, the offending test observer needs a field-type tweak (e.g., `Rc<RefCell<_>>` → `Arc<Mutex<_>>`). Inspect and fix in-place; do NOT remove the bound.

- [ ] **Step 3: Verify dreamcode still compiles**

Run: `cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend" && cargo check -p reverie_agent`

Expected: clean. `ChannelObserver` (smol::channel::Sender + Arc<AtomicBool>) is Send + Sync.

- [ ] **Step 4: Commit**

```bash
cd "/Users/dennis/programming projects/reverie"
git add crates/reverie-deepagent/src/planner.rs
git commit -m "$(cat <<'EOF'
deepagent: PlannerObserver gains Send + Sync supertrait bound

Required for Phase 1.5e's nested-subagent observer threading.
spawn_parallel shares the observer across std::thread::scope thread
boundaries; that only works if the observer is both Send (can move
into a thread) and Sync (can be borrowed across threads via &dyn).

All existing impls (NoopObserver, dreamcode's ChannelObserver, the
reverie test observers RecordingObserver / StopOnBackendError /
ObserverStoppingBeforeNth) already use Send + Sync types, so the
bound is non-breaking in practice.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Reverie — split `spawn` into `spawn` delegate + `spawn_with_observer` (TDD)

**Files (reverie repo):**
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/subagent.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block at the bottom of `subagent.rs`, after the last existing test:

```rust
    #[test]
    fn spawn_with_observer_forwards_child_actions() {
        use crate::planner::{NextAction, NoopObserver, PlannerObserver, SpawnObservation};
        use std::sync::{Arc, Mutex};

        struct RecordingObserver {
            actions: Arc<Mutex<Vec<String>>>,
        }
        impl PlannerObserver for RecordingObserver {
            fn on_action(&self, action: &NextAction) {
                self.actions.lock().unwrap().push(format!("{action:?}"));
            }
            fn on_spawn_complete(&self, _obs: &SpawnObservation) {}
            fn should_stop(&self) -> bool { false }
        }

        let (_p, parent_run) = fresh_run();
        // Child backend scripts a productive child run.
        let child_factory: Arc<dyn Fn() -> Box<dyn crate::planner::LlmBackend + Send> + Send + Sync> =
            Arc::new(|| {
                Box::new(crate::planner::MockLlmBackend::new([
                    NextAction::AddTodo("child-alpha".into()),
                    NextAction::SetStatus(1, crate::todos::TodoStatus::Completed),
                    NextAction::NoOp,
                ]))
            });
        // Parent backend is only used as the "parent backend" arg; spawn does
        // its own child-backend factory call.
        let parent_backend = crate::planner::MockLlmBackend::with_child_factory(
            vec![],
            Some(child_factory),
        );

        let recorded = Arc::new(Mutex::new(Vec::<String>::new()));
        let observer = RecordingObserver { actions: recorded.clone() };

        let req = SpawnRequest {
            persona: "researcher".into(),
            task: "scan".into(),
            vfs_subdir: "sub/r".into(),
        };

        let resp = spawn_with_observer(
            &parent_run,
            &req,
            &parent_backend as &dyn crate::planner::LlmBackend,
            &SpawnConfig::default(),
            &observer,
        );
        assert_eq!(resp.status, SpawnStatus::Success, "scripted child should succeed");

        let recorded = recorded.lock().unwrap();
        let joined = recorded.join(" | ");
        assert!(
            joined.contains("AddTodo"),
            "observer should have recorded child's AddTodo — got: {joined}"
        );
        assert!(
            joined.contains("child-alpha"),
            "observer should have seen the AddTodo's description — got: {joined}"
        );
    }
```

Note: the test calls `spawn_with_observer` which doesn't exist yet. That's the failing test.

- [ ] **Step 2: Run the test; verify it fails to compile**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent spawn_with_observer_forwards_child_actions`

Expected: FAIL — `spawn_with_observer` not found.

- [ ] **Step 3: Add `spawn_with_observer` and make `spawn` delegate**

In `crates/reverie-deepagent/src/subagent.rs`, update the imports at the top:

```rust
use crate::planner::{BackendError, LlmBackend, TerminationReason, run_planner_with_ctx};
```

Replace with:

```rust
use crate::planner::{
    BackendError, LlmBackend, NoopObserver, PlannerObserver, TerminationReason,
    run_planner_with_observer,
};
```

Note: we're removing the `run_planner_with_ctx` import. Step 4 below confirms it's unused after the refactor.

Now find the existing `pub fn spawn(...)` (around line 275). Replace its ENTIRE body with a delegate, and add the new `spawn_with_observer` right after it with the original body plus one line changed:

```rust
pub fn spawn(
    parent: &Run,
    req: &SpawnRequest,
    backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
) -> SpawnResponse {
    spawn_with_observer(parent, req, backend, cfg, &NoopObserver)
}

pub fn spawn_with_observer(
    parent: &Run,
    req: &SpawnRequest,
    backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
) -> SpawnResponse {
    let max = cfg.clamped_max_depth();
    let prospective_depth = parent.depth + 1;
    if prospective_depth > max {
        warn!(
            parent_depth = parent.depth,
            max_depth = max,
            persona = %req.persona,
            "deepagent: spawn refused — depth limit reached"
        );
        return SpawnResponse {
            persona: req.persona.clone(),
            status: SpawnStatus::DepthLimit,
            summary: format!(
                "spawn refused: parent depth {} + 1 > max_depth {}",
                parent.depth, max
            ),
            child_run_id: String::new(),
            child_todos: TodoList::new(),
            child_iterations: 0,
        };
    }

    let child_run = match Run::new_child(parent, &req.vfs_subdir) {
        Ok(r) => r,
        Err(e) => return refused(req, format!("allocate child run: {e}")),
    };
    let (mut child_backend, backend_source) = match build_child_backend(&req.persona, backend, cfg)
    {
        Ok(pair) => pair,
        Err(e) => return refused(req, format!("build child backend: {e}")),
    };

    info!(
        persona = %req.persona,
        task = %req.task,
        vfs_subdir = %req.vfs_subdir,
        child_depth = child_run.depth,
        child_run_id = %child_run.id,
        backend_source = backend_source,
        "deepagent: spawning child run"
    );

    // Phase 1.5e: thread the parent observer into the child planner so
    // child actions fire on the same observer.
    let result = run_planner_with_observer(
        &child_run,
        child_backend.as_mut(),
        cfg.child_max_iterations,
        cfg,
        observer,
    );

    let status = match result.termination {
        TerminationReason::Completed => SpawnStatus::Success,
        _ => SpawnStatus::Failed,
    };
    let summary = format!(
        "subagent '{}' terminated {:?} after {} iterations (task: {})",
        req.persona, result.termination, result.iterations, req.task
    );
    SpawnResponse {
        persona: req.persona.clone(),
        status,
        summary,
        child_run_id: child_run.id.clone(),
        child_todos: result.todos,
        child_iterations: result.iterations,
    }
}
```

- [ ] **Step 4: Run the full reverie suite**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent`

Expected: 80/80 pass (79 pre-existing + 1 new). If `run_planner_with_ctx` is flagged as an unused-import warning, leave it — Task 3 will remove it after `spawn_parallel`'s path migrates too. If it's flagged as UNUSED (hard error), that means nothing else in subagent.rs references it; delete the import now.

Actually: the test at line 630 (`use crate::planner::{MockLlmBackend, NextAction};`) does not use `run_planner_with_ctx` either. If removing the top-level `run_planner_with_ctx` import (already done in Step 3) causes a compile error in `drive_prepared_child` — that's Task 3's territory, so we temporarily re-add the import just for Task 2 to land cleanly:

Revert the imports block to include `run_planner_with_ctx` temporarily:

```rust
use crate::planner::{
    BackendError, LlmBackend, NoopObserver, PlannerObserver, TerminationReason,
    run_planner_with_ctx, run_planner_with_observer,
};
```

Task 3 removes `run_planner_with_ctx` once `drive_prepared_child` migrates.

- [ ] **Step 5: Commit**

```bash
cd "/Users/dennis/programming projects/reverie"
git add crates/reverie-deepagent/src/subagent.rs
git commit -m "$(cat <<'EOF'
deepagent: split spawn into no-observer delegate + spawn_with_observer

spawn_with_observer threads the caller's PlannerObserver into the
child planner loop via run_planner_with_observer. The existing spawn
becomes a one-line delegate that passes &NoopObserver, so every
existing caller (including subagent's internal spawn_parallel path)
keeps working byte-identically.

Test spawn_with_observer_forwards_child_actions confirms a scripted
child's AddTodo fires on the parent's observer.

Task 3 splits spawn_parallel the same way.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Reverie — split `spawn_parallel` + update `drive_prepared_child` to accept observer (TDD)

**Files (reverie repo):**
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/subagent.rs`

- [ ] **Step 1: Write the failing test**

Append to the tests module in `subagent.rs`, after `spawn_with_observer_forwards_child_actions`:

```rust
    #[test]
    fn spawn_parallel_with_observer_forwards_all_siblings() {
        use crate::planner::{NextAction, PlannerObserver, SpawnObservation};
        use std::sync::{Arc, Mutex};

        struct RecordingObserver {
            actions: Arc<Mutex<Vec<String>>>,
        }
        impl PlannerObserver for RecordingObserver {
            fn on_action(&self, action: &NextAction) {
                self.actions.lock().unwrap().push(format!("{action:?}"));
            }
            fn on_spawn_complete(&self, _obs: &SpawnObservation) {}
            fn should_stop(&self) -> bool { false }
        }

        let (_p, parent_run) = fresh_run();
        let child_factory: Arc<dyn Fn() -> Box<dyn crate::planner::LlmBackend + Send> + Send + Sync> =
            Arc::new(|| {
                // Each sibling emits a distinctive AddTodo. Because both siblings
                // use the same factory, we rely on the order of AddTodo emission
                // to distinguish them via a sibling-index atomic.
                static NEXT_IDX: std::sync::atomic::AtomicU32 =
                    std::sync::atomic::AtomicU32::new(0);
                let idx = NEXT_IDX.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let desc = if idx % 2 == 0 { "alpha" } else { "beta" };
                Box::new(crate::planner::MockLlmBackend::new([
                    NextAction::AddTodo(desc.into()),
                    NextAction::SetStatus(1, crate::todos::TodoStatus::Completed),
                    NextAction::NoOp,
                ]))
            });
        let parent_backend = crate::planner::MockLlmBackend::with_child_factory(
            vec![],
            Some(child_factory),
        );

        let recorded = Arc::new(Mutex::new(Vec::<String>::new()));
        let observer = RecordingObserver { actions: recorded.clone() };

        let reqs = vec![
            SpawnRequest {
                persona: "a".into(),
                task: "do-a".into(),
                vfs_subdir: "sub/a".into(),
            },
            SpawnRequest {
                persona: "b".into(),
                task: "do-b".into(),
                vfs_subdir: "sub/b".into(),
            },
        ];

        let resps = spawn_parallel_with_observer(
            &parent_run,
            &reqs,
            &parent_backend as &dyn crate::planner::LlmBackend,
            &SpawnConfig::default(),
            &observer,
        );
        assert_eq!(resps.len(), 2);
        assert!(
            resps.iter().all(|r| r.status == SpawnStatus::Success),
            "both siblings should succeed"
        );

        let recorded = recorded.lock().unwrap();
        let joined = recorded.join(" | ");
        assert!(
            joined.contains("alpha"),
            "observer should have recorded sibling A's AddTodo — got: {joined}"
        );
        assert!(
            joined.contains("beta"),
            "observer should have recorded sibling B's AddTodo — got: {joined}"
        );
    }
```

- [ ] **Step 2: Run the test; verify it fails to compile**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent spawn_parallel_with_observer_forwards_all_siblings`

Expected: FAIL — `spawn_parallel_with_observer` not found.

- [ ] **Step 3: Update `drive_prepared_child` to accept observer**

In `crates/reverie-deepagent/src/subagent.rs`, find the private helper `drive_prepared_child` (around line 434):

```rust
fn drive_prepared_child(mut p: PreparedChild, cfg: &SpawnConfig) -> (usize, SpawnResponse) {
    info!(
        persona = %p.persona,
        child_run_id = %p.run.id,
        child_depth = p.run.depth,
        "deepagent: parallel subagent starting"
    );
    let result = run_planner_with_ctx(&p.run, p.backend.as_mut(), cfg.child_max_iterations, cfg);
    // …rest unchanged
```

Replace with:

```rust
fn drive_prepared_child(
    mut p: PreparedChild,
    cfg: &SpawnConfig,
    observer: &dyn PlannerObserver,
) -> (usize, SpawnResponse) {
    info!(
        persona = %p.persona,
        child_run_id = %p.run.id,
        child_depth = p.run.depth,
        "deepagent: parallel subagent starting"
    );
    let result = run_planner_with_observer(
        &p.run,
        p.backend.as_mut(),
        cfg.child_max_iterations,
        cfg,
        observer,
    );
    let status = match result.termination {
        TerminationReason::Completed => SpawnStatus::Success,
        _ => SpawnStatus::Failed,
    };
    let resp = SpawnResponse {
        persona: p.persona.clone(),
        status,
        summary: format!(
            "subagent '{}' terminated {:?} after {} iterations (task: {})",
            p.persona, result.termination, result.iterations, p.task
        ),
        child_run_id: p.run.id.clone(),
        child_todos: result.todos,
        child_iterations: result.iterations,
    };
    (p.idx, resp)
}
```

- [ ] **Step 4: Add `spawn_parallel_with_observer` and make `spawn_parallel` delegate**

Find the existing `pub fn spawn_parallel` (around line 480). Replace it with:

```rust
pub fn spawn_parallel(
    parent: &Run,
    requests: &[SpawnRequest],
    backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
) -> Vec<SpawnResponse> {
    spawn_parallel_with_observer(parent, requests, backend, cfg, &NoopObserver)
}

pub fn spawn_parallel_with_observer(
    parent: &Run,
    requests: &[SpawnRequest],
    backend: &dyn LlmBackend,
    cfg: &SpawnConfig,
    observer: &(dyn PlannerObserver),
) -> Vec<SpawnResponse> {
    if requests.is_empty() {
        return Vec::new();
    }
    let Preflight { prepared, mut results } = preflight_parallel(parent, requests, backend, cfg);

    // Run concurrently via scoped threads. Each thread writes its result
    // slot under a Mutex. Phase 1.5e: each thread also receives the
    // parent observer (must be Send + Sync).
    let slots: Mutex<&mut Vec<Option<SpawnResponse>>> = Mutex::new(&mut results);
    std::thread::scope(|s| {
        for p in prepared {
            let slots_ref = &slots;
            let cfg_c = cfg.clone();
            let observer_ref = observer;
            s.spawn(move || {
                let (idx, resp) = drive_prepared_child(p, &cfg_c, observer_ref);
                let mut guard = slots_ref.lock().unwrap();
                guard[idx] = Some(resp);
            });
        }
    });

    finalize_results(results)
}
```

- [ ] **Step 5: Update the async variant to pass `&NoopObserver`**

Still in `subagent.rs`, find the `#[cfg(feature = "async-spawn")] pub async fn spawn_parallel_async` (around line 522). Its body calls `drive_prepared_child(p, &cfg_c)`. Update to pass `&NoopObserver`:

```rust
    for p in prepared {
        let cfg_c = cfg.clone();
        set.spawn_blocking(move || drive_prepared_child(p, &cfg_c, &NoopObserver));
    }
```

Note: `NoopObserver` needs to be accessible here. It's imported at the top of subagent.rs (from Task 2). `&NoopObserver` is a zero-sized reference that satisfies `'static` so the `spawn_blocking` closure's Send + 'static bound is satisfied.

- [ ] **Step 6: Remove the now-unused `run_planner_with_ctx` import**

At the top of `subagent.rs`, the import line from Task 2 had a temporary `run_planner_with_ctx` entry. Update:

Before:
```rust
use crate::planner::{
    BackendError, LlmBackend, NoopObserver, PlannerObserver, TerminationReason,
    run_planner_with_ctx, run_planner_with_observer,
};
```

After:
```rust
use crate::planner::{
    BackendError, LlmBackend, NoopObserver, PlannerObserver, TerminationReason,
    run_planner_with_observer,
};
```

- [ ] **Step 7: Run the full reverie suite including the async feature**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent`

Expected: 81/81 pass (79 pre-existing + Task 2's 1 + this task's 1).

Then run with the async feature to make sure the async path still compiles:

Run: `cd "/Users/dennis/programming projects/reverie" && cargo check -p reverie-deepagent --features async-spawn`

Expected: clean. If the async path fails with a `NoopObserver: !Send` or similar, the issue is that NoopObserver must be 'static for tokio::task::spawn_blocking — `&NoopObserver` is `'static` because `NoopObserver` is `ZST + Sync`, but the borrow reference isn't `'static`. If so, change to owned: `&NoopObserver` → pass the zero-sized value by move. Try:

```rust
set.spawn_blocking(move || drive_prepared_child(p, &cfg_c, &NoopObserver));
```

`NoopObserver` is a unit-like struct so constructing it inline is free. If that also fails, we may need to change the async path's drive_prepared_child signature to take `Arc<dyn PlannerObserver>` — but this is unlikely; the `&NoopObserver` borrow is `'static` because `NoopObserver`'s only instance is a ZST that lives forever.

- [ ] **Step 8: Commit**

```bash
cd "/Users/dennis/programming projects/reverie"
git add crates/reverie-deepagent/src/subagent.rs
git commit -m "$(cat <<'EOF'
deepagent: split spawn_parallel + thread observer through siblings

spawn_parallel_with_observer routes each parallel sibling's planner
loop through run_planner_with_observer, passing the caller's
observer via drive_prepared_child. The observer is shared across
std::thread::scope threads — safe now that PlannerObserver is
Send + Sync (Task 1).

Existing spawn_parallel delegates with &NoopObserver; async variant
(spawn_parallel_async, behind async-spawn feature) also passes
&NoopObserver so its behavior is unchanged for this phase.

Test spawn_parallel_with_observer_forwards_all_siblings confirms
both siblings' AddTodo actions are recorded on the shared observer.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Reverie — planner loop calls `_with_observer` variants

**Files (reverie repo):**
- Modify: `/Users/dennis/programming projects/reverie/crates/reverie-deepagent/src/planner.rs`

- [ ] **Step 1: Update the subagent imports**

At the top of `crates/reverie-deepagent/src/planner.rs`, find:

```rust
use crate::subagent::{
    SpawnConfig, SpawnRequest, SpawnResponse, SpawnStatus, spawn as spawn_one, spawn_parallel,
};
```

Replace with:

```rust
use crate::subagent::{
    SpawnConfig, SpawnRequest, SpawnResponse, SpawnStatus,
    spawn_parallel_with_observer,
    spawn_with_observer as spawn_one_with_observer,
};
```

- [ ] **Step 2: Update the `Spawn` match arm**

In `run_planner_with_observer_and_todos` (the body that contains the match on `action`), find:

```rust
            NextAction::Spawn(req) => {
                let resp = spawn_one(run, &req, backend as &dyn LlmBackend, spawn_cfg);
                let obs = SpawnObservation::from(&resp);
                observer.on_spawn_complete(&obs);
                pending_observations.push(obs);
                spawn_log.push(resp);
                tool_calls_made += 1;
            },
```

Replace with:

```rust
            NextAction::Spawn(req) => {
                let resp = spawn_one_with_observer(
                    run,
                    &req,
                    backend as &dyn LlmBackend,
                    spawn_cfg,
                    observer,
                );
                let obs = SpawnObservation::from(&resp);
                observer.on_spawn_complete(&obs);
                pending_observations.push(obs);
                spawn_log.push(resp);
                tool_calls_made += 1;
            },
```

- [ ] **Step 3: Update the `ParallelSpawn` match arm**

In the same match block, find:

```rust
            NextAction::ParallelSpawn(reqs) => {
                let resps = spawn_parallel(run, &reqs, backend as &dyn LlmBackend, spawn_cfg);
                for r in &resps {
                    let obs = SpawnObservation::from(r);
                    observer.on_spawn_complete(&obs);
                    pending_observations.push(obs);
                }
                spawn_log.extend(resps);
                tool_calls_made += 1;
            },
```

Replace with:

```rust
            NextAction::ParallelSpawn(reqs) => {
                let resps = spawn_parallel_with_observer(
                    run,
                    &reqs,
                    backend as &dyn LlmBackend,
                    spawn_cfg,
                    observer,
                );
                for r in &resps {
                    let obs = SpawnObservation::from(r);
                    observer.on_spawn_complete(&obs);
                    pending_observations.push(obs);
                }
                spawn_log.extend(resps);
                tool_calls_made += 1;
            },
```

- [ ] **Step 4: Run the full reverie suite**

Run: `cd "/Users/dennis/programming projects/reverie" && cargo test -p reverie-deepagent`

Expected: 81/81 pass. Existing tests like `spawn_actions_are_recorded` and `parallel_spawn_action_runs_siblings_and_logs_all` exercise the planner loop → subagent path; they use `run_planner` (which ultimately routes through the new `_with_observer` path with NoopObserver), so behavior is unchanged.

- [ ] **Step 5: Verify dreamcode compiles and existing tests pass**

Run: `cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend" && cargo test -p reverie_agent`

Expected: 17/17 pass.

Run: `cargo check -p agent_ui`

Expected: clean.

- [ ] **Step 6: Commit**

```bash
cd "/Users/dennis/programming projects/reverie"
git add crates/reverie-deepagent/src/planner.rs
git commit -m "$(cat <<'EOF'
deepagent: planner loop routes subagent spawns through _with_observer

The Spawn and ParallelSpawn match arms in the planner's main loop
now call spawn_with_observer / spawn_parallel_with_observer with
the loop's observer parameter. For NoopObserver (the default for
all non-observer-aware callers) this is a byte-identical pass-
through; for a real observer (dreamcode's ChannelObserver) the
child planner's per-iteration on_action / on_spawn_complete /
should_stop callbacks now fire on the same observer as the parent.

Depth limits, iteration caps, spawn_log ordering, and pending-
observations injection are all unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 7: Verify dreamcode picks up the new reverie behaviour at the HEAD of the path dep**

Run: `cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend" && cargo check -p reverie_agent && cargo test -p reverie_agent`

Expected: 17/17 dreamcode tests pass. ChannelObserver is unchanged but now receives nested-subagent actions when a real planner run fires a `Spawn` / `ParallelSpawn` — not directly tested here (requires live LLM + agent panel), but verified end-to-end in the manual smoke step in Task 5.

---

## Task 5: Dreamcode — update `docs/reverie-agent.md`

**Files (dreamcode worktree):**
- Modify: `docs/reverie-agent.md`

- [ ] **Step 1: Remove the "No live streaming within subagents" limitation bullet**

In `docs/reverie-agent.md`, find the "## Known limitations" section. Remove the bullet:

```markdown
- **No live streaming within subagents.** The observer only fires on the top-level planner loop; nested subagent planners run to completion before their `SpawnObservation` ships.
```

Replace with:

```markdown
- **Subagent events interleave with parent events chronologically.** There's no UI grouping or indentation — a subagent's `[add_todo]` chunk appears in the same flat chat as the parent's. The `[spawn] <persona> :: <task>` breadcrumb before and `[subagent <persona>] <Status>: <summary>` after bracket each child's block for visual context.
```

- [ ] **Step 2: Update the "What you'll see" section**

In the same file, find the "## What you'll see" section. After the bullet list of chunk kinds and before (or as) the final-line description, insert:

```markdown
When the planner spawns a subagent, the child's planner steps appear inline in the same chat — interleaved between the `[spawn]` breadcrumb and the `[subagent … Success]` terminal chunk. Parallel siblings' events interleave nondeterministically with each other.
```

- [ ] **Step 3: Verify docs**

Run: `head -80 docs/reverie-agent.md`

Expected: the "What you'll see" section has the new paragraph; the Known limitations bullet is updated.

- [ ] **Step 4: Commit**

```bash
cd "/Users/dennis/programming projects/dreamcode/.worktrees/reverie-agent-backend"
git add docs/reverie-agent.md
git commit -m "$(cat <<'EOF'
docs(reverie-agent): Phase 1.5e nested subagent streaming

Per-iteration events from nested subagent planners now fire on the
top-level ChannelObserver and surface in the agent panel alongside
the parent's. Removes the "no live streaming within subagents"
limitation and documents the interleaved-chronologically UX
(subagent blocks are visually bracketed by the existing [spawn]
and [subagent] breadcrumbs, not grouped or indented).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**1. Spec coverage:**

| Spec section | Tasks |
|---|---|
| §1 Architecture — observer threaded through subagent spawns | Tasks 2, 3, 4 |
| §2.1 PlannerObserver: Send + Sync | Task 1 |
| §2.2 spawn_with_observer + spawn delegate | Task 2 |
| §2.3 spawn_parallel_with_observer + spawn_parallel delegate | Task 3 |
| §2.4 Planner match arms route through _with_observer | Task 4 |
| §2.5 No dreamcode code changes | Confirmed in Tasks 4 Step 7 and Task 5 |
| §3.1 Observer panic propagation via thread::scope | Not code — behaviour documented; existing std::thread::scope semantics apply |
| §3.2 should_stop during parallel siblings | Existing should_stop flow per sibling; no code change needed beyond observer propagation |
| §3.3 Send + Sync break analysis | Task 1 Step 2 verifies all existing impls satisfy |
| §3.4 Depth recursion | Implicit — spawn_with_observer recurses via the planner loop which now routes through spawn_one_with_observer |
| §3.5 spawn_stub unchanged | No changes to spawn_stub in any task |
| §3.6 External callers of spawn/spawn_parallel | The delegate pattern means external callers see identical behaviour (`&NoopObserver` is equivalent to the prior `run_planner_with_ctx`'s built-in NoopObserver) |
| §4.1 Reverie tests (2) | Tasks 2 and 3 each add one |
| §4.2 No new dreamcode tests | Confirmed |
| §5 Known limitations | Task 5 (docs) |

**2. Placeholder scan:** No "TBD", "TODO", or vague error-handling placeholders. Every edit shows exact before/after text.

**3. Type consistency:**
- `spawn_with_observer(parent: &Run, req: &SpawnRequest, backend: &dyn LlmBackend, cfg: &SpawnConfig, observer: &dyn PlannerObserver) -> SpawnResponse` consistent across Task 2 Step 3 (def) and Task 4 Step 2 (call via `spawn_one_with_observer` alias).
- `spawn_parallel_with_observer(parent: &Run, requests: &[SpawnRequest], backend: &dyn LlmBackend, cfg: &SpawnConfig, observer: &dyn PlannerObserver) -> Vec<SpawnResponse>` consistent across Task 3 Step 4 (def) and Task 4 Step 3 (call).
- `drive_prepared_child(p: PreparedChild, cfg: &SpawnConfig, observer: &dyn PlannerObserver)` consistent across Task 3 Step 3 (def), Step 4 (sync call), Step 5 (async call).
- `PlannerObserver: Send + Sync` consistent across Task 1 and all uses (Task 2/3/4).
- Import aliases (`spawn_one_with_observer`, `spawn_parallel_with_observer`) spelled identically in Task 4 Step 1 (import) and Task 4 Steps 2/3 (call sites).

**4. Spec-to-plan drift notes:**
- Spec §2.2 wrote `&(dyn PlannerObserver)` in one example and `&dyn PlannerObserver` in another. Plan uses `&dyn PlannerObserver` everywhere (no parens) for consistency with existing reverie style.
- Spec §3.1 mentions panic propagation via thread::scope — this is std library behavior we inherit, no code change needed, so no task line item for it. Called out in this review to confirm intentional.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-21-phase-1-5e-nested-subagent-streaming.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

**Which approach?**
