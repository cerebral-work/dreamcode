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
        self.inner.clone().new_session(project, work_dirs, cx)
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
        self.inner
            .clone()
            .load_session(session_id, project, work_dirs, title, cx)
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
        self.inner
            .clone()
            .resume_session(session_id, project, work_dirs, title, cx)
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
                    .save_passive(
                        &session_id_str,
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
