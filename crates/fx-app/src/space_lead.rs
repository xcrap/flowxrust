//! Every space's lead: the orchestrator thread its header opens.
//!
//! A lead is an ordinary thread (`Agent::lead_of`) with the orchestrator's
//! brief as its system prompt and FlowX's tools on. It is made the first
//! time its space's header is opened, never up front, so a space nobody
//! orchestrates has no empty thread behind it. It is titled after its
//! space and kept in step with a rename, and when its space is deleted it
//! stays, as an ordinary thread in no space, so no conversation is lost.
//!
//! Which thread leads a space is one rule, `snapshot_index::space_leads`:
//! the first listed thread naming it. The index answers it for the paths
//! that run every frame, and [`lead_in`] by a scan of one record for the
//! edits, which change a copy of the snapshot the index has not seen.

use std::sync::{Arc, Mutex};

use fx_agent::conversation::Conversation;
use fx_core::Agent;
use fx_core::policy::orchestrator;
use fx_store::PersistedProject;
use uuid::Uuid;

use crate::snapshot_index::{listed_slots, space_leads};
use crate::state::{AppState, local_override};

impl AppState {
    /// The thread a space's header opens, if it has been made.
    ///
    /// A scan of the space's project, for the edits; a frame asks the index
    /// (`SnapshotIndex::space_led_by`).
    pub fn space_lead(&self, space_id: Uuid) -> Option<Uuid> {
        let record = self
            .snapshot
            .projects
            .iter()
            .find(|record| record.spaces.iter().any(|held| held.space.id == space_id))?;
        lead_in(record, space_id).map(|slot| record.agents[slot].id)
    }

    /// A space's lead, made now if it has none: a thread in the space,
    /// titled after it, configured as a new thread in its project would be,
    /// with the orchestrator's brief as its system prompt and FlowX's tools
    /// on. Saved before it exists in memory, and not selected; the caller
    /// opens it.
    ///
    /// A space has one lead. A thread that led it but is archived or missing
    /// gives the role up to the new one in the same write, so restoring it
    /// brings back an ordinary thread rather than a second lead. `None`,
    /// with `last_error` saying why, when the space is gone or the save
    /// failed.
    pub fn ensure_space_lead(&mut self, space_id: Uuid) -> Option<Uuid> {
        if let Some(lead) = self.space_lead(space_id) {
            return Some(lead);
        }
        if !self.spaces_can_change() {
            return None;
        }
        let Some((project_id, held)) = self.space(space_id) else {
            self.last_error = Some("This space no longer exists.".into());
            return None;
        };
        let title = orchestrator::title(&held.space.name);
        let provider = self.new_thread_provider(project_id);
        let mut agent = Agent::new(title.clone());
        agent.space = Some(space_id);
        agent.lead_of = Some(space_id);
        agent.configuration = self.default_configuration(&provider);
        agent.configuration.system_prompt = Some(orchestrator::BRIEF.into());
        agent.configuration.flowx_tools = Some(true);
        let id = agent.id;

        let mut next = self.snapshot.clone();
        let Some(record) = next
            .projects
            .iter_mut()
            .find(|record| record.project.as_ref().is_some_and(|p| p.id == project_id))
        else {
            self.last_error = Some("This project is no longer open.".into());
            return None;
        };
        release_leads(record, space_id);
        // First, as every new thread is put.
        record.project.as_mut()?.agent_order.insert(0, id);
        record.agents.push(agent);
        // Named as a rename names a thread: without it, the first sync pass
        // after its turn replaced the space's name with its first prompt.
        local_override(record, id).title = Some(title);
        if !self.commit_thread_change(next, "start this space's lead") {
            return None;
        }
        self.conversations
            .insert(id, Arc::new(Mutex::new(Conversation::new(id))));
        Some(id)
    }

    /// The provider a new thread in a project starts on: that of the thread
    /// the project shows, as `create_thread` reads it once the project is
    /// entered, and Codex when it shows none.
    fn new_thread_provider(&self, project_id: Uuid) -> String {
        let Some(record) = self
            .snapshot
            .projects
            .iter()
            .find(|record| record.project.as_ref().is_some_and(|p| p.id == project_id))
        else {
            return "codex".into();
        };
        let shown = self
            .snapshot
            .active_agent_id
            .filter(|id| record.agents.iter().any(|agent| agent.id == *id))
            .or(record.last_selected_agent_id);
        shown
            .and_then(|id| {
                let agent = record.agents.iter().find(|agent| agent.id == id)?;
                agent
                    .configuration
                    .provider_id
                    .clone()
                    .or_else(|| self.native_defaults(id).provider_id)
            })
            .unwrap_or_else(|| "codex".into())
    }

    /// Give a reset lead's replacement its role: Reset keeps a thread the
    /// provider knows and opens a fresh one in its place, and the fresh one
    /// is what the space's header must open from now on. The old thread
    /// keeps its conversation, as an ordinary thread in the space.
    pub(crate) fn hand_over_lead(&mut self, from: Uuid, to: Uuid) {
        let Some(space) = self.space_lead_of(from) else {
            return;
        };
        let Some(name) = self.space(space).map(|(_, held)| held.space.name.clone()) else {
            return;
        };
        let Some(record) = self
            .snapshot
            .projects
            .iter_mut()
            .find(|record| record.agents.iter().any(|agent| agent.id == from))
        else {
            return;
        };
        let title = orchestrator::title(&name);
        for agent in &mut record.agents {
            if agent.id == from {
                release(agent);
            } else if agent.id == to {
                agent.lead_of = Some(space);
                agent.space = Some(space);
                agent.title = title.clone();
            }
        }
        record.auto_title_pending.retain(|id| *id != to);
        local_override(record, to).title = Some(title);
    }

    /// The space a thread leads, by a scan of its project.
    fn space_lead_of(&self, agent_id: Uuid) -> Option<Uuid> {
        let space = self.thread(agent_id)?.lead_of?;
        (self.space_lead(space) == Some(agent_id)).then_some(space)
    }

    /// What a thread's composer says while it is empty. A frame asks, so
    /// it reads the index.
    pub(crate) fn composer_placeholder(&self, agent_id: Uuid) -> String {
        self.snapshot_index()
            .space_led_by(agent_id)
            .and_then(|space| self.space(space))
            .map_or_else(
                || crate::state::COMPOSER_PLACEHOLDER.to_string(),
                |(_, held)| orchestrator::placeholder(&held.space.name),
            )
    }

    /// What a worker's mark says: which space's lead started it. `None` for
    /// a thread no lead started.
    pub(crate) fn started_by(&self, agent: &Agent) -> Option<String> {
        let lead = agent.spawned_by?;
        let index = self.snapshot_index();
        let space = index
            .space_led_by(lead)
            .and_then(|space| self.space(space))
            .map(|(_, held)| held.space.name.trim().to_string())
            .filter(|name| !name.is_empty());
        Some(match (space, index.agent(&self.snapshot, lead)) {
            (Some(name), _) => format!("Started by the {name} lead"),
            // A lead whose space is gone is an ordinary thread now.
            (None, Some(thread)) if !thread.title.trim().is_empty() => {
                format!("Started by {}", thread.title.trim())
            }
            _ => "Started by a space's lead".into(),
        })
    }

    /// A thread's title by its id, for a row that names one. A frame asks,
    /// so it reads the index.
    pub(crate) fn thread_title(&self, agent_id: Uuid) -> Option<String> {
        let index = self.snapshot_index();
        let agent = index.agent(&self.snapshot, agent_id)?;
        Some(match agent.title.trim() {
            "" => "Untitled thread".to_string(),
            title => title.to_string(),
        })
    }
}

/// The position of a space's lead in a record's threads.
pub(crate) fn lead_in(record: &PersistedProject, space_id: Uuid) -> Option<usize> {
    space_leads(record, &listed_slots(record))
        .get(&space_id)
        .copied()
}

/// Keep every thread that leads a space titled after it, as a rename
/// names a thread, binding included: a lead archived when its space was
/// renamed comes back under the new name.
pub(crate) fn retitle_leads(record: &mut PersistedProject, space_id: Uuid, name: &str) {
    let title = orchestrator::title(name);
    let leads: Vec<Uuid> = record
        .agents
        .iter()
        .filter(|agent| agent.lead_of == Some(space_id))
        .map(|agent| agent.id)
        .collect();
    for id in leads {
        if let Some(agent) = record.agents.iter_mut().find(|agent| agent.id == id) {
            agent.title = title.clone();
        }
        record.auto_title_pending.retain(|pending| *pending != id);
        local_override(record, id).title = Some(title.clone());
        for binding in &mut record.native_bindings {
            if binding.agent_id == id {
                binding.title = title.clone();
            }
        }
    }
}

/// Make every thread that leads a space an ordinary one.
pub(crate) fn release_leads(record: &mut PersistedProject, space_id: Uuid) {
    for agent in &mut record.agents {
        if agent.lead_of == Some(space_id) {
            release(agent);
        }
    }
}

/// A lead no more: its role, and the brief that told it it had one. A
/// thread told it leads a space, with no space to lead and none of the
/// tools that would, would try to orchestrate. Its conversation, title and
/// every other setting stay as they were, the read tools included.
fn release(agent: &mut Agent) {
    agent.lead_of = None;
    if agent.configuration.system_prompt.as_deref() == Some(orchestrator::BRIEF) {
        agent.configuration.system_prompt = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx_core::Project;
    use fx_store::{AppPaths, PersistedApp, Store};
    use gpui::{AppContext, Entity, TestAppContext};

    const FIRST: Uuid = Uuid::from_u128(1);
    const SECOND: Uuid = Uuid::from_u128(2);

    /// A project of two threads, the first on Claude and open, and a space
    /// "Auth" with the second in it. Each test owns its directory.
    fn seeded(test: &mut TestAppContext) -> (tempfile::TempDir, AppPaths, Entity<AppState>, Uuid) {
        let temp = tempfile::tempdir().unwrap();
        let mut project = Project::new(temp.path().to_string_lossy());
        project.agent_order = vec![FIRST, SECOND];
        let project_id = project.id;
        let mut first = Agent::with_id(FIRST, "First");
        first.configuration.provider_id = Some("claude".into());
        let app = PersistedApp {
            active_project_id: Some(project_id),
            active_agent_id: Some(FIRST),
            projects: vec![PersistedProject {
                project: Some(project),
                agents: vec![first, Agent::with_id(SECOND, "Second")],
                is_expanded: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let paths = AppPaths::at(temp.path().join("state"));
        paths.ensure().unwrap();
        Store::new(paths.clone()).save_app(&app).unwrap();
        let state = test.update(|cx| cx.new(|cx| AppState::restore(paths.clone(), cx)));
        let space = test.update(|cx| {
            state.update(cx, |state, _| {
                state
                    .create_space(project_id, "Auth", Some(SECOND))
                    .unwrap()
            })
        });
        (temp, paths, state, space)
    }

    fn agent(state: &AppState, id: Uuid) -> Agent {
        state.thread(id).unwrap().clone()
    }

    fn overridden_title(state: &AppState, id: Uuid) -> Option<String> {
        state.snapshot.projects[0]
            .thread_overrides
            .iter()
            .find(|entry| entry.agent_id == id)
            .and_then(|entry| entry.title.clone())
    }

    #[test]
    fn a_space_has_no_lead_until_asked_and_then_always_the_same_one() {
        let mut test = TestAppContext::single();
        let (_dir, paths, state, space) = seeded(&mut test);
        let id = test.update(|cx| {
            state.update(cx, |state, _| {
                assert_eq!(state.space_lead(space), None, "nothing made up front");
                let id = state.ensure_space_lead(space).unwrap();
                let lead = agent(state, id);
                assert_eq!(lead.title, "Auth");
                assert_eq!((lead.space, lead.lead_of), (Some(space), Some(space)));
                assert_eq!(
                    lead.configuration.system_prompt.as_deref(),
                    Some(orchestrator::BRIEF)
                );
                assert_eq!(lead.configuration.flowx_tools, Some(true));
                // As a new thread in the project would be: on the provider of
                // the thread it shows.
                assert_eq!(lead.configuration.provider_id.as_deref(), Some("claude"));
                // Named as a rename names a thread, so a sync keeps it.
                assert_eq!(overridden_title(state, id).as_deref(), Some("Auth"));
                assert!(!state.snapshot.projects[0].auto_title_pending.contains(&id));
                // Made, not opened: the caller opens it.
                assert_eq!(state.snapshot.active_agent_id, Some(FIRST));
                assert_eq!(state.ensure_space_lead(space), Some(id));
                assert_eq!(state.snapshot.projects[0].agents.len(), 3);
                id
            })
        });
        // Saved with the write that made it.
        let restored = test.update(|cx| cx.new(|cx| AppState::restore(paths, cx)));
        test.read(|cx| assert_eq!(restored.read(cx).space_lead(space), Some(id)));
    }

    #[test]
    fn data_holding_two_leads_for_a_space_keeps_the_first_and_lists_the_other() {
        let mut test = TestAppContext::single();
        let (_dir, _paths, state, space) = seeded(&mut test);
        test.update(|cx| {
            state.update(cx, |state, _| {
                let first = state.ensure_space_lead(space).unwrap();
                let mut other = Agent::new("Auth (other machine)");
                other.space = Some(space);
                other.lead_of = Some(space);
                let other_id = other.id;
                let record = &mut state.snapshot.projects[0];
                record
                    .project
                    .as_mut()
                    .unwrap()
                    .agent_order
                    .insert(0, other_id);
                record.agents.push(other);
                assert_eq!(state.space_lead(space), Some(first));
                assert_eq!(state.ensure_space_lead(space), Some(first));
                let index = state.snapshot_index();
                assert_eq!(index.space_led_by(first), Some(space));
                assert_eq!(index.space_led_by(other_id), None);
                // The second is an ordinary row in its space.
                let rows: Vec<Uuid> = state.active_agents().iter().map(|a| a.id).collect();
                assert!(rows.contains(&other_id) && !rows.contains(&first));
            })
        });
    }

    #[test]
    fn renaming_a_space_renames_its_lead_and_its_composer() {
        let mut test = TestAppContext::single();
        let (_dir, _paths, state, space) = seeded(&mut test);
        test.update(|cx| {
            state.update(cx, |state, cx| {
                let id = state.ensure_space_lead(space).unwrap();
                assert_eq!(
                    state.composer_placeholder(id),
                    "Tell the Auth lead what to get done…"
                );
                assert_eq!(
                    state.composer_placeholder(SECOND),
                    crate::state::COMPOSER_PLACEHOLDER
                );
                let field = state.draft(id, cx);
                assert_eq!(
                    field.read(cx).placeholder_text(),
                    "Tell the Auth lead what to get done…"
                );
                let ordinary = state.draft(FIRST, cx);
                assert_eq!(
                    ordinary.read(cx).placeholder_text(),
                    crate::state::COMPOSER_PLACEHOLDER
                );

                assert!(state.rename_space(space, "  Billing  "));
                let lead = agent(state, id);
                assert_eq!(lead.title, "Billing");
                assert_eq!(overridden_title(state, id).as_deref(), Some("Billing"));
                assert_eq!(
                    state.composer_placeholder(id),
                    "Tell the Billing lead what to get done…"
                );
                // The thread in the space keeps its own name.
                assert_eq!(agent(state, SECOND).title, "Second");
            })
        });
    }

    #[test]
    fn deleting_a_space_keeps_its_lead_as_an_ordinary_thread() {
        let mut test = TestAppContext::single();
        let (_dir, _paths, state, space) = seeded(&mut test);
        test.update(|cx| {
            state.update(cx, |state, _| {
                let id = state.ensure_space_lead(space).unwrap();
                assert!(state.delete_space(space));
                let lead = agent(state, id);
                assert_eq!((lead.space, lead.lead_of), (None, None));
                assert_eq!(lead.title, "Auth");
                // Told it leads a space, it would try to orchestrate.
                assert_eq!(lead.configuration.system_prompt, None);
                assert_eq!(lead.configuration.flowx_tools, Some(true));
                assert!(state.conversations.contains_key(&id), "nothing is lost");
                // Listed as a row now, and numbered.
                assert!(state.active_agents().iter().any(|agent| agent.id == id));
            })
        });
    }

    #[test]
    fn a_lead_put_away_gives_its_role_to_the_next() {
        let mut test = TestAppContext::single();
        let (_dir, _paths, state, space) = seeded(&mut test);
        test.update(|cx| {
            state.update(cx, |state, _| {
                let old = state.ensure_space_lead(space).unwrap();
                assert!(state.set_thread_archived(old, true));
                assert_eq!(state.space_lead(space), None);
                let new = state.ensure_space_lead(space).unwrap();
                assert_ne!(new, old);
                assert_eq!(agent(state, old).lead_of, None);
                // Restored, it is an ordinary thread in the space, not a
                // second lead.
                assert!(state.set_thread_archived(old, false));
                assert_eq!(state.space_lead(space), Some(new));
                assert_eq!(agent(state, old).space, Some(space));
                assert!(state.active_agents().iter().any(|agent| agent.id == old));
            })
        });
    }

    #[test]
    fn a_lead_stays_in_its_space_and_a_reset_hands_its_role_on() {
        let mut test = TestAppContext::single();
        let (_dir, _paths, state, space) = seeded(&mut test);
        test.update(|cx| {
            state.update(cx, |state, _| {
                let id = state.ensure_space_lead(space).unwrap();
                assert!(!state.move_thread_to_space(id, None));
                assert!(state.last_error.as_deref().unwrap().contains("stays"));
                assert!(state.move_thread_to_space(id, Some(space)), "a no-op");
                // An ordinary thread still moves.
                assert!(state.move_thread_to_space(SECOND, None));

                // What Reset does to a thread its provider knows: a fresh
                // one in its place, which leads from now on.
                state.select_agent(id);
                let fresh = state.create_thread().unwrap();
                state.hand_over_lead(id, fresh);
                assert_eq!(state.space_lead(space), Some(fresh));
                let lead = agent(state, fresh);
                assert_eq!((lead.title.as_str(), lead.space), ("Auth", Some(space)));
                assert!(
                    !state.snapshot.projects[0]
                        .auto_title_pending
                        .contains(&fresh)
                );
                assert_eq!(overridden_title(state, fresh).as_deref(), Some("Auth"));
                let old = agent(state, id);
                assert_eq!((old.lead_of, old.space), (None, Some(space)));
                assert_eq!(old.configuration.system_prompt, None);
                // Handing over from a thread that leads nothing does nothing.
                state.hand_over_lead(SECOND, id);
                assert_eq!(state.space_lead(space), Some(fresh));
            })
        });
    }

    #[test]
    fn a_codex_sections_space_keeps_its_header_while_it_has_a_lead() {
        let mut test = TestAppContext::single();
        let (_dir, _paths, state, _space) = seeded(&mut test);
        test.update(|cx| {
            state.update(cx, |state, _| {
                let mut mirror = fx_core::Space::new("Pinned");
                mirror.codex_section = Some("pinned".into());
                let mirror_id = mirror.id;
                state.snapshot.projects[0]
                    .spaces
                    .push(fx_store::PersistedSpace::new(mirror));
                let drawn = |state: &AppState| {
                    state.snapshot_index().records[0]
                        .spaces
                        .iter()
                        .any(|space| space.id == mirror_id)
                };
                assert!(!drawn(state), "a mirror that lists nothing is not drawn");
                let id = state.ensure_space_lead(mirror_id).unwrap();
                assert!(drawn(state));
                assert_eq!(state.snapshot_index().space_led_by(id), Some(mirror_id));
            })
        });
    }
}
