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

mod http_tests {
    use crate::http::ReverieHttpClient;
    use futures::AsyncReadExt as _;
    use http_client::{AsyncBody, FakeHttpClient, Method, Response};
    use std::sync::{Arc as StdArc, Mutex};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    #[test]
    fn smart_context_parses_response() {
        let http = FakeHttpClient::create(|req| async move {
            assert_eq!(req.method(), Method::GET);
            let uri = req.uri().to_string();
            assert!(uri.contains("/context/smart"), "{uri}");
            assert!(uri.contains("q=how+do+I+X"), "{uri}");
            assert!(uri.contains("project=test-proj"), "{uri}");
            Ok(Response::builder()
                .status(200)
                .body(AsyncBody::from(
                    r###"{"context":"## Memory\n- item 1\n- item 2\n"}"###.to_string(),
                ))
                .unwrap())
        });
        let client = ReverieHttpClient::new(
            Some("http://example.test".to_string()),
            "test-proj".to_string(),
            http,
        );
        let result = block_on(client.smart_context("how do I X")).unwrap();
        let ctx = result.expect("should have returned Some");
        assert!(ctx.content.contains("item 1"));
        assert!(ctx.content.contains("item 2"));
    }

    #[test]
    fn smart_context_returns_none_on_transport_error() {
        let http =
            FakeHttpClient::create(
                |_req| async move { Err(anyhow::anyhow!("connection refused")) },
            );
        let client = ReverieHttpClient::new(
            Some("http://example.test".to_string()),
            "p".to_string(),
            http,
        );
        let result = block_on(client.smart_context("anything")).unwrap();
        assert!(result.is_none(), "transport error should degrade to None");
    }

    #[test]
    fn smart_context_returns_none_on_5xx() {
        let http = FakeHttpClient::create(|_req| async move {
            Ok(Response::builder()
                .status(500)
                .body(AsyncBody::from(r#"{"error":"boom"}"#.to_string()))
                .unwrap())
        });
        let client = ReverieHttpClient::new(
            Some("http://example.test".to_string()),
            "p".to_string(),
            http,
        );
        let result = block_on(client.smart_context("anything")).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn smart_context_returns_none_on_empty_context() {
        let http = FakeHttpClient::create(|_req| async move {
            Ok(Response::builder()
                .status(200)
                .body(AsyncBody::from(r#"{"context":""}"#.to_string()))
                .unwrap())
        });
        let client = ReverieHttpClient::new(
            Some("http://example.test".to_string()),
            "p".to_string(),
            http,
        );
        let result = block_on(client.smart_context("anything")).unwrap();
        assert!(result.is_none(), "empty context collapses to None");
    }

    #[test]
    fn save_observation_serializes_correct_body() {
        let captured: StdArc<Mutex<Option<(String, String)>>> = StdArc::new(Mutex::new(None));
        let captured_for_handler = captured.clone();
        let http = FakeHttpClient::create(move |mut req| {
            let captured_for_handler = captured_for_handler.clone();
            async move {
                assert_eq!(req.method(), Method::POST);
                let uri = req.uri().to_string();
                let mut body = String::new();
                req.body_mut().read_to_string(&mut body).await.unwrap();
                *captured_for_handler.lock().unwrap() = Some((uri, body));
                Ok(Response::builder()
                    .status(201)
                    .body(AsyncBody::from(r#"{"id":1,"status":"saved"}"#.to_string()))
                    .unwrap())
            }
        });

        let client = ReverieHttpClient::new(
            Some("http://example.test".to_string()),
            "myproj".to_string(),
            http,
        );
        block_on(client.save_observation(
            "session-42",
            "user prompt",
            "hello world",
            "zed-agent-user-intent",
        ))
        .unwrap();

        let (uri, body) = captured.lock().unwrap().clone().expect("body captured");
        // Direct-add endpoint, not /observations/passive — extractor-free path
        // so raw prompt/summary text actually lands.
        assert!(uri.ends_with("/observations"), "{uri}");
        assert!(!uri.contains("/observations/passive"), "{uri}");
        assert!(body.contains(r#""session_id":"session-42""#), "{body}");
        assert!(body.contains(r#""type":"note""#), "{body}");
        assert!(body.contains(r#""title":"user prompt""#), "{body}");
        assert!(body.contains(r#""content":"hello world""#), "{body}");
        assert!(body.contains(r#""project":"myproj""#), "{body}");
        assert!(
            body.contains(r#""source":"zed-agent-user-intent""#),
            "{body}"
        );
    }

    #[test]
    fn save_observation_tolerates_transport_error() {
        let http =
            FakeHttpClient::create(
                |_req| async move { Err(anyhow::anyhow!("connection refused")) },
            );
        let client = ReverieHttpClient::new(
            Some("http://example.test".to_string()),
            "p".to_string(),
            http,
        );
        let result = block_on(client.save_observation("s", "t", "c", "x"));
        assert!(
            result.is_ok(),
            "save_observation must never propagate transport errors"
        );
    }

    #[test]
    fn save_observation_tolerates_5xx() {
        let http = FakeHttpClient::create(|_req| async move {
            Ok(Response::builder()
                .status(500)
                .body(AsyncBody::from(r#"{"error":"boom"}"#.to_string()))
                .unwrap())
        });
        let client = ReverieHttpClient::new(
            Some("http://example.test".to_string()),
            "p".to_string(),
            http,
        );
        let result = block_on(client.save_observation("s", "t", "c", "x"));
        assert!(result.is_ok(), "save_observation must swallow 5xx quietly");
    }
}

mod session_slot_tests {
    use crate::connection::{SessionState, acquire_run_slot};
    use parking_lot::Mutex;
    use reverie_deepagent::{Run, TodoList};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn fresh_state() -> Arc<Mutex<SessionState>> {
        let parent = TempDir::new().unwrap();
        // Leak the TempDir on purpose — the Run's scratch_root needs to outlive
        // this helper. Tests get a fresh process each `cargo test` invocation.
        let parent_path = parent.keep();
        let root = parent_path.join("session-test");
        std::fs::create_dir_all(&root).unwrap();
        let vfs = reverie_deepagent::Vfs::new(&root).unwrap();
        let run = Run {
            id: "session-test".into(),
            scratch_root: root,
            vfs,
            depth: 0,
        };
        Arc::new(Mutex::new(SessionState {
            run: Arc::new(run),
            todos: TodoList::new(),
            in_progress: false,
        }))
    }

    #[test]
    fn rejects_when_in_progress() {
        let state = fresh_state();
        state.lock().in_progress = true;
        let result = acquire_run_slot(&state);
        assert!(result.is_err(), "should reject when in_progress");
        let msg = result.err().unwrap().to_string();
        assert!(
            msg.contains("already in progress"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn returns_current_todos_snapshot() {
        let state = fresh_state();
        state.lock().todos.add("alpha");

        let (_run, initial, _guard) = acquire_run_slot(&state).unwrap();
        assert_eq!(initial.entries().len(), 1);
        assert_eq!(initial.entries()[0].description, "alpha");

        // Releasing the slot so lock is available, then mutating state.todos
        // — the snapshot we captured must not reflect the later mutation
        // (proves it's a clone, not a reference).
        drop(_guard);
        state.lock().todos.add("beta");
        assert_eq!(initial.entries().len(), 1, "snapshot should be a clone");
    }

    #[test]
    fn guard_clears_in_progress_on_drop() {
        let state = fresh_state();
        let (_run, _todos, guard) = acquire_run_slot(&state).unwrap();
        assert!(state.lock().in_progress, "acquire sets in_progress");
        drop(guard);
        assert!(
            !state.lock().in_progress,
            "dropping the guard must clear in_progress"
        );
    }

    #[test]
    fn guard_clears_in_progress_on_panic() {
        let state = fresh_state();
        let state_for_panic = state.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (_run, _todos, _guard) = acquire_run_slot(&state_for_panic).unwrap();
            panic!("simulated failure while holding the slot");
        }));
        assert!(
            result.is_err(),
            "the panic should propagate out of catch_unwind"
        );
        assert!(
            !state.lock().in_progress,
            "in_progress must be cleared even when the guard is dropped via panic"
        );
    }
}

mod augment_tests {
    use crate::augment::augment_prompt_blocks;
    use crate::http::SmartContext;
    use agent_client_protocol::schema as acp;

    fn text_block(s: &str) -> acp::ContentBlock {
        acp::ContentBlock::Text(acp::TextContent::new(s.to_string()))
    }

    fn block_text(b: &acp::ContentBlock) -> Option<String> {
        match b {
            acp::ContentBlock::Text(t) => Some(t.text.to_string()),
            _ => None,
        }
    }

    #[test]
    fn augment_prompt_blocks_prepends_memory_when_present() {
        let blocks = vec![text_block("hello")];
        let memory = Some(SmartContext {
            content: "- item 1".into(),
        });
        let out = augment_prompt_blocks(blocks, memory);
        assert_eq!(out.len(), 2);
        assert_eq!(block_text(&out[0]).unwrap(), "Relevant memory:\n- item 1\n");
        assert_eq!(block_text(&out[1]).unwrap(), "hello");
    }

    #[test]
    fn augment_prompt_blocks_passes_through_on_no_memory() {
        let blocks = vec![text_block("hello")];
        let out = augment_prompt_blocks(blocks, None);
        assert_eq!(out.len(), 1);
        assert_eq!(block_text(&out[0]).unwrap(), "hello");
    }

    #[test]
    fn augment_prompt_blocks_skips_empty_memory() {
        let blocks = vec![text_block("hello")];
        let memory = Some(SmartContext {
            content: "   \n".into(),
        });
        let out = augment_prompt_blocks(blocks, memory);
        assert_eq!(out.len(), 1, "whitespace-only memory should be skipped");
        assert_eq!(block_text(&out[0]).unwrap(), "hello");
    }
}

mod cancel_race_tests {
    use crate::connection::race_with_cancel;

    #[test]
    fn race_with_cancel_fires_err_when_cancel_pre_seeded() {
        let (tx, rx) = smol::channel::bounded::<()>(1);
        tx.try_send(()).unwrap();

        let result: anyhow::Result<()> = futures::executor::block_on(race_with_cancel(
            futures::future::pending::<anyhow::Result<()>>(),
            &rx,
            "cancelled",
        ));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("cancelled"),
            "expected error message to contain 'cancelled'"
        );
    }

    #[test]
    fn race_with_cancel_lets_work_win_when_no_signal() {
        let (_tx, rx) = smol::channel::bounded::<()>(1);

        let result: anyhow::Result<u32> =
            futures::executor::block_on(race_with_cancel(async { Ok(42u32) }, &rx, "cancelled"));
        assert_eq!(result.unwrap(), 42);
    }
}
