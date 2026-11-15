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
/// So would one with an `@` in it (`packages/@scope/x.ts`), which
/// [`mentioned_paths`] ends a mention at, as the CLIs' own pattern does.
pub fn is_mentionable(path: &str) -> bool {
    !path.is_empty() && !path.chars().any(|c| c.is_whitespace() || c == '@')
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
    Query::new(query).score(&fold(path))
}

/// A path as matching reads it: lowercased once, character by character.
/// Lowercasing can grow a string (`İ` is two characters lowercased), and a
/// path where it did is matched as written instead, or the positions scored
/// below would not line up with the path's own.
fn fold(path: &str) -> String {
    if path.is_ascii() {
        return path.to_ascii_lowercase();
    }
    let lowered: String = path.chars().flat_map(char::to_lowercase).collect();
    if lowered.chars().count() == path.chars().count() {
        lowered
    } else {
        path.to_string()
    }
}

/// What was typed, lowercased once per ranking rather than once per path.
struct Query {
    chars: Vec<char>,
    text: String,
}

impl Query {
    fn new(query: &str) -> Self {
        let chars: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
        let text = chars.iter().collect();
        Self { chars, text }
    }

    /// [`match_score`] over a path already [`fold`]ed. Allocates nothing: it
    /// runs for every listed file on every keystroke.
    fn score(&self, folded: &str) -> Option<u32> {
        if self.chars.is_empty() {
            return Some(empty_query_score(folded));
        }
        let name_byte = folded.rfind('/').map_or(0, |slash| slash + 1);
        let name_start = folded[..name_byte].chars().count();

        let mut score: i64 = 100;
        let mut chars = folded.chars().enumerate();
        // The character before the one being read, for word boundaries.
        let mut before: Option<char> = None;
        let mut previous: Option<usize> = None;
        for &wanted in &self.chars {
            let (found, preceding) = loop {
                let (index, c) = chars.next()?;
                let preceding = before.replace(c);
                if c == wanted {
                    break (index, preceding);
                }
            };
            let boundary = preceding.is_none_or(|c| matches!(c, '/' | '_' | '-' | '.' | ' '));
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
        }

        let name = &folded[name_byte..];
        if name == self.text {
            score += 60;
        } else if name.starts_with(&self.text) {
            score += 30;
        } else if name.contains(&self.text) {
            score += 15;
        }
        let length = if folded.is_ascii() {
            folded.len()
        } else {
            folded.chars().count()
        };
        score -= (length as i64 / 8).min(20);
        Some(score.max(1) as u32)
    }
}

/// With nothing typed, the files nearest the root come first: what a reader
/// is likeliest to mean before they have narrowed anything down.
fn empty_query_score(path: &str) -> u32 {
    let depth = path.matches('/').count() as u32;
    1_000u32.saturating_sub(depth * 50)
}

/// A folder's files, ready to rank for an `@` on every keystroke.
///
/// Each path is folded for matching once, when the listing is built. Ranked
/// as plain strings, every keystroke lowercased each of a large checkout's
/// 50,000 paths with five allocations apiece, then sorted every match with
/// two more per comparison: 30–58 ms a keystroke on the UI thread, in release.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MentionPaths {
    paths: Vec<String>,
    folded: Vec<String>,
}

impl MentionPaths {
    pub fn new(paths: Vec<String>) -> Self {
        let folded = paths.iter().map(|path| fold(path)).collect();
        Self { paths, folded }
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.paths.get(index).map(String::as_str)
    }

    /// The paths a query ranks, best first and at most `limit`, as positions
    /// in [`Self::paths`]. Equal scores fall back to the shorter path, then to
    /// name order, so the list is stable across repaints; with nothing typed,
    /// name order alone, so the root's files read as a directory listing.
    ///
    /// Only the best `limit` are put in order. The picker shows a handful of
    /// rows, and sorting every match of a one-letter query sorted the listing.
    pub fn rank(&self, query: &str, limit: usize) -> Vec<usize> {
        if limit == 0 {
            return Vec::new();
        }
        let query = Query::new(query);
        let mut scored: Vec<(u32, usize)> = self
            .folded
            .iter()
            .enumerate()
            .filter_map(|(index, folded)| query.score(folded).map(|score| (score, index)))
            .collect();
        let by_length = !query.chars.is_empty();
        let order = |a: &(u32, usize), b: &(u32, usize)| {
            b.0.cmp(&a.0)
                .then_with(|| {
                    if by_length {
                        self.paths[a.1].len().cmp(&self.paths[b.1].len())
                    } else {
                        Ordering::Equal
                    }
                })
                .then_with(|| self.folded[a.1].cmp(&self.folded[b.1]))
                .then_with(|| self.paths[a.1].cmp(&self.paths[b.1]))
        };
        if scored.len() > limit {
            scored.select_nth_unstable_by(limit - 1, order);
            scored.truncate(limit);
        }
        scored.sort_unstable_by(order);
        scored.into_iter().map(|(_, index)| index).collect()
    }
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
    fn a_path_with_an_at_sign_cannot_be_mentioned() {
        // A mention is read up to the next `@`, so Codex would be handed
        // `packages/` for a scoped package's file.
        let path = "packages/@scope/x.ts";
        assert_ne!(mentioned_paths(&mention_text(path)), [path]);
        assert!(!is_mentionable(path));
        assert!(!is_mentionable("@types/index.d.ts"));
        assert!(is_mentionable("packages/scope/x.ts"));
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

    fn listing(paths: &[&str]) -> MentionPaths {
        MentionPaths::new(paths.iter().map(|p| p.to_string()).collect())
    }

    fn ranked<'a>(query: &str, paths: &'a [&str]) -> Vec<&'a str> {
        listing(paths)
            .rank(query, 10)
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
        let listed = listing(&paths);
        assert_eq!(listed.rank("", 2).len(), 2);
        assert_eq!(listed.rank("zzz", 2), Vec::<usize>::new());
        assert_eq!(listed.rank("rs", 0), Vec::<usize>::new());
    }

    #[test]
    fn keeping_only_the_best_rows_keeps_the_order_a_full_sort_gives() {
        // Many equal scores, so the tie-breaks decide which rows are kept.
        let paths: Vec<String> = (0..500)
            .map(|i| {
                format!(
                    "{}/{}/File{}.rs",
                    ["src", "Src", "lib"][i % 3],
                    i % 7,
                    i % 50
                )
            })
            .collect();
        let listed = MentionPaths::new(paths);
        for query in ["", "f", "file1", "src/f", "s"] {
            let all = listed.rank(query, usize::MAX);
            for limit in [1, 15, 100] {
                assert_eq!(
                    listed.rank(query, limit),
                    all[..limit.min(all.len())],
                    "{query:?} at {limit}"
                );
            }
        }
    }

    #[test]
    fn a_keystroke_ranks_a_large_listing_in_milliseconds() {
        // The picker re-ranks on every keystroke, on the UI thread, over a
        // listing of up to 50,000 paths. Before paths were folded once and
        // only the shown rows sorted, this took 160–230 ms in a debug build.
        let listed = MentionPaths::new(
            (0..50_000)
                .map(|i| format!("Crates/Module{}/src/Sub_{}/File{i}.rs", i % 97, i % 13))
                .collect(),
        );
        for query in ["", "c", "cr", "file4", "sub_3/file"] {
            let started = std::time::Instant::now();
            let rows = listed.rank(query, 15);
            let elapsed = started.elapsed();
            assert_eq!(rows.len(), 15, "{query:?}");
            assert!(
                elapsed < std::time::Duration::from_millis(60),
                "{query:?} took {elapsed:?} over 50,000 paths"
            );
        }
    }

    #[test]
    fn a_non_ascii_path_is_matched_without_shifting_its_positions() {
        // `İ` lowercases to two characters; the path is then matched as typed
        // rather than against positions that no longer line up.
        assert!(match_score("i", "İstanbul/İ.rs").is_none());
        assert!(match_score("stanbul", "İstanbul/x.rs").is_some());
        assert!(match_score("é", "café.rs").is_some());
        // Folded once, a mixed-case non-ASCII path still matches either case.
        let listed = listing(&["Docs/Ébauche.md", "İstanbul/x.rs"]);
        assert_eq!(listed.rank("éba", 5), [0]);
        assert_eq!(listed.rank("ÉBA", 5), [0]);
        assert_eq!(listed.rank("stanbul", 5), [1]);
    }
}
