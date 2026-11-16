use std::rc::Rc;

use gpui::prelude::*;
use gpui::{
    App, ClickEvent, CursorStyle, FocusHandle, IntoElement, ParentElement, SharedString,
    StyleRefinement, Window, div,
};

use crate::icon::Icon;
use crate::styled::{FxRefinement, FxStyled, px_len};
use crate::theme::{
    BorderWidth, Colors, IconSize, Layout, Radii, Spacing, TextRole, WithAlpha, global,
};

/// Visual weight of a labelled button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonStyle {
    /// Solid accent fill. One per screen at most.
    #[default]
    Primary,
    /// Surface fill with a hairline edge. The default for most actions.
    Secondary,
    /// No chrome until hovered.
    Ghost,
    /// Destructive action.
    Danger,
}

/// Opacity applied to a disabled control.
const DISABLED_OPACITY: f32 = 0.52;
/// Opacity applied while a button is held down.
///
/// The original scaled the control to 0.97. GPUI's style refinement cannot
/// transform an element, so press feedback is an opacity dip instead — which
/// also avoids re-running layout on every press.
pub(crate) const PRESSED_OPACITY: f32 = 0.94;

/// How a button looks, as a function of style and hover.
///
/// Split out from rendering so the color decisions can be asserted directly,
/// without standing up a window. `labelled_button` applies the hovered
/// palette's `background` and `foreground` and nothing else, so a hover has
/// to live in those two fields.
#[derive(Debug, Clone, Copy)]
pub struct ButtonPalette {
    pub foreground: gpui::Rgba,
    pub background: gpui::Rgba,
    pub border: gpui::Rgba,
    /// The keyboard focus ring, drawn as the border while focused.
    pub focus_ring: gpui::Rgba,
}

/// A translucent tint laid over itself: what painting it twice would show,
/// as one fill.
fn layered(tint: gpui::Rgba) -> gpui::Rgba {
    tint.with_alpha(1.0 - (1.0 - tint.a) * (1.0 - tint.a))
}

/// The Danger fill, deepened under the pointer by layering the muted token a
/// second time rather than by a hand-tuned second opacity.
fn danger_fill(hovered: bool, theme: &crate::theme::Theme) -> gpui::Rgba {
    let tint = Colors::error_muted(theme);
    if hovered { layered(tint) } else { tint }
}

/// The Danger label: `error_ink` where it clears AA on the button's own fill,
/// at rest and hovered, over every surface a button sits on; `fg` where it
/// does not.
///
/// The badge's rule. `error` on its own tint was 3.1:1 over `bg_surface`, and
/// even `error_ink` stays under 4.5:1 there, so today the label is `fg` in
/// both modes: the tint and the red edge carry the tone.
fn danger_label_ink(theme: &crate::theme::Theme) -> gpui::Rgba {
    crate::theme::legible_or(
        Colors::error_ink(theme),
        Colors::fg(theme),
        &danger_fills(theme),
        crate::theme::TEXT_FLOOR,
    )
}

/// The Danger ring: the focus color where it clears 3:1 on the red tint,
/// rest and hovered, and `fg` where it does not (light violet is 2.9:1 on
/// the hovered tint over `bg_surface`).
fn danger_focus_ring(theme: &crate::theme::Theme) -> gpui::Rgba {
    crate::theme::legible_or(
        Colors::focus_ring(theme),
        Colors::fg(theme),
        &danger_fills(theme),
        crate::theme::NON_TEXT_FLOOR,
    )
}

/// The Danger fill at rest and hovered, over every surface a button sits on.
fn danger_fills(theme: &crate::theme::Theme) -> [gpui::Rgba; 6] {
    let surfaces = [
        Colors::bg(theme),
        Colors::bg_elevated(theme),
        Colors::bg_surface(theme),
    ];
    std::array::from_fn(|i| crate::theme::composite(danger_fill(i >= 3, theme), surfaces[i % 3]))
}

pub fn button_palette(
    style: ButtonStyle,
    hovered: bool,
    theme: &crate::theme::Theme,
) -> ButtonPalette {
    use gpui::Rgba;
    let clear = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    match style {
        ButtonStyle::Primary => ButtonPalette {
            foreground: Colors::on_accent(theme),
            background: if hovered {
                Colors::accent_hover(theme)
            } else {
                Colors::accent(theme)
            },
            border: clear,
            // A ring in the focus color can be the accent itself, which on an
            // accent fill is no ring at all.
            focus_ring: Colors::on_accent(theme),
        },
        ButtonStyle::Secondary => ButtonPalette {
            foreground: Colors::fg(theme),
            // The wash is laid over the surface rather than replacing it:
            // GPUI's hover replaces the background, and a bare 4% wash let the
            // panel through, so the hovered button was the flatter one.
            background: if hovered {
                crate::theme::composite(Colors::bg_hover(theme), Colors::bg_surface(theme))
            } else {
                Colors::bg_surface(theme)
            },
            border: Colors::border(theme),
            focus_ring: Colors::focus_ring(theme),
        },
        ButtonStyle::Ghost => ButtonPalette {
            foreground: if hovered {
                Colors::fg(theme)
            } else {
                Colors::fg_secondary(theme)
            },
            background: if hovered {
                Colors::bg_hover(theme)
            } else {
                clear
            },
            border: clear,
            focus_ring: Colors::focus_ring(theme),
        },
        ButtonStyle::Danger => ButtonPalette {
            foreground: danger_label_ink(theme),
            background: danger_fill(hovered, theme),
            border: Colors::error(theme).with_alpha(0.3),
            focus_ring: danger_focus_ring(theme),
        },
    }
}

/// A labelled button, optionally with a leading glyph.
///
/// Returned as the element itself, so a view can `track_focus` it and show
/// its focus ring.
pub fn button(
    label: impl Into<SharedString>,
    style: ButtonStyle,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    button_with_icon(label, None, style, enabled, on_click)
}

/// A labelled button with a leading glyph.
pub fn button_with_icon(
    label: impl Into<SharedString>,
    icon: Option<Icon>,
    style: ButtonStyle,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    labelled_button(
        label,
        icon,
        move |hovered, theme| button_palette(style, hovered, theme),
        enabled,
        on_click,
    )
}

/// The body of every labelled button, painted from `palette(hovered, theme)`.
fn labelled_button(
    label: impl Into<SharedString>,
    icon: Option<Icon>,
    palette: impl Fn(bool, &crate::theme::Theme) -> ButtonPalette,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let theme = global::current();
    let label = label.into();
    let button_id: SharedString = format!("button:{label}").into();
    let hover = palette(true, &theme);
    let palette = palette(false, &theme);
    let icon_element = icon.map(|i| i.render(IconSize::Control, palette.foreground, &theme));

    let mut content = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .min_h(px_len(Layout::CONTROL_HEIGHT))
        .gap(px_len(Spacing::XS))
        .debug_selector(|| "button-content".into());
    if let Some(el) = icon_element {
        content = content.child(
            div()
                .flex()
                .flex_shrink_0()
                .debug_selector(|| "button-icon".into())
                .child(el),
        );
    }
    content = content
        .child(
            div()
                .whitespace_nowrap()
                .fx_role(TextRole::BodyMedium)
                .debug_selector(|| "button-label".into())
                .child(label),
        )
        .px(px_len(Spacing::MD))
        .py(px_len(Spacing::XXS))
        .rounded(px_len(Radii::SM))
        .border(px_len(BorderWidth::HAIRLINE))
        .border_color(palette.border)
        .bg(palette.background)
        .text_color(palette.foreground);

    // The ring is wider than the resting hairline, so the padding gives the
    // difference back and the button keeps its size while focused.
    let give_back = BorderWidth::focus_give_back(BorderWidth::HAIRLINE);
    content
        .opacity(if enabled { 1.0 } else { DISABLED_OPACITY })
        .id(button_id)
        .focus(|s: StyleRefinement| {
            s.fx_focus_ring_in(palette.focus_ring)
                .px(px_len(Spacing::MD - give_back))
                .py(px_len(Spacing::XXS - give_back))
        })
        .hover(|s: StyleRefinement| {
            if !enabled {
                return s;
            }
            s.fx_background(hover.background)
                .fx_text_color(hover.foreground)
        })
        .active(|s: StyleRefinement| {
            if enabled {
                s.fx_opacity(PRESSED_OPACITY)
            } else {
                s
            }
        })
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::default()
        })
        .on_click(move |event, window, cx| {
            if enabled {
                on_click(event, window, cx);
            }
        })
}

/// How a navigation destination looks: a ghost button, or the selected one.
///
/// The selected destination keeps its fill under the pointer. The ghost hover
/// replaced it with a lighter wash, which read as the selection dropping away.
pub fn navigation_palette(
    selected: bool,
    hovered: bool,
    theme: &crate::theme::Theme,
) -> ButtonPalette {
    if !selected {
        return button_palette(ButtonStyle::Ghost, hovered, theme);
    }
    let clear = gpui::Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    ButtonPalette {
        foreground: Colors::fg(theme),
        background: Colors::bg_selected(theme),
        border: clear,
        focus_ring: Colors::focus_ring(theme),
    }
}

/// A full-width, left-aligned destination in a settings or navigation list.
pub fn navigation_button(
    label: impl Into<SharedString>,
    icon: Icon,
    selected: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    labelled_button(
        label,
        Some(icon),
        move |hovered, theme| navigation_palette(selected, hovered, theme),
        true,
        on_click,
    )
    .w_full()
    .justify_start()
}

/// How much chrome an icon button shows at rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IconButtonVariant {
    /// A subtle surface fill; used in pane headers.
    #[default]
    Filled,
    /// No chrome at rest, highlight on hover. Selection tints the glyph with
    /// the accent. Used in the window title bar.
    Ghost,
}

/// Default edge length for an icon button.
pub const ICON_BUTTON_SIZE: f32 = Layout::CONTROL_HEIGHT;
/// Compact toolbar size.
const SMALL_ICON_BUTTON_SIZE: f32 = Layout::CONTROL_COMPACT_HEIGHT;
/// Row-trailing controls keep a compact target with a smaller glyph.
const TINY_ICON_BUTTON_SIZE: f32 = Layout::CONTROL_COMPACT_HEIGHT;

/// The colors an icon button resolves to.
#[derive(Debug, Clone, Copy)]
pub struct IconButtonPalette {
    pub foreground: gpui::Rgba,
    pub background: gpui::Rgba,
    pub border: gpui::Rgba,
}

pub fn icon_button_palette(
    variant: IconButtonVariant,
    selected: bool,
    hovered: bool,
    enabled: bool,
    tint: Option<gpui::Rgba>,
    theme: &crate::theme::Theme,
) -> IconButtonPalette {
    use gpui::Rgba;
    let clear = Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };

    let foreground = if !enabled {
        Colors::fg_quaternary(theme)
    } else if let Some(tint) = tint {
        tint
    } else if variant == IconButtonVariant::Ghost {
        if selected {
            Colors::accent(theme)
        } else if hovered {
            Colors::fg(theme)
        } else {
            Colors::fg_tertiary(theme)
        }
    } else if selected || hovered {
        Colors::fg(theme)
    } else {
        Colors::fg_tertiary(theme)
    };

    let background = if variant == IconButtonVariant::Ghost {
        if enabled && hovered {
            Colors::bg_hover(theme)
        } else {
            clear
        }
    } else if !enabled {
        Colors::bg_surface(theme).with_alpha(0.3)
    } else if selected {
        Colors::bg_selected(theme)
    } else if hovered {
        Colors::bg_hover(theme)
    } else {
        Colors::bg_surface(theme).with_alpha(0.55)
    };

    // The edge does not answer the pointer: the focus ring is drawn as the
    // border, and GPUI applies a hover after the focus refinement, so a hover
    // edge would hide the ring under the pointer. The fill carries the hover.
    let border = if variant == IconButtonVariant::Filled && selected {
        Colors::border_medium(theme)
    } else {
        clear
    };

    IconButtonPalette {
        foreground,
        background,
        border,
    }
}

/// Everything that varies about an icon button.
///
/// Grouped into a struct because the parameter list was nine positional
/// arguments, at which point a caller has to count to get it right.
#[derive(Debug, Clone, PartialEq)]
pub struct IconButtonProps {
    pub icon: Icon,
    /// Accessible label and tooltip text.
    pub label: SharedString,
    pub selected: bool,
    /// Overrides the state-derived foreground.
    pub tint: Option<gpui::Rgba>,
    pub enabled: bool,
    pub variant: IconButtonVariant,
    pub size: f32,
    pub icon_size: IconSize,
}

impl IconButtonProps {
    pub fn new(icon: Icon, label: impl Into<SharedString>) -> Self {
        Self {
            icon,
            label: label.into(),
            selected: false,
            tint: None,
            enabled: true,
            variant: IconButtonVariant::default(),
            size: ICON_BUTTON_SIZE,
            icon_size: IconSize::Control,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn tint(mut self, tint: gpui::Rgba) -> Self {
        self.tint = Some(tint);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn variant(mut self, variant: IconButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn icon_size(mut self, size: IconSize) -> Self {
        self.icon_size = size;
        self
    }

    /// The compact size, for a toolbar beside text.
    pub fn small(mut self) -> Self {
        self.size = SMALL_ICON_BUTTON_SIZE;
        self.icon_size = IconSize::Control;
        self
    }

    /// The row-trailing size, for a disclosure or per-row action.
    ///
    /// Keeps the compact hit target with a smaller glyph, so a trailing action
    /// does not compete with the row label.
    pub fn tiny(mut self) -> Self {
        self.size = TINY_ICON_BUTTON_SIZE;
        self.icon_size = IconSize::Small;
        self
    }

    /// No resting fill, so the control borrows its row's background.
    pub fn ghost(mut self) -> Self {
        self.variant = IconButtonVariant::Ghost;
        self
    }

    /// Derive a stable element id, so two buttons with the same label in
    /// different regions do not collide.
    fn element_id(&self) -> SharedString {
        format!("icon-button:{}:{}", self.icon.name(), self.label).into()
    }
}

/// Hit target for a row-trailing icon button.
///
/// The whole minimum-sized control owns click handling, including its padding.
const TINY_ICON_BUTTON_HIT: f32 = Layout::CONTROL_COMPACT_HEIGHT;

/// A square icon-only control.
///
/// Its edge is fixed, so the focus ring needs no padding given back.
pub fn icon_button(
    props: IconButtonProps,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let theme = global::current();
    // Resolve the id before destructuring, since it reads the whole struct.
    let button_id = props.element_id();
    let IconButtonProps {
        icon,
        label,
        selected,
        tint,
        enabled,
        variant,
        size,
        icon_size,
    } = props;

    let palette = icon_button_palette(variant, selected, false, enabled, tint, &theme);
    let debug_label = format!("icon-button:{label}");
    let corner = if variant == IconButtonVariant::Ghost {
        Radii::SM
    } else {
        Radii::XS
    };

    div()
        .debug_selector(move || debug_label.clone())
        .flex()
        .size(px_len(size.max(TINY_ICON_BUTTON_HIT)))
        .flex_shrink_0()
        .flex_col()
        .items_center()
        .justify_center()
        .rounded(px_len(corner))
        .border(px_len(BorderWidth::HAIRLINE))
        .border_color(palette.border)
        .bg(palette.background)
        .child(icon.render(icon_size, palette.foreground, &theme))
        // `.id` upgrades the element to `Stateful<Div>`, which is what carries
        // `on_click` and `tooltip` in GPUI 0.2.2.
        .id(button_id)
        .focus(|s: StyleRefinement| s.fx_focus_ring())
        .hover(|s: StyleRefinement| {
            let p = icon_button_palette(variant, selected, true, enabled, tint, &theme);
            s.fx_background(p.background)
        })
        .active(|s| {
            if enabled {
                s.fx_opacity(PRESSED_OPACITY)
            } else {
                s
            }
        })
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::default()
        })
        .tooltip(move |window, cx| crate::components::tooltip::tooltip(label.clone(), window, cx))
        .on_click(move |event, window, cx| {
            if enabled {
                on_click(event, window, cx);
            }
        })
}

/// A dense button for panel toolbars, where vertical space is scarce.
pub fn compact_action_button(
    label: impl Into<SharedString>,
    icon: Option<Icon>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    compact_action_toggle(label, icon, false, true, on_click)
}

/// How a compact action looks: `(foreground, background, border)`.
pub fn compact_action_palette(
    selected: bool,
    hovered: bool,
    theme: &crate::theme::Theme,
) -> (gpui::Rgba, gpui::Rgba, gpui::Rgba) {
    if selected {
        (
            Colors::fg(theme),
            Colors::bg_selected(theme),
            Colors::border_medium(theme),
        )
    } else if hovered {
        (
            Colors::fg(theme),
            Colors::bg_hover(theme),
            Colors::border_subtle(theme),
        )
    } else {
        (
            Colors::fg_secondary(theme),
            Colors::bg_elevated(theme),
            Colors::border_subtle(theme),
        )
    }
}

/// A compact action that can show an on state, such as a panel it reveals,
/// or be disabled while the work it starts is still running.
pub fn compact_action_toggle(
    label: impl Into<SharedString>,
    icon: Option<Icon>,
    selected: bool,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let theme = global::current();
    let label = label.into();
    let button_id: SharedString = format!("compact-action:{label}").into();
    let debug_label = button_id.to_string();
    let (foreground, background, border) = compact_action_palette(selected, false, &theme);

    let mut content = div()
        .debug_selector(move || debug_label)
        .flex()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap(px_len(Spacing::XXS));
    if let Some(i) = icon {
        content = content.child(i.render(IconSize::Micro, foreground, &theme));
    }
    content = content
        .child(
            div()
                .whitespace_nowrap()
                .fx_role(TextRole::CaptionMedium)
                .child(label),
        )
        .px(px_len(Spacing::SM))
        .min_h(px_len(Layout::CONTROL_COMPACT_HEIGHT))
        .rounded(px_len(Radii::SM))
        .border(px_len(BorderWidth::HAIRLINE))
        .border_color(border)
        .bg(background)
        .text_color(foreground)
        .opacity(if enabled { 1.0 } else { DISABLED_OPACITY });

    content
        .id(button_id)
        .focus(|s: StyleRefinement| {
            s.fx_focus_ring().px(px_len(
                Spacing::SM - BorderWidth::focus_give_back(BorderWidth::HAIRLINE),
            ))
        })
        .hover(move |s: StyleRefinement| {
            if !enabled || selected {
                return s;
            }
            let (foreground, background, _) = compact_action_palette(false, true, &theme);
            s.fx_background(background).fx_text_color(foreground)
        })
        .active(move |s| {
            if enabled {
                s.fx_opacity(PRESSED_OPACITY)
            } else {
                s
            }
        })
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::default()
        })
        .on_click(move |event, window, cx| {
            if enabled {
                on_click(event, window, cx);
            }
        })
}

/// A glyph-sized action for rows too dense for an icon button's hit target,
/// such as a diff file header that shares the code rows' height.
///
/// The click stops propagating, so a glyph inside a clickable row does not
/// also trigger the row.
pub fn glyph_button(
    id: impl Into<SharedString>,
    icon: Icon,
    label: impl Into<SharedString>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let theme = global::current();
    let label = label.into();
    let debug_label = format!("glyph-button:{label}");
    div()
        .debug_selector(move || debug_label.clone())
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .p(px_len(Spacing::XXXS))
        .rounded(px_len(Radii::XS))
        .child(icon.render(IconSize::Small, Colors::fg_tertiary(&theme), &theme))
        .id(id.into())
        .focus(|s: StyleRefinement| {
            s.fx_focus_ring()
                .p(px_len(Spacing::XXXS - BorderWidth::focus_give_back(0.0)))
        })
        .hover(|s: StyleRefinement| s.fx_background(Colors::bg_hover(&theme)))
        .active(|s| s.fx_opacity(PRESSED_OPACITY))
        .cursor(CursorStyle::PointingHand)
        .tooltip(move |window, cx| crate::components::tooltip::tooltip(label.clone(), window, cx))
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            on_click(event, window, cx);
        })
}

/// A text-only disclosure, such as "Show full message".
///
/// No chrome at all: it reads as part of the content it expands rather than as
/// a toolbar control, which is how the original drew it under a long prompt.
pub fn text_disclosure(
    label: impl Into<SharedString>,
    expanded: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let theme = global::current();
    let label = label.into();
    let chevron = if expanded {
        Icon::CHEVRON_UP
    } else {
        Icon::CHEVRON_DOWN
    };
    div()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .gap(px_len(Spacing::XS))
        .text_color(Colors::fg_secondary(&theme))
        .child(
            div()
                .whitespace_nowrap()
                .fx_role(TextRole::CaptionMedium)
                .child(label.clone()),
        )
        .child(chevron.render(IconSize::Micro, Colors::fg_secondary(&theme), &theme))
        .rounded(px_len(Radii::XS))
        .id(SharedString::from(format!("disclosure:{label}")))
        // No padding to give back, so the ring takes its width from a
        // negative margin and nothing beside the disclosure moves.
        .focus(|s: StyleRefinement| {
            s.fx_focus_ring()
                .m(px_len(-BorderWidth::focus_give_back(0.0)))
        })
        .hover(|s: StyleRefinement| s.fx_text_color(Colors::fg(&theme)))
        .active(|s| s.fx_opacity(PRESSED_OPACITY))
        .cursor(CursorStyle::PointingHand)
        .on_click(on_click)
}

/// The focus ring of a choice row.
///
/// A selected row's edge is already the accent, and the focus color can be
/// the accent itself, so focusing the selected row would change only the
/// edge's width. It takes `fg` instead, which reads on its fill like any ring.
pub fn choice_row_focus_ring(selected: bool, theme: &crate::theme::Theme) -> gpui::Rgba {
    if selected {
        Colors::fg(theme)
    } else {
        Colors::focus_ring(theme)
    }
}

/// The fill a choice row is painted with.
pub fn choice_row_fill(selected: bool, theme: &crate::theme::Theme) -> gpui::Rgba {
    if selected {
        Colors::bg_selected(theme)
    } else {
        Colors::bg_elevated(theme)
    }
}

/// A selectable answer or preference, with enough room for its explanation.
/// Unlike toolbar buttons, both labels wrap within the available panel width.
///
/// The row's id comes from its label, so two groups that can offer the same
/// label each need an id of their own around their rows. Without one, both
/// rows share one interaction state: a press on either shows on both, and a
/// click on the second never arrives.
///
/// With `focus`, the keyboard reaches the row as it reaches a dialog's
/// buttons: the dialog lists the handle in its Tab order, the row shows its
/// focus ring, and Space chooses it. A dialog whose answers are choices
/// alone had no other way to be answered from the keyboard.
pub fn choice_row(
    label: impl Into<SharedString>,
    description: impl Into<SharedString>,
    selected: bool,
    enabled: bool,
    focus: Option<&FocusHandle>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let on_click = Rc::new(on_click);
    let on_key: super::dialog::DialogHandler = {
        let on_click = on_click.clone();
        Rc::new(move |window, cx| on_click(&ClickEvent::default(), window, cx))
    };
    let theme = global::current();
    let label = label.into();
    let description = description.into();
    let mut labels = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px_len(Spacing::XXS))
        .child(
            div()
                .fx_role(TextRole::BodyMedium)
                .text_color(Colors::fg(&theme))
                .child(label.clone()),
        );
    if !description.is_empty() {
        labels = labels.child(
            div()
                .fx_role(TextRole::Caption)
                .text_color(Colors::fg_secondary(&theme))
                .child(description),
        );
    }
    let row = div()
        .id(SharedString::from(format!("choice:{label}")))
        .w_full()
        .min_w_0()
        .flex()
        .items_start()
        .gap(px_len(Spacing::SM))
        .p(px_len(Spacing::MD))
        .rounded(px_len(Radii::SM))
        .border(px_len(BorderWidth::REGULAR))
        .border_color(if selected {
            Colors::accent(&theme)
        } else {
            Colors::border_subtle(&theme)
        })
        .bg(choice_row_fill(selected, &theme))
        .opacity(if enabled { 1.0 } else { DISABLED_OPACITY })
        .cursor(if enabled {
            CursorStyle::PointingHand
        } else {
            CursorStyle::default()
        })
        .focus(move |s| {
            s.fx_focus_ring_in(choice_row_focus_ring(selected, &theme))
                .p(px_len(
                    Spacing::MD - BorderWidth::focus_give_back(BorderWidth::REGULAR),
                ))
        })
        .hover(move |s| {
            if enabled && !selected {
                s.fx_background(Colors::bg_hover(&theme))
            } else {
                s
            }
        })
        .active(move |s| {
            if enabled {
                s.fx_opacity(PRESSED_OPACITY)
            } else {
                s
            }
        })
        .on_click(move |event, window, cx| {
            if enabled {
                on_click(event, window, cx)
            }
        })
        .child(
            div()
                .flex_shrink_0()
                .pt(px_len(Spacing::XXXS))
                .child(if selected {
                    Icon::CHECKMARK_CIRCLE_FILL.render(
                        IconSize::Medium,
                        Colors::accent(&theme),
                        &theme,
                    )
                } else {
                    div()
                        .size(px_len(
                            crate::theme::Typography::icon(IconSize::Medium, &theme).size,
                        ))
                        .rounded_full()
                        .border(px_len(BorderWidth::REGULAR))
                        .border_color(Colors::border_medium(&theme))
                        .into_any_element()
                }),
        )
        .child(labels);
    super::dialog::focusable(row, focus.cloned(), on_key, enabled)
}

/// A row's trailing control that shows only while the row is hovered, or
/// always while `pinned`: the row is selected, or the control's menu is open.
///
/// Drawn transparent rather than hidden. GPUI registers no hitbox for a
/// hidden element, and the control has to stay where it is: the pointer
/// reaches it by hovering the row, which is what shows it, and a menu or
/// shortcut that names it still finds it. The row names itself with
/// `.group(group)`.
pub fn revealed_on_hover(
    control: gpui::Stateful<gpui::Div>,
    group: impl Into<SharedString>,
    pinned: bool,
) -> gpui::Stateful<gpui::Div> {
    if pinned {
        return control;
    }
    control
        .opacity(0.0)
        .group_hover(group, |s: StyleRefinement| s.fx_opacity(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{AccentColor, AppearanceMode, BaseTone, Theme};

    #[test]
    fn a_row_control_shows_on_hover_and_stays_while_pinned() {
        let control = || {
            icon_button(
                IconButtonProps::new(Icon::ELLIPSIS, "Actions"),
                |_, _, _| {},
            )
        };
        let mut resting = revealed_on_hover(control(), "row", false);
        assert_eq!(resting.style().opacity, Some(0.0), "unseen at rest");
        assert!(
            resting.style().visibility.is_none(),
            "transparent, never hidden, so the hit target stays"
        );
        let mut pinned = revealed_on_hover(control(), "row", true);
        assert_ne!(pinned.style().opacity, Some(0.0), "a pinned control shows");
    }

    fn themes() -> Vec<Theme> {
        let mut out = Vec::new();
        for tone in BaseTone::ALL {
            for mode in [AppearanceMode::Light, AppearanceMode::Dark] {
                out.push(Theme {
                    base_tone: tone,
                    appearance_mode: mode,
                    ..Default::default()
                });
            }
        }
        out
    }

    #[test]
    fn every_button_style_has_a_distinct_resting_background() {
        let theme = Theme::default();
        let seen: Vec<_> = [
            ButtonStyle::Primary,
            ButtonStyle::Secondary,
            ButtonStyle::Ghost,
            ButtonStyle::Danger,
        ]
        .into_iter()
        .map(|s| button_palette(s, false, &theme).background)
        .collect();
        for (i, a) in seen.iter().enumerate() {
            for b in &seen[i + 1..] {
                assert_ne!(a, b, "two button styles share a resting background");
            }
        }
    }

    const STYLES: [ButtonStyle; 4] = [
        ButtonStyle::Primary,
        ButtonStyle::Secondary,
        ButtonStyle::Ghost,
        ButtonStyle::Danger,
    ];

    #[test]
    fn every_style_changes_the_fill_its_hover_applies() {
        // `labelled_button` applies the hovered palette's background and
        // foreground, nothing else. A style whose hover lived only in a field
        // the renderer never read gave no feedback at all: Danger did.
        for theme in themes() {
            for style in STYLES {
                let rest = button_palette(style, false, &theme);
                let hover = button_palette(style, true, &theme);
                assert_ne!(
                    rest.background, hover.background,
                    "{style:?} in {:?} {:?} draws the same fill under the pointer",
                    theme.base_tone, theme.appearance_mode
                );
            }
        }
    }

    #[test]
    fn a_hovered_secondary_button_stands_further_from_its_panel() {
        // GPUI's hover replaces the background. Swapping the opaque surface
        // for a 4% wash let the panel show through, so the hovered button
        // was flatter than the resting one.
        for theme in themes() {
            let rest = button_palette(ButtonStyle::Secondary, false, &theme);
            let hover = button_palette(ButtonStyle::Secondary, true, &theme);
            assert_eq!(hover.background.a, 1.0, "the hovered fill is a wash");
            for panel in [Colors::bg(&theme), Colors::bg_elevated(&theme)] {
                let lift =
                    |fill| crate::theme::contrast(crate::theme::composite(fill, panel), panel);
                assert!(
                    lift(hover.background) > lift(rest.background),
                    "{:?} {:?}: hovered {:.3} vs resting {:.3}",
                    theme.base_tone,
                    theme.appearance_mode,
                    lift(hover.background),
                    lift(rest.background)
                );
            }
        }
    }

    #[test]
    fn ghost_and_primary_carry_no_border() {
        let theme = Theme::default();
        for style in [ButtonStyle::Ghost, ButtonStyle::Primary] {
            assert_eq!(
                button_palette(style, false, &theme).border.a,
                0.0,
                "{style:?} should have no border"
            );
        }
        assert!(
            button_palette(ButtonStyle::Secondary, false, &theme)
                .border
                .a
                > 0.0
        );
    }

    #[test]
    fn the_danger_hover_is_its_tint_laid_over_itself() {
        // One fill that paints what the muted token painted twice would, so
        // the renderer, which applies a single background, shows it.
        for theme in themes() {
            let rest = button_palette(ButtonStyle::Danger, false, &theme).background;
            let hover = button_palette(ButtonStyle::Danger, true, &theme).background;
            assert_eq!((hover.r, hover.g, hover.b), (rest.r, rest.g, rest.b));
            for surface in [Colors::bg(&theme), Colors::bg_surface(&theme)] {
                let twice = crate::theme::composite(rest, crate::theme::composite(rest, surface));
                let once = crate::theme::composite(hover, surface);
                for (a, b) in [(once.r, twice.r), (once.g, twice.g), (once.b, twice.b)] {
                    assert!((a - b).abs() < 1e-5, "{once:?} is not {twice:?}");
                }
            }
        }
    }

    #[test]
    fn every_focus_ring_clears_non_text_contrast_on_its_own_fill() {
        // The ring is the element's border, painted at the edge of its own
        // fill, so it has to read against that fill over every surface, at
        // rest and hovered. Primary's fill is the accent, so its ring is
        // `on_accent`, held to its label's 3:1 on the resting accent; the
        // lighter hovered accent is a label question too (blue's is 2.96:1).
        for theme in themes() {
            for accent in crate::theme::AccentColor::ALL {
                let theme = Theme {
                    accent_color: accent,
                    ..theme
                };
                for style in STYLES {
                    let ring = button_palette(style, false, &theme).focus_ring;
                    let states: &[bool] = if style == ButtonStyle::Primary {
                        &[false]
                    } else {
                        &[false, true]
                    };
                    for &hovered in states {
                        let fill = button_palette(style, hovered, &theme).background;
                        for surface in [
                            Colors::bg(&theme),
                            Colors::bg_elevated(&theme),
                            Colors::bg_surface(&theme),
                        ] {
                            let ratio = crate::theme::contrast(
                                ring,
                                crate::theme::composite(fill, surface),
                            );
                            assert!(
                                ratio >= 3.0,
                                "{accent:?} {:?} {:?}: the {style:?} ring is {ratio:.2}:1 (hovered {hovered})",
                                theme.base_tone,
                                theme.appearance_mode
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_focused_choice_row_never_draws_the_edge_it_has_when_selected() {
        for theme in themes() {
            for accent in crate::theme::AccentColor::ALL {
                let theme = Theme {
                    accent_color: accent,
                    ..theme
                };
                for selected in [false, true] {
                    let ring = choice_row_focus_ring(selected, &theme);
                    let fill = crate::theme::composite(
                        choice_row_fill(selected, &theme),
                        Colors::bg_elevated(&theme),
                    );
                    assert!(crate::theme::contrast(ring, fill) >= 3.0);
                }
                assert_ne!(
                    choice_row_focus_ring(true, &theme),
                    Colors::accent(&theme),
                    "focusing the selected row would only thicken its edge"
                );
            }
        }
    }

    #[test]
    fn the_focus_ring_keeps_every_control_its_size() {
        // A control gives back from its padding what the ring adds to its
        // resting border, so nothing beside it moves when it takes focus.
        for resting in [0.0, BorderWidth::HAIRLINE, BorderWidth::REGULAR] {
            let give_back = BorderWidth::focus_give_back(resting);
            assert_eq!(resting + give_back, BorderWidth::FOCUS_RING);
        }
        // Every padding that gives some back has that much to give.
        for (padding, resting) in [
            (Spacing::MD, BorderWidth::HAIRLINE),
            (Spacing::XXS, BorderWidth::HAIRLINE),
            (Spacing::SM, BorderWidth::HAIRLINE),
            (Spacing::XXXS, 0.0),
            (Spacing::MD, BorderWidth::REGULAR),
        ] {
            assert!(padding - BorderWidth::focus_give_back(resting) >= 0.0);
        }
    }

    #[test]
    fn primary_foreground_stays_legible_on_its_fill_in_every_theme() {
        for theme in themes() {
            for accent in AccentColor::ALL {
                let theme = Theme {
                    accent_color: accent,
                    ..theme
                };
                let p = button_palette(ButtonStyle::Primary, false, &theme);
                let ratio = crate::theme::contrast(p.foreground, p.background);
                assert!(
                    ratio >= 3.0,
                    "{accent:?} {}: primary label is {ratio}:1",
                    theme.appearance_mode
                );
            }
        }
    }

    #[test]
    fn a_disabled_icon_button_recedes() {
        let theme = Theme::default();
        let enabled =
            icon_button_palette(IconButtonVariant::Filled, false, false, true, None, &theme);
        let disabled =
            icon_button_palette(IconButtonVariant::Filled, false, false, false, None, &theme);
        assert_ne!(enabled.foreground, disabled.foreground);
        assert_ne!(enabled.background, disabled.background);
    }

    #[test]
    fn a_ghost_icon_button_has_no_chrome_at_rest() {
        let theme = Theme::default();
        let p = icon_button_palette(IconButtonVariant::Ghost, false, false, true, None, &theme);
        assert_eq!(p.background.a, 0.0);
        assert_eq!(p.border.a, 0.0);
    }

    #[test]
    fn an_icon_button_answers_the_pointer_with_its_fill_never_its_edge() {
        // The edge belongs to the focus ring, which a hover applied after it
        // would otherwise replace under the pointer.
        for theme in themes() {
            for variant in [IconButtonVariant::Filled, IconButtonVariant::Ghost] {
                for selected in [false, true] {
                    let rest = icon_button_palette(variant, selected, false, true, None, &theme);
                    let hover = icon_button_palette(variant, selected, true, true, None, &theme);
                    assert_eq!(rest.border, hover.border, "{variant:?} {selected}");
                    if !selected {
                        assert_ne!(rest.background, hover.background, "{variant:?} is inert");
                    }
                }
            }
        }
        let theme = Theme::default();
        let rest = icon_button_palette(IconButtonVariant::Filled, false, false, true, None, &theme);
        assert_eq!(rest.border.a, 0.0, "resting border should be invisible");
    }

    #[test]
    fn an_explicit_tint_beats_every_state() {
        let theme = Theme::default();
        let tint = crate::theme::Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        for variant in [IconButtonVariant::Filled, IconButtonVariant::Ghost] {
            for selected in [true, false] {
                for hovered in [true, false] {
                    let p =
                        icon_button_palette(variant, selected, hovered, true, Some(tint), &theme);
                    assert_eq!(p.foreground, tint, "tint was overridden");
                }
            }
        }
    }

    #[test]
    fn a_selected_compact_action_reads_as_on_in_every_theme() {
        for theme in themes() {
            let (rest_fg, rest_bg, _) = compact_action_palette(false, false, &theme);
            let (on_fg, on_bg, _) = compact_action_palette(true, false, &theme);
            let (_, hover_bg, _) = compact_action_palette(false, true, &theme);
            assert_ne!(rest_bg, on_bg);
            assert_ne!(rest_bg, hover_bg, "no hover feedback");
            // The fills are translucent washes over the elevated panel.
            let panel = Colors::bg_elevated(&theme);
            for (fg, bg) in [(on_fg, on_bg), (rest_fg, rest_bg)] {
                let fill = crate::theme::composite(bg, panel);
                assert!(crate::theme::contrast(fg, fill) >= 4.5);
            }
        }
    }

    #[test]
    fn a_selected_navigation_button_keeps_its_fill_under_the_pointer() {
        for theme in themes() {
            let selected = navigation_palette(true, false, &theme);
            assert_eq!(selected.background, Colors::bg_selected(&theme));
            assert_eq!(
                navigation_palette(true, true, &theme).background,
                selected.background
            );
            // The others still answer the pointer, as a ghost button does.
            let rest = navigation_palette(false, false, &theme);
            let hover = navigation_palette(false, true, &theme);
            assert_eq!(rest.background.a, 0.0);
            assert_eq!(hover.background, Colors::bg_hover(&theme));
        }
    }

    #[test]
    fn selection_tints_a_ghost_glyph_with_the_accent() {
        let theme = Theme::default();
        let selected =
            icon_button_palette(IconButtonVariant::Ghost, true, false, true, None, &theme);
        assert_eq!(selected.foreground, Colors::accent(&theme));
    }
}
