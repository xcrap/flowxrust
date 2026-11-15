//! Surfaces and separators: cards, dividers, and the flat panel look.

use gpui::{
    App, InteractiveElement, IntoElement, MouseButton, ParentElement, StatefulInteractiveElement,
    Styled, StyledText, Window, div,
};

use super::button::{IconButtonProps, icon_button};
use super::rich_text::{InlineSpan, InlineStyle, inline_run};
use crate::icon::Icon;
use crate::styled::{FxStyled, px_len};
use crate::theme::{
    BorderWidth, Colors, IconSize, NON_TEXT_FLOOR, Radii, Shadow, Spacing, TextRole, Theme,
    Typography, composite, global, legible_or,
};

/// Which way a divider runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DividerAxis {
    #[default]
    Horizontal,
    Vertical,
}

/// A one-point rule in the border color.
pub fn divider_with_axis(axis: DividerAxis) -> impl IntoElement {
    let theme = global::current();
    let rule = div().flex_shrink_0().bg(Colors::border_subtle(&theme));
    match axis {
        DividerAxis::Horizontal => rule.h(px_len(BorderWidth::HAIRLINE)).w_full(),
        DividerAxis::Vertical => rule.w(px_len(BorderWidth::HAIRLINE)).h_full(),
    }
}

/// A horizontal divider, the common case.
pub fn divider() -> impl IntoElement {
    divider_with_axis(DividerAxis::Horizontal)
}

/// A padded surface: `bg_surface` fill, hairline edge, `lg` radius.
///
/// This is FlowX's default container. It is deliberately opaque — no material,
/// no vibrancy, no translucency — because a translucent card over an opaque
/// window is the single thing that most reliably makes a custom UI look like a
/// native one.
pub fn card(content: impl IntoElement, padding: f32) -> impl IntoElement {
    let theme = global::current();
    div()
        .p(px_len(padding))
        .bg(Colors::bg_surface(&theme))
        .rounded(px_len(Radii::LG))
        .border(px_len(BorderWidth::HAIRLINE))
        .border_color(Colors::border(&theme))
        .child(content)
}

/// A card at the default padding.
pub fn card_default(content: impl IntoElement) -> impl IntoElement {
    card(content, Spacing::MD)
}

/// The floating-panel look: `bg_surface` fill, `border_medium` edge, `xl`
/// radius. Used by menus and the command palette.
pub fn panel(content: impl IntoElement) -> impl IntoElement {
    let theme = global::current();
    div()
        .bg(Colors::bg_surface(&theme))
        .rounded(px_len(Radii::XL))
        .border(px_len(BorderWidth::HAIRLINE))
        .border_color(Colors::border_medium(&theme))
        .child(content)
}

/// A surface that floats above the window: a menu, a tooltip, a popup.
///
/// The fill, the hairline edge, the `xl` corner and the popover shadow are on
/// this element, and whatever scrolls is a child of it ([`floating_scroll`]).
/// GPUI 0.2.2 paints an element's shadow before it pushes that element's own
/// overflow mask, so the shadow is never clipped by the element's bounds; it
/// is clipped by an ancestor that clips. Keeping the scroll on a child keeps
/// it that way: the scroll offset and its mask reach the rows, and nothing
/// that frames the panel.
pub fn floating(content: impl IntoElement) -> gpui::Div {
    let theme = global::current();
    let palette = floating_palette(&theme);
    div()
        .flex()
        .flex_col()
        .bg(palette.fill)
        .rounded(px_len(Radii::XL))
        .border(px_len(BorderWidth::HAIRLINE))
        .border_color(palette.edge)
        .fx_shadow(Shadow::popover(palette.shadow))
        .child(content)
}

/// A floating surface whose content scrolls past `max_height`.
pub fn floating_scroll(
    id: impl Into<gpui::ElementId>,
    max_height: f32,
    content: impl IntoElement,
) -> gpui::Div {
    floating(
        div()
            .id(id)
            .flex()
            .flex_col()
            .max_h(px_len(max_height))
            .overflow_y_scroll()
            .child(content),
    )
}

/// How a floating surface is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatingPalette {
    pub fill: gpui::Rgba,
    pub edge: gpui::Rgba,
    /// The shadow's base color, faded by the popover recipe.
    pub shadow: gpui::Rgba,
}

pub fn floating_palette(theme: &crate::theme::Theme) -> FloatingPalette {
    FloatingPalette {
        fill: Colors::bg_surface(theme),
        edge: Colors::border_medium(theme),
        shadow: Colors::overlay(theme),
    }
}

/// The one size an empty state's glyph is drawn at, whatever is empty: the
/// window, the sidebar, a transcript or the inspector. They were drawn at
/// 18, 22 and 44pt.
pub const EMPTY_STATE_ICON: IconSize = IconSize::Illustration;

/// An empty state's leading slot, a square of [`EMPTY_STATE_ICON`]'s size,
/// so a spinner in place of a glyph leaves the title where it was.
pub fn empty_state_slot(content: impl IntoElement) -> gpui::Div {
    let edge = Typography::icon(EMPTY_STATE_ICON, &global::current()).size;
    div()
        .debug_selector(|| "empty-state-glyph".into())
        .size(px_len(edge))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(content)
}

/// An empty state's glyph, muted, at [`EMPTY_STATE_ICON`].
pub fn empty_state_glyph(icon: Icon) -> gpui::Div {
    let theme = global::current();
    empty_state_slot(icon.render(EMPTY_STATE_ICON, Colors::fg_tertiary(&theme), &theme))
}

/// An empty-state block: a large muted glyph, a title, and a hint.
pub fn empty_state(
    icon: impl IntoElement,
    title: impl Into<gpui::SharedString>,
    hint: impl Into<gpui::SharedString>,
) -> impl IntoElement {
    let theme = global::current();
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .text_center()
        .gap(px_len(Spacing::SM))
        .py(px_len(Spacing::XXL))
        .px(px_len(Spacing::LG))
        .child(icon)
        .child(
            div()
                .fx_role(crate::theme::TextRole::BodyMedium)
                .text_color(Colors::fg_secondary(&theme))
                .child(title.into()),
        )
        .child(
            div()
                .fx_role(crate::theme::TextRole::Caption)
                .text_color(Colors::fg_tertiary(&theme))
                .child(hint.into()),
        )
}

/// A banner: a tinted fill carrying same-hue text.
///
/// The `*_muted` tokens are deliberately translucent, so the surface behind the
/// banner shows through and the banner reads as a wash rather than a slab.
pub fn banner(tint: gpui::Rgba, ink: gpui::Rgba, content: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px_len(Spacing::SM))
        .px(px_len(Spacing::MD))
        .py(px_len(Spacing::SM))
        .rounded(px_len(Radii::MD))
        .bg(tint)
        // The ink cascades to the banner's children, so a caller only has to
        // pass the hue once.
        .text_color(ink)
        .child(content)
}

/// What a notice is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeTone {
    Error,
    Warning,
    Info,
}

impl NoticeTone {
    pub const ALL: [Self; 3] = [Self::Error, Self::Warning, Self::Info];

    fn icon(self) -> Icon {
        match self {
            Self::Error | Self::Warning => Icon::EXCLAMATIONMARK_TRIANGLE_FILL,
            Self::Info => Icon::EXCLAMATIONMARK_CIRCLE_FILL,
        }
    }
}

/// How a notice is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoticePalette {
    /// The tone's tint.
    pub fill: gpui::Rgba,
    /// The message: `fg`, which clears AA on every tint.
    pub text: gpui::Rgba,
    /// The glyph that carries the tone.
    pub glyph: gpui::Rgba,
}

/// A notice's colours.
///
/// The glyph is the tone's hue where it clears 3:1 on its own tint over
/// every surface a notice sits on, the deeper ink where that does, and `fg`
/// where neither does: light-mode `warning` is 2.3:1 on its tint.
pub fn notice_palette(tone: NoticeTone, theme: &Theme) -> NoticePalette {
    let (fill, hue, ink) = match tone {
        NoticeTone::Error => (
            Colors::error_muted(theme),
            Colors::error(theme),
            Colors::error_ink(theme),
        ),
        NoticeTone::Warning => (
            Colors::warning_muted(theme),
            Colors::warning(theme),
            Colors::warning_ink(theme),
        ),
        NoticeTone::Info => (
            Colors::info_muted(theme),
            Colors::info(theme),
            Colors::info(theme),
        ),
    };
    let fills = [
        Colors::bg(theme),
        Colors::bg_elevated(theme),
        Colors::bg_surface(theme),
    ]
    .map(|surface| composite(fill, surface));
    let fg = Colors::fg(theme);
    NoticePalette {
        fill,
        text: fg,
        glyph: legible_or(
            hue,
            legible_or(ink, fg, &fills, NON_TEXT_FLOOR),
            &fills,
            NON_TEXT_FLOOR,
        ),
    }
}

/// A banner that says what happened: the tone's glyph, then the message.
pub fn notice(tone: NoticeTone, message: impl Into<gpui::SharedString>) -> gpui::Div {
    notice_body(tone, message.into(), None)
}

/// A notice the reader can clear, with a ghost button labelled `label` at
/// its trailing edge.
pub fn dismissible_notice(
    tone: NoticeTone,
    message: impl Into<gpui::SharedString>,
    label: impl Into<gpui::SharedString>,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
) -> gpui::Div {
    notice_body(
        tone,
        message.into(),
        Some((label.into(), Box::new(on_dismiss))),
    )
}

type Dismiss = (gpui::SharedString, Box<dyn Fn(&mut Window, &mut App)>);

fn notice_body(
    tone: NoticeTone,
    message: gpui::SharedString,
    dismiss: Option<Dismiss>,
) -> gpui::Div {
    let theme = global::current();
    let palette = notice_palette(tone, &theme);
    div()
        .debug_selector(|| "notice".into())
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_start()
        .gap(px_len(Spacing::SM))
        .px(px_len(Spacing::MD))
        .py(px_len(Spacing::SM))
        .rounded(px_len(Radii::MD))
        .bg(palette.fill)
        .text_color(palette.text)
        .child(
            // On the message's first line, whatever its length.
            div()
                .flex_shrink_0()
                .h(px_len(
                    Typography::style(TextRole::Caption, &theme).size
                        * TextRole::Caption.line_height_multiplier(),
                ))
                .flex()
                .items_center()
                .child(tone.icon().render(IconSize::Small, palette.glyph, &theme)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .fx_role(TextRole::Caption)
                .child(message),
        )
        .children(dismiss.map(|(label, on_dismiss)| {
            div().flex_shrink_0().child(icon_button(
                IconButtonProps::new(Icon::XMARK, label).tiny().ghost(),
                move |_, window, cx| on_dismiss(window, cx),
            ))
        }))
}

/// A single-line notice used above a panel or inside a header.
pub fn inline_notice(ink: gpui::Rgba, content: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .text_color(ink)
        .child(content)
}

/// The hairline used under a section header inside a panel.
pub fn section_divider() -> impl IntoElement {
    divider()
}

/// A section header in the sidebar style, wrapping in narrow panels.
pub fn section_header(label: impl Into<gpui::SharedString>) -> impl IntoElement {
    let theme = global::current();
    div()
        .w_full()
        .min_w_0()
        .fx_overline()
        .text_color(Colors::fg_secondary(&theme))
        .child(label.into())
}

/// Window title and optional context share one baseline and truncate together.
pub fn window_title(title: &str, context: Option<&str>) -> impl IntoElement {
    let theme = global::current();
    let mut text = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title_end = text.len();
    let mut runs = vec![inline_run(
        &InlineSpan {
            range: 0..title_end,
            style: InlineStyle::default(),
        },
        TextRole::BodyMedium,
        &theme,
    )];
    if let Some(context) = context.filter(|context| !context.trim().is_empty()) {
        text.push_str(" · ");
        text.push_str(&context.split_whitespace().collect::<Vec<_>>().join(" "));
        let mut context_run = inline_run(
            &InlineSpan {
                range: title_end..text.len(),
                style: InlineStyle::default(),
            },
            TextRole::Body,
            &theme,
        );
        context_run.color = Colors::fg_secondary(&theme).into();
        runs.push(context_run);
    }
    div()
        .w_full()
        .min_w_0()
        .fx_body_medium()
        // GPUI 0.2.2 caches nowrap's unconstrained first measurement. A one-line
        // clamp remeasures at the final width so the ellipsis is actually drawn.
        .whitespace_normal()
        .text_ellipsis()
        .line_clamp(1)
        .child(StyledText::new(text).with_runs(runs))
}

/// Keep a title-bar control's mouse press out of its draggable parent. The
/// wrapper hugs the control so the surrounding padding remains draggable.
pub fn titlebar_control(control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .flex_shrink_0()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(control)
}

/// A list row's fill: the selection, which it keeps under the pointer, or
/// the hover wash, or nothing.
///
/// GPUI's hover refinement replaces the background rather than layering on
/// it, so an unconditional `.hover(bg_hover)` turned a selected row's 8%
/// selection into a 4% wash, which read as the selection dropping away.
pub fn list_row_fill(selected: bool, hovered: bool, theme: &Theme) -> gpui::Rgba {
    if selected {
        Colors::bg_selected(theme)
    } else if hovered {
        Colors::bg_hover(theme)
    } else {
        gpui::Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }
    }
}

/// A path's folders, with their trailing slash, and its file name.
///
/// A path with no folder is shown from the root it is relative to, `./`, so
/// a file at the top of a repository still reads as a path.
pub fn split_path(path: &str) -> (String, String) {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some((folder, name)) => (format!("{folder}/"), name.to_string()),
        None => ("./".to_string(), trimmed.to_string()),
    }
}

/// A path on one line whose folders give way before its file name does.
///
/// GPUI 0.2.2 truncates text only at its end (`TextOverflow::Truncate`), so a
/// path is two runs: the folders shrink first and end in an ellipsis, and the
/// file name keeps its width unless it alone is wider than the line. The
/// caller sets the type role; both runs inherit it.
pub fn path_label(path: &str, folder_ink: gpui::Rgba, name_ink: gpui::Rgba) -> gpui::Div {
    let (folder, name) = split_path(path);
    div()
        .min_w_0()
        .flex()
        .flex_row()
        .overflow_hidden()
        .child(
            div()
                .debug_selector(|| "path-label-folder".into())
                .min_w_0()
                .flex_shrink()
                .truncate()
                .text_color(folder_ink)
                .child(folder),
        )
        .child(
            div()
                .debug_selector(|| "path-label-name".into())
                .min_w_0()
                .max_w_full()
                .flex_shrink_0()
                .truncate()
                .text_color(name_ink)
                .child(name),
        )
}

/// The hairline thickness used for every card and panel edge.
pub const HAIRLINE: f32 = BorderWidth::HAIRLINE;

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AbsoluteLength, Overflow};

    fn hairline(width: Option<AbsoluteLength>) -> bool {
        width == Some(px_len(BorderWidth::HAIRLINE))
    }

    #[test]
    fn a_floating_surface_carries_the_popover_shadow_a_hairline_and_never_scrolls() {
        // The panel frames; its child scrolls. The menu panel scrolled itself
        // and went without a shadow, on the belief that its bounds clipped it.
        let mut panel = floating_scroll("probe", 120.0, div());
        let style = panel.style();
        let shadow = style.box_shadow.clone().unwrap_or_default();
        assert_eq!(shadow.len(), 1, "a floating surface has one shadow");
        let recipe = Shadow::popover(Colors::overlay(&global::current()));
        assert_eq!(shadow[0].blur_radius, gpui::px(recipe.blur));
        assert_eq!(shadow[0].offset.y, gpui::px(recipe.y));
        let edges = &style.border_widths;
        assert!(hairline(edges.top) && hairline(edges.right));
        assert!(hairline(edges.bottom) && hairline(edges.left));
        assert_eq!(style.corner_radii.top_left, Some(px_len(Radii::XL)));
        assert!(
            style.overflow.y.is_none_or(|o| o == Overflow::Visible),
            "the panel itself scrolls"
        );
    }

    #[test]
    fn every_notice_glyph_clears_three_to_one_on_its_own_tint() {
        // Light-mode `warning` is 2.3:1 on its tint, under the 3:1 a glyph
        // that carries the tone needs.
        for tone in crate::theme::BaseTone::ALL {
            for appearance_mode in [
                crate::theme::AppearanceMode::Light,
                crate::theme::AppearanceMode::Dark,
            ] {
                let theme = Theme {
                    base_tone: tone,
                    appearance_mode,
                    ..Default::default()
                };
                for notice in NoticeTone::ALL {
                    let palette = notice_palette(notice, &theme);
                    for surface in [
                        Colors::bg(&theme),
                        Colors::bg_elevated(&theme),
                        Colors::bg_surface(&theme),
                    ] {
                        let fill = composite(palette.fill, surface);
                        let glyph = crate::theme::contrast(palette.glyph, fill);
                        let text = crate::theme::contrast(palette.text, fill);
                        assert!(
                            glyph >= 3.0,
                            "{tone:?} {appearance_mode:?} {notice:?}: {glyph}"
                        );
                        assert!(
                            text >= 4.5,
                            "{tone:?} {appearance_mode:?} {notice:?}: {text}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_empty_state_glyph_sits_in_one_slot_size() {
        let edge = Typography::icon(EMPTY_STATE_ICON, &global::current()).size;
        let mut glyph = empty_state_glyph(Icon::FOLDER);
        let size = glyph.style().size.clone();
        assert_eq!(size.width, Some(px_len(edge).into()));
        assert_eq!(size.height, Some(px_len(edge).into()));
        assert_eq!(EMPTY_STATE_ICON, IconSize::Illustration);
    }

    #[test]
    fn a_selected_row_keeps_its_fill_under_the_pointer() {
        for appearance_mode in [
            crate::theme::AppearanceMode::Light,
            crate::theme::AppearanceMode::Dark,
        ] {
            let theme = Theme {
                appearance_mode,
                ..Default::default()
            };
            let selected = list_row_fill(true, false, &theme);
            assert_eq!(selected, Colors::bg_selected(&theme));
            assert_eq!(list_row_fill(true, true, &theme), selected);
            assert_eq!(list_row_fill(false, true, &theme), Colors::bg_hover(&theme));
            assert_eq!(list_row_fill(false, false, &theme).a, 0.0);
        }
    }

    #[test]
    fn a_path_splits_into_its_folders_and_its_file_name() {
        assert_eq!(
            split_path("crates/fx-app/src/diff_view.rs"),
            ("crates/fx-app/src/".to_string(), "diff_view.rs".to_string())
        );
        assert_eq!(
            split_path("README.md"),
            ("./".to_string(), "README.md".to_string())
        );
        assert_eq!(
            split_path("vendor/lib/"),
            ("vendor/".to_string(), "lib".to_string())
        );
    }

    #[test]
    fn a_floating_surface_is_an_opaque_fill_with_a_visible_edge() {
        for appearance_mode in [
            crate::theme::AppearanceMode::Light,
            crate::theme::AppearanceMode::Dark,
        ] {
            let theme = crate::theme::Theme {
                appearance_mode,
                ..Default::default()
            };
            let palette = floating_palette(&theme);
            assert_eq!(palette.fill.a, 1.0, "a floating panel is opaque");
            assert_ne!(palette.edge, palette.fill);
        }
    }
}
