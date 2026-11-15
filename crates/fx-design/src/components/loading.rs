//! A spinner for work whose end FlowX cannot see coming: a transcript being
//! read, a provider's threads arriving, a diff being taken.
//!
//! It turns on the activity clock, as every spinner does: the running status
//! glyph asks for its next beat as it is drawn, so it shares one repaint with
//! every other motion on screen, and a spinner no longer in the tree asks for
//! nothing, so motion ends with the wait. Under reduced motion it is a still
//! ring.

use std::time::Instant;

use gpui::prelude::*;
use gpui::{IntoElement, SharedString, div};

use super::status::{StatusGlyph, status_glyph};
use crate::theme::Beat;

/// A spinning status glyph. `id` names the element, so two waits on one
/// screen are two elements.
pub fn loading_spinner(id: impl Into<SharedString>) -> impl IntoElement {
    div()
        .id(id.into())
        .debug_selector(|| "loading-spinner".into())
        .flex_shrink_0()
        .child(status_glyph(StatusGlyph::Running, true))
}

/// The spinner's angle at `now`, in degrees: the activity clock's, so every
/// spinner on screen turns together from one origin.
pub fn spin_angle(now: Instant) -> f32 {
    Beat::at(now).spin_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::FxAnimation;
    use std::time::Duration;

    #[test]
    fn the_spinner_turns_once_a_second_and_wraps() {
        let start = Instant::now();
        let base = spin_angle(start);
        // A quarter turn is three of the twelve beats in a second.
        let later = spin_angle(start + FxAnimation::BEAT * 3);
        let turned = (later - base).rem_euclid(360.0);
        assert!(
            (turned - 90.0).abs() < 1e-3,
            "a quarter second turned {turned} degrees"
        );
        assert_eq!(spin_angle(start + FxAnimation::BEAT * 12), base);
        for offset_ms in [0, 7, 400, 799, 800, 12_345] {
            let angle = spin_angle(start + Duration::from_millis(offset_ms));
            assert!((0.0..360.0).contains(&angle), "{angle}");
        }
    }
}
