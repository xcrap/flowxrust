//! A review of the checkout's changes by the other provider.
//!
//! Started from the inspector's Review button, which offers both providers
//! with the one that did not write the changes first. The review is an
//! ordinary thread at Read Only beside the one it reviews, opened with
//! [`review::PROMPT`] under [`review::BRIEF`]; it streams and is stopped like
//! any turn. When it has answered, the row above its composer sends the
//! findings into the reviewed thread's draft, and that thread is shown with
//! its composer focused. Nothing is sent to the reviewed thread's agent until
//! the reader sends it: FlowX never sends a prompt on its own.

use fx_core::Agent;
use fx_core::policy::review;
use fx_design::components::{MenuItem, MenuModel, MenuSection};
use gpui::{App, Entity, SharedString, Window};
use uuid::Uuid;

use crate::state::AppState;

/// The owner of the inspector's review menu, and of the choices it routes.
pub(crate) const MENU_OWNER: &str = "review";

/// The two reviewers, the one that did not write `author`'s changes first.
pub(crate) fn menu(author: &str) -> MenuModel {
    let first = review::reviewer_for(author);
    let second = review::reviewer_for(first);
    let item = |provider: &str, subtitle: &str| {
        MenuItem::new(
            provider,
            format!("Review with {}", review::provider_label(provider)),
        )
        .subtitle(subtitle)
    };
    MenuModel::new(vec![MenuSection::new(
        "reviewers",
        vec![
            item(first, "A second opinion, read only"),
            item(second, "The same provider, read only"),
        ],
    )])
}

/// The provider whose changes a review from here would read: the active
/// thread's, or the author of what it reviews when it is a review itself.
pub(crate) fn author(state: &AppState) -> String {
    let Some(agent) = state.active_agent() else {
        return "codex".into();
    };
    let author = agent
        .review_of
        .and_then(|source| state.thread(source).cloned())
        .unwrap_or(agent);
    provider_of(state, &author)
}

fn provider_of(state: &AppState, agent: &Agent) -> String {
    agent
        .configuration
        .provider_id
        .clone()
        .or_else(|| state.native_defaults(agent.id).provider_id)
        .unwrap_or_else(|| "codex".into())
}

/// Start a review by `provider` of the active thread's checkout, and show it.
pub(crate) fn start(state: &Entity<AppState>, provider: &str, cx: &mut App) {
    if !matches!(provider, "codex" | "claude") {
        return;
    }
    let created = state.update(cx, |state, cx| {
        state.save_all(cx);
        state.create_review_thread(provider)
    });
    let id = match created {
        Ok(id) => id,
        Err(error) => {
            state.update(cx, |state, cx| {
                state.last_error = Some(error);
                cx.notify();
            });
            return;
        }
    };
    let (agent, root) = {
        let state = state.read(cx);
        (state.thread(id).cloned(), state.project_root_for(id))
    };
    if let (Some(agent), Some(root)) = (agent, root) {
        crate::start_turn_with(state, &root, agent, review::PROMPT.into(), Vec::new(), cx);
    }
    state.update(cx, |_, cx| cx.notify());
}

/// Put a review's findings in the reviewed thread's draft, show that thread
/// and focus its composer. When there is nothing to send, the review's own
/// composer says why.
pub(crate) fn send_back(
    state: &Entity<AppState>,
    review_id: Uuid,
    window: &mut Window,
    cx: &mut App,
) {
    let handoff = state.read(cx).review_handoff(review_id);
    let (source, text) = match handoff {
        Ok(handoff) => handoff,
        Err(error) => {
            state.update(cx, |state, cx| {
                state.set_composer_notice(review_id, Some(error));
                cx.notify();
            });
            return;
        }
    };
    let field = state.update(cx, |state, cx| {
        let field = state.append_prompt_context(source, &text, cx)?;
        state.open_thread(source, cx);
        state.save();
        cx.notify();
        Some(field)
    });
    match field {
        Some(field) => window.focus(&field.read(cx).focus_handle()),
        // The reviewed thread's composer could not take it, and says why
        // there; this one says where to look.
        None => state.update(cx, |state, cx| {
            state.set_composer_notice(
                review_id,
                Some("The findings could not be added to the reviewed thread's draft.".into()),
            );
            cx.notify();
        }),
    }
}

/// The row above a review's composer, or `None` for a thread that is not one.
pub(crate) fn row(state: &AppState, agent: &Agent) -> Option<crate::composer::ReviewRow> {
    let source = agent.review_of?;
    let reviewed = state.thread(source);
    Some(crate::composer::ReviewRow {
        source_title: SharedString::from(
            reviewed
                .map(|source| source.title.clone())
                .unwrap_or_else(|| "a deleted thread".into()),
        ),
        can_send: reviewed.is_some() && state.review_handoff(agent.id).is_ok(),
        source_exists: reviewed.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx_agent::conversation::Phase;
    use fx_core::models::{ConversationMessage, MessageRole};
    use fx_core::{AgentAccess, Project};
    use fx_store::{AppPaths, PersistedApp, PersistedProject};
    use gpui::AppContext;

    const AUTHOR: Uuid = Uuid::from_u128(1);
    const OTHER: Uuid = Uuid::from_u128(2);

    /// A project with two threads, the first on Codex and in a worktree of
    /// the project, selected. Each test owns its directory.
    fn seeded(cx: &mut App) -> (tempfile::TempDir, Entity<AppState>) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().to_string_lossy().into_owned();
        let mut project = Project::new(&root);
        project.agent_order = vec![AUTHOR, OTHER];
        let mut author = Agent::with_id(AUTHOR, "Parser rewrite");
        author.configuration.provider_id = Some("codex".into());
        author.workspace = Some(fx_core::models::agent::ChatWorkspace {
            path: format!("{root}/worktree"),
            repository: root.clone(),
            common_dir: format!("{root}/.git"),
            branch: "flowx/parser".into(),
            base_commit: "fixture".into(),
        });
        let app = PersistedApp {
            active_project_id: Some(project.id),
            active_agent_id: Some(AUTHOR),
            projects: vec![PersistedProject {
                project: Some(project),
                agents: vec![author, Agent::with_id(OTHER, "Other")],
                is_expanded: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let paths = AppPaths::at(temp.path().join("state"));
        paths.ensure().unwrap();
        fx_store::Store::new(paths.clone()).save_app(&app).unwrap();
        (temp, cx.new(|cx| AppState::restore(paths, cx)))
    }

    fn answer(state: &AppState, review: Uuid, reply: &str) {
        let conversation = state.conversation_by_agent(review).unwrap();
        let mut conversation = conversation.lock().unwrap();
        conversation.append(ConversationMessage::text(MessageRole::User, review::PROMPT));
        conversation.append(ConversationMessage::text(
            MessageRole::Assistant,
            "Let me read the diff.",
        ));
        conversation.append(ConversationMessage::text(MessageRole::Assistant, reply));
    }

    #[test]
    fn the_menu_offers_the_other_provider_first() {
        for (author, first, second) in [("claude", "codex", "claude"), ("codex", "claude", "codex")]
        {
            let model = menu(author);
            let ids: Vec<_> = model
                .sections()
                .iter()
                .flat_map(|section| section.items.iter().map(|item| item.id.as_str()))
                .collect();
            assert_eq!(ids, [first, second], "{author}");
        }
    }

    #[test]
    fn a_review_is_a_read_only_thread_in_the_folder_it_reviews() {
        let test = gpui::TestAppContext::single();
        let (dir, state) = test.update(seeded);
        test.update(|cx| {
            state.update(cx, |state, _| {
                assert_eq!(author(state), "codex");
                let id = state.create_review_thread("claude").unwrap();
                let review = state.thread(id).unwrap().clone();
                assert_eq!(review.review_of, Some(AUTHOR));
                assert_eq!(review.title, "Review: Parser rewrite");
                assert_eq!(review.workspace, state.thread(AUTHOR).unwrap().workspace);
                assert_eq!(review.configuration.provider_id.as_deref(), Some("claude"));
                assert_eq!(
                    review.configuration.agent_access,
                    Some(AgentAccess::ReadOnly)
                );
                assert_eq!(
                    review.configuration.system_prompt.as_deref(),
                    Some(review::BRIEF)
                );
                assert_eq!(state.snapshot.active_agent_id, Some(id));
                // Named after what it reviews, so not renamed after its prompt.
                assert!(!state.snapshot.projects[0].auto_title_pending.contains(&id));
                // From the review, the author is still the reviewed thread's.
                assert_eq!(author(state), "codex");
                let again = state.create_review_thread("codex").unwrap();
                assert_eq!(state.thread(again).unwrap().review_of, Some(AUTHOR));
                // Saved before it existed in memory: a relaunch has it.
                let saved = fx_store::Store::new(AppPaths::at(dir.path().join("state")))
                    .load_app()
                    .unwrap();
                assert!(
                    saved.projects[0]
                        .agents
                        .iter()
                        .any(|agent| agent.id == id && agent.review_of == Some(AUTHOR))
                );
            })
        });
    }

    #[test]
    fn the_findings_go_back_only_once_the_review_has_answered() {
        let test = gpui::TestAppContext::single();
        let (_dir, state) = test.update(seeded);
        test.update(|cx| {
            state.update(cx, |state, _| {
                let id = state.create_review_thread("claude").unwrap();
                assert_eq!(
                    state.review_handoff(id).unwrap_err(),
                    "The review has not answered yet."
                );
                answer(state, id, "- [P1] Off by one — stats.py:3");
                let (source, text) = state.review_handoff(id).unwrap();
                assert_eq!(source, AUTHOR);
                assert!(text.starts_with("Claude reviewed the uncommitted changes"));
                assert!(text.contains("- [P1] Off by one — stats.py:3"));
                assert!(!text.contains("Let me read"), "{text}");
                // A turn under way is a follow-up whose answer is not in yet.
                let conversation = state.conversation_by_agent(id).unwrap();
                conversation.lock().unwrap().set_phase(Phase::Running);
                assert!(state.review_handoff(id).is_err());
                conversation.lock().unwrap().set_phase(Phase::Idle);
                assert!(state.review_handoff(id).is_ok());
                assert!(state.review_handoff(OTHER).is_err(), "not a review");
                assert!(state.delete_thread(AUTHOR));
                assert_eq!(
                    state.review_handoff(id).unwrap_err(),
                    "The reviewed thread no longer exists."
                );
                let row = row(state, state.thread(id).unwrap()).unwrap();
                assert!(!row.source_exists && !row.can_send);
            })
        });
    }

    #[test]
    fn a_review_draws_what_it_reviews_above_its_composer() {
        let mut test = gpui::TestAppContext::single();
        let (_dir, state) = test.update(seeded);
        test.update(|cx| crate::AppHandle::install(state.clone(), cx));
        test.update(|cx| {
            state.update(cx, |state, _| state.create_review_thread("claude").unwrap())
        });
        let for_view = state.clone();
        let window = test.add_window(move |_, _| crate::FlowX::new(for_view));
        test.refresh().unwrap();
        let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
        assert!(visual.debug_bounds("composer.review").is_some());
        assert!(
            visual
                .debug_bounds("compact-action:Send findings")
                .is_some()
        );
        // An ordinary thread has no such row: its own state says so, since a
        // selector drawn once stays visible to `debug_bounds`.
        test.update(|cx| {
            let state = state.read(cx);
            assert!(row(state, state.thread(OTHER).unwrap()).is_none());
        });
    }

    struct Blank;
    impl gpui::Render for Blank {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    #[test]
    fn sending_the_findings_fills_the_reviewed_draft_and_shows_it() {
        let mut test = gpui::TestAppContext::single();
        let (_dir, state) = test.update(seeded);
        let window = test.add_window(|_, _| Blank);
        let review = test.update(|cx| {
            state.update(cx, |state, cx| {
                let id = state.create_review_thread("claude").unwrap();
                // The reader's own words in the reviewed draft stay first.
                let draft = state.draft(AUTHOR, cx);
                draft.update(cx, |field, cx| field.restore_value("My note", cx));
                answer(state, id, "- [P2] Leak — a.rs:9");
                id
            })
        });
        window
            .update(&mut test, |_, window, cx| {
                send_back(&state, review, window, cx);
            })
            .unwrap();
        test.update(|cx| {
            let draft = state.update(cx, |state, cx| state.draft(AUTHOR, cx));
            let value = draft.read(cx).value().to_string();
            assert!(value.starts_with("My note\n\nClaude reviewed"), "{value}");
            assert!(value.contains("- [P2] Leak — a.rs:9"), "{value}");
            assert_eq!(state.read(cx).snapshot.active_agent_id, Some(AUTHOR));
        });
    }
}
