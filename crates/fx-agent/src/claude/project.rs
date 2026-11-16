//! What a Claude desktop project writes into the threads it starts.
//!
//! A Claude desktop project is a chat on claude.ai whose coordinator, another
//! Claude session, starts Claude Code threads on the Mac and relays the
//! reader's words to them. Only those threads are on disk; the chat is not.
//! The thread's transcript carries the project's traffic as user records in
//! three shapes, read here from Claude Code 2.1.325's own files:
//!
//! - `<relay from="coordinator" reason="spawn">`, meta: the brief the
//!   coordinator started the thread with, in a `<note>`.
//! - `<wake reason="mention">` (or `message-edited`), the record that starts
//!   a turn: the reader's own message from the project, verbatim, in a
//!   `<message from="human">` inside `<project id="chan_…">`, with a
//!   `<system-note>` for the model that nobody wrote to the thread.
//! - `<project_claude_message>`, recorded as a task notification (`origin`
//!   `{kind: task-notification, subkind: projects-relay}`): a `<relay>`
//!   quoting the reader's messages as `<cited author="user">` and adding the
//!   coordinator's `<note>`.
//!
//! Read as plain prompts, the first was hidden as meta, the third as a task
//! notification, and the second was the thread's prompt and title as raw
//! markup. What the reader wrote is their prompt, and what the coordinator
//! wrote is its note: another session wrote that, not the reader.

/// What one project record says, split by who wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Envelope {
    /// The reader's own words, every message the record carries, in order.
    pub prompt: Option<String>,
    /// What the project's coordinator wrote to the thread.
    pub note: Option<String>,
    /// The project, by Claude's id for it, where the record names one.
    pub project: Option<String>,
}

/// The record's text read as a project's, or `None` when it is not one.
pub(crate) fn read(text: &str) -> Option<Envelope> {
    let text = text.trim();
    if let Some((_, body)) = whole(text, "wake") {
        let project = element(body, "project").and_then(|(attributes, _)| {
            attribute(attributes, "id").filter(|id| !id.trim().is_empty())
        });
        let prompt = joined(
            elements(body, "message")
                .into_iter()
                .filter(|(attributes, _)| attribute(attributes, "from").as_deref() == Some("human"))
                .map(|(_, body)| body),
        );
        // A wake that carries none of the reader's words is not one this
        // build knows; shown as written, nothing it says is lost.
        return prompt.map(|prompt| Envelope {
            prompt: Some(prompt),
            note: None,
            project,
        });
    }
    let relay = whole(text, "project_claude_message")
        .map(|(_, body)| body.trim())
        .unwrap_or(text);
    let (attributes, body) = whole(relay, "relay")?;
    if attribute(attributes, "from").as_deref() != Some("coordinator") {
        return None;
    }
    let prompt = joined(
        elements(body, "cited")
            .into_iter()
            .filter(|(attributes, _)| attribute(attributes, "author").as_deref() == Some("user"))
            .map(|(_, body)| body),
    );
    let note = element(body, "note").and_then(|(_, body)| block(body));
    (prompt.is_some() || note.is_some()).then_some(Envelope {
        prompt,
        note,
        project: None,
    })
}

/// The bodies' text, each as a block, joined by a blank line.
fn joined<'a>(bodies: impl Iterator<Item = &'a str>) -> Option<String> {
    let blocks: Vec<String> = bodies.filter_map(block).collect();
    (!blocks.is_empty()).then(|| blocks.join("\n\n"))
}

/// `text` when it is exactly one `<name …>…</name>` element: its attributes
/// and its body.
fn whole<'a>(text: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    let close = format!("</{name}>");
    if !text.ends_with(&close) {
        return None;
    }
    let (attributes, body, start) = open_tag(text, 0, name)?;
    if start != 0 {
        return None;
    }
    Some((attributes, &body[..body.len().checked_sub(close.len())?]))
}

/// The first `<name …>…</name>` in `text`.
fn element<'a>(text: &'a str, name: &str) -> Option<(&'a str, &'a str)> {
    elements(text, name).into_iter().next()
}

/// Every `<name …>…</name>` in `text`, in order, none nested in another.
fn elements<'a>(text: &'a str, name: &str) -> Vec<(&'a str, &'a str)> {
    let close = format!("</{name}>");
    let mut found = Vec::new();
    let mut from = 0;
    while let Some((attributes, rest, _)) = open_tag(text, from, name) {
        let Some(length) = rest.find(&close) else {
            break;
        };
        found.push((attributes, &rest[..length]));
        // `rest` is a suffix of `text`, so its offset is the difference.
        from = text.len() - rest.len() + length + close.len();
    }
    found
}

/// The first `<name` tag at or after `from`: its attributes, everything
/// after its `>`, and where it starts.
fn open_tag<'a>(text: &'a str, from: usize, name: &str) -> Option<(&'a str, &'a str, usize)> {
    let opening = format!("<{name}");
    let mut at = from;
    loop {
        let start = at + text.get(at..)?.find(&opening)?;
        let after = start + opening.len();
        // `<message` is not `<messages`: the name ends at a space or `>`.
        match text[after..].chars().next() {
            Some(next) if next == '>' || next.is_whitespace() => {
                let end = after + text[after..].find('>')?;
                return Some((&text[after..end], &text[end + 1..], start));
            }
            Some(_) => at = after,
            None => return None,
        }
    }
}

/// One attribute's value, unescaped. Names are matched whole, so `id` is not
/// `author-id`.
fn attribute(attributes: &str, name: &str) -> Option<String> {
    let mut rest = attributes;
    loop {
        rest = rest.trim_start();
        let equals = rest.find('=')?;
        let key = rest[..equals].trim();
        let value = rest[equals + 1..].strip_prefix('"')?;
        let quote = value.find('"')?;
        if key == name {
            return Some(unescape(&value[..quote]));
        }
        rest = &value[quote + 1..];
    }
}

/// An element's body as the text it holds: unescaped, and, when it is laid
/// out as a block (opening on a new line, as Claude indents a cited message
/// or a note), without that indentation. A body that opens inline is the
/// person's text as typed, and keeps its own.
fn block(body: &str) -> Option<String> {
    let text = unescape(body);
    let text = if text.trim_start_matches([' ', '\t']).starts_with('\n') {
        let lines: Vec<&str> = text.lines().collect();
        let indent = lines
            .iter()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(0);
        lines
            .iter()
            .map(|line| line.get(indent..).unwrap_or_else(|| line.trim_start()))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text
    };
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// XML's five entities and numeric references; anything else as written.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find(';').filter(|end| *end <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shapes as Claude Code 2.1.325 recorded them in a project's thread,
    // with ids shortened.
    const SPAWN: &str = r#"<relay from="coordinator" session="session_01F8" current-time="2026-11-16T09:22:48Z" reason="spawn">
  The note below was written by the coordinator session, a Claude session, not by your user.
  <note>
      Continue the phased JS→TS migration in /Users/me/game.
      Work directly on main &amp; report percentages.
  </note>
</relay>"#;

    const WAKE: &str = r#"<wake reason="mention" current-time="2026-11-16T09:22:46Z">
  <project id="chan_012r" type="project">
    <message trigger="true" from="human" trust="principal" author-id="user_01F8" id="cmsg_012r42" sent-at="2026-11-16T09:22:30Z" mention="true">Keep going. I need to know the status, how much percentage already, but keep going.</message>
  </project>
</wake>
"#;

    const THREADED_WAKE: &str = r#"<wake reason="mention" current-time="2026-11-16T09:51:22Z">
  <project id="chan_012r" type="project">
    <thread ts="cmsg_012rPM">
      <message trigger="true" from="human" trust="principal" author-id="user_01F8" id="cmsg_012rAh" sent-at="2026-11-16T09:51:22Z" mention="true">continue…

  with the &lt;b&gt; tags kept</message>
    </thread>
  </project>
  <system-note>If one quick read gets you the answer, just answer.</system-note>
</wake>"#;

    const RELAYED: &str = r#"<project_claude_message session="session_01F8" thread_id="cmsg_012rPM">
<relay from="coordinator" session="session_01F8" current-time="2026-11-16T02:45:13Z">
  The messages below are picked by the coordinator session and not necessarily consecutive. User messages are copied verbatim by the server.
  <cited id="cmsg_012rAt" author="user" name="XCrap" at="2026-11-16T02:44:59Z" where="timeline">
      Yes
  </cited>
  <cited id="cmsg_012r4E" author="user" name="XCrap" at="2026-11-16T02:45:08Z" where="timeline">
      Move with whats possible
  </cited>
  The note below was written by the coordinator session, a Claude session, not by your user.
  <note>
      The user wants the next migration batch now.
  </note>
</relay>
</project_claude_message>
"#;

    #[test]
    fn a_spawn_relay_is_the_coordinator_s_brief() {
        assert_eq!(
            read(SPAWN),
            Some(Envelope {
                prompt: None,
                note: Some(
                    "Continue the phased JS→TS migration in /Users/me/game.\n\
                     Work directly on main & report percentages."
                        .into()
                ),
                project: None,
            })
        );
    }

    #[test]
    fn a_wake_is_the_reader_s_message_and_names_its_project() {
        assert_eq!(
            read(WAKE),
            Some(Envelope {
                prompt: Some(
                    "Keep going. I need to know the status, how much percentage already, \
                     but keep going."
                        .into()
                ),
                note: None,
                project: Some("chan_012r".into()),
            })
        );
        // Inside a thread, with the model's system note left out and the
        // reader's own line breaks and markup kept.
        let threaded = read(THREADED_WAKE).unwrap();
        assert_eq!(
            threaded.prompt.as_deref(),
            Some("continue…\n\n  with the <b> tags kept")
        );
        assert_eq!(threaded.project.as_deref(), Some("chan_012r"));
    }

    #[test]
    fn a_relayed_message_is_the_reader_s_words_and_the_coordinator_s_note() {
        assert_eq!(
            read(RELAYED),
            Some(Envelope {
                prompt: Some("Yes\n\nMove with whats possible".into()),
                note: Some("The user wants the next migration batch now.".into()),
                project: None,
            })
        );
    }

    #[test]
    fn anything_else_is_not_a_project_s() {
        for text in [
            "Fix the build",
            "<wake reason=\"mention\"><project id=\"c\"></project></wake>",
            "<relay from=\"someone\" reason=\"spawn\"><note>hi</note></relay>",
            "<relay from=\"coordinator\">nothing quoted</relay>",
            "<task-notification><status>completed</status></task-notification>",
            "<wake reason=\"mention\">unclosed",
            "Read <wake reason=\"x\"><message from=\"human\">a</message></wake>",
        ] {
            assert_eq!(read(text), None, "{text}");
        }
    }

    #[test]
    fn attributes_are_matched_by_their_whole_name() {
        let attributes = r#" author-id="user_1" id="cmsg_2" from="human""#;
        assert_eq!(attribute(attributes, "id").as_deref(), Some("cmsg_2"));
        assert_eq!(attribute(attributes, "from").as_deref(), Some("human"));
        assert_eq!(attribute(attributes, "trust"), None);
        // `<messages>` is not `<message>`.
        assert!(elements("<messages>a</messages>", "message").is_empty());
    }

    #[test]
    fn entities_are_decoded_and_a_bare_ampersand_kept() {
        assert_eq!(
            unescape("a &amp; b &lt;c&gt; &#233;&#x41; &nope; & done"),
            "a & b <c> éA &nope; & done"
        );
    }
}
