use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{Timestamp, now};

/// A named group of threads inside one project.
///
/// FlowX's own: no provider knows about it, except that one may mirror a
/// provider's grouping (`provider_section`): a Codex section, or a Claude
/// desktop project. A thread names the space it is in
/// (`Agent::space`), so a thread is in at most one space by construction,
/// and deleting the thread takes its membership with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Space {
    pub id: Uuid,
    pub name: String,
    /// One of [`Space::COLORS`], by name. A name this build does not know is
    /// kept, so a newer build's colour survives a round trip, and shown as
    /// no colour.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "string_or_none"
    )]
    pub color: Option<String>,
    /// The provider grouping this space mirrors, by its id: a Codex
    /// section's, or a Claude desktop project's behind
    /// [`Self::CLAUDE_PROJECT`]. Both are account-wide, so each project that
    /// has threads in one gets a space of its own for it. Saved under its
    /// first name, `codex_section`, which every build so far reads.
    #[serde(
        rename = "codex_section",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "string_or_none"
    )]
    pub provider_section: Option<String>,
    pub created_at: Timestamp,
}

impl Space {
    /// The colours a space can be drawn in: the accent names, which the
    /// design system resolves. Kept here as names because `fx-core` cannot
    /// see the design system; `fx-app` checks the two lists agree.
    pub const COLORS: [&'static str; 5] = ["violet", "blue", "emerald", "orange", "rose"];

    /// The longest name a space keeps, in characters.
    pub const MAX_NAME: usize = 120;

    /// What a Claude desktop project's id is prefixed with as a mirrored
    /// section, so it can never be taken for a Codex section's.
    pub const CLAUDE_PROJECT: &'static str = "claude-project:";

    /// What the provider calls the grouping this space mirrors, for the
    /// reader; `None` for a space of the reader's own.
    pub fn mirror_kind(&self) -> Option<&'static str> {
        let section = self.provider_section.as_deref()?;
        Some(if section.starts_with(Self::CLAUDE_PROJECT) {
            "Claude project"
        } else {
            "Codex section"
        })
    }

    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            color: None,
            provider_section: None,
            created_at: now(),
        }
    }

    /// A name fit to save: trimmed, on one line, and bounded. `None` when
    /// nothing is left, which no space is called.
    pub fn clean_name(name: &str) -> Option<String> {
        let one_line = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let bounded: String = one_line.chars().take(Self::MAX_NAME).collect();
        let trimmed = bounded.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    /// The colour by a name this build draws: one of [`Self::COLORS`],
    /// ignoring case. Anything else is no colour.
    pub fn known_color(name: &str) -> Option<&'static str> {
        Self::COLORS
            .into_iter()
            .find(|known| known.eq_ignore_ascii_case(name.trim()))
    }
}

/// An optional string, and anything that is not one as absent: a colour or a
/// section id from a newer build, in a shape this one cannot read, costs that
/// value and not the space.
fn string_or_none<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_str().map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed_to_one_bounded_line_and_blank_is_none() {
        assert_eq!(
            Space::clean_name("  Auth\n  work  ").as_deref(),
            Some("Auth work")
        );
        assert_eq!(Space::clean_name(" \n\t "), None);
        let long = "x".repeat(Space::MAX_NAME + 40);
        assert_eq!(
            Space::clean_name(&long).unwrap().chars().count(),
            Space::MAX_NAME
        );
    }

    #[test]
    fn only_an_accent_name_is_a_colour() {
        assert_eq!(Space::known_color("Blue"), Some("blue"));
        assert_eq!(Space::known_color(" rose "), Some("rose"));
        assert_eq!(Space::known_color("#ff0000"), None);
        assert_eq!(Space::known_color(""), None);
    }

    #[test]
    fn a_malformed_colour_or_section_costs_only_that_value() {
        let space: Space = serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(7),
            "name": "Pinned",
            "color": {"rgb": [1, 2, 3]},
            "codex_section": 12,
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        assert_eq!(space.name, "Pinned");
        assert_eq!(space.color, None);
        assert_eq!(space.provider_section, None);
        // An unknown colour name is a string, so it is kept as written.
        let space: Space = serde_json::from_value(serde_json::json!({
            "id": Uuid::from_u128(7), "name": "P", "color": "teal",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .unwrap();
        assert_eq!(space.color.as_deref(), Some("teal"));
    }

    #[test]
    fn a_mirror_says_whose_grouping_it_is() {
        let mut space = Space::new("Pinned");
        assert_eq!(space.mirror_kind(), None);
        space.provider_section = Some("pinned".into());
        assert_eq!(space.mirror_kind(), Some("Codex section"));
        space.provider_section = Some(format!("{}chan_1", Space::CLAUDE_PROJECT));
        assert_eq!(space.mirror_kind(), Some("Claude project"));
        // Saved under the name every earlier build reads.
        let value = serde_json::to_value(&space).unwrap();
        assert_eq!(value["codex_section"], "claude-project:chan_1");
    }

    #[test]
    fn a_plain_space_writes_no_optional_fields() {
        let value = serde_json::to_value(Space::new("Plain")).unwrap();
        assert!(value.get("color").is_none());
        assert!(value.get("codex_section").is_none());
    }
}
