use reverie_deepagent::backends::protocol::{
    JSON_PROTOCOL_SUFFIX, parse_action, render_state_with_observations,
};
use reverie_deepagent::prompt::DEEPAGENT_BASE_PROMPT;
use reverie_deepagent::{BackendError, LlmBackend, NextAction, SpawnObservation, TodoList, Vfs};
use std::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

pub struct LlmCallRequest {
    pub messages: Vec<(Role, String)>,
    pub reply: mpsc::Sender<Result<String, String>>,
}

// LlmBackend impl that forwards every turn to a driver running on Zed's
// foreground executor. The backend itself is sync (the trait requires it) and
// runs on a dedicated planner thread, so blocking on an mpsc recv is safe. The
// driver owns the `LanguageModel` handle and dispatches async completion
// calls; this split keeps the backend free of any GPUI dependency.
pub struct ZedLlmBackend {
    transcript: Vec<(Role, String)>,
    system_prompt: String,
    request_tx: mpsc::Sender<LlmCallRequest>,
}

impl ZedLlmBackend {
    pub fn new(request_tx: mpsc::Sender<LlmCallRequest>) -> Self {
        let system_prompt = format!("{DEEPAGENT_BASE_PROMPT}{JSON_PROTOCOL_SUFFIX}");
        let transcript = vec![(Role::System, system_prompt.clone())];
        Self {
            transcript,
            system_prompt,
            request_tx,
        }
    }

    #[cfg(test)]
    pub(crate) fn transcript(&self) -> &[(Role, String)] {
        &self.transcript
    }
}

impl LlmBackend for ZedLlmBackend {
    fn next_action(
        &mut self,
        todos: &TodoList,
        vfs: &Vfs,
        observations: &[SpawnObservation],
    ) -> Result<NextAction, BackendError> {
        let user_msg = render_state_with_observations(todos, vfs, observations);
        self.transcript.push((Role::User, user_msg));

        let (reply_tx, reply_rx) = mpsc::channel();
        self.request_tx
            .send(LlmCallRequest {
                messages: self.transcript.clone(),
                reply: reply_tx,
            })
            .map_err(|e| BackendError::Transport(e.to_string()))?;

        let text = reply_rx
            .recv()
            .map_err(|e| BackendError::Transport(e.to_string()))?
            .map_err(BackendError::Transport)?;

        self.transcript.push((Role::Assistant, text.clone()));
        parse_action(&text)
    }

    fn inject_nudge(&mut self, msg: &str) {
        self.transcript.push((Role::User, format!("NUDGE: {msg}")));
    }

    fn child(&self) -> Result<Box<dyn LlmBackend + Send>, BackendError> {
        let child = Self {
            transcript: vec![(Role::System, self.system_prompt.clone())],
            system_prompt: self.system_prompt.clone(),
            request_tx: self.request_tx.clone(),
        };
        Ok(Box::new(child))
    }
}
