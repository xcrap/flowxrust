//! The tools, as a model is told about them, and the calls they accept.
//!
//! Every tool here only reads. Each says so twice: in its description, which
//! the model reads, and in its `readOnlyHint` annotation, which a client may
//! use to skip asking. FlowX's own approval policies do not rely on the hint;
//! they know the names (`fx_core::policy::flowx_tools`).

use fx_core::policy::flowx_tools::{LIST_THREADS, READ_THREAD};
use serde_json::{Value, json};

/// How many messages `read_thread` returns when the model does not say.
pub const DEFAULT_COUNT: usize = 10;
/// The most it returns, whatever the model asks for.
pub const MAX_COUNT: usize = 50;

/// One call the app is asked to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCall {
    ListThreads,
    ReadThread {
        /// A FlowX thread id, as `list_threads` gives it.
        thread: uuid::Uuid,
        count: usize,
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
                return Err(CallError::InvalidArguments(
                    "The arguments must be an object.".into(),
                ));
            }
        };
        match name {
            LIST_THREADS => Ok(Self::ListThreads),
            READ_THREAD => {
                let thread = arguments
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .ok_or_else(|| {
                        CallError::InvalidArguments(
                            "`thread_id` is required: pass an `id` from list_threads.".into(),
                        )
                    })?;
                let thread = uuid::Uuid::parse_str(thread).map_err(|_| {
                    CallError::InvalidArguments(format!(
                        "`{thread}` is not a FlowX thread id. Pass an `id` exactly as \
                         list_threads returned it."
                    ))
                })?;
                let count = match arguments.get("count") {
                    None | Some(Value::Null) => DEFAULT_COUNT,
                    Some(count) => match count.as_u64() {
                        Some(0) | None => {
                            return Err(CallError::InvalidArguments(format!(
                                "`count` must be a whole number from 1 to {MAX_COUNT}."
                            )));
                        }
                        Some(count) => usize::try_from(count).unwrap_or(MAX_COUNT).min(MAX_COUNT),
                    },
                };
                Ok(Self::ReadThread { thread, count })
            }
            other => Err(CallError::UnknownTool(other.to_owned())),
        }
    }

    /// The tool's name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::ListThreads => LIST_THREADS,
            Self::ReadThread { .. } => READ_THREAD,
        }
    }

    /// The arguments, as they cross the socket.
    pub fn arguments(&self) -> Value {
        match self {
            Self::ListThreads => json!({}),
            Self::ReadThread { thread, count } => {
                json!({ "thread_id": thread.to_string(), "count": count })
            }
        }
    }
}

/// What `tools/list` answers.
pub fn catalog() -> Value {
    json!({
        "tools": [
            {
                "name": LIST_THREADS,
                "title": "List FlowX threads",
                "description": "List the threads in the FlowX project you are working in. \
                    FlowX is the app running you; each of its threads is a separate \
                    conversation with an agent (Claude Code or Codex) in this project's \
                    folder. Returns JSON with the project's name and folder, and for each \
                    thread: `id` (pass it to read_thread), `title`, `provider` (`claude` or \
                    `codex`), `status` (`running`: a turn is in progress; `waiting`: it is \
                    blocked on the user's approval or answer; `idle`: it is not working), \
                    `archived`, and `current` (true for the thread you are in). Read-only: \
                    it changes nothing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                },
                "annotations": annotations("List FlowX threads")
            },
            {
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
                "annotations": annotations("Read a FlowX thread")
            }
        ]
    })
}

fn annotations(title: &str) -> Value {
    json!({
        "title": title,
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "6f0c1f2e-3a4b-4c5d-8e6f-708192a3b4c5";

    #[test]
    fn the_catalog_lists_both_tools_as_reads_with_schemas() {
        let catalog = catalog();
        let tools = catalog["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, [LIST_THREADS, READ_THREAD]);
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert_eq!(tool["annotations"]["readOnlyHint"], true, "{tool}");
            assert_eq!(tool["annotations"]["destructiveHint"], false, "{tool}");
            let description = tool["description"].as_str().unwrap();
            assert!(description.contains("Read-only"), "{description}");
        }
        assert_eq!(tools[1]["inputSchema"]["required"], json!(["thread_id"]));
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
        let thread = uuid::Uuid::parse_str(THREAD).unwrap();
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
    fn an_unknown_tool_is_not_an_argument_error() {
        assert_eq!(
            ToolCall::parse("spawn_thread", &json!({})),
            Err(CallError::UnknownTool("spawn_thread".into()))
        );
    }

    #[test]
    fn a_call_crosses_the_socket_and_reads_back_the_same() {
        for call in [
            ToolCall::ListThreads,
            ToolCall::ReadThread {
                thread: uuid::Uuid::parse_str(THREAD).unwrap(),
                count: 3,
            },
        ] {
            assert_eq!(ToolCall::parse(call.name(), &call.arguments()), Ok(call));
        }
    }
}
