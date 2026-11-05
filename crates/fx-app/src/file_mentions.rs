//! The composer's `@` file picker: the files under a thread's execution root,
//! listed off the UI thread and kept per root, and the picking session a
//! typed `@` opens over them.
//!
//! The pure halves live in `fx-core`: `policy::mention` finds the token at
//! the caret and ranks paths for it, and `services::file_listing` lists a
//! folder. This module is what the app keeps between frames, and what the
//! composer's popup is built from.

use std::sync::Arc;
use std::time::{Duration, Instant};

use fx_core::policy::mention::{self, MentionToken};
use fx_core::services::FileListing;
use fx_design::components::{MenuItem, MenuModel, MenuSection};
use fx_design::theme::Layout;
use gpui::SharedString;
use uuid::Uuid;

use crate::composer::{MentionPopup, MentionRow};

/// How long a root's listing serves new `@`s before it is listed again. A
/// turn writes files for as long as it runs, so a listing is not kept for the
/// life of the app; nor is it rebuilt per keystroke, which on a large
/// checkout would run `git ls-files` at typing speed.
pub const INDEX_TTL: Duration = Duration::from_secs(15);

/// The most files one root is listed to. Past this the listing is a prefix,
/// and the popup says so rather than pretending the file is not there.
pub const MAX_INDEXED_FILES: usize = 50_000;

/// The one section the picker's menu model has.
const SECTION: &str = "files";

/// The files under one execution root, as the picker ranks them.
#[derive(Debug)]
pub struct FileIndex {
    pub root: String,
    /// Root-relative paths with `/` separators, in name order. Shared, so a
    /// session re-ranking them copies nothing.
    pub paths: Arc<Vec<String>>,
    pub truncated: bool,
    pub built: Instant,
}

impl FileIndex {
    pub fn new(root: impl Into<String>, listing: FileListing) -> Self {
        Self {
            root: root.into(),
            paths: Arc::new(listing.paths),
            truncated: listing.truncated,
            built: Instant::now(),
        }
    }

    /// Whether the listing is recent enough to serve another `@`.
    pub fn is_fresh(&self, now: Instant) -> bool {
        now.duration_since(self.built) < INDEX_TTL
    }
}

/// An `@` being typed in a composer, and the picker over it.
///
/// The rows are a [`MenuModel`], which owns the highlight and how the arrow
/// keys move it, so the picker navigates exactly as the app's menus do:
/// pointer and keyboard share one highlight, and movement wraps.
#[derive(Debug)]
pub struct MentionSession {
    pub agent_id: Uuid,
    pub root: String,
    pub token: MentionToken,
    index: Option<Arc<FileIndex>>,
    /// Indices into the index's paths, best match first.
    results: Vec<usize>,
    model: MenuModel,
}

impl MentionSession {
    /// Open the picker for a token. Without an index yet, it opens empty and
    /// says it is listing; [`Self::set_index`] fills it when the listing lands.
    pub fn open(
        agent_id: Uuid,
        root: impl Into<String>,
        token: MentionToken,
        index: Option<Arc<FileIndex>>,
    ) -> Self {
        let mut session = Self {
            agent_id,
            root: root.into(),
            token,
            index,
            results: Vec::new(),
            model: MenuModel::new(Vec::new()),
        };
        session.refilter();
        session
    }

    /// The reader typed on. Re-rank for the new token, keeping the highlight
    /// on the same file where it is still listed. Says whether anything
    /// changed, so an unchanged token costs no repaint.
    pub fn retarget(&mut self, token: MentionToken) -> bool {
        if self.token == token {
            return false;
        }
        self.token = token;
        self.refilter();
        true
    }

    /// The root's listing arrived, or was refreshed.
    pub fn set_index(&mut self, index: Arc<FileIndex>) {
        self.index = Some(index);
        self.refilter();
    }

    pub fn is_loading(&self) -> bool {
        self.index.is_none()
    }

    fn refilter(&mut self) {
        let Some(index) = &self.index else {
            self.results.clear();
            self.model = MenuModel::new(Vec::new());
            return;
        };
        self.results = mention::rank(&self.token.query, &index.paths, Layout::MENTION_MAX_ROWS);
        let previous = self.model.highlighted().map(str::to_string);
        let items = self
            .results
            .iter()
            .map(|&i| {
                let path = &index.paths[i];
                MenuItem::new(path.clone(), file_name(path))
            })
            .collect();
        self.model = MenuModel::new(vec![MenuSection::new(SECTION, items)]);
        let kept = previous.is_some_and(|key| self.model.set_highlight(&key));
        if !kept {
            self.model.highlight_first();
        }
    }

    pub fn move_highlight(&mut self, delta: isize) -> bool {
        self.model.move_highlight(delta)
    }

    /// Highlight a row the pointer is over. Says whether it moved, so the row
    /// already highlighted costs nothing as the pointer travels along it.
    pub fn set_highlight(&mut self, key: &str) -> bool {
        if self.model.highlighted() == Some(key) {
            return false;
        }
        self.model.set_highlight(key)
    }

    /// The file a row key names, or the highlighted one when no key is given.
    pub fn path_for(&self, key: Option<&str>) -> Option<&str> {
        let key = key.or_else(|| self.model.highlighted())?;
        self.rows()
            .find(|path| MenuModel::key_for(SECTION, path) == key)
    }

    fn rows(&self) -> impl Iterator<Item = &str> {
        let paths = self.index.as_ref().map(|index| &index.paths);
        self.results
            .iter()
            .filter_map(move |&i| paths.and_then(|paths| paths.get(i)).map(String::as_str))
    }

    /// What the composer draws.
    pub fn popup(&self) -> MentionPopup {
        MentionPopup {
            query: self.token.query.clone(),
            rows: self
                .rows()
                .map(|path| MentionRow {
                    key: MenuModel::key_for(SECTION, path),
                    name: SharedString::from(file_name(path).to_string()),
                    directory: SharedString::from(directory(path).to_string()),
                    path: path.to_string(),
                })
                .collect(),
            highlighted: self.model.highlighted().map(str::to_string),
            loading: self.is_loading(),
            truncated: self.index.as_ref().is_some_and(|index| index.truncated),
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx_core::services::ListingSource;

    fn index(paths: &[&str]) -> Arc<FileIndex> {
        Arc::new(FileIndex::new(
            "/repo",
            FileListing {
                paths: paths.iter().map(|path| path.to_string()).collect(),
                truncated: false,
                source: ListingSource::Walk,
            },
        ))
    }

    fn token(query: &str) -> MentionToken {
        MentionToken {
            range: 4..5 + query.len(),
            query: query.to_string(),
        }
    }

    const PATHS: [&str; 4] = [
        "AGENTS.md",
        "crates/fx-app/src/commands.rs",
        "crates/fx-app/src/composer.rs",
        "crates/fx-core/src/policy/mention.rs",
    ];

    #[test]
    fn a_session_ranks_the_root_s_files_for_its_token_and_highlights_the_best() {
        let session =
            MentionSession::open(Uuid::new_v4(), "/repo", token("comp"), Some(index(&PATHS)));
        let popup = session.popup();
        let paths: Vec<&str> = popup.rows.iter().map(|row| row.path.as_str()).collect();
        assert_eq!(
            paths,
            ["crates/fx-app/src/composer.rs"],
            "`comp` is in no other name or path"
        );
        assert_eq!(popup.rows[0].name.as_ref(), "composer.rs");
        assert_eq!(popup.rows[0].directory.as_ref(), "crates/fx-app/src");
        assert_eq!(
            popup.highlighted.as_deref(),
            Some(popup.rows[0].key.as_str())
        );
        assert_eq!(
            session.path_for(None),
            Some("crates/fx-app/src/composer.rs")
        );
        assert_eq!(session.path_for(Some("files|nowhere.rs")), None);
        assert!(!popup.loading);

        // Two names that match alike read in name order, and a path that only
        // matches in its directories comes after both.
        let session =
            MentionSession::open(Uuid::new_v4(), "/repo", token("co"), Some(index(&PATHS)));
        let popup = session.popup();
        let paths: Vec<&str> = popup.rows.iter().map(|row| row.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "crates/fx-app/src/commands.rs",
                "crates/fx-app/src/composer.rs",
                "crates/fx-core/src/policy/mention.rs",
            ]
        );
        assert_eq!(
            session.path_for(Some(&popup.rows[1].key)),
            Some("crates/fx-app/src/composer.rs")
        );
    }

    #[test]
    fn retargeting_keeps_the_highlighted_file_where_it_is_still_listed() {
        let mut session =
            MentionSession::open(Uuid::new_v4(), "/repo", token("c"), Some(index(&PATHS)));
        assert!(session.move_highlight(1), "the second row");
        let chosen = session.path_for(None).unwrap().to_string();
        assert!(
            !session.retarget(token("c")),
            "the same token changes nothing"
        );
        assert!(session.retarget(token(&chosen[chosen.rfind('/').unwrap() + 1..][..4])));
        assert_eq!(
            session.path_for(None),
            Some(chosen.as_str()),
            "the reader's choice survives the re-ranking"
        );
        assert!(session.retarget(token("zzz")));
        assert!(session.popup().rows.is_empty());
        assert_eq!(session.path_for(None), None, "nothing to take");
    }

    #[test]
    fn a_session_opened_before_its_listing_fills_in_when_it_lands() {
        let mut session = MentionSession::open(Uuid::new_v4(), "/repo", token("AG"), None);
        let popup = session.popup();
        assert!(popup.loading && popup.rows.is_empty() && popup.highlighted.is_none());
        assert_eq!(session.path_for(None), None);
        session.set_index(index(&PATHS));
        let popup = session.popup();
        assert!(!popup.loading);
        assert_eq!(popup.rows[0].path, "AGENTS.md");
        assert_eq!(
            popup.rows[0].directory.as_ref(),
            "",
            "a root file has no directory"
        );
        assert!(!session.set_highlight(&popup.rows[0].key), "already there");
    }

    #[test]
    fn an_index_goes_stale_after_its_ttl() {
        let index = FileIndex::new(
            "/repo",
            fx_core::services::FileListing {
                paths: Vec::new(),
                truncated: true,
                source: ListingSource::Git,
            },
        );
        assert!(index.is_fresh(index.built));
        assert!(!index.is_fresh(index.built + INDEX_TTL));
    }
}
