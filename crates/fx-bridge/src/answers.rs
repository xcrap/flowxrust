//! What the tools answer, as the model reads it.
//!
//! The app gathers the facts; this shapes them, and bounds them. A model's
//! context is the scarce thing, so every answer has a ceiling: a long thread
//! list is cut, a long message is shortened, and a long excerpt drops its
//! oldest messages first and says how many.

use fx_core::{ConversationMessage, MessageRole};
use serde::Serialize;
use serde_json::json;

/// The most threads one listing names.
pub const MAX_LISTED: usize = 100;
/// The most characters one message contributes to an excerpt.
pub const MAX_MESSAGE_CHARS: usize = 4_000;
/// The most characters of messages one excerpt carries.
pub const MAX_ANSWER_CHARS: usize = 24_000;

/// What a thread is doing, in the three words the tools use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// A turn is in progress.
    Running,
    /// Blocked on the user: an approval or a question, here or in another app.
    Waiting,
    /// Not working: never started, finished, failed or stopped.
    Idle,
}

impl Status {
    pub fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Idle => "idle",
        }
    }
}

/// One thread, as `list_threads` names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadEntry {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub status: Status,
    pub archived: bool,
    /// Whether this is the thread that asked.
    pub current: bool,
    /// For a space's orchestrator only: whether the thread is in its space,
    /// so it can send to, wait for and stop it. Absent for anyone else.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_space: Option<bool>,
    /// For a space's orchestrator only: whether it started the thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawned_by_you: Option<bool>,
}

/// `list_threads`' answer: open threads first, then archived ones, each in
/// the order given, cut at [`MAX_LISTED`].
pub fn thread_list(project_name: &str, project_folder: &str, threads: &[ThreadEntry]) -> String {
    let mut ordered: Vec<&ThreadEntry> = threads.iter().filter(|t| !t.archived).collect();
    ordered.extend(threads.iter().filter(|t| t.archived));
    let shown: Vec<&ThreadEntry> = ordered.into_iter().take(MAX_LISTED).collect();
    let mut answer = json!({
        "project": { "name": project_name, "folder": project_folder },
        "threads": shown,
    });
    if threads.len() > shown.len() {
        answer["omitted"] = json!(threads.len() - shown.len());
        answer["note"] = json!(format!(
            "Only the first {MAX_LISTED} of {} threads are listed, open threads first.",
            threads.len()
        ));
    }
    serde_json::to_string_pretty(&answer).unwrap_or_default()
}

/// Who wrote a message, among the two whose prose is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    User,
    Assistant,
}

/// One message's prose, already shortened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prose {
    pub speaker: Speaker,
    pub text: String,
}

/// The prose of a transcript's latest `count` messages that have any, oldest
/// first, and how many messages in the whole transcript have prose.
///
/// Prose is what the user wrote and what the assistant said: text and code
/// blocks of user and assistant messages. Tool calls and their results,
/// thinking, compactions, images and system rows are not prose, and a
/// message with none of it is skipped rather than counted.
pub fn latest_prose<'a>(
    messages: impl DoubleEndedIterator<Item = &'a ConversationMessage>,
    count: usize,
) -> (Vec<Prose>, usize) {
    let mut taken = Vec::new();
    let mut total = 0;
    for message in messages.rev() {
        let speaker = match message.role {
            MessageRole::User => Speaker::User,
            MessageRole::Assistant => Speaker::Assistant,
            MessageRole::System | MessageRole::Tool => continue,
        };
        let mut blocks = message
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .peekable();
        if blocks.peek().is_none() {
            continue;
        }
        total += 1;
        if taken.len() < count {
            let text = blocks.collect::<Vec<_>>().join("\n");
            taken.push(Prose {
                speaker,
                text: shorten(&text, MAX_MESSAGE_CHARS),
            });
        }
    }
    taken.reverse();
    (taken, total)
}

/// `read_thread`'s answer.
///
/// `partial` is the reply being written now, if any. It is shown after the
/// rest, marked, and does not count toward the messages asked for.
pub fn thread_excerpt(
    thread: &ThreadEntry,
    prose: &[Prose],
    total: usize,
    partial: Option<&str>,
) -> String {
    let partial = partial
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| shorten(text, MAX_MESSAGE_CHARS));
    let mut budget = MAX_ANSWER_CHARS.saturating_sub(partial.as_ref().map_or(0, |p| p.len()));
    // Newest first, so a cut drops the oldest.
    let mut kept: Vec<String> = Vec::new();
    for (position, message) in prose.iter().enumerate().rev() {
        let label = match message.speaker {
            Speaker::User => "[user]",
            Speaker::Assistant => "[assistant]",
        };
        let entry = format!("{label}\n{}", message.text);
        // The newest message is always kept; it is shortened already.
        if entry.len() > budget && position + 1 != prose.len() {
            break;
        }
        budget = budget.saturating_sub(entry.len());
        kept.push(entry);
    }
    kept.reverse();
    let dropped = prose.len() - kept.len();

    let mut answer = thread_header(thread);
    answer.push('\n');
    if kept.is_empty() && partial.is_none() {
        answer.push_str("It has no messages yet.");
        return answer;
    }
    answer.push_str(&match kept.len() {
        0 => "No finished messages yet.".to_string(),
        1 if total == 1 => "Its only message:".to_string(),
        shown if shown == total => format!("All {shown} messages, oldest first:"),
        shown => format!("The last {shown} of {total} messages, oldest first:"),
    });
    if dropped > 0 {
        answer.push_str(&format!(
            " ({dropped} older message{} left out to keep this short.)",
            if dropped == 1 { " was" } else { "s were" }
        ));
    }
    for entry in &kept {
        answer.push_str("\n\n");
        answer.push_str(entry);
    }
    if let Some(partial) = partial {
        answer.push_str("\n\n[assistant, still writing]\n");
        answer.push_str(&partial);
    }
    answer
}

/// The line an excerpt opens with: which thread, and what it is doing.
pub fn thread_header(thread: &ThreadEntry) -> String {
    format!(
        "Thread \"{}\" ({}) · {} · {}{}",
        thread.title,
        thread.id,
        thread.provider,
        thread.status.word(),
        if thread.archived { " · archived" } else { "" },
    )
}

/// What a worker is doing, in the words the orchestration tools use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerStatus {
    /// A turn is in progress.
    Running,
    /// Blocked on the user: an approval or a question. Settled, for a wait:
    /// only the user can move it on.
    Waiting,
    /// Its last turn answered.
    Completed,
    /// Its last turn failed.
    Failed,
    /// Its last turn was stopped before it answered.
    Stopped,
    /// It has never had a turn.
    Idle,
}

impl WorkerStatus {
    /// Whether a wait stops waiting for it.
    pub fn is_settled(self) -> bool {
        self != Self::Running
    }
}

/// One thread, as a wait or a stop reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkerReport {
    pub thread_id: String,
    pub title: String,
    pub status: WorkerStatus,
    /// What the user has to do, while the thread is waiting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    /// The last reply to its latest prompt, shortened past
    /// [`MAX_MESSAGE_CHARS`]. `None` while that prompt has none.
    pub last_reply: Option<String>,
}

impl WorkerReport {
    /// A report with its reply shortened to what an answer carries.
    pub fn new(
        thread_id: String,
        title: String,
        status: WorkerStatus,
        waiting_for: Option<String>,
        last_reply: Option<&str>,
    ) -> Self {
        Self {
            thread_id,
            title,
            status,
            waiting_for,
            last_reply: last_reply
                .map(str::trim)
                .filter(|reply| !reply.is_empty())
                .map(|reply| shorten(reply, MAX_MESSAGE_CHARS)),
        }
    }
}

/// `start_thread`'s answer.
pub fn thread_started(
    thread_id: &str,
    title: &str,
    provider: &str,
    worktree: Option<(&str, &str)>,
) -> String {
    let answer = json!({
        "thread_id": thread_id,
        "title": title,
        "provider": provider,
        "worktree": worktree.map(|(path, branch)| json!({ "path": path, "branch": branch })),
        "status": "running",
    });
    serde_json::to_string_pretty(&answer).unwrap_or_default()
}

/// `send_to_thread`'s answer.
pub fn prompt_sent(thread_id: &str) -> String {
    serde_json::to_string_pretty(&json!({ "thread_id": thread_id, "status": "running" }))
        .unwrap_or_default()
}

/// `wait_for_threads`' answer: whether the condition held, and every
/// thread's state, in the order asked.
pub fn wait_result(
    until: crate::tools::Until,
    settled: bool,
    waited_seconds: u64,
    threads: &[WorkerReport],
) -> String {
    let mut answer = json!({
        "settled": settled,
        "until": until.word(),
        "waited_seconds": waited_seconds,
        "threads": threads,
    });
    let waiting: Vec<&str> = threads
        .iter()
        .filter(|thread| thread.status == WorkerStatus::Waiting)
        .map(|thread| thread.title.as_str())
        .collect();
    let note = if !waiting.is_empty() {
        Some(format!(
            "Waiting on the user: {}. Only the user can unblock {}; tell them what {} for.",
            waiting.join(", "),
            if waiting.len() == 1 { "it" } else { "them" },
            if waiting.len() == 1 {
                "it waits"
            } else {
                "each waits"
            },
        ))
    } else if !settled {
        Some(format!(
            "Still running after {waited_seconds} s. Call wait_for_threads again to keep waiting."
        ))
    } else {
        None
    };
    if let Some(note) = note {
        answer["note"] = json!(note);
    }
    serde_json::to_string_pretty(&answer).unwrap_or_default()
}

/// `stop_thread`'s answer: the thread as it is once the stop has settled, or
/// as it was when it was not running.
pub fn stop_result(report: &WorkerReport, was_running: bool) -> String {
    let mut answer = serde_json::to_value(report).unwrap_or_default();
    answer["was_running"] = json!(was_running);
    if !was_running {
        answer["note"] = json!("It was not running, so nothing was stopped.");
    } else if report.status == WorkerStatus::Running {
        answer["note"] = json!("It is still stopping. Call wait_for_threads to see it settle.");
    }
    serde_json::to_string_pretty(&answer).unwrap_or_default()
}

/// At most `max` characters of `text`, saying how much was cut.
fn shorten(text: &str, max: usize) -> String {
    let length = text.chars().count();
    if length <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max).collect();
    format!("{kept}… [shortened: {} more characters]", length - max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx_core::MessageContent;

    fn entry(title: &str, archived: bool) -> ThreadEntry {
        ThreadEntry {
            id: format!("id-{title}"),
            title: title.into(),
            provider: "codex".into(),
            status: Status::Idle,
            archived,
            current: false,
            in_space: None,
            spawned_by_you: None,
        }
    }

    fn message(role: MessageRole, content: Vec<MessageContent>) -> ConversationMessage {
        ConversationMessage::new(role, content)
    }

    fn text(text: &str) -> MessageContent {
        MessageContent::Text { text: text.into() }
    }

    #[test]
    fn a_listing_is_json_with_open_threads_first() {
        let threads = [
            entry("old", true),
            ThreadEntry {
                status: Status::Running,
                current: true,
                ..entry("mine", false)
            },
            entry("other", false),
        ];
        let answer: serde_json::Value =
            serde_json::from_str(&thread_list("site", "/w/site", &threads)).unwrap();
        assert_eq!(answer["project"]["name"], "site");
        assert_eq!(answer["project"]["folder"], "/w/site");
        let titles: Vec<&str> = answer["threads"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["title"].as_str().unwrap())
            .collect();
        assert_eq!(titles, ["mine", "other", "old"]);
        assert_eq!(answer["threads"][0]["status"], "running");
        assert_eq!(answer["threads"][0]["current"], true);
        assert_eq!(answer["threads"][2]["archived"], true);
        assert!(answer.get("omitted").is_none());
    }

    #[test]
    fn a_long_listing_is_cut_and_says_so() {
        let threads: Vec<ThreadEntry> = (0..MAX_LISTED + 7)
            .map(|n| entry(&n.to_string(), n % 2 == 0))
            .collect();
        let answer: serde_json::Value =
            serde_json::from_str(&thread_list("p", "/p", &threads)).unwrap();
        assert_eq!(answer["threads"].as_array().unwrap().len(), MAX_LISTED);
        assert_eq!(answer["omitted"], 7);
        // Every open thread made the cut.
        let open = threads.iter().filter(|t| !t.archived).count();
        assert!(
            answer["threads"].as_array().unwrap()[..open]
                .iter()
                .all(|t| t["archived"] == false)
        );
    }

    #[test]
    fn only_user_and_assistant_prose_is_read() {
        let messages = [
            message(MessageRole::User, vec![text("Fix the login bug")]),
            message(
                MessageRole::Assistant,
                vec![MessageContent::Thinking {
                    text: "secret plan".into(),
                }],
            ),
            message(
                MessageRole::Assistant,
                vec![MessageContent::ToolUse {
                    id: "1".into(),
                    name: "Bash".into(),
                    input: "rm -rf build".into(),
                }],
            ),
            message(
                MessageRole::Tool,
                vec![MessageContent::ToolResult {
                    id: "1".into(),
                    content: "removed".into(),
                    is_error: false,
                }],
            ),
            message(MessageRole::System, vec![text("Context compacted")]),
            message(
                MessageRole::Assistant,
                vec![text("Found it."), text("  "), text("It was the cookie.")],
            ),
        ];
        let (prose, total) = latest_prose(messages.iter(), 10);
        assert_eq!(total, 2);
        assert_eq!(
            prose,
            [
                Prose {
                    speaker: Speaker::User,
                    text: "Fix the login bug".into()
                },
                Prose {
                    speaker: Speaker::Assistant,
                    text: "Found it.\nIt was the cookie.".into()
                },
            ]
        );
    }

    #[test]
    fn the_latest_messages_are_taken_and_the_total_still_counts_them_all() {
        let messages: Vec<ConversationMessage> = (0..30)
            .map(|n| {
                let role = if n % 2 == 0 {
                    MessageRole::User
                } else {
                    MessageRole::Assistant
                };
                message(role, vec![text(&format!("message {n}"))])
            })
            .collect();
        let (prose, total) = latest_prose(messages.iter(), 3);
        assert_eq!(total, 30);
        let texts: Vec<&str> = prose.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["message 27", "message 28", "message 29"]);
    }

    #[test]
    fn an_excerpt_labels_each_message_and_marks_a_reply_in_progress() {
        let thread = ThreadEntry {
            status: Status::Running,
            ..entry("Login", false)
        };
        let prose = [
            Prose {
                speaker: Speaker::User,
                text: "Fix it".into(),
            },
            Prose {
                speaker: Speaker::Assistant,
                text: "Done".into(),
            },
        ];
        let answer = thread_excerpt(&thread, &prose, 9, Some("Now testing"));
        assert!(
            answer.starts_with("Thread \"Login\" (id-Login) · codex · running\n"),
            "{answer}"
        );
        assert!(answer.contains("The last 2 of 9 messages, oldest first:"));
        let user = answer.find("[user]\nFix it").unwrap();
        let assistant = answer.find("[assistant]\nDone").unwrap();
        let partial = answer
            .find("[assistant, still writing]\nNow testing")
            .unwrap();
        assert!(user < assistant && assistant < partial, "{answer}");
    }

    #[test]
    fn a_long_message_is_shortened_and_a_long_excerpt_drops_its_oldest() {
        let long = "é".repeat(MAX_MESSAGE_CHARS + 50);
        let messages = [message(MessageRole::Assistant, vec![text(&long)])];
        let (prose, _) = latest_prose(messages.iter(), 1);
        assert!(prose[0].text.ends_with("… [shortened: 50 more characters]"));
        assert_eq!(
            prose[0].text.chars().filter(|c| *c == 'é').count(),
            MAX_MESSAGE_CHARS
        );

        let many: Vec<Prose> = (0..20)
            .map(|n| Prose {
                speaker: Speaker::Assistant,
                text: format!("{n:02}{}", "x".repeat(MAX_MESSAGE_CHARS - 2)),
            })
            .collect();
        let answer = thread_excerpt(&entry("t", false), &many, 20, None);
        assert!(answer.len() < MAX_ANSWER_CHARS + 1_000, "{}", answer.len());
        assert!(
            answer.contains("older messages were left out"),
            "{answer:.200}"
        );
        assert!(answer.contains("\n19x"), "the newest message was dropped");
        assert!(!answer.contains("\n00x"), "the oldest message was kept");
    }

    #[test]
    fn an_empty_thread_says_so() {
        let answer = thread_excerpt(&entry("new", false), &[], 0, None);
        assert!(answer.ends_with("It has no messages yet."), "{answer}");
        let starting = thread_excerpt(&entry("new", false), &[], 0, Some("Hel"));
        assert!(starting.contains("No finished messages yet."), "{starting}");
        assert!(starting.ends_with("[assistant, still writing]\nHel"));
    }

    #[test]
    fn only_a_lead_s_listing_says_which_threads_are_in_its_space() {
        let plain: serde_json::Value =
            serde_json::from_str(&thread_list("p", "/p", &[entry("a", false)])).unwrap();
        let thread = &plain["threads"][0];
        assert!(thread.get("in_space").is_none() && thread.get("spawned_by_you").is_none());
        let lead = ThreadEntry {
            in_space: Some(true),
            spawned_by_you: Some(false),
            ..entry("a", false)
        };
        let listed: serde_json::Value =
            serde_json::from_str(&thread_list("p", "/p", &[lead])).unwrap();
        assert_eq!(listed["threads"][0]["in_space"], true);
        assert_eq!(listed["threads"][0]["spawned_by_you"], false);
    }

    fn report(status: WorkerStatus, reply: Option<&str>) -> WorkerReport {
        WorkerReport::new("id".into(), "Parser".into(), status, None, reply)
    }

    #[test]
    fn a_wait_reports_each_thread_and_caps_its_reply_with_a_note() {
        // The model's context is the scarce thing: four workers' essays
        // would crowd out the plan they report to.
        let long = "x".repeat(MAX_MESSAGE_CHARS + 25);
        let answer: serde_json::Value = serde_json::from_str(&wait_result(
            crate::tools::Until::All,
            true,
            12,
            &[
                report(WorkerStatus::Completed, Some(&long)),
                report(WorkerStatus::Failed, None),
            ],
        ))
        .unwrap();
        assert_eq!(answer["settled"], true);
        assert_eq!(answer["until"], "all");
        assert_eq!(answer["waited_seconds"], 12);
        let first = &answer["threads"][0];
        assert_eq!(first["status"], "completed");
        let reply = first["last_reply"].as_str().unwrap();
        assert!(
            reply.ends_with("… [shortened: 25 more characters]"),
            "{reply}"
        );
        assert!(first.get("waiting_for").is_none());
        assert_eq!(answer["threads"][1]["status"], "failed");
        assert_eq!(answer["threads"][1]["last_reply"], serde_json::Value::Null);
        assert!(answer.get("note").is_none());
    }

    #[test]
    fn a_wait_says_who_waits_on_the_user_and_when_it_ran_out_of_time() {
        let waiting = WorkerReport::new(
            "id".into(),
            "Migrations".into(),
            WorkerStatus::Waiting,
            Some("approval to run Bash".into()),
            None,
        );
        assert!(WorkerStatus::Waiting.is_settled());
        assert!(!WorkerStatus::Running.is_settled());
        let answer: serde_json::Value = serde_json::from_str(&wait_result(
            crate::tools::Until::Any,
            true,
            0,
            &[waiting, report(WorkerStatus::Running, None)],
        ))
        .unwrap();
        assert_eq!(answer["threads"][0]["waiting_for"], "approval to run Bash");
        let note = answer["note"].as_str().unwrap();
        assert!(
            note.contains("Migrations") && note.contains("tell them"),
            "{note}"
        );
        let timed_out: serde_json::Value = serde_json::from_str(&wait_result(
            crate::tools::Until::All,
            false,
            240,
            &[report(WorkerStatus::Running, Some("Half way."))],
        ))
        .unwrap();
        assert_eq!(timed_out["settled"], false);
        assert!(
            timed_out["note"]
                .as_str()
                .unwrap()
                .contains("Call wait_for_threads again")
        );
    }

    #[test]
    fn a_start_names_its_worktree_or_null() {
        let started: serde_json::Value = serde_json::from_str(&thread_started(
            "id",
            "Parser",
            "claude",
            Some(("/w/x", "flowx/abc")),
        ))
        .unwrap();
        assert_eq!(started["status"], "running");
        assert_eq!(started["worktree"]["branch"], "flowx/abc");
        let plain: serde_json::Value =
            serde_json::from_str(&thread_started("id", "Parser", "codex", None)).unwrap();
        assert_eq!(plain["worktree"], serde_json::Value::Null);
        assert_eq!(plain["provider"], "codex");
    }
}
