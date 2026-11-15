//! Bounded native-history workers. No provider IO runs on the GPUI thread.
//!
//! The owner calls `poll_native_sync` every 500ms. Each provider has one worker
//! and one in-flight request, so an offline provider cannot block the other or
//! accumulate a backlog while a project is switched repeatedly.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::{Duration, Instant};

use fx_agent::native::{NativeThread, NativeThreadIndex};
use fx_store::AppPaths;
use uuid::Uuid;

pub(crate) const PROVIDERS: [&str; 2] = ["codex", "claude"];
pub(crate) const INDEX_INTERVAL: Duration = Duration::from_secs(5);
/// How often an expanded project that is not the active one is read: its rows
/// are on screen, so a thread archived elsewhere must not linger, but a pass
/// per project every five seconds is more than a glance needs.
pub(crate) const BACKGROUND_INDEX_INTERVAL: Duration = Duration::from_secs(30);
pub(crate) const HISTORY_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeSyncStatus {
    pub syncing: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct Ticket {
    pub project: Uuid,
    pub root: String,
    pub provider: String,
    pub generation: u64,
    pub kind: RequestKind,
}

#[derive(Debug, Clone)]
pub(crate) enum RequestKind {
    Index {
        candidates: Vec<Uuid>,
        missing_candidates: Vec<(Uuid, String)>,
    },
    History {
        agent: Uuid,
        session: String,
        revision: u64,
        known_revision: Option<String>,
    },
}

#[derive(Debug)]
pub(crate) enum Payload {
    Index {
        index: NativeThreadIndex,
        sessions: Vec<(Uuid, String)>,
        protected: Vec<Uuid>,
    },
    /// Boxed: a thread is several times the size of the other payloads.
    History(Box<NativeThread>),
    Unchanged,
    /// The provider's CLI is not installed: nothing to list, nothing wrong,
    /// and no proof that any of its threads is gone.
    NotInstalled,
}

#[derive(Debug)]
pub(crate) struct Completion {
    pub ticket: Ticket,
    pub result: Result<Payload, String>,
}

struct Worker {
    sender: SyncSender<Ticket>,
    receiver: Receiver<Completion>,
    pending: Option<Ticket>,
}

#[derive(Default)]
pub(crate) struct NativeSync {
    workers: HashMap<String, Worker>,
    pub generations: HashMap<Uuid, u64>,
    pub index_due: HashMap<(Uuid, String), Instant>,
    pub history_due: HashMap<Uuid, Instant>,
    pub warnings: HashMap<(Uuid, String), Vec<String>>,
    pub history_warnings: HashMap<Uuid, String>,
    pub selected: Option<(Uuid, Uuid)>,
    pub completed_turns: HashMap<Uuid, Instant>,
    pub last_was_index: HashMap<String, bool>,
    pub loaded_revisions: HashMap<Uuid, String>,
    #[cfg(test)]
    test_enabled: bool,
}

impl NativeSync {
    pub fn allowed(&self) -> bool {
        #[cfg(test)]
        {
            self.test_enabled
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    #[cfg(test)]
    pub fn test_worker(&mut self, provider: &str) -> (Receiver<Ticket>, SyncSender<Completion>) {
        let (sender, jobs) = sync_channel(1);
        let (results, receiver) = sync_channel(1);
        self.workers.insert(
            provider.into(),
            Worker {
                sender,
                receiver,
                pending: None,
            },
        );
        self.test_enabled = true;
        (jobs, results)
    }

    pub fn generation(&self, project: Uuid) -> u64 {
        self.generations.get(&project).copied().unwrap_or_default()
    }

    pub fn invalidate(&mut self, project: Uuid) {
        *self.generations.entry(project).or_default() += 1;
        self.index_due.retain(|(id, _), _| *id != project);
        self.warnings.retain(|(id, _), _| *id != project);
    }

    pub fn ready(&self, provider: &str) -> bool {
        self.workers
            .get(provider)
            .is_none_or(|worker| worker.pending.is_none())
    }

    pub fn pending_for(&self, project: Uuid) -> bool {
        self.workers.values().any(|worker| {
            worker
                .pending
                .as_ref()
                .is_some_and(|ticket| ticket.project == project)
        })
    }

    pub fn history_loading(&self, agent: Uuid) -> bool {
        self.workers.values().any(|worker| {
            worker.pending.as_ref().is_some_and(|ticket| {
            matches!(ticket.kind, RequestKind::History { agent: pending, .. } if pending == agent)
        })
        })
    }

    pub fn submit(&mut self, ticket: Ticket, _paths: AppPaths) -> Result<(), String> {
        let provider = ticket.provider.clone();
        if !self.workers.contains_key(&provider) {
            #[cfg(test)]
            return Err(
                "A native test backend must be explicitly installed for every provider.".into(),
            );
            #[cfg(not(test))]
            self.workers
                .insert(provider.clone(), Worker::start(&provider, _paths)?);
        }
        let worker = self.workers.get_mut(&provider).unwrap();
        worker
            .sender
            .try_send(ticket.clone())
            .map_err(|error| error.to_string())?;
        self.last_was_index
            .insert(provider, matches!(ticket.kind, RequestKind::Index { .. }));
        worker.pending = Some(ticket);
        Ok(())
    }

    pub fn drain(&mut self) -> Vec<Completion> {
        let mut out = Vec::new();
        for worker in self.workers.values_mut() {
            match worker.receiver.try_recv() {
                Ok(completion) => {
                    worker.pending = None;
                    out.push(completion);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if let Some(ticket) = worker.pending.take() {
                        out.push(Completion { ticket, result: Err("Native history worker stopped unexpectedly. Restart FlowX to reconnect.".into()) });
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        out
    }
}

#[cfg(not(test))]
impl Worker {
    fn start(provider: &str, paths: AppPaths) -> Result<Self, String> {
        let (sender, jobs) = sync_channel::<Ticket>(1);
        let (results, receiver) = sync_channel(1);
        let provider = provider.to_owned();
        std::thread::Builder::new().name(format!("native-{provider}")).spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
            let codex = fx_agent::codex::native::CodexNativeStore::new(fx_core::services::runtime::RuntimeDiscovery::default());
            let claude = fx_agent::claude::native::ClaudeNativeStore::default();
            let store = fx_store::Store::new(paths);
            let mut drafts = DraftChecks::default();
            for ticket in jobs {
                let result = match &ticket.kind {
                    RequestKind::Index { candidates, missing_candidates } => {
                        let result = if provider == "codex" {
                            match &runtime {
                                Ok(runtime) => runtime.block_on(codex.list_if_installed(&ticket.root)).map_err(|error| error.to_string()),
                                Err(error) => Err(error.to_string()),
                            }
                        } else {
                            claude.list(&ticket.root).map(Some).map_err(|error| error.to_string())
                        };
                        result.map(|index| {
                            let Some(mut index) = index else { return Payload::NotInstalled };
                            let sessions = candidates.iter().filter_map(|&agent| {
                                match store.load_conversation(ticket.project, agent) {
                                    Ok(Some(conversation)) if conversation.agent_id == agent => conversation.session_id.map(|session| (agent, session)),
                                    Ok(_) => None,
                                    Err(error) => {
                                        index.warnings.push(format!("Could not check a saved thread for duplicate native history: {error}"));
                                        None
                                    }
                                }
                            }).collect();
                            let listed: HashSet<&str> = index.threads.iter().map(|entry| entry.summary.identity.session_id.as_str()).collect();
                            let missing: Vec<Uuid> = missing_candidates.iter().filter(|(_, session)| !listed.contains(session.as_str())).map(|(agent, _)| *agent).collect();
                            let protected = missing.iter().copied().filter(|agent| {
                                drafts.protect(&store, ticket.project, *agent, &mut index.warnings)
                            }).collect();
                            drafts.retain(ticket.project, &missing.into_iter().collect());
                            Payload::Index { index, sessions, protected }
                        })
                    }
                    RequestKind::History { session, known_revision, .. } => {
                        let result = if provider == "codex" {
                            match &runtime {
                                Ok(runtime) => runtime.block_on(codex.read_if_changed(&ticket.root, session, known_revision.as_deref())).map_err(|error| error.to_string()),
                                Err(error) => Err(error.to_string()),
                            }
                        } else {
                            claude.read_if_changed(&ticket.root, session, known_revision.as_deref()).map_err(|error| error.to_string())
                        };
                        result.map(|history| history.map(|history| Payload::History(Box::new(history))).unwrap_or(Payload::Unchanged))
                    }
                };
                if results.send(Completion { ticket, result }).is_err() { break; }
            }
        }).map_err(|error| error.to_string())?;
        Ok(Self {
            sender,
            receiver,
            pending: None,
        })
    }
}

/// What each missing thread's saved transcript says about unsent input, by
/// the stamp of the files it was read from.
///
/// Every index pass asked it of every missing thread, hidden ones included,
/// and with Claude deleting transcripts after 30 days that is most of an old
/// project: each answer read and parsed a saved transcript of up to 32 MiB.
/// Every save renames a new file into place, so a changed file has a new
/// stamp; an unreadable one is not remembered, and is asked again.
#[derive(Default)]
pub(crate) struct DraftChecks {
    known: HashMap<(Uuid, Uuid), (DraftStamp, bool)>,
    #[cfg(test)]
    reads: usize,
}

/// A transcript's primary and recovery copy, as `load_conversation` reads
/// one or the other.
type DraftStamp = [Option<FileStamp>; 2];

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    inode: (u64, u64),
    changed: (i64, i64),
}

fn file_stamp(path: &std::path::Path) -> Option<FileStamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        inode: (metadata.dev(), metadata.ino()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    })
}

impl DraftChecks {
    /// [`protect_missing_thread`], read again only once its file changed.
    pub fn protect(
        &mut self,
        store: &fx_store::Store,
        project: Uuid,
        agent: Uuid,
        warnings: &mut Vec<String>,
    ) -> bool {
        let path = store.paths().conversation_file(project, agent);
        // Taken before the read, so a save that lands during it leaves a
        // stamp that no longer matches.
        let stamp = [
            file_stamp(&path),
            file_stamp(&fx_store::write::backup_path(&path)),
        ];
        if let Some((known, protected)) = self.known.get(&(project, agent))
            && *known == stamp
        {
            return *protected;
        }
        #[cfg(test)]
        {
            self.reads += 1;
        }
        let before = warnings.len();
        let protected = protect_missing_thread(store, project, agent, warnings);
        if warnings.len() == before {
            self.known.insert((project, agent), (stamp, protected));
        } else {
            self.known.remove(&(project, agent));
        }
        protected
    }

    /// Forget the threads of `project` that are no longer checked.
    pub fn retain(&mut self, project: Uuid, checked: &HashSet<Uuid>) {
        self.known
            .retain(|(id, agent), _| *id != project || checked.contains(agent));
    }
}

/// Unopened chats have no live composer to inspect. Keep every kind of saved
/// input reachable, and fail closed when its recovery file cannot be read.
pub(crate) fn protect_missing_thread(
    store: &fx_store::Store,
    project: Uuid,
    agent: Uuid,
    warnings: &mut Vec<String>,
) -> bool {
    match store.load_conversation(project, agent) {
        Ok(Some(conversation)) => conversation.has_pending_input(),
        Err(error) => {
            warnings.push(format!(
                "Could not check a missing native thread's saved draft: {error}"
            ));
            true
        }
        Ok(None) => false,
    }
}

pub(crate) fn same_project(left: &str, right: &str) -> bool {
    fn canonical(path: &str) -> std::path::PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path))
    }
    // The same text resolves to the same place, and nearly every thread
    // records its project's root exactly as the project does. Resolving both
    // anyway was two `realpath` calls per thread at each of four checks, on
    // every index pass, on the UI thread.
    left == right || canonical(left) == canonical(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_threads_draft_is_read_again_only_once_its_file_changes() {
        // Every pass read and parsed each missing thread's saved transcript,
        // hidden ones included, to see whether it held a draft.
        let temp = tempfile::tempdir().unwrap();
        let store = fx_store::Store::new(AppPaths::at(temp.path()));
        let (project, agent) = (Uuid::new_v4(), Uuid::new_v4());
        let mut checks = DraftChecks::default();
        let mut warnings = Vec::new();
        assert!(!checks.protect(&store, project, agent, &mut warnings));
        assert!(!checks.protect(&store, project, agent, &mut warnings));
        assert_eq!(checks.reads, 1);
        let mut saved = fx_store::PersistedConversation::from_messages(agent, Vec::new());
        saved.draft_text = "Unsent".into();
        store.save_conversation(project, agent, &saved).unwrap();
        assert!(checks.protect(&store, project, agent, &mut warnings));
        assert!(checks.protect(&store, project, agent, &mut warnings));
        assert_eq!(checks.reads, 2);
        saved.draft_text.clear();
        store.save_conversation(project, agent, &saved).unwrap();
        assert!(!checks.protect(&store, project, agent, &mut warnings));
        assert_eq!(checks.reads, 3);
        assert!(warnings.is_empty());
        // A thread that is no longer missing is forgotten.
        checks.retain(project, &HashSet::new());
        assert!(checks.known.is_empty());
    }
}
