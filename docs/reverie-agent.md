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

Clicking **Stop** in the agent panel flips a per-session `AtomicBool` that the planner checks at the top of every iteration. The run terminates with `TerminationReason::Cancelled` on the next loop top. In-flight LLM calls continue until their `stream_completion_text` future resolves; cancellation granularity is per-iteration, not mid-call.

## Known Phase 1 limitations

- **No persistence between prompts.** Each prompt builds a fresh `Run` with an empty todo list and empty scratch vfs.
- **No live streaming within subagents.** The observer only fires on the top-level planner loop; nested subagent planners run to completion before their `SpawnObservation` ships.
- **Cancel is coarse.** It stops the *next* iteration, not the in-flight LLM call.
- **No memory integration yet.** The reverie daemon's `/search/v2`, `/context/smart`, and `/observations` endpoints are not queried from Zed in Phase 1 — a separate MCP context-server (Phase 0 of the broader plan) is the intended integration for memory retrieval.
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
