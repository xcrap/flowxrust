//! Surfaces and separators: cards, dividers, and the flat panel look.

use gpui::{
    InteractiveElement, IntoElement, MouseButton, ParentElement, StatefulInteractiveElement,
    Styled, StyledText, div,
};

use super::rich_text::{InlineSpan, InlineStyle, inline_run};
use crate::styled::{FxStyled, px_len};
use crate::theme::{BorderWidth, Colors, Radii, Shadow, Spacing, TextRole, global};

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
