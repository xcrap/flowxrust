//! Lookups over the saved workspace that the window makes on every frame.
//!
//! The snapshot is a list of projects, each a list of threads. Asking it which
//! project a thread is in, what its binding says, or which threads the sidebar
//! lists is a scan, and the sidebar asked once per row on every repaint: with
//! two thousand threads that was millions of comparisons a frame, plus a copy
//! of the whole snapshot. [`SnapshotIndex`] answers the same questions from
//! maps, and is rebuilt only after the snapshot has been borrowed mutably.
//!
//! Knowing when that happened is [`Versioned`]'s job. Every `&mut` access goes
//! through `DerefMut`, which counts it, so a mutation cannot reach the
//! snapshot without moving its generation — including the many that assign a
//! field directly. A mutable borrow that changes nothing costs one rebuild on
//! the next frame, never a stale answer.

use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use fx_core::Agent;
use fx_store::{PersistedApp, PersistedBinding, PersistedProject};
use uuid::Uuid;

/// A value whose mutable borrows are counted.
///
/// Deliberately not `Clone`: `snapshot.clone()` resolves through `Deref` to
/// the inner value's clone, so a copy taken to edit and commit later is a plain
/// value, and writing it back (`*snapshot = next`) is a mutable borrow that
/// moves the generation like any other.
pub struct Versioned<T> {
    value: T,
    generation: u64,
}

impl<T> Versioned<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            generation: 0,
        }
    }

    /// How many times the value has been borrowed mutably. Equal generations
    /// mean equal contents.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl<T> Deref for Versioned<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Versioned<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.generation = self.generation.wrapping_add(1);
        &mut self.value
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Versioned<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.value.fmt(f)
    }
}

/// One project record's share of the index, by the record's position in the
/// snapshot.
#[derive(Debug, Default)]
pub struct RecordIndex {
    /// The threads the sidebar lists, in its order, as positions in the
    /// record's `agents`: the project's order, less archived and missing
    /// threads and ids with no thread this build can read.
    pub visible: Rc<[usize]>,
    /// Every id in the project's order, listed or not.
    pub ordered: HashSet<Uuid>,
    /// The archived threads, as positions in the record's `agents`, in that
    /// order.
    pub archived: Rc<[usize]>,
    pub unseen: HashSet<Uuid>,
    /// The spaces the sidebar draws, in their order, each with its listed
    /// threads. A space mirroring a provider's section is drawn only while
    /// it lists a thread; one of the reader's own is drawn empty too, since
    /// they just made it to put threads in.
    pub spaces: Rc<[SpaceIndex]>,
    /// The listed threads in no space the sidebar draws, in its order.
    pub ungrouped: Rc<[usize]>,
    /// Each listed thread's space, for the threads in one that is drawn.
    pub space_of: HashMap<Uuid, Uuid>,
}

/// One space's share of its record's index.
#[derive(Debug, Clone)]
pub struct SpaceIndex {
    /// Where the space is in the record's `spaces`.
    pub slot: usize,
    pub id: Uuid,
    /// Its listed threads, as positions in the record's `agents`, in the
    /// sidebar's order: newest first, as everywhere.
    pub threads: Rc<[usize]>,
}

/// Maps over one generation of the snapshot.
///
/// Positions are only meaningful for the generation the index was built from,
/// so every lookup that resolves one checks that it found what it was asked
/// for. Within one render nothing can mutate the snapshot, so in practice the
/// check never fails.
#[derive(Debug, Default)]
pub struct SnapshotIndex {
    generation: Option<u64>,
    /// Each thread's record and position in it, the first a scan would find.
    agents: HashMap<Uuid, (usize, usize)>,
    /// Each thread's project: the first record that has a project and lists
    /// the thread, as `AppState::project_for_agent` always chose.
    projects: HashMap<Uuid, Uuid>,
    /// Each thread's first native binding, by record and position.
    bindings: HashMap<Uuid, (usize, usize)>,
    pub records: Vec<RecordIndex>,
}

impl SnapshotIndex {
    pub fn build(snapshot: &Versioned<PersistedApp>) -> Self {
        let mut index = Self {
            generation: Some(snapshot.generation()),
            ..Self::default()
        };
        for (position, record) in snapshot.projects.iter().enumerate() {
            index.records.push(Self::record(record));
            for (slot, agent) in record.agents.iter().enumerate() {
                index.agents.entry(agent.id).or_insert((position, slot));
                if let Some(project) = &record.project {
                    index.projects.entry(agent.id).or_insert(project.id);
                }
            }
            for (slot, binding) in record.native_bindings.iter().enumerate() {
                index
                    .bindings
                    .entry(binding.agent_id)
                    .or_insert((position, slot));
            }
        }
        index
    }

    fn record(record: &PersistedProject) -> RecordIndex {
        // The first thread with an id, as `iter().find` picked it.
        let mut slots: HashMap<Uuid, usize> = HashMap::with_capacity(record.agents.len());
        for (slot, agent) in record.agents.iter().enumerate() {
            slots.entry(agent.id).or_insert(slot);
        }
        let archived: HashSet<Uuid> = record.archived_agent_ids.iter().copied().collect();
        let missing: HashSet<&Uuid> = record.missing_native_agent_ids.iter().collect();
        let order = record
            .project
            .as_ref()
            .map_or(&[][..], |project| project.agent_order.as_slice());
        let visible: Rc<[usize]> = order
            .iter()
            .filter(|id| !archived.contains(id) && !missing.contains(id))
            .filter_map(|id| slots.get(id).copied())
            .collect();
        // One pass over the listed threads, whatever the number of spaces.
        let mut space_slots: HashMap<Uuid, usize> = HashMap::with_capacity(record.spaces.len());
        for (slot, space) in record.spaces.iter().enumerate() {
            space_slots.entry(space.space.id).or_insert(slot);
        }
        let mut grouped: Vec<Vec<usize>> = vec![Vec::new(); record.spaces.len()];
        let mut ungrouped = Vec::new();
        let mut space_of = HashMap::new();
        for &slot in visible.iter() {
            let agent = &record.agents[slot];
            match agent.space.and_then(|space| space_slots.get(&space)) {
                Some(&at) => {
                    grouped[at].push(slot);
                    space_of.insert(agent.id, record.spaces[at].space.id);
                }
                None => ungrouped.push(slot),
            }
        }
        let spaces = record
            .spaces
            .iter()
            .enumerate()
            .zip(grouped)
            .filter(|((slot, space), threads)| {
                space_slots.get(&space.space.id) == Some(slot)
                    && (space.space.codex_section.is_none() || !threads.is_empty())
            })
            .map(|((slot, space), threads)| SpaceIndex {
                slot,
                id: space.space.id,
                threads: threads.into(),
            })
            .collect();
        RecordIndex {
            visible,
            spaces,
            ungrouped: ungrouped.into(),
            space_of,
            ordered: order.iter().copied().collect(),
            archived: record
                .agents
                .iter()
                .enumerate()
                .filter(|(_, agent)| archived.contains(&agent.id))
                .map(|(slot, _)| slot)
                .collect(),
            unseen: record.unseen_completion_ids.iter().copied().collect(),
        }
    }

    /// Whether this index describes `snapshot` as it is now.
    pub fn is_current(&self, snapshot: &Versioned<PersistedApp>) -> bool {
        self.generation == Some(snapshot.generation())
    }

    pub fn agent<'a>(&self, snapshot: &'a PersistedApp, id: Uuid) -> Option<&'a Agent> {
        let (record, slot) = *self.agents.get(&id)?;
        snapshot
            .projects
            .get(record)?
            .agents
            .get(slot)
            .filter(|agent| agent.id == id)
    }

    pub fn project_for_agent(&self, id: Uuid) -> Option<Uuid> {
        self.projects.get(&id).copied()
    }

    pub fn binding<'a>(
        &self,
        snapshot: &'a PersistedApp,
        id: Uuid,
    ) -> Option<&'a PersistedBinding> {
        let (record, slot) = *self.bindings.get(&id)?;
        snapshot
            .projects
            .get(record)?
            .native_bindings
            .get(slot)
            .filter(|binding| binding.agent_id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx_core::Project;

    #[test]
    fn a_mutable_borrow_moves_the_generation_and_a_shared_one_does_not() {
        let mut value = Versioned::new(vec![1]);
        let start = value.generation();
        assert_eq!(value.len(), 1);
        assert_eq!(value.generation(), start);
        value.push(2);
        assert_eq!(value.generation(), start + 1);
        // A copy taken to edit is a plain value; writing it back counts.
        let mut next: Vec<i32> = value.clone();
        next.push(3);
        assert_eq!(value.generation(), start + 1);
        *value = next;
        assert_eq!(value.generation(), start + 2);
        assert_eq!(*value, vec![1, 2, 3]);
    }

    #[test]
    fn the_index_lists_what_the_sidebar_lists_and_finds_what_a_scan_finds() {
        let agents: Vec<Agent> = (0..5).map(|i| Agent::new(format!("T{i}"))).collect();
        let ids: Vec<Uuid> = agents.iter().map(|agent| agent.id).collect();
        let unknown = Uuid::new_v4();
        let mut project = Project::new("/tmp/indexed");
        // Out of storage order, with an id no thread has.
        project.agent_order = vec![ids[3], unknown, ids[0], ids[1], ids[2], ids[4]];
        let project_id = project.id;
        let binding = |agent_id: Uuid, session: &str| PersistedBinding {
            agent_id,
            provider_id: "claude".into(),
            session_id: session.into(),
            title: String::new(),
            working_directory: String::new(),
            updated_at: fx_core::models::now(),
            model: None,
            effort: None,
            agent_access: None,
            is_archived: false,
            revision: String::new(),
            context_window_size: None,
            local_history_pending: false,
            local_start: None,
            section: None,
        };
        let snapshot = Versioned::new(PersistedApp {
            projects: vec![
                // A record without a project owns no thread's project.
                PersistedProject {
                    project: None,
                    agents: vec![agents[4].clone()],
                    ..Default::default()
                },
                PersistedProject {
                    project: Some(project),
                    agents: agents.clone(),
                    archived_agent_ids: vec![ids[1]],
                    missing_native_agent_ids: vec![ids[2]],
                    unseen_completion_ids: vec![ids[0]],
                    native_bindings: vec![binding(ids[0], "first"), binding(ids[0], "second")],
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        let index = SnapshotIndex::build(&snapshot);
        assert!(index.is_current(&snapshot));
        let listed: Vec<Uuid> = index.records[1]
            .visible
            .iter()
            .map(|&slot| snapshot.projects[1].agents[slot].id)
            .collect();
        assert_eq!(listed, vec![ids[3], ids[0], ids[4]]);
        assert!(index.records[0].visible.is_empty());
        assert!(index.records[1].ordered.contains(&unknown));
        assert_eq!(&index.records[1].archived[..], &[1]);
        assert!(index.records[1].unseen.contains(&ids[0]));
        // The thread in both records is found where a scan meets it first,
        // but its project is the first record that has one.
        assert_eq!(index.agent(&snapshot, ids[4]).unwrap().id, ids[4]);
        assert_eq!(index.project_for_agent(ids[4]), Some(project_id));
        assert_eq!(index.project_for_agent(unknown), None);
        assert_eq!(
            index.binding(&snapshot, ids[0]).unwrap().session_id,
            "first"
        );
        assert!(index.binding(&snapshot, ids[1]).is_none());
    }

    #[test]
    fn spaces_list_their_threads_in_the_sidebars_order_and_the_rest_are_ungrouped() {
        use fx_store::PersistedSpace;
        let mut agents: Vec<Agent> = (0..6).map(|i| Agent::new(format!("T{i}"))).collect();
        let ids: Vec<Uuid> = agents.iter().map(|agent| agent.id).collect();
        let work = fx_core::Space::new("Work");
        let empty = fx_core::Space::new("Empty");
        let mut mirrored = fx_core::Space::new("Pinned");
        mirrored.codex_section = Some("pinned".into());
        let mut drained = fx_core::Space::new("Drained");
        drained.codex_section = Some("drained".into());
        agents[0].space = Some(work.id);
        agents[2].space = Some(work.id);
        agents[3].space = Some(mirrored.id);
        // Archived, so the mirrored space it is in lists nothing.
        agents[4].space = Some(drained.id);
        // A space that is not there groups nothing.
        agents[5].space = Some(Uuid::new_v4());
        let mut project = Project::new("/tmp/spaces");
        project.agent_order = vec![ids[2], ids[5], ids[0], ids[1], ids[3], ids[4]];
        let snapshot = Versioned::new(PersistedApp {
            projects: vec![PersistedProject {
                project: Some(project),
                agents: agents.clone(),
                archived_agent_ids: vec![ids[4]],
                spaces: vec![
                    PersistedSpace::new(work.clone()),
                    PersistedSpace::new(empty.clone()),
                    PersistedSpace::new(mirrored.clone()),
                    PersistedSpace::new(drained.clone()),
                ],
                ..Default::default()
            }],
            ..Default::default()
        });
        let index = SnapshotIndex::build(&snapshot);
        let record = &index.records[0];
        let names = |slots: &[usize]| -> Vec<Uuid> {
            slots
                .iter()
                .map(|&slot| snapshot.projects[0].agents[slot].id)
                .collect()
        };
        let drawn: Vec<(Uuid, Vec<Uuid>)> = record
            .spaces
            .iter()
            .map(|space| (space.id, names(&space.threads)))
            .collect();
        assert_eq!(
            drawn,
            vec![
                (work.id, vec![ids[2], ids[0]]),
                (empty.id, vec![]),
                (mirrored.id, vec![ids[3]]),
            ]
        );
        assert_eq!(record.spaces[2].slot, 2);
        assert_eq!(names(&record.ungrouped), vec![ids[5], ids[1]]);
        assert_eq!(record.space_of.get(&ids[0]), Some(&work.id));
        assert_eq!(record.space_of.get(&ids[5]), None);
        assert_eq!(record.space_of.get(&ids[4]), None, "archived is not listed");
        // The flat list is what it was: search and ⌘1–9 read it.
        assert_eq!(
            names(&record.visible),
            vec![ids[2], ids[5], ids[0], ids[1], ids[3]]
        );
    }
}
