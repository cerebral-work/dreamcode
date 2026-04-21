use acp_thread::{AcpThread, AgentConnection, UserMessageId};
use action_log::ActionLog;
use agent_client_protocol as acp;
use anyhow::Result;
use collections::HashMap;
use gpui::{App, AppContext as _, Entity, SharedString, Task, WeakEntity};
use language_model::LanguageModel;
use parking_lot::Mutex;
use project::{AgentId, Project};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use util::path_list::PathList;

use crate::server::REVERIE_AGENT_ID;

struct Session {
    thread: WeakEntity<AcpThread>,
    cancel: Arc<AtomicBool>,
}

pub struct ReverieAgentConnection {
    model: Arc<dyn LanguageModel>,
    sessions: Arc<Mutex<HashMap<acp::SessionId, Session>>>,
}

impl ReverieAgentConnection {
    pub fn new(model: Arc<dyn LanguageModel>) -> Self {
        Self {
            model,
            sessions: Arc::new(Mutex::new(HashMap::default())),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn model(&self) -> &Arc<dyn LanguageModel> {
        &self.model
    }
}

impl AgentConnection for ReverieAgentConnection {
    fn agent_id(&self) -> AgentId {
        REVERIE_AGENT_ID.clone()
    }

    fn telemetry_id(&self) -> SharedString {
        "reverie".into()
    }

    fn auth_methods(&self) -> &[acp::AuthMethod] {
        &[]
    }

    fn authenticate(&self, _method: acp::AuthMethodId, _cx: &mut App) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn new_session(
        self: Rc<Self>,
        project: Entity<Project>,
        work_dirs: PathList,
        cx: &mut App,
    ) -> Task<Result<Entity<AcpThread>>> {
        let session_id = acp::SessionId::new(uuid::Uuid::new_v4().to_string());
        let action_log = cx.new(|_| ActionLog::new(project.clone()));
        let capabilities_rx = watch::Receiver::constant(
            acp::PromptCapabilities::new()
                .image(false)
                .audio(false)
                .embedded_context(true),
        );
        let connection: Rc<dyn AgentConnection> = self.clone();
        let thread_session_id = session_id.clone();
        let thread = cx.new(|cx| {
            AcpThread::new(
                None,
                None,
                Some(work_dirs),
                connection,
                project,
                action_log,
                thread_session_id,
                capabilities_rx,
                cx,
            )
        });
        self.sessions.lock().insert(
            session_id,
            Session {
                thread: thread.downgrade(),
                cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        Task::ready(Ok(thread))
    }

    fn prompt(
        &self,
        _id: UserMessageId,
        _params: acp::PromptRequest,
        _cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        // Task 4 wires this to DeepAgent::execute on a background thread.
        Task::ready(Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)))
    }

    fn cancel(&self, session_id: &acp::SessionId, _cx: &mut App) {
        if let Some(session) = self.sessions.lock().get(session_id) {
            session
                .cancel
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }
}
