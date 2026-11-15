//! One creation flow for the keyboard, toolbar and project menu.

use crate::{AppState, text_field::TextField, workspace_ui};
use fx_core::AgentConfiguration;
use fx_design::components::{
    ButtonStyle, DialogAction, DialogClose, DialogHeader, DialogWidth, choice_row, cycle_focus,
    dialog, dialog_body, dialog_footer, dialog_scrim,
};
use fx_design::styled::{FxStyled, px_len};
use fx_design::theme::{Colors, Spacing, global};
use gpui::{
    App, Context, Entity, FocusHandle, MouseButton, Render, Subscription, Task, Window, div,
    prelude::*,
};
use uuid::Uuid;

pub(crate) struct NewChat {
    state: Entity<AppState>,
    project: Uuid,
    root: String,
    id: Uuid,
    configuration: AgentConfiguration,
    worktree: bool,
    branch: Entity<TextField>,
    focus: FocusHandle,
    cancel_focus: FocusHandle,
    confirm_focus: FocusHandle,
    close_focus: FocusHandle,
    error: Option<String>,
    creating: bool,
    prepared: Option<fx_core::models::agent::ChatWorkspace>,
    task: Option<Task<()>>,
    _changed: Subscription,
}

pub(crate) fn open(
    state: Entity<AppState>,
    project: Option<Uuid>,
    window: &mut Window,
    cx: &mut App,
) {
    if workspace_ui::has_modal(&state, cx) {
        return;
    }
    let current = state.read(cx);
    let Some(project) = project
        .or(current.snapshot.active_project_id)
        .and_then(|id| current.project(id))
        .cloned()
    else {
        return;
    };
    let provider = current
        .active_agent()
        .and_then(|a| a.configuration.provider_id)
        .unwrap_or_else(|| "codex".into());
    let configuration = current.default_configuration(&provider);
    let id = Uuid::new_v4();
    let dialog = cx.new(|cx| {
        let branch = cx.new(|cx| {
            let mut field = TextField::new(cx).placeholder("Branch name").max_len(200);
            field.set_value(format!("flowx/{}", &id.simple().to_string()[..8]), cx);
            field
        });
        let changed = cx.observe(&branch, |_, _, cx| cx.notify());
        NewChat {
            state: state.clone(),
            project: project.id,
            root: project.root_path,
            id,
            configuration,
            worktree: false,
            branch,
            focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle(),
            confirm_focus: cx.focus_handle(),
            close_focus: cx.focus_handle(),
            error: None,
            creating: false,
            prepared: None,
            task: None,
            _changed: changed,
        }
    });
    dialog.read(cx).focus.focus(window);
    workspace_ui::handle(cx).update(cx, |ui, cx| {
        ui.new_chat = Some(dialog);
        cx.notify();
    });
    state.update(cx, |_, cx| cx.notify());
}

impl NewChat {
    fn close(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        workspace_ui::handle(cx).update(cx, |ui, cx| {
            ui.new_chat = None;
            cx.notify();
        });
        window.blur();
        self.state.update(cx, |_, cx| cx.notify());
    }

    fn choose(&mut self, worktree: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating || self.prepared.is_some() {
            return;
        }
        self.worktree = worktree;
        self.error = None;
        if worktree {
            self.branch.read(cx).focus_handle().focus(window);
        } else {
            self.focus.focus(window);
        }
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating || (self.worktree && self.branch.read(cx).is_composing()) {
            return;
        }
        self.error = None;
        if !self.worktree || self.prepared.is_some() {
            let result = self.state.update(cx, |state, cx| {
                state.save_all(cx);
                let result = state.create_workspace_thread(
                    self.project,
                    self.id,
                    self.configuration.clone(),
                    self.prepared.clone(),
                );
                cx.notify();
                result
            });
            match result {
                Ok(_) => self.close(window, cx),
                Err(error) => {
                    self.error = Some(error);
                    cx.notify();
                }
            }
            return;
        }
        self.focus.focus(window);
        self.creating = true;
        let root = std::path::PathBuf::from(&self.root);
        let destination = self
            .state
            .read(cx)
            .store
            .paths()
            .root()
            .join("worktrees")
            .join(self.project.to_string())
            .join(self.id.to_string());
        let recovery = destination.display().to_string();
        let branch = self.branch.read(cx).value().to_string();
        let job = crate::inspector::repository::spawn_git(move || async move {
            fx_store::paths::create_private_dir(destination.parent().unwrap())
                .map_err(|e| e.to_string())?;
            fx_core::git::worktree::create(&root, &destination, &branch).await
        });
        self.task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = crate::inspector::repository::finish_git(job).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.creating = false;
                this.task = None;
                let result = result.and_then(|workspace| {
                    this.prepared = Some(workspace.clone());
                    this.state.update(cx, |state, cx| {
                        state.save_all(cx);
                        let result = state.create_workspace_thread(
                            this.project, this.id, this.configuration.clone(), Some(workspace),
                        );
                        cx.notify();
                        result
                    }).map_err(|error| format!("{error}\nThe checkout is kept at {recovery}. Retry saving this conversation."))
                });
                match result {
                    Ok(_) => this.close(window, cx),
                    Err(error) => {
                        this.error = Some(error);
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }
}

impl Render for NewChat {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = global::current();
        let editable = !self.creating && self.prepared.is_none();
        let body = dialog_body()
            .child(choice_row(
                "Current folder",
                "Use the project's existing checkout and changes.",
                !self.worktree, editable,
                cx.listener(|this, _, window, cx| this.choose(false, window, cx)),
            ))
            .child(choice_row(
                "New worktree",
                "Create a separate branch from the latest commit. Uncommitted changes stay in the original folder.",
                self.worktree, editable,
                cx.listener(|this, _, window, cx| this.choose(true, window, cx)),
            ))
            .children(self.worktree.then(|| {
                let branch = if editable {
                    self.branch.clone().into_any_element()
                } else {
                    div().fx_body().child(self.branch.read(cx).value().to_string()).into_any_element()
                };
                div().flex().flex_col().gap(px_len(Spacing::XS))
                    .child(div().fx_caption().text_color(Colors::fg_secondary(&theme)).child("New branch"))
                    .child(branch)
                    .child(div().fx_caption().text_color(Colors::fg_secondary(&theme))
                        .child("Archiving or deleting a chat keeps its worktree and branch."))
            }))
            .children(self.error.clone().map(|error| {
                div().fx_caption().text_color(Colors::fg(&theme)).child(error)
            }));
        let can_confirm =
            !self.creating && (!self.worktree || !self.branch.read(cx).value().trim().is_empty());
        let this = cx.entity();
        let (close, cancel, confirm) = (this.clone(), this.clone(), this.clone());
        let header = DialogHeader::new("New conversation").close(
            DialogClose::new("Close", move |window, cx| {
                close.update(cx, |this, cx| this.close(window, cx));
            })
            .focus(&self.close_focus),
        );
        let footer = dialog_footer(
            None,
            [
                DialogAction::new("Cancel", ButtonStyle::Secondary, move |window, cx| {
                    cancel.update(cx, |this, cx| this.close(window, cx));
                })
                .enabled(!self.creating)
                .focus(&self.cancel_focus),
                DialogAction::new(
                    if self.creating {
                        "Creating worktree…"
                    } else if self.prepared.is_some() {
                        "Save conversation"
                    } else {
                        "Create conversation"
                    },
                    ButtonStyle::Primary,
                    move |window, cx| {
                        confirm.update(cx, |this, cx| this.confirm(window, cx));
                    },
                )
                .enabled(can_confirm)
                .focus(&self.confirm_focus),
            ],
        );
        let mut order = Vec::new();
        if self.worktree && editable {
            order.push(self.branch.read(cx).focus_handle());
        }
        if !self.creating {
            order.push(self.cancel_focus.clone());
        }
        if can_confirm {
            order.push(self.confirm_focus.clone());
        }
        order.push(self.close_focus.clone());
        dialog_scrim("new-chat-dialog")
            .debug_selector(|| "new-chat-dialog".into())
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .capture_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if cycle_focus(&order, event, window) {
                        cx.stop_propagation();
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "escape" => {
                            this.close(window, cx);
                            cx.stop_propagation();
                        }
                        "enter" => {
                            this.confirm(window, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }),
            )
            .child(dialog(DialogWidth::Compact, header, body, footer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FlowX;
    use fx_store::AppPaths;
    use std::path::Path;

    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new(fx_core::git::GIT)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(root)
            .envs(fx_core::git::hardened_environment())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn creating_switching_reopening_and_deleting_a_worktree_chat_preserves_both_checkouts() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        git(&source, &["init", "-q", "-b", "main"]);
        git(&source, &["config", "user.name", "FlowX test"]);
        git(&source, &["config", "user.email", "qa@example.invalid"]);
        std::fs::write(source.join("sample"), "committed").unwrap();
        git(&source, &["add", "sample"]);
        git(&source, &["commit", "-qm", "Initial"]);
        std::fs::write(source.join("sample"), "unsaved work").unwrap();
        let paths = AppPaths::at(temp.path().join("state"));
        let mut test = gpui::TestAppContext::single();
        let state = test.update(|cx| cx.new(|cx| AppState::restore(paths.clone(), cx)));
        let original = state.update(&mut test, |state, _| {
            state.add_project(source.to_str().unwrap());
            state.create_thread().unwrap()
        });
        let window = test.add_window({
            let state = state.clone();
            move |_, _| FlowX::new(state)
        });
        window
            .update(&mut test, |_, window, cx| {
                open(state.clone(), None, window, cx);
                let dialog = workspace_ui::handle(cx).read(cx).new_chat.clone().unwrap();
                dialog.update(cx, |dialog, cx| {
                    dialog.choose(true, window, cx);
                    dialog
                        .branch
                        .update(cx, |field, cx| field.set_value("flowx/isolated", cx));
                    dialog.confirm(window, cx);
                });
            })
            .unwrap();
        for _ in 0..1000 {
            test.run_until_parked();
            if test.update(|cx| workspace_ui::handle(cx).read(cx).new_chat.is_none()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        test.update(|cx| {
            if let Some(dialog) = &workspace_ui::handle(cx).read(cx).new_chat {
                panic!("creation did not finish: {:?}", dialog.read(cx).error);
            }
        });
        let (id, workspace) = test.read(|cx| {
            let state = state.read(cx);
            let agent = state.active_agent().unwrap();
            let workspace = agent.workspace.unwrap();
            assert_eq!(
                state.active_workspace_root().as_deref(),
                Some(workspace.path.as_str())
            );
            assert_eq!(state.recovery_target(agent.id).unwrap().1, workspace.path);
            (agent.id, workspace)
        });
        assert_eq!(
            std::fs::read_to_string(Path::new(&workspace.path).join("sample")).unwrap(),
            "committed"
        );
        assert_eq!(
            std::fs::read_to_string(source.join("sample")).unwrap(),
            "unsaved work"
        );
        let monitor = test.update(|cx| crate::inspector::monitor::shared(&state, cx));
        state.update(&mut test, |state, _| state.select_agent(original));
        monitor.update(&mut test, |monitor, cx| {
            monitor.sync(cx);
            assert_eq!(monitor.project(), Some(source.to_str().unwrap()));
        });
        state.update(&mut test, |state, _| state.select_agent(id));
        monitor.update(&mut test, |monitor, cx| {
            monitor.sync(cx);
            assert_eq!(monitor.project(), Some(workspace.path.as_str()));
            assert!(
                monitor.snapshot().is_none(),
                "old checkout's snapshot must be cleared"
            );
        });
        state.update(&mut test, |state, cx| {
            state.save_all(cx);
            state.flush_saves();
        });
        let restored = test.update(|cx| cx.new(|cx| AppState::restore(paths.clone(), cx)));
        restored.update(&mut test, |state, _| {
            assert_eq!(
                state.thread(id).unwrap().workspace.as_ref(),
                Some(&workspace)
            );
            assert_eq!(
                state.project_root_for(original).as_deref(),
                Some(source.to_str().unwrap())
            );
            assert!(state.delete_thread(id));
        });
        assert!(Path::new(&workspace.path).join(".git").is_file());
        assert_eq!(
            std::fs::read_to_string(source.join("sample")).unwrap(),
            "unsaved work"
        );
    }
}
