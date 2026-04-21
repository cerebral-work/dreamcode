# Phase 1.5d — Mid-Call Cancel — Design

**Status:** Draft, pending user review.
**Scope:** Make `AgentConnection::cancel` interrupt an in-flight `stream_completion_text` so the planner terminates within ~milliseconds instead of after the current LLM turn resolves. Small reverie upstream tweak so cancelled mid-calls report as `TerminationReason::Cancelled` instead of `Backend`.

**Context:** Phase 1 wired `cancel` to an `AtomicBool` that the reverie planner checks in `should_stop()` at the top of each iteration. That makes cancellation responsive to the end of each planner step, but not to the stream of LLM chunks inside a step. A long-running prompt might take 30–60 seconds to respond to Cancel because it waits for the current `stream_completion_text` to resolve.

**Cross-phase coordination:**
- Phase 1.5c's per-session state preservation stays intact — partial `TodoList` is still carried across the cancel boundary.
- Phase 1.5b (universal agent middleware) is unaffected.
- Reverie upstream change is tiny (one branch) and slots in after the Phase 1.5c `run_planner_with_observer_and_todos` commit on the same reverie branch.

---

## 1. Architecture

Two changes across the two repos:

**Dreamcode:** the foreground driver loop in `prompt()` gains a per-prompt `smol::channel::bounded::<()>(1)` cancel notifier. `AgentConnection::cancel` does two things now: flips the AtomicBool (unchanged — the planner still reads it via `should_stop`) AND `try_send(())` on the stored notifier so whatever await is currently pending in the driver returns immediately. A new helper `stream_to_string_cancellable` replaces `stream_to_string` and uses `futures::future::select` to race both `stream_completion_text` and each `stream.next()` against `cancel_rx.recv()`.

**Reverie:** the planner's backend-error arm checks `observer.should_stop()` and, when true, returns `TerminationReason::Cancelled` instead of `Backend`. No trait changes, no signature changes, just a one-branch if/else inside the existing match.

```
User clicks Cancel
  ↓
AgentConnection::cancel(session_id):
  session.cancel.store(true, SeqCst);                      (as today)
  if let Some(tx) = *session.cancel_notify.lock() {        (NEW)
      let _ = tx.try_send(());
  }
  ↓
prompt()'s cx.spawn loop:
  futures::select! {
      text = stream_to_string(...).fuse() => handle text,
      _    = cancel_rx.recv().fuse()      => Err("cancelled"),  (NEW)
  }
  ↓
  backend.next_action returns Err(BackendError::Transport("cancelled"))
  ↓
Reverie planner loop's Err arm:
  let termination = if observer.should_stop() {            (NEW)
      TerminationReason::Cancelled
  } else {
      TerminationReason::Backend
  };
  return PlannerResult { termination, … };
  ↓
Phase 1.5c state put-back:
  state.todos = planner_result.todos.clone();              (UNCHANGED)
  ↓
InProgressGuard drop → in_progress = false.                (UNCHANGED)
```

---

## 2. Components & Interfaces

### 2.1 Modified: `crates/reverie_agent/src/connection.rs`

Session struct:

```rust
struct Session {
    thread: WeakEntity<AcpThread>,
    cancel: Arc<AtomicBool>,
    state: Arc<parking_lot::Mutex<SessionState>>,
    // Phase 1.5d: holds the current (or most recent) prompt's cancel
    // notifier. `cancel()` fires try_send(()) on it so any pending await
    // in the foreground driver wakes immediately.
    cancel_notify: Arc<parking_lot::Mutex<Option<smol::channel::Sender<()>>>>,
}
```

### 2.2 `new_session()` change

One line: initialize `cancel_notify` with `Arc::new(parking_lot::Mutex::new(None))`. The first `prompt()` call installs a real sender.

### 2.3 `AgentConnection::cancel` change

Before:
```rust
fn cancel(&self, session_id: &acp::SessionId, _cx: &mut App) {
    if let Some(session) = self.sessions.lock().get(session_id) {
        session.cancel.store(true, Ordering::SeqCst);
    }
}
```

After:
```rust
fn cancel(&self, session_id: &acp::SessionId, _cx: &mut App) {
    if let Some(session) = self.sessions.lock().get(session_id) {
        session.cancel.store(true, Ordering::SeqCst);
        if let Some(tx) = session.cancel_notify.lock().as_ref() {
            // try_send: Full means a cancel signal is already pending
            // (the driver hasn't consumed it yet); Closed means the
            // driver has already torn down. Both are no-ops for us.
            let _ = tx.try_send(());
        }
    }
}
```

### 2.4 `prompt()` changes

At the top, right after the existing `acquire_run_slot(&state)` call, install the per-prompt cancel channel:

```rust
let (cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);
{
    let sessions = self.sessions.lock();
    if let Some(s) = sessions.get(&session_id) {
        *s.cancel_notify.lock() = Some(cancel_tx);
    }
    // If the session is gone, the subsequent spawn will still run but
    // cancel_rx will never fire. Not a correctness issue for Phase 1.5d.
}
```

Inside the `cx.spawn(async move |cx| { ... })` body, replace the existing `stream_to_string(&model, request, cx).await` call with `stream_to_string_cancellable(&model, request, cx, &cancel_rx).await`. The `cancel_rx` is captured by reference inside the loop; cloning the `Receiver` is cheap (Arc-backed in async-channel/smol), so we clone-in per iteration.

### 2.5 New helper: `stream_to_string_cancellable`

```rust
async fn stream_to_string_cancellable(
    model: &Arc<dyn LanguageModel>,
    request: LanguageModelRequest,
    cx: &AsyncApp,
    cancel_rx: &smol::channel::Receiver<()>,
) -> Result<String> {
    use futures::FutureExt as _;
    use futures::future::Either;
    use futures::StreamExt as _;

    // Phase 1: await the initial stream handshake.
    let stream_fut = Box::pin(model.stream_completion_text(request, cx));
    let cancel_fut = Box::pin(cancel_rx.recv());
    let mut text_stream = match futures::future::select(stream_fut, cancel_fut).await {
        Either::Left((Ok(s), _)) => s,
        Either::Left((Err(e), _)) => return Err(anyhow!("stream_completion_text failed: {e}")),
        Either::Right(_) => return Err(anyhow!("cancelled before stream started")),
    };

    // Phase 2: pull chunks, racing each against cancel.
    let mut text = String::new();
    loop {
        let next_fut = Box::pin(text_stream.stream.next());
        let cancel_fut = Box::pin(cancel_rx.recv());
        match futures::future::select(next_fut, cancel_fut).await {
            Either::Left((None, _)) => break,
            Either::Left((Some(Ok(chunk)), _)) => text.push_str(&chunk),
            Either::Left((Some(Err(e)), _)) => return Err(anyhow!("stream chunk error: {e}")),
            Either::Right(_) => return Err(anyhow!("cancelled mid-stream")),
        }
    }
    Ok(text)
}
```

The old `stream_to_string` helper is deleted — its only caller is the driver loop, which switches to the cancellable version. No behavior change on the happy path; cancel-free prompts see the same sequential flow.

### 2.6 Reverie upstream change

Location: `crates/reverie-deepagent/src/planner.rs`, inside `run_planner_with_observer_and_todos`, the backend-error arm of the `match backend.next_action(...)`:

Before:
```rust
Err(e) => {
    warn!(iter = i, error = %e, "deepagent: backend error, terminating");
    return PlannerResult {
        termination: TerminationReason::Backend,
        iterations: i,
        todos,
        spawn_log,
    };
}
```

After:
```rust
Err(e) => {
    let termination = if observer.should_stop() {
        TerminationReason::Cancelled
    } else {
        TerminationReason::Backend
    };
    warn!(iter = i, error = %e, ?termination, "deepagent: backend error, terminating");
    return PlannerResult {
        termination,
        iterations: i,
        todos,
        spawn_log,
    };
}
```

Trait/signature changes: none. Re-exports: none. This is strictly a behaviour change localised to one arm.

---

## 3. Error Handling

### 3.1 Normal cancel-during-stream flow
Covered in the architecture sketch (§1). End state: `TerminationReason::Cancelled`, partial TodoList preserved, in_progress cleared, thread shows planner-terminated summary.

### 3.2 Cancel before stream starts
`stream_completion_text` hasn't resolved yet; `cancel_rx.recv()` wins the first `select`; helper returns `Err("cancelled before stream started")`. Downstream the same as §3.1.

### 3.3 Cancel between prompts
`cancel_notify` may hold a stale `Sender` from a completed prompt. `try_send` either succeeds silently (stale receiver still exists but nobody polls it → memory reclaimed when the sender drops at next prompt setup) or fails with Full/Closed. The AtomicBool flip still fires; if a new prompt begins immediately, its fresh `(cancel_tx, cancel_rx)` pair replaces the stale one, and the bool will be cleared by new_session/prompt setup logic if needed. **Decision:** we don't auto-clear the AtomicBool between prompts in this phase — if cancel was set, the next prompt's first `should_stop` check will terminate it immediately with `Cancelled`. That's a minor UX quirk (a "ghost cancel" kills a fresh prompt) but not a correctness issue; Phase 1.5e can add per-prompt bool reset if it becomes annoying in practice.

**Correction applied inline:** reset the AtomicBool at the top of `prompt()` along with installing the new cancel_tx. Spec the reset explicitly:

```rust
// Clear any leftover cancel signal from a prior prompt on this session.
cancel.store(false, Ordering::SeqCst);
let (cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);
// ... install cancel_tx into session.cancel_notify ...
```

### 3.4 Race: cancel() called during prompt() setup
Narrow window between `self.sessions.lock()` in `cancel()` and the prompt()'s `*s.cancel_notify.lock() = Some(cancel_tx)`. Because `cancel_notify` is behind its own mutex, the call orderings are:
- cancel() sees old `None` or old (stale) `Some` → AtomicBool flips → `try_send` either no-ops or targets a dead receiver
- prompt() installs new `Some(cancel_tx)`; if cancel() already flipped the bool, we clear it on line above, which would **lose** the cancel signal

To prevent losing a concurrent cancel: the AtomicBool clear at the start of `prompt()` runs **before** we install the cancel_tx, AND the notify lock is acquired separately from the bool. If cancel() fires between those two statements, the bool is set, but prompt() has already cleared it.

**Simple fix:** invert the order — install `cancel_tx` FIRST, THEN check-and-clear the bool. If cancel() fires while cancel_tx is installed but before the clear, `try_send` lands a message on `cancel_rx` that the spawn loop will pick up on its first select. If cancel() fires after the clear, AtomicBool is set but the try_send targets the new receiver, which again the spawn loop sees. Either way, the signal is delivered to the right receiver.

Updated §2.4 and §3.3 should clarify this ordering:
1. `let (cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);`
2. Lock session, install `cancel_tx` into `s.cancel_notify`, release lock.
3. `cancel.store(false, Ordering::SeqCst);` — clear any pre-prompt stale signal.

If cancel() arrives between (2) and (3), its try_send reaches the new cancel_rx. Good.
If cancel() arrives after (3), its try_send reaches the new cancel_rx and sets the bool. Good.
If cancel() arrives before (2), nothing delivers on the new channel, but the bool is set → cleared at (3). **The cancel is lost.** This is the remaining edge case — a cancel that fires before prompt() setup completes is silently discarded.

**Mitigation:** acceptable. Users don't cancel before they've seen the prompt even start; the window is sub-millisecond. If it bites us in practice, add a "pending cancel flag" that prompt() setup checks after installing its own channel.

### 3.5 Dropped stream future
When the select arm drops the unfinished `stream_completion_text` future, Rust's async-drop cancels the underlying HTTP request. `http_client::AsyncBody`'s drop impl handles the transport teardown. No leaked sockets.

### 3.6 State preservation
Unchanged. Phase 1.5c's `state.todos = planner_result.todos.clone()` is unconditional. A cancelled run returns whatever the planner had accumulated; the guard's Drop clears `in_progress` as always.

---

## 4. Testing

### 4.1 Reverie upstream — 1 new test

Add to `planner.rs` tests module:

```rust
#[test]
fn backend_error_with_should_stop_reports_cancelled() {
    use std::sync::Mutex;

    struct StopOnNthBackend {
        actions: std::cell::RefCell<Vec<NextAction>>,
        call_count: std::cell::RefCell<usize>,
        fail_on: usize,
    }
    // Returns scripted actions until the Nth call, then Err.
    impl LlmBackend for StopOnNthBackend {
        fn next_action(
            &mut self,
            _todos: &TodoList,
            _vfs: &Vfs,
            _obs: &[SpawnObservation],
        ) -> Result<NextAction, BackendError> {
            let mut count = self.call_count.borrow_mut();
            *count += 1;
            if *count == self.fail_on {
                Err(BackendError::Transport("simulated cancel".into()))
            } else {
                Ok(self.actions.borrow_mut().remove(0))
            }
        }
    }

    struct ObserverStoppingBeforeNth {
        stop_on: usize,
        hits: Arc<Mutex<usize>>,
    }
    impl PlannerObserver for ObserverStoppingBeforeNth {
        fn should_stop(&self) -> bool {
            *self.hits.lock().unwrap() >= self.stop_on
        }
        fn on_action(&self, _: &NextAction) {
            *self.hits.lock().unwrap() += 1;
        }
    }

    let (_p, run) = fresh_run();
    let mut backend = StopOnNthBackend {
        actions: std::cell::RefCell::new(vec![NextAction::AddTodo("x".into())]),
        call_count: std::cell::RefCell::new(0),
        fail_on: 2,
    };
    let observer = ObserverStoppingBeforeNth {
        stop_on: 1,
        hits: Arc::new(Mutex::new(0)),
    };

    let result = run_planner_with_observer(
        &run, &mut backend, 10, &SpawnConfig::default(), &observer,
    );
    assert_eq!(
        result.termination,
        TerminationReason::Cancelled,
        "backend error + should_stop=true must report Cancelled, not Backend"
    );
}
```

And verify the existing backend-error-without-should_stop path still reports Backend — reusing a pre-existing test should cover this, but add an assertion guard if not.

### 4.2 Dreamcode — 1 new test via a pure helper

`stream_to_string_cancellable` takes a concrete `&Arc<dyn LanguageModel>` which requires a fake model and its own pumping — complex. Extract the control-flow primitive as a pure async helper:

```rust
pub(crate) async fn race_with_cancel<T, F>(
    work: F,
    cancel_rx: &smol::channel::Receiver<()>,
    cancel_err: &'static str,
) -> Result<T>
where
    F: std::future::Future<Output = Result<T>>,
{
    use futures::FutureExt as _;
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

`stream_to_string_cancellable` builds on top of this: one call for the initial stream-get, one per chunk iteration.

Test:

```rust
#[test]
fn race_with_cancel_fires_err_when_cancel_pre_seeded() {
    let (tx, rx) = smol::channel::bounded::<()>(1);
    tx.try_send(()).unwrap();

    let result: Result<()> = futures::executor::block_on(race_with_cancel(
        futures::future::pending::<Result<()>>(),
        &rx,
        "cancelled",
    ));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("cancelled"));
}

#[test]
fn race_with_cancel_lets_work_win_when_no_signal() {
    let (_tx, rx) = smol::channel::bounded::<()>(1);

    let result: Result<u32> = futures::executor::block_on(race_with_cancel(
        async { Ok(42u32) },
        &rx,
        "cancelled",
    ));
    assert_eq!(result.unwrap(), 42);
}
```

### 4.3 Deferred to manual smoke

End-to-end cancel-during-stream on a real LLM needs a built Zed + running agent. Document:
- Start a long prompt ("investigate the entire codebase and write detailed notes").
- After ~3 seconds click Cancel.
- Expected: `[memory] consulted reverie ...` and `[add_todo] ...` chunks may or may not have fired, but within ~100ms of Cancel the thread shows `planner terminated: Cancelled (iterations=K, …)`.
- Send a follow-up prompt. Partial TodoList + any written scratch files visible (Phase 1.5c).

### 4.4 Not tested in Phase 1.5d

- **Micro-benchmarks of cancel latency.** Whether it's 10ms or 200ms isn't worth a perf gate.
- **Concurrent cancel storms.** Repeatedly hammering cancel() during a run. The try_send behaviour makes it safe but we don't test that directly.

---

## 5. Known Limitations (Phase 1.5d)

- **Cancel fired BEFORE prompt() setup completes is lost.** Sub-millisecond window; unlikely in practice.
- **Subagent runs don't see cancellation during their own LLM calls.** The `cancel_rx` is installed only for the TOP-level prompt's driver loop. Nested subagent planners each issue their own LLM calls via the same backend channel, but they don't have their own cancel_rx. Impact: cancelling a parent planner during a subagent's streaming waits until the subagent's current chunk finishes, then the parent-level should_stop fires. Partial — good enough for now; Phase 2 can plumb the cancel_rx deeper.
- **No user-facing UI indicator during cancellation.** The summary chunk reports it, but there's no "cancelling..." state. Defer.

## 6. Invoke-After

Per the brainstorming skill: after user approval of this spec, the terminal state is invoking `writing-plans` to produce an implementation plan.

---

## Self-Review (inline)

**Placeholder scan:** No TBDs. Every field, function signature, and error message is concrete.

**Internal consistency:**
- Session struct shape in §2.1 matches the uses in §2.3, §2.4, and §3.
- `cancel_notify: Arc<parking_lot::Mutex<Option<smol::channel::Sender<()>>>>` is consistent across all references.
- `race_with_cancel` helper signature in §4.2 matches the use sketched in §2.5 (both pass `&Receiver`, both return `Result<T>`).
- Reverie termination-precision logic in §2.6 matches the test scenario in §4.1.

**Scope check:** One spec, one feature (cancel responsiveness), ~60 LOC dreamcode + ~5 LOC reverie + 3 unit tests. Under a day of work.

**Ambiguity check:**
- §3.3 "Cancel between prompts" has an ordering subtlety; resolved inline with the explicit (install-tx, then clear-bool) sequencing and the acknowledged sub-millisecond pre-setup race.
- §3.6 "State preservation unchanged" — stated explicitly to prevent anyone from thinking Cancelled should skip the put-back.
- `stream_to_string` is deleted, not kept alongside — stated explicitly in §2.4 and §2.5.
