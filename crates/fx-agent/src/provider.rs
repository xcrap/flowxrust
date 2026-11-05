//! The provider interface.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use fx_core::{CompactionTrigger, Timestamp};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("{0}")]
    Unavailable(String),
    #[error("the turn was cancelled")]
    Cancelled,
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// A model a provider offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub context_window: u64,
    #[serde(default)]
    pub max_context_window: u64,
    #[serde(default)]
    pub supported_efforts: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub is_default: bool,
}

/// Health of a provider's CLI on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProviderHealth {
    #[default]
    Unknown,
    Available {
        path: String,
        version: String,
    },
    Quarantined {
        path: String,
    },
    NotFound,
}

impl ProviderHealth {
    pub fn is_usable(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Available { path, .. } | Self::Quarantined { path } => Some(path),
            _ => None,
        }
    }
}

/// Identity of a provider-native thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThreadIdentity {
    pub provider_id: String,
    pub session_id: String,
}

/// A provider thread, as listed.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadSummary {
    pub identity: ThreadIdentity,
    pub title: String,
    pub working_directory: String,
    pub updated_at: Timestamp,
    pub is_archived: bool,
}

/// One thing that happened during a turn.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ProviderEvent {
    Lifecycle(Lifecycle),
    TextDelta(String),
    /// The message the deltas were building is complete; the next delta
    /// starts a new one.
    TextEnd,
    /// A whole message that did not stream.
    Text(String),
    /// A piece of the model's reasoning, as far as its provider shows it:
    /// Codex streams its summary, Claude its summarized thinking.
    ThinkingDelta(String),
    /// The reasoning the deltas were building is complete.
    ThinkingEnd,
    /// Reasoning that did not stream.
    Thinking(String),
    /// How much the model has thought so far, by the provider's estimate.
    /// Claude reports it even where the thinking itself is withheld, so a
    /// long think has something to show for itself.
    ThinkingProgress {
        estimated_tokens: u64,
    },
    /// A tool call. `input` is what the provider has said of its arguments so
    /// far: Claude reports a call as its block opens, before the arguments
    /// stream, so it can be empty and grow by [`Self::ToolInputDelta`].
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    /// More of a call's arguments, appended to what the call was reported
    /// with. Partial JSON until the call is whole.
    ToolInputDelta {
        id: String,
        delta: String,
    },
    /// A call's arguments whole, replacing whatever streamed.
    ToolInput {
        id: String,
        input: String,
    },
    /// A piece of a running call's output. Shown while the call runs and
    /// never recorded: its result carries the whole.
    ToolOutputDelta {
        id: String,
        delta: String,
    },
    ToolResult {
        id: String,
        content: String,
        is_error: bool,
    },
    Approval(ApprovalRequest),
    UserInput(UserInputRequest),
    /// The provider stopped waiting on the approval or question it asked
    /// under this id: the call was aborted, or another client answered it.
    /// Nothing is to be sent for it any more, and the request is no longer
    /// the reader's to answer.
    InteractionWithdrawn {
        id: String,
    },
    /// The agent rewrote its task list. Codex reports its `update_plan` calls
    /// this way rather than as a tool item; Claude's `TodoWrite` arrives as an
    /// ordinary tool call instead.
    PlanUpdated(fx_core::policy::TurnPlan),
    /// Usage for one turn, added to the thread's totals.
    Usage(Usage),
    /// The thread's running totals as the provider counts them, which replace
    /// rather than add to what the conversation holds.
    CumulativeUsage(Usage),
    ContextUsage {
        used: u64,
        window: u64,
    },
    /// What fills the context, by category, as Claude's `/context` counts it.
    /// Carries the total and the window too, which are the freshest figures
    /// the provider has.
    ContextBreakdown(ContextBreakdown),
    /// The provider compacted the context. The conversation records it where
    /// it happened, as a boundary between the rows around it.
    Compacted(Compaction),
    /// What the model now reads in place of the compacted history. Claude
    /// sends it as the message after the boundary; it belongs to the last
    /// compaction that has none.
    CompactionSummary(String),
    /// The account's usage limits, as the provider read them off its last
    /// response. Account-level, not the thread's: the app keeps one per
    /// provider.
    RateLimits(RateLimits),
    ModelChanged {
        model: String,
        reason: String,
    },
    Error(String),
    Done {
        stop_reason: String,
    },
    /// A background task the turn started — a subagent, or a command run in
    /// the background — started, reported progress, or finished.
    Task(TaskUpdate),
    /// The provider took input FlowX wrote into the running turn: the message
    /// FlowX sent under this id, or, from a provider that does not say which,
    /// everything written so far.
    ///
    /// Taken, not accepted. Both providers accept a steer at once and read it
    /// at their next request, after whatever they are saying or running.
    InputTaken {
        id: Option<String>,
    },
}

/// One report about a background task.
///
/// Partial by design: `task_started` names the task, `task_progress` counts its
/// work, and `task_notification` says how it ended. Each field is `None` where
/// that report did not carry it, and the receiver keeps what it already knew.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskUpdate {
    /// The provider's own id for the task.
    pub id: String,
    /// The tool call that started it.
    pub tool_use_id: Option<String>,
    pub description: Option<String>,
    /// What runs it, in the provider's words: `local_bash`, `local_agent`.
    pub kind: Option<String>,
    /// The subagent type, for a task that is an agent.
    pub subagent_type: Option<String>,
    pub status: TaskStatus,
    pub summary: Option<String>,
    /// The last tool the task used.
    pub last_tool: Option<String>,
    pub tool_uses: Option<u64>,
    pub tokens: Option<u64>,
    /// How long the task had been running when this was reported.
    pub duration_ms: Option<u64>,
}

/// Where a background task is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskStatus {
    #[default]
    Running,
    Completed,
    Failed,
    /// Killed or stopped before it finished, by the user, the provider, or
    /// FlowX giving up on it.
    Stopped,
}

impl TaskStatus {
    pub fn is_running(self) -> bool {
        self == Self::Running
    }

    /// The provider's word for how a task ended. Anything other than success
    /// or failure is a stop: `killed`, `stopped`, and whatever a later version
    /// adds, none of which is a result to report as one.
    pub fn from_notification(status: &str) -> Self {
        match status {
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Stopped,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Lifecycle {
    TurnStarted,
    /// The provider is summarizing the context. Ends with
    /// [`ProviderEvent::Compacted`].
    Compacting,
    Preparing,
}

/// One compaction of the context, as the provider reported it.
///
/// Claude's `compact_boundary` names the trigger and the token counts either
/// side; Codex's `contextCompaction` item names nothing, so every field has a
/// default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compaction {
    pub trigger: CompactionTrigger,
    pub pre_tokens: Option<u64>,
    pub post_tokens: Option<u64>,
}

/// What fills the context window, as Claude's `get_context_usage` reports it:
/// the system prompt, the tools, memory files, the messages, the buffer kept
/// for compaction, and what is free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBreakdown {
    pub categories: Vec<ContextCategory>,
    /// Tokens in use, which is what the context meter shows.
    pub total_tokens: u64,
    /// The window these counts are measured against.
    pub window: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextCategory {
    pub name: String,
    pub tokens: u64,
    pub kind: ContextCategoryKind,
}

/// What a category's tokens are doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContextCategoryKind {
    /// In the context now.
    #[default]
    Used,
    /// Loaded only when needed, so counted but not in use: deferred tools.
    Deferred,
    /// Kept free so compaction has room to work.
    Buffer,
    /// Not in use.
    Free,
}

impl ContextCategoryKind {
    pub fn parse(kind: &str) -> Self {
        match kind {
            "deferred" => Self::Deferred,
            "buffer" => Self::Buffer,
            "free" => Self::Free,
            _ => Self::Used,
        }
    }
}

/// The account's usage limits, as the provider last read them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RateLimits {
    /// Whether the account can still send, as of the last response.
    pub status: RateLimitStatus,
    /// Each window the provider tracks, in the provider's order.
    pub windows: Vec<RateLimitWindow>,
    /// The plan the limits belong to, where the provider names it.
    pub plan: Option<String>,
}

impl RateLimits {
    /// The fullest window, which is the one the reader needs to know about.
    pub fn hottest(&self) -> Option<&RateLimitWindow> {
        self.windows.iter().max_by(|a, b| a.used.total_cmp(&b.used))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RateLimitStatus {
    #[default]
    Allowed,
    /// Allowed, but the provider is warning that a window is nearly used up.
    Warning,
    /// The last request was refused for the limit.
    Rejected,
}

/// One usage window: a fraction used, and when it resets.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitWindow {
    pub kind: RateLimitKind,
    /// The fraction of the window used. Usually within 0–1; a provider can
    /// report more when usage legitimately ran past the cap.
    pub used: f32,
    pub resets_at: Option<Timestamp>,
}

/// Which window a limit is, named so both providers' windows read alike:
/// Claude's `five_hour` and `seven_day`, Codex's primary and secondary by
/// their durations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitKind {
    FiveHour,
    SevenDay,
    /// A window neither provider had when this was written, labelled as the
    /// provider named it.
    Other(String),
}

impl RateLimitKind {
    /// A window by its length in minutes, which is how Codex names its two.
    pub fn from_minutes(minutes: u64) -> Self {
        match minutes {
            300 => Self::FiveHour,
            10_080 => Self::SevenDay,
            minutes if minutes % 1440 == 0 => Self::Other(format!("{}-day", minutes / 1440)),
            minutes if minutes % 60 == 0 => Self::Other(format!("{}-hour", minutes / 60)),
            minutes => Self::Other(format!("{minutes}-minute")),
        }
    }

    /// The window's name as the UI shows it.
    pub fn label(&self) -> String {
        match self {
            Self::FiveHour => "5-hour limit".into(),
            Self::SevenDay => "Weekly limit".into(),
            Self::Other(name) => format!("{name} limit"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalRequest {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
    pub risk: RiskLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RiskLevel {
    Safe,
    #[default]
    Moderate,
    Dangerous,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UserInputRequest {
    pub id: String,
    /// Empty for a request that asks only for consent: an MCP server's
    /// confirmation, or a link it asks the reader to open.
    pub questions: Vec<Question>,
    /// Who is asking, when it is not the agent: an MCP server's name.
    pub source: Option<String>,
    /// What the asker said about the request as a whole.
    pub message: Option<String>,
    /// A web page the asker wants opened. Answering is consenting to open it.
    pub link: Option<String>,
}

/// A response to one request from a running provider turn.
///
/// `id` is the enclosing ApprovalRequest/UserInputRequest id. Question answers
/// pair each nested `Question::id` with its chosen label or free-form text.
/// Transport-specific wire ids never escape into UI callbacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserResponse {
    Approval {
        id: String,
        approved: bool,
    },
    Questions {
        id: String,
        answers: Vec<(String, String)>,
    },
    CancelQuestions {
        id: String,
    },
}

impl UserResponse {
    pub fn id(&self) -> &str {
        match self {
            Self::Approval { id, .. }
            | Self::Questions { id, .. }
            | Self::CancelQuestions { id } => id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Question {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<Option_>,
    /// Whether a typed answer is taken. Without it, and with options, the
    /// answer is one of the options.
    pub is_other: bool,
    pub is_secret: bool,
    /// Whether it may be left blank.
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Option_ {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub total_tokens: u64,
}

/// The most questions Codex asks in one turn.
pub const MAX_QUESTIONS: usize = 3;
/// The most options any one question offers.
pub const MAX_OPTIONS: usize = 10;

/// Where a running turn's events go.
#[async_trait]
pub trait StreamSink: Send {
    async fn emit(&self, event: ProviderEvent);
}

/// One agent runtime.
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    fn health(&self) -> ProviderHealth;
    fn models(&self) -> Vec<Model>;
    async fn list_threads(
        &self,
        working_directory: &str,
    ) -> Result<Vec<ThreadSummary>, ProviderError>;
    async fn read_thread(&self, identity: &ThreadIdentity) -> Result<String, ProviderError>;
}
