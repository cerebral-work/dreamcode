use acp_thread::AgentConnection;
use agent_servers::{AgentServer, AgentServerDelegate};
use anyhow::{Context as _, Result};
use gpui::{App, Entity, Task};
use http_client::HttpClient;
use language_model::{LanguageModel, LanguageModelRegistry};
use project::{AgentId, Project};
use std::any::Any;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};
use ui::IconName;

use crate::ReverieHttpClient;
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

    fn resolve_base_url() -> Option<String> {
        std::env::var("REVERIE_URL").ok()
    }

    fn resolve_project(project: &Entity<Project>, cx: &App) -> String {
        if let Ok(from_env) = std::env::var("REVERIE_PROJECT") {
            return from_env;
        }
        project
            .read(cx)
            .visible_worktrees(cx)
            .next()
            .map(|wt| wt.read(cx).root_name().as_unix_str().to_string())
            .unwrap_or_else(|| "unknown-project".to_string())
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
        project: Entity<Project>,
        cx: &mut App,
    ) -> Task<Result<Rc<dyn AgentConnection>>> {
        let model = match Self::default_model(cx) {
            Ok(m) => m,
            Err(e) => return Task::ready(Err(e)),
        };
        let base_url = Self::resolve_base_url();
        let project_name = Self::resolve_project(&project, cx);
        let http: Arc<dyn HttpClient> = project.read(cx).client().http_client();
        let http_client = ReverieHttpClient::new(base_url, project_name, http);
        let connection: Rc<dyn AgentConnection> =
            Rc::new(ReverieAgentConnection::new(model, http_client));
        Task::ready(Ok(connection))
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}
