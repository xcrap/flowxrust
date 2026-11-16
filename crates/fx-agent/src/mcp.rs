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
//!
//! Both bound each tool call, and both take the bound per server, so a server
//! whose calls run long says so in its own entry and every other server keeps
//! the user's setting: Claude's `timeout`, in milliseconds (Claude Code
//! 2.1.325's stdio schema; it overrides `MCP_TOOL_TIMEOUT` for that server
//! alone), and Codex's `tool_timeout_sec`, in seconds as a float (codex-cli
//! 0.167's `McpServerConfig`).

use std::collections::BTreeMap;
use std::time::Duration;

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
    /// How long one tool call may take before the provider gives up on it.
    /// `None` leaves the provider's own default, or the user's.
    pub tool_timeout: Option<Duration>,
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

    /// The timeout in milliseconds, as Claude reads it. Claude ignores a
    /// value under a second and falls through to its default, so a shorter
    /// one is rounded up rather than lost.
    fn timeout_ms(&self) -> Option<u64> {
        self.tool_timeout.map(|timeout| {
            u64::try_from(timeout.as_millis())
                .unwrap_or(u64::MAX)
                .max(1_000)
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
            if let Some(timeout) = server.timeout_ms() {
                entry["timeout"] = json!(timeout);
            }
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
        .map(|server| {
            let mut entry = server.entry();
            if let Some(timeout) = server.tool_timeout {
                // A float, as Codex writes it back itself: the key is read as
                // `Option<f64>` seconds.
                entry["tool_timeout_sec"] = json!(timeout.as_secs_f64());
            }
            (format!("mcp_servers.{}", server.name), entry)
        })
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
        tool_timeout: None,
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
        // No timeout stated: the provider's, or the user's, applies.
        assert!(entry.get("timeout").is_none(), "{entry}");
    }

    #[test]
    fn a_server_s_tool_timeout_is_stated_in_each_provider_s_own_unit() {
        // A lead's wait holds a call open for minutes. Each provider takes
        // the bound in the server's own entry, so FlowX's server is given
        // longer without changing any other server's.
        let server = McpServer {
            tool_timeout: Some(Duration::from_secs(600)),
            ..fixture()
        };
        let claude: Value =
            serde_json::from_str(&claude_config(std::slice::from_ref(&server))).unwrap();
        assert_eq!(claude["mcpServers"]["flowx"]["timeout"], 600_000);
        let codex = codex_config(std::slice::from_ref(&server));
        assert_eq!(codex["mcp_servers.flowx"]["tool_timeout_sec"], 600.0);
        assert!(codex["mcp_servers.flowx"]["tool_timeout_sec"].is_f64());
        assert!(
            codex_config(&[fixture()])["mcp_servers.flowx"]
                .get("tool_timeout_sec")
                .is_none()
        );
        // Claude reads under a second as unset, so it is never sent as one.
        let short = McpServer {
            tool_timeout: Some(Duration::from_millis(10)),
            ..fixture()
        };
        let claude: Value = serde_json::from_str(&claude_config(&[short])).unwrap();
        assert_eq!(claude["mcpServers"]["flowx"]["timeout"], 1_000);
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
