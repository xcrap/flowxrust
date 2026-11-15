//! Thread-scoped rename and deletion dialogs.

use fx_design::components::{
    ButtonStyle, DialogAction, DialogClose, DialogHeader, DialogWidth, cycle_focus, dialog,
    dialog_body, dialog_footer, dialog_scrim,
};
use fx_design::styled::FxStyled;
use fx_design::theme::{Colors, global};
use gpui::prelude::*;
use gpui::{App, Context, Entity, FocusHandle, MouseButton, Render, Subscription, Window, div};
use uuid::Uuid;

use crate::{AppState, text_field::TextField, workspace_ui};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThreadAction {
    Rename,
    Delete,
}

pub struct ThreadDialog {
    state: Entity<AppState>,
    agent_id: Uuid,
    action: ThreadAction,
    title: String,
    field: Entity<TextField>,
    focus: FocusHandle,
    cancel_focus: FocusHandle,
    confirm_focus: FocusHandle,
    close_focus: FocusHandle,
    error: Option<String>,
    settled: bool,
    _observation: Subscription,
}

pub fn open(
    state: Entity<AppState>,
    agent_id: Uuid,
    action: ThreadAction,
    window: &mut Window,
    cx: &mut App,
) {
    if workspace_ui::has_modal(&state, cx) {
        return;
    }
    let Some(agent) = state
        .read(cx)
        .snapshot
        .projects
        .iter()
        .flat_map(|record| &record.agents)
        .find(|agent| agent.id == agent_id)
        .cloned()
    else {
        return;
    };
    if state.read(cx).is_thread_busy(agent_id) {
        return;
    }
    let menu = state.read(cx).menu_layer.clone();
    menu.update(cx, |menu, cx| {
        menu.dismiss();
        cx.notify();
    });
    let dialog = cx.new(|cx| {
        let field = cx.new(|cx| {
            let mut field = TextField::new(cx).placeholder("Thread name").max_len(200);
            field.set_value(agent.title.clone(), cx);
            field
        });
        let observation = cx.observe(&field, |_, _, cx| cx.notify());
        ThreadDialog {
            state,
            agent_id,
            action,
            title: agent.title,
            field,
            focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            confirm_focus: cx.focus_handle(),
            close_focus: cx.focus_handle(),
            error: None,
            settled: false,
            _observation: observation,
        }
    });
    let focus = if action == ThreadAction::Rename {
        dialog.read(cx).field.read(cx).focus_handle()
    } else {
        dialog.read(cx).focus.clone()
    };
    workspace_ui::handle(cx).update(cx, |ui, cx| {
        ui.thread_dialog = Some(dialog);
        cx.notify();
    });
    focus.focus(window);
}

impl ThreadDialog {
    fn can_confirm(&self, cx: &App) -> bool {
        !self.settled
            && !self.state.read(cx).is_thread_busy(self.agent_id)
            && (self.action == ThreadAction::Delete
                || !self.field.read(cx).value().trim().is_empty())
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settled = true;
        workspace_ui::handle(cx).update(cx, |ui, cx| {
            ui.thread_dialog = None;
            cx.notify();
        });
        window.blur();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_confirm(cx) {
            return;
        }
        let title = self.field.read(cx).value().to_string();
        let success = self.state.update(cx, |state, cx| {
            state.save_all(cx);
            let success = match self.action {
                ThreadAction::Rename => state.rename_thread(self.agent_id, &title),
                ThreadAction::Delete => state.delete_thread(self.agent_id),
            };
            cx.notify();
            success
        });
        if success {
            self.close(window, cx);
        } else {
            self.error = Some(
                self.state
                    .read(cx)
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "This thread is no longer available.".into()),
            );
            cx.notify();
        }
    }
}

impl Render for ThreadDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = global::current();
        let deleting = self.action == ThreadAction::Delete;
        let this = cx.entity();
        let can_confirm = self.can_confirm(cx);
        let mut body = dialog_body();
        if deleting {
            body = body
                .child(div().fx_body().text_color(Colors::fg(&theme)).child(self.title.clone()))
                .child(
                    div()
                        .fx_caption()
                        .text_color(Colors::fg_secondary(&theme))
                        .child("This deletes the saved conversation and draft from FlowX. Project files, worktrees, branches and the provider's own history are kept. This cannot be undone."),
                );
        } else {
            body = body.child(self.field.clone());
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .fx_caption()
                    .text_color(Colors::fg(&theme))
                    .child(error.clone()),
            );
        }
        let (cancel, confirm, close) = (this.clone(), this.clone(), this.clone());
        let header = DialogHeader::new(if deleting {
            "Delete thread?"
        } else {
            "Rename thread"
        })
        .close(
            DialogClose::new("Close", move |window, cx| {
                close.update(cx, |dialog, cx| dialog.close(window, cx));
            })
            .focus(&self.close_focus),
        );
        let footer = dialog_footer(
            None,
            [
                DialogAction::new("Cancel", ButtonStyle::Secondary, move |window, cx| {
                    cancel.update(cx, |dialog, cx| dialog.close(window, cx));
                })
                .focus(&self.cancel_focus),
                DialogAction::new(
                    if deleting { "Delete thread" } else { "Save" },
                    if deleting {
                        ButtonStyle::Danger
                    } else {
                        ButtonStyle::Primary
                    },
                    move |window, cx| {
                        confirm.update(cx, |dialog, cx| dialog.confirm(window, cx));
                    },
                )
                .enabled(can_confirm)
                .focus(&self.confirm_focus),
            ],
        );
        let mut order = Vec::new();
        if !deleting {
            order.push(self.field.read(cx).focus_handle());
        }
        order.push(self.cancel_focus.clone());
        if can_confirm {
            order.push(self.confirm_focus.clone());
        }
        order.push(self.close_focus.clone());
        dialog_scrim("thread-dialog")
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .capture_key_down(move |event, window, cx| {
                if cycle_focus(&order, event, window) {
                    cx.stop_propagation();
                    return;
                }
                let escape = event.keystroke.key == "escape";
                let submit = event.keystroke.key == "enter" && !deleting;
                if escape || submit {
                    cx.stop_propagation();
                    this.update(cx, |dialog, cx| {
                        if escape {
                            dialog.close(window, cx);
                        } else {
                            dialog.confirm(window, cx);
                        }
                    });
                }
            })
            .child(dialog(DialogWidth::Compact, header, body, footer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fx_core::{Agent, Project};
    use fx_store::{AppPaths, PersistedApp, PersistedProject, Store};
    use gpui::TestAppContext;

    fn seeded(test: &mut TestAppContext, paths: &AppPaths) -> (Entity<AppState>, Uuid, Uuid) {
        let first = Agent::new("First");
        let second = Agent::new("Second");
        let mut project = Project::new(paths.root().to_string_lossy());
        project.agent_order = vec![first.id, second.id];
        let snapshot = PersistedApp {
            active_project_id: Some(project.id),
            active_agent_id: Some(first.id),
            projects: vec![PersistedProject {
                project: Some(project),
                agents: vec![first.clone(), second.clone()],
                is_expanded: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        Store::new(paths.clone()).save_app(&snapshot).unwrap();
        let state = test.update(|cx| cx.new(|cx| AppState::restore(paths.clone(), cx)));
        test.update(|cx| {
            crate::AppHandle::install(state.clone(), cx);
            cx.bind_keys([
                gpui::KeyBinding::new("cmd-,", crate::ShowSettings, None),
                gpui::KeyBinding::new("cmd-o", crate::NewProject, None),
                gpui::KeyBinding::new("cmd-n", crate::NewThread, None),
            ]);
        });
        (state, first.id, second.id)
    }

    #[test]
    fn rename_menu_targets_its_thread_and_enter_saves_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("state"));
        let mut test = TestAppContext::single();
        let (state, first, second) = seeded(&mut test, &paths);
        let view_state = state.clone();
        let window = test.add_window(move |_, _| crate::FlowX::new(view_state));
        test.refresh().unwrap();
        window
            .update(&mut test, |_, window, cx| {
                crate::dispatch_menu_choice(&format!("thread:{second}"), "rename", window, cx);
                let dialog = workspace_ui::handle(cx)
                    .read(cx)
                    .thread_dialog
                    .clone()
                    .unwrap();
                let field = dialog.read(cx).field.clone();
                field.update(cx, |field, cx| field.set_value("Renamed second", cx));
            })
            .unwrap();
        test.refresh().unwrap();
        test.simulate_keystrokes(window.into(), "cmd-, cmd-o cmd-n");
        test.update(|cx| {
            let ui = workspace_ui::handle(cx);
            assert!(ui.read(cx).settings.is_none());
            assert!(state.read(cx).picker.is_none());
            assert_eq!(state.read(cx).snapshot.projects[0].agents.len(), 2);
        });
        test.simulate_keystrokes(window.into(), "enter");
        test.update(|cx| {
            assert!(workspace_ui::handle(cx).read(cx).thread_dialog.is_none());
            assert_eq!(state.read(cx).snapshot.active_agent_id, Some(first));
        });
        let saved = Store::new(paths).load_app().unwrap();
        assert_eq!(saved.projects[0].agents[0].title, "First");
        assert_eq!(saved.projects[0].agents[1].title, "Renamed second");
    }

    #[test]
    fn delete_requires_explicit_confirmation_and_escape_preserves_the_thread() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("state"));
        let mut test = TestAppContext::single();
        let (state, _, second) = seeded(&mut test, &paths);
        let view_state = state.clone();
        let window = test.add_window(move |_, _| crate::FlowX::new(view_state));
        test.refresh().unwrap();
        window
            .update(&mut test, |_, window, cx| {
                crate::dispatch_menu_choice(&format!("thread:{second}"), "delete", window, cx);
            })
            .unwrap();
        test.refresh().unwrap();
        test.simulate_keystrokes(window.into(), "enter");
        test.update(|cx| {
            assert!(workspace_ui::handle(cx).read(cx).thread_dialog.is_some());
            assert_eq!(state.read(cx).snapshot.projects[0].agents.len(), 2);
        });
        test.simulate_keystrokes(window.into(), "escape");
        test.update(|cx| assert!(workspace_ui::handle(cx).read(cx).thread_dialog.is_none()));
        assert_eq!(
            Store::new(paths).load_app().unwrap().projects[0]
                .agents
                .len(),
            2
        );
    }

    #[test]
    fn tab_reaches_the_dialogs_buttons_and_space_presses_the_focused_one() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("state"));
        let mut test = TestAppContext::single();
        let (state, _, second) = seeded(&mut test, &paths);
        let view_state = state.clone();
        let window = test.add_window(move |_, _| crate::FlowX::new(view_state));
        test.refresh().unwrap();
        window
            .update(&mut test, |_, window, cx| {
                crate::dispatch_menu_choice(&format!("thread:{second}"), "delete", window, cx);
            })
            .unwrap();
        test.refresh().unwrap();
        let dialog = test.update(|cx| {
            workspace_ui::handle(cx)
                .read(cx)
                .thread_dialog
                .clone()
                .unwrap()
        });
        let focused = |test: &mut TestAppContext, pick: fn(&ThreadDialog) -> &FocusHandle| {
            window
                .update(test, |_, window, cx| {
                    pick(dialog.read(cx)).is_focused(window)
                })
                .unwrap()
        };
        test.simulate_keystrokes(window.into(), "tab");
        assert!(
            focused(&mut test, |d| &d.cancel_focus),
            "Tab starts at Cancel"
        );
        test.simulate_keystrokes(window.into(), "tab");
        assert!(focused(&mut test, |d| &d.confirm_focus));
        test.simulate_keystrokes(window.into(), "tab tab");
        assert!(
            focused(&mut test, |d| &d.cancel_focus),
            "Tab wraps past the close button"
        );
        test.simulate_keystrokes(window.into(), "shift-tab");
        assert!(focused(&mut test, |d| &d.close_focus));
        test.simulate_keystrokes(window.into(), "shift-tab shift-tab");
        assert!(focused(&mut test, |d| &d.cancel_focus));
        // Space presses Cancel, never the destructive default.
        test.simulate_keystrokes(window.into(), "space");
        test.update(|cx| {
            assert!(workspace_ui::handle(cx).read(cx).thread_dialog.is_none());
            assert_eq!(state.read(cx).snapshot.projects[0].agents.len(), 2);
        });
    }

    #[test]
    fn confirmed_delete_cleans_up_the_target_and_its_observers() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("state"));
        let mut test = TestAppContext::single();
        let (state, first, second) = seeded(&mut test, &paths);
        test.update(|cx| state.update(cx, |state, _| state.select_agent(second)));
        let view_state = state.clone();
        let window = test.add_window(move |_, _| crate::FlowX::new(view_state));
        test.refresh().unwrap();
        test.update(|cx| {
            state.update(cx, |state, cx| {
                state.select_agent(first);
                cx.notify();
            })
        });
        test.refresh().unwrap();
        window
            .update(&mut test, |view, window, cx| {
                assert!(view.draft_values.contains_key(&second));
                crate::dispatch_menu_choice(&format!("thread:{second}"), "delete", window, cx);
                let dialog = workspace_ui::handle(cx)
                    .read(cx)
                    .thread_dialog
                    .clone()
                    .unwrap();
                dialog.update(cx, |dialog, cx| dialog.confirm(window, cx));
            })
            .unwrap();
        test.refresh().unwrap();
        window
            .update(&mut test, |view, _, cx| {
                assert!(!view.draft_values.contains_key(&second));
                assert!(!view.draft_subscriptions.contains_key(&second));
                assert_eq!(state.read(cx).snapshot.active_agent_id, Some(first));
                assert!(workspace_ui::handle(cx).read(cx).thread_dialog.is_none());
            })
            .unwrap();
        let saved = Store::new(paths).load_app().unwrap();
        assert_eq!(saved.projects[0].agents.len(), 1);
        assert_eq!(saved.projects[0].agents[0].id, first);
    }
}
