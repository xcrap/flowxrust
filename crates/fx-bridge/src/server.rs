//! `flowx --mcp-server`: the MCP server a provider launches.
//!
//! It answers the protocol itself (see [`crate::mcp`]) and forwards each
//! tool call to the app, one connection per call. Three rules:
//!
//! - **An unreachable app is the tool's error, never a crash.** A call while
//!   FlowX is gone, slow or refusing is answered with an error result the
//!   model reads, and the server keeps serving: the provider is still running
//!   the turn, and a dead MCP server would fail every later call too.
//! - **It ends when its input does**, which is how both providers stop an MCP
//!   server, after giving the calls already asked a moment to be answered.
//! - **It ends when the socket goes**, which is the app quitting: a socket it
//!   once saw and that is no longer there will not come back for this server,
//!   whose token was made for that launch. A socket never seen is not "gone";
//!   the server stays and answers each call with why it cannot reach FlowX.
//!
//! The input is read on a task of its own, so a line half-read is never lost
//! to the select that also watches the socket.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::lines::{Line, read_line};
use crate::mcp::{self, Action};
use crate::tools::{DEFAULT_ANSWER_TIMEOUT, Role, ToolCall};
use crate::wire::{MAX_REPLY_BYTES, Reply, Request};

/// The most one message from the client may be.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// Where the app is, and who this server speaks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub socket: PathBuf,
    pub thread: String,
    pub token: String,
}

impl Endpoint {
    /// Read from this process's environment.
    pub fn from_env() -> Option<Self> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// Read from any set of variables; all three must be there.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let read = |name| get(name).filter(|value: &String| !value.trim().is_empty());
        Some(Self {
            socket: PathBuf::from(read(crate::ENV_SOCKET)?),
            thread: read(crate::ENV_THREAD)?,
            token: read(crate::ENV_TOKEN)?,
        })
    }
}

/// How the server ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The client closed its input.
    InputClosed,
    /// The app's socket went away.
    SocketGone,
}

/// The server's timings, the version it reports and who it speaks for.
#[derive(Debug, Clone)]
pub struct Options {
    pub version: String,
    /// Which tools it lists: a lead's server lists the orchestration tools.
    pub role: Role,
    /// How often the socket is looked for.
    pub watch_interval: Duration,
    pub connect_timeout: Duration,
    /// How long an ordinary call may take, end to end. A call the app needs
    /// longer for (a wait, a start) is given what it needs beyond an
    /// ordinary one on top ([`call_budget`]).
    pub call_timeout: Duration,
    /// How long calls already asked may take to be answered once the input
    /// has closed.
    pub exit_grace: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            role: Role::Thread,
            watch_interval: Duration::from_secs(1),
            connect_timeout: Duration::from_secs(3),
            call_timeout: Duration::from_secs(30),
            exit_grace: Duration::from_secs(3),
        }
    }
}

pub const NOT_CONFIGURED: &str = "FlowX's tools are not set up for this process: \
    FLOWX_MCP_SOCKET, FLOWX_MCP_THREAD or FLOWX_MCP_TOKEN is missing. They work only in \
    a thread FlowX itself started with its tools turned on.";

/// Serve MCP on `input` and `output` until the input closes or the socket
/// goes.
pub async fn serve<R, W>(
    input: R,
    mut output: W,
    endpoint: Option<Endpoint>,
    options: Options,
) -> std::io::Result<Ended>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let endpoint = endpoint.map(Arc::new);
    let options = Arc::new(options);
    let (lines_tx, mut lines) = tokio::sync::mpsc::channel::<std::io::Result<Line>>(16);
    let reader = tokio::spawn(async move {
        let mut input = BufReader::new(input);
        loop {
            let line = read_line(&mut input, MAX_MESSAGE_BYTES).await;
            let last = !matches!(line, Ok(Line::Data(_) | Line::TooLong));
            if lines_tx.send(line).await.is_err() || last {
                return;
            }
        }
    });
    let (replies_tx, mut replies) = tokio::sync::mpsc::channel::<Value>(64);
    let mut calls = tokio::task::JoinSet::new();
    let mut watch = tokio::time::interval(options.watch_interval);
    watch.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Looked for at once, not at the first tick: a socket that goes before
    // the timer first fires was still there when the server started.
    let present = |endpoint: &Endpoint| std::fs::symlink_metadata(&endpoint.socket).is_ok();
    let mut seen = endpoint.as_deref().is_some_and(present);

    let ended = loop {
        tokio::select! {
            biased;
            Some(reply) = replies.recv() => write(&mut output, &reply).await?,
            line = lines.recv() => match line {
                None | Some(Ok(Line::End)) => break Ok(Ended::InputClosed),
                Some(Err(error)) => break Err(error),
                Some(Ok(Line::TooLong)) => {
                    let too_long = mcp::error(&Value::Null, mcp::INVALID_REQUEST, "Message too large");
                    write(&mut output, &too_long).await?;
                }
                Some(Ok(Line::Data(bytes))) => match mcp::handle(&bytes, &options.version, options.role) {
                    Action::Reply(reply) => write(&mut output, &reply).await?,
                    Action::Ignore => {}
                    Action::Forward { id, call } => {
                        let (endpoint, options, replies) =
                            (endpoint.clone(), options.clone(), replies_tx.clone());
                        calls.spawn(async move {
                            let result = forward(endpoint.as_deref(), &call, &options).await;
                            let _ = replies.send(mcp::result(&id, result)).await;
                        });
                    }
                },
            },
            Some(_) = calls.join_next(), if !calls.is_empty() => {}
            _ = watch.tick() => {
                if let Some(endpoint) = &endpoint {
                    if present(endpoint) {
                        seen = true;
                    } else if seen {
                        break Ok(Ended::SocketGone);
                    }
                }
            }
        }
    };
    reader.abort();
    drop(replies_tx);
    match ended {
        Ok(Ended::InputClosed) => {
            // What was asked before the input closed is still answered, for
            // a client that closes its end and reads on.
            let _ = tokio::time::timeout(options.exit_grace, async {
                while calls.join_next().await.is_some() {}
            })
            .await;
            calls.abort_all();
            while let Ok(reply) = replies.try_recv() {
                write(&mut output, &reply).await?;
            }
        }
        _ => calls.abort_all(),
    }
    output.flush().await?;
    ended
}

async fn write<W: AsyncWrite + Unpin>(output: &mut W, message: &Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(message).map_err(std::io::Error::other)?;
    line.push(b'\n');
    output.write_all(&line).await?;
    output.flush().await
}

/// A tool result for one call: the app's answer, or why there is none.
async fn forward(endpoint: Option<&Endpoint>, call: &ToolCall, options: &Options) -> Value {
    let Some(endpoint) = endpoint else {
        return mcp::tool_result(NOT_CONFIGURED, true);
    };
    match ask(endpoint, call, options).await {
        Ok(reply) => mcp::tool_result(&reply.text, !reply.ok),
        Err(why) => mcp::tool_result(&why, true),
    }
}

/// How long one call may take end to end: the server's own allowance, and
/// whatever more the app is given to answer this call than an ordinary one.
pub fn call_budget(call: &ToolCall, options: &Options) -> Duration {
    options.call_timeout + call.answer_timeout().saturating_sub(DEFAULT_ANSWER_TIMEOUT)
}

async fn ask(endpoint: &Endpoint, call: &ToolCall, options: &Options) -> Result<Reply, String> {
    let stream = tokio::time::timeout(
        options.connect_timeout,
        UnixStream::connect(&endpoint.socket),
    )
    .await
    .map_err(|_| "FlowX did not accept the connection in time. It may be busy; try again.")?
    .map_err(unreachable)?;
    let (read, mut write) = stream.into_split();
    let request = Request {
        thread: endpoint.thread.clone(),
        token: endpoint.token.clone(),
        tool: call.name().into(),
        arguments: call.arguments(),
    };
    let exchange = async {
        write
            .write_all(&request.to_line())
            .await
            .map_err(|error| format!("FlowX closed the connection: {error}"))?;
        let mut reader = BufReader::new(read);
        read_line(&mut reader, MAX_REPLY_BYTES)
            .await
            .map_err(|error| format!("FlowX's answer could not be read: {error}"))
    };
    let line = tokio::time::timeout(call_budget(call, options), exchange)
        .await
        .map_err(|_| "FlowX did not answer in time. It may be busy; try again.".to_string())??;
    match line {
        Line::Data(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "FlowX sent an answer this server could not read.".to_string()),
        Line::TooLong => Err("FlowX's answer was too large to pass on.".into()),
        Line::End => Err("FlowX closed the connection without answering.".into()),
    }
}

fn unreachable(error: std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "FlowX is not running: its socket is gone. These \
            tools work only while the FlowX app that started you is open."
            .into(),
        std::io::ErrorKind::ConnectionRefused => "FlowX is not answering: nothing is \
            listening on its socket. The app may have quit."
            .into(),
        _ => format!("FlowX could not be reached: {error}"),
    }
}

/// The binary's whole MCP mode: serve stdio for the endpoint in the
/// environment, and exit.
pub fn run_from_env() -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("flowx: could not start the MCP server: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let options = Options {
        role: Role::from_value(std::env::var(crate::ENV_ROLE).ok().as_deref()),
        ..Options::default()
    };
    let ended = runtime.block_on(serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        Endpoint::from_env(),
        options,
    ));
    // A read of stdin blocked in the runtime's pool cannot be cancelled, and
    // waiting for it would keep the process after the socket went.
    runtime.shutdown_background();
    match ended {
        Ok(_) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flowx: the MCP server stopped: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listener::{Bridge, Call, handler};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, DuplexStream};

    fn fast() -> Options {
        Options {
            version: "test".into(),
            role: Role::Thread,
            watch_interval: Duration::from_millis(20),
            connect_timeout: Duration::from_secs(2),
            call_timeout: Duration::from_secs(5),
            exit_grace: Duration::from_secs(2),
        }
    }

    /// A client: what it writes, what it reads, and the server's ending.
    struct Client {
        input: DuplexStream,
        output: tokio::io::Lines<BufReader<DuplexStream>>,
        server: tokio::task::JoinHandle<std::io::Result<Ended>>,
    }

    impl Client {
        fn start(endpoint: Option<Endpoint>) -> Self {
            let (input, server_input) = tokio::io::duplex(64 * 1024);
            let (server_output, output) = tokio::io::duplex(1024 * 1024);
            let server = tokio::spawn(serve(server_input, server_output, endpoint, fast()));
            Self {
                input,
                output: BufReader::new(output).lines(),
                server,
            }
        }

        async fn send(&mut self, message: Value) {
            let mut line = serde_json::to_vec(&message).unwrap();
            line.push(b'\n');
            self.input.write_all(&line).await.unwrap();
        }

        async fn next(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(10), self.output.next_line())
                .await
                .expect("the server answered")
                .unwrap()
                .expect("a line");
            serde_json::from_str(&line).unwrap()
        }

        async fn call(&mut self, id: u64, name: &str, arguments: Value) -> Value {
            self.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                             "params": {"name": name, "arguments": arguments}}))
                .await;
            let reply = self.next().await;
            assert_eq!(reply["id"], id, "{reply}");
            reply["result"].clone()
        }
    }

    fn text(result: &Value) -> &str {
        result["content"][0]["text"].as_str().unwrap()
    }

    /// An app that answers every call by naming it and its thread.
    fn app(parent: &std::path::Path) -> Bridge {
        let answer = handler(|call: Call| async move {
            crate::wire::Reply::ok(format!("{} for {}", call.call.name(), call.thread))
        });
        Bridge::start(parent, answer).unwrap()
    }

    fn endpoint_for(bridge: &Bridge, thread: &str) -> Endpoint {
        Endpoint {
            socket: bridge.socket().to_path_buf(),
            thread: thread.into(),
            token: bridge.token_for(thread),
        }
    }

    #[tokio::test]
    async fn a_whole_session_is_answered_through_the_app() {
        let parent = tempfile::tempdir().unwrap();
        let bridge = app(parent.path());
        let thread = uuid::Uuid::new_v4().to_string();
        let mut client = Client::start(Some(endpoint_for(&bridge, &thread)));

        client
            .send(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                         "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "test", "version": "1"}}}))
            .await;
        let initialized = client.next().await;
        assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
        client
            .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        client
            .send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
            .await;
        // The notification was not answered: the next line is the listing.
        let listed = client.next().await;
        assert_eq!(listed["id"], 1);
        assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 2);

        let result = client.call(2, "list_threads", json!({})).await;
        assert_eq!(result["isError"], false);
        assert_eq!(text(&result), format!("list_threads for {thread}"));

        client
            .send(json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list"}))
            .await;
        assert_eq!(client.next().await["error"]["code"], mcp::METHOD_NOT_FOUND);

        drop(client.input);
        assert_eq!(client.server.await.unwrap().unwrap(), Ended::InputClosed);
    }

    #[tokio::test]
    async fn an_unreachable_app_is_the_tool_s_error_and_the_server_keeps_serving() {
        // A socket left behind by an app that crashed: there, but refusing.
        let parent = tempfile::tempdir().unwrap();
        let stale = parent.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        let thread = uuid::Uuid::new_v4().to_string();
        let mut client = Client::start(Some(Endpoint {
            socket: stale,
            thread,
            token: "t".into(),
        }));
        let result = client.call(1, "list_threads", json!({})).await;
        assert_eq!(result["isError"], true);
        assert!(text(&result).contains("FlowX is not answering"), "{result}");
        // Still serving.
        client
            .send(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}))
            .await;
        assert_eq!(
            client.next().await,
            json!({"jsonrpc": "2.0", "id": 2, "result": {}})
        );
    }

    #[tokio::test]
    async fn a_socket_never_there_and_a_missing_environment_are_both_explained() {
        let parent = tempfile::tempdir().unwrap();
        let mut client = Client::start(Some(Endpoint {
            socket: parent.path().join("never.sock"),
            thread: uuid::Uuid::new_v4().to_string(),
            token: "t".into(),
        }));
        let result = client.call(1, "list_threads", json!({})).await;
        assert!(text(&result).contains("FlowX is not running"), "{result}");
        // Never seen, so not gone: the server is still here.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!client.server.is_finished());

        let mut unconfigured = Client::start(None);
        let result = unconfigured.call(1, "list_threads", json!({})).await;
        assert_eq!(result["isError"], true);
        assert_eq!(text(&result), NOT_CONFIGURED);
    }

    #[tokio::test]
    async fn the_server_ends_when_the_app_s_socket_goes() {
        let parent = tempfile::tempdir().unwrap();
        let bridge = app(parent.path());
        let mut client = Client::start(Some(endpoint_for(&bridge, "thread")));
        client
            .send(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}))
            .await;
        client.next().await;
        bridge.shutdown();
        let ended = tokio::time::timeout(Duration::from_secs(5), client.server)
            .await
            .expect("the server ended")
            .unwrap()
            .unwrap();
        assert_eq!(ended, Ended::SocketGone);
    }

    #[tokio::test]
    async fn a_call_asked_just_before_the_input_closes_is_still_answered() {
        let parent = tempfile::tempdir().unwrap();
        let bridge = app(parent.path());
        let thread = uuid::Uuid::new_v4().to_string();
        let mut client = Client::start(Some(endpoint_for(&bridge, &thread)));
        client
            .send(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
                         "params": {"name": "list_threads", "arguments": {}}}))
            .await;
        client.input.shutdown().await.unwrap();
        let reply = client.next().await;
        assert_eq!(reply["id"], 9);
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(client.server.await.unwrap().unwrap(), Ended::InputClosed);
    }

    #[test]
    fn the_endpoint_needs_all_three_variables() {
        let full = |name: &str| Some(format!("value-of-{name}"));
        let endpoint = Endpoint::from_vars(full).unwrap();
        assert_eq!(endpoint.thread, "value-of-FLOWX_MCP_THREAD");
        for missing in [crate::ENV_SOCKET, crate::ENV_THREAD, crate::ENV_TOKEN] {
            assert!(
                Endpoint::from_vars(|name| (name != missing).then(|| "x".to_string())).is_none(),
                "{missing}"
            );
            assert!(
                Endpoint::from_vars(|name| Some(if name == missing { " " } else { "x" }.into()))
                    .is_none(),
                "{missing} blank"
            );
        }
    }
}
