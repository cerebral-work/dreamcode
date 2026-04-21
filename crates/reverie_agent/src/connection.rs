use acp_thread::{AcpThread, AgentConnection, UserMessageId};
use action_log::ActionLog;
use agent_client_protocol as acp;
use anyhow::{Context as _, Result, anyhow};
use collections::HashMap;
use futures::StreamExt as _;
use gpui::{App, AppContext as _, AsyncApp, Entity, SharedString, Task, WeakEntity};
use language_model::{
    LanguageModel, LanguageModelRequest, LanguageModelRequestMessage, MessageContent, Role,
};
use parking_lot::Mutex;
use project::{AgentId, Project};
use reverie_deepagent::{Run, SpawnConfig, run_planner_with_observer};
use std::any::Any;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use util::path_list::PathList;

use crate::backend::{self, LlmCallRequest, ZedLlmBackend};
use crate::observer::ChannelObserver;
use crate::server::REVERIE_AGENT_ID;

const DEFAULT_MAX_ITERATIONS: u32 = 32;

struct Session {
    thread: WeakEntity<AcpThread>,
    cancel: Arc<AtomicBool>,
    state: Arc<Mutex<SessionState>>,
}

pub(crate) struct SessionState {
    pub(crate) run: Arc<reverie_deepagent::Run>,
    pub(crate) todos: reverie_deepagent::TodoList,
    pub(crate) in_progress: bool,
}

/// RAII guard that clears `in_progress` when dropped, including on panic
/// unwind. Returned by [`acquire_run_slot`] so the caller's normal control
/// flow (success, early return, or panic) all converge on "slot released".
pub(crate) struct InProgressGuard {
    state: Arc<Mutex<SessionState>>,
}

impl Drop for InProgressGuard {
    fn drop(&mut self) {
        self.state.lock().in_progress = false;
    }
}

/// Acquire the per-session "run in progress" slot. Returns the Arc<Run> and a
/// snapshot of the current TodoList (cloned, not shared), plus an
/// InProgressGuard that clears the flag on drop.
///
/// Rejects with an error when a run is already in progress on this session —
/// concurrent prompts on the same session are not supported in Phase 1.5c.
pub(crate) fn acquire_run_slot(
    state: &Arc<Mutex<SessionState>>,
) -> Result<(
    Arc<reverie_deepagent::Run>,
    reverie_deepagent::TodoList,
    InProgressGuard,
)> {
    let mut st = state.lock();
    if st.in_progress {
        return Err(anyhow!(
            "a run is already in progress for this session; cancel it first"
        ));
    }
    st.in_progress = true;
    let run = st.run.clone();
    let initial_todos = st.todos.clone();
    Ok((
        run,
        initial_todos,
        InProgressGuard {
            state: state.clone(),
        },
    ))
}

pub struct ReverieAgentConnection {
    model: Arc<dyn LanguageModel>,
    sessions: Arc<Mutex<HashMap<acp::SessionId, Session>>>,
    http_client: Arc<crate::ReverieHttpClient>,
}

impl ReverieAgentConnection {
    pub fn new(
        model: Arc<dyn LanguageModel>,
        http_client: Arc<crate::ReverieHttpClient>,
    ) -> Self {
        Self {
            model,
            sessions: Arc::new(Mutex::new(HashMap::default())),
            http_client,
        }
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
        let run = match reverie_deepagent::Run::new_default() {
            Ok(r) => Arc::new(r),
            Err(e) => {
                return Task::ready(Err(anyhow!("Run::new_default failed: {e}")));
            }
        };
        let session_state = Arc::new(Mutex::new(SessionState {
            run,
            todos: reverie_deepagent::TodoList::new(),
            in_progress: false,
        }));
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
                state: session_state,
            },
        );
        Task::ready(Ok(thread))
    }

    fn prompt(
        &self,
        _id: UserMessageId,
        params: acp::PromptRequest,
        cx: &mut App,
    ) -> Task<Result<acp::PromptResponse>> {
        let session_id = params.session_id.clone();
        let mut user_text = user_text_from_prompt(&params.prompt);

        let (thread_weak, cancel) = {
            let sessions = self.sessions.lock();
            match sessions.get(&session_id) {
                Some(s) => (s.thread.clone(), s.cancel.clone()),
                None => {
                    return Task::ready(Err(anyhow!(
                        "unknown session {:?}",
                        session_id.0.as_ref()
                    )));
                }
            }
        };

        let model = self.model.clone();
        let http_client = self.http_client.clone();

        cx.spawn(async move |cx| {
            // Memory retrieval: prepend context from reverie before the planner
            // starts. Failures degrade silently (Ok(None) from smart_context).
            let memory = http_client
                .smart_context(&user_text)
                .await
                .unwrap_or(None);
            let original_prompt = user_text.clone();
            if let Some(ctx) = &memory {
                let breadcrumb = format!(
                    "[memory] consulted reverie (project={})",
                    http_client.project()
                );
                let chunk = acp::ContentChunk::new(acp::ContentBlock::Text(
                    acp::TextContent::new(breadcrumb),
                ));
                let _ = thread_weak.update(cx, |thread, cx| {
                    if let Err(e) = thread.handle_session_update(
                        acp::SessionUpdate::AgentMessageChunk(chunk),
                        cx,
                    ) {
                        log::debug!("reverie: memory breadcrumb rejected: {e}");
                    }
                });
                user_text = format!("Relevant memory:\n{}\n\n{}", ctx.content, user_text);
            }

            let (req_tx, req_rx) = smol::channel::unbounded::<LlmCallRequest>();
            let (event_tx, event_rx) = smol::channel::unbounded::<acp::SessionUpdate>();

            let cancel_for_planner = cancel.clone();
            let user_text_for_planner = user_text.clone();
            let planner_task = smol::unblock(move || -> Result<reverie_deepagent::PlannerResult> {
                let mut backend = ZedLlmBackend::new(req_tx);
                // The user's prompt is bolted onto the system transcript as an
                // extra user turn so the model sees intent on iteration 1.
                // Run::new_default creates a fresh scratch dir per prompt —
                // Phase 1 has no cross-prompt persistence.
                backend.seed_user_message(&user_text_for_planner);

                let observer = ChannelObserver::new(event_tx, cancel_for_planner);
                let run =
                    Run::new_default().map_err(|e| anyhow!("vfs init failed: {e}"))?;
                Ok(run_planner_with_observer(
                    &run,
                    &mut backend,
                    DEFAULT_MAX_ITERATIONS,
                    &SpawnConfig::default(),
                    &observer,
                ))
            });

            // Pump observer events into the AcpThread concurrently so they
            // ship while the main loop is awaiting a stream_completion_text
            // response. The task ends naturally when the planner drops the
            // observer (and therefore its event_tx) on termination.
            let event_drain_thread = thread_weak.clone();
            let event_drain = cx.spawn(async move |cx| {
                while let Ok(update) = event_rx.recv().await {
                    if event_drain_thread
                        .update(cx, |thread, cx| {
                            if let Err(e) = thread.handle_session_update(update, cx) {
                                log::debug!("reverie: observer update rejected: {e}");
                            }
                        })
                        .is_err()
                    {
                        log::debug!("reverie: session thread dropped, halting event drain");
                        break;
                    }
                }
            });

            while let Ok(req) = req_rx.recv().await {
                let request = build_language_model_request(req.messages);
                let text_result = stream_to_string(&model, request, cx).await;
                let reply_payload = match text_result {
                    Ok(text) => Ok(text),
                    Err(e) => Err(e.to_string()),
                };
                if req.reply.send(reply_payload).is_err() {
                    log::warn!(
                        "reverie planner dropped its reply channel while the driver still held a request"
                    );
                }
            }

            let planner_result = planner_task
                .await
                .context("reverie planner thread failed")?;

            // Wait for any remaining observer events to flush before the
            // final summary so UI ordering matches planner ordering.
            event_drain.await;

            let summary = format!(
                "planner terminated: {:?} (iterations={}, todos_pending={}, spawns={})",
                planner_result.termination,
                planner_result.iterations,
                planner_result.todos.pending_count(),
                planner_result.spawn_log.len()
            );

            let summary_chunk = acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(summary.clone()),
            ));
            let update_result = thread_weak.update(cx, |thread, cx| {
                if let Err(e) = thread.handle_session_update(
                    acp::SessionUpdate::AgentMessageChunk(summary_chunk),
                    cx,
                ) {
                    log::warn!("reverie: failed to push final summary update: {e}");
                }
            });
            if update_result.is_err() {
                log::debug!("reverie: session thread dropped before final summary");
            }

            // Auto-save on clean terminations only. Fire-and-forget — save_passive
            // never propagates errors, so let-underscore is correct here.
            if matches!(
                planner_result.termination,
                reverie_deepagent::TerminationReason::Completed
            ) {
                let session_id_str = session_id.0.as_ref().to_string();
                let _ = http_client
                    .save_passive(
                        &session_id_str,
                        &original_prompt,
                        "zed-agent-user-intent",
                    )
                    .await;
                let _ = http_client
                    .save_passive(&session_id_str, &summary, "zed-agent-run-summary")
                    .await;
            }

            Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
        })
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

fn user_text_from_prompt(blocks: &[acp::ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|b| match b {
            acp::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_language_model_request(messages: Vec<(backend::Role, String)>) -> LanguageModelRequest {
    let messages = messages
        .into_iter()
        .map(|(role, content)| LanguageModelRequestMessage {
            role: match role {
                backend::Role::System => Role::System,
                backend::Role::User => Role::User,
                backend::Role::Assistant => Role::Assistant,
            },
            content: vec![MessageContent::Text(content)],
            cache: false,
            reasoning_details: None,
        })
        .collect();
    LanguageModelRequest {
        messages,
        ..Default::default()
    }
}

async fn stream_to_string(
    model: &Arc<dyn LanguageModel>,
    request: LanguageModelRequest,
    cx: &AsyncApp,
) -> Result<String> {
    let mut text_stream = model
        .stream_completion_text(request, cx)
        .await
        .map_err(|e| anyhow!("stream_completion_text failed: {e}"))?;
    let mut text = String::new();
    while let Some(chunk) = text_stream.stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("stream chunk error: {e}"))?;
        text.push_str(&chunk);
    }
    Ok(text)
}
