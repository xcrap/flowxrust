//! Hover tooltips.
//!
//! FlowX leans on tooltips instead of permanent labels in dense toolbars, so this
//! has to look deliberate: a small [`floating`] surface, the same edge, fill and
//! shadow as a menu, at a corner sized for one line rather than a panel.
//!
//! GPUI 0.2.2 builds a tooltip from an `AnyView`, so the body is a real view
//! rather than an element tree. That also means it gets its own layout and
//! paint, which is why it can be styled independently of its anchor.

use gpui::{
    AnyView, App, AppContext as _, Context, IntoElement, ParentElement as _, Render, SharedString,
    Styled, Window, div,
};

use super::surface::floating;
use crate::styled::{FxStyled, px_len};
use crate::theme::{Colors, Radii, Spacing, TextRole, global};

/// Gap between the anchor and its tooltip.
pub const TOOLTIP_GAP: f32 = Spacing::XS;
/// Maximum width before the label wraps.
const TOOLTIP_MAX_WIDTH: f32 = 280.0;
/// A tooltip is one line, so it keeps a button's corner rather than a panel's.
pub const TOOLTIP_RADIUS: f32 = Radii::SM;

/// A tooltip body.
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = global::current();
        floating(
            div()
                .px(px_len(Spacing::SM))
                .py(px_len(Spacing::XS))
                .fx_role(TextRole::Caption)
                .text_color(Colors::fg_secondary(&theme))
                .child(self.text.clone()),
        )
        // The margin is what separates the tooltip from the control it
        // describes; GPUI places the tooltip against the anchor's edge.
        .mt(px_len(TOOLTIP_GAP))
        .max_w(px_len(TOOLTIP_MAX_WIDTH))
        .rounded(px_len(TOOLTIP_RADIUS))
    }
}

/// Build a tooltip for `text`.
pub fn tooltip(text: impl Into<SharedString>, _window: &mut Window, cx: &mut App) -> AnyView {
    AnyView::from(cx.new(|_cx| Tooltip::new(text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tooltip_sits_on_the_spacing_and_radius_scales() {
        // `XXS + 2.0` and a bare 6.0 were the same 6pt written off the scale.
        assert!(Spacing::ALL.contains(&TOOLTIP_GAP));
        assert!(Radii::ALL.contains(&TOOLTIP_RADIUS));
    }
}
