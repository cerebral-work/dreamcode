# Remaining Phase 0 / 1 / 2 Work

Date: 2026-05-02 (status updated 2026-05-06)
Scope: Snapshot of what is still outstanding across the dreamcode ↔ reverie integration phases. Use this as the single landing page for "what's next" before picking up the next chunk of work.

> **Status as of 2026-05-06: all listed phase tracks have landed on `main`.** The integration is feature-complete from the perspective of this plan. Future work belongs to new initiatives, not this checklist.

## Status at a glance

| Phase  | Track                                  | State            |
|--------|----------------------------------------|------------------|
| 0      | MCP context server (in `reverie`)      | Merged (reverie PR #325, 2026-05-03) |
| 1      | In-process reverie-deepagent backend   | Merged (PR #1)   |
| 1.5a   | Memory auto-retrieval                  | Merged           |
| 1.5b   | Universal memory middleware            | Merged           |
| 1.5c   | Per-session persistence                | Merged           |
| 1.5d   | Mid-call cancel                        | Merged           |
| 1.5e   | Nested subagent streaming              | Merged (reverie `34cfd21`, 2026-04-21) |
| 2      | Dream Inspector panel                  | Merged (PRs #1, #3) |

## Items, with resolutions

### 1. ~~Merge reverie Phase 0 PR~~ — DONE (2026-05-03)

- Repo: `cerebral-work/reverie`
- Branch: `feat/phase-0-mcp-context-server`
- PR: [#325 — feat(mcp): expand /mcp surface to six tools (Phase 0)](https://github.com/cerebral-work/reverie/pull/325) — **merged**.
- Tools shipped: `search_memory`, `smart_context`, `add_observation`, `add_observation_passive`, `dream_status`, `dream_last_report`.

### 2. ~~Implement Phase 1.5e — nested subagent streaming~~ — DONE (2026-04-21)

- Reverie commit: `34cfd21 deepagent: split spawn into no-observer delegate + spawn_with_observer` (on `main`).
- The implementation threads the parent's `PlannerObserver` into spawned children via `spawn_with_observer` / `spawn_parallel_with_observer`. Child planner `NextAction` events fire on the parent's observer, so dreamcode's `ChannelObserver` ships them as `AgentMessageChunk`s — child `[add_todo]`, `[set_status]`, `[vfs_write]` chunks interleave between the parent's `[spawn]` breadcrumb and `[subagent ... Status]` terminal chunk.
- Confirmed by the `spawn_with_observer_forwards_child_actions` test in `crates/reverie-deepagent/src/subagent.rs` and by manual smoke (long architecture-prompt produces interleaved child chunks).
- Dreamcode-side docs already describe the behavior: `docs/reverie-agent.md:128`.
- The dedicated plan doc `docs/superpowers/plans/2026-04-21-phase-1-5e-nested-subagent-streaming.md` still has its 28 checkboxes unchecked — those are historical and weren't ticked off as the work landed; the code state is what's authoritative.

### 3. ~~Upstream Zed sync~~ — DONE (PR #4, 2026-05-04)

- Branch `chore/sync-upstream-zed` was merged into `main` as PR #4.
- Sync hygiene fixups for upstream API drift (`acp::*` → `agent_client_protocol::schema::*`, `AgentPanel::create_thread` removal, `Agent::server` signature change) included in the same PR.

## References

- Phase 0 plan / design:
  - `docs/superpowers/plans/2026-04-29-phase-0-mcp-context-server.md`
  - `docs/superpowers/specs/2026-04-29-phase-0-mcp-context-server-design.md`
- Consolidated smoke-test runbook for all phases: commit `d309ae3 docs: consolidated smoke-test runbook for all phases`.
