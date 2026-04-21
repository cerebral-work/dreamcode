//! Zed AgentServer implementation backed by the reverie-deepagent crate.
//!
//! Mirrors the in-process shape of `crate::agent::NativeAgentServer` —
//! no subprocess; the planner loop runs on a dedicated OS thread and
//! emits session updates back to the GPUI foreground via a channel.

mod backend;
mod connection;
mod observer;
mod server;

#[cfg(test)]
mod tests;

pub use server::{REVERIE_AGENT_ID, ReverieAgentServer};
