use std::rc::Rc;

use gpui::prelude::*;
use gpui::{App, CursorStyle, ParentElement, SharedString, Styled, Window, div};

use crate::styled::{FxRefinement, FxStyled, px_len};
use crate::theme::{BorderWidth, Colors, Layout, Radii, Spacing, TextRole, global};

/// Height of a segment.
const SEGMENT_HEIGHT: f32 = Layout::CONTROL_COMPACT_HEIGHT - Spacing::XXS;

/// The callback every segment of a segmented control shares.
///
/// Takes the value by reference to match the shape of a GPUI
/// `Context::listener`, so a control can be wired straight to one.
type SelectionHandler<T> = Rc<dyn Fn(&T, &mut Window, &mut App)>;

/// One choice in a segmented control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentedOption<T> {
    pub value: T,
    pub title: SharedString,
}

impl<T> SegmentedOption<T> {
    pub fn new(value: T, title: impl Into<SharedString>) -> Self {
        Self {
            value,
            title: title.into(),
        }
    }
}

/// How one segment is painted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentPalette {
    pub foreground: gpui::Rgba,
    pub background: gpui::Rgba,
}

/// The selected segment is `fg` on the selection wash and keeps it under the
/// pointer. The others are `fg_secondary`, and `fg` on the hover wash.
///
/// The unselected labels were `fg_tertiary`, 3.7:1 on the track: a caption
/// that names a choice is text, and needs AA. The selected one still leads,
/// by its fill and a step of ink.
pub fn segment_palette(
    selected: bool,
    hovered: bool,
    theme: &crate::theme::Theme,
) -> SegmentPalette {
    if selected {
        SegmentPalette {
            foreground: Colors::fg(theme),
            background: Colors::bg_selected(theme),
        }
    } else if hovered {
        SegmentPalette {
            foreground: Colors::fg(theme),
            background: Colors::bg_hover(theme),
        }
    } else {
        SegmentPalette {
            foreground: Colors::fg_secondary(theme),
            background: gpui::Rgba {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            },
        }
    }
}

/// The track every segment is painted over.
pub fn segmented_track(theme: &crate::theme::Theme) -> gpui::Rgba {
    Colors::bg_surface(theme)
}

/// A row of exclusive choices, with the selected one filled.
///
/// Sized to its content rather than stretched, so a two-option control in a
/// toolbar does not read as a full-width field. The control is one focus
/// target, its track: a view that tracks focus on it gets the ring around
/// the whole control.
pub fn segmented_control<T: PartialEq + Clone + 'static>(
    options: &[SegmentedOption<T>],
    selected: &T,
    on_select: impl Fn(&T, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let theme = global::current();

    // Every segment needs the same callback, so share one copy rather than
    // requiring the caller's closure to be cloneable.
    let on_select: SelectionHandler<T> = Rc::new(on_select);

    let segments = options.iter().map(|option| {
        let is_selected = &option.value == selected;
        let value = option.value.clone();
        let rest = segment_palette(is_selected, false, &theme);
        let hover = segment_palette(is_selected, true, &theme);
        div()
            .flex()
            .flex_shrink_0()
            .px(px_len(Spacing::SM))
            .h(px_len(SEGMENT_HEIGHT))
            .flex_col()
            .justify_center()
            .whitespace_nowrap()
            .rounded(px_len(Radii::XS))
            .bg(rest.background)
            .text_color(rest.foreground)
            .fx_role(TextRole::CaptionMedium)
            .child(option.title.clone())
            // `.id` upgrades to `Stateful<Div>`, which carries `on_click`.
            .id(gpui::SharedString::from(format!(
                "segment:{}",
                option.title
            )))
            .cursor(CursorStyle::PointingHand)
            .hover(move |s| {
                s.fx_background(hover.background)
                    .fx_text_color(hover.foreground)
            })
            .on_click({
                let on_select = Rc::clone(&on_select);
                move |_event, window, cx| on_select(&value, window, cx)
            })
    });

    let id = options
        .iter()
        .map(|option| option.title.as_ref())
        .collect::<Vec<_>>()
        .join("|");
    div()
        .debug_selector(|| "segmented-control".into())
        .flex()
        .flex_shrink_0()
        .flex_row()
        .gap(px_len(Spacing::XXXS))
        .p(px_len(Spacing::XXXS))
        .bg(segmented_track(&theme))
        .rounded(px_len(Radii::SM))
        .children(segments.collect::<Vec<_>>())
        .id(gpui::SharedString::from(format!("segmented:{id}")))
        // The track has no edge at rest, so its padding gives back the ring.
        .focus(|s| {
            s.fx_focus_ring()
                .p(px_len(Spacing::XXXS - BorderWidth::focus_give_back(0.0)))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_option_carries_its_value_and_title() {
        let o = SegmentedOption::new(2u8, "Two");
        assert_eq!(o.value, 2);
        assert_eq!(o.title, "Two");
    }

    #[test]
    fn options_compare_by_value_not_title() {
        // Two options can share a label while meaning different things.
        let a = SegmentedOption::new(1u8, "Same");
        let b = SegmentedOption::new(2u8, "Same");
        assert_ne!(a, b);
    }
}
