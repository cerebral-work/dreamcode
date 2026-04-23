use acp_thread::{
    AgentConnection, AgentModelSelector, AgentSessionConfigOptions, AgentSessionList,
    AgentSessionModes, AgentSessionRetry, AgentSessionSetTitle, AgentSessionTruncate,
    AgentTelemetry, UserMessageId,
};
use agent_client_protocol::{self as acp};
use anyhow::Result;
use gpui::{App, Entity, SharedString, Task};
use project::{AgentId, Project};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use task::SpawnInTerminal;
use util::path_list::PathList;

use crate::ReverieHttpClient;
use crate::http::SmartContext;

/// Insert a "Relevant memory:\n<ctx>\n" text ContentBlock at position 0 of
/// the prompt's block list when memory is present and non-empty. Otherwise
/// return the blocks unchanged.
pub(crate) fn augment_prompt_blocks(
    mut blocks: Vec<acp::ContentBlock>,
    memory: Option<SmartContext>,
) -> Vec<acp::ContentBlock> {
    if let Some(ctx) = memory
        && !ctx.content.trim().is_empty()
    {
        let memory_block = acp::ContentBlock::Text(acp::TextContent::new(format!(
            "Relevant memory:\n{}\n",
            ctx.content
        )));
        blocks.insert(0, memory_block);
    }
    blocks
}

pub(crate) struct ReverieAugmentedConnection {
    inner: Rc<dyn AgentConnection>,
    http_client: Arc<ReverieHttpClient>,
}

impl ReverieAugmentedConnection {
    pub(crate) fn new(
        inner: Rc<dyn AgentConnection>,
        http_client: Arc<ReverieHttpClient>,
    ) -> Self {
        Self { inner, http_client }
    }
}

impl AgentConnection for ReverieAugmentedConnection {
    fn agent_id(&self) -> AgentId {
        self.inner.agent_id()
    }

    fn telemetry_id(&self) -> SharedString {
        self.inner.telemetry_id()
    }

    fn auth_methods(&self) -> &[acp::AuthMethod] {
        self.inner.auth_methods()
    }

    fn authenticate(&self, method: acp::AuthMethodId, cx: &mut App) -> Task<Result<()>> {
        self.inner.authenticate(method, cx)
    }

    fn new_session(
        self: Rc<Self>,
        project: Entity<Project>,
        work_dirs: PathList,
        cx: &mut App,
    ) -> Task<Result<Entity<acp_thread::AcpThread>>> {
        let this = self.clone();
        let inner_task = self.inner.clone().new_session(project, work_dirs, cx);
        cx.spawn(async move |cx| {
            let thread = inner_task.await?;
            install_wrapper_connection(&thread, this, cx)?;
            Ok(thread)
        })
    }

    fn supports_load_session(&self) -> bool {
        self.inner.supports_load_session()
    }

    fn load_session(
        self: Rc<Self>,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        title: Option<SharedString>,
        cx: &mut App,
    ) -> Task<Result<Entity<acp_thread::AcpThread>>> {
        let this = self.clone();
        let inner_task = self
            .inner
            .clone()
            .load_session(session_id, project, work_dirs, title, cx);
        cx.spawn(async move |cx| {
            let thread = inner_task.await?;
            install_wrapper_connection(&thread, this, cx)?;
            Ok(thread)
        })
    }

    fn supports_close_session(&self) -> bool {
        self.inner.supports_close_session()
    }

    fn close_session(
        self: Rc<Self>,
        session_id: &acp::SessionId,
        cx: &mut App,
    ) -> Task<Result<()>> {
        self.inner.clone().close_session(session_id, cx)
    }

    fn supports_resume_session(&self) -> bool {
        self.inner.supports_resume_session()
    }

    fn resume_session(
        self: Rc<Self>,
        session_id: acp::SessionId,
        project: Entity<Project>,
        work_dirs: PathList,
        title: Option<SharedString>,
        cx: &mut App,
    ) -> Task<Result<Entity<acp_thread::AcpThread>>> {
        let this = self.clone();
        let inner_task = self
            .inner
            .clone()
            .resume_session(session_id, project, work_dirs, title, cx);
        cx.spawn(async move |cx| {
            let thread = inner_task.await?;
            install_wrapper_connection(&thread, this, cx)?;
            Ok(thread)
        })
    }

    fn supports_session_history(&self) -> bool {
        self.inner.supports_session_history()
    }

    fn terminal_auth_task(
        &self,
        method: &acp::AuthMethodId,
        cx: &App,
    ) -> Option<Task<Result<SpawnInTerminal>>> {
        self.inner.terminal_auth_task(method, cx)
    }

    fn prompt(
        &self,
        id: UserMessageId,
        mut params: acp::PromptRequest,
        cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        let http = self.http_client.clone();
        let inner = self.inner.clone();
        let user_text = user_text_from_prompt(&params.prompt);
        let session_id = params.session_id.clone();

        cx.spawn(async move |cx| {
            let memory = http.smart_context(&user_text).await.ok().flatten();
            let blocks = std::mem::take(&mut params.prompt);
            params.prompt = augment_prompt_blocks(blocks, memory);

            let response = cx
                .update(|cx| inner.prompt(id, params, cx))
                .await?;

            if matches!(response.stop_reason, acp::StopReason::EndTurn) {
                let session_id_str = session_id.0.as_ref().to_string();
                let _ = http
                    .save_observation(
                        &session_id_str,
                        "user prompt",
                        &user_text,
                        "zed-augment-user-intent",
                    )
                    .await;
            }
            Ok(response)
        })
    }

    fn retry(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionRetry>> {
        self.inner.retry(session_id, cx)
    }

    fn cancel(&self, session_id: &acp::SessionId, cx: &mut App) {
        self.inner.cancel(session_id, cx)
    }

    fn truncate(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionTruncate>> {
        self.inner.truncate(session_id, cx)
    }

    fn set_title(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionSetTitle>> {
        self.inner.set_title(session_id, cx)
    }

    fn model_selector(
        &self,
        session_id: &acp::SessionId,
    ) -> Option<Rc<dyn AgentModelSelector>> {
        self.inner.model_selector(session_id)
    }

    fn telemetry(&self) -> Option<Rc<dyn AgentTelemetry>> {
        self.inner.telemetry()
    }

    fn session_modes(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionModes>> {
        self.inner.session_modes(session_id, cx)
    }

    fn session_config_options(
        &self,
        session_id: &acp::SessionId,
        cx: &App,
    ) -> Option<Rc<dyn AgentSessionConfigOptions>> {
        self.inner.session_config_options(session_id, cx)
    }

    fn session_list(&self, cx: &mut App) -> Option<Rc<dyn AgentSessionList>> {
        self.inner.session_list(cx)
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}

// Swap the AcpThread's stored AgentConnection with `wrapper` so subsequent
// `thread.connection().prompt(...)` calls route through our augment layer.
// NativeAgentConnection::new_session creates a fresh self-referential
// connection and installs it in the thread, which would otherwise bypass
// this wrapper entirely (the wrapper's new_session is a pure delegate).
fn install_wrapper_connection(
    thread: &Entity<acp_thread::AcpThread>,
    wrapper: Rc<ReverieAugmentedConnection>,
    cx: &mut gpui::AsyncApp,
) -> Result<()> {
    cx.update(|cx| {
        thread.update(cx, |thread, _| {
            let wrapper_conn: Rc<dyn AgentConnection> = wrapper;
            thread.set_connection(wrapper_conn);
        });
    });
    Ok(())
}

fn user_text_from_prompt(blocks: &[acp::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            acp::ContentBlock::Text(t) => Some(t.text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct ReverieAugmentedAgentServer {
    inner: Box<dyn agent_servers::AgentServer>,
    http_client: Arc<ReverieHttpClient>,
}

impl ReverieAugmentedAgentServer {
    pub fn new(
        inner: Box<dyn agent_servers::AgentServer>,
        http_client: Arc<ReverieHttpClient>,
    ) -> Self {
        Self { inner, http_client }
    }
}

impl agent_servers::AgentServer for ReverieAugmentedAgentServer {
    fn agent_id(&self) -> AgentId {
        self.inner.agent_id()
    }

    fn logo(&self) -> ui::IconName {
        self.inner.logo()
    }

    fn connect(
        &self,
        delegate: agent_servers::AgentServerDelegate,
        project: Entity<Project>,
        cx: &mut App,
    ) -> Task<Result<Rc<dyn AgentConnection>>> {
        let http = self.http_client.clone();
        let inner_task = self.inner.connect(delegate, project, cx);
        cx.spawn(async move |_cx| {
            let inner_conn = inner_task.await?;
            let wrapped: Rc<dyn AgentConnection> =
                Rc::new(ReverieAugmentedConnection::new(inner_conn, http));
            Ok(wrapped)
        })
    }

    fn default_mode(&self, cx: &App) -> Option<acp::SessionModeId> {
        self.inner.default_mode(cx)
    }

    fn set_default_mode(
        &self,
        mode_id: Option<acp::SessionModeId>,
        fs: Arc<dyn fs::Fs>,
        cx: &mut App,
    ) {
        self.inner.set_default_mode(mode_id, fs, cx)
    }

    fn default_model(&self, cx: &App) -> Option<acp::ModelId> {
        self.inner.default_model(cx)
    }

    fn set_default_model(
        &self,
        model_id: Option<acp::ModelId>,
        fs: Arc<dyn fs::Fs>,
        cx: &mut App,
    ) {
        self.inner.set_default_model(model_id, fs, cx)
    }

    fn favorite_model_ids(&self, cx: &mut App) -> collections::HashSet<acp::ModelId> {
        self.inner.favorite_model_ids(cx)
    }

    fn default_config_option(&self, config_id: &str, cx: &App) -> Option<String> {
        self.inner.default_config_option(config_id, cx)
    }

    fn set_default_config_option(
        &self,
        config_id: &str,
        value_id: Option<&str>,
        fs: Arc<dyn fs::Fs>,
        cx: &mut App,
    ) {
        self.inner
            .set_default_config_option(config_id, value_id, fs, cx)
    }

    fn favorite_config_option_value_ids(
        &self,
        config_id: &acp::SessionConfigId,
        cx: &mut App,
    ) -> collections::HashSet<acp::SessionConfigValueId> {
        self.inner.favorite_config_option_value_ids(config_id, cx)
    }

    fn toggle_favorite_config_option_value(
        &self,
        config_id: acp::SessionConfigId,
        value_id: acp::SessionConfigValueId,
        should_be_favorite: bool,
        fs: Arc<dyn fs::Fs>,
        cx: &App,
    ) {
        self.inner.toggle_favorite_config_option_value(
            config_id,
            value_id,
            should_be_favorite,
            fs,
            cx,
        )
    }

    fn toggle_favorite_model(
        &self,
        model_id: acp::ModelId,
        should_be_favorite: bool,
        fs: Arc<dyn fs::Fs>,
        cx: &App,
    ) {
        self.inner
            .toggle_favorite_model(model_id, should_be_favorite, fs, cx)
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}

/// Wrap `inner` in a `ReverieAugmentedAgentServer` that injects reverie
/// memory into every prompt() call routed through the inner server's
/// connection. Returns `None` if `inner` IS the Reverie agent itself (to
/// avoid double-retrieval with its in-process memory path) or if
/// http_client construction fails; callers fall back to the un-wrapped
/// inner in those cases.
pub fn augment_with_memory(
    inner: Box<dyn agent_servers::AgentServer>,
    project: &Entity<Project>,
    cx: &App,
) -> Option<Rc<dyn agent_servers::AgentServer>> {
    if inner.agent_id() == *crate::REVERIE_AGENT_ID {
        return None;
    }
    let base_url = std::env::var("REVERIE_URL").ok();
    let project_name = resolve_project_name_for_augment(project, cx);
    let http_client_arc: Arc<dyn http_client::HttpClient> =
        project.read(cx).client().http_client();
    let http_client = ReverieHttpClient::new(base_url, project_name, http_client_arc);
    Some(Rc::new(ReverieAugmentedAgentServer::new(inner, http_client))
        as Rc<dyn agent_servers::AgentServer>)
}

fn resolve_project_name_for_augment(project: &Entity<Project>, cx: &App) -> String {
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
