//! Thread-scoped rename and deletion dialogs, and naming a space.

use fx_design::components::{
    ButtonStyle, DialogAction, DialogClose, DialogHeader, DialogWidth, NoticeTone, cycle_focus,
    dialog, dialog_body, dialog_footer, dialog_scrim, notice,
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

/// What a space's name is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceAction {
    /// A new space in `project`, which takes `thread` in when one is given:
    /// "New space…" from a thread's "Move to space".
    Create {
        project: Uuid,
        thread: Option<Uuid>,
    },
    Rename {
        space: Uuid,
    },
}

/// What the dialog acts on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Subject {
    Thread(Uuid, ThreadAction),
    Space(SpaceAction),
}

/// The one name-or-confirm dialog, for a thread or a space. One slot, so
/// opening either while the other is up is refused as any modal is.
pub struct ThreadDialog {
    state: Entity<AppState>,
    subject: Subject,
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
            subject: Subject::Thread(agent_id, action),
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

/// Ask for a space's name: a new space's, or a new name for one.
pub fn open_space(state: Entity<AppState>, action: SpaceAction, window: &mut Window, cx: &mut App) {
    if workspace_ui::has_modal(&state, cx) {
        return;
    }
    let current = match action {
        SpaceAction::Create { project, .. } => {
            if state.read(cx).project(project).is_none() {
                return;
            }
            String::new()
        }
        SpaceAction::Rename { space } => match state.read(cx).space(space) {
            Some((_, held)) => held.space.name.clone(),
            None => return,
        },
    };
    let menu = state.read(cx).menu_layer.clone();
    menu.update(cx, |menu, cx| {
        menu.dismiss();
        cx.notify();
    });
    let dialog = cx.new(|cx| {
        let field = cx.new(|cx| {
            let mut field = TextField::new(cx)
                .placeholder("Space name")
                .max_len(fx_core::Space::MAX_NAME);
            field.set_value(current.clone(), cx);
            field
        });
        let observation = cx.observe(&field, |_, _, cx| cx.notify());
        ThreadDialog {
            state,
            subject: Subject::Space(action),
            title: current,
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
    let focus = dialog.read(cx).field.read(cx).focus_handle();
    workspace_ui::handle(cx).update(cx, |ui, cx| {
        ui.thread_dialog = Some(dialog);
        cx.notify();
    });
    focus.focus(window);
}

impl ThreadDialog {
    fn deleting(&self) -> bool {
        matches!(self.subject, Subject::Thread(_, ThreadAction::Delete))
    }

    fn can_confirm(&self, cx: &App) -> bool {
        if self.settled {
            return false;
        }
        let named = !self.field.read(cx).value().trim().is_empty();
        match self.subject {
            Subject::Thread(agent, action) => {
                !self.state.read(cx).is_thread_busy(agent)
                    && (action == ThreadAction::Delete || named)
            }
            Subject::Space(_) => named,
        }
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
        let subject = self.subject;
        let success = self.state.update(cx, |state, cx| {
            state.save_all(cx);
            let success = match subject {
                Subject::Thread(agent, ThreadAction::Rename) => state.rename_thread(agent, &title),
                Subject::Thread(agent, ThreadAction::Delete) => state.delete_thread(agent),
                Subject::Space(SpaceAction::Create { project, thread }) => {
                    state.create_space(project, &title, thread).is_some()
                }
                Subject::Space(SpaceAction::Rename { space }) => state.rename_space(space, &title),
            };
            cx.notify();
            success
        });
        if success {
            self.close(window, cx);
        } else {
            self.error = Some(self.state.read(cx).last_error.clone().unwrap_or_else(|| {
                match subject {
                    Subject::Thread(..) => "This thread is no longer available.",
                    Subject::Space(_) => "This space could not be saved.",
                }
                .into()
            }));
            cx.notify();
        }
    }
}

impl Render for ThreadDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = global::current();
        let deleting = self.deleting();
        let this = cx.entity();
        let (heading, confirm_label) = match self.subject {
            Subject::Thread(_, ThreadAction::Delete) => ("Delete thread?", "Delete thread"),
            Subject::Thread(_, ThreadAction::Rename) => ("Rename thread", "Save"),
            Subject::Space(SpaceAction::Create { .. }) => ("New space", "Create space"),
            Subject::Space(SpaceAction::Rename { .. }) => ("Rename space", "Save"),
        };
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
            body = body.child(notice(NoticeTone::Error, error.clone()));
        }
        let (cancel, confirm, close) = (this.clone(), this.clone(), this.clone());
        let header = DialogHeader::new(heading).close(
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
                    confirm_label,
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
    fn spaces_are_made_moved_into_renamed_coloured_and_deleted_from_their_menus() {
        use fx_design::components::{MenuItem, MenuModel, MenuSection};
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::at(dir.path().join("state"));
        let mut test = TestAppContext::single();
        let (state, first, second) = seeded(&mut test, &paths);
        let project = test.read(|cx| state.read(cx).snapshot.active_project_id.unwrap());
        let view_state = state.clone();
        let window = test.add_window(move |_, _| crate::FlowX::new(view_state));
        test.refresh().unwrap();
        let choose = |test: &mut TestAppContext, owner: String, chosen: &str| {
            window
                .update(test, |_, window, cx| {
                    crate::dispatch_menu_choice(&owner, chosen, window, cx);
                })
                .unwrap();
            test.refresh().unwrap();
        };
        let dialog = |test: &mut TestAppContext| {
            test.update(|cx| workspace_ui::handle(cx).read(cx).thread_dialog.clone())
        };
        let name = |test: &mut TestAppContext, text: &str| {
            let entity = dialog(test).unwrap();
            let field = test.read(|cx| entity.read(cx).field.clone());
            test.update(|cx| field.update(cx, |field, cx| field.set_value(text, cx)));
            test.refresh().unwrap();
        };
        let owner = |test: &TestAppContext| {
            test.read(|cx| {
                state
                    .read(cx)
                    .menu_layer
                    .read(cx)
                    .owner()
                    .map(str::to_owned)
            })
        };

        // From the project's menu, named in the dialog. Unnamed, it waits.
        choose(&mut test, format!("project:{project}"), "new-space");
        test.simulate_keystrokes(window.into(), "enter");
        assert!(dialog(&mut test).is_some());
        assert!(test.read(|cx| state.read(cx).spaces_of(project).is_empty()));
        name(&mut test, "Auth");
        test.simulate_keystrokes(window.into(), "enter");
        assert!(dialog(&mut test).is_none());
        let space = test.read(|cx| state.read(cx).spaces_of(project)[0].space.clone());
        assert_eq!(space.name, "Auth");

        // A thread's "Move to space" opens the picker where its menu was,
        // and Right opens it as Return does.
        let at = gpui::point(gpui::px(30.0), gpui::px(60.0));
        test.update(|cx| {
            let layer = state.read(cx).menu_layer.clone();
            layer.update(cx, |layer, cx| {
                let mut model = MenuModel::new(vec![MenuSection::new(
                    "actions",
                    vec![MenuItem::new("move-to-space", "Move to space").submenu()],
                )]);
                model.highlight_first();
                layer.open_at(model, format!("thread:{second}"), at);
                cx.notify();
            });
        });
        test.refresh().unwrap();
        test.simulate_keystrokes(window.into(), "right");
        assert_eq!(owner(&test), Some(format!("thread-space:{second}")));
        test.read(|cx| {
            let layer = state.read(cx).menu_layer.read(cx);
            assert_eq!(layer.position(), Some(at));
            let picker = layer.model().unwrap();
            assert_eq!(
                picker.highlighted(),
                Some(format!("spaces|space:{}", space.id).as_str()),
                "the keyboard carries on from the first row"
            );
        });
        test.simulate_keystrokes(window.into(), "enter");
        assert_eq!(
            test.read(|cx| state.read(cx).thread(second).unwrap().space),
            Some(space.id)
        );

        // "New space…" from the picker makes one and moves the thread in.
        choose(&mut test, format!("thread-space:{first}"), "new-space");
        name(&mut test, "Fresh");
        test.simulate_keystrokes(window.into(), "enter");
        let fresh = test.read(|cx| state.read(cx).thread(first).unwrap().space.unwrap());
        assert_ne!(fresh, space.id);
        choose(
            &mut test,
            format!("thread-space:{first}"),
            "remove-from-space",
        );
        assert_eq!(
            test.read(|cx| state.read(cx).thread(first).unwrap().space),
            None
        );

        // The header's menu: renamed in the dialog it fills in, coloured
        // from the menu its "Colour" opens, and deleted.
        choose(&mut test, format!("space:{}", space.id), "rename-space");
        let entity = dialog(&mut test).unwrap();
        let field = test.read(|cx| entity.read(cx).field.clone());
        assert_eq!(test.read(|cx| field.read(cx).value().to_string()), "Auth");
        name(&mut test, "Auth work");
        test.simulate_keystrokes(window.into(), "enter");
        choose(&mut test, format!("space:{}", space.id), "space-colour");
        assert_eq!(owner(&test), Some(format!("space-colour:{}", space.id)));
        choose(
            &mut test,
            format!("space-colour:{}", space.id),
            "colour:rose",
        );
        let saved = Store::new(paths.clone()).load_app().unwrap();
        let held = &saved.projects[0].spaces[0].space;
        assert_eq!(held.name, "Auth work");
        assert_eq!(held.color.as_deref(), Some("rose"));
        choose(
            &mut test,
            format!("space-colour:{}", space.id),
            "colour:none",
        );
        choose(&mut test, format!("space:{}", space.id), "delete-space");
        let saved = Store::new(paths).load_app().unwrap();
        assert_eq!(saved.projects[0].spaces.len(), 1, "only Fresh is left");
        assert_eq!(saved.projects[0].agents.len(), 2, "and both threads");
        assert!(
            saved.projects[0]
                .agents
                .iter()
                .all(|agent| agent.space.is_none())
        );
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
