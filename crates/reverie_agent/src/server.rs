use project::AgentId;
use std::sync::LazyLock;

pub static REVERIE_AGENT_ID: LazyLock<AgentId> =
    LazyLock::new(|| AgentId::new("reverie"));

pub struct ReverieAgentServer;
