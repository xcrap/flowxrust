//! Where a prompt mentions a file with `@`, and which files it most likely
//! means.
//!
//! Both CLIs read an `@path` in a prompt as a file to attach. Claude Code
//! expands it before the model sees the prompt, in print mode and through
//! stream-json input alike (checked live against 2.1.283: the model named the
//! file's first heading with no tool call). Codex takes the same path as a
//! `mention` input item beside the text. Neither reads a mention past
//! whitespace, so a path with a space in it cannot be mentioned and is not
//! offered. The composer's job is to find the token being typed and rank the
//! project's files for it; this module is the pure half of that.

use std::cmp::Ordering;
use std::ops::Range;

/// An `@` the reader is typing a file name after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionToken {
    /// The token in the draft, from its `@` to the caret, in bytes.
    pub range: Range<usize>,
    /// What was typed after the `@`.
    pub query: String,
}

/// The characters an `@` may follow and still open a mention. An `@` after a
/// letter is an email address or a scoped package name, not a mention; one
/// after another `@` is read from the second, as the CLIs' own pattern does.
const OPENERS: [char; 7] = ['(', '[', '{', '<', '"', '\'', '@'];

/// The mention being typed at the caret, if there is one.
///
/// The token runs from an `@` that starts the draft or follows whitespace or
/// an opening bracket, to the caret, with no whitespace in between; and the
/// caret has to be at the token's end, so a caret moved back into the middle
/// of a finished mention does not reopen the picker over it.
pub fn mention_at(text: &str, caret: usize) -> Option<MentionToken> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let before = &text[..caret];
    let at = before.rfind('@')?;
    let query = &before[at + 1..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    let preceding = before[..at].chars().next_back();
    if !preceding.is_none_or(|c| c.is_whitespace() || OPENERS.contains(&c)) {
        return None;
    }
    if text[caret..]
        .chars()
        .next()
        .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    Some(MentionToken {
        range: at..caret,
        query: query.to_string(),
    })
}

/// Whether a path can be mentioned at all: both CLIs read a mention up to the
/// next whitespace, so a path with any in it would reach the model cut short.
pub fn is_mentionable(path: &str) -> bool {
    !path.is_empty() && !path.chars().any(char::is_whitespace)
}

/// What a chosen file becomes in the draft: `@path` and a space, so typing
/// goes on after it rather than into it.
pub fn mention_text(path: &str) -> String {
    format!("@{path} ")
}

/// The `@path` tokens in a prompt, in order, each once. Read the way the
/// CLIs read them: an `@` that starts the text or follows whitespace or an
/// opening bracket, then everything up to the next whitespace or `@`.
pub fn mentioned_paths(text: &str) -> Vec<&str> {
    let mut paths: Vec<&str> = Vec::new();
    let mut preceding: Option<char> = None;
    let mut rest = text;
    let mut offset = 0;
    while let Some(at) = rest.find('@') {
        let opens = preceding
            .filter(|_| at == 0)
            .or_else(|| rest[..at].chars().next_back())
            .is_none_or(|c| c.is_whitespace() || OPENERS.contains(&c));
        let after = &rest[at + 1..];
        let end = after
            .find(|c: char| c.is_whitespace() || c == '@')
            .unwrap_or(after.len());
        let path = &after[..end];
        if opens && !path.is_empty() && !paths.contains(&path) {
            paths.push(path);
        }
        preceding = Some('@');
        let advance = at + 1;
        offset += advance;
        rest = &text[offset..];
    }
    paths
}

/// How well a query matches a path, higher is better; `None` when the
/// query's characters do not all appear in the path in order. Case does not
/// count. A match that starts a path segment or a word in it, one inside the
/// file's own name, and a run of adjacent matches each score more; characters
/// skipped between matches and a long path each cost a little, so `comp`
/// finds `composer.rs` before `src/components/mod.rs`.
pub fn match_score(query: &str, path: &str) -> Option<u32> {
    let query: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if query.is_empty() {
        return Some(empty_query_score(path));
    }
    let path_chars: Vec<char> = path.chars().collect();
    let lowered: Vec<char> = path_chars.iter().flat_map(|c| c.to_lowercase()).collect();
    // Lowercasing can grow a string; score on it only when it did not, which
    // is every ASCII path. Otherwise the positions below would not line up.
    let lowered = if lowered.len() == path_chars.len() {
        lowered
    } else {
        path_chars.clone()
    };
    let name_start = path_chars
        .iter()
        .rposition(|&c| c == '/')
        .map_or(0, |slash| slash + 1);

    let mut score: i64 = 100;
    let mut position = 0usize;
    let mut previous: Option<usize> = None;
    for &wanted in &query {
        let found = lowered[position..]
            .iter()
            .position(|&c| c == wanted)
            .map(|offset| position + offset)?;
        let boundary = found == 0 || matches!(lowered[found - 1], '/' | '_' | '-' | '.' | ' ');
        if boundary {
            score += 10;
        }
        if found == name_start {
            score += 8;
        }
        if found >= name_start {
            score += 3;
        }
        if previous.is_some_and(|previous| previous + 1 == found) {
            score += 6;
        }
        if let Some(previous) = previous {
            let gap = found - previous - 1;
            score -= (gap as i64).min(10);
        } else {
            score -= (found as i64 / 4).min(10);
        }
        previous = Some(found);
        position = found + 1;
    }

    let name: String = lowered[name_start..].iter().collect();
    let typed: String = query.iter().collect();
    if name == typed {
        score += 60;
    } else if name.starts_with(&typed) {
        score += 30;
    } else if name.contains(&typed) {
        score += 15;
    }
    score -= (path_chars.len() as i64 / 8).min(20);
    Some(score.max(1) as u32)
}

/// With nothing typed, the files nearest the root come first: what a reader
/// is likeliest to mean before they have narrowed anything down.
fn empty_query_score(path: &str) -> u32 {
    let depth = path.matches('/').count() as u32;
    1_000u32.saturating_sub(depth * 50)
}

/// The paths a query ranks, best first and at most `limit`, as positions in
/// `paths`. Equal scores fall back to the shorter path, then to name order,
/// so the list is stable across repaints; with nothing typed, name order
/// alone, so the root's files read as a directory listing.
pub fn rank(query: &str, paths: &[String], limit: usize) -> Vec<usize> {
    let mut scored: Vec<(u32, usize)> = paths
        .iter()
        .enumerate()
        .filter_map(|(index, path)| match_score(query, path).map(|score| (score, index)))
        .collect();
    let by_length = !query.is_empty();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| {
                if by_length {
                    paths[a.1].len().cmp(&paths[b.1].len())
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| compare_paths(&paths[a.1], &paths[b.1]))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(_, index)| index).collect()
}

fn compare_paths(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str, caret: usize) -> Option<(Range<usize>, String)> {
        mention_at(text, caret).map(|token| (token.range, token.query))
    }

    #[test]
    fn a_mention_opens_at_an_at_that_starts_a_word() {
        assert_eq!(token("@", 1), Some((0..1, String::new())));
        assert_eq!(token("Fix @comp", 9), Some((4..9, "comp".into())));
        assert_eq!(
            token("See (@src/lib.rs", 16),
            Some((5..16, "src/lib.rs".into()))
        );
        assert_eq!(token("line one\n@", 10), Some((9..10, String::new())));
        // An `@` inside a word is an address or a package, not a mention.
        assert_eq!(token("mail me@example", 15), None);
        assert_eq!(
            token("npm i @types/node", 17).map(|t| t.1),
            Some("types/node".into())
        );
        assert_eq!(token("use react@18", 12), None);
    }

    #[test]
    fn a_mention_is_only_the_token_the_caret_ends() {
        // Whitespace ends the token; the caret past it is not in a mention.
        assert_eq!(token("@comp done", 10), None);
        // A caret inside a finished mention does not reopen it.
        assert_eq!(token("@composer.rs", 5), None);
        // …but at its very end it does, which is how editing the tail works.
        assert_eq!(
            token("@composer.rs", 12).map(|t| t.1),
            Some("composer.rs".into())
        );
        // A caret between two tokens stays with the one before it.
        assert_eq!(token("@a @b", 2).map(|t| t.1), Some("a".into()));
        // Offsets past the end or inside a character are never a mention.
        assert_eq!(token("@é", 2), None);
        assert_eq!(token("@é", 9), None);
    }

    #[test]
    fn a_path_with_whitespace_cannot_be_mentioned() {
        assert!(is_mentionable("src/lib.rs"));
        assert!(!is_mentionable("My Notes/todo.md"));
        assert!(!is_mentionable(""));
        assert_eq!(mention_text("src/lib.rs"), "@src/lib.rs ");
    }

    #[test]
    fn the_mentioned_paths_are_read_the_way_the_clis_read_them() {
        assert_eq!(
            mentioned_paths("Compare @src/a.rs with @src/b.rs, then @src/a.rs again"),
            ["src/a.rs", "src/b.rs,"]
        );
        assert_eq!(mentioned_paths("@README.md"), ["README.md"]);
        assert_eq!(mentioned_paths("(@docs/x.md)"), ["docs/x.md)"]);
        assert_eq!(
            mentioned_paths("mail me@example.com and npm@9"),
            Vec::<&str>::new()
        );
        assert_eq!(mentioned_paths("@ alone and @@double"), ["double"]);
        assert_eq!(mentioned_paths("line\n@a.rs\n@b.rs"), ["a.rs", "b.rs"]);
    }

    fn ranked<'a>(query: &str, paths: &'a [&str]) -> Vec<&'a str> {
        let owned: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        rank(query, &owned, 10)
            .into_iter()
            .map(|index| paths[index])
            .collect()
    }

    #[test]
    fn a_query_matches_its_characters_in_order_whatever_the_case() {
        assert!(match_score("comp", "crates/fx-app/src/composer.rs").is_some());
        assert!(match_score("CMP", "composer.rs").is_some());
        assert!(match_score("cpm", "composer.rs").is_none(), "out of order");
        assert!(match_score("zz", "composer.rs").is_none());
        assert_eq!(match_score("", "a.rs"), Some(empty_query_score("a.rs")));
    }

    #[test]
    fn the_file_named_wins_over_a_folder_that_merely_contains_the_letters() {
        let paths = [
            "crates/fx-design/src/components/mod.rs",
            "crates/fx-app/src/composer.rs",
            "crates/fx-app/src/command_palette.rs",
            "Cargo.toml",
        ];
        assert_eq!(ranked("comp", &paths)[0], "crates/fx-app/src/composer.rs");
        assert_eq!(
            ranked("composer", &paths)[0],
            "crates/fx-app/src/composer.rs"
        );
        // A segment the query spells wins over scattered letters.
        assert_eq!(
            ranked("components", &paths)[0],
            "crates/fx-design/src/components/mod.rs"
        );
        // A path query follows the slashes.
        assert_eq!(
            ranked("design/mod", &paths),
            ["crates/fx-design/src/components/mod.rs"]
        );
        // An exact file name beats a prefix match on a longer one.
        let paths = ["src/lib.rs", "src/library.rs", "src/liberty/mod.rs"];
        assert_eq!(ranked("lib.rs", &paths)[0], "src/lib.rs");
        assert_eq!(ranked("lib", &paths)[0], "src/lib.rs");
    }

    #[test]
    fn nothing_typed_lists_the_root_first_in_name_order() {
        let paths = [
            "src/deep/nested/file.rs",
            "src/main.rs",
            "README.md",
            "Cargo.toml",
            "cargo.lock",
        ];
        assert_eq!(
            ranked("", &paths),
            [
                "cargo.lock",
                "Cargo.toml",
                "README.md",
                "src/main.rs",
                "src/deep/nested/file.rs"
            ]
        );
    }

    #[test]
    fn ranking_is_stable_and_bounded() {
        let paths = ["b.rs", "a.rs", "c.rs", "aa.rs"];
        assert_eq!(ranked("", &paths), ["a.rs", "aa.rs", "b.rs", "c.rs"]);
        // With a query, the shorter of two equal matches comes first.
        assert_eq!(ranked("rs", &["ab.rs", "a.rs"]), ["a.rs", "ab.rs"]);
        let owned: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        assert_eq!(rank("", &owned, 2).len(), 2);
        assert_eq!(rank("zzz", &owned, 2), Vec::<usize>::new());
    }

    #[test]
    fn a_non_ascii_path_is_matched_without_shifting_its_positions() {
        // `İ` lowercases to two characters; the path is then matched as typed
        // rather than against positions that no longer line up.
        assert!(match_score("i", "İstanbul/İ.rs").is_none());
        assert!(match_score("stanbul", "İstanbul/x.rs").is_some());
        assert!(match_score("é", "café.rs").is_some());
    }
}
