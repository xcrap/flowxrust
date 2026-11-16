//! `flowx --mcp-server`, as a provider sees it: the built binary, spawned
//! with the environment FlowX launches it with, over real pipes, talking to a
//! real app socket.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fx_bridge::listener::{Bridge, Call, handler};
use fx_bridge::wire::Reply;
use serde_json::{Value, json};

const BUDGET: Duration = Duration::from_secs(10);

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn spawn(environment: &[(&str, String)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_flowx"));
        command
            .arg(fx_bridge::SERVER_FLAG)
            // As Codex launches an MCP server: a bare environment plus the
            // entry's own variables. Nothing here may depend on more.
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (name, value) in environment {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("the binary starts");
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            stdin: child.stdin.take(),
            child,
            lines,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn next(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(BUDGET)
            .expect("the server answered");
        serde_json::from_str(&line).expect("one JSON message per line")
    }

    /// Wait for the process to end on its own.
    fn exit_within(&mut self, budget: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + budget;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("the server did not exit");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_provider_initializes_lists_and_calls_through_the_binary_and_it_ends_with_the_app() {
    let parent = tempfile::tempdir().unwrap();
    let bridge = Bridge::start(
        parent.path(),
        handler(|call: Call| async move {
            Reply::ok(format!("{} for {}", call.call.name(), call.thread))
        }),
    )
    .unwrap();
    let thread = uuid::Uuid::new_v4().to_string();
    let mut server = Server::spawn(&[
        (
            fx_bridge::ENV_SOCKET,
            bridge.socket().to_string_lossy().into_owned(),
        ),
        (fx_bridge::ENV_THREAD, thread.clone()),
        (fx_bridge::ENV_TOKEN, bridge.token_for(&thread)),
    ]);

    server.send(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                       "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                                  "clientInfo": {"name": "claude-code", "version": "2"}}}));
    let initialized = server.next();
    assert_eq!(initialized["id"], 0);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "flowx");
    assert_eq!(
        initialized["result"]["capabilities"],
        json!({"tools": {"listChanged": false}})
    );
    server.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    server.send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let listed = server.next();
    assert_eq!(listed["id"], 1, "the notification was answered: {listed}");
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["list_threads", "read_thread"]);

    server.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                       "params": {"name": "read_thread",
                                  "arguments": {"thread_id": thread, "count": 2}}}));
    let called = server.next();
    assert_eq!(called["id"], 2);
    assert_eq!(called["result"]["isError"], false);
    assert_eq!(
        called["result"]["content"][0]["text"],
        format!("read_thread for {thread}")
    );

    server.send(json!({"jsonrpc": "2.0", "id": 3, "method": "sampling/createMessage"}));
    assert_eq!(server.next()["error"]["code"], -32601);

    // The app quits: its socket goes, and the server with it, though its
    // input is still open.
    bridge.shutdown();
    assert!(server.exit_within(BUDGET).success());
}

#[test]
fn without_an_app_a_call_is_the_tool_s_error_and_the_server_ends_with_its_input() {
    let parent = tempfile::tempdir().unwrap();
    // A socket a crashed app left: there, with nobody listening.
    let stale = parent.path().join("flowx.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    for environment in [
        vec![
            (fx_bridge::ENV_SOCKET, stale.to_string_lossy().into_owned()),
            (fx_bridge::ENV_THREAD, uuid::Uuid::new_v4().to_string()),
            (fx_bridge::ENV_TOKEN, "stale".to_string()),
        ],
        Vec::new(),
    ] {
        let mut server = Server::spawn(&environment);
        server.send(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
                           "params": {"name": "list_threads", "arguments": {}}}));
        let called = server.next();
        assert_eq!(called["id"], 7);
        assert_eq!(called["result"]["isError"], true, "{called}");
        let text = called["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("FlowX"), "{text}");
        server.send(json!({"jsonrpc": "2.0", "id": 8, "method": "ping"}));
        assert_eq!(
            server.next(),
            json!({"jsonrpc": "2.0", "id": 8, "result": {}})
        );
        drop(server.stdin.take());
        assert!(server.exit_within(BUDGET).success());
    }
}
