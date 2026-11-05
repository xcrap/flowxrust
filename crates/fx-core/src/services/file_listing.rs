//! The files a project folder offers to an `@` mention.
//!
//! A checkout is listed by git — tracked and untracked files, minus what the
//! repository ignores — because that is the one listing that respects every
//! `.gitignore` the way the user's own tools do. A folder that is not a
//! checkout is walked instead, skipping the build and dependency directories
//! the Files panel skips. Both are bounded: a listing past `limit` entries is
//! cut and says so, rather than building an index the size of `node_modules`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use walkdir::WalkDir;

use crate::git::command::{GIT, hardened_environment};
use crate::policy::ProjectFileIndexPolicy;
use crate::policy::mention::is_mentionable;

/// How deep the walk of a plain folder goes. Deeper than this is a dependency
/// tree or a generated one, not something a reader means to mention.
pub const MAX_WALK_DEPTH: usize = 16;

/// How long git may take to list the files before the walk takes over.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Where a listing came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListingSource {
    /// `git ls-files`, so the repository's ignore rules applied.
    Git,
    /// A bounded walk of the folder, with the Files panel's skip list.
    Walk,
}

/// A project folder's files, as root-relative paths with `/` separators, in
/// name order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileListing {
    pub paths: Vec<String>,
    /// More files existed than `limit` allowed, so the list is a prefix.
    pub truncated: bool,
    pub source: ListingSource,
}

/// List the files under `root`, at most `limit` of them.
///
/// Paths a mention cannot carry (any with whitespace) are left out, since
/// neither CLI reads a mention past a space. Synchronous and bounded, so the
/// caller runs it on a background thread.
pub fn list_files(root: &Path, limit: usize) -> FileListing {
    match git_listing(root, limit) {
        Some(listing) => listing,
        None => walk_listing(root, limit),
    }
}

/// `git ls-files` for a checkout; `None` where `root` is not one, or git is
/// missing or fails. Tracked files that were deleted from the working tree
/// are still in the index, so a second listing names them and they are
/// dropped: a mention of a file that is gone helps nobody.
fn git_listing(root: &Path, limit: usize) -> Option<FileListing> {
    if !Path::new(GIT).exists() {
        return None;
    }
    let listed = git_lines(
        root,
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let deleted = git_lines(root, &["ls-files", "-z", "--deleted"]).unwrap_or_default();
    let mut paths: Vec<String> = listed
        .into_iter()
        .filter(|path| !deleted.contains(path))
        .filter(|path| is_mentionable(path))
        .collect();
    paths.sort();
    paths.dedup();
    let truncated = paths.len() > limit;
    paths.truncate(limit);
    Some(FileListing {
        paths,
        truncated,
        source: ListingSource::Git,
    })
}

/// One NUL-separated git listing, or `None` when git could not give one.
fn git_lines(root: &Path, args: &[&str]) -> Option<Vec<String>> {
    let mut command = Command::new(GIT);
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env_clear()
        .envs(std::env::vars().filter(|(key, _)| key == "PATH" || key == "HOME"))
        .envs(hardened_environment())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let output = read_with_timeout(&mut child, GIT_TIMEOUT)?;
    Some(
        output
            .split(|&byte| byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| String::from_utf8_lossy(entry).into_owned())
            .collect(),
    )
}

/// The child's stdout, or `None` if it failed or outlived the timeout. The
/// read runs on a thread of its own so a quiet child cannot hold the caller
/// past the timeout; the child is killed either way.
fn read_with_timeout(child: &mut std::process::Child, timeout: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).ok().map(|_| bytes)
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader.join().ok().flatten()?;
    status.filter(|status| status.success()).map(|_| bytes)
}

/// A bounded walk for a folder that is not a checkout.
fn walk_listing(root: &Path, limit: usize) -> FileListing {
    let mut paths = Vec::new();
    let mut truncated = false;
    let walker = WalkDir::new(root)
        .follow_links(false)
        .min_depth(1)
        .max_depth(MAX_WALK_DEPTH)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
            !ProjectFileIndexPolicy::should_skip(relative, entry.file_type().is_dir())
        });
    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let path = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if !is_mentionable(&path) {
            continue;
        }
        if paths.len() == limit {
            truncated = true;
            break;
        }
        paths.push(path);
    }
    paths.sort();
    FileListing {
        paths,
        truncated,
        source: ListingSource::Walk,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new(GIT)
            .arg("-C")
            .arg(root)
            .args(args)
            .envs(hardened_environment())
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn a_checkout_is_listed_by_git_without_what_it_ignores() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "-q", "-b", "main"]);
        write(root, ".gitignore", "target/\n*.log\n");
        write(root, "src/lib.rs", "");
        write(root, "src/with space.rs", "");
        write(root, "README.md", "");
        write(root, "target/debug/out.bin", "");
        write(root, "notes.log", "");
        write(root, "gone.rs", "");
        git(root, &["add", "-A"]);
        git(root, &["commit", "-q", "-m", "init"]);
        // Untracked but not ignored: listed. Tracked but deleted: not.
        write(root, "src/new.rs", "");
        fs::remove_file(root.join("gone.rs")).unwrap();

        let listing = list_files(root, 100);
        assert_eq!(listing.source, ListingSource::Git);
        assert!(!listing.truncated);
        assert_eq!(
            listing.paths,
            [".gitignore", "README.md", "src/lib.rs", "src/new.rs"]
        );
    }

    #[test]
    fn a_plain_folder_is_walked_with_the_skip_list_and_a_bound() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(root, "b.txt", "");
        write(root, "a.txt", "");
        write(root, "docs/guide.md", "");
        write(root, "node_modules/left-pad/index.js", "");
        write(root, "target/debug/app", "");
        write(root, "has space.txt", "");

        let listing = list_files(root, 100);
        assert_eq!(listing.source, ListingSource::Walk);
        assert_eq!(listing.paths, ["a.txt", "b.txt", "docs/guide.md"]);

        let bounded = list_files(root, 2);
        assert!(bounded.truncated);
        assert_eq!(bounded.paths.len(), 2);
    }

    #[test]
    fn a_missing_folder_lists_nothing() {
        let listing = list_files(Path::new("/definitely/not/here"), 10);
        assert!(listing.paths.is_empty());
        assert!(!listing.truncated);
    }
}
