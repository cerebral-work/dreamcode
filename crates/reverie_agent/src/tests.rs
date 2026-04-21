use crate::backend::{LlmCallRequest, Role, ZedLlmBackend};
use reverie_deepagent::{LlmBackend, NextAction, Run, TodoList};
use std::thread;
use tempfile::TempDir;

fn fresh_run() -> (TempDir, Run) {
    let parent = TempDir::new().unwrap();
    let run = Run::new_under(parent.path()).unwrap();
    (parent, run)
}

#[test]
fn backend_parses_add_todo_action() {
    let (req_tx, req_rx) = smol::channel::unbounded::<LlmCallRequest>();
    let driver = thread::spawn(move || {
        let request = req_rx.recv_blocking().unwrap();
        assert!(matches!(request.messages.first(), Some((Role::System, _))));
        assert!(matches!(request.messages.last(), Some((Role::User, _))));
        request
            .reply
            .send(Ok(
                r#"{"action":"add_todo","description":"investigate the bug"}"#.to_string(),
            ))
            .unwrap();
    });

    let mut backend = ZedLlmBackend::new(req_tx);
    let (_tmp, run) = fresh_run();
    let todos = TodoList::new();
    let action = backend.next_action(&todos, &run.vfs, &[]).expect("ok");
    driver.join().unwrap();

    match action {
        NextAction::AddTodo(s) => assert_eq!(s, "investigate the bug"),
        other => panic!("expected AddTodo, got {other:?}"),
    }

    let transcript = backend.transcript();
    assert_eq!(transcript.len(), 3);
    assert_eq!(transcript[0].0, Role::System);
    assert_eq!(transcript[1].0, Role::User);
    assert_eq!(transcript[2].0, Role::Assistant);
}

#[test]
fn child_has_fresh_transcript() {
    let (req_tx, req_rx) = smol::channel::unbounded::<LlmCallRequest>();
    let driver = thread::spawn(move || {
        let parent_request = req_rx.recv_blocking().unwrap();
        assert_eq!(parent_request.messages.len(), 2, "parent: [System, User]");
        parent_request
            .reply
            .send(Ok(r#"{"action":"noop"}"#.to_string()))
            .unwrap();

        let child_request = req_rx.recv_blocking().unwrap();
        assert_eq!(
            child_request.messages.len(),
            2,
            "child should start fresh with [System, User], no parent history"
        );
        child_request
            .reply
            .send(Ok(r#"{"action":"noop"}"#.to_string()))
            .unwrap();
    });

    let mut parent = ZedLlmBackend::new(req_tx);
    let (_tmp, run) = fresh_run();
    let todos = TodoList::new();
    parent.next_action(&todos, &run.vfs, &[]).unwrap();

    let mut child = parent.child().unwrap();
    child.next_action(&todos, &run.vfs, &[]).unwrap();

    driver.join().unwrap();
}

#[test]
fn inject_nudge_appends_user_turn_before_next_state() {
    let (req_tx, req_rx) = smol::channel::unbounded::<LlmCallRequest>();
    let driver = thread::spawn(move || {
        let request = req_rx.recv_blocking().unwrap();
        assert_eq!(request.messages.len(), 3);
        assert_eq!(request.messages[1].0, Role::User);
        assert!(request.messages[1].1.contains("NUDGE: wake up"));
        request
            .reply
            .send(Ok(r#"{"action":"noop"}"#.to_string()))
            .unwrap();
    });

    let mut backend = ZedLlmBackend::new(req_tx);
    backend.inject_nudge("wake up");
    let (_tmp, run) = fresh_run();
    backend
        .next_action(&TodoList::new(), &run.vfs, &[])
        .unwrap();
    driver.join().unwrap();
}

#[test]
fn transport_error_propagates_on_dropped_driver() {
    let (req_tx, req_rx) = smol::channel::unbounded::<LlmCallRequest>();
    drop(req_rx);

    let mut backend = ZedLlmBackend::new(req_tx);
    let (_tmp, run) = fresh_run();
    let result = backend.next_action(&TodoList::new(), &run.vfs, &[]);
    assert!(
        matches!(result, Err(reverie_deepagent::BackendError::Transport(_))),
        "expected Transport error when driver is gone"
    );
}
