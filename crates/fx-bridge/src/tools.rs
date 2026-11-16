//! The tools, as a model is told about them, and the calls they accept.
//!
//! Two kinds. The reads (`list_threads`, `read_thread`) are offered to every
//! thread that has the server. A space's orchestrator is offered four more,
//! which start, steer, wait for and stop the space's other threads; the
//! server lists those only when its environment says it speaks for a lead
//! ([`Role`]). That hint is not proof: the app checks on every call that the
//! asking thread leads a space.
//!
//! Each tool says what it does twice: in its description, which the model
//! reads, and in its annotations, which a client may use to decide whether to
//! ask. FlowX's own approval policies do not rely on the annotations; they
//! know the names (`fx_core::policy::flowx_tools`).

use std::time::Duration;

use fx_core::policy::flowx_tools::{
    LIST_THREADS, READ_THREAD, SEND_TO_THREAD, START_THREAD, STOP_THREAD, WAIT_FOR_THREADS,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// How many messages `read_thread` returns when the model does not say.
pub const DEFAULT_COUNT: usize = 10;
/// The most it returns, whatever the model asks for.
pub const MAX_COUNT: usize = 50;

/// The longest title `start_thread` takes, in characters.
pub const MAX_TITLE_CHARS: usize = 80;
/// The longest prompt `start_thread` and `send_to_thread` take, in
/// characters: the composer's own cap, so a lead can send nothing a reader
/// could not have typed.
pub const MAX_PROMPT_CHARS: usize = 32_000;
/// How long `wait_for_threads` waits when the model does not say, and the
/// most it waits whatever it asks for.
pub const MAX_WAIT_SECONDS: u64 = 240;
/// The most threads one wait watches.
pub const MAX_WAITED_THREADS: usize = 16;
/// How many of a space's threads may run at once before `start_thread` and
/// `send_to_thread` refuse to start another. Enforced by the app; here so
/// the tools' descriptions say the same number.
pub const MAX_RUNNING_WORKERS: usize = 4;

/// How long the app has to answer an ordinary call.
pub const DEFAULT_ANSWER_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a `start_thread` may take: making a worktree runs `git worktree
/// add`, which FlowX gives two minutes, and finding the provider may ask a
/// login shell.
pub const START_ANSWER_TIMEOUT: Duration = Duration::from_secs(150);
/// How long `stop_thread` waits for the stopped turn to settle before it
/// answers with what it sees.
pub const STOP_SETTLE: Duration = Duration::from_secs(10);
/// What a wait's answer may take beyond the wait itself: the last poll and
/// reading saved transcripts.
pub const WAIT_MARGIN: Duration = Duration::from_secs(15);

/// The per-call timeout a lead's provider is given for FlowX's server, above
/// the longest any call here takes ([`ToolCall::answer_timeout`] plus the
/// server's own margin).
///
/// Both CLIs bound an MCP tool call, and a wait holds one open for up to
/// [`MAX_WAIT_SECONDS`]:
///
/// - **codex-cli 0.167** reads `tool_timeout_sec` (seconds, a float) from
///   the server's `mcp_servers.<name>` entry and otherwise uses
///   `DEFAULT_TOOL_TIMEOUT`, 300 s (`codex-rs/codex-mcp/src/rmcp_client.rs`
///   at `rust-v0.167.0`; the key is in the binary's strings beside
///   `startup_timeout_sec`). Older releases defaulted to 60 s.
/// - **Claude Code 2.1.325** reads a per-server `timeout` (milliseconds) from
///   the stdio entry, which "overrides the MCP_TOOL_TIMEOUT environment
///   variable for this server", then `MCP_TOOL_TIMEOUT`, then 1e8 ms. A
///   second, idle bound (no response or progress) is
///   `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT`, 30 minutes for a stdio server,
///   raised to the per-server timeout and capped by it. Both read from the
///   binary.
///
/// Neither default is below the wait, but a user's own `MCP_TOOL_TIMEOUT`
/// may be, and Codex's was until recently. So a lead's entry states this
/// timeout for FlowX's server alone, and every other server keeps the user's.
pub const LEAD_TOOL_TIMEOUT: Duration = Duration::from_secs(600);

/// Who the server speaks for, as its environment says: which tools it lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// Any thread that has FlowX's tools: the reads.
    #[default]
    Thread,
    /// A space's orchestrator: the reads and the orchestration tools.
    Lead,
}

impl Role {
    /// The role an environment variable's value names. Anything but
    /// [`crate::ROLE_LEAD`] is an ordinary thread.
    pub fn from_value(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(crate::ROLE_LEAD) => Self::Lead,
            _ => Self::Thread,
        }
    }
}

/// Which of the threads a wait waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Until {
    /// Every thread has settled.
    #[default]
    All,
    /// At least one has.
    Any,
}

impl Until {
    pub fn word(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Any => "any",
        }
    }
}

/// One call the app is asked to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCall {
    ListThreads,
    ReadThread {
        /// A FlowX thread id, as `list_threads` gives it.
        thread: Uuid,
        count: usize,
    },
    StartThread {
        title: String,
        prompt: String,
        /// `claude` or `codex`; the lead's own when `None`.
        provider: Option<String>,
        /// Whether the thread gets a checkout of its own.
        worktree: bool,
    },
    SendToThread {
        thread: Uuid,
        prompt: String,
    },
    WaitForThreads {
        /// In the order asked, each once.
        threads: Vec<Uuid>,
        until: Until,
        /// Whole seconds, from 1 to [`MAX_WAIT_SECONDS`].
        timeout_seconds: u64,
    },
    StopThread {
        thread: Uuid,
    },
}

/// Why a call was not made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// No tool has that name: a protocol error, by MCP's rules.
    UnknownTool(String),
    /// The tool exists and its arguments are wrong: the tool's own error,
    /// which the model reads and can correct.
    InvalidArguments(String),
}

impl ToolCall {
    /// Read a call from a tool's name and its arguments.
    pub fn parse(name: &str, arguments: &Value) -> Result<Self, CallError> {
        let arguments = match arguments {
            Value::Null => &Value::Object(Default::default()),
            Value::Object(_) => arguments,
            _ => {
                return Err(invalid("The arguments must be an object."));
            }
        };
        match name {
            LIST_THREADS => Ok(Self::ListThreads),
            READ_THREAD => {
                let thread = thread_id(arguments, "thread_id", "list_threads")?;
                let count = match arguments.get("count") {
                    None | Some(Value::Null) => DEFAULT_COUNT,
                    Some(count) => match count.as_u64() {
                        Some(0) | None => {
                            return Err(invalid(format!(
                                "`count` must be a whole number from 1 to {MAX_COUNT}."
                            )));
                        }
                        Some(count) => usize::try_from(count).unwrap_or(MAX_COUNT).min(MAX_COUNT),
                    },
                };
                Ok(Self::ReadThread { thread, count })
            }
            START_THREAD => {
                // One line: a title is a sidebar row.
                let title = text(arguments, "title", "a short name for the thread")?
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let length = title.chars().count();
                if length > MAX_TITLE_CHARS {
                    return Err(invalid(format!(
                        "`title` is {length} characters; it may be at most {MAX_TITLE_CHARS}. \
                         Shorten it."
                    )));
                }
                let prompt = prompt(arguments)?;
                let provider = match arguments.get("provider") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(provider))
                        if matches!(provider.trim(), "claude" | "codex") =>
                    {
                        Some(provider.trim().to_owned())
                    }
                    Some(other) => {
                        return Err(invalid(format!(
                            "`provider` must be \"claude\" or \"codex\", not {other}. Leave it \
                             out to use your own."
                        )));
                    }
                };
                let worktree = match arguments.get("worktree") {
                    None | Some(Value::Null) => false,
                    Some(Value::Bool(worktree)) => *worktree,
                    Some(_) => return Err(invalid("`worktree` must be true or false.")),
                };
                Ok(Self::StartThread {
                    title,
                    prompt,
                    provider,
                    worktree,
                })
            }
            SEND_TO_THREAD => Ok(Self::SendToThread {
                thread: thread_id(arguments, "thread_id", "start_thread or list_threads")?,
                prompt: prompt(arguments)?,
            }),
            WAIT_FOR_THREADS => {
                let ids = arguments
                    .get("thread_ids")
                    .and_then(Value::as_array)
                    .filter(|ids| !ids.is_empty())
                    .ok_or_else(|| {
                        invalid(
                            "`thread_ids` is required: a list of the ids start_thread \
                             returned, at least one.",
                        )
                    })?;
                let mut threads = Vec::new();
                for id in ids {
                    let id = id
                        .as_str()
                        .and_then(|id| Uuid::parse_str(id.trim()).ok())
                        .ok_or_else(|| {
                            invalid(format!(
                                "{id} in `thread_ids` is not a FlowX thread id. Pass each \
                                 `thread_id` exactly as start_thread returned it."
                            ))
                        })?;
                    if !threads.contains(&id) {
                        threads.push(id);
                    }
                }
                if threads.len() > MAX_WAITED_THREADS {
                    return Err(invalid(format!(
                        "One wait watches at most {MAX_WAITED_THREADS} threads."
                    )));
                }
                let until = match arguments.get("until") {
                    None | Some(Value::Null) => Until::All,
                    Some(Value::String(until)) if until.trim() == "all" => Until::All,
                    Some(Value::String(until)) if until.trim() == "any" => Until::Any,
                    Some(_) => return Err(invalid("`until` must be \"all\" or \"any\".")),
                };
                let timeout_seconds = match arguments.get("timeout_seconds") {
                    None | Some(Value::Null) => MAX_WAIT_SECONDS,
                    Some(seconds) => match seconds.as_u64() {
                        Some(0) | None => {
                            return Err(invalid(format!(
                                "`timeout_seconds` must be a whole number from 1 to \
                                 {MAX_WAIT_SECONDS}."
                            )));
                        }
                        Some(seconds) => seconds.min(MAX_WAIT_SECONDS),
                    },
                };
                Ok(Self::WaitForThreads {
                    threads,
                    until,
                    timeout_seconds,
                })
            }
            STOP_THREAD => Ok(Self::StopThread {
                thread: thread_id(arguments, "thread_id", "start_thread or list_threads")?,
            }),
            other => Err(CallError::UnknownTool(other.to_owned())),
        }
    }

    /// The tool's name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ListThreads => LIST_THREADS,
            Self::ReadThread { .. } => READ_THREAD,
            Self::StartThread { .. } => START_THREAD,
            Self::SendToThread { .. } => SEND_TO_THREAD,
            Self::WaitForThreads { .. } => WAIT_FOR_THREADS,
            Self::StopThread { .. } => STOP_THREAD,
        }
    }

    /// Whether only a space's orchestrator may make this call.
    pub fn is_orchestration(&self) -> bool {
        fx_core::policy::flowx_tools::ORCHESTRATION.contains(&self.name())
    }

    /// How long the app may take to answer it. A wait takes as long as it
    /// asked for; a start may make a worktree.
    pub fn answer_timeout(&self) -> Duration {
        match self {
            Self::WaitForThreads {
                timeout_seconds, ..
            } => Duration::from_secs(*timeout_seconds) + WAIT_MARGIN,
            Self::StartThread { .. } => START_ANSWER_TIMEOUT,
            Self::StopThread { .. } => STOP_SETTLE + WAIT_MARGIN,
            Self::ListThreads | Self::ReadThread { .. } | Self::SendToThread { .. } => {
                DEFAULT_ANSWER_TIMEOUT
            }
        }
    }

    /// The arguments, as they cross the socket.
    pub fn arguments(&self) -> Value {
        match self {
            Self::ListThreads => json!({}),
            Self::ReadThread { thread, count } => {
                json!({ "thread_id": thread.to_string(), "count": count })
            }
            Self::StartThread {
                title,
                prompt,
                provider,
                worktree,
            } => json!({
                "title": title,
                "prompt": prompt,
                "provider": provider,
                "worktree": worktree,
            }),
            Self::SendToThread { thread, prompt } => {
                json!({ "thread_id": thread.to_string(), "prompt": prompt })
            }
            Self::WaitForThreads {
                threads,
                until,
                timeout_seconds,
            } => json!({
                "thread_ids": threads.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                "until": until.word(),
                "timeout_seconds": timeout_seconds,
            }),
            Self::StopThread { thread } => json!({ "thread_id": thread.to_string() }),
        }
    }
}

fn invalid(why: impl Into<String>) -> CallError {
    CallError::InvalidArguments(why.into())
}

/// A thread id argument, and where the model finds one.
fn thread_id(arguments: &Value, key: &str, from: &str) -> Result<Uuid, CallError> {
    let thread = arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .ok_or_else(|| invalid(format!("`{key}` is required: pass an id from {from}.")))?;
    Uuid::parse_str(thread).map_err(|_| {
        invalid(format!(
            "`{thread}` is not a FlowX thread id. Pass an id exactly as {from} returned it."
        ))
    })
}

/// A required, non-blank text argument, trimmed.
fn text(arguments: &Value, key: &str, what: &str) -> Result<String, CallError> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid(format!("`{key}` is required: {what}.")))
}

fn prompt(arguments: &Value) -> Result<String, CallError> {
    let prompt = text(arguments, "prompt", "what the thread is asked to do")?;
    let length = prompt.chars().count();
    if length > MAX_PROMPT_CHARS {
        return Err(invalid(format!(
            "`prompt` is {length} characters; it may be at most {MAX_PROMPT_CHARS}. Put \
             long material in a file in the project and name the file instead."
        )));
    }
    Ok(prompt)
}

/// What `tools/list` answers for a server speaking as `role`.
pub fn catalog(role: Role) -> Value {
    let mut tools = vec![
        json!({
            "name": LIST_THREADS,
            "title": "List FlowX threads",
            "description": format!(
                "List the threads in the FlowX project you are working in. \
                FlowX is the app running you; each of its threads is a separate \
                conversation with an agent (Claude Code or Codex) in this project's \
                folder. Returns JSON with the project's name and folder, and for each \
                thread: `id` (pass it to read_thread), `title`, `provider` (`claude` or \
                `codex`), `status` (`running`: a turn is in progress; `waiting`: it is \
                blocked on the user's approval or answer; `idle`: it is not working), \
                `archived`, and `current` (true for the thread you are in).{} Read-only: \
                it changes nothing.",
                if role == Role::Lead {
                    " As a space's orchestrator you also get `in_space` (the thread is in \
                    your space, so you can send to, wait for and stop it) and \
                    `spawned_by_you`."
                } else {
                    ""
                }
            ),
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            },
            "annotations": reads("List FlowX threads")
        }),
        json!({
            "name": READ_THREAD,
            "title": "Read a FlowX thread",
            "description": format!(
                "Read the latest messages of one thread in your FlowX project, oldest \
                first, each labelled with who wrote it ([user] or [assistant]). Only \
                what the user wrote and the assistant's prose replies are included: tool \
                calls, their output and reasoning are left out, a very long message is \
                shortened, and the whole answer is kept short, so ask for fewer messages \
                rather than more. A reply still being written is marked as such. Call \
                list_threads first to find the thread's id. Read-only: it changes \
                nothing. Returns at most {MAX_COUNT} messages."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "thread_id": {
                        "type": "string",
                        "description": "The thread's `id`, exactly as list_threads returned it."
                    },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_COUNT,
                        "default": DEFAULT_COUNT,
                        "description": "How many of the latest messages to return."
                    }
                },
                "required": ["thread_id"],
                "additionalProperties": false
            },
            "annotations": reads("Read a FlowX thread")
        }),
    ];
    if role == Role::Lead {
        tools.extend(orchestration_tools());
    }
    json!({ "tools": tools })
}

/// The four tools only a lead is offered.
fn orchestration_tools() -> [Value; 4] {
    let thread_id = json!({
        "type": "string",
        "description": "The thread's id, exactly as start_thread or list_threads returned it."
    });
    let prompt = json!({
        "type": "string",
        "maxLength": MAX_PROMPT_CHARS,
        "description": "What the thread is asked to do, as you would write it to an agent \
            that has not seen this conversation: the goal, the files involved, and what to \
            report back. It sees only this prompt and the project."
    });
    [
        json!({
            "name": START_THREAD,
            "title": "Start a FlowX thread",
            "description": format!(
                "Start a new thread in your space and send it its first prompt. \
                The thread is a separate agent (Claude Code or Codex) working in this \
                project; the user sees it under your space in FlowX's sidebar, with the \
                title you give. It runs at your own access level and does not get FlowX's \
                tools. With `worktree: true` it works in its own Git checkout on a new \
                branch at the current commit, so its edits cannot collide with other \
                threads'; give each thread that edits files its own worktree. Returns JSON \
                with `thread_id`, `title`, `provider`, `worktree` (`path` and `branch`, or \
                null) and `status` (`running`). At most {MAX_RUNNING_WORKERS} of your \
                space's threads run at once, and none start while the provider's usage \
                limit is nearly used up. Then call wait_for_threads to collect what it \
                answers."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "maxLength": MAX_TITLE_CHARS,
                        "description": "A short name for the thread, as the sidebar shows it."
                    },
                    "prompt": prompt,
                    "provider": {
                        "type": "string",
                        "enum": ["claude", "codex"],
                        "description": "Which agent runs it. Leave it out to use your own."
                    },
                    "worktree": {
                        "type": "boolean",
                        "default": false,
                        "description": "Whether the thread gets its own Git checkout."
                    }
                },
                "required": ["title", "prompt"],
                "additionalProperties": false
            },
            "annotations": acts("Start a FlowX thread", false, false)
        }),
        json!({
            "name": SEND_TO_THREAD,
            "title": "Send to a FlowX thread",
            "description": "Send a follow-up prompt to a thread in your space that has \
                finished its turn, starting its next one; the thread keeps its \
                conversation, so say only what is new. A thread still running is refused: \
                call wait_for_threads first. Returns JSON with `thread_id` and `status` \
                (`running`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "thread_id": thread_id.clone(),
                    "prompt": prompt
                },
                "required": ["thread_id", "prompt"],
                "additionalProperties": false
            },
            "annotations": acts("Send to a FlowX thread", false, false)
        }),
        json!({
            "name": WAIT_FOR_THREADS,
            "title": "Wait for FlowX threads",
            "description": format!(
                "Wait until threads in your space finish their turns (`until: \"all\"`, \
                the default) or until the first one does (`\"any\"`), for at most \
                `timeout_seconds` (default and maximum {MAX_WAIT_SECONDS}). Returns JSON \
                with `settled` (whether the condition held before the timeout) and, for \
                each thread, `thread_id`, `title`, `status` (`running`, `waiting`, \
                `completed`, `failed`, `stopped` or `idle`) and `last_reply`, its last \
                reply to its latest prompt, shortened past about 4,000 characters. A \
                thread that is `waiting` is blocked on the user, approving a command or \
                answering a question, and says what in `waiting_for`: it counts as \
                finished here, and only the user can unblock it, so tell them. When the \
                wait times out, call it again. Read-only: it changes nothing."
            ),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "thread_ids": {
                        "type": "array",
                        "items": thread_id,
                        "minItems": 1,
                        "maxItems": MAX_WAITED_THREADS,
                        "description": "The threads to wait for."
                    },
                    "until": {
                        "type": "string",
                        "enum": ["all", "any"],
                        "default": "all",
                        "description": "Wait for every thread, or for the first."
                    },
                    "timeout_seconds": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_WAIT_SECONDS,
                        "default": MAX_WAIT_SECONDS,
                        "description": "The most to wait, in seconds."
                    }
                },
                "required": ["thread_ids"],
                "additionalProperties": false
            },
            "annotations": reads("Wait for FlowX threads")
        }),
        json!({
            "name": STOP_THREAD,
            "title": "Stop a FlowX thread",
            "description": "Stop a thread in your space that is running, as the user's \
                Stop button does: its turn ends and the commands it started are ended \
                with it. What it already changed stays. Returns JSON with the thread's \
                `thread_id`, `title` and `status` once it has stopped.",
            "inputSchema": {
                "type": "object",
                "properties": { "thread_id": thread_id },
                "required": ["thread_id"],
                "additionalProperties": false
            },
            "annotations": acts("Stop a FlowX thread", true, true)
        }),
    ]
}

fn reads(title: &str) -> Value {
    json!({
        "title": title,
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    })
}

fn acts(title: &str, destructive: bool, idempotent: bool) -> Value {
    json!({
        "title": title,
        "readOnlyHint": false,
        "destructiveHint": destructive,
        "idempotentHint": idempotent,
        "openWorldHint": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx_core::policy::flowx_tools::{ORCHESTRATION, READ_ONLY};

    const THREAD: &str = "6f0c1f2e-3a4b-4c5d-8e6f-708192a3b4c5";
    const OTHER: &str = "7a1d2f3e-4b5c-4d6e-9f70-8192a3b4c5d6";

    fn names(catalog: &Value) -> Vec<&str> {
        catalog["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn the_catalog_lists_both_tools_as_reads_with_schemas() {
        let catalog = catalog(Role::Thread);
        let tools = catalog["tools"].as_array().unwrap();
        assert_eq!(names(&catalog), [LIST_THREADS, READ_THREAD]);
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert_eq!(tool["annotations"]["readOnlyHint"], true, "{tool}");
            assert_eq!(tool["annotations"]["destructiveHint"], false, "{tool}");
            let description = tool["description"].as_str().unwrap();
            assert!(description.contains("Read-only"), "{description}");
            assert!(!description.contains("in_space"), "{description}");
        }
        assert_eq!(tools[1]["inputSchema"]["required"], json!(["thread_id"]));
    }

    #[test]
    fn only_a_lead_is_offered_the_orchestration_tools_and_only_waiting_reads() {
        // The hint decides the listing; the app still refuses a non-lead.
        let catalog = catalog(Role::Lead);
        let listed = names(&catalog);
        assert_eq!(
            listed,
            [
                LIST_THREADS,
                READ_THREAD,
                START_THREAD,
                SEND_TO_THREAD,
                WAIT_FOR_THREADS,
                STOP_THREAD
            ]
        );
        for tool in catalog["tools"].as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            let read = tool["annotations"]["readOnlyHint"].as_bool().unwrap();
            // The annotation agrees with the names the approval policies read.
            assert_eq!(read, READ_ONLY.contains(&name), "{name}");
            assert!(ORCHESTRATION.contains(&name) || [LIST_THREADS, READ_THREAD].contains(&name));
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{name}");
        }
        assert!(
            catalog["tools"][0]["description"]
                .as_str()
                .unwrap()
                .contains("in_space")
        );
    }

    #[test]
    fn the_role_is_a_lead_only_when_the_environment_says_exactly_so() {
        assert_eq!(Role::from_value(Some("lead")), Role::Lead);
        assert_eq!(Role::from_value(Some(" lead\n")), Role::Lead);
        for other in [None, Some(""), Some("Lead"), Some("thread"), Some("leader")] {
            assert_eq!(Role::from_value(other), Role::Thread, "{other:?}");
        }
    }

    #[test]
    fn list_threads_takes_no_arguments_and_tolerates_none_at_all() {
        for arguments in [json!({}), Value::Null, json!({"ignored": 1})] {
            assert_eq!(
                ToolCall::parse(LIST_THREADS, &arguments),
                Ok(ToolCall::ListThreads)
            );
        }
        assert!(matches!(
            ToolCall::parse(LIST_THREADS, &json!([1])),
            Err(CallError::InvalidArguments(_))
        ));
    }

    #[test]
    fn read_thread_needs_an_id_and_bounds_its_count() {
        let thread = Uuid::parse_str(THREAD).unwrap();
        assert_eq!(
            ToolCall::parse(READ_THREAD, &json!({"thread_id": THREAD})),
            Ok(ToolCall::ReadThread {
                thread,
                count: DEFAULT_COUNT
            })
        );
        assert_eq!(
            ToolCall::parse(READ_THREAD, &json!({"thread_id": THREAD, "count": 500})),
            Ok(ToolCall::ReadThread {
                thread,
                count: MAX_COUNT
            })
        );
        for wrong in [
            json!({}),
            json!({"thread_id": "not-an-id"}),
            json!({"thread_id": 7}),
            json!({"thread_id": THREAD, "count": 0}),
            json!({"thread_id": THREAD, "count": -3}),
            json!({"thread_id": THREAD, "count": "five"}),
        ] {
            assert!(
                matches!(
                    ToolCall::parse(READ_THREAD, &wrong),
                    Err(CallError::InvalidArguments(_))
                ),
                "{wrong}"
            );
        }
    }

    #[test]
    fn start_thread_needs_a_short_title_and_a_prompt_and_reads_its_options() {
        assert_eq!(
            ToolCall::parse(
                START_THREAD,
                &json!({"title": "  Fix\nthe   parser ", "prompt": " Do it. "})
            ),
            Ok(ToolCall::StartThread {
                title: "Fix the parser".into(),
                prompt: "Do it.".into(),
                provider: None,
                worktree: false,
            })
        );
        assert_eq!(
            ToolCall::parse(
                START_THREAD,
                &json!({"title": "t", "prompt": "p", "provider": "claude", "worktree": true})
            ),
            Ok(ToolCall::StartThread {
                title: "t".into(),
                prompt: "p".into(),
                provider: Some("claude".into()),
                worktree: true,
            })
        );
        let exactly = "é".repeat(MAX_TITLE_CHARS);
        assert!(ToolCall::parse(START_THREAD, &json!({"title": exactly, "prompt": "p"})).is_ok());
        for wrong in [
            json!({"prompt": "p"}),
            json!({"title": "  ", "prompt": "p"}),
            json!({"title": "t"}),
            json!({"title": "t", "prompt": ""}),
            json!({"title": "x".repeat(MAX_TITLE_CHARS + 1), "prompt": "p"}),
            json!({"title": "t", "prompt": "x".repeat(MAX_PROMPT_CHARS + 1)}),
            json!({"title": "t", "prompt": "p", "provider": "gemini"}),
            json!({"title": "t", "prompt": "p", "provider": 1}),
            json!({"title": "t", "prompt": "p", "worktree": "yes"}),
        ] {
            assert!(
                matches!(
                    ToolCall::parse(START_THREAD, &wrong),
                    Err(CallError::InvalidArguments(_))
                ),
                "{wrong}"
            );
        }
    }

    #[test]
    fn a_wait_reads_its_threads_once_each_and_bounds_its_timeout() {
        let (thread, other) = (
            Uuid::parse_str(THREAD).unwrap(),
            Uuid::parse_str(OTHER).unwrap(),
        );
        assert_eq!(
            ToolCall::parse(
                WAIT_FOR_THREADS,
                &json!({"thread_ids": [THREAD, OTHER, THREAD]})
            ),
            Ok(ToolCall::WaitForThreads {
                threads: vec![thread, other],
                until: Until::All,
                timeout_seconds: MAX_WAIT_SECONDS,
            })
        );
        assert_eq!(
            ToolCall::parse(
                WAIT_FOR_THREADS,
                &json!({"thread_ids": [OTHER], "until": "any", "timeout_seconds": 9_000})
            ),
            Ok(ToolCall::WaitForThreads {
                threads: vec![other],
                until: Until::Any,
                timeout_seconds: MAX_WAIT_SECONDS,
            })
        );
        let too_many: Vec<String> = (0..=MAX_WAITED_THREADS)
            .map(|_| Uuid::new_v4().to_string())
            .collect();
        for wrong in [
            json!({}),
            json!({"thread_ids": []}),
            json!({"thread_ids": THREAD}),
            json!({"thread_ids": ["nope"]}),
            json!({"thread_ids": too_many}),
            json!({"thread_ids": [THREAD], "until": "some"}),
            json!({"thread_ids": [THREAD], "timeout_seconds": 0}),
            json!({"thread_ids": [THREAD], "timeout_seconds": 2.5}),
        ] {
            assert!(
                matches!(
                    ToolCall::parse(WAIT_FOR_THREADS, &wrong),
                    Err(CallError::InvalidArguments(_))
                ),
                "{wrong}"
            );
        }
    }

    #[test]
    fn a_call_is_given_as_long_as_it_needs_and_the_lead_s_timeout_covers_them_all() {
        let wait = ToolCall::WaitForThreads {
            threads: vec![Uuid::new_v4()],
            until: Until::All,
            timeout_seconds: MAX_WAIT_SECONDS,
        };
        assert_eq!(
            wait.answer_timeout(),
            Duration::from_secs(MAX_WAIT_SECONDS) + WAIT_MARGIN
        );
        assert_eq!(
            ToolCall::ListThreads.answer_timeout(),
            DEFAULT_ANSWER_TIMEOUT
        );
        // What the server allows a call end to end is its own timeout plus
        // what the call needs beyond an ordinary one; the provider must allow
        // more than that, or it gives up on a wait the app is still running.
        let server = crate::server::Options::default().call_timeout;
        for call in [
            wait,
            ToolCall::StartThread {
                title: "t".into(),
                prompt: "p".into(),
                provider: None,
                worktree: true,
            },
            ToolCall::StopThread {
                thread: Uuid::new_v4(),
            },
        ] {
            let budget = server + call.answer_timeout().saturating_sub(DEFAULT_ANSWER_TIMEOUT);
            assert!(budget < LEAD_TOOL_TIMEOUT, "{call:?}");
            assert!(call.is_orchestration(), "{call:?}");
        }
    }

    #[test]
    fn an_unknown_tool_is_not_an_argument_error() {
        assert_eq!(
            ToolCall::parse("spawn_thread", &json!({})),
            Err(CallError::UnknownTool("spawn_thread".into()))
        );
    }

    #[test]
    fn a_call_crosses_the_socket_and_reads_back_the_same() {
        let thread = Uuid::parse_str(THREAD).unwrap();
        for call in [
            ToolCall::ListThreads,
            ToolCall::ReadThread { thread, count: 3 },
            ToolCall::StartThread {
                title: "Parser".into(),
                prompt: "Rewrite it".into(),
                provider: Some("codex".into()),
                worktree: true,
            },
            ToolCall::StartThread {
                title: "Parser".into(),
                prompt: "Rewrite it".into(),
                provider: None,
                worktree: false,
            },
            ToolCall::SendToThread {
                thread,
                prompt: "And the tests".into(),
            },
            ToolCall::WaitForThreads {
                threads: vec![thread, Uuid::parse_str(OTHER).unwrap()],
                until: Until::Any,
                timeout_seconds: 30,
            },
            ToolCall::StopThread { thread },
        ] {
            assert_eq!(ToolCall::parse(call.name(), &call.arguments()), Ok(call));
        }
    }
}
