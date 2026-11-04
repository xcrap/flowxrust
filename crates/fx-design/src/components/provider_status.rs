//! Provider identity and live activity in one compact sidebar tile.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{AnyElement, App, IntoElement, Rgba, SharedString, Window, div, px};

use super::{ActivityState, spinner_arc};
use crate::icon::Icon;
use crate::theme::{
    BorderWidth, Colors, FxAnimation, IconSize, Radii, Spacing, Theme, WithAlpha, global,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProviderIdentity {
    #[default]
    Codex,
    Claude,
}

impl ProviderIdentity {
    pub fn color(self, theme: &Theme) -> Rgba {
        match self {
            Self::Codex => Colors::provider_codex(theme),
            Self::Claude => Colors::provider_claude(theme),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude",
        }
    }
}

/// What a status tile paints, split out so it can be asserted without a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProviderStatusPalette {
    /// The glyph: the provider's identity, or `error` once a turn has failed.
    pub glyph: Rgba,
    pub border: Rgba,
    /// The provider's tint behind the glyph.
    pub fill: Rgba,
}

/// The tile's colours for one activity, at one point of the waiting pulse.
///
/// `pulse` runs from 0 to 1 and back while waiting, and is ignored otherwise.
/// The fill is always the provider's tint, so a failed tile still says whose
/// it is; only the glyph and its edge turn to `error`.
pub fn provider_status_palette(
    provider: ProviderIdentity,
    activity: ActivityState,
    pulse: f32,
    theme: &Theme,
) -> ProviderStatusPalette {
    let identity = provider.color(theme);
    let glyph = if activity == ActivityState::Error {
        Colors::error(theme)
    } else {
        identity
    };
    let (border, fill) = if activity == ActivityState::Waiting {
        let pulse = pulse.clamp(0.0, 1.0);
        (0.3 + 0.4 * pulse, 0.08 + 0.1 * pulse)
    } else {
        (0.25, 0.12)
    };
    ProviderStatusPalette {
        glyph,
        border: glyph.with_alpha(border),
        fill: identity.with_alpha(fill),
    }
}

/// How long from `now` until the next multiple of `frame`, counted from one
/// process-wide origin, so timers started at different moments fire together
/// and their refreshes land in one frame.
fn delay_to_next_beat(now: Instant, frame: Duration) -> Duration {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(|| now);
    let frame_ns = frame.as_nanos().max(1);
    let into = now.saturating_duration_since(origin).as_nanos() % frame_ns;
    Duration::from_nanos((frame_ns - into) as u64)
}

pub fn provider_status(
    id: impl Into<SharedString>,
    provider: ProviderIdentity,
    activity: ActivityState,
    label: &'static str,
) -> impl IntoElement {
    ProviderStatus {
        id: id.into(),
        provider,
        activity,
        label,
    }
}

#[derive(IntoElement)]
struct ProviderStatus {
    id: SharedString,
    provider: ProviderIdentity,
    activity: ActivityState,
    label: &'static str,
}

impl RenderOnce for ProviderStatus {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = global::current();
        let (clock, scheduled) = window.with_element_namespace(self.id.clone(), |window| {
            (
                window.use_keyed_state("activity-clock", cx, |_, _| Instant::now()),
                window.use_keyed_state("activity-frame", cx, |_, _| false),
            )
        });
        let elapsed = clock.read(cx).elapsed().as_secs_f32();
        let waiting = self.activity == ActivityState::Waiting;
        let running = self.activity == ActivityState::Running;
        // One timed repaint at a time, not a frame per display refresh. Settled
        // tiles schedule nothing, so motion ends as soon as the state does.
        if (running || waiting) && !*scheduled.read(cx) {
            scheduled.update(cx, |scheduled, _| *scheduled = true);
            let frame = if waiting || !window.is_window_active() {
                FxAnimation::QUIET_ACTIVITY_FRAME
            } else {
                FxAnimation::ACTIVITY_FRAME
            };
            // Every tile wakes on the same beat. Each one counting from its
            // own last paint, three running threads refreshed the window at
            // three unrelated phases, close to the display's full rate.
            let delay = delay_to_next_beat(cx.background_executor().now(), frame);
            window
                .spawn(cx, async move |cx| {
                    // The platform's timer, which a test clock can advance.
                    cx.background_executor().timer(delay).await;
                    let _ = cx.update(|window, cx| {
                        scheduled.update(cx, |scheduled, _| *scheduled = false);
                        window.refresh();
                    });
                })
                .detach();
        }
        let pulse = if waiting {
            let phase = elapsed / FxAnimation::STATUS_PULSE.as_secs_f32();
            0.5 + 0.5 * (phase * std::f32::consts::TAU).cos()
        } else {
            1.0
        };
        let palette = provider_status_palette(self.provider, self.activity, pulse, &theme);
        let ink = palette.glyph;
        let glyph: AnyElement = match self.activity {
            ActivityState::Running => spinner_arc(
                ink,
                elapsed / FxAnimation::ACTIVITY_SPIN.as_secs_f32() * 360.0,
            )
            .into_any_element(),
            ActivityState::Completed => Icon::CHECKMARK
                .render(IconSize::Small, ink, &theme)
                .into_any_element(),
            ActivityState::Error => Icon::XMARK
                .render(IconSize::Small, ink, &theme)
                .into_any_element(),
            ActivityState::Waiting => div()
                .flex()
                .gap(px(Spacing::XXXS))
                .children((0..2).map(|_| {
                    div()
                        .w(px(Spacing::XXXS))
                        .h(px(Spacing::SM))
                        .rounded(px(Radii::XS))
                        .bg(ink)
                }))
                .into_any_element(),
            ActivityState::Idle => div()
                .size(px(Spacing::XXS))
                .rounded_full()
                .bg(ink)
                .into_any_element(),
        };
        let detail = format!("{} · {}", self.provider.label(), self.label);
        div()
            .id(self.id)
            .debug_selector(|| "provider-status".into())
            .size(px(Spacing::XL))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(Radii::SM))
            .border(px(BorderWidth::HAIRLINE))
            .border_color(palette.border)
            .bg(palette.fill)
            .tooltip(move |window, cx| super::tooltip(detail.clone(), window, cx))
            .child(glyph)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_started_apart_wake_on_the_same_beat() {
        let frame = FxAnimation::ACTIVITY_FRAME;
        let start = Instant::now();
        let first = delay_to_next_beat(start, frame);
        assert!(first > Duration::ZERO && first <= frame);
        for offset_ms in [1, 7, 20, 32, 45, 101] {
            let later = start + Duration::from_millis(offset_ms);
            let next = later + delay_to_next_beat(later, frame);
            let beat = start + first;
            let apart = next.saturating_duration_since(beat).as_nanos() % frame.as_nanos();
            assert_eq!(
                apart, 0,
                "a tile started {offset_ms} ms later woke off the beat"
            );
            assert!(next > later, "a beat is never in the past");
        }
    }
}
