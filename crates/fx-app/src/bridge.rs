//! FlowX's end of the MCP bridge: the tools an agent calls, answered from
//! app state.
//!
//! The listener (`fx_bridge::listener`) serves connections on a thread of its
//! own and hands each authenticated call here. App state lives on the UI
//! thread, so a call is queued to a foreground task, which reads the state
//! inside one `update` and answers: the same pattern turn updates use to get
//! back to the window. Anything slow is done back on the bridge's thread: a
//! thread FlowX has not loaded is read from its saved transcript there, never
//! on the UI thread.
//!
//! Three rules:
//!
//! - **A thread asks about its own project.** The listener has proved which
//!   thread is asking; `list_threads` lists that thread's project, and
//!   `read_thread` refuses a thread from any other.
//! - **The opt-in is read on every call.** A thread whose tools were turned
//!   off since its provider started is refused, though its server still runs
//!   and its token is still valid.
//! - **Only a thread that opted in is launched with the server**
//!   ([`BridgeAccess::server_for`]); for every other thread a turn launches
//!   exactly as it did before the bridge.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::StreamExt;
use fx_agent::mcp::McpServer;
use fx_bridge::answers::{self, Status, ThreadEntry};
use fx_bridge::listener::{Bridge, Call};
use fx_bridge::tools::ToolCall;
use fx_bridge::wire::Reply;
use fx_core::Agent;
use fx_design::components::ActivityState;
use gpui::{App, Entity};
use uuid::Uuid;

use crate::AppState;

/// What a turn needs to give its provider the bridge.
#[derive(Clone, Debug)]
pub struct BridgeAccess {
    bridge: Arc<Bridge>,
    /// This executable, which is also the MCP server.
    executable: PathBuf,
}

impl BridgeAccess {
    /// The server a thread's provider is launched with, or `None` for a
    /// thread that has not opted in.
    pub fn server_for(&self, agent: &Agent) -> Option<McpServer> {
        if !agent.configuration.uses_flowx_tools() {
            return None;
        }
        let thread = agent.id.to_string();
        Some(McpServer {
            name: fx_core::policy::flowx_tools::SERVER.into(),
            command: self.executable.to_string_lossy().into_owned(),
            args: vec![fx_bridge::SERVER_FLAG.into()],
            env: BTreeMap::from([
                (
                    fx_bridge::ENV_SOCKET.into(),
                    self.bridge.socket().to_string_lossy().into_owned(),
                ),
                (fx_bridge::ENV_TOKEN.into(), self.bridge.token_for(&thread)),
                (fx_bridge::ENV_THREAD.into(), thread),
            ]),
            tool_timeout: None,
        })
    }

    /// Stop listening and remove the socket.
    pub fn shutdown(&self) {
        self.bridge.shutdown();
    }
}

/// The running bridge, for whatever launches a turn.
pub struct BridgeHandle(pub BridgeAccess);

impl gpui::Global for BridgeHandle {}

impl BridgeHandle {
    pub fn access(cx: &App) -> Option<BridgeAccess> {
        cx.try_global::<Self>().map(|handle| handle.0.clone())
    }
}

/// Listen for this launch, in the user's temporary directory.
///
/// A bridge that cannot start costs only the tools: the app runs, and a
/// thread that opted in launches without them.
pub fn start(state: Entity<AppState>, cx: &mut App) {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            tracing::warn!(%error, "FlowX's tools are off: this executable could not be found");
            return;
        }
    };
    if let Err(error) = start_in(state, &std::env::temp_dir(), executable, cx) {
        tracing::warn!(%error, "FlowX's tools are off: the bridge could not listen");
    }
}

/// [`start`], with where to listen and what to launch named.
pub fn start_in(
    state: Entity<AppState>,
    parent: &Path,
    executable: PathBuf,
    cx: &mut App,
) -> std::io::Result<BridgeAccess> {
    let (jobs, mut queue) = futures::channel::mpsc::unbounded::<Job>();
    let bridge = Bridge::start(
        parent,
        fx_bridge::listener::handler(move |call| {
            let jobs = jobs.clone();
            async move { ask(&jobs, call).await }
        }),
    )?;
    let access = BridgeAccess {
        bridge: Arc::new(bridge),
        executable,
    };
    cx.set_global(BridgeHandle(access.clone()));
    cx.spawn(async move |cx| {
        while let Some(job) = queue.next().await {
            let answer = cx
                .update(|cx| answer(state.read(cx), &job.call))
                .unwrap_or_else(|_| Answer::Ready(Reply::refused(QUITTING)));
            let _ = job.reply.send(answer);
        }
    })
    .detach();
    let quitting = access.clone();
    cx.on_app_quit(move |_| {
        quitting.shutdown();
        async {}
    })
    .detach();
    Ok(access)
}

const QUITTING: &str = "FlowX is quitting, so it cannot answer.";

/// One call waiting for the UI thread.
struct Job {
    call: Call,
    reply: futures::channel::oneshot::Sender<Answer>,
}

/// What the UI thread decided.
pub(crate) enum Answer {
    Ready(Reply),
    /// The thread is not loaded: read its saved transcript off the UI thread.
    FromDisk(DiskRead),
}

/// A saved transcript to read, and what to say about it.
pub(crate) struct DiskRead {
    store: fx_store::Store,
    project_id: Uuid,
    entry: ThreadEntry,
    thread: Uuid,
    count: usize,
}

impl DiskRead {
    fn run(self) -> Reply {
        match self.store.load_conversation(self.project_id, self.thread) {
            Ok(Some(saved)) => {
                let (prose, total) = answers::latest_prose(saved.messages.iter(), self.count);
                Reply::ok(answers::thread_excerpt(&self.entry, &prose, total, None))
            }
            Ok(None) => Reply::ok(answers::thread_excerpt(&self.entry, &[], 0, None)),
            Err(error) => Reply::refused(format!(
                "FlowX could not read this thread's saved transcript: {error}"
            )),
        }
    }
}

/// Ask the UI thread, and finish off it whatever it hands back.
async fn ask(jobs: &futures::channel::mpsc::UnboundedSender<Job>, call: Call) -> Reply {
    let (reply, answered) = futures::channel::oneshot::channel();
    if jobs.unbounded_send(Job { call, reply }).is_err() {
        return Reply::refused(QUITTING);
    }
    match answered.await {
        Ok(Answer::Ready(reply)) => reply,
        Ok(Answer::FromDisk(read)) => tokio::task::spawn_blocking(move || read.run())
            .await
            .unwrap_or_else(|_| Reply::refused("FlowX could not read this thread.")),
        Err(_) => Reply::refused(QUITTING),
    }
}

/// Answer a call from app state. Reads only.
pub(crate) fn answer(state: &AppState, call: &Call) -> Answer {
    // Made current once, so each thread's lookups below are answered from
    // it rather than by a scan per thread.
    let _ = state.snapshot_index();
    let Some((record, asking)) = state.snapshot.projects.iter().find_map(|record| {
        record.project.as_ref()?;
        Some((record, shown(record, call.thread)?))
    }) else {
        return Answer::Ready(Reply::refused(
            "FlowX no longer has the thread these tools were started for.",
        ));
    };
    if !asking.configuration.uses_flowx_tools() {
        return Answer::Ready(Reply::refused(
            "FlowX's tools are turned off for this thread. The user can turn them on from \
             the thread's access menu.",
        ));
    }
    let project = record.project.as_ref().expect("found with a project");
    match call.call {
        ToolCall::ListThreads => {
            let threads: Vec<ThreadEntry> = record
                .agents
                .iter()
                .filter(|agent| !record.missing_native_agent_ids.contains(&agent.id))
                .map(|agent| entry(state, record, agent, call.thread))
                .collect();
            Answer::Ready(Reply::ok(answers::thread_list(
                &project.name,
                &project.root_path,
                &threads,
            )))
        }
        ToolCall::ReadThread { thread, count } => {
            let Some(agent) = shown(record, thread) else {
                return Answer::Ready(Reply::refused(
                    "No thread with that id is in this project. Call list_threads for the \
                     ids it has.",
                ));
            };
            let entry = entry(state, record, agent, call.thread);
            if let Some(conversation) = state.conversation_by_agent(thread) {
                let conversation = conversation
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let (prose, total) = answers::latest_prose(conversation.messages(), count);
                let partial = conversation
                    .is_busy()
                    .then(|| conversation.streaming_text());
                return Answer::Ready(Reply::ok(answers::thread_excerpt(
                    &entry, &prose, total, partial,
                )));
            }
            if state.store.has_conversation(project.id, thread) {
                return Answer::FromDisk(DiskRead {
                    store: state.store.clone(),
                    project_id: project.id,
                    entry,
                    thread,
                    count,
                });
            }
            if state.native_binding(thread).is_some() {
                return Answer::Ready(Reply::ok(format!(
                    "{}\nFlowX has not loaded this thread's history yet: it was started \
                     outside FlowX and has not been opened here. Ask the user to open it in \
                     FlowX, then read it again.",
                    answers::thread_header(&entry)
                )));
            }
            Answer::Ready(Reply::ok(answers::thread_excerpt(&entry, &[], 0, None)))
        }
        ToolCall::StartThread { .. }
        | ToolCall::SendToThread { .. }
        | ToolCall::WaitForThreads { .. }
        | ToolCall::StopThread { .. } => Answer::Ready(Reply::refused(
            "FlowX does not run a space's threads from its orchestrator yet.",
        )),
    }
}

/// A thread the project shows: listed, and not one its provider deleted.
fn shown(record: &fx_store::PersistedProject, thread: Uuid) -> Option<&Agent> {
    record
        .agents
        .iter()
        .find(|agent| agent.id == thread)
        .filter(|_| !record.missing_native_agent_ids.contains(&thread))
}

fn entry(
    state: &AppState,
    record: &fx_store::PersistedProject,
    agent: &Agent,
    asking: Uuid,
) -> ThreadEntry {
    // Read before the conversation is locked: both of these lock it too,
    // and the lock is not reentrant.
    let busy = state.is_thread_busy(agent.id);
    let elsewhere = state.waiting_elsewhere(agent.id).map(str::to_owned);
    let phase = state.conversation_by_agent(agent.id).map(|conversation| {
        conversation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .phase()
    });
    let (activity, _) = crate::thread_presentation::activity(
        phase,
        busy,
        agent.execution_state,
        elsewhere.as_deref(),
    );
    let title = agent.title.trim();
    ThreadEntry {
        id: agent.id.to_string(),
        title: if title.is_empty() {
            "Untitled thread".into()
        } else {
            title.into()
        },
        provider: match crate::thread_presentation::provider(state, agent) {
            fx_design::components::ProviderIdentity::Claude => "claude".into(),
            _ => "codex".into(),
        },
        status: match activity {
            ActivityState::Running => Status::Running,
            ActivityState::Waiting => Status::Waiting,
            ActivityState::Idle | ActivityState::Completed | ActivityState::Error => Status::Idle,
        },
        archived: record.archived_agent_ids.contains(&agent.id),
        current: agent.id == asking,
        in_space: None,
        spawned_by_you: None,
    }
}

/// A bridge for a test, listening in `parent` and answering nothing.
#[cfg(test)]
pub(crate) fn test_access(parent: &Path) -> BridgeAccess {
    BridgeAccess {
        bridge: Arc::new(
            Bridge::start(
                parent,
                fx_bridge::listener::handler(|_| async { Reply::refused("test") }),
            )
            .unwrap(),
        ),
        executable: PathBuf::from("/Applications/FlowX.app/Contents/MacOS/flowx"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx_agent::Phase;
    use fx_core::{AgentExecutionState, ConversationMessage, MessageRole, Project};
    use fx_store::{AppPaths, PersistedApp, PersistedProject, Store};
    use gpui::AppContext;

    struct Seeded {
        _dir: tempfile::TempDir,
        test: gpui::TestAppContext,
        state: Entity<AppState>,
        project: Project,
        asking: Agent,
        running: Agent,
        archived: Agent,
        saved: Agent,
        gone: Agent,
        other_project_thread: Agent,
    }

    fn agent(title: &str, provider: &str) -> Agent {
        let mut agent = Agent::new(title);
        agent.configuration.provider_id = Some(provider.into());
        agent
    }

    /// A project of five threads, one of them asking, and a second project.
    fn seeded() -> Seeded {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("state"));
        let project = Project::new(dir.path().join("site").to_str().unwrap());
        let mut asking = agent("Orchestrator", "claude");
        asking.configuration.flowx_tools = Some(true);
        let running = agent("Worker", "codex");
        let mut archived = agent("  ", "codex");
        archived.execution_state = AgentExecutionState::Success;
        let saved = agent("Saved", "claude");
        let gone = agent("Deleted by Codex", "codex");
        let other_project_thread = agent("Elsewhere", "codex");
        let store = Store::new(paths.clone());
        store
            .save_app(&PersistedApp {
                projects: vec![
                    PersistedProject {
                        project: Some(project.clone()),
                        agents: vec![
                            asking.clone(),
                            running.clone(),
                            archived.clone(),
                            saved.clone(),
                            gone.clone(),
                        ],
                        archived_agent_ids: vec![archived.id],
                        missing_native_agent_ids: vec![gone.id],
                        ..Default::default()
                    },
                    PersistedProject {
                        project: Some(Project::new(dir.path().join("other").to_str().unwrap())),
                        agents: vec![other_project_thread.clone()],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
            .unwrap();
        // A transcript on disk that nothing has loaded.
        let mut transcript = fx_agent::Conversation::new(saved.id);
        transcript.append(ConversationMessage::text(MessageRole::User, "Saved prompt"));
        transcript.append(ConversationMessage::text(
            MessageRole::Assistant,
            "Saved answer",
        ));
        store
            .save_conversation(
                project.id,
                saved.id,
                &crate::state::persisted_from(&transcript),
            )
            .unwrap();
        let test = gpui::TestAppContext::single();
        let state = test.update(|cx| cx.new(|cx| AppState::restore(paths, cx)));
        Seeded {
            _dir: dir,
            test,
            state,
            project,
            asking,
            running,
            archived,
            saved,
            gone,
            other_project_thread,
        }
    }

    fn ask_now(seeded: &Seeded, thread: Uuid, call: ToolCall) -> Reply {
        let answer = seeded
            .test
            .update(|cx| answer(seeded.state.read(cx), &Call { thread, call }));
        match answer {
            Answer::Ready(reply) => reply,
            Answer::FromDisk(read) => read.run(),
        }
    }

    #[test]
    fn list_threads_lists_the_asking_thread_s_project_and_what_each_is_doing() {
        let seeded = seeded();
        // The worker is mid-turn: its conversation says so.
        seeded.test.update(|cx| {
            seeded.state.update(cx, |state, _| {
                let conversation = state.conversation(seeded.project.id, seeded.running.id);
                conversation.lock().unwrap().set_phase(Phase::Running);
            })
        });
        let reply = ask_now(&seeded, seeded.asking.id, ToolCall::ListThreads);
        assert!(reply.ok, "{}", reply.text);
        let listing: serde_json::Value = serde_json::from_str(&reply.text).unwrap();
        assert_eq!(listing["project"]["name"], "site");
        assert_eq!(listing["project"]["folder"], seeded.project.root_path);
        let threads = listing["threads"].as_array().unwrap();
        let ids: Vec<&str> = threads.iter().map(|t| t["id"].as_str().unwrap()).collect();
        // Open threads first; the deleted one and the other project's are not listed.
        assert_eq!(
            ids,
            [
                seeded.asking.id.to_string(),
                seeded.running.id.to_string(),
                seeded.saved.id.to_string(),
                seeded.archived.id.to_string(),
            ]
        );
        assert!(!reply.text.contains(&seeded.gone.id.to_string()));
        assert!(
            !reply
                .text
                .contains(&seeded.other_project_thread.id.to_string())
        );
        assert_eq!(threads[0]["current"], true);
        assert_eq!(threads[0]["provider"], "claude");
        assert_eq!(threads[1]["current"], false);
        assert_eq!(threads[1]["status"], "running");
        assert_eq!(threads[1]["provider"], "codex");
        assert_eq!(threads[2]["status"], "idle");
        assert_eq!(threads[3]["archived"], true);
        assert_eq!(threads[3]["title"], "Untitled thread");
        // A finished turn is idle, not a fourth word.
        assert_eq!(threads[3]["status"], "idle");
    }

    #[test]
    fn read_thread_reads_a_loaded_thread_s_prose_and_its_reply_in_progress() {
        let seeded = seeded();
        seeded.test.update(|cx| {
            seeded.state.update(cx, |state, _| {
                let conversation = state.conversation(seeded.project.id, seeded.running.id);
                let mut conversation = conversation.lock().unwrap();
                conversation.append(ConversationMessage::text(MessageRole::User, "Build it"));
                conversation.append(ConversationMessage::new(
                    MessageRole::Assistant,
                    vec![fx_core::MessageContent::ToolUse {
                        id: "1".into(),
                        name: "Bash".into(),
                        input: "cargo build".into(),
                    }],
                ));
                conversation.set_phase(Phase::Running);
                conversation.apply(fx_agent::ProviderEvent::TextDelta("Building now".into()));
            })
        });
        let reply = ask_now(
            &seeded,
            seeded.asking.id,
            ToolCall::ReadThread {
                thread: seeded.running.id,
                count: 5,
            },
        );
        assert!(reply.ok, "{}", reply.text);
        assert!(reply.text.contains("· codex · running"), "{}", reply.text);
        assert!(reply.text.contains("[user]\nBuild it"), "{}", reply.text);
        assert!(!reply.text.contains("cargo build"), "{}", reply.text);
        assert!(
            reply
                .text
                .ends_with("[assistant, still writing]\nBuilding now"),
            "{}",
            reply.text
        );
    }

    #[test]
    fn read_thread_reads_a_thread_nothing_has_loaded_from_its_saved_transcript() {
        let seeded = seeded();
        let answer = seeded.test.update(|cx| {
            answer(
                seeded.state.read(cx),
                &Call {
                    thread: seeded.asking.id,
                    call: ToolCall::ReadThread {
                        thread: seeded.saved.id,
                        count: 1,
                    },
                },
            )
        });
        let Answer::FromDisk(read) = answer else {
            panic!("an unloaded thread was read on the UI thread");
        };
        let reply = read.run();
        assert!(reply.ok, "{}", reply.text);
        assert!(
            reply.text.contains("The last 1 of 2 messages"),
            "{}",
            reply.text
        );
        assert!(
            reply.text.ends_with("[assistant]\nSaved answer"),
            "{}",
            reply.text
        );
        // Nothing was loaded into the app by reading it.
        seeded.test.update(|cx| {
            assert!(
                seeded
                    .state
                    .read(cx)
                    .conversation_by_agent(seeded.saved.id)
                    .is_none()
            );
        });
    }

    #[test]
    fn a_provider_thread_flowx_has_not_opened_says_how_to_read_it() {
        let seeded = seeded();
        let binding: fx_store::PersistedBinding = serde_json::from_value(serde_json::json!({
            "agentId": seeded.running.id,
            "providerId": "codex",
            "sessionId": "codex-thread",
            "updatedAt": "2026-11-01T00:00:00Z",
        }))
        .unwrap();
        seeded.test.update(|cx| {
            seeded.state.update(cx, |state, _| {
                state.snapshot.projects[0].native_bindings.push(binding);
            })
        });
        let reply = ask_now(
            &seeded,
            seeded.asking.id,
            ToolCall::ReadThread {
                thread: seeded.running.id,
                count: 3,
            },
        );
        assert!(reply.ok, "{}", reply.text);
        assert!(
            reply.text.starts_with("Thread \"Worker\""),
            "{}",
            reply.text
        );
        assert!(
            reply.text.contains("Ask the user to open it in FlowX"),
            "{}",
            reply.text
        );
    }

    #[test]
    fn a_thread_reads_only_its_own_project_and_only_while_opted_in() {
        let seeded = seeded();
        for thread in [
            seeded.other_project_thread.id,
            seeded.gone.id,
            Uuid::new_v4(),
        ] {
            let reply = ask_now(
                &seeded,
                seeded.asking.id,
                ToolCall::ReadThread { thread, count: 1 },
            );
            assert!(!reply.ok, "{thread}: {}", reply.text);
            assert!(
                reply.text.contains("No thread with that id"),
                "{}",
                reply.text
            );
        }
        // A thread that never opted in is refused, token or not.
        let reply = ask_now(&seeded, seeded.running.id, ToolCall::ListThreads);
        assert!(!reply.ok);
        assert!(reply.text.contains("turned off"), "{}", reply.text);
        // And so is one that opted out after its provider started.
        seeded.test.update(|cx| {
            seeded.state.update(cx, |state, _| {
                state.snapshot.projects[0].agents[0]
                    .configuration
                    .flowx_tools = None;
            })
        });
        let reply = ask_now(&seeded, seeded.asking.id, ToolCall::ListThreads);
        assert!(!reply.ok);
        // A thread FlowX no longer has.
        let reply = ask_now(&seeded, Uuid::new_v4(), ToolCall::ListThreads);
        assert!(!reply.ok);
        assert!(reply.text.contains("no longer has"), "{}", reply.text);
    }

    #[test]
    fn only_a_thread_that_opted_in_is_given_the_server() {
        let parent = tempfile::tempdir().unwrap();
        let access = test_access(parent.path());
        let mut agent = Agent::new("Thread");
        assert_eq!(access.server_for(&agent), None);
        agent.configuration.flowx_tools = Some(false);
        assert_eq!(access.server_for(&agent), None);
        agent.configuration.flowx_tools = Some(true);
        let server = access.server_for(&agent).unwrap();
        assert_eq!(server.name, "flowx");
        assert_eq!(
            server.command,
            "/Applications/FlowX.app/Contents/MacOS/flowx"
        );
        assert_eq!(server.args, ["--mcp-server"]);
        let thread = agent.id.to_string();
        assert_eq!(server.env[fx_bridge::ENV_THREAD], thread);
        assert_eq!(
            server.env[fx_bridge::ENV_SOCKET],
            access.bridge.socket().to_string_lossy()
        );
        // The token is the thread's own.
        assert_eq!(
            server.env[fx_bridge::ENV_TOKEN],
            access.bridge.token_for(&thread)
        );
        assert_ne!(
            server.env[fx_bridge::ENV_TOKEN],
            access.bridge.token_for(&Uuid::new_v4().to_string())
        );
    }

    #[test]
    fn a_call_through_the_socket_is_answered_on_the_ui_thread() {
        let mut seeded = seeded();
        let parent = tempfile::tempdir().unwrap();
        let access = seeded.test.update(|cx| {
            start_in(
                seeded.state.clone(),
                parent.path(),
                PathBuf::from("/flowx"),
                cx,
            )
            .unwrap()
        });
        let server = access.server_for(&seeded.asking).unwrap();
        let request = fx_bridge::wire::Request {
            thread: server.env[fx_bridge::ENV_THREAD].clone(),
            token: server.env[fx_bridge::ENV_TOKEN].clone(),
            tool: "list_threads".into(),
            arguments: serde_json::json!({}),
        };
        let socket = PathBuf::from(&server.env[fx_bridge::ENV_SOCKET]);
        // The caller is a real process's worth of IO on a thread of its own;
        // the app answers when its foreground executor runs.
        let caller = std::thread::spawn(move || {
            use std::io::{BufRead, Write};
            let mut stream = std::os::unix::net::UnixStream::connect(socket).unwrap();
            stream.write_all(&request.to_line()).unwrap();
            let mut line = String::new();
            std::io::BufReader::new(stream)
                .read_line(&mut line)
                .unwrap();
            serde_json::from_str::<Reply>(&line).unwrap()
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !caller.is_finished() {
            assert!(std::time::Instant::now() < deadline, "no answer");
            seeded.test.run_until_parked();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let reply = caller.join().unwrap();
        assert!(reply.ok, "{}", reply.text);
        assert!(reply.text.contains(&seeded.running.id.to_string()));
        // The global the turn launcher reads is the one that answered.
        seeded.test.update(|cx| {
            let installed = BridgeHandle::access(cx).unwrap();
            assert_eq!(installed.bridge.socket(), access.bridge.socket());
        });
        access.shutdown();
        assert!(!access.bridge.socket().exists());
    }
}
