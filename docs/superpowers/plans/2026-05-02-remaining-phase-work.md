# Remaining Phase 0 / 1 / 2 Work

Date: 2026-05-02
Scope: Snapshot of what is still outstanding across the dreamcode ↔ reverie integration phases. Use this as the single landing page for "what's next" before picking up the next chunk of work.

## Status at a glance

| Phase  | Track                                  | State            |
|--------|----------------------------------------|------------------|
| 0      | MCP context server (in `reverie`)      | Merged (reverie PR #325, 2026-05-03) |
| 1      | In-process reverie-deepagent backend   | Merged (PR #1)   |
| 1.5a   | Memory auto-retrieval                  | Merged           |
| 1.5b   | Universal memory middleware            | Merged           |
| 1.5c   | Per-session persistence                | Merged           |
| 1.5d   | Mid-call cancel                        | Merged           |
| 1.5e   | Nested subagent streaming              | **Not started** (design + plan only) |
| 2      | Dream Inspector panel                  | Merged (PRs #1, #3) |

## Outstanding work

### 1. ~~Merge reverie Phase 0 PR~~ — DONE (2026-05-03)

- Repo: `cerebral-work/reverie`
- Branch: `feat/phase-0-mcp-context-server`
- PR: [#325 — feat(mcp): expand /mcp surface to six tools (Phase 0)](https://github.com/cerebral-work/reverie/pull/325) — **merged**.
- Tools shipped: `search_memory`, `smart_context`, `add_observation`, `add_observation_passive`, `dream_status`, `dream_last_report`.

### 2. Implement Phase 1.5e — nested subagent streaming

- Plan: `docs/superpowers/plans/2026-04-21-phase-1-5e-nested-subagent-streaming.md` (28 checklist items, 0 completed)
- Design: `docs/superpowers/specs/2026-04-21-phase-1-5e-nested-subagent-streaming-design.md`
- Goal: Surface nested subagent activity from reverie-deepagent through the existing `PlannerObserver` hook so the Zed agent panel can stream sub-step progress, not just top-level planner steps.
- This is the only Phase 1.5 sub-track with no implementation commits — every other 1.5 track is on `main`.
- Action: open a worktree off `main`, follow the existing implementation plan, ship behind the same `reverie_agent` integration that 1.5a–1.5d landed under.

### 3. (Out of scope reminder) Upstream Zed sync

- Branch `chore/sync-upstream-zed` carries a large pending diff that merges upstream Zed changes. This is **not** phase work — track it separately and do not bundle phase commits into it.

## Suggested order

1. Get reverie PR #325 across the line (small, blocks nothing else but unblocks future MCP-only consumers).
2. Implement 1.5e (the only remaining feature track).
3. Then the integration project is feature-complete; remaining work is upstream sync hygiene + whatever new asks come in.

## References

- Phase 0 plan / design:
  - `docs/superpowers/plans/2026-04-29-phase-0-mcp-context-server.md`
  - `docs/superpowers/specs/2026-04-29-phase-0-mcp-context-server-design.md`
- Consolidated smoke-test runbook for all phases: commit `d309ae3 docs: consolidated smoke-test runbook for all phases`.
