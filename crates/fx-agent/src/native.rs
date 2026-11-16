//! Provider-owned history used by the workspace's read-only synchronization.

use fx_core::{ConversationMessage, NativeDefaults};

use crate::ThreadSummary;

#[derive(Debug, Clone, PartialEq)]
pub struct NativeThreadSummary {
    pub summary: ThreadSummary,
    pub defaults: NativeDefaults,
    /// Changes whenever provider metadata or transcript contents change.
    pub revision: String,
    /// What the thread is blocked on, when the provider reports a process
    /// FlowX does not run waiting on the user: an approval, a question, an
    /// MCP server's request. FlowX cannot answer it — only the process that
    /// asked can hear the answer — so it is shown, not offered.
    pub waiting_elsewhere: Option<String>,
    /// Whether the thread is running a turn in another app right now, as its
    /// provider reports it: Claude's `sessions/<pid>.json` with `status:
    /// "busy"`. Shown, so a thread worked on elsewhere does not read as idle
    /// while its history grows.
    pub running_elsewhere: bool,
    /// The section the provider files the thread under, when it has them:
    /// Codex's, which are account-wide and named by the user (the Codex app
    /// keeps its pins as one), and the Claude desktop project that started a
    /// Claude thread, which the thread's own messages name.
    pub section: Option<NativeSection>,
}

/// A provider's own grouping of threads, as it reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSection {
    /// Opaque, and stable across a rename.
    pub id: String,
    pub name: String,
    /// The provider's colour for it, as it spells it.
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NativeThread {
    pub entry: NativeThreadSummary,
    pub messages: Vec<ConversationMessage>,
    pub current_context_tokens: Option<u64>,
    /// A size or protocol limit must be surfaced, never silently called full history.
    pub history_truncated: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NativeThreadIndex {
    pub threads: Vec<NativeThreadSummary>,
    pub warnings: Vec<String>,
}
