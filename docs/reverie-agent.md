# Reverie Agent (Phase 1)

The Reverie agent runs [reverie-deepagent](https://github.com/cerebral-work/reverie) in-process inside Zed. Selecting **Reverie** in the agent panel dispatches prompts through reverie's planner — a Rust port of LangChain's deepagents — using whichever language model you've configured as Zed's default.

## Prerequisites

- Your local `dreamcode` (this fork) and [`reverie`](https://github.com/cerebral-work/reverie) checkouts must live as siblings:

  ```
  programming projects/
    dreamcode/     <- this repo
    reverie/       <- cloned from cerebral-work/reverie
  ```

  The workspace's `reverie-deepagent` path dependency is resolved relative to this layout. If your checkouts live elsewhere, edit `reverie-deepagent = { path = "..." }` in the workspace `Cargo.toml`.

- A configured language model in Zed (Anthropic, OpenAI, Google, Ollama, etc.). The Reverie agent uses `LanguageModelRegistry::default_model()` at session-start time.

## Usage

1. Open the agent panel (`cmd-?` on macOS).
2. Pick **Reverie** from the agent selector.
3. Type a prompt. The planner will break it into todos, optionally write to a scratch vfs, and possibly spawn subagents. Each step appears live in the chat as an `AgentMessageChunk`.

## Memory (auto-retrieval)

When the reverie daemon is running on `localhost:7437`, the Reverie agent automatically:

1. **Before each prompt** — calls `GET /context/smart?q=<your prompt>&project=<project name>` with a 5s timeout. If the daemon returns relevant context, it's prepended to the prompt as `Relevant memory:\n<block>\n\n<your prompt>` and the agent panel shows a one-line breadcrumb: `[memory] consulted reverie (project=<name>)`.
2. **After each Completed run** — fires two fire-and-forget POSTs to `/observations/passive`:
   - `{ session_id, content: <your prompt>, project, source: "zed-agent-user-intent" }`
   - `{ session_id, content: <run summary>, project, source: "zed-agent-run-summary" }`

Non-Completed runs (MaxIterations, GaveUp, Backend error, EmptyCompletion, Cancelled) do NOT save — only clean terminations contribute to the corpus.

### Disabling

Memory integration has no explicit on/off switch; instead, it degrades silently when reverie is unreachable. To disable, point `REVERIE_URL` at a closed port:

```json
{
  "agent_servers": {
    "reverie": {
      "env": {
        "REVERIE_URL": "http://127.0.0.1:1"
      }
    }
  }
}
```

The first failed call per session logs `"reverie daemon unreachable at <url>, continuing without memory..."` at info level; subsequent failures log at debug only.

### Project name

Retrieval and save payloads are scoped to a project. Resolution order:
1. `agent_servers.reverie.env.REVERIE_PROJECT` in `settings.json`.
2. Shell env `REVERIE_PROJECT`.
3. First visible worktree's root directory name.
4. Literal `"unknown-project"` if no worktree is open.

## Persistent session state

Each agent panel thread keeps its own persistent planner state for the lifetime of the Zed session:

- **TodoList carries over.** When you send prompt 2 in the same thread, the planner sees the todos from prompt 1's final state. Pending, in-progress, and completed entries are all preserved. Phrase follow-ups like "keep going" or "update the status on todo 3" and the planner sees them in its initial state.
- **Scratch Vfs is stable.** Every session has its own scratch dir at `<zed-data-dir>/reverie-runs/<session_id>/`. Files the planner wrote in prompt 1 (via `vfs_write`) are readable in prompt 2.
- **LLM transcript does NOT carry over.** Each prompt gets a fresh transcript; the LLM is told "here are your current todos" and "here's your scratch" via reverie's state rendering. This is deliberate — it keeps token cost stable across long threads and matches how the deepagent is designed to operate.

### New thread = fresh state

Click the **+** button in the agent panel to start a new thread. The new thread gets its own `session_id`, its own scratch dir, and an empty TodoList.

### Phase 1.5c limitations

- **No cross-Zed-restart persistence.** Closing Zed throws away the in-memory TodoList; the scratch dir stays on disk but nothing points at it anymore. Phase B will serialize session state across restarts.
- **No cleanup of old scratch dirs.** `<zed-data-dir>/reverie-runs/` accumulates a directory per session indefinitely. If you need to reclaim space, delete the directory manually. Phase B adds retention.
- **No concurrent prompts on the same thread.** If you try to send a second prompt while the first is still running, you'll get `"a run is already in progress for this session; cancel it first"`. Use the panel's Cancel button, or start a new thread.
- **No UI to list or resume past sessions.** Each thread's scratch dir is named by `session_id`, which isn't surfaced in Zed's history view today. Phase C will address.

## What you'll see

The panel renders planner steps as inline chunks:

- `[add_todo] <description>` — new todo queued
- `[set_status] todo #N → <status>` — todo transitioned
- `[vfs_write] <path>\n---\n<snippet>` — scratch file written (content truncated at 200 chars for display)
- `[spawn] <persona> :: <task>` — subagent kicked off
- `[parallel_spawn] <siblings>` — fan-out spawn
- `[subagent <persona>] <Status>: <summary>` — each spawn completion
- `[note] ...`, `[give_up]`, `[noop]` — planner control signals
- Final line: `planner terminated: <reason> (iterations=N, todos_pending=K, spawns=M)`

When the planner spawns a subagent, the child's planner steps appear inline in the same chat — interleaved between the `[spawn]` breadcrumb and the `[subagent … Success]` terminal chunk. Parallel siblings' events interleave nondeterministically with each other; the `[spawn]` / `[subagent]` brackets still identify each block's persona.

## Settings

Reverie uses no custom settings beyond the shared `agent_servers` block. It accepts the same defaults other agents do and does not need API key plumbing — it reuses the language model you've already configured in Zed.

To select Reverie as your default:

```json
{
  "agent": {
    "default_agent": "reverie"
  }
}
```

## Canceling a run

Clicking **Stop** in the agent panel interrupts the run within milliseconds, including while an LLM call is streaming. Internally: a per-session `AtomicBool` is flipped (so the planner's iteration-top `should_stop` check catches it) AND a per-prompt `smol::channel` notifier fires so any pending await in the foreground driver (the `stream_completion_text` call, or a `stream.next()` chunk await) wakes immediately. The in-flight HTTP request is torn down via async drop; the planner's next call to the backend sees a transport error, and because `should_stop` is true at that moment, the run terminates with `TerminationReason::Cancelled` (not `Backend`).

Partial state is preserved (Phase 1.5c): whatever the `TodoList` had accumulated before cancel stays, and the scratch `Vfs` is untouched. Send a follow-up prompt in the same thread to continue.

### Subagent limit

The cancel notifier is installed only for the top-level prompt's foreground driver. A subagent's own nested LLM calls aren't interrupted mid-stream — the parent-level `should_stop` catches them at the next planner iteration boundary. Deep cancellation through subagents is future work.

## Known limitations

- **Session state is in-memory only.** See "Persistent session state" above for what carries across prompts (TodoList + Vfs) and what doesn't (LLM transcript, cross-restart state).
- **Subagent events interleave with parent events chronologically.** There's no UI grouping or indentation — a subagent's `[add_todo]` chunk appears in the same flat chat as the parent's. The `[spawn] <persona> :: <task>` breadcrumb before and `[subagent <persona>] <Status>: <summary>` after bracket each child's block for visual context.
- **Cancel interrupts the top-level run but not nested subagent streams.** Subagent LLM calls finish their current chunk before noticing cancellation. See "Canceling a run" above.
- **Retrieval is once-per-prompt, not per-iteration.** Memory is consulted at prompt start only. A future phase may add per-iteration or per-spawn retrieval.
- **No memory available to non-Reverie agents.** Claude / Gemini / Zed-native agents see no memory. Phase 1.5b (a universal `ReverieAugmentedConnection` wrapper) addresses this.
- **No explicit opt-out UI.** Disable by pointing `REVERIE_URL` at a closed port (see Memory section).
- **No subagent runtime isolation.** Subagents share Zed's language-model handle; if the model you picked rate-limits, all nested planners hit the same ceiling.

## Architecture at a glance

```
AgentPanel (Zed UI)
   │  new_session / prompt / cancel
   ▼
ReverieAgentConnection   ───►  foreground driver loop
   │  holds Arc<dyn LanguageModel>       │
   │                                      │ drives stream_completion_text
   │  cx.spawn (foreground)               ▼
   │                                   Zed LanguageModel
   ▼
smol::unblock (blocking pool)
   │  ZedLlmBackend ─► smol::channel ─► driver
   │  ChannelObserver ─► smol::channel ─► AcpThread updates
   ▼
reverie_deepagent::run_planner_with_observer
   │
   └─► DeepAgent planner loop
        AddTodo / SetStatus / VfsWrite / Spawn / ParallelSpawn / ...
```

The backend is on the blocking thread (reverie's planner loop is synchronous); the driver is on Zed's foreground executor (async LLM calls). Two `smol::channel` streams bridge them: one for LLM requests, one for session updates.

## References

- Implementation plan: `docs/superpowers/plans/2026-04-20-reverie-agent-backend.md`
- Reverie planner + observer: `reverie/crates/reverie-deepagent/src/planner.rs`
- Zed AgentServer trait: `crates/agent_servers/src/agent_servers.rs`
- Template for in-process server: `crates/agent/src/native_agent_server.rs`
