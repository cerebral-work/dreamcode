//! Dream Inspector — bottom-dock panel that shows a live feed of
//! reverie-daemon events (observations, retrievals, dream phases) by
//! polling the reveried `GET /events/recent` endpoint.
//!
//! See `docs/superpowers/specs/2026-04-22-phase-2-dream-inspector-design.md`
//! for the contract this crate implements.

pub mod categories;
pub mod feed;
pub mod http;
pub mod panel;

// pub use panel::DreamInspectorPanel;  // re-exported after Task B5

use gpui::App;

/// Called from `crates/zed/src/zed.rs` during app init to register the
/// panel's toggle action. Actual panel construction happens per-workspace
/// via [`DreamInspectorPanel::load`].
pub fn init(_cx: &mut App) {
    // Action registration is handled by the `actions!` macro in panel.rs;
    // this hook exists so the zed.rs init site has a stable surface even
    // if more global wiring is added later.
}
