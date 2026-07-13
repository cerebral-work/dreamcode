# Upstream Zed Sync to v1.10.2 — Design

Date: 2026-07-12
Status: Approved (design), implementation pending
Related: CER-1600 (sync automation), CER-1601 (fork CI enablement), PR #9 (workflow)

## Context

`cerebral-work/dreamcode` is a permanent Zed fork carrying ~71 divergent commits
(new crates `reverie_agent` and `dream_inspector`, vendored Cerebral themes, MCP
context docs). The last upstream sync was PR #4 (2026-05-04, from `main` at
`9155bf4`). As of 2026-07-12 the fork is **1,305 commits behind** the latest
stable upstream release tag **`v1.10.2`** (cut 2026-07-10).

This is the recurring "Upstream Zed sync" chore, run manually here because the
backlog exceeds what the monthly automation (`sync_upstream.yml`) is designed to
absorb — the automation resolves only deterministic conflicts and never
compile-fixes.

## Goals

- Merge upstream `v1.10.2` into the fork on a `chore/sync-upstream-v1.10.2`
  branch and open one reviewable PR against `main`.
- Reach a **building, clippy-clean** tree: `./script/clippy`
  (`cargo clippy --workspace --release --all-targets --all-features -- --deny warnings`)
  passes, and the smoke-test runbook (`docs/smoke-test-runbook.md`) passes.
- Preserve the Reverie integration's behavior; adapt it to upstream refactors
  with minimal, behavior-preserving changes.
- Revise the `docs/superpowers` specs and plans to reflect the new baseline.

## Non-goals

- Enabling fork CI (org-gated off; tracked separately in CER-1601). The done bar
  for this work is **local** `./script/clippy` + smoke, with no CI backstop.
- Redesigning the Reverie integration. Semantic adaptations are recorded, not
  reimagined.
- Syncing to bleeding-edge `upstream/main` (rejected in favor of the stable tag
  for a reproducible base).

## Base decision: stable tag, not `main`

Target the newest stable release tag (`v*` excluding `pre`/`rc`/`nightly`) —
`v1.10.2`. Versus `main` it is 1,305 vs 1,512 commits behind and has a cleaner
conflict set (one workflow modify/delete instead of five), while being only two
days old. The monthly automation is repointed to the same policy (PR #9).

## Approach

Single catch-up PR mirroring the established PR #4 (`chore/sync-upstream-zed`)
pattern: merge + conflict resolution + compile fixes + doc revisions in one PR.
Rejected: splitting code/docs PRs (docs are small, code PR is the bulk anyway),
and driving the catch-up through the workflow (it cannot compile-fix a
1,305-commit jump).

## Conflict resolution plan

The `git merge v1.10.2` produces 5 conflicts:

| File | Type | Resolution |
|------|------|------------|
| `.github/workflows/track_duplicate_bot_effectiveness.yml` | modify/delete | Keep deleted (`git rm`) — we removed community bots in PR #2. |
| `Cargo.toml` | content | Keep **both** deps: upstream's `resvg` and our `reverie-deepagent` (alphabetical: `resvg` before `reverie-deepagent`). |
| `crates/agent_ui/src/agent_ui.rs` | content | Union imports (keep `Project`, add upstream's `prompt_store::{self, …, rules_to_skills_migration}` and `rope::Point`); add a `ReverieAgent` early-return arm to upstream's rewritten `From<AgentId> for Agent`. |
| `crates/agent_ui/src/agent_panel.rs` | content | Take upstream's `ExtensionStore` subscription (our inline `NativeAgent` `request_connection` is superseded by upstream's `ensure_native_agent_connection` on worktree events); union the **Reverie** and **Terminal** context-menu items. |
| `crates/agent_ui/src/thread_import.rs` | content | Re-express our reverie-augment-skip (`server_without_augment`) over upstream's revised connection-request flow. |

Unused-import risk (e.g. `Project`) and behavior parity are confirmed by the
compile-fix loop, not assumed.

## Compile-drift strategy

The merge resolves textually but will not compile, because ~1,305 commits of API
drift do not surface as conflicts:

- The `ReverieAgent` enum variant merges cleanly, making upstream's rewritten
  `match self { … }` blocks (`id()`, `label()`, `is_native()`, …) non-exhaustive.
- The `ExtensionEvents` → `ExtensionStore` /
  `sync_agent_servers_from_extensions` → `migrate_agent_server_from_extensions`
  rename leaves dangling references.
- `reverie_agent` and `dream_inspector` call Zed APIs that moved.

Method: use Serena's LSP tools for **navigation** —
`find_referencing_symbols` on `Agent`, on the renamed APIs, and across the
custom crates to enumerate drift sites; `get_diagnostics_for_file` for a fast
per-file inner loop. `./script/clippy` remains the **authoritative** green (rust-
analyzer diagnostics can lag on a fresh merge). Fixes stay minimal and
behavior-preserving; every non-trivial Reverie adaptation is recorded.

## Verification (done bar)

1. `git merge` conflicts resolved, merge committed. **[checkpoint]**
2. `./script/clippy` exits 0. **[checkpoint]**
3. Smoke test per `docs/smoke-test-runbook.md` passes. **[checkpoint]**

No CI validates this (CER-1601). Checkpoints are reported rather than run dark.

## Docs revision

- `docs/superpowers/plans/2026-05-02-remaining-phase-work.md` — add a v1.10.2
  entry under item #3 listing the concrete API-drift fixups (as PR #4's entry
  did), and note the now-tag-based automation.
- Any `docs/superpowers/specs/*` whose described API changed during the port —
  identified while fixing drift, annotated with the new API.
- This design doc records the plan.

## Risks

- **Drift depth is unknown until the compiler talks.** The compile-fix loop is
  the unpredictable cost; checkpoints bound it.
- **Semantic calls in product code.** Adaptations to the Reverie integration are
  minimal and recorded for the owner to audit; no silent redesign.
- **No CI backstop.** Local `./script/clippy` + smoke is the only gate until
  CER-1601 lands.
