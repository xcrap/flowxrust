use gpui::prelude::*;
use gpui::{
    App, ClickEvent, IntoElement, ParentElement, Rgba, SharedString, Styled, Window, div, px,
};

use crate::icon::Icon;
use crate::styled::{FxRefinement, FxStyled, px_len};
use crate::theme::{
    Beat, Colors, FxAnimation, IconSize, Motion, Spacing, TextRole, Typography, WithAlpha, global,
};

/// One layer centered in a fixed square. Absolute positioning keeps overlaid
/// rings, fills, and glyphs out of flex layout and prevents them shrinking each
/// other into strips when their combined widths exceed the control.
fn centered_layer(outer: f32, inner: f32) -> gpui::Div {
    let inset = (outer - inner) / 2.0;
    div()
        .absolute()
        .left(px(inset))
        .top(px(inset))
        .size(px(inner))
}

/// Semantic tone of a badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeTone {
    #[default]
    Neutral,
    Accent,
    AccentSecondary,
    Success,
    Warning,
    Error,
    Info,
}

impl BadgeTone {
    pub const ALL: [Self; 7] = [
        Self::Neutral,
        Self::Accent,
        Self::AccentSecondary,
        Self::Success,
        Self::Warning,
        Self::Error,
        Self::Info,
    ];

    /// The hue, used for a badge's leading dot and by callers that have already
    /// established a legible background.
    pub fn hue(self, theme: &crate::theme::Theme) -> Rgba {
        match self {
            Self::Neutral => Colors::fg_secondary(theme),
            Self::Accent => Colors::accent(theme),
            Self::AccentSecondary => Colors::accent_secondary(theme),
            Self::Success => Colors::success(theme),
            Self::Warning => Colors::warning(theme),
            Self::Error => Colors::error(theme),
            Self::Info => Colors::info(theme),
        }
    }

    /// The tinted fill behind the label.
    pub fn fill(self, theme: &crate::theme::Theme) -> Rgba {
        match self {
            // Neutral is not a status, so it borrows the selected wash rather
            // than inventing a gray tint that would not adapt per mode.
            Self::Neutral => Colors::bg_selected(theme),
            Self::Accent => Colors::accent_muted(theme),
            Self::AccentSecondary => Colors::accent_secondary_muted(theme),
            Self::Success => Colors::success_muted(theme),
            Self::Warning => Colors::warning_muted(theme),
            Self::Error => Colors::error_muted(theme),
            Self::Info => Colors::info_muted(theme),
        }
    }

    /// The color of a badge's leading dot.
    ///
    /// A dot is a graphic, so it clears the 3:1 non-text bar rather than AA.
    pub fn dot(self, theme: &crate::theme::Theme) -> Rgba {
        match self {
            Self::Neutral => Colors::fg_tertiary(theme),
            other => other.hue(theme),
        }
    }

    /// The color of a badge's *label*.
    ///
    /// A badge is a 12pt caption at medium weight, so its text needs AA
    /// (4.5:1) against its own fill. The hue alone does not manage that: in
    /// light mode `warning` over its own tint lands near 2.6:1, which is
    /// unreadable at that size.
    ///
    /// So the hue is used wherever it is legible, and the strongest foreground
    /// takes over where it is not. This keeps the tint — which is what actually
    /// communicates the tone — while never shipping an illegible label.
    ///
    /// The fill is translucent, so what it reads as depends on what is under
    /// it. A badge sits on a panel as often as on a card, so both surfaces are
    /// measured: judged over the panel alone, the dark "Ready" label on a
    /// provider card was 4.2:1.
    pub fn text_ink(self, theme: &crate::theme::Theme) -> Rgba {
        let fills = [Colors::bg_elevated(theme), Colors::bg_surface(theme)]
            .map(|surface| crate::theme::composite(self.fill(theme), surface));
        crate::theme::legible_or(
            self.hue(theme),
            Colors::fg(theme),
            &fills,
            Self::TEXT_CONTRAST_FLOOR,
        )
    }
}

impl BadgeTone {
    /// AA for text, which is what a badge's label is.
    const TEXT_CONTRAST_FLOOR: f64 = 4.5;
}

/// Diameter of a badge's optional leading dot.
const BADGE_DOT_SIZE: f32 = 6.0;

/// A pill-shaped label, optionally led by a dot.
pub fn badge(text: impl Into<SharedString>, tone: BadgeTone, show_dot: bool) -> impl IntoElement {
    let theme = global::current();
    let mut content = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px_len(Spacing::XXS));
    if show_dot {
        content = content.child(
            div()
                .size(px_len(BADGE_DOT_SIZE))
                .rounded_full()
                .bg(tone.dot(&theme)),
        );
    }
    content
        .child(div().fx_role(TextRole::CaptionMedium).child(text.into()))
        .px(px_len(Spacing::SM))
        .py(px_len(Spacing::XXXS))
        .bg(tone.fill(&theme))
        .text_color(tone.text_ink(&theme))
        .rounded_full()
}

/// Outer and inner diameters of an activity dot.
const ACTIVITY_OUTER: f32 = 16.0;
const ACTIVITY_INNER: f32 = 7.0;
const ACTIVITY_RING: f32 = 13.0;
const ACTIVITY_STROKE: f32 = 1.5;

/// What an activity dot is reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityState {
    /// Present but doing nothing.
    #[default]
    Idle,
    /// A turn is in flight.
    Running,
    /// Blocked on an approval or a question.
    Waiting,
    /// Finished successfully.
    Completed,
    /// Failed.
    Error,
}

impl ActivityState {
    pub const ALL: [Self; 5] = [
        Self::Idle,
        Self::Running,
        Self::Waiting,
        Self::Completed,
        Self::Error,
    ];

    /// The status ring's color. `None` means the ring is an arc of the identity
    /// color, which is what `Running` draws.
    pub fn ring_color(self, identity: Rgba, theme: &crate::theme::Theme) -> Option<Rgba> {
        match self {
            Self::Idle => Some(identity.with_alpha(0.14)),
            Self::Running => None,
            Self::Waiting => Some(Colors::warning(theme)),
            Self::Completed => Some(Colors::success(theme)),
            Self::Error => Some(Colors::error(theme)),
        }
    }
}

/// A compact identity dot that can also report live activity.
///
/// This keeps a sidebar row to a single line: the dot replaces a second badge or
/// a status label beside the title. A running dot's arc turns on the activity
/// clock, and holds still under reduced motion.
pub fn activity_dot(identity: Rgba, state: ActivityState) -> impl IntoElement {
    let theme = global::current();
    let inner = div()
        .size(px_len(ACTIVITY_INNER))
        .rounded_full()
        .bg(identity);

    let ring: gpui::AnyElement = match state.ring_color(identity, &theme) {
        // A filled disc, sitting behind the identity dot.
        Some(color) => div()
            .size(px_len(ACTIVITY_RING))
            .rounded_full()
            .bg(color)
            .into_any_element(),
        // An arc, drawn as rotated capsules, on the activity clock.
        None => super::spinner(identity).into_any_element(),
    };

    let ring_size = if state == ActivityState::Running {
        SPINNER_RING_SIZE
    } else {
        ACTIVITY_RING
    };
    div()
        .relative()
        .size(px_len(ACTIVITY_OUTER))
        .flex_shrink_0()
        .debug_selector(|| "activity-dot".into())
        .child(
            centered_layer(ACTIVITY_OUTER, ring_size)
                .debug_selector(|| "activity-ring".into())
                .child(ring),
        )
        .child(
            centered_layer(ACTIVITY_OUTER, ACTIVITY_INNER)
                .debug_selector(|| "activity-inner".into())
                .child(inner),
        )
}

/// Number of segments used to approximate the spinner arc.
const SPINNER_SEGMENTS: usize = 12;
/// Fraction of the circle the arc spans.
const SPINNER_START: f32 = 0.08;
const SPINNER_END: f32 = 0.68;
const SPINNER_RING_SIZE: f32 = 14.0;

/// A partial ring, approximated with rotated capsules.
///
/// `spin` is in degrees. This draws one frame and asks for no other: the
/// spinner that turns is [`spinner`](super::spinner), which reads its angle
/// from the activity clock.
pub fn spinner_arc(color: Rgba, spin: f32) -> impl IntoElement {
    let radius = (SPINNER_RING_SIZE - ACTIVITY_STROKE) / 2.0;
    let stroke = ACTIVITY_STROKE;
    let center = SPINNER_RING_SIZE / 2.0;
    let span = (SPINNER_END - SPINNER_START).clamp(0.0, 1.0);

    // Lay the capsules out on the circumference rather than rotating one
    // capsule, so the arc has no visible seam as it spins. Only the segments
    // inside [SPINNER_START, SPINNER_END) are drawn, which is what makes this
    // an arc rather than a ring.
    let segments: Vec<_> = (0..SPINNER_SEGMENTS)
        .filter_map(|i| {
            let t = (i as f32 + 0.5) / SPINNER_SEGMENTS as f32;
            if t < SPINNER_START || t > SPINNER_END {
                return None;
            }
            // Progress along the arc, so the trailing end can be faded out.
            let progress = (t - SPINNER_START) / span;
            let angle = (t * 360.0 + spin).to_radians();
            let fade = 0.35 + 0.65 * progress;
            Some(
                div()
                    .absolute()
                    .size(px_len(stroke))
                    .rounded_full()
                    .bg(color.with_alpha(fade))
                    .left(px(center + radius * angle.cos() - stroke / 2.0))
                    .top(px(center + radius * angle.sin() - stroke / 2.0))
                    .into_any_element(),
            )
        })
        .collect();

    div()
        .relative()
        .size(px_len(SPINNER_RING_SIZE))
        .child(div().absolute().size(px_len(SPINNER_RING_SIZE)))
        .children(segments)
        .overflow_hidden()
        .into_any_element()
}

/// A steady ring used when motion is reduced.
pub fn static_ring(color: Rgba) -> impl IntoElement {
    div()
        .size(px_len(SPINNER_RING_SIZE))
        .rounded_full()
        .border_2()
        .border_color(color.with_alpha(0.75))
}

/// Where a piece of work is: a tool call, a background task, a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusGlyph {
    Running,
    Done,
    Failed,
    /// Ended before it finished, by the reader or by the work around it.
    Stopped,
}

impl StatusGlyph {
    pub const ALL: [Self; 4] = [Self::Running, Self::Done, Self::Failed, Self::Stopped];

    /// The word a test or an accessibility label uses for the state.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }
}

/// The one size every status glyph is drawn at, so a done mark, a failure
/// and a stopped mark line up.
///
/// Small, not Medium. At 14pt, a finished call's mark made GPUI's list
/// reserve about 5,300pt for an activity group once it was opened: the group
/// was laid out at the top of that slot and everything above it, the prompt
/// included, was pushed out of view, leaving the transcript blank above the
/// reply. Seen in the real app on two threads and gone at 11pt; the test
/// platform's text system does not reproduce it, so no view test can catch
/// a change back.
pub const STATUS_GLYPH_SIZE: IconSize = IconSize::Small;

/// The colour of a status glyph.
///
/// Each glyph is the only sign of its state, so it clears the 3:1 a graphic
/// needs on every surface a row sits on, bare and hovered: the hue where it
/// reads, a deeper ink where it does not. Light-mode `success` is 2.9:1 on
/// `bg_surface` and takes `success_ink`; the accent takes `accent_ink`,
/// since dark violet is 3.0:1 on `bg_elevated`. `error` reads everywhere.
pub fn status_glyph_ink(glyph: StatusGlyph, theme: &crate::theme::Theme) -> Rgba {
    let fills = theme.palette().text_fills();
    let legible = |hue, fallback| {
        crate::theme::legible_or(hue, fallback, &fills, crate::theme::NON_TEXT_FLOOR)
    };
    match glyph {
        StatusGlyph::Running => Colors::accent_ink(theme),
        StatusGlyph::Done => legible(Colors::success(theme), Colors::success_ink(theme)),
        StatusGlyph::Failed => legible(Colors::error(theme), Colors::error_ink(theme)),
        StatusGlyph::Stopped => legible(Colors::fg_tertiary(theme), Colors::fg_secondary(theme)),
    }
}

/// A status glyph at [`STATUS_GLYPH_SIZE`].
///
/// `live` turns a running glyph into the spinner, on the activity clock;
/// without it, a running glyph is a still ring, which is what work that has
/// stopped reporting should show. Reduced motion stills it either way.
pub fn status_glyph(glyph: StatusGlyph, live: bool) -> gpui::AnyElement {
    let theme = global::current();
    let ink = status_glyph_ink(glyph, &theme);
    let slot = Typography::icon(STATUS_GLYPH_SIZE, &theme).size;
    let mark = match glyph {
        // The spinner keeps its own size: squeezed into the marks' slot, its
        // ring spilled over the edges.
        StatusGlyph::Running if live => return super::spinner(ink).into_any_element(),
        StatusGlyph::Running => return static_ring(ink).into_any_element(),
        StatusGlyph::Done => Icon::CHECKMARK_CIRCLE_FILL.render(STATUS_GLYPH_SIZE, ink, &theme),
        StatusGlyph::Failed => Icon::XMARK_CIRCLE_FILL.render(STATUS_GLYPH_SIZE, ink, &theme),
        StatusGlyph::Stopped => Icon::STOP_CIRCLE_FILL.render(STATUS_GLYPH_SIZE, ink, &theme),
    };
    div()
        .debug_selector(move || format!("status-glyph:{}", glyph.word()))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(slot))
        .child(mark)
        .into_any_element()
}

/// One dot of the typing indicator, at rest.
const TYPING_DOT_SIZE: f32 = 4.0;
/// The square each dot sits in: the lit dot's size, so the dot growing and
/// shrinking moves nothing beside it.
pub const TYPING_DOT_CELL: f32 = TYPING_DOT_SIZE * FxAnimation::TYPING_DOT_EMPHASIS;

/// Three dots that cycle while the assistant composes a reply, on the
/// activity clock. Under reduced motion they hold still at an even weight.
pub fn typing_indicator() -> TypingIndicator {
    TypingIndicator
}

#[derive(IntoElement)]
pub struct TypingIndicator;

impl RenderOnce for TypingIndicator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let beat = super::motion::activity_beat(Motion::Typing, window, cx);
        typing_dots(beat.map(Beat::typing_dot))
    }
}

/// The typing indicator with dot `lit` emphasised, or with every dot even
/// for reduced motion. One frame, asking for no other.
pub fn typing_dots(lit: Option<usize>) -> gpui::Div {
    let theme = global::current();
    let reduce_motion = lit.is_none();
    let phase = lit.unwrap_or(0);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px_len(Spacing::XXS))
        .children(
            (0..3)
                .map(|i| {
                    // GPUI cannot scale an element without a transform, so the
                    // emphasis is expressed as a slightly larger dot, inside a
                    // cell that is always the larger size.
                    let size =
                        TYPING_DOT_SIZE * FxAnimation::typing_dot_scale(i, phase, reduce_motion);
                    div()
                        .debug_selector(move || format!("typing-dot:{i}"))
                        .size(px_len(TYPING_DOT_CELL))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .size(px(size))
                                .rounded_full()
                                .bg(Colors::fg_tertiary(&theme))
                                .opacity(FxAnimation::typing_dot_opacity(i, phase, reduce_motion)),
                        )
                })
                .collect::<Vec<_>>(),
        )
}

/// The caret's width. A hairline would disappear at some zoom levels and a
/// wider bar stops reading as a cursor.
pub const CARET_WIDTH: f32 = 2.0;
/// The caret's opacity on the half of its blink it is faded.
pub const CARET_FADED_OPACITY: f32 = 0.25;

/// The caret at the end of a reply still streaming in, `height` tall,
/// blinking on the activity clock: solid half a second, faded half a second.
///
/// A blink is not travel, so it keeps blinking under reduced motion, as the
/// system's own caret does.
pub fn streaming_caret(color: Rgba, height: f32) -> StreamingCaret {
    StreamingCaret { color, height }
}

#[derive(IntoElement)]
pub struct StreamingCaret {
    color: Rgba,
    height: f32,
}

impl RenderOnce for StreamingCaret {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        super::motion::request_activity_frame(Motion::Caret, window, cx);
        let solid = Beat::at(super::motion::now(cx)).caret_solid();
        div()
            .debug_selector(move || {
                format!("streaming-caret:{}", if solid { "solid" } else { "faded" })
            })
            .w(px(CARET_WIDTH))
            .flex_shrink_0()
            .h(px(self.height))
            .rounded_full()
            .bg(self.color)
            .opacity(if solid { 1.0 } else { CARET_FADED_OPACITY })
    }
}

/// Progress fraction at which the context ring escalates its color.
const CONTEXT_WARNING_AT: f32 = 0.75;
const CONTEXT_CRITICAL_AT: f32 = 0.9;

/// The color a context-usage ring adopts at a given fill level.
pub fn context_ring_color(progress: Option<f32>, theme: &crate::theme::Theme) -> Rgba {
    match progress {
        None => Colors::accent(theme),
        Some(p) if p >= CONTEXT_CRITICAL_AT => Colors::error(theme),
        Some(p) if p >= CONTEXT_WARNING_AT => Colors::warning(theme),
        Some(_) => Colors::accent(theme),
    }
}

/// A filled action button wrapped by a context-usage ring.
///
/// This is the composer's send control: the ring around it reports how much of
/// the model's context the conversation has used, so running long is visible
/// before it becomes a failure.
pub fn context_ring_icon(
    icon: Icon,
    progress: Option<f32>,
    tint: Rgba,
    size: f32,
    enabled: bool,
    hovered: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let theme = global::current();
    let clamped = progress.map(|p| p.clamp(0.0, 1.0));
    // An empty ring is indistinguishable from the track, so the progress arc
    // only appears once there is some.
    let shows_progress = clamped.is_some_and(|p| p > 0.0);
    let fill = if !enabled {
        Colors::bg_hover(&theme)
    } else if hovered {
        tint.with_alpha(0.86)
    } else {
        tint
    };
    let glyph = if enabled {
        Colors::on_accent(&theme)
    } else {
        Colors::fg_quaternary(&theme)
    };

    // The track, the progress arc, the button, then the glyph.
    let ring = div()
        .relative()
        .size(px_len(size))
        .flex_shrink_0()
        .debug_selector(|| "context-ring".into())
        .child(
            centered_layer(size, size)
                .debug_selector(|| "context-ring-track".into())
                .rounded_full()
                .border_2()
                .border_color(Colors::border_medium(&theme)),
        )
        .child(
            centered_layer(size, size)
                .rounded_full()
                .border_2()
                .border_color(
                    context_ring_color(clamped, &theme).with_alpha(if shows_progress {
                        1.0
                    } else {
                        0.0
                    }),
                )
                .opacity(if shows_progress { 1.0 } else { 0.0 }),
        )
        .child(
            centered_layer(size, size - Spacing::SM)
                .debug_selector(|| "context-ring-fill".into())
                .rounded_full()
                .bg(fill),
        )
        .child(
            centered_layer(size, Typography::icon(IconSize::Control, &theme).size)
                .debug_selector(|| "context-ring-glyph".into())
                .child(icon.render(IconSize::Control, glyph, &theme)),
        );

    ring
        // `.id` upgrades to `Stateful<Div>`, which carries `on_click`.
        .id("context-ring-action")
        .hover(|s| {
            if enabled && hovered {
                s.fx_opacity(1.0)
            } else {
                s
            }
        })
        .on_click(move |event, window, cx| {
            if enabled {
                on_click(event, window, cx);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{AppearanceMode, BaseTone, Theme, contrast};

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
    fn badge_hues_are_distinguishable_within_a_mode() {
        for theme in themes() {
            let hues: Vec<_> = BadgeTone::ALL.iter().map(|t| t.hue(&theme)).collect();
            for (i, a) in hues.iter().enumerate() {
                for (j, b) in hues.iter().enumerate().skip(i + 1) {
                    assert_ne!(
                        a,
                        b,
                        "{:?} and {:?} share a hue in {:?}",
                        BadgeTone::ALL[i],
                        BadgeTone::ALL[j],
                        theme.appearance_mode
                    );
                }
            }
        }
    }

    /// A tone's fill over each surface a badge is drawn on, as it renders.
    fn fills(tone: BadgeTone, theme: &Theme) -> [Rgba; 2] {
        [Colors::bg_elevated(theme), Colors::bg_surface(theme)]
            .map(|surface| crate::theme::composite(tone.fill(theme), surface))
    }

    #[test]
    fn badge_text_stays_legible_on_its_own_fill() {
        // A badge label is a 12pt caption, so it needs AA. This is the check
        // that motivated `text_ink` falling back to a strong foreground. The
        // provider rows in Settings put badges on a card, where the dark
        // "Ready" label measured over the panel alone was 4.2:1.
        for theme in themes() {
            for tone in BadgeTone::ALL {
                for fill in fills(tone, &theme) {
                    let ratio = contrast(tone.text_ink(&theme), fill);
                    assert!(
                        ratio >= 4.5,
                        "{tone:?} badge text is {ratio}:1 in {:?} {:?}",
                        theme.base_tone,
                        theme.appearance_mode
                    );
                }
            }
        }
    }

    #[test]
    fn the_text_fallback_keeps_the_hue_wherever_the_hue_is_legible() {
        // The fallback must not flatten every badge to gray. Where the hue is
        // legible on both surfaces, it is kept.
        let theme = Theme {
            appearance_mode: AppearanceMode::Dark,
            ..Default::default()
        };
        let mut kept = 0;
        for tone in BadgeTone::ALL {
            if fills(tone, &theme)
                .iter()
                .all(|fill| contrast(tone.hue(&theme), *fill) >= 4.5)
            {
                assert_eq!(
                    tone.text_ink(&theme),
                    tone.hue(&theme),
                    "{tone:?} could have kept its hue"
                );
                kept += 1;
            }
        }
        // Neutral and warning. The other dark tints are legible on a panel but
        // not on a card, and a badge does not know which one it is on.
        assert!(kept >= 2, "only {kept} tones kept their hue in dark mode");
    }

    #[test]
    fn the_neutral_badge_keeps_its_hue_in_both_modes() {
        // Neutral reads `fg_secondary`, which is built to clear AA everywhere.
        for theme in themes() {
            assert_eq!(
                BadgeTone::Neutral.text_ink(&theme),
                BadgeTone::Neutral.hue(&theme)
            );
        }
    }

    #[test]
    fn every_badge_tone_has_a_translucent_fill_except_neutral() {
        for theme in themes() {
            for tone in BadgeTone::ALL {
                let fill = tone.fill(&theme);
                if tone == BadgeTone::Neutral {
                    // Neutral borrows `bg_selected`, which is a fixed alpha.
                    assert!(fill.a < 1.0);
                } else {
                    assert!(fill.a > 0.0 && fill.a < 1.0, "{tone:?}");
                }
            }
        }
    }

    #[test]
    fn the_neutral_badge_dot_differs_from_its_text() {
        // Both use foreground tokens; keeping them apart stops the pill from
        // reading as one flat shape.
        let theme = Theme::default();
        assert_ne!(
            BadgeTone::Neutral.dot(&theme),
            BadgeTone::Neutral.text_ink(&theme)
        );
    }

    #[test]
    fn only_the_running_state_draws_an_arc() {
        let theme = Theme::default();
        let identity = Colors::accent(&theme);
        assert!(
            ActivityState::Running
                .ring_color(identity, &theme)
                .is_none()
        );
        for state in [
            ActivityState::Idle,
            ActivityState::Waiting,
            ActivityState::Completed,
            ActivityState::Error,
        ] {
            assert!(
                state.ring_color(identity, &theme).is_some(),
                "{state:?} should use a filled ring"
            );
        }
    }

    #[test]
    fn the_status_rings_use_the_expected_semantic_hues() {
        let theme = Theme::default();
        let identity = Colors::accent(&theme);
        assert_eq!(
            ActivityState::Waiting.ring_color(identity, &theme),
            Some(Colors::warning(&theme))
        );
        assert_eq!(
            ActivityState::Completed.ring_color(identity, &theme),
            Some(Colors::success(&theme))
        );
        assert_eq!(
            ActivityState::Error.ring_color(identity, &theme),
            Some(Colors::error(&theme))
        );
    }

    #[test]
    fn the_idle_ring_is_a_faint_wash_of_the_identity_color() {
        let theme = Theme::default();
        let identity = Colors::accent(&theme);
        let ring = ActivityState::Idle.ring_color(identity, &theme).unwrap();
        assert_eq!(
            (ring.r, ring.g, ring.b),
            (identity.r, identity.g, identity.b)
        );
        assert!(
            ring.a < 0.3,
            "the idle ring should be a wash, got {}",
            ring.a
        );
    }

    #[test]
    fn the_spinner_arc_covers_its_documented_fraction_of_the_circle() {
        let span = SPINNER_END - SPINNER_START;
        assert!(
            (0.5..0.7).contains(&span),
            "the arc should cover roughly three fifths of the circle, was {span}"
        );
    }

    #[test]
    fn the_arc_is_drawn_with_enough_segments_to_look_smooth() {
        // Twelve capsules is indistinguishable from a trimmed stroke at 14pt.
        // Fewer would show as facets; more would cost draw calls for nothing.
        assert_eq!(
            SPINNER_SEGMENTS, 12,
            "changing the segment count changes the arc's smoothness"
        );
        // At least one segment must land inside the arc, or nothing renders.
        let inside = (0..SPINNER_SEGMENTS)
            .filter(|i| {
                let t = (*i as f32 + 0.5) / SPINNER_SEGMENTS as f32;
                (SPINNER_START..=SPINNER_END).contains(&t)
            })
            .count();
        assert!(inside >= 5, "only {inside} segments land inside the arc");
    }

    #[test]
    fn the_context_ring_escalates_as_the_window_fills() {
        let theme = Theme::default();
        let empty = context_ring_color(Some(0.1), &theme);
        let high = context_ring_color(Some(0.8), &theme);
        let full = context_ring_color(Some(0.95), &theme);

        assert_eq!(empty, Colors::accent(&theme));
        assert_eq!(high, Colors::warning(&theme));
        assert_eq!(full, Colors::error(&theme));
    }

    #[test]
    fn an_unknown_context_level_falls_back_to_the_accent() {
        let theme = Theme::default();
        assert_eq!(context_ring_color(None, &theme), Colors::accent(&theme));
    }

    #[test]
    fn every_finished_mark_shares_one_size_no_larger_than_the_spinner() {
        // The transcript drew a 9pt check and an 11pt failure side by side.
        // They share one slot now, and at 14pt that slot made GPUI's list
        // mis-measure an opened group of finished calls, so it stays at
        // Small; the spinner keeps its own ring.
        let theme = Theme::default();
        assert_eq!(STATUS_GLYPH_SIZE, IconSize::Small);
        assert!(Typography::icon(STATUS_GLYPH_SIZE, &theme).size <= SPINNER_RING_SIZE);
    }

    #[test]
    fn every_status_glyph_reads_as_its_own_state() {
        for theme in themes() {
            let inks: Vec<_> = StatusGlyph::ALL
                .iter()
                .map(|glyph| status_glyph_ink(*glyph, &theme))
                .collect();
            for (i, a) in inks.iter().enumerate() {
                for b in &inks[i + 1..] {
                    assert_ne!(a, b, "two states share a colour in {theme:?}");
                }
            }
            let done = status_glyph_ink(StatusGlyph::Done, &theme);
            assert!(done.g > done.r && done.g > done.b, "done is not a green");
            let failed = status_glyph_ink(StatusGlyph::Failed, &theme);
            assert!(
                failed.r > failed.g && failed.r > failed.b,
                "failed is not a red"
            );
        }
    }

    #[test]
    fn the_typing_indicator_always_lights_exactly_one_dot() {
        // The rhythm depends on this staying at three; changing the count
        // changes how the indicator reads.
        const DOTS: usize = 3;
        for phase in 0..DOTS {
            let lit = (0..DOTS)
                .filter(|i| FxAnimation::typing_dot_opacity(*i, phase, false) > 0.5)
                .count();
            assert_eq!(lit, 1, "phase {phase} lit {lit} dots");
        }
    }
}
