//! An MCP server a provider is launched with, in each provider's own terms.
//!
//! Both CLIs start a stdio MCP server from a command, its arguments and its
//! environment. They are told about it differently:
//!
//! - **Claude** takes `--mcp-config` with the JSON of a `.mcp.json` file:
//!   `{"mcpServers": {name: {type, command, args, env}}}`. The flag takes a
//!   list, so it is written `--mcp-config=<json>`, or the argument after it
//!   would be read as a second config. `--restricted` still loads it; only
//!   `--strict-mcp-config` would drop the user's own servers, and FlowX does
//!   not pass that.
//! - **Codex** takes `thread/start`'s and `thread/resume`'s `config`, a map of
//!   config overrides keyed by dotted path, the same as `codex -c`. The key is
//!   `mcp_servers.<name>`, never `mcp_servers` itself: a whole-table key is a
//!   claim on every server the user configured, where the dotted key adds one.
//!   Codex hands an MCP server only a minimal environment of its own plus the
//!   entry's `env`, so everything the server needs goes there.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

/// One stdio MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    /// What the provider calls it, and what prefixes its tools'
    /// names (`mcp__<name>__<tool>`). Letters, digits, `_` and `-` only: a
    /// dot would split Codex's dotted key.
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl McpServer {
    /// Whether the name is one both providers read as one name.
    pub fn has_valid_name(&self) -> bool {
        !self.name.is_empty()
            && self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    fn entry(&self) -> Value {
        json!({
            "command": self.command,
            "args": self.args,
            "env": self.env,
        })
    }
}

/// Claude's `--mcp-config` value for these servers.
pub fn claude_config(servers: &[McpServer]) -> String {
    let servers: Map<String, Value> = servers
        .iter()
        .filter(|server| server.has_valid_name())
        .map(|server| {
            let mut entry = server.entry();
            entry["type"] = json!("stdio");
            (server.name.clone(), entry)
        })
        .collect();
    json!({ "mcpServers": servers }).to_string()
}

/// Codex's `config` overrides for these servers.
pub fn codex_config(servers: &[McpServer]) -> Map<String, Value> {
    servers
        .iter()
        .filter(|server| server.has_valid_name())
        .map(|server| (format!("mcp_servers.{}", server.name), server.entry()))
        .collect()
}

#[cfg(test)]
pub(crate) fn fixture() -> McpServer {
    McpServer {
        name: "flowx".into(),
        command: "/Applications/FlowX.app/Contents/MacOS/flowx".into(),
        args: vec!["--mcp-server".into()],
        env: BTreeMap::from([
            (
                "FLOWX_MCP_SOCKET".into(),
                "/tmp/flowx-bridge-x/flowx.sock".into(),
            ),
            ("FLOWX_MCP_THREAD".into(), "thread-1".into()),
            ("FLOWX_MCP_TOKEN".into(), "secret".into()),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_reads_the_server_as_a_dot_mcp_json_entry() {
        let config: Value = serde_json::from_str(&claude_config(&[fixture()])).unwrap();
        let entry = &config["mcpServers"]["flowx"];
        assert_eq!(entry["type"], "stdio");
        assert_eq!(
            entry["command"],
            "/Applications/FlowX.app/Contents/MacOS/flowx"
        );
        assert_eq!(entry["args"], json!(["--mcp-server"]));
        assert_eq!(entry["env"]["FLOWX_MCP_THREAD"], "thread-1");
        assert_eq!(config.as_object().unwrap().len(), 1, "{config}");
    }

    #[test]
    fn codex_gets_one_dotted_key_per_server_and_never_the_whole_table() {
        // `mcp_servers` itself would be a claim on the user's own servers.
        let config = codex_config(&[fixture()]);
        assert_eq!(config.keys().collect::<Vec<_>>(), ["mcp_servers.flowx"]);
        let entry = &config["mcp_servers.flowx"];
        assert_eq!(entry["args"], json!(["--mcp-server"]));
        assert_eq!(
            entry["env"]["FLOWX_MCP_SOCKET"],
            "/tmp/flowx-bridge-x/flowx.sock"
        );
        assert!(entry.get("type").is_none(), "{entry}");
    }

    #[test]
    fn a_name_either_provider_would_split_is_left_out() {
        for name in ["", "flow.x", "flow x", "flowx/tools"] {
            let server = McpServer {
                name: name.into(),
                ..fixture()
            };
            assert!(
                codex_config(std::slice::from_ref(&server)).is_empty(),
                "{name}"
            );
            let claude: Value = serde_json::from_str(&claude_config(&[server])).unwrap();
            assert_eq!(claude["mcpServers"], json!({}), "{name}");
        }
    }
}
