# Phase 2 — Dream Inspector Panel (design)

**Date:** 2026-04-22

**Status:** Draft — awaiting user review

## Goal

Add a bottom-dock panel in dreamcode (Zed fork) that shows a live, filterable
feed of reverie's memory-related events (observations captured, context
retrievals served, dream phases completed). Gives users visibility into what
memory is *doing* as agents work — complementing Phase 1's pipe that feeds
memory *into* prompts.

Phase 2 is the third leg of the dreamcode↔reverie integration:

- **Phase 1** — reverie deepagent as an in-process AgentServer (shipped).
- **Phase 1.5a/b/c/d/e/f** — retrieval/save middleware, cancel, subagent
  streaming, universal augment (shipped).
- **Phase 2 (this spec)** — Dream Inspector panel.

## Non-goals

- No write UI for the memory store. No manual dream triggers, observation
  edits, deletion flows. Panel is strictly read.
- No chunk/summary browser. That's the rejected "Memory Explorer" scope (B
  in brainstorming). May come later.
- No SSE / real-time streaming in v1. Polling-only. SSE is an optional
  upgrade path that keeps v1 clients working unchanged.
- No Redis client in Zed. Reveried proxies.

## Architecture

```
┌──────────── Zed (dreamcode fork) ─────────────┐          ┌─── reveried (new code) ───┐
│                                               │          │                           │
│   crates/dream_inspector                      │          │   dream_inspector module  │
│   ├── panel entity (Render + Panel impl)      │  HTTP    │   ├── GET /events/recent  │
│   ├── ring buffer (last 500 events)           │  poll    │   │   ?after=<id>&limit   │
│   ├── poll task (owned by panel entity)       │──────────│   │   &categories=...     │
│   │                                           │          │   │   → { events,        │
│   ├── filter category state (persisted)       │          │   │       next_after }    │
│   └── workspace dock registration             │          │   └── (Redis XREAD       │
│                                               │          │       backend, internal)  │
└───────────────────────────────────────────────┘          └───────────────────────────┘
```

- Only new HTTP surface is `/events/recent`.
- Zed gains no Redis dependency. Reveried reads Redis on Zed's behalf and
  translates its native `Event` enum into a stable wire shape (category + type
  + summary), so Zed does not break when upstream adds event variants.
- Panel's poll task lifetime is tied to the panel entity — closing the panel
  stops polling; no background traffic.

## Upstream addition (reverie)

Add one HTTP route. This is the only non-additive-to-Zed upstream change.

### Route

```
GET /events/recent?after=<redis_stream_id>&limit=<n>&categories=<csv>
  → 200 application/json
  {
    "events": [ WireEvent, ... ],
    "next_after": "<redis_stream_id>"
  }
```

- `after` — opaque cursor. First poll may omit; server returns up to `limit`
  most-recent events and a fresh `next_after`.
- `limit` — max events per response. Server caps at 200; default 100.
- `categories` — optional comma list. If present, server filters to the given
  categories before returning. Unknown categories are ignored (forward-compat).

### Wire shape

```json
{
  "id": "1776911878-0",          // redis XADD id, monotonic, opaque to Zed
  "ts_ms": 1776911878012,        // ms since unix epoch
  "category": "memory-io",       // one of the six listed below
  "type": "obs.capture",         // dot-prefixed within category
  "summary": "\"user prompt\" · source=zed-augment-user-intent",
  "fields": { /* decoded Event payload, opaque JSON */ }
}
```

### Category → Event-variant mapping (reveried-side)

| category      | reverie `Event` variants it covers                                                  | `type` strings                                           |
|---------------|-------------------------------------------------------------------------------------|----------------------------------------------------------|
| `memory-io`   | `ObservationCaptured`, `ContextRequest`, `ContextResponse`                          | `obs.capture`, `ctx.request`, `ctx.response`             |
| `dream`       | `PhaseStart`, `PhaseEnd`, `DreamPhaseCompleted`                                     | `phase.start`, `phase.end`, `dream.phase`                |
| `tx`          | `TxBegin`, `TxCommit`, `TxAbort`                                                    | `tx.begin`, `tx.commit`, `tx.abort`                      |
| `coord`       | `SessionRegistered`, `SessionDeregistered`, `LockAcquired`, `InboxDelivered`, `DispatchOrderIssued`, `PeerMessage`, `CoordOrder` | `session.registered`, `session.deregistered`, `lock.acquired`, `inbox.delivered`, `dispatch.order`, `peer.message`, `coord.order` |
| `gate`        | `GateDecision`                                                                      | `gate.decision`                                          |
| `permission`  | `PermissionRequest`, `PermissionGrant`                                              | `permission.request`, `permission.grant`                 |

Variants that don't map to any category (`LockReleased`, `BioFrame`, `WorkRequest`/`WorkCompleted`) are intentionally dropped on the wire side for v1 — they're either noise or not yet actionable. Future categories can be added without breaking clients.

### Error & empty responses

- Redis unreachable on the reveried side → `503 {"error":"events stream unavailable"}`.
- No new events since `after` → `200 {"events":[], "next_after":"<unchanged>"}`.

### Tests (reveried side)

- Parity: route shape matches spec (status codes, JSON keys, category values).
- Filtering: `categories=memory-io` excludes non-matching variants.
- Cursor: two sequential calls with the first's `next_after` don't return the
  same events twice.
- Redis-down: route returns 503 cleanly.

## Zed crate — `crates/dream_inspector`

### Layout

```
crates/dream_inspector/
├── Cargo.toml
├── src/
│   ├── dream_inspector.rs      # crate root, re-exports, init()
│   ├── panel.rs                # DreamInspectorPanel: Render + Panel impls
│   ├── feed.rs                 # FeedModel: entity holding ring buffer + poll state
│   ├── http.rs                 # client for GET /events/recent (mirrors ReverieHttpClient pattern)
│   └── categories.rs           # Category enum + defaults + settings serialization
└── .rules                      # only if non-obvious patterns land
```

No `mod.rs` files (per project CLAUDE.md).

### Registration

`init()` is called from `crates/zed/src/zed.rs` at app startup alongside the
terminal panel registration. It:

1. Registers the panel as a bottom-dock `Panel` via `workspace::Panel` trait.
2. Registers a `workspace::DeserializeItem` for state restore across restarts.
3. Registers a `dream_inspector::Toggle` action for keyboard binding.

### Cargo.toml dependencies

- `gpui` — UI
- `ui` — component primitives (matches Zed's design system)
- `workspace` — Panel trait, dock registration
- `http_client` — HTTP (same client used by `reverie_agent::ReverieHttpClient`)
- `settings` — for persisted category toggles
- `serde`, `serde_json`, `anyhow`, `log`, `smol`, `futures`

No Redis deps.

## Panel behavior

### Layout

- Bottom dock, tabbed alongside Terminal. Tab label: `Dream`. Tab icon:
  `IconName::Sparkle` or equivalent (match existing Zed dock iconography —
  final choice deferred to implementation).
- Visible when opened via action or dock toggle; never auto-opens.

### Content regions (top to bottom)

1. **Header bar** — category pills + control buttons (pause, clear, follow toggle).
2. **Feed** — virtualized scrollable list of events, newest at bottom when
   follow mode is on.
3. **Status line** — faded subtitle at bottom: `connected · 42 events` or an
   error banner when reveried is unreachable.

### Row design (compact one-liner)

```
HH:MM:SS  type           summary
12:34:22  obs.capture    "user prompt" · source=zed-augment-user-intent
12:34:22  ctx.request    q="yo what's up" · project=reverie-agent-backend
12:34:22  ctx.response   hits=3 · bytes=412
12:34:30  dream.phase    consolidate → Committed · 7 chunks
```

- Column widths: time (70px), type (90px), summary (flex, ellipsis on overflow).
- Type is colored by category:
  - `memory-io` → cyan
  - `dream` → magenta
  - `tx` → gray
  - `coord`, `gate`, `permission` → muted gray (only visible if toggled on)
- Monospace font (matches terminal panel's font).
- No wrapping — overflow truncates with ellipsis. Full payload available via
  expand (future v1.1) or terminal-style scroll-to-detail (future).

### Filter bar

- Six category pills: `memory-io`, `dream`, `tx`, `coord`, `gate`, `permission`.
- `memory-io` and `dream` lit (green accent) by default.
- Others muted gray by default.
- Click toggles state; state is persisted to Zed settings under
  `agent.reverie.dream_inspector.categories`.
- Toggling sends the new `categories` query param on the next poll.
- Client-side filtering also applies to already-buffered events so the UI
  updates immediately (no visible lag until next poll).

### Follow mode

- Top-right toggle, on by default.
- Auto-scroll to latest event when follow is on.
- Scrolling up manually disables follow.
- A "Jump to latest" button re-enables.

### Ring buffer

- Last 500 events client-side. Oldest dropped. No virtual-scroll beyond buffer.
- This is a debug feed, not an archive — if you want persistent history, query
  `/observations/recent` directly (different feature, not in scope).

## Polling

- **Cadence**: adaptive.
  - `1s` interval after a poll that returned ≥1 new event.
  - `3s` interval after an empty poll.
  - Immediate `1s` return on the next event arrival.
- **Pause toggle** in the filter bar stops polling entirely. Paused state
  persisted across sessions? No — pause is per-session.
- **Task lifetime**: the poll `Task` is owned by the panel entity (stored as
  a field). Dropping the entity cancels the task. No background polling when
  the panel is closed.
- **Cursor**: the feed holds the last `next_after` cursor and passes it on
  each subsequent call. Lost on panel restart — first poll after restart
  returns up to `limit` most-recent events as a seed.

## Error, empty, down states

| State | Visual |
|-------|--------|
| Fresh panel, no events yet | Centered `Waiting for events… (memory-io + dream)` |
| Connected, feed caught up, no new events | Bottom status line: `connected · 42 events · idle` |
| Reveried unreachable (network or 503) | Orange banner at top: `reverie daemon unreachable at http://localhost:7437. Retrying at current poll interval (1–3s adaptive).` Polling continues; existing buffered events stay visible. |
| Reveried returns 5xx | Same banner, different text: `reverie returned HTTP 500. Retrying at current poll interval (1–3s adaptive).` |
| Redis down on reveried side (503) | Same banner: `reverie events stream unavailable (Redis). Retrying at current poll interval (1–3s adaptive).` |

- On first error, one `log::info` line via a `first-fail` atomic (reuses the
  pattern from `reverie_agent::http::ReverieHttpClient::note_first_fail`).
  Subsequent failures log at `debug` until a success resets the flag.

## Settings integration

New settings key under the existing `agent.reverie` namespace:

```json
{
  "agent": {
    "reverie": {
      "url": "http://localhost:7437",
      "dream_inspector": {
        "categories": ["memory-io", "dream"]
      }
    }
  }
}
```

No new top-level namespace — dream inspector is a feature of the reverie
integration.

## Action & keybinding

- Action: `dream_inspector::Toggle` — opens/closes the panel in the bottom dock.
- No default keybinding shipped in v1; users bind via `keymap.json` if they want one.

## Open questions / deferred

- **SSE upgrade path**: add `GET /events/stream` later. Panel auto-detects via
  a capability probe or falls back to polling. Out of scope for v1 but the
  design leaves it clean: the panel already consumes a stream-of-events, so
  swapping transport is additive.
- **Detail view for a single event**: click a row → shows full `fields`
  payload in a side sheet. Deferred; one-liner is enough for v1.
- **Export / copy**: not in v1.
- **Toggling a category ON doesn't retroactively include past events**: because
  the server-side `categories=` filter is lossy — once the cursor has advanced
  past events that were filtered out server-side, those events are gone from
  the client's perspective. Observed during smoke: toggling `tx` ON didn't
  reveal an already-seeded `tx.commit` row; a freshly-seeded one did appear.
  Workable fix options for v1.1: (a) stop passing `categories=` to the server
  and filter purely client-side, (b) reset the client cursor when the filter
  set expands, (c) keep per-category cursors. (a) is simplest and matches the
  spec's "client-side filtering applies to already-buffered events" intent.
- **Filter-state persistence to Zed settings**: the spec promised
  `agent.reverie.dream_inspector.categories` persistence but v1 ships with
  ephemeral per-session filter state. Add in v1.1.

## Repositories touched

- **reverie** (`/Users/dennis/programming projects/reverie`) — one new route
  (§"Upstream addition"). A single PR there.
- **dreamcode** (this repo) — the `crates/dream_inspector` crate plus the
  init/registration hook in `crates/zed/src/zed.rs`. Single PR.

The two can land independently: reveried's new route is additive and
backward-compatible; the Zed panel is unreachable without it (panel will show
the "reverie daemon unreachable" state against a reveried lacking the route,
so there's no hard fail).

## Implementation phases (for the plan doc)

- **P2-A (reverie repo)**: new `GET /events/recent` route, wire-shape types,
  Event → category mapping, tests.
- **P2-B (dreamcode)**: `crates/dream_inspector` scaffold, HTTP client, ring
  buffer, feed model, adaptive poll.
- **P2-C (dreamcode)**: Panel UI, bottom-dock registration, row rendering,
  filter pills, follow mode.
- **P2-D (dreamcode)**: Settings persistence, `dream_inspector::Toggle`
  action, workspace restore.
- **P2-E (dreamcode)**: Error/empty/down states, first-fail logging, polish.
- **Smoke test**: end-to-end against live reveried with the new route
  deployed.
