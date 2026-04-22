# Dreamcode ↔ Reverie Smoke Test Runbook

One-shot manual verification across all six phases (1, 1.5a, 1.5b, 1.5c, 1.5d, 1.5e). Assumes a built Zed release binary at `target/release/zed` and reveried running on `localhost:7437`.

## 0. Preconditions

```bash
# Reveried is up
curl -s http://localhost:7437/health | jq '.status'
# Expected: "ok"

# Seed context so retrieval has something to return
curl -s -X POST http://localhost:7437/observations \
  -H 'content-type: application/json' \
  -d '{"title":"smoke test seed","content":"Dreamcode wraps reverie deepagent as a Zed AgentServer. Phase 1.5c keeps TodoList persistent across prompts in the same thread.","project":"dreamcode","topic_key":"smoke/seed"}'

# Smart context returns it
curl -s "http://localhost:7437/context/smart?q=how+does+memory+work+in+dreamcode&project=dreamcode" | jq '.context' | head -c 200
```

## 1. Phase 1 — Reverie agent appears in the panel

1. Launch Zed from the worktree: `./target/release/zed "/path/to/some/project"`.
2. Open the agent panel (`cmd-?`).
3. Click the agent selector dropdown. **Expected:** "Reverie" appears as an option alongside Zed Agent, Claude, etc.

## 2. Phase 1.5a — Reverie agent uses memory

1. Select **Reverie** from the agent selector.
2. Prompt: `"what do you know about how memory works in dreamcode?"`
3. **Expected chat output** (approximate, depends on model):
   - A chunk: `[memory] consulted reverie (project=<your-project-name>)`
   - Planner's own step chunks: `[add_todo] ...`, `[set_status] ...`, etc.
   - Final summary: `planner terminated: Completed (iterations=N, todos_pending=0, spawns=0)`.
   - The LLM's scratch or todo entries should reference "TodoList persistent across prompts" or similar from the seeded observation.
4. After the run: `curl -s localhost:7437/observations/recent | jq '.[] | select(.source == "zed-agent-user-intent") | .content'`. **Expected:** your prompt text appears.

## 3. Phase 1.5c — Per-session persistence

In the **same thread** as Phase 1.5a's prompt:

1. Prompt: `"what todos do you have so far?"`
2. **Expected:** the planner sees the TodoList from the previous prompt (it'll reference the prior todos in its initial state rendering).
3. Verify the scratch dir:
   ```bash
   ls ~/Library/Application\ Support/Zed/reverie-runs/
   # Expected (macOS): a directory named after the thread's session UUID, with vfs files from prior prompts.
   ```
4. Click the **+** to start a new thread; repeat the same prompt. **Expected:** planner sees an empty TodoList (fresh session).

## 4. Phase 1.5d — Mid-call cancel

1. In a fresh thread with **Reverie**, prompt: `"generate a long detailed report on this project's architecture, including file-by-file analysis of every crate"`.
2. Watch for the planner to start streaming chunks (wait ~2-3 seconds).
3. Click the **Stop** button in the agent panel while an LLM response is mid-stream.
4. **Expected within ~milliseconds (not seconds):**
   - The summary chunk reads `planner terminated: Cancelled (iterations=N, ...)`.
   - Partial work (any `[add_todo]` or `[vfs_write]` chunks that fired before cancel) is preserved.
5. Send a follow-up prompt: `"continue from where you left off"`. **Expected:** planner sees the partial todos from the cancelled run.

## 5. Phase 1.5e — Nested subagent streaming

1. In a fresh thread with **Reverie**, prompt: `"break this task into three parallel subagents: one to research X, one to plan Y, one to draft Z. spawn them in parallel."`
2. **Expected:**
   - Parent planner emits `[spawn] <persona> :: <task>` or `[parallel_spawn] ...` chunks.
   - **Interleaved** between the spawn breadcrumbs and the `[subagent <persona>] Success: ...` completion chunks, you should see each child's own `[add_todo]`, `[set_status]`, `[vfs_write]` chunks.
   - Before Phase 1.5e, there'd be a gap with ONLY the breadcrumbs and no child-step visibility.

## 6. Phase 1.5b — Universal memory middleware

1. Switch the agent selector to **Zed Agent** (NOT Reverie).
2. Prompt: `"what do you know about dreamcode's architecture?"`
3. **Expected chat output:**
   - No `[memory] consulted reverie` breadcrumb (by design — middleware doesn't emit chunks).
   - The LLM's response should reference information from the seeded observations (since `Relevant memory:\n...` was prepended invisibly to the prompt).
4. Verify save:
   ```bash
   curl -s localhost:7437/observations/recent | jq '.[] | select(.source == "zed-augment-user-intent") | .content'
   ```
   **Expected:** the prompt text from step 2 appears.

## 7. Phase 1.5b — Per-agent opt-out

1. Add to Zed's `settings.json`:
   ```json
   {
     "agent_servers": {
       "claude-acp": {
         "env": { "REVERIE_AUGMENT": "0" }
       }
     }
   }
   ```
2. Restart Zed or start a new thread with Claude.
3. Prompt: `"what's in dreamcode?"`
4. **Expected:** no new `zed-augment-user-intent` entry in `/observations/recent`. The LLM response shouldn't reflect seeded context.

## 8. Graceful degradation when reveried is down

1. Stop reveried: `pkill reveried`.
2. In Zed, try a new prompt with either Reverie or Zed Agent.
3. **Expected:**
   - First failed call logs at `info` level: `"reverie daemon unreachable at http://localhost:7437, continuing without memory. Start reveried or set REVERIE_URL."`
   - The prompt still runs and returns a response — no UI error, no frozen UI.
   - Subsequent failures log at `debug` only (suppressed from default console output).

## Troubleshooting

- **Agent panel doesn't show "Reverie":** check `cargo check -p agent_ui` is clean; check `crates/agent_ui/src/agent_ui.rs` has `Agent::ReverieAgent` variant registered.
- **`[memory]` chunk never appears:** reveried might be returning empty context. `curl /context/smart?q=...&project=<name>` directly to verify.
- **Stop button hangs:** Phase 1.5d's cancel_notify channel should fire immediately. Check Zed's logs for `"reverie: observer update rejected"` or similar.
- **Scratch dir not created:** check `paths::data_dir()` on your platform; verify no permission issues.
