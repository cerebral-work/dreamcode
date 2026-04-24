//! Zed AgentServer implementation backed by the reverie-deepagent crate.
//!
//! Mirrors the in-process shape of `crate::agent::NativeAgentServer` —
//! no subprocess; the planner loop runs on a dedicated OS thread and
//! emits session updates back to the GPUI foreground via a channel.

mod augment;
mod backend;
mod connection;
mod http;
mod observer;
mod server;

#[cfg(test)]
mod tests;

pub use augment::augment_with_memory;
pub use http::ReverieHttpClient;
pub use server::{REVERIE_AGENT_ID, ReverieAgentServer};
