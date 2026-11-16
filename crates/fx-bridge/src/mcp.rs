//! MCP over stdio, as far as a tools-only server speaks it.
//!
//! Newline-delimited JSON-RPC 2.0. Every message is decided here, purely:
//! what to answer at once, what to forward to the app, and what to leave
//! unanswered. The server loop only moves the bytes.
//!
//! - `initialize` is answered with the client's protocol version when this
//!   server knows it, and its own latest otherwise, which the client may
//!   then refuse. Only the `tools` capability is offered.
//! - `ping` is answered with an empty result, `tools/list` with the catalog.
//! - `tools/call` for a known tool goes to the app. An unknown tool is a
//!   protocol error (-32602, as MCP specifies); wrong arguments are the
//!   tool's own error result, which the model reads and can correct.
//! - Any other request is method-not-found (-32601). A notification
//!   (`notifications/initialized` among them) and a response are never
//!   answered: JSON-RPC forbids a reply to either.
//! - A line that is not JSON is a parse error (-32700) with a null id; one
//!   that is JSON but not a request is invalid (-32600). A batch is invalid
//!   too: MCP dropped batching in 2025-06-18.

use serde_json::{Value, json};

use crate::tools::{self, CallError, ToolCall};

/// The protocol versions this server can speak, newest first. The tools
/// methods it uses are the same in each.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

/// What the server tells the client about itself.
pub const INSTRUCTIONS: &str = "Tools for the FlowX project you are working in: \
    list_threads lists its threads (separate agent conversations in FlowX), and \
    read_thread reads a thread's latest messages. Both only read.";

/// What to do with one line from the client.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Write this message back.
    Reply(Value),
    /// Ask the app, and answer request `id` with what it says.
    Forward { id: Value, call: ToolCall },
    /// Write nothing.
    Ignore,
}

/// Decide what one line asks for.
pub fn handle(line: &[u8], server_version: &str) -> Action {
    let message: Value = match serde_json::from_slice(line) {
        Ok(message) => message,
        Err(_) if line.iter().all(u8::is_ascii_whitespace) => return Action::Ignore,
        Err(_) => {
            return Action::Reply(error(&Value::Null, PARSE_ERROR, "Parse error"));
        }
    };
    let Some(object) = message.as_object() else {
        return Action::Reply(error(
            &Value::Null,
            INVALID_REQUEST,
            "Invalid request: expected one JSON-RPC message object",
        ));
    };
    let id = object.get("id").cloned();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // A response to a request this server never sent, or nonsense. A
        // response is never answered; anything else with an id is invalid.
        let is_response = object.contains_key("result") || object.contains_key("error");
        return match id {
            Some(id) if !is_response => {
                Action::Reply(error(&id, INVALID_REQUEST, "Invalid request: no method"))
            }
            _ => Action::Ignore,
        };
    };
    let Some(id) = id else {
        // A notification: `notifications/initialized`, `notifications/cancelled`
        // and anything newer alike.
        return Action::Ignore;
    };
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => Action::Reply(result(&id, initialize(&params, server_version))),
        "ping" => Action::Reply(result(&id, json!({}))),
        "tools/list" => Action::Reply(result(&id, tools::catalog())),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
            match ToolCall::parse(name, &arguments) {
                Ok(call) => Action::Forward { id, call },
                Err(CallError::UnknownTool(name)) => {
                    Action::Reply(error(&id, INVALID_PARAMS, &format!("Unknown tool: {name}")))
                }
                Err(CallError::InvalidArguments(why)) => {
                    Action::Reply(result(&id, tool_result(&why, true)))
                }
            }
        }
        other => Action::Reply(error(
            &id,
            METHOD_NOT_FOUND,
            &format!("Method not found: {other}"),
        )),
    }
}

fn initialize(params: &Value, server_version: &str) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": fx_core::policy::flowx_tools::SERVER,
            "title": "FlowX",
            "version": server_version,
        },
        "instructions": INSTRUCTIONS,
    })
}

/// A successful response.
pub fn result(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// An error response.
pub fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// A tool's result: its text, and whether it is the tool's error.
pub fn tool_result(text: &str, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(line: &str) -> Value {
        match handle(line.as_bytes(), "9.9.9") {
            Action::Reply(reply) => reply,
            other => panic!("{line}: {other:?}"),
        }
    }

    #[test]
    fn initialize_echoes_a_version_it_knows_and_offers_only_tools() {
        let answer = reply(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"claude-code","version":"2"}}}"#,
        );
        assert_eq!(answer["id"], 0);
        let result = &answer["result"];
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert_eq!(
            result["capabilities"],
            json!({"tools": {"listChanged": false}})
        );
        assert_eq!(result["serverInfo"]["name"], "flowx");
        assert_eq!(result["serverInfo"]["version"], "9.9.9");
        assert!(
            result["instructions"]
                .as_str()
                .unwrap()
                .contains("read_thread")
        );
    }

    #[test]
    fn initialize_answers_an_unknown_version_with_its_own_latest() {
        let answer = reply(
            r#"{"jsonrpc":"2.0","id":"a","method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#,
        );
        assert_eq!(answer["id"], "a");
        assert_eq!(answer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn ping_and_tools_list_are_answered_at_once() {
        assert_eq!(
            reply(r#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#),
            json!({"jsonrpc":"2.0","id":4,"result":{}})
        );
        let listed = reply(r#"{"jsonrpc":"2.0","id":5,"method":"tools/list","params":{}}"#);
        assert_eq!(listed["result"], tools::catalog());
    }

    #[test]
    fn a_known_call_is_forwarded_with_its_id() {
        assert_eq!(
            handle(
                br#"{"jsonrpc":"2.0","id":"call-1","method":"tools/call","params":{"name":"list_threads","arguments":{}}}"#,
                "1"
            ),
            Action::Forward {
                id: json!("call-1"),
                call: ToolCall::ListThreads
            }
        );
    }

    #[test]
    fn an_unknown_tool_is_a_protocol_error_and_bad_arguments_are_the_tool_s() {
        let unknown = reply(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"delete_everything"}}"#,
        );
        assert_eq!(unknown["error"]["code"], INVALID_PARAMS);
        let wrong = reply(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"read_thread","arguments":{}}}"#,
        );
        assert_eq!(wrong["result"]["isError"], true);
        assert!(
            wrong["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("thread_id")
        );
    }

    #[test]
    fn any_other_request_is_method_not_found() {
        for method in ["resources/list", "prompts/list", "completion/complete", "x"] {
            let answer = reply(&format!(
                r#"{{"jsonrpc":"2.0","id":8,"method":"{method}"}}"#
            ));
            assert_eq!(answer["id"], 8);
            assert_eq!(answer["error"]["code"], METHOD_NOT_FOUND, "{method}");
        }
    }

    #[test]
    fn notifications_and_responses_are_never_answered() {
        for line in [
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}"#,
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"list_threads"}}"#,
            r#"{"jsonrpc":"2.0","id":3,"result":{}}"#,
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":1,"message":"no"}}"#,
            "",
            "   ",
        ] {
            assert_eq!(handle(line.as_bytes(), "1"), Action::Ignore, "{line}");
        }
    }

    #[test]
    fn malformed_lines_are_parse_or_invalid_request_errors() {
        let broken = reply("{not json");
        assert_eq!(broken["error"]["code"], PARSE_ERROR);
        assert_eq!(broken["id"], Value::Null);
        for line in [
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#,
            r#""ping""#,
            r#"{"jsonrpc":"2.0","id":9}"#,
        ] {
            assert_eq!(reply(line)["error"]["code"], INVALID_REQUEST, "{line}");
        }
    }
}
