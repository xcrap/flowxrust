//! Dialogs: one frame, header, body and footer for every centred sheet.
//!
//! Every dialog in FlowX is the same object: a scrim over the window, an
//! opaque surface at one of two widths, a heading in one role with an
//! optional ghost close button, a body, and a footer of buttons ending in
//! the default one. The dialogs were built by hand and had drifted on every
//! one of those: two had no shadow, the others had theirs on a padded
//! wrapper, the titles were two roles, and the close buttons two variants.
//!
//! The buttons are reachable from the keyboard. Each takes a focus handle
//! that its dialog keeps, so the button's focus ring shows once it is
//! focused; Tab and Shift-Tab move through the dialog's controls in order
//! ([`cycle_focus`]), Space presses the focused button, and Return stays the
//! dialog's default action, as it is in a macOS sheet.

use std::rc::Rc;

use gpui::prelude::*;
use gpui::{AnyElement, App, FocusHandle, KeyDownEvent, MouseButton, SharedString, Window, div};

use super::button::{ButtonStyle, IconButtonProps, button_with_icon, icon_button};
use super::surface::divider;
use crate::icon::Icon;
use crate::styled::{FxStyled, px_len};
use crate::theme::{
    BorderWidth, Colors, IconSize, Layout, Radii, Shadow, ShadowStyle, Spacing, TextRole, Theme,
    global,
};

/// How wide a dialog is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialogWidth {
    /// One question or one field: rename, delete, a new chat, a project.
    #[default]
    Compact,
    /// Several questions, or text that needs a measure to read.
    Regular,
}

impl DialogWidth {
    /// The width in points, before a narrow window shrinks it.
    pub const fn points(self) -> f32 {
        match self {
            Self::Compact => Layout::DIALOG_WIDTH,
            Self::Regular => Layout::DIALOG_WIDE_WIDTH,
        }
    }
}

/// The one role every dialog's title is set in.
pub const DIALOG_TITLE_ROLE: TextRole = TextRole::Title2;

/// What runs when a dialog's control is pressed, by pointer or key.
pub type DialogHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// How a dialog is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialogPalette {
    pub fill: gpui::Rgba,
    pub edge: gpui::Rgba,
    /// The wash over the window behind the dialog.
    pub scrim: gpui::Rgba,
    pub shadow: ShadowStyle,
}

/// The dialog's colours: an opaque `bg_surface` with a `border_medium` edge,
/// over the token layer's strongest overlay.
///
/// The shadow is the dialog recipe rather than the popover's: it is drawn
/// over the scrim, and the popover's 35% shadow all but vanishes against a
/// scrim of the same colour.
pub fn dialog_palette(theme: &Theme) -> DialogPalette {
    let overlay = Colors::overlay(theme);
    DialogPalette {
        fill: Colors::bg_surface(theme),
        edge: Colors::border_medium(theme),
        scrim: overlay,
        shadow: Shadow::dialog(overlay),
    }
}

/// The scrim a dialog sits on: it covers the window, occludes what is
/// behind it, and centres the dialog with a margin to the window's edge.
///
/// What a press on the scrim does is the dialog's to decide: backing out, or
/// nothing at all.
pub fn dialog_scrim(id: impl Into<gpui::ElementId>) -> gpui::Stateful<gpui::Div> {
    let theme = global::current();
    div()
        .id(id)
        .occlude()
        .absolute()
        .inset_0()
        .size_full()
        .bg(dialog_palette(&theme).scrim)
        .flex()
        .items_center()
        .justify_center()
        .p(px_len(Spacing::XL))
}

/// The dialog's surface at `width` points, narrowed by a small window.
///
/// The shadow is on the surface itself, not on a padded wrapper: GPUI 0.2.2
/// paints an element's shadow outside its own mask (see `floating`), so the
/// wrapper's padding only moved the shadow off the panel's edge. A press on
/// the surface stops there, so it never reads as a press on the scrim.
pub fn dialog_surface(width: f32) -> gpui::Div {
    let theme = global::current();
    let palette = dialog_palette(&theme);
    div()
        .w(px_len(width))
        .max_w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .bg(palette.fill)
        .rounded(px_len(Radii::XL))
        .border(px_len(BorderWidth::HAIRLINE))
        .border_color(palette.edge)
        .fx_shadow(palette.shadow)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// The close button in a dialog's corner.
pub struct DialogClose {
    pub label: SharedString,
    pub focus: Option<FocusHandle>,
    pub on_close: DialogHandler,
}

impl DialogClose {
    pub fn new(
        label: impl Into<SharedString>,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            focus: None,
            on_close: Rc::new(on_close),
        }
    }

    /// Reachable from the keyboard through `focus`.
    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
}

/// A ghost close button: no chrome until the pointer or the keyboard is on
/// it, so it does not compete with the title.
pub fn dialog_close_button(close: DialogClose) -> gpui::Stateful<gpui::Div> {
    let DialogClose {
        label,
        focus,
        on_close,
    } = close;
    let on_click = on_close.clone();
    let control = icon_button(
        IconButtonProps::new(Icon::XMARK, label).ghost(),
        move |_, window, cx| on_click(window, cx),
    );
    focusable(control, focus, on_close, true)
}

/// What a dialog's header says.
pub struct DialogHeader {
    pub title: SharedString,
    pub subtitle: Option<SharedString>,
    pub icon: Option<Icon>,
    pub close: Option<DialogClose>,
}

impl DialogHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            icon: None,
            close: None,
        }
    }

    /// A line under the title: what is asked, or who asks it.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// A glyph before the title.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn close(mut self, close: DialogClose) -> Self {
        self.close = Some(close);
        self
    }
}

/// A dialog's title in [`DIALOG_TITLE_ROLE`].
pub fn dialog_title(title: impl Into<SharedString>) -> gpui::Div {
    div()
        .debug_selector(|| "dialog-title".into())
        .min_w_0()
        .fx_role(DIALOG_TITLE_ROLE)
        .text_color(Colors::fg(&global::current()))
        .child(title.into())
}

/// The header: an optional glyph, the title over an optional subtitle, and an
/// optional close button at the trailing edge.
pub fn dialog_header(header: DialogHeader) -> gpui::Div {
    let theme = global::current();
    let DialogHeader {
        title,
        subtitle,
        icon,
        close,
    } = header;
    div()
        .w_full()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px_len(Spacing::MD))
        .px(px_len(Spacing::XXL))
        .py(px_len(Spacing::XL))
        .children(
            icon.map(|icon| icon.render(IconSize::Large, Colors::fg_secondary(&theme), &theme)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px_len(Spacing::XXS))
                .child(dialog_title(title))
                .children(subtitle.map(|subtitle| {
                    div()
                        .fx_caption()
                        .text_color(Colors::fg_secondary(&theme))
                        .child(subtitle)
                })),
        )
        .children(close.map(dialog_close_button))
}

/// The body's insets and rhythm.
pub fn dialog_body() -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px_len(Spacing::MD))
        .p(px_len(Spacing::XXL))
}

/// One of the footer's buttons.
pub struct DialogAction {
    pub label: SharedString,
    pub icon: Option<Icon>,
    pub style: ButtonStyle,
    pub enabled: bool,
    pub focus: Option<FocusHandle>,
    pub on_activate: DialogHandler,
    /// A name a test can find the button by.
    pub selector: Option<SharedString>,
}

impl DialogAction {
    pub fn new(
        label: impl Into<SharedString>,
        style: ButtonStyle,
        on_activate: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            icon: None,
            style,
            enabled: true,
            focus: None,
            on_activate: Rc::new(on_activate),
            selector: None,
        }
    }

    pub fn selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Reachable from the keyboard through `focus`.
    pub fn focus(mut self, focus: &FocusHandle) -> Self {
        self.focus = Some(focus.clone());
        self
    }
}

/// A footer button that the keyboard can reach and press.
pub fn dialog_button(action: DialogAction) -> gpui::Stateful<gpui::Div> {
    let DialogAction {
        label,
        icon,
        style,
        enabled,
        focus,
        on_activate,
        selector,
    } = action;
    let on_click = on_activate.clone();
    let control = button_with_icon(label, icon, style, enabled, move |_, window, cx| {
        on_click(window, cx)
    });
    let control = match selector {
        Some(selector) => control.debug_selector(move || selector.to_string()),
        None => control,
    };
    focusable(control, focus, on_activate, enabled)
}

/// Track `focus` on a control, so its focus ring shows, and press it with
/// Space while it holds the focus.
fn focusable(
    control: gpui::Stateful<gpui::Div>,
    focus: Option<FocusHandle>,
    on_activate: DialogHandler,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    let Some(focus) = focus else {
        return control;
    };
    control
        .track_focus(&focus)
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.key == "space" && !event.keystroke.modifiers.modified() {
                cx.stop_propagation();
                if enabled {
                    on_activate(window, cx);
                }
            }
        })
}

/// The footer: an optional note at the leading edge, then the buttons at the
/// trailing one, the default button last.
pub fn dialog_footer(
    note: Option<AnyElement>,
    actions: impl IntoIterator<Item = DialogAction>,
) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px_len(Spacing::MD))
        .px(px_len(Spacing::XXL))
        .py(px_len(Spacing::LG))
        .child(div().flex_1().min_w_0().children(note))
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px_len(Spacing::SM))
                .children(actions.into_iter().map(dialog_button)),
        )
}

/// A whole dialog: the surface, its header, a rule, the body, a rule and
/// the footer.
pub fn dialog(
    width: DialogWidth,
    header: DialogHeader,
    body: impl IntoElement,
    footer: impl IntoElement,
) -> gpui::Div {
    dialog_surface(width.points())
        .child(dialog_header(header))
        .child(divider())
        .child(body)
        .child(divider())
        .child(footer)
}

/// Where Tab moves the focus among `len` controls, from the one at
/// `current`: the next, or with `backwards` the previous, wrapping at either
/// end. From none of them, Tab starts at the first and Shift-Tab at the last.
pub fn next_focus_index(len: usize, current: Option<usize>, backwards: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match (current, backwards) {
        (None, false) => 0,
        (None, true) => len - 1,
        (Some(index), false) => (index + 1) % len,
        (Some(index), true) => (index + len - 1) % len,
    })
}

/// Move the focus through a dialog's controls on Tab and Shift-Tab, and say
/// whether the key was taken.
///
/// `order` is the dialog's controls in reading order, fields included, with
/// the disabled ones left out. Called from the dialog's `capture_key_down`,
/// so a field never sees the Tab, and the focus never leaves the dialog for
/// whatever is behind its scrim.
pub fn cycle_focus(order: &[FocusHandle], event: &KeyDownEvent, window: &mut Window) -> bool {
    let modifiers = &event.keystroke.modifiers;
    if event.keystroke.key != "tab"
        || modifiers.control
        || modifiers.alt
        || modifiers.platform
        || modifiers.function
    {
        return false;
    }
    let current = order.iter().position(|focus| focus.is_focused(window));
    if let Some(next) = next_focus_index(order.len(), current, modifiers.shift) {
        order[next].focus(window);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AbsoluteLength, Fill};

    fn hairline(width: Option<AbsoluteLength>) -> bool {
        width == Some(px_len(BorderWidth::HAIRLINE))
    }

    #[test]
    fn every_dialog_width_is_a_layout_token() {
        assert_eq!(DialogWidth::Compact.points(), Layout::DIALOG_WIDTH);
        assert_eq!(DialogWidth::Regular.points(), Layout::DIALOG_WIDE_WIDTH);
        assert!(DialogWidth::Compact.points() < DialogWidth::Regular.points());
    }

    #[test]
    fn a_dialog_surface_carries_the_dialog_shadow_a_hairline_and_the_xl_corner() {
        // Two dialogs had no shadow and three had it on a padded wrapper.
        let mut surface = dialog_surface(DialogWidth::Compact.points());
        let style = surface.style();
        let shadow = style.box_shadow.clone().unwrap_or_default();
        assert_eq!(shadow.len(), 1, "a dialog has one shadow, on its surface");
        let recipe = dialog_palette(&global::current()).shadow;
        assert_eq!(shadow[0].blur_radius, gpui::px(recipe.blur));
        assert_eq!(shadow[0].offset.y, gpui::px(recipe.y));
        let edges = &style.border_widths;
        assert!(hairline(edges.top) && hairline(edges.right));
        assert!(hairline(edges.bottom) && hairline(edges.left));
        assert_eq!(style.corner_radii.top_left, Some(px_len(Radii::XL)));
        assert!(matches!(style.background, Some(Fill::Color(_))));
    }

    #[test]
    fn a_dialog_is_opaque_over_its_scrim_in_both_modes() {
        for appearance_mode in [
            crate::theme::AppearanceMode::Light,
            crate::theme::AppearanceMode::Dark,
        ] {
            let theme = Theme {
                appearance_mode,
                ..Default::default()
            };
            let palette = dialog_palette(&theme);
            assert_eq!(palette.fill.a, 1.0, "a dialog is opaque");
            assert!(palette.scrim.a > 0.0 && palette.scrim.a < 1.0);
            assert_ne!(palette.edge, palette.fill);
        }
    }

    #[test]
    fn every_dialog_title_is_set_in_one_role() {
        // Two were Title3 and two Title2.
        let theme = global::current();
        let role = crate::theme::Typography::style(DIALOG_TITLE_ROLE, &theme);
        let mut title = dialog_title("Rename thread");
        let text = title.style().text.clone().expect("a text style");
        assert_eq!(text.font_size, Some(px_len(role.size)));
        assert_eq!(text.font_weight, Some(role.weight));
    }

    #[test]
    fn the_close_button_is_a_ghost_at_rest() {
        // One dialog's close was filled and another's had none.
        let mut close = dialog_close_button(DialogClose::new("Close", |_, _| {}));
        let background = close.style().background.clone();
        let Some(Fill::Color(color)) = background else {
            panic!("the close button has no fill at all");
        };
        assert!(color.is_transparent(), "a ghost has no resting fill");
    }

    #[test]
    fn tab_moves_through_a_dialogs_controls_and_wraps() {
        assert_eq!(next_focus_index(0, None, false), None);
        assert_eq!(next_focus_index(3, None, false), Some(0));
        assert_eq!(next_focus_index(3, None, true), Some(2));
        assert_eq!(next_focus_index(3, Some(0), false), Some(1));
        assert_eq!(next_focus_index(3, Some(2), false), Some(0));
        assert_eq!(next_focus_index(3, Some(0), true), Some(2));
        assert_eq!(next_focus_index(1, Some(0), false), Some(0));
    }
}
