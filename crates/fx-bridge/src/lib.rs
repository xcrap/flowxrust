//! `fx-bridge` — how an agent running inside FlowX calls FlowX.
//!
//! Two halves, joined by a Unix socket:
//!
//! - **The server** (`flowx --mcp-server`) is what a provider launches for a
//!   thread that opted in. It speaks MCP over stdio to Claude Code or Codex,
//!   and forwards each tool call to the running app ([`server`]).
//! - **The listener** is the app's end: a socket in a private directory that
//!   answers only a caller presenting its thread's token ([`listener`]).
//!
//! The server learns where the app is, who it is speaking for and how to
//! prove it from three variables in its environment, which FlowX sets when it
//! launches the provider ([`ENV_SOCKET`], [`ENV_THREAD`], [`ENV_TOKEN`]). A
//! fourth, [`ENV_ROLE`], says whether that thread leads a space, which is
//! what decides the tools it lists; it proves nothing, and the app checks
//! every call.
//!
//! Nothing here reads app state. The app answers a call with text; what the
//! tools are called, what they take and how their answers are shaped is here
//! ([`tools`], [`answers`]), so it is tested without a window.

pub mod answers;
pub mod lines;
pub mod listener;
pub mod mcp;
pub mod server;
pub mod token;
pub mod tools;
pub mod wire;

/// The argument that starts the binary as an MCP server instead of the app.
pub const SERVER_FLAG: &str = "--mcp-server";

/// The listener's socket path.
pub const ENV_SOCKET: &str = "FLOWX_MCP_SOCKET";
/// The FlowX thread the server speaks for.
pub const ENV_THREAD: &str = "FLOWX_MCP_THREAD";
/// The token that proves it: [`token::Secret::token_for`] that thread.
pub const ENV_TOKEN: &str = "FLOWX_MCP_TOKEN";
/// [`ROLE_LEAD`] for a space's orchestrator, absent otherwise.
pub const ENV_ROLE: &str = "FLOWX_MCP_ROLE";
/// The role value that lists the orchestration tools.
pub const ROLE_LEAD: &str = "lead";
