//! The tools FlowX itself offers to the agents it runs.
//!
//! An agent reaches them through `flowx --mcp-server`, an MCP server both
//! providers launch for a thread that opted in. They name its tools as every
//! MCP tool is named, `mcp__<server>__<tool>`: Claude in `can_use_tool`, and
//! FlowX for Codex's approval elicitation, which names the server and quotes
//! the tool.
//!
//! The names live here, below both the server that offers the tools and the
//! approval policies that judge them, so the two cannot disagree about what a
//! call is called.

/// The MCP server's name, as both providers are given it.
pub const SERVER: &str = "flowx";

/// Lists the threads of the asking thread's project.
pub const LIST_THREADS: &str = "list_threads";

/// Reads a thread's latest messages.
pub const READ_THREAD: &str = "read_thread";

/// Every tool the server offers that only reads.
pub const READ_ONLY: [&str; 2] = [LIST_THREADS, READ_THREAD];

/// Whether a provider's tool call is one of FlowX's read-only tools, by the
/// name the provider gives it.
///
/// Exact names only. A tool another server offers under a similar name, or a
/// later FlowX tool that writes, is not a read because it resembles one.
pub fn is_read_only_call(provider_tool_name: &str) -> bool {
    provider_tool_name
        .strip_prefix("mcp__")
        .and_then(|rest| rest.strip_prefix(SERVER))
        .and_then(|rest| rest.strip_prefix("__"))
        .is_some_and(|tool| READ_ONLY.contains(&tool))
}

/// The name a provider gives one of FlowX's tools.
pub fn provider_tool_name(tool: &str) -> String {
    format!("mcp__{SERVER}__{tool}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_flowx_s_own_read_tools_count_as_reads() {
        assert!(is_read_only_call("mcp__flowx__list_threads"));
        assert!(is_read_only_call("mcp__flowx__read_thread"));
        for other in [
            // Another server's tool of the same name.
            "mcp__probe__list_threads",
            // A server whose name only starts like FlowX's.
            "mcp__flowxy__list_threads",
            "mcp__flowx__list_threads_and_delete",
            "mcp__flowx__",
            "mcp__flowx",
            "list_threads",
            "Read",
            "",
        ] {
            assert!(!is_read_only_call(other), "{other}");
        }
    }

    #[test]
    fn the_provider_name_is_the_one_the_classifier_accepts() {
        for tool in READ_ONLY {
            assert!(is_read_only_call(&provider_tool_name(tool)), "{tool}");
        }
    }
}
