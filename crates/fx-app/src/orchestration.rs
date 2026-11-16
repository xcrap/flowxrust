//! A space's orchestrator, and the threads it runs.
//!
//! The thread that leads a space (`Agent::lead_of`) is given four tools
//! besides the reads: `start_thread`, `send_to_thread`, `wait_for_threads`
//! and `stop_thread`. They arrive through the bridge like any call (see
//! `bridge.rs`); this is what they do to app state. Six rules:
//!
//! - **Every call proves its lead.** The server lists these tools only for a
//!   lead, but that is a hint in its environment, so each call checks here
//!   that the asking thread leads a space that still exists.
//! - **A lead reaches its own space only.** A thread it sends to, waits for
//!   or stops is in the lead's project and the lead's space, and is not the
//!   lead itself.
//! - **Nothing slow runs on the UI thread.** Finding the provider's CLI runs
//!   on the bridge's thread and making a worktree on the Git worker; a wait
//!   sleeps on the bridge's thread and reads the threads here every
//!   [`POLL_INTERVAL`], each read a moment's work.
//! - **A start is checked twice**: before the slow part, and again when the
//!   thread is made, here, where nothing runs beside it. Two starts racing
//!   for the last of [`MAX_RUNNING_WORKERS`] cannot both take it.
//! - **A worker starts as a new thread does**: the provider's defaults, the
//!   lead's access, its title kept as given, FlowX's tools off, and not
//!   selected, because the reader is watching the lead.
//! - **The usage left is the user's.** Past [`LIMIT_WARNING_AT`] of any
//!   window, or once the provider refused for the limit, a lead starts
//!   nothing on that provider.

use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fx_bridge::answers::{self, WorkerReport, WorkerStatus};
use fx_bridge::listener::Call;
use fx_bridge::tools::{STOP_SETTLE, ToolCall, Until};
use fx_bridge::wire::Reply;
use fx_core::models::agent::ChatWorkspace;
use fx_core::services::runtime::{BinarySpec, RuntimeDiscovery, Validation};
use fx_core::{Agent, AgentAccess, AgentExecutionState};
use fx_design::components::LIMIT_WARNING_AT;
use gpui::{App, Entity};
use uuid::Uuid;

use super::AppState;
use crate::bridge::Answer;

pub use fx_bridge::tools::MAX_RUNNING_WORKERS;

/// How often a wait reads the threads it waits for.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

const NO_THREAD: &str = "FlowX no longer has the thread these tools were started for.";

/// The asking thread, as a space's orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lead {
    pub id: Uuid,
    pub space: Uuid,
    pub project_id: Uuid,
    /// The project's folder, where a worker without a worktree runs and
    /// from whose checkout a worktree is made.
    pub project_root: String,
    /// The lead's provider and access, resolved as its own turns are.
    pub provider: String,
    pub access: AgentAccess,
}

impl AppState {
    /// The asking thread as a lead, or why it is not one, in words the model
    /// can act on.
    pub(crate) fn lead(&self, asking: Uuid, tool: &str) -> Result<Lead, String> {
        let (record, agent) = self
            .snapshot
            .projects
            .iter()
            .filter(|record| record.project.is_some())
            .find_map(|record| {
                let agent = record.agents.iter().find(|agent| agent.id == asking)?;
                Some((record, agent))
            })
            .filter(|(record, _)| !record.missing_native_agent_ids.contains(&asking))
            .ok_or(NO_THREAD)?;
        let Some(space) = agent.lead_of else {
            return Err(format!(
                "{tool} is only for a FlowX space's orchestrator, and this thread does not \
                 lead a space. You can still use list_threads and read_thread; to run \
                 other threads, ask the user to do this from a space's orchestrator."
            ));
        };
        if !record.spaces.iter().any(|kept| kept.space.id == space) {
            return Err(
                "The space this thread led has been deleted, so there is nothing \
                        for it to orchestrate. Ask the user to open a space's orchestrator."
                    .into(),
            );
        }
        // One lead per space, by the one rule its header follows. A second
        // thread naming the space is drawn as an ordinary row, and as a lead
        // it could send to or stop the one the reader sees.
        if self.space_lead(space) != Some(asking) {
            return Err(format!(
                "{tool} is only for a FlowX space's orchestrator, and another thread now \
                 leads this space. Ask the user to open the space's orchestrator."
            ));
        }
        // Only inside a turn the reader started: a start still being made
        // when the reader pressed Stop, finished afterwards, would run a
        // thread nobody asked for.
        let stopping = self.conversations.get(&asking).is_some_and(|conversation| {
            conversation
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .phase()
                == fx_agent::Phase::Cancelling
        });
        if !self.is_thread_busy(asking) || stopping {
            return Err(format!(
                "{tool} works only while this thread's own turn runs, and it has ended."
            ));
        }
        let project = record.project.as_ref().expect("filtered on a project");
        let resolved =
            crate::configuration::with_native_defaults(agent.clone(), self.native_defaults(asking));
        Ok(Lead {
            id: asking,
            space,
            project_id: project.id,
            project_root: project.root_path.clone(),
            provider: resolved
                .configuration
                .provider_id
                .clone()
                .unwrap_or_else(|| "codex".into()),
            access: resolved
                .configuration
                .resolved_access(AgentAccess::default()),
        })
    }

    /// A thread of the lead's space, or why the lead cannot reach it.
    pub(crate) fn worker(&self, lead: &Lead, thread: Uuid) -> Result<&Agent, String> {
        if thread == lead.id {
            return Err("That is your own thread. Name a thread you started, as \
                        start_thread or list_threads gave it."
                .into());
        }
        self.space_threads(lead)
            .find(|agent| agent.id == thread)
            .ok_or_else(|| {
                format!(
                    "No thread with the id {thread} is in your space. Call list_threads: the \
                     threads with `in_space` true are the ones you can reach."
                )
            })
    }

    /// The threads of the lead's space, the lead aside.
    fn space_threads(&self, lead: &Lead) -> impl Iterator<Item = &Agent> + '_ {
        let (project, space, lead) = (lead.project_id, lead.space, lead.id);
        self.snapshot
            .projects
            .iter()
            .filter(move |record| {
                record
                    .project
                    .as_ref()
                    .is_some_and(|found| found.id == project)
            })
            .flat_map(move |record| {
                record.agents.iter().filter(move |agent| {
                    agent.id != lead
                        && agent.space == Some(space)
                        && !record.missing_native_agent_ids.contains(&agent.id)
                        // Another space's lead is never a worker, wherever
                        // it is filed.
                        && !agent
                            .lead_of
                            .is_some_and(|led| self.space_lead(led) == Some(agent.id))
                })
            })
    }

    /// How many of the lead's space's threads have a turn under way, those
    /// waiting on the user among them: each holds a provider process.
    pub(crate) fn running_workers(&self, lead: &Lead) -> usize {
        self.space_threads(lead)
            .filter(|agent| self.is_thread_busy(agent.id))
            .count()
    }

    /// Whether the lead may start another turn on `provider`.
    pub(crate) fn may_start(&self, lead: &Lead, provider: &str) -> Result<(), String> {
        if let Some(refusal) = self.usage_refusal(provider) {
            return Err(refusal);
        }
        let running = self.running_workers(lead);
        if running >= MAX_RUNNING_WORKERS {
            return Err(format!(
                "{running} of your space's threads are already running, the most FlowX \
                 runs at once in one space. Wait for one with wait_for_threads (until: \
                 \"any\"), or stop one with stop_thread, and then try again."
            ));
        }
        Ok(())
    }

    /// Why no new work may start on `provider`, if its account's usage is
    /// past the warning line or it refused a request for the limit. A window
    /// that has reset since it was reported no longer counts.
    pub(crate) fn usage_refusal(&self, provider: &str) -> Option<String> {
        let limits = self.rate_limits_for(provider)?;
        let now = fx_core::models::now();
        let current = fx_agent::provider::RateLimits {
            windows: limits
                .windows
                .iter()
                .filter(|window| window.resets_at.is_none_or(|resets| resets > now))
                .cloned()
                .collect(),
            ..limits.clone()
        };
        // Whether a refusal still holds is read from every window it was
        // reported with, the ones that have reset since included.
        let (_, limited) = crate::thread_presentation::usage_limits(limits, now);
        let (windows, _) = crate::thread_presentation::usage_limits(&current, now);
        let hottest = windows
            .iter()
            .max_by(|a, b| a.fraction.total_cmp(&b.fraction));
        let pressing = hottest.filter(|window| window.fraction >= LIMIT_WARNING_AT);
        if !limited && pressing.is_none() {
            return None;
        }
        let name = fx_core::policy::review::provider_label(provider);
        // Past the line, the window says the most; otherwise the provider
        // refused, which is all there is to say.
        let what = match pressing {
            Some(window) => format!(
                "{name}'s {} is {:.0}% used{}",
                window.label.to_lowercase(),
                window.fraction * 100.0,
                window
                    .resets
                    .as_ref()
                    .map(|resets| format!(" ({resets})"))
                    .unwrap_or_default()
            ),
            None => format!("{name} refused its last request for the account's usage limit"),
        };
        Some(format!(
            "{what}. FlowX starts no work for an orchestrator once a usage limit is {:.0}% \
             used, so what is left stays the user's. Tell the user: they can raise it by \
             resetting their usage, or wait for the limit to reset.",
            LIMIT_WARNING_AT * 100.0
        ))
    }

    /// The provider a thread runs on.
    fn provider_of(&self, agent: &Agent) -> String {
        agent
            .configuration
            .provider_id
            .clone()
            .or_else(|| self.native_defaults(agent.id).provider_id)
            .unwrap_or_else(|| "codex".into())
    }

    /// What a thread is doing, and what it waits for when the user has to
    /// act.
    pub(crate) fn worker_status(&self, agent: &Agent) -> (WorkerStatus, Option<String>) {
        let id = agent.id;
        if self.is_thread_busy(id) {
            let asked = self
                .runners
                .contains_key(&id)
                .then(|| self.pending_interactions.get(&id))
                .flatten()
                .and_then(|queue| queue.front())
                .and_then(waiting_for);
            if let Some(asked) = asked {
                return (WorkerStatus::Waiting, Some(asked));
            }
            let phase = self.conversations.get(&id).map(|conversation| {
                conversation
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .phase()
            });
            return match phase {
                Some(
                    phase
                    @ (fx_agent::Phase::WaitingForApproval | fx_agent::Phase::WaitingForInput),
                ) => (WorkerStatus::Waiting, Some(phase.label().to_lowercase())),
                _ => (WorkerStatus::Running, None),
            };
        }
        if let Some(elsewhere) = self.waiting_elsewhere(id) {
            return (
                WorkerStatus::Waiting,
                Some(format!("the user, in another app: {elsewhere}")),
            );
        }
        // A turn the provider's own app runs is running, whatever FlowX
        // last saved: read as settled, a wait ended at once on a thread
        // still at work.
        if self.running_elsewhere(id) {
            return (WorkerStatus::Running, None);
        }
        let status = match agent.execution_state {
            AgentExecutionState::Success => WorkerStatus::Completed,
            AgentExecutionState::Failure => WorkerStatus::Failed,
            // A stopped turn is saved as idle, as is one that outlived the
            // process that ran it: either way it ended without answering.
            AgentExecutionState::Idle
            | AgentExecutionState::Running
            | AgentExecutionState::WaitingForApproval => {
                if self.has_had_a_turn(agent.id) {
                    WorkerStatus::Stopped
                } else {
                    WorkerStatus::Idle
                }
            }
        };
        (status, None)
    }

    fn has_had_a_turn(&self, id: Uuid) -> bool {
        match self.conversations.get(&id) {
            Some(conversation) => conversation
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .messages()
                .any(|message| message.role == fx_core::MessageRole::User),
            None => {
                self.native_binding(id).is_some()
                    || self
                        .project_for_agent(id)
                        .is_some_and(|project| self.store.has_conversation(project, id))
            }
        }
    }

    /// Each thread a wait asks about, read now.
    pub(crate) fn poll_workers(&self, poll: &Poll) -> Result<Vec<Polled>, String> {
        let lead = self.lead(poll.lead, poll.tool)?;
        Ok(poll
            .threads
            .iter()
            .map(|&thread| match self.worker(&lead, thread) {
                Ok(agent) => self.polled(&lead, agent),
                // Deleted, or moved out of the space, since the wait began.
                Err(_) => Polled {
                    report: WorkerReport::new(
                        thread.to_string(),
                        "A thread no longer in your space".into(),
                        WorkerStatus::Stopped,
                        None,
                        None,
                    ),
                    unread: None,
                },
            })
            .collect())
    }

    fn polled(&self, lead: &Lead, agent: &Agent) -> Polled {
        let (status, waiting) = self.worker_status(agent);
        let loaded = self.conversations.get(&agent.id).map(|conversation| {
            fx_core::policy::review::findings(
                conversation
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .messages(),
            )
        });
        let title = agent.title.trim();
        Polled {
            report: WorkerReport::new(
                agent.id.to_string(),
                if title.is_empty() {
                    "Untitled thread".into()
                } else {
                    title.into()
                },
                status,
                waiting,
                loaded.as_ref().and_then(Option::as_deref),
            ),
            // Nothing loaded: its last reply is in its saved transcript,
            // read off this thread when the wait ends.
            unread: loaded.is_none().then(|| Unread {
                store: self.store.clone(),
                project: lead.project_id,
                thread: agent.id,
            }),
        }
    }

    /// Make the thread a start asked for, in the lead's space, saved, and
    /// not selected.
    fn create_worker(
        &mut self,
        lead: &Lead,
        plan: &StartPlan,
        workspace: Option<ChatWorkspace>,
    ) -> Result<Uuid, String> {
        let mut agent = Agent::with_id(plan.thread, plan.title.clone());
        agent.configuration = self.default_configuration(&plan.provider);
        agent.configuration.agent_access = Some(lead.access);
        agent.configuration.flowx_tools = None;
        agent.space = Some(lead.space);
        agent.spawned_by = Some(lead.id);
        agent.workspace = workspace;
        self.insert_thread(lead.project_id, agent, false, false)
    }
}

/// What a waiting thread asks the user, in a few words.
fn waiting_for(event: &fx_agent::ProviderEvent) -> Option<String> {
    let clip = |text: &str| {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.chars().count() > 200 {
            format!("{}…", text.chars().take(200).collect::<String>())
        } else {
            text
        }
    };
    match event {
        fx_agent::ProviderEvent::Approval(request) => {
            // Claude's `Bash` names its command at the top; Codex's
            // `commandExecution` under `arguments`, as argv or a line.
            let command = request
                .input
                .get("arguments")
                .unwrap_or(&request.input)
                .get("command")
                .and_then(|command| match command {
                    serde_json::Value::String(line) => Some(line.clone()),
                    serde_json::Value::Array(argv) => Some(
                        argv.iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    _ => None,
                });
            Some(match command {
                Some(command) => format!("approval to run `{}`", clip(&command)),
                None => format!("approval to use {}", request.tool_name),
            })
        }
        fx_agent::ProviderEvent::UserInput(request) => Some(match request.questions.as_slice() {
            [] => format!(
                "a confirmation{}",
                request
                    .message
                    .as_deref()
                    .map(|message| format!(": {}", clip(message)))
                    .unwrap_or_default()
            ),
            [only] => format!("an answer to \"{}\"", clip(&only.question)),
            [first, rest @ ..] => format!(
                "answers to \"{}\" and {} more question{}",
                clip(&first.question),
                rest.len(),
                if rest.len() == 1 { "" } else { "s" }
            ),
        }),
        _ => None,
    }
}

/// Answer an orchestration call, on the UI thread.
pub(crate) fn answer(state: &Entity<AppState>, call: &Call, cx: &mut App) -> Answer {
    let tool = call.call.name();
    match &call.call {
        ToolCall::StartThread {
            title,
            prompt,
            provider,
            worktree,
        } => start(
            state.read(cx),
            call.thread,
            StartRequest {
                title: title.clone(),
                prompt: prompt.clone(),
                provider: provider.clone(),
                worktree: *worktree,
            },
            cx,
        ),
        ToolCall::SendToThread { thread, prompt } => {
            Answer::Ready(send(state, call.thread, *thread, prompt, cx))
        }
        ToolCall::WaitForThreads {
            threads,
            until,
            timeout_seconds,
        } => {
            let state = state.read(cx);
            let lead = match state.lead(call.thread, tool) {
                Ok(lead) => lead,
                Err(why) => return Answer::Ready(Reply::refused(why)),
            };
            let outside: Vec<String> = threads
                .iter()
                .filter(|thread| state.worker(&lead, **thread).is_err())
                .map(Uuid::to_string)
                .collect();
            if !outside.is_empty() {
                return Answer::Ready(Reply::refused(format!(
                    "Not in your space: {}. Wait only for threads you can reach: call \
                     list_threads, and pass the ids with `in_space` true.",
                    outside.join(", ")
                )));
            }
            Answer::Wait(WaitPlan {
                lead: lead.id,
                tool,
                threads: threads.clone(),
                until: *until,
                timeout: Duration::from_secs(*timeout_seconds),
                shape: Shape::Wait,
            })
        }
        ToolCall::StopThread { thread } => {
            let checked = {
                let state = state.read(cx);
                state
                    .lead(call.thread, tool)
                    .and_then(|lead| state.worker(&lead, *thread).map(|_| lead))
            };
            let lead = match checked {
                Ok(lead) => lead,
                Err(why) => return Answer::Ready(Reply::refused(why)),
            };
            // As the reader's Stop does: the runner interrupts the provider
            // and ends every tool it started.
            let was_running = state
                .read(cx)
                .runners
                .get(thread)
                .map(crate::session::TurnRunner::interrupt)
                .is_some();
            Answer::Wait(WaitPlan {
                lead: lead.id,
                tool,
                threads: vec![*thread],
                until: Until::All,
                timeout: if was_running {
                    STOP_SETTLE
                } else {
                    Duration::ZERO
                },
                shape: Shape::Stop { was_running },
            })
        }
        ToolCall::ListThreads | ToolCall::ReadThread { .. } => Answer::Ready(Reply::refused(
            "FlowX routed a read to its orchestration tools. Try again.",
        )),
    }
}

/// What `start_thread` asked for.
struct StartRequest {
    title: String,
    prompt: String,
    provider: Option<String>,
    worktree: bool,
}

/// `start_thread`'s first half, on the UI thread: everything that reads
/// state, before the slow part.
fn start(state: &AppState, asking: Uuid, request: StartRequest, cx: &App) -> Answer {
    let lead = match state.lead(asking, fx_core::policy::flowx_tools::START_THREAD) {
        Ok(lead) => lead,
        Err(why) => return Answer::Ready(Reply::refused(why)),
    };
    let provider = request.provider.unwrap_or_else(|| lead.provider.clone());
    if let Err(why) = state.may_start(&lead, &provider) {
        return Answer::Ready(Reply::refused(why));
    }
    let Some(runtime) = crate::session::provider_runtime(&provider, std::env::vars().collect(), cx)
    else {
        return Answer::Ready(Reply::refused(format!(
            "FlowX cannot run {provider} threads. Pass `provider`: \"claude\" or \"codex\"."
        )));
    };
    let thread = Uuid::new_v4();
    let worktree = request.worktree.then(|| WorktreePlan {
        root: PathBuf::from(&lead.project_root),
        // Where the New worktree dialog puts one: private app data, by
        // project and thread.
        destination: state
            .store
            .paths()
            .root()
            .join("worktrees")
            .join(lead.project_id.to_string())
            .join(thread.to_string()),
        branch: format!("flowx/{}", &thread.simple().to_string()[..8]),
    });
    Answer::Start(Box::new(StartPlan {
        lead,
        thread,
        title: request.title,
        prompt: request.prompt,
        provider,
        runtime,
        worktree,
    }))
}

/// A start, checked, waiting for its slow half.
pub(crate) struct StartPlan {
    lead: Lead,
    /// The id the thread will have, chosen first so its worktree can be
    /// named after it.
    thread: Uuid,
    title: String,
    prompt: String,
    provider: String,
    runtime: (RuntimeDiscovery, BinarySpec),
    worktree: Option<WorktreePlan>,
}

struct WorktreePlan {
    root: PathBuf,
    destination: PathBuf,
    branch: String,
}

impl WorktreePlan {
    /// A linked checkout on a new branch at the current commit, made on the
    /// Git worker as the New worktree dialog makes one.
    async fn make(&self) -> Result<ChatWorkspace, String> {
        if !self.root.join(".git").exists() {
            return Err(format!(
                "{} is not a Git repository, so this thread cannot have a worktree. Start \
                 it without `worktree`, or ask the user to make the project a repository.",
                self.root.display()
            ));
        }
        let (root, destination, branch) = (
            self.root.clone(),
            self.destination.clone(),
            self.branch.clone(),
        );
        let job = crate::inspector::repository::spawn_git(move || async move {
            if let Some(parent) = destination.parent() {
                fx_store::paths::create_private_dir(parent).map_err(|e| e.to_string())?;
            }
            fx_core::git::worktree::create(&root, &destination, &branch).await
        });
        crate::inspector::repository::finish_git(job)
            .await
            .map_err(|error| format!("FlowX could not make a worktree for this thread: {error}"))
    }
}

/// A start whose provider is there and whose worktree, if any, is made.
pub(crate) struct Launch {
    plan: StartPlan,
    workspace: Option<ChatWorkspace>,
}

impl StartPlan {
    /// `start_thread`'s slow half, off the UI thread: that the provider is
    /// installed, and the worktree.
    pub(crate) async fn prepare(self) -> Result<Launch, Reply> {
        let (discovery, spec) = self.runtime.clone();
        // A login shell may be asked where the binary is, and a signature
        // checked: blocking work, kept off the bridge's own thread too.
        let found = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(discovery.find(&spec))
        })
        .await
        .unwrap_or(Validation::Missing);
        let name = &self.runtime.1.display_name;
        match found {
            Validation::Usable(_) => {}
            Validation::Quarantined(path) => {
                return Err(Reply::refused(format!(
                    "{name} at {} has not been approved to run on this Mac, so FlowX cannot \
                     start a thread on it. Ask the user to approve it, or start the thread \
                     with the other provider.",
                    path.display()
                )));
            }
            Validation::Missing => {
                return Err(Reply::refused(format!(
                    "{name} is not installed, so FlowX cannot start a {} thread. Start it \
                     with the other provider, or ask the user to install it.{}",
                    self.provider,
                    self.runtime
                        .1
                        .install_hint
                        .as_deref()
                        .map(|hint| format!(" {hint}"))
                        .unwrap_or_default()
                )));
            }
        }
        let workspace = match &self.worktree {
            Some(worktree) => Some(worktree.make().await.map_err(Reply::refused)?),
            None => None,
        };
        Ok(Launch {
            plan: self,
            workspace,
        })
    }
}

/// `start_thread`'s last half, on the UI thread: check again, make the
/// thread, and start its first turn.
pub(crate) fn launch(state: &Entity<AppState>, launch: Launch, cx: &mut App) -> Reply {
    let Launch { plan, workspace } = launch;
    let kept = workspace
        .as_ref()
        .map(|workspace| {
            format!(
                " Its worktree was made and is kept at {} on branch {}.",
                workspace.path, workspace.branch
            )
        })
        .unwrap_or_default();
    // The slow half gave the lead time to change and another start time to
    // take the last slot.
    let checked = {
        let state = state.read(cx);
        state
            .lead(plan.lead.id, fx_core::policy::flowx_tools::START_THREAD)
            .and_then(|lead| state.may_start(&lead, &plan.provider).map(|()| lead))
    };
    let lead = match checked {
        Ok(lead) => lead,
        Err(why) => return Reply::refused(format!("{why}{kept}")),
    };
    // The thread is saved as it is made; the transcripts streaming
    // meanwhile are the writer thread's, never flushed from here.
    let created = state.update(cx, |state, cx| {
        let created = state.create_worker(&lead, &plan, workspace.clone());
        cx.notify();
        created
    });
    let id = match created {
        Ok(id) => id,
        Err(why) => {
            return Reply::refused(format!("FlowX could not make the thread: {why}{kept}"));
        }
    };
    let (agent, root) = {
        let state = state.read(cx);
        (state.thread(id).cloned(), state.project_root_for(id))
    };
    let started = match (agent, root) {
        (Some(agent), Some(root)) => {
            crate::start_turn_with(state, &root, agent, plan.prompt.clone(), Vec::new(), cx)
        }
        _ => false,
    };
    state.update(cx, |_, cx| cx.notify());
    if !started {
        return Reply::refused(format!(
            "FlowX made the thread \"{}\" ({id}) in your space but could not start its first \
             turn. Send it the prompt with send_to_thread, or ask the user to look at it.",
            plan.title
        ));
    }
    Reply::ok(answers::thread_started(
        &id.to_string(),
        &plan.title,
        &plan.provider,
        workspace
            .as_ref()
            .map(|workspace| (workspace.path.as_str(), workspace.branch.as_str())),
    ))
}

/// `send_to_thread`, on the UI thread: a turn started as the composer
/// starts one.
fn send(state: &Entity<AppState>, asking: Uuid, thread: Uuid, prompt: &str, cx: &mut App) -> Reply {
    let checked = {
        let state = state.read(cx);
        state
            .lead(asking, fx_core::policy::flowx_tools::SEND_TO_THREAD)
            .and_then(|lead| {
                let worker = state.worker(&lead, thread)?;
                let title = worker.title.trim().to_owned();
                // A turn in another app holds the session too: a second
                // process resuming it would answer beside it.
                if state.is_thread_busy(thread)
                    || state.running_elsewhere(thread)
                    || state.waiting_elsewhere(thread).is_some()
                {
                    return Err(match state.worker_status(worker) {
                        (WorkerStatus::Waiting, Some(what)) => format!(
                            "\"{title}\" is waiting for the user ({what}), so it takes no \
                             prompt until they answer. Tell the user."
                        ),
                        _ => format!("\"{title}\" is still running: call wait_for_threads first."),
                    });
                }
                state.may_start(&lead, &state.provider_of(worker))?;
                let root = state.project_root_for(thread).ok_or(
                    "That thread's folder is no longer available. Ask the user to look at it.",
                )?;
                Ok((worker.clone(), root, lead.project_id))
            })
    };
    let (agent, root, project_id) = match checked {
        Ok(target) => target,
        Err(why) => return Reply::refused(why),
    };
    // A turn starts from the loaded transcript. One nothing has loaded is
    // read in the background, as opening it does, never here on the UI
    // thread, and the lead asks again.
    let loaded = state.update(cx, |state, cx| {
        state.request_conversation(project_id, thread, cx).is_some()
    });
    if !loaded {
        return Reply::refused(format!(
            "FlowX is opening \"{}\"'s saved transcript. Call send_to_thread again in a \
             moment.",
            agent.title.trim()
        ));
    }
    if !crate::start_turn_with(state, &root, agent, prompt.to_owned(), Vec::new(), cx) {
        return Reply::refused(
            "FlowX could not start a turn on that thread. Call wait_for_threads to see what \
             it is doing.",
        );
    }
    state.update(cx, |_, cx| cx.notify());
    Reply::ok(answers::prompt_sent(&thread.to_string()))
}

/// What a wait reads on each poll.
pub(crate) struct Poll {
    lead: Uuid,
    tool: &'static str,
    threads: Vec<Uuid>,
}

/// One thread, as one poll read it.
pub(crate) struct Polled {
    report: WorkerReport,
    /// Where its last reply is, when nothing has loaded its transcript.
    unread: Option<Unread>,
}

struct Unread {
    store: fx_store::Store,
    project: Uuid,
    thread: Uuid,
}

/// How a wait's answer reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Wait,
    /// `stop_thread`, which waits for the stopped turn to settle.
    Stop {
        was_running: bool,
    },
}

/// A wait, checked, run off the UI thread.
pub(crate) struct WaitPlan {
    lead: Uuid,
    tool: &'static str,
    threads: Vec<Uuid>,
    until: Until,
    timeout: Duration,
    shape: Shape,
}

impl WaitPlan {
    /// Poll until the threads settle as asked or the time is up, sleeping
    /// between polls on the caller's runtime. `poll` asks the UI thread.
    pub(crate) async fn run<F, Fut>(self, mut poll: F) -> Reply
    where
        F: FnMut(Poll) -> Fut,
        Fut: Future<Output = Answer>,
    {
        let started = Instant::now();
        let deadline = started + self.timeout;
        loop {
            let polled = match poll(Poll {
                lead: self.lead,
                tool: self.tool,
                threads: self.threads.clone(),
            })
            .await
            {
                Answer::Polled(Ok(polled)) => polled,
                Answer::Polled(Err(why)) => return Reply::refused(why),
                Answer::Ready(reply) => return reply,
                _ => return Reply::refused("FlowX could not read the threads."),
            };
            let settled = |thread: &Polled| thread.report.status.is_settled();
            let settled = match self.until {
                Until::All => polled.iter().all(settled),
                Until::Any => polled.iter().any(settled),
            };
            let now = Instant::now();
            if settled || now >= deadline {
                let reports = read_unloaded(polled).await;
                return Reply::ok(match self.shape {
                    Shape::Wait => answers::wait_result(
                        self.until,
                        settled,
                        now.duration_since(started).as_secs(),
                        &reports,
                    ),
                    Shape::Stop { was_running } => answers::stop_result(&reports[0], was_running),
                });
            }
            tokio::time::sleep(POLL_INTERVAL.min(deadline - now)).await;
        }
    }
}

/// The reports, with each unloaded thread's last reply read from its saved
/// transcript, off the caller's thread.
async fn read_unloaded(polled: Vec<Polled>) -> Vec<WorkerReport> {
    let mut reports = Vec::with_capacity(polled.len());
    for Polled { mut report, unread } in polled {
        if let Some(Unread {
            store,
            project,
            thread,
        }) = unread
        {
            let reply = tokio::task::spawn_blocking(move || {
                store
                    .load_conversation(project, thread)
                    .ok()
                    .flatten()
                    .and_then(|saved| fx_core::policy::review::findings(saved.messages.iter()))
            })
            .await
            .ok()
            .flatten();
            report = WorkerReport::new(
                report.thread_id,
                report.title,
                report.status,
                report.waiting_for,
                reply.as_deref(),
            );
        }
        reports.push(report);
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{Work, respond};
    use crate::session::{TestRuntimes, TurnRunner};
    use fx_agent::Phase;
    use fx_agent::provider::{RateLimitKind, RateLimitStatus, RateLimitWindow, RateLimits};
    use fx_core::{ConversationMessage, MessageRole, Project};
    use fx_store::{AppPaths, PersistedApp, PersistedProject, PersistedSpace};
    use gpui::AppContext;
    use serde_json::{Value, json};

    /// A project with a space led by a Claude thread, two threads of that
    /// space, a thread in no space that opted in to the tools, a thread in
    /// another space, and a second project. Each test owns its directory.
    struct Fixture {
        dir: tempfile::TempDir,
        test: gpui::TestAppContext,
        state: Entity<AppState>,
        project: Project,
        space: Uuid,
        lead: Agent,
        /// Started by the lead, on Codex.
        codex: Agent,
        /// Put in the space by the reader, on Claude.
        claude: Agent,
        ordinary: Agent,
        elsewhere: Agent,
        other_project: Agent,
    }

    fn agent(title: &str, provider: &str) -> Agent {
        let mut agent = Agent::new(title);
        agent.configuration.provider_id = Some(provider.into());
        agent
    }

    fn seeded() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site");
        std::fs::create_dir_all(&root).unwrap();
        let project = Project::new(root.to_str().unwrap());
        let space = fx_core::Space::new("Parser");
        let other_space = fx_core::Space::new("Docs");
        let mut lead = agent("Orchestrator", "claude");
        lead.lead_of = Some(space.id);
        lead.space = Some(space.id);
        lead.configuration.agent_access = Some(AgentAccess::Supervised);
        let mut codex = agent("Lexer", "codex");
        codex.space = Some(space.id);
        codex.spawned_by = Some(lead.id);
        let mut claude = agent("Grammar", "claude");
        claude.space = Some(space.id);
        let mut ordinary = agent("Notes", "codex");
        ordinary.configuration.flowx_tools = Some(true);
        let mut elsewhere = agent("Docs pass", "codex");
        elsewhere.space = Some(other_space.id);
        let other_project = agent("Elsewhere", "codex");
        let paths = AppPaths::at(dir.path().join("state"));
        paths.ensure().unwrap();
        fx_store::Store::new(paths.clone())
            .save_app(&PersistedApp {
                active_project_id: Some(project.id),
                active_agent_id: Some(lead.id),
                projects: vec![
                    PersistedProject {
                        project: Some(project.clone()),
                        agents: vec![
                            lead.clone(),
                            codex.clone(),
                            claude.clone(),
                            ordinary.clone(),
                            elsewhere.clone(),
                        ],
                        spaces: vec![
                            PersistedSpace::new(space.clone()),
                            PersistedSpace::new(other_space),
                        ],
                        is_expanded: true,
                        ..Default::default()
                    },
                    PersistedProject {
                        project: Some(Project::new(dir.path().join("other").to_str().unwrap())),
                        agents: vec![other_project.clone()],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })
            .unwrap();
        let test = gpui::TestAppContext::single();
        let state = test.update(|cx| cx.new(|cx| AppState::restore(paths, cx)));
        // A lead's calls arrive during a turn the reader started.
        test.update(|cx| {
            state.update(cx, |state, _| {
                state.register_runner(lead.id, TurnRunner::detached().0)
            })
        });
        Fixture {
            dir,
            test,
            state,
            project,
            space: space.id,
            lead,
            codex,
            claude,
            ordinary,
            elsewhere,
            other_project,
        }
    }

    fn ask(fixture: &Fixture, asking: Uuid, call: ToolCall) -> Answer {
        fixture.test.update(|cx| {
            respond(
                &fixture.state,
                Work::Call(Call {
                    thread: asking,
                    call,
                }),
                cx,
            )
        })
    }

    #[track_caller]
    fn refused(answer: Answer) -> String {
        match answer {
            Answer::Ready(reply) => {
                assert!(!reply.ok, "answered: {}", reply.text);
                reply.text
            }
            _ => panic!("not refused at once"),
        }
    }

    #[track_caller]
    fn plan(answer: Answer) -> Box<StartPlan> {
        match answer {
            Answer::Start(plan) => plan,
            Answer::Ready(reply) => panic!("not started: {}", reply.text),
            _ => panic!("not a start"),
        }
    }

    #[track_caller]
    fn wait_plan(answer: Answer) -> WaitPlan {
        match answer {
            Answer::Wait(plan) => plan,
            Answer::Ready(reply) => panic!("not waited: {}", reply.text),
            _ => panic!("not a wait"),
        }
    }

    fn start(title: &str) -> ToolCall {
        ToolCall::StartThread {
            title: title.into(),
            prompt: "Rewrite the lexer".into(),
            provider: None,
            worktree: false,
        }
    }

    fn send(thread: Uuid) -> ToolCall {
        ToolCall::SendToThread {
            thread,
            prompt: "And the tests".into(),
        }
    }

    fn wait(threads: &[Uuid], until: Until, timeout_seconds: u64) -> ToolCall {
        ToolCall::WaitForThreads {
            threads: threads.to_vec(),
            until,
            timeout_seconds,
        }
    }

    fn stop(thread: Uuid) -> ToolCall {
        ToolCall::StopThread { thread }
    }

    /// A turn under way on `thread`, with no provider behind it.
    fn busy(fixture: &Fixture, thread: Uuid) {
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                state.register_runner(thread, TurnRunner::detached().0)
            })
        });
    }

    /// `thread`'s turn as it ends: its prompt, its reply, and its outcome.
    fn finish(fixture: &Fixture, thread: Uuid, reply: &str, outcome: AgentExecutionState) {
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                state.runners.remove(&thread);
                let conversation = state.conversation(fixture.project.id, thread);
                {
                    let mut conversation = conversation.lock().unwrap();
                    conversation.append(ConversationMessage::text(MessageRole::User, "Do it"));
                    if !reply.is_empty() {
                        conversation
                            .append(ConversationMessage::text(MessageRole::Assistant, reply));
                    }
                    conversation.set_phase(Phase::Idle);
                }
                state.record_execution_state(thread, outcome);
            })
        });
    }

    /// Run a wait as the bridge's thread does: `between` runs before each
    /// poll, with the number of polls before it.
    fn run_wait(fixture: &Fixture, plan: WaitPlan, mut between: impl FnMut(usize)) -> Value {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut polls = 0;
        let reply = runtime.block_on(plan.run(|poll| {
            between(polls);
            polls += 1;
            std::future::ready(
                fixture
                    .test
                    .update(|cx| respond(&fixture.state, Work::Poll(poll), cx)),
            )
        }));
        assert!(reply.ok, "{}", reply.text);
        serde_json::from_str(&reply.text).unwrap()
    }

    fn limits(used: f32, resets_in_hours: i64, status: RateLimitStatus) -> RateLimits {
        RateLimits {
            status,
            windows: vec![RateLimitWindow {
                kind: RateLimitKind::SevenDay,
                used,
                resets_at: Some(fx_core::models::now() + chrono::Duration::hours(resets_in_hours)),
            }],
            plan: None,
        }
    }

    fn set_limits(fixture: &Fixture, provider: &str, limits: RateLimits) {
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                state.rate_limits.insert(provider.into(), limits);
            })
        });
    }

    fn thread_count(fixture: &Fixture) -> usize {
        fixture
            .test
            .read(|cx| fixture.state.read(cx).snapshot.projects[0].agents.len())
    }

    #[test]
    fn only_a_lead_may_orchestrate_and_only_while_its_space_exists() {
        // The server lists these tools only for a lead, but that is a hint in
        // its environment: the app is the one that knows.
        let fixture = seeded();
        let target = fixture.codex.id;
        let calls = [
            start("t"),
            send(target),
            wait(&[target], Until::All, 5),
            stop(target),
        ];
        for call in calls.clone() {
            let name = call.name();
            let why = refused(ask(&fixture, fixture.ordinary.id, call));
            assert!(
                why.starts_with(&format!("{name} is only for a FlowX space's orchestrator")),
                "{why}"
            );
            let why = refused(ask(&fixture, Uuid::new_v4(), start("t")));
            assert!(why.contains("no longer has"), "{why}");
        }
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                let space = fixture.space;
                state.snapshot.projects[0]
                    .spaces
                    .retain(|kept| kept.space.id != space);
            })
        });
        for call in calls {
            let why = refused(ask(&fixture, fixture.lead.id, call));
            assert!(why.contains("has been deleted"), "{why}");
        }
    }

    #[test]
    fn a_lead_reaches_only_threads_of_its_own_space_and_project() {
        let fixture = seeded();
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                // One a provider deleted is gone, though it is still listed.
                let gone = fixture.claude.id;
                state.snapshot.projects[0]
                    .missing_native_agent_ids
                    .push(gone);
            })
        });
        for outsider in [
            fixture.lead.id,
            fixture.ordinary.id,
            fixture.elsewhere.id,
            fixture.other_project.id,
            fixture.claude.id,
            Uuid::new_v4(),
        ] {
            for call in [send(outsider), stop(outsider)] {
                let why = refused(ask(&fixture, fixture.lead.id, call));
                assert!(
                    why.contains("in your space") || why.contains("your own thread"),
                    "{outsider}: {why}"
                );
            }
        }
        // One outsider spoils a wait, and is named.
        let why = refused(ask(
            &fixture,
            fixture.lead.id,
            wait(&[fixture.codex.id, fixture.elsewhere.id], Until::Any, 5),
        ));
        assert!(why.contains(&fixture.elsewhere.id.to_string()), "{why}");
        assert!(!why.contains(&fixture.codex.id.to_string()), "{why}");
        wait_plan(ask(
            &fixture,
            fixture.lead.id,
            wait(&[fixture.codex.id], Until::Any, 5),
        ));
    }

    #[test]
    fn no_new_work_starts_past_the_usage_warning_line() {
        // What is left of the account's usage is the user's, not the lead's.
        let fixture = seeded();
        set_limits(
            &fixture,
            "claude",
            limits(0.85, 72, RateLimitStatus::Warning),
        );
        let why = refused(ask(&fixture, fixture.lead.id, start("t")));
        assert!(
            why.starts_with("Claude's weekly limit is 85% used (resets in"),
            "{why}"
        );
        assert!(
            why.contains("they can raise it by resetting their usage"),
            "{why}"
        );
        let why = refused(ask(&fixture, fixture.lead.id, send(fixture.claude.id)));
        assert!(why.contains("85% used"), "{why}");
        // Another provider's account is its own.
        plan(ask(
            &fixture,
            fixture.lead.id,
            ToolCall::StartThread {
                title: "t".into(),
                prompt: "p".into(),
                provider: Some("codex".into()),
                worktree: false,
            },
        ));
        // A window that has reset since it was reported no longer counts.
        set_limits(
            &fixture,
            "claude",
            limits(0.97, -1, RateLimitStatus::Warning),
        );
        plan(ask(&fixture, fixture.lead.id, start("t")));
        set_limits(
            &fixture,
            "claude",
            limits(0.5, 72, RateLimitStatus::Allowed),
        );
        plan(ask(&fixture, fixture.lead.id, start("t")));
        // Refused for the limit: nothing more, whatever the fraction says.
        set_limits(
            &fixture,
            "claude",
            limits(0.3, 72, RateLimitStatus::Rejected),
        );
        let why = refused(ask(&fixture, fixture.lead.id, start("t")));
        assert!(why.starts_with("Claude refused its last request"), "{why}");
        // Until the window that refused resets: held after it, every start
        // on that provider was refused until its next turn.
        set_limits(
            &fixture,
            "claude",
            limits(1.0, -1, RateLimitStatus::Rejected),
        );
        plan(ask(&fixture, fixture.lead.id, start("t")));
    }

    #[test]
    fn a_lead_acts_only_as_its_space_s_one_lead_and_only_while_its_turn_runs() {
        let fixture = seeded();
        // A second thread naming the space is drawn as an ordinary row, and
        // is no lead: as one it could send to or stop the lead the reader
        // sees.
        let second = fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                let mut second = agent("Old orchestrator", "claude");
                second.lead_of = Some(fixture.space);
                second.space = Some(fixture.space);
                let id = second.id;
                state.snapshot.projects[0].agents.push(second);
                state.register_runner(id, TurnRunner::detached().0);
                id
            })
        });
        for call in [send(fixture.codex.id), stop(fixture.lead.id), start("t")] {
            let why = refused(ask(&fixture, second, call));
            assert!(why.contains("another thread now leads this space"), "{why}");
        }
        // Stopped while a start was being made: checked again as the thread
        // is made, it makes none.
        let raced = plan(ask(&fixture, fixture.lead.id, start("Late")));
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                state.runners.remove(&fixture.lead.id);
            })
        });
        let before = thread_count(&fixture);
        let reply = fixture.test.update(|cx| {
            launch(
                &fixture.state,
                Launch {
                    plan: *raced,
                    workspace: None,
                },
                cx,
            )
        });
        assert!(!reply.ok);
        assert!(reply.text.contains("has ended"), "{}", reply.text);
        assert_eq!(thread_count(&fixture), before);
        let why = refused(ask(&fixture, fixture.lead.id, send(fixture.codex.id)));
        assert!(why.contains("has ended"), "{why}");
    }

    #[test]
    fn another_space_s_lead_is_never_a_worker_wherever_it_is_filed() {
        let fixture = seeded();
        let other_lead = fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                let mut other = agent("Docs orchestrator", "codex");
                other.lead_of = fixture.elsewhere.space;
                other.space = Some(fixture.space);
                let id = other.id;
                let record = &mut state.snapshot.projects[0];
                record.agents.push(other);
                record.project.as_mut().unwrap().agent_order.push(id);
                assert_eq!(state.space_lead(fixture.elsewhere.space.unwrap()), Some(id));
                id
            })
        });
        for call in [send(other_lead), stop(other_lead)] {
            let why = refused(ask(&fixture, fixture.lead.id, call));
            assert!(why.contains("in your space"), "{why}");
        }
    }

    #[test]
    fn a_worker_running_in_another_app_is_running_and_takes_no_prompt() {
        // Read from its saved outcome, it settled a wait at once, and a
        // prompt resumed its session in a second process beside the app's.
        let fixture = seeded();
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                state.running_elsewhere.insert(fixture.claude.id);
            })
        });
        let why = refused(ask(&fixture, fixture.lead.id, send(fixture.claude.id)));
        assert_eq!(
            why,
            "\"Grammar\" is still running: call wait_for_threads first."
        );
        let status = fixture.test.read(|cx| {
            let state = fixture.state.read(cx);
            state
                .worker_status(state.thread(fixture.claude.id).unwrap())
                .0
        });
        assert_eq!(status, WorkerStatus::Running);
    }

    #[test]
    fn a_prompt_to_a_worker_nothing_has_loaded_reads_its_transcript_in_the_background() {
        // Read on the UI thread, a long transcript held the window while it
        // was parsed.
        let mut fixture = seeded();
        let (project, worker) = (fixture.project.id, fixture.claude.id);
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                let saved = fx_store::PersistedConversation::from_messages(
                    worker,
                    vec![ConversationMessage::text(MessageRole::User, "Earlier")],
                );
                state
                    .store
                    .save_conversation(project, worker, &saved)
                    .unwrap();
                assert!(!state.conversations.contains_key(&worker));
            })
        });
        let why = refused(ask(&fixture, fixture.lead.id, send(worker)));
        assert!(why.contains("Call send_to_thread again"), "{why}");
        fixture.test.read(|cx| {
            let state = fixture.state.read(cx);
            assert!(state.is_loading_conversation(worker));
            assert!(!state.conversations.contains_key(&worker));
        });
        fixture.test.run_until_parked();
        fixture.test.read(|cx| {
            assert!(fixture.state.read(cx).conversations.contains_key(&worker));
        });
    }

    #[test]
    fn a_space_runs_at_most_four_threads_at_once_even_when_starts_race() {
        let fixture = seeded();
        let extra: Vec<Uuid> = fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                (0..3)
                    .map(|n| {
                        let mut worker = agent(&format!("Worker {n}"), "codex");
                        worker.space = Some(fixture.space);
                        let id = worker.id;
                        state.snapshot.projects[0].agents.push(worker);
                        id
                    })
                    .collect()
            })
        });
        // Neither the lead's own turn nor another space's counts.
        busy(&fixture, fixture.lead.id);
        busy(&fixture, fixture.elsewhere.id);
        for thread in [fixture.codex.id, fixture.claude.id, extra[0]] {
            busy(&fixture, thread);
        }
        let raced = plan(ask(&fixture, fixture.lead.id, start("Late")));
        busy(&fixture, extra[1]);
        let why = refused(ask(&fixture, fixture.lead.id, start("t")));
        assert!(
            why.starts_with("4 of your space's threads are already running"),
            "{why}"
        );
        let why = refused(ask(&fixture, fixture.lead.id, send(extra[2])));
        assert!(why.contains("already running"), "{why}");
        // The start that was checked before the fourth began is checked again
        // as its thread is made, and makes none.
        let before = thread_count(&fixture);
        let reply = fixture.test.update(|cx| {
            launch(
                &fixture.state,
                Launch {
                    plan: *raced,
                    workspace: None,
                },
                cx,
            )
        });
        assert!(!reply.ok);
        assert!(reply.text.contains("already running"), "{}", reply.text);
        assert_eq!(thread_count(&fixture), before);
    }

    #[test]
    fn a_running_or_waiting_worker_is_refused_a_prompt() {
        let fixture = seeded();
        busy(&fixture, fixture.codex.id);
        let why = refused(ask(&fixture, fixture.lead.id, send(fixture.codex.id)));
        assert_eq!(
            why,
            "\"Lexer\" is still running: call wait_for_threads first."
        );
        busy(&fixture, fixture.claude.id);
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                state
                    .pending_interactions
                    .entry(fixture.claude.id)
                    .or_default()
                    .push_back(fx_agent::ProviderEvent::Approval(
                        fx_agent::provider::ApprovalRequest {
                            id: "a1".into(),
                            tool_name: "Bash".into(),
                            input: json!({"command": "cargo test"}),
                            risk: fx_agent::provider::RiskLevel::Dangerous,
                        },
                    ));
            })
        });
        let why = refused(ask(&fixture, fixture.lead.id, send(fixture.claude.id)));
        assert!(
            why.starts_with("\"Grammar\" is waiting for the user (approval to run `cargo test`)"),
            "{why}"
        );
        // Codex names its command as argv, under `arguments`.
        let codex = fx_agent::ProviderEvent::Approval(fx_agent::provider::ApprovalRequest {
            id: "a2".into(),
            tool_name: "commandExecution".into(),
            input: json!({"arguments": {"command": ["cargo", "build"]}}),
            risk: fx_agent::provider::RiskLevel::Dangerous,
        });
        assert_eq!(
            waiting_for(&codex).as_deref(),
            Some("approval to run `cargo build`")
        );
    }

    #[test]
    fn a_wait_returns_as_soon_as_its_threads_settle() {
        let fixture = seeded();
        let (codex, claude) = (fixture.codex.id, fixture.claude.id);
        busy(&fixture, codex);
        busy(&fixture, claude);
        let started = Instant::now();
        let all = wait_plan(ask(
            &fixture,
            fixture.lead.id,
            wait(&[codex, claude], Until::All, 60),
        ));
        let answer = run_wait(&fixture, all, |polls| match polls {
            2 => finish(
                &fixture,
                codex,
                "Lexer rewritten.",
                AgentExecutionState::Success,
            ),
            4 => finish(&fixture, claude, "", AgentExecutionState::Failure),
            _ => {}
        });
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it waited out its timeout"
        );
        assert_eq!(answer["settled"], true);
        assert_eq!(answer["threads"][0]["thread_id"], codex.to_string());
        assert_eq!(answer["threads"][0]["status"], "completed");
        assert_eq!(answer["threads"][0]["last_reply"], "Lexer rewritten.");
        assert_eq!(answer["threads"][1]["status"], "failed");
        assert_eq!(answer["threads"][1]["last_reply"], Value::Null);

        // `any` is done at the first.
        busy(&fixture, codex);
        busy(&fixture, claude);
        let any = wait_plan(ask(
            &fixture,
            fixture.lead.id,
            wait(&[codex, claude], Until::Any, 60),
        ));
        let answer = run_wait(&fixture, any, |polls| {
            if polls == 1 {
                finish(
                    &fixture,
                    claude,
                    "Grammar done.",
                    AgentExecutionState::Success,
                );
            }
        });
        assert_eq!(answer["settled"], true);
        assert_eq!(answer["until"], "any");
        assert_eq!(answer["threads"][0]["status"], "running");
        assert_eq!(answer["threads"][1]["status"], "completed");
    }

    #[test]
    fn a_wait_returns_at_its_timeout_with_what_is_still_running() {
        let fixture = seeded();
        busy(&fixture, fixture.codex.id);
        let started = Instant::now();
        let answer = run_wait(
            &fixture,
            wait_plan(ask(
                &fixture,
                fixture.lead.id,
                wait(&[fixture.codex.id], Until::All, 1),
            )),
            |_| {},
        );
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_secs(1) && waited < Duration::from_secs(5),
            "{waited:?}"
        );
        assert_eq!(answer["settled"], false);
        assert_eq!(answer["threads"][0]["status"], "running");
        assert!(
            answer["note"]
                .as_str()
                .unwrap()
                .contains("Call wait_for_threads again")
        );
    }

    #[test]
    fn a_worker_blocked_on_the_user_settles_a_wait_as_waiting() {
        // Only the user can move it on, so the lead must hear of it rather
        // than wait out the timeout.
        let fixture = seeded();
        busy(&fixture, fixture.claude.id);
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                state
                    .pending_interactions
                    .entry(fixture.claude.id)
                    .or_default()
                    .push_back(fx_agent::ProviderEvent::UserInput(
                        fx_agent::provider::UserInputRequest {
                            id: "q1".into(),
                            questions: vec![fx_agent::provider::Question {
                                id: "db".into(),
                                header: "Database".into(),
                                question: "Which database?".into(),
                                ..Default::default()
                            }],
                            source: None,
                            message: None,
                            link: None,
                        },
                    ));
            })
        });
        let started = Instant::now();
        let answer = run_wait(
            &fixture,
            wait_plan(ask(
                &fixture,
                fixture.lead.id,
                wait(&[fixture.claude.id], Until::All, 60),
            )),
            |_| {},
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(answer["settled"], true);
        let thread = &answer["threads"][0];
        assert_eq!(thread["status"], "waiting");
        assert_eq!(thread["waiting_for"], "an answer to \"Which database?\"");
        assert!(answer["note"].as_str().unwrap().contains("Grammar"));
    }

    #[test]
    fn stop_thread_stops_the_turn_as_the_reader_s_stop_does_and_reports_it_settled() {
        let fixture = seeded();
        let (runner, mut stopped) = TurnRunner::detached_stop();
        fixture.test.update(|cx| {
            fixture.state.update(cx, |state, _| {
                state.register_runner(fixture.codex.id, runner)
            })
        });
        let plan = wait_plan(ask(&fixture, fixture.lead.id, stop(fixture.codex.id)));
        assert!(stopped.try_recv().is_ok(), "the runner was not stopped");
        // The cancelled turn settles as idle with its prompt in the
        // transcript: stopped, not idle.
        let answer = run_wait(&fixture, plan, |polls| {
            if polls == 1 {
                finish(&fixture, fixture.codex.id, "", AgentExecutionState::Idle);
            }
        });
        assert_eq!(answer["status"], "stopped");
        assert_eq!(answer["was_running"], true);
        assert!(answer.get("note").is_none(), "{answer}");
        // Nothing running: said so, at once.
        let answer = run_wait(
            &fixture,
            wait_plan(ask(&fixture, fixture.lead.id, stop(fixture.claude.id))),
            |_| {},
        );
        assert_eq!(answer["status"], "idle");
        assert_eq!(answer["was_running"], false);
        assert!(
            answer["note"]
                .as_str()
                .unwrap()
                .contains("nothing was stopped")
        );
    }

    #[test]
    fn a_lead_s_listing_says_which_threads_it_can_reach_and_which_it_started() {
        let fixture = seeded();
        let listing = |asking: Uuid| -> Value {
            match ask(&fixture, asking, ToolCall::ListThreads) {
                Answer::Ready(reply) => {
                    assert!(reply.ok, "{}", reply.text);
                    serde_json::from_str(&reply.text).unwrap()
                }
                _ => panic!("a listing is answered at once"),
            }
        };
        // A lead needs no opt-in: leading a space is what the tools are for.
        assert_eq!(fixture.lead.configuration.flowx_tools, None);
        let listed = listing(fixture.lead.id);
        let row = |id: Uuid| {
            listed["threads"]
                .as_array()
                .unwrap()
                .iter()
                .find(|thread| thread["id"] == id.to_string())
                .cloned()
                .unwrap()
        };
        assert_eq!(row(fixture.codex.id)["in_space"], true);
        assert_eq!(row(fixture.codex.id)["spawned_by_you"], true);
        assert_eq!(row(fixture.claude.id)["in_space"], true);
        assert_eq!(row(fixture.claude.id)["spawned_by_you"], false);
        assert_eq!(row(fixture.elsewhere.id)["in_space"], false);
        assert_eq!(row(fixture.lead.id)["in_space"], false);
        assert_eq!(row(fixture.lead.id)["current"], true);
        // Anyone else is told neither.
        let ordinary = listing(fixture.ordinary.id);
        for thread in ordinary["threads"].as_array().unwrap() {
            assert!(thread.get("in_space").is_none() && thread.get("spawned_by_you").is_none());
        }
    }

    fn python() -> Option<String> {
        let output = std::process::Command::new("/usr/bin/env")
            .args(["which", "python3"])
            .output()
            .ok()?;
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then_some(path)
    }

    /// A fake `claude` that answers each prompt with "Done: <prompt>" and
    /// records what it saw, beside itself, in `received.jsonl`.
    fn fake_claude(dir: &std::path::Path) -> Option<(RuntimeDiscovery, BinarySpec)> {
        let python = python()?;
        let script = dir.join("claude.py");
        std::fs::write(
            &script,
            r#"
import json, os, sys
from pathlib import Path
LOG = Path(__file__).with_name("received.jsonl")
SESSION = "22222222-3333-4444-8555-666666666666"
def send(value): print(json.dumps(value), flush=True)
def record(value):
    with open(LOG, "a") as log: log.write(json.dumps(value) + "\n")
if sys.argv[1:] == ["agents", "--json"]:
    print("[]")
    sys.exit(0)
record({"args": sys.argv[1:], "cwd": os.getcwd()})
for line in sys.stdin:
    message = json.loads(line)
    record(message)
    if message.get("type") == "control_request" and (message.get("request") or {}).get("subtype") == "get_context_usage":
        send({"type":"control_response","response":{"subtype":"success","request_id":message.get("request_id"),
              "response":{"categories":[],"totalTokens":10,"maxTokens":200000}}})
        continue
    if message.get("type") == "user":
        prompt = message["message"]["content"][0]["text"]
        send({"type":"system","subtype":"init","session_id":SESSION})
        send({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Done: " + prompt}}})
        send({"type":"result","subtype":"end_turn","session_id":SESSION,"usage":{"input_tokens":5,"output_tokens":2}})
"#,
        )
        .ok()?;
        Some((
            RuntimeDiscovery::default().without_extra_directories(),
            BinarySpec::new("claude", "Claude Code", vec![python])
                .with_arguments(vec![script.display().to_string()]),
        ))
    }

    fn install(fixture: &Fixture, provider: &str, runtime: (RuntimeDiscovery, BinarySpec)) {
        fixture.test.update(|cx| {
            let mut fakes = cx
                .try_global::<TestRuntimes>()
                .map(|fakes| fakes.0.clone())
                .unwrap_or_default();
            fakes.insert(provider.into(), runtime);
            cx.set_global(TestRuntimes(fakes));
        });
    }

    fn prepare(plan: Box<StartPlan>) -> Result<Launch, Reply> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(plan.prepare())
    }

    #[test]
    fn a_missing_provider_and_a_project_without_git_are_refused_before_anything_is_made() {
        let fixture = seeded();
        let Some(claude) = fake_claude(fixture.dir.path()) else {
            return;
        };
        let missing = BinarySpec::new(
            "claude",
            "Claude Code",
            vec![fixture.dir.path().join("no-claude").display().to_string()],
        )
        .install_hint("Install it with npm.");
        install(
            &fixture,
            "claude",
            (
                RuntimeDiscovery::default().without_extra_directories(),
                missing,
            ),
        );
        let before = thread_count(&fixture);
        let Err(reply) = prepare(plan(ask(&fixture, fixture.lead.id, start("t")))) else {
            panic!("a start went ahead without its provider");
        };
        assert!(
            reply.text.starts_with(
                "Claude Code is not installed, so FlowX cannot start a claude thread."
            ),
            "{}",
            reply.text
        );
        assert!(
            reply.text.ends_with("Install it with npm."),
            "{}",
            reply.text
        );

        install(&fixture, "claude", claude);
        let worktree = ToolCall::StartThread {
            title: "t".into(),
            prompt: "p".into(),
            provider: None,
            worktree: true,
        };
        let Err(reply) = prepare(plan(ask(&fixture, fixture.lead.id, worktree))) else {
            panic!("a worktree was made outside a repository");
        };
        assert!(
            reply.text.contains("is not a Git repository"),
            "{}",
            reply.text
        );
        assert_eq!(thread_count(&fixture), before);
        assert!(!fixture.dir.path().join("state/worktrees").exists());
    }

    fn git(root: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new(fx_core::git::GIT)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(root)
            .envs(fx_core::git::hardened_environment())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Call a tool as the lead's server does, through the socket, keeping the
    /// app's executor and the turns' updates moving until it is answered.
    fn through_socket(
        fixture: &mut Fixture,
        access: &crate::bridge::BridgeAccess,
        tool: &str,
        arguments: Value,
    ) -> Value {
        let server = access.server_for(&fixture.lead).unwrap();
        let request = fx_bridge::wire::Request {
            thread: server.env[fx_bridge::ENV_THREAD].clone(),
            token: server.env[fx_bridge::ENV_TOKEN].clone(),
            tool: tool.into(),
            arguments,
        };
        let socket = PathBuf::from(&server.env[fx_bridge::ENV_SOCKET]);
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
        let deadline = Instant::now() + Duration::from_secs(60);
        while !caller.is_finished() {
            assert!(Instant::now() < deadline, "{tool} was never answered");
            fixture.test.run_until_parked();
            let state = fixture.state.clone();
            fixture.test.update(|cx| {
                state.update(cx, |state, cx| {
                    state.drain_updates_with_context(cx);
                })
            });
            std::thread::sleep(Duration::from_millis(5));
        }
        let reply = caller.join().unwrap();
        assert!(reply.ok, "{tool}: {}", reply.text);
        serde_json::from_str(&reply.text).unwrap()
    }

    #[test]
    fn start_thread_runs_a_worker_in_its_own_worktree_and_wait_and_send_follow_it() {
        let mut fixture = seeded();
        let Some(claude) = fake_claude(fixture.dir.path()) else {
            return;
        };
        install(&fixture, "claude", claude);
        let root = PathBuf::from(&fixture.project.root_path);
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "FlowX test"]);
        git(&root, &["config", "user.email", "qa@example.invalid"]);
        std::fs::write(root.join("lexer.rs"), "fn lex() {}").unwrap();
        git(&root, &["add", "lexer.rs"]);
        git(&root, &["commit", "-qm", "Initial"]);
        let parent = tempfile::tempdir().unwrap();
        let state = fixture.state.clone();
        let access = fixture.test.update(|cx| {
            crate::bridge::start_in(state, parent.path(), PathBuf::from("/flowx"), cx).unwrap()
        });

        let started = through_socket(
            &mut fixture,
            &access,
            "start_thread",
            json!({"title": "Lexer rewrite", "prompt": "Rewrite the lexer", "worktree": true}),
        );
        assert_eq!(started["status"], "running");
        assert_eq!(started["title"], "Lexer rewrite");
        // The lead's provider, since none was named.
        assert_eq!(started["provider"], "claude");
        let id: Uuid = started["thread_id"].as_str().unwrap().parse().unwrap();
        let path = started["worktree"]["path"].as_str().unwrap().to_owned();
        assert!(
            started["worktree"]["branch"]
                .as_str()
                .unwrap()
                .starts_with("flowx/")
        );
        fixture.test.read(|cx| {
            let state = fixture.state.read(cx);
            let worker = state.thread(id).unwrap();
            assert_eq!(worker.title, "Lexer rewrite");
            assert_eq!(worker.space, Some(fixture.space));
            assert_eq!(worker.spawned_by, Some(fixture.lead.id));
            assert_eq!(worker.lead_of, None);
            assert_eq!(worker.workspace.as_ref().unwrap().path, path);
            assert_eq!(state.project_root_for(id).as_deref(), Some(path.as_str()));
            assert_eq!(worker.configuration.provider_id.as_deref(), Some("claude"));
            // The lead's access, and none of FlowX's tools.
            assert_eq!(
                worker.configuration.agent_access,
                Some(AgentAccess::Supervised)
            );
            assert_eq!(worker.configuration.flowx_tools, None);
            // Named as asked, and kept so: not queued for an automatic name.
            assert!(!state.snapshot.projects[0].auto_title_pending.contains(&id));
            // The reader is still watching the lead.
            assert_eq!(state.snapshot.active_agent_id, Some(fixture.lead.id));
        });

        let waited = through_socket(
            &mut fixture,
            &access,
            "wait_for_threads",
            json!({"thread_ids": [id.to_string()], "timeout_seconds": 60}),
        );
        assert_eq!(waited["settled"], true, "{waited}");
        assert_eq!(waited["threads"][0]["status"], "completed");
        assert_eq!(
            waited["threads"][0]["last_reply"],
            "Done: Rewrite the lexer"
        );
        // The fake ran in the worktree and was given the prompt.
        let received = std::fs::read_to_string(fixture.dir.path().join("received.jsonl")).unwrap();
        let first: Value = serde_json::from_str(received.lines().next().unwrap()).unwrap();
        assert_eq!(first["cwd"], path);
        assert!(received.contains("Rewrite the lexer"));

        let sent = through_socket(
            &mut fixture,
            &access,
            "send_to_thread",
            json!({"thread_id": id.to_string(), "prompt": "And the tests"}),
        );
        assert_eq!(
            sent,
            json!({"thread_id": id.to_string(), "status": "running"})
        );
        let waited = through_socket(
            &mut fixture,
            &access,
            "wait_for_threads",
            json!({"thread_ids": [id.to_string()]}),
        );
        assert_eq!(waited["threads"][0]["last_reply"], "Done: And the tests");
        // The row the reader sees is the one the composer would have made.
        fixture.test.read(|cx| {
            let conversation = fixture.state.read(cx).conversation_by_agent(id).unwrap();
            let conversation = conversation.lock().unwrap();
            let prompts: Vec<String> = conversation
                .messages()
                .filter(|message| message.role == MessageRole::User)
                .map(|message| message.text_content())
                .collect();
            assert_eq!(prompts, ["Rewrite the lexer", "And the tests"]);
        });
        access.shutdown();
    }
}
