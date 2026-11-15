//! A spinner for work whose end FlowX cannot see coming: a transcript being
//! read, a provider's threads arriving, a diff being taken.
//!
//! It turns on its own. Each frame schedules the next repaint on the activity
//! beat, and only while it is drawn: a spinner that is no longer in the tree
//! schedules nothing, so motion ends with the wait.

use std::sync::OnceLock;
use std::time::Instant;

use gpui::prelude::*;
use gpui::{App, IntoElement, SharedString, Window, div};

use super::status::{StatusGlyph, status_glyph};
use crate::theme::FxAnimation;

/// A spinning status glyph, keyed by `id` so its schedule survives repaints.
pub fn loading_spinner(id: impl Into<SharedString>) -> impl IntoElement {
    LoadingSpinner { id: id.into() }
}

/// The spinner's angle at `now`, in degrees, counted from one process-wide
/// origin so every spinner on screen turns together.
pub fn spin_angle(now: Instant) -> f32 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(|| now);
    let turns = now.saturating_duration_since(origin).as_secs_f32()
        / FxAnimation::ACTIVITY_SPIN.as_secs_f32();
    turns.fract() * 360.0
}

#[derive(IntoElement)]
struct LoadingSpinner {
    id: SharedString,
}

impl RenderOnce for LoadingSpinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let scheduled = window.with_element_namespace(self.id.clone(), |window| {
            window.use_keyed_state("loading-frame", cx, |_, _| false)
        });
        if !*scheduled.read(cx) {
            scheduled.update(cx, |scheduled, _| *scheduled = true);
            let frame = if window.is_window_active() {
                FxAnimation::ACTIVITY_FRAME
            } else {
                FxAnimation::QUIET_ACTIVITY_FRAME
            };
            window
                .spawn(cx, async move |cx| {
                    // The platform's timer, which a test clock can advance.
                    cx.background_executor().timer(frame).await;
                    let _ = cx.update(|window, cx| {
                        scheduled.update(cx, |scheduled, _| *scheduled = false);
                        window.refresh();
                    });
                })
                .detach();
        }
        div()
            .debug_selector(|| "loading-spinner".into())
            .flex_shrink_0()
            .child(status_glyph(
                StatusGlyph::Running,
                Some(spin_angle(Instant::now())),
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_spinner_turns_once_per_spin_and_wraps() {
        let start = Instant::now();
        let base = spin_angle(start);
        let quarter = FxAnimation::ACTIVITY_SPIN / 4;
        let later = spin_angle(start + quarter);
        let turned = (later - base).rem_euclid(360.0);
        assert!(
            (turned - 90.0).abs() < 1.0,
            "a quarter spin turned {turned} degrees"
        );
        for offset_ms in [0, 7, 400, 799, 800, 12_345] {
            let angle = spin_angle(start + Duration::from_millis(offset_ms));
            assert!((0.0..360.0).contains(&angle), "{angle}");
        }
    }
}
