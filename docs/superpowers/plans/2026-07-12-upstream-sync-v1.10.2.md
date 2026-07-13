# Upstream Zed Sync to v1.10.2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Merge upstream Zed `v1.10.2` into the `cerebral-work/dreamcode` fork, reaching a clippy-clean, smoke-passing tree, and record the sync in the phase docs.

**Architecture:** One catch-up PR on `chore/sync-upstream-v1.10.2` (already created off `main`, spec committed). Resolve 5 merge conflicts with predetermined resolutions, then iterate a compile-fix loop over API drift (Serena LSP for navigation, `./script/clippy` for the authoritative gate), then smoke test, then revise docs.

**Tech Stack:** Rust (Zed workspace), GPUI, Cargo, `./script/clippy`, git, Serena LSP tools.

**Spec:** `docs/superpowers/specs/2026-07-12-upstream-sync-v1.10.2-design.md`

## Global Constraints

- Base ref: upstream tag `v1.10.2` (already fetched via the `upstream` remote → `https://github.com/zed-industries/zed`).
- Authoritative gate: `./script/clippy` (= `cargo clippy --workspace --release --all-targets --all-features -- --deny warnings`) exits 0. Never substitute bare `cargo clippy`.
- Final gate: smoke test per `docs/smoke-test-runbook.md` passes.
- No CI validates this work (fork CI is org-gated off — CER-1601). Local gates only.
- Rust rules (repo CLAUDE.md): no `unwrap()`/panic; propagate with `?`; no `let _ =` on fallible ops; full-word names; never create `mod.rs`.
- Reverie-integration changes are **minimal and behavior-preserving**. Record every non-trivial adaptation for the PR body / roadmap doc.
- Work only on branch `chore/sync-upstream-v1.10.2`. Do not touch `main` or the PR #9 branch.
- The stray working-tree `.gitignore` change is stashed; do not restore it into any port commit.

---

### Task 1: Merge v1.10.2 and resolve all conflicts

**Files:**
- Modify (resolve): `Cargo.toml`
- Modify (resolve): `crates/agent_ui/src/agent_ui.rs`
- Modify (resolve): `crates/agent_ui/src/agent_panel.rs`
- Modify (resolve): `crates/agent_ui/src/thread_import.rs`
- Delete (resolve): `.github/workflows/track_duplicate_bot_effectiveness.yml`

**Interfaces:**
- Produces: a committed merge commit on `chore/sync-upstream-v1.10.2` with zero conflict markers and a clean index. Later tasks assume `Agent::ReverieAgent`, `reverie_agent::REVERIE_AGENT_ID`, and `Agent::server_without_augment` still exist (they are ours; drift tasks confirm).

- [ ] **Step 1: Start the merge**

```bash
cd /Volumes/Containers/dreamcode
git status --short   # expect clean (only untracked .remember/ .serena/)
git merge --no-edit v1.10.2
```
Expected: `Automatic merge failed; fix conflicts` with 5 unmerged paths.

- [ ] **Step 2: Resolve the deterministic deletion**

We removed community bots in PR #2; keep it deleted.
```bash
git rm -q .github/workflows/track_duplicate_bot_effectiveness.yml
```

- [ ] **Step 3: Resolve `Cargo.toml` — keep both deps**

In the conflict at `[workspace.dependencies]`, replace the whole
`<<<<<<< … >>>>>>>` block with both entries in alphabetical order (`resvg`
before `reverie-deepagent`):
```toml
resvg = { version = "0.46.0", default-features = false, features = [
    "text",
    "system-fonts",
    "memmap-fonts",
    "raster-images",
] }
reverie-deepagent = { path = "../reverie/crates/reverie-deepagent" }
```

- [ ] **Step 4: Resolve `agent_ui.rs` imports — union**

Replace the imports conflict block with (keep `Project`, adopt upstream's additions):
```rust
use project::{AgentId, DisableAiSettings, Project};
use prompt_store::{self, PromptBuilder, rules_to_skills_migration};
use rope::Point;
```
Note: if `Project` turns out unused after the port, clippy's `-D warnings` will
flag it in Task 6; remove it then, do not pre-guess here.

- [ ] **Step 5: Resolve `agent_ui.rs` `From<AgentId> for Agent` — adopt upstream early-return, add ReverieAgent**

Make the function read exactly:
```rust
    fn from(id: AgentId) -> Self {
        if id.as_ref() == agent::ZED_AGENT_ID.as_ref() {
            return Self::NativeAgent;
        }
        if id.as_ref() == reverie_agent::REVERIE_AGENT_ID.as_ref() {
            return Self::ReverieAgent;
        }
        #[cfg(any(test, feature = "test-support"))]
        if id.as_ref() == "stub" {
            return Self::Stub;
        }
        Self::Custom { id }
    }
```

- [ ] **Step 6: Resolve `agent_panel.rs` hunk 1 — take upstream's ExtensionStore subscription**

Upstream's own `connection_store` is created immediately below the conflict
(`let connection_store = cx.new(|cx| AgentConnectionStore::new(project.clone(), cx));`),
and native-agent registration now happens via `ensure_native_agent_connection`
on worktree events. So **accept the upstream side** of this hunk (the
`ExtensionStore::try_global(cx).map(|store| … migrate_agent_server_from_extensions …)`
block) and delete the HEAD side (the `ExtensionEvents` subscription + inline
`AgentConnectionStore::new` / `request_connection(Agent::NativeAgent, …)` block).

- [ ] **Step 7: Resolve `agent_panel.rs` hunk 2 — union the menu items**

The conflict is two additive context-menu entries. Keep **both**: HEAD's
`.item(ContextMenuEntry::new("Reverie")…)` block, followed by upstream's
`.when(supports_terminal, |menu| { menu.item(ContextMenuEntry::new("Terminal")…) })`
block. Remove the markers; keep Reverie first, then Terminal.

- [ ] **Step 8: Resolve `thread_import.rs` — re-express the augment-skip over upstream's flow**

Read upstream's version of the function first:
```bash
git show v1.10.2:crates/agent_ui/src/thread_import.rs | sed -n '680,740p'
```
Adopt upstream's loop/closure structure (the `let agent_id = agent_id.clone();`
capture and `move |result| (agent_id, remote_connection, result)` naming), but
replace the server construction with our non-augmenting variant and keep the
explanatory comment:
```rust
            // Thread import doesn't have a Project handle in scope; skip
            // reverie memory augmentation for this path.
            let server = agent.server_without_augment(<dyn Fs>::global(cx), ThreadStore::global(cx));
```
The rest of the loop (building `entry` via `store.request_connection(agent, server, cx)`
and pushing to `wait_for_connection_tasks`) follows upstream's shape.

- [ ] **Step 9: Verify no markers remain and commit the merge**

```bash
git diff --check                      # expect no conflict-marker output
git grep -nE '^(<<<<<<<|=======|>>>>>>>)' -- ':!docs' || echo "no markers"
git add -A
git commit --no-edit
```
Expected: merge commit created. (Tree will NOT compile yet — drift tasks follow.)

---

### Task 2: Fix `ReverieAgent` non-exhaustive matches

**Files:**
- Modify: `crates/agent_ui/src/**` (sites found by Serena), possibly other crates that `match` on `Agent`.

**Interfaces:**
- Consumes: `Agent::ReverieAgent` variant (present since it is ours).
- Produces: every `match self { … }` / `match agent { … }` over `Agent` handles `ReverieAgent`.

- [ ] **Step 1: Enumerate match sites**

Use Serena `find_referencing_symbols` on the `Agent` enum (in
`crates/agent_ui/src/agent_ui.rs`) and on the `ReverieAgent` variant. Cross-check:
```bash
git grep -nE 'Self::(NativeAgent|Custom)\b|Agent::(NativeAgent|Custom)\b' -- crates | wc -l
```
List each `match` that produces a value or behavior per-variant.

- [ ] **Step 2: Add a `ReverieAgent` arm to each**

For each match, add an arm mirroring the closest existing arm's shape. Reference
the pre-merge HEAD version to recover the original ReverieAgent behavior:
```bash
git show origin/main:crates/agent_ui/src/agent_ui.rs | grep -n 'ReverieAgent'
```
Do not invent behavior — reproduce what HEAD did for `ReverieAgent`, adapted to
upstream's arm signature.

- [ ] **Step 3: Verify the agent_ui crate advances past these errors**

Run: `./script/clippy -p agent_ui 2>&1 | grep -E 'non-exhaustive|ReverieAgent' || echo "no non-exhaustive left"`
Expected: no remaining non-exhaustive/`ReverieAgent` match errors (other errors may remain for later tasks).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "agent_ui: handle ReverieAgent in match arms after v1.10.2 merge"
```

---

### Task 3: Fix the `ExtensionEvents` → `ExtensionStore` rename fallout

**Files:**
- Modify: `crates/agent_ui/src/agent_panel.rs` and any other referrers Serena finds.

**Interfaces:**
- Produces: no references to removed symbols `ExtensionEvents`, `sync_agent_servers_from_extensions`; behavior routed through upstream's `ExtensionStore` / `migrate_agent_server_from_extensions` / `ensure_native_agent_connection`.

- [ ] **Step 1: Find dangling references**

```bash
git grep -nE 'ExtensionEvents|sync_agent_servers_from_extensions' -- crates
```
Also use Serena `find_referencing_symbols` on any of our methods that called the
old API.

- [ ] **Step 2: Adapt or remove each site**

If a site is ours (e.g. a helper that called `sync_agent_servers_from_extensions`),
route it to upstream's replacement or delete it if superseded by
`ensure_native_agent_connection`. Keep changes minimal; record any non-trivial
choice in a scratch note for the PR body.

- [ ] **Step 3: Verify**

Run: `./script/clippy -p agent_ui 2>&1 | grep -E 'ExtensionEvents|sync_agent_servers' || echo "clean"`
Expected: `clean`.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "agent_ui: route extension events through upstream ExtensionStore"
```

---

### Task 4: Fix `reverie_agent` crate drift

**Files:**
- Modify: `crates/reverie_agent/**` as needed.

- [ ] **Step 1: Compile the crate in isolation**

Run: `./script/clippy -p reverie_agent`
Read each error. For each, use Serena `find_symbol` / `goToDefinition` on the
Zed API it calls to find the new signature/path.

- [ ] **Step 2: Fix drift minimally**

Update call sites to the new upstream APIs (mirror how upstream's own callers use
them — find one with Serena `find_referencing_symbols`). No behavior redesign;
preserve the crate's existing contract (`docs/reverie-agent.md`).

- [ ] **Step 3: Verify**

Run: `./script/clippy -p reverie_agent`
Expected: exit 0.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "reverie_agent: resolve upstream API drift for v1.10.2"
```

---

### Task 5: Fix `dream_inspector` crate drift

**Files:**
- Modify: `crates/dream_inspector/**` as needed.

- [ ] **Step 1: Compile the crate in isolation**

Run: `./script/clippy -p dream_inspector`

- [ ] **Step 2: Fix drift minimally**

Same method as Task 4: Serena to find new signatures, mirror upstream callers,
preserve existing behavior (panel registration in `zed` workspace init, SSE feed).

- [ ] **Step 3: Verify**

Run: `./script/clippy -p dream_inspector`
Expected: exit 0.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "dream_inspector: resolve upstream API drift for v1.10.2"
```

---

### Task 6: Whole-workspace clippy green (authoritative gate)

**Files:**
- Modify: any crate surfaced by the full workspace build (e.g. `crates/zed` registration, cross-crate call sites).

- [ ] **Step 1: Run the full gate**

Run: `./script/clippy`
Read remaining errors/warnings (`-D warnings` fails on any).

- [ ] **Step 2: Fix remaining drift**

Iterate: fix, re-run the affected `-p <crate>`, then the full gate. Includes any
unused import from Task 1 Step 4 (e.g. `Project`) — remove if flagged.

- [ ] **Step 3: Verify clean**

Run: `./script/clippy`
Expected: exit 0, no warnings.

- [ ] **Step 4: Regenerate `Cargo.lock` if needed and commit**

```bash
cargo update -w   # only if Cargo.lock is stale vs merged Cargo.toml
git add -A
git commit -m "Reach clippy-clean after v1.10.2 upstream merge"
```

---

### Task 7: Smoke test

- [ ] **Step 1: Run the runbook**

Follow `docs/smoke-test-runbook.md` end to end (build/run `zed`, exercise the
reverie_agent panel, dream_inspector feed). Note the app must build in release
per the runbook.

- [ ] **Step 2: Record results**

Capture pass/fail per runbook section into a scratch note for the PR body. If any
step fails, return to the relevant crate task; do not proceed on a red smoke.
Expected: all runbook sections pass.

---

### Task 8: Revise phase docs

**Files:**
- Modify: `docs/superpowers/plans/2026-05-02-remaining-phase-work.md`
- Modify: any `docs/superpowers/specs/*` whose described API changed (identified during Tasks 3–6).

- [ ] **Step 1: Add the v1.10.2 sync entry**

Under item #3 ("Upstream Zed sync"), add a dated sub-entry for the v1.10.2 sync
listing the concrete API-drift fixups made (mirroring PR #4's entry format:
`old API → new API`), and note that the monthly automation now tracks the latest
stable release tag (PR #9 / CER-1600) and that fork CI is tracked in CER-1601.

- [ ] **Step 2: Annotate drifted specs**

For each spec whose API changed, add a short "Updated for v1.10.2:" note stating
the new API. Do not rewrite the spec's history.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers
git commit -m "docs: record v1.10.2 upstream sync and API-drift fixups"
```

---

### Task 9: Open the PR

- [ ] **Step 1: Push the branch**

```bash
git push -u origin chore/sync-upstream-v1.10.2
```

- [ ] **Step 2: Open the PR (pin the fork repo)**

Assemble the body into a temp file first (avoids heredoc/stdin pitfalls), then
create the PR pinned to the fork (`--repo` is required — `gh` otherwise defaults
the base to the parent `zed-industries/zed`):
```bash
body="$(mktemp)"
{
  printf 'Syncs the fork with upstream Zed **v1.10.2** (latest stable tag).\n\n'
  printf '- Commits merged: **%s**\n' "$(git rev-list --count v1.10.2..HEAD 2>/dev/null || echo '~1305')"
  printf '- `./script/clippy` green; smoke test (docs/smoke-test-runbook.md) passed.\n'
  printf '- Fork CI is org-gated off (CER-1601), so validation was local.\n\n'
  printf '### Reverie integration adaptations\n'
  printf '%s\n' "$(cat /tmp/reverie-adaptations.md 2>/dev/null || echo '- (fill from the scratch note kept during Tasks 1/3)')"
  printf '\nRelease Notes:\n\n- Improved editor by syncing with upstream Zed v1.10.2\n'
} > "$body"
gh pr create --repo cerebral-work/dreamcode --base main \
  --head chore/sync-upstream-v1.10.2 \
  --title "Sync upstream Zed to v1.10.2" \
  --body-file "$body"
```

- [ ] **Step 3: Restore the stashed `.gitignore` noise onto `main`**

```bash
git checkout main
git stash list | grep -q 'hook gitignore' && git stash pop || echo "nothing to restore"
```

---

## Notes on verification philosophy

`./script/clippy` (`-D warnings`, release, all-targets, all-features) is the
authoritative compile gate; per-crate `-p` runs are the fast inner loop. Serena
LSP (`find_referencing_symbols`, `find_symbol`, `get_diagnostics_for_file`)
locates drift sites but does not certify — clippy does. Checkpoints to report:
merge-resolved (Task 1), first full clippy-green (Task 6), smoke-pass (Task 7).
