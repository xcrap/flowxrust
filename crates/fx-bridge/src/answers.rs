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
}
