//! What an edit tool is about to do to a file, as diff lines.
//!
//! An agent's edit arrives as a tool call long before the inspector's poll sees
//! the file change, and a `Write` of a new file never shows there as a diff at
//! all. Both providers say what the edit is in the call's arguments: Claude's
//! `Edit` carries the old and new text, its `Write` the whole file, and Codex's
//! `fileChange` a unified diff per file. This reads each into the diff lines
//! the viewer already draws, so the transcript can show the change where the
//! call is.
//!
//! Pure: arguments in, lines out. Nothing here reads a file, so a preview is
//! only ever what the tool *said*, which for a replace-all edit or one the tool
//! then refused may differ from what happened. The call's result says that.

use serde_json::Value;

use crate::git::diff::{DiffLine, DiffLineKind, parse_section};

/// The most lines a preview shows per section. A `Write` of a generated file
/// is not the whole file.
pub const MAX_PREVIEW_LINES: usize = 200;

/// The most lines either side of an edit may have for a line diff to be
/// computed between them. Past it the old text is shown removed and the new
/// added, which is right more often than a diff of two large unrelated blocks.
pub const MAX_DIFFED_LINES: usize = 400;

/// Unchanged lines kept either side of a change; the rest fold into a notice.
const CONTEXT_LINES: usize = 3;

/// An edit as the tool described it, one section per file or per edit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EditPreview {
    pub sections: Vec<EditSection>,
}

/// One file's part of an edit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EditSection {
    pub path: Option<String>,
    pub lines: Vec<DiffLine>,
    /// Lines past [`MAX_PREVIEW_LINES`] that are not in `lines`.
    pub omitted: usize,
    pub added: usize,
    pub removed: usize,
}

impl EditPreview {
    /// Whether a tool of this name edits files, so its call counts as an edit
    /// and may have a preview.
    pub fn is_edit_tool(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "edit" | "write" | "multiedit" | "filechange" | "apply_patch" | "applypatch"
        )
    }

    /// The preview a call's arguments describe, or `None` for a tool that is
    /// not an edit, arguments that do not parse, or an edit that says nothing
    /// about its content.
    pub fn from_tool(name: &str, input: &str) -> Option<Self> {
        if !Self::is_edit_tool(name) {
            return None;
        }
        let input: Value = serde_json::from_str(input).ok()?;
        let object = input.as_object()?;
        let path = object
            .get("file_path")
            .or_else(|| object.get("path"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let sections = match name.to_ascii_lowercase().as_str() {
            "edit" => {
                // Claude's history records what the edit did as a structured
                // patch, which is exact where the arguments are only intent.
                if let Some(patch) = object
                    .get("_flowxStructuredPatch")
                    .and_then(Value::as_array)
                {
                    vec![from_structured_patch(path, patch)]
                } else {
                    let old = object.get("old_string").and_then(Value::as_str)?;
                    let new = object.get("new_string").and_then(Value::as_str)?;
                    vec![section(path, line_diff(old, new))]
                }
            }
            "multiedit" => object
                .get("edits")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(|edit| {
                    let old = edit.get("old_string").and_then(Value::as_str)?;
                    let new = edit.get("new_string").and_then(Value::as_str)?;
                    Some(section(path.clone(), line_diff(old, new)))
                })
                .collect(),
            "write" => {
                let content = object.get("content").and_then(Value::as_str)?;
                vec![section(path, line_diff("", content))]
            }
            "filechange" | "apply_patch" | "applypatch" => object
                .get("changes")
                .and_then(Value::as_array)?
                .iter()
                .filter_map(|change| {
                    let path = change
                        .get("path")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let diff = change.get("diff").and_then(Value::as_str)?;
                    let parsed = parse_section(path.clone(), diff.lines())?;
                    Some(section(path.or(Some(parsed.path)), parsed.lines))
                })
                .collect(),
            _ => return None,
        };
        if sections.is_empty() || sections.iter().all(|section| section.lines.is_empty()) {
            return None;
        }
        Some(Self { sections })
    }

    /// Lines added and removed across every section.
    pub fn totals(&self) -> (usize, usize) {
        self.sections
            .iter()
            .fold((0, 0), |(added, removed), section| {
                (added + section.added, removed + section.removed)
            })
    }
}

/// Count, fold long unchanged runs, and cap one section's lines.
fn section(path: Option<String>, lines: Vec<DiffLine>) -> EditSection {
    let added = lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Addition)
        .count();
    let removed = lines
        .iter()
        .filter(|line| line.kind == DiffLineKind::Deletion)
        .count();
    let mut lines = fold_context(lines);
    let omitted = lines.len().saturating_sub(MAX_PREVIEW_LINES);
    lines.truncate(MAX_PREVIEW_LINES);
    EditSection {
        path,
        lines,
        omitted,
        added,
        removed,
    }
}

/// Keep [`CONTEXT_LINES`] of context either side of each change and say how
/// many lines each longer unchanged run held.
fn fold_context(lines: Vec<DiffLine>) -> Vec<DiffLine> {
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.kind != DiffLineKind::Context)
        .map(|(index, _)| index)
        .collect();
    if changed.is_empty() {
        return lines;
    }
    let keep = |index: usize| {
        changed
            .iter()
            .any(|changed| changed.abs_diff(index) <= CONTEXT_LINES)
    };
    let mut out = Vec::with_capacity(lines.len());
    let mut folded = 0usize;
    let flush = |out: &mut Vec<DiffLine>, folded: &mut usize| {
        if *folded > 0 {
            out.push(DiffLine {
                kind: DiffLineKind::Meta,
                text: format!(
                    "… {} unchanged {}",
                    folded,
                    if *folded == 1 { "line" } else { "lines" }
                ),
                old_line: None,
                new_line: None,
                markers: 0,
            });
            *folded = 0;
        }
    };
    for (index, line) in lines.into_iter().enumerate() {
        if keep(index) {
            flush(&mut out, &mut folded);
            out.push(line);
        } else {
            folded += 1;
        }
    }
    flush(&mut out, &mut folded);
    out
}

/// A line diff of two texts: context, deletions and additions, numbered from
/// one on each side.
///
/// A longest-common-subsequence over lines, which is what `diff` does without
/// its heuristics. Quadratic in the line counts, so past [`MAX_DIFFED_LINES`]
/// either side it is the whole old text removed and the whole new text added.
pub fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let old: Vec<&str> = split_lines(old);
    let new: Vec<&str> = split_lines(new);
    let mut out = Vec::with_capacity(old.len() + new.len());
    let mut old_line = 1u32;
    let mut new_line = 1u32;
    let mut push = |kind: DiffLineKind, text: &str, old: Option<u32>, new: Option<u32>| {
        let marker = match kind {
            DiffLineKind::Addition => '+',
            DiffLineKind::Deletion => '-',
            _ => ' ',
        };
        out.push(DiffLine {
            kind,
            text: format!("{marker}{text}"),
            old_line: old,
            new_line: new,
            markers: 1,
        });
    };

    if old.len() > MAX_DIFFED_LINES || new.len() > MAX_DIFFED_LINES {
        for line in &old {
            push(DiffLineKind::Deletion, line, Some(old_line), None);
            old_line += 1;
        }
        for line in &new {
            push(DiffLineKind::Addition, line, None, Some(new_line));
            new_line += 1;
        }
        return out;
    }

    // lcs[i][j]: the longest common subsequence of old[i..] and new[j..].
    let mut lcs = vec![vec![0u16; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            push(
                DiffLineKind::Context,
                old[i],
                Some(old_line),
                Some(new_line),
            );
            i += 1;
            j += 1;
            old_line += 1;
            new_line += 1;
        } else if i < old.len() && (j == new.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            // Deletions before the additions that replace them, as `diff`
            // prints a changed run.
            push(DiffLineKind::Deletion, old[i], Some(old_line), None);
            i += 1;
            old_line += 1;
        } else {
            push(DiffLineKind::Addition, new[j], None, Some(new_line));
            j += 1;
            new_line += 1;
        }
    }
    out
}

/// A text's lines, without a trailing empty line for its final newline, and
/// none at all for an empty text.
fn split_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Claude's `structuredPatch`: hunks of `lines` each marked ` `, `-` or `+`.
fn from_structured_patch(path: Option<String>, hunks: &[Value]) -> EditSection {
    let mut lines = Vec::new();
    for hunk in hunks {
        let mut old_line = hunk.get("oldStart").and_then(Value::as_u64).unwrap_or(0) as u32;
        let mut new_line = hunk.get("newStart").and_then(Value::as_u64).unwrap_or(0) as u32;
        if hunks.len() > 1 {
            lines.push(DiffLine {
                kind: DiffLineKind::Hunk,
                text: format!("@@ -{old_line} +{new_line} @@"),
                old_line: None,
                new_line: None,
                markers: 0,
            });
        }
        for line in hunk
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let (kind, old, new) = match line.as_bytes().first() {
                Some(b'+') => (DiffLineKind::Addition, None, Some(new_line)),
                Some(b'-') => (DiffLineKind::Deletion, Some(old_line), None),
                _ => (DiffLineKind::Context, Some(old_line), Some(new_line)),
            };
            if old.is_some() {
                old_line += 1;
            }
            if new.is_some() {
                new_line += 1;
            }
            lines.push(DiffLine {
                kind,
                text: line.to_string(),
                old_line: old,
                new_line: new,
                markers: 1,
            });
        }
    }
    section(path, lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(lines: &[DiffLine]) -> String {
        lines
            .iter()
            .map(|line| match line.kind {
                DiffLineKind::Context => ' ',
                DiffLineKind::Addition => '+',
                DiffLineKind::Deletion => '-',
                DiffLineKind::Hunk => '@',
                DiffLineKind::Meta => '…',
            })
            .collect()
    }

    #[test]
    fn an_edit_is_a_line_diff_of_its_old_and_new_text() {
        let input = serde_json::json!({
            "file_path": "/p/a.rs",
            "old_string": "fn a() {\n    one();\n    two();\n}\n",
            "new_string": "fn a() {\n    one();\n    three();\n}\n",
        });
        let preview = EditPreview::from_tool("Edit", &input.to_string()).unwrap();
        let [section] = preview.sections.as_slice() else {
            panic!("{preview:?}");
        };
        assert_eq!(section.path.as_deref(), Some("/p/a.rs"));
        assert_eq!(kinds(&section.lines), "  -+ ");
        assert_eq!(section.lines[2].text, "-    two();");
        assert_eq!(section.lines[3].text, "+    three();");
        assert_eq!(
            (section.lines[2].old_line, section.lines[2].new_line),
            (Some(3), None)
        );
        assert_eq!(
            (section.lines[3].old_line, section.lines[3].new_line),
            (None, Some(3))
        );
        assert_eq!((section.added, section.removed), (1, 1));
        assert_eq!(preview.totals(), (1, 1));
        assert_eq!(section.omitted, 0);
        // The marker is one column, and the content is read without it.
        assert_eq!(section.lines[2].content(), "    two();");
    }

    #[test]
    fn a_write_is_all_additions_and_is_capped() {
        let content: String = (1..=MAX_PREVIEW_LINES + 40)
            .map(|line| format!("line {line}\n"))
            .collect();
        let input = serde_json::json!({ "file_path": "new.txt", "content": content });
        let preview = EditPreview::from_tool("Write", &input.to_string()).unwrap();
        let section = &preview.sections[0];
        assert_eq!(section.lines.len(), MAX_PREVIEW_LINES);
        assert_eq!(section.omitted, 40);
        assert_eq!(
            section.added,
            MAX_PREVIEW_LINES + 40,
            "the count is the whole"
        );
        assert!(
            section
                .lines
                .iter()
                .all(|line| line.kind == DiffLineKind::Addition)
        );
        assert_eq!(section.lines[0].text, "+line 1");
    }

    #[test]
    fn long_unchanged_runs_fold_into_a_notice() {
        let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let new = old.replacen("10\n", "ten\n", 1);
        let lines = fold_context(line_diff(&old, &new));
        assert_eq!(kinds(&lines), "…   -+   …");
        assert_eq!(lines[0].text, "… 6 unchanged lines");
        assert_eq!(lines[9].text, "… 7 unchanged lines");
        // Nothing folds when nothing changed; the text is shown as it is.
        let same = fold_context(line_diff("a\nb\n", "a\nb\n"));
        assert_eq!(kinds(&same), "  ");
    }

    #[test]
    fn a_multi_edit_is_a_section_per_edit() {
        let input = serde_json::json!({
            "file_path": "/p/a.rs",
            "edits": [
                { "old_string": "a", "new_string": "b" },
                { "old_string": "c", "new_string": "d" },
            ],
        });
        let preview = EditPreview::from_tool("MultiEdit", &input.to_string()).unwrap();
        assert_eq!(preview.sections.len(), 2);
        assert_eq!(kinds(&preview.sections[1].lines), "-+");
        assert_eq!(preview.sections[1].path.as_deref(), Some("/p/a.rs"));
    }

    #[test]
    fn a_codex_file_change_is_read_from_its_unified_diffs() {
        let input = serde_json::json!({
            "type": "fileChange",
            "changes": [
                { "path": "/p/a.rs", "kind": { "type": "update", "move_path": null },
                  "diff": "@@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n" },
                { "path": "/p/new.rs", "kind": { "type": "add" }, "diff": "@@ -0,0 +1 @@\n+new\n" },
            ],
        });
        let preview = EditPreview::from_tool("fileChange", &input.to_string()).unwrap();
        assert_eq!(preview.sections.len(), 2);
        assert_eq!(preview.sections[0].path.as_deref(), Some("/p/a.rs"));
        assert_eq!(kinds(&preview.sections[0].lines), "@ -+");
        assert_eq!(preview.sections[1].path.as_deref(), Some("/p/new.rs"));
        assert_eq!(preview.totals(), (2, 1));
    }

    #[test]
    fn a_recorded_edit_prefers_the_patch_the_tool_reported() {
        let input = serde_json::json!({
            "file_path": "/p/a.rs",
            "old_string": "intent",
            "new_string": "differs",
            "_flowxStructuredPatch": [
                { "oldStart": 4, "newStart": 4, "lines": [" ctx", "-old", "+new"] },
                { "oldStart": 9, "newStart": 9, "lines": ["+more"] },
            ],
        });
        let preview = EditPreview::from_tool("Edit", &input.to_string()).unwrap();
        let section = &preview.sections[0];
        assert_eq!(kinds(&section.lines), "@ -+@+");
        assert_eq!(section.lines[2].old_line, Some(5));
        assert_eq!(section.lines[3].new_line, Some(5));
        assert_eq!((section.added, section.removed), (2, 1));
    }

    #[test]
    fn edits_too_large_to_diff_are_shown_whole() {
        let old: String = (0..MAX_DIFFED_LINES + 1)
            .map(|n| format!("{n}\n"))
            .collect();
        let lines = line_diff(&old, "x\n");
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.kind == DiffLineKind::Deletion)
                .count(),
            MAX_DIFFED_LINES + 1
        );
        assert_eq!(lines.last().unwrap().text, "+x");
    }

    #[test]
    fn only_edit_tools_with_readable_arguments_have_a_preview() {
        assert!(EditPreview::is_edit_tool("Edit"));
        assert!(EditPreview::is_edit_tool("fileChange"));
        assert!(!EditPreview::is_edit_tool("Read"));
        assert!(EditPreview::from_tool("Read", r#"{"file_path":"/a"}"#).is_none());
        assert!(EditPreview::from_tool("Edit", "{\"file_path\": \"/a\", \"old_str").is_none());
        assert!(EditPreview::from_tool("Edit", r#"{"file_path":"/a"}"#).is_none());
        assert!(EditPreview::from_tool("Write", r#"{"file_path":"/a","content":""}"#).is_none());
        assert!(EditPreview::from_tool("fileChange", r#"{"changes":[]}"#).is_none());
    }

    #[test]
    fn line_splitting_matches_what_an_editor_shows() {
        assert!(split_lines("").is_empty());
        assert_eq!(split_lines("a"), ["a"]);
        assert_eq!(split_lines("a\n"), ["a"]);
        assert_eq!(split_lines("a\n\n"), ["a", ""]);
        // An edit that adds a trailing newline changes nothing visible.
        assert_eq!(kinds(&line_diff("a", "a\n")), " ");
    }
}
