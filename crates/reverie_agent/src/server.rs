use acp_thread::AgentConnection;
use agent_servers::{AgentServer, AgentServerDelegate};
use anyhow::{Context as _, Result};
use gpui::{App, Entity, Task};
use language_model::{LanguageModel, LanguageModelRegistry};
use project::{AgentId, Project};
use std::any::Any;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use ui::IconName;

use crate::connection::ReverieAgentConnection;

pub static REVERIE_AGENT_ID: LazyLock<AgentId> =
    LazyLock::new(|| AgentId::new("reverie"));

pub struct ReverieAgentServer;

impl ReverieAgentServer {
    pub fn new() -> Self {
        Self
    }

    fn default_model(cx: &App) -> Result<Arc<dyn LanguageModel>> {
        LanguageModelRegistry::read_global(cx)
            .default_model()
            .map(|m| m.model)
            .context("no default language model configured — pick one in settings before using the Reverie agent")
    }
}

impl Default for ReverieAgentServer {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentServer for ReverieAgentServer {
    fn agent_id(&self) -> AgentId {
        REVERIE_AGENT_ID.clone()
    }

    fn logo(&self) -> IconName {
        IconName::ZedAgent
    }

    fn connect(
        &self,
        _delegate: AgentServerDelegate,
        _project: Entity<Project>,
        cx: &mut App,
    ) -> Task<Result<Rc<dyn AgentConnection>>> {
        let model = match Self::default_model(cx) {
            Ok(m) => m,
            Err(e) => return Task::ready(Err(e)),
        };
        let connection: Rc<dyn AgentConnection> = Rc::new(ReverieAgentConnection::new(model));
        Task::ready(Ok(connection))
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}
