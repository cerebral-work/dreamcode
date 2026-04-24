use agent_client_protocol as acp;
use reverie_deepagent::{NextAction, PlannerObserver, SpawnObservation};
use smol::channel::Sender;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// Translates planner callbacks into acp::SessionUpdate messages and ships
// them onto a channel that a foreground task drains into the AcpThread.
//
// Each NextAction becomes an AgentMessageChunk so Zed's agent UI can render
// it inline. SpawnObservations become separate AgentMessageChunks carrying
// the child summary.
pub struct ChannelObserver {
    tx: Sender<acp::SessionUpdate>,
    cancel: Arc<AtomicBool>,
}

impl ChannelObserver {
    pub fn new(tx: Sender<acp::SessionUpdate>, cancel: Arc<AtomicBool>) -> Self {
        Self { tx, cancel }
    }

    fn action_to_update(action: &NextAction) -> acp::SessionUpdate {
        let (title, body) = match action {
            NextAction::AddTodo(s) => ("add_todo", s.clone()),
            NextAction::SetStatus(id, st) => ("set_status", format!("todo #{id} → {st:?}")),
            NextAction::VfsWrite { path, contents } => (
                "vfs_write",
                format!(
                    "{path}\n---\n{}",
                    truncate_for_display(contents, DISPLAY_CONTENT_LIMIT)
                ),
            ),
            NextAction::Spawn(req) => ("spawn", format!("{} :: {}", req.persona, req.task)),
            NextAction::ParallelSpawn(reqs) => (
                "parallel_spawn",
                reqs.iter()
                    .map(|r| format!("{} :: {}", r.persona, r.task))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            NextAction::Note(s) => ("note", s.clone()),
            NextAction::GiveUp => ("give_up", String::new()),
            NextAction::NoOp => ("noop", String::new()),
        };
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
            acp::TextContent::new(format!("[{title}] {body}")),
        )))
    }
}

impl PlannerObserver for ChannelObserver {
    fn on_action(&self, action: &NextAction) {
        let update = Self::action_to_update(action);
        // try_send drops the event if no one is listening — preferable to
        // blocking the planner thread on a stuck consumer.
        if self.tx.try_send(update).is_err() {
            log::debug!("reverie observer: dropping action update, channel closed or full");
        }
    }

    fn on_spawn_complete(&self, observation: &SpawnObservation) {
        let body = format!(
            "[subagent {}] {:?}: {}",
            observation.persona, observation.status, observation.summary
        );
        let update = acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
            acp::ContentBlock::Text(acp::TextContent::new(body)),
        ));
        if self.tx.try_send(update).is_err() {
            log::debug!("reverie observer: dropping spawn observation, channel closed or full");
        }
    }

    fn should_stop(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
}

const DISPLAY_CONTENT_LIMIT: usize = 200;

fn truncate_for_display(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push_str("… [truncated]");
    out
}
