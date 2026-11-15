//! Motion tokens.
//!
//! Views never write a duration or an easing curve. A transition picks one
//! duration from the scale and one [`Easing`], so every movement in the app
//! feels like it came from the same hand:
//!
//! | Duration | | For |
//! |---|---|---|
//! | `MICRO` | 100 ms | a caret, a press, anything barely perceived |
//! | `QUICK` | 150 ms | hover and state feedback |
//! | `BASE` | 200 ms | the default for an element changing in place |
//! | `PANEL` | 250 ms | a panel or sheet opening or closing |
//! | `EMPHASIS` | 600 ms | the rare motion meant to be watched |
//!
//! Something arriving enters, something leaving exits, something already on
//! screen moves, and only a continuous rotation is linear. An entering
//! element that travels rises by [`FxAnimation::RISE`].
//!
//! GPUI drives animation through `gpui::Animation` plus a progress callback;
//! [`Easing::curve`] is what its `with_easing` takes. [`PanelTransition`]
//! preserves position when a panel's direction changes before it finishes.

use std::time::{Duration, Instant};

use super::metrics::Spacing;

pub struct FxAnimation;

/// How a motion accelerates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Easing {
    /// Ease-out cubic: fast, then settling. For what arrives.
    Enter,
    /// Ease-in cubic: gathering speed as it goes. For what leaves.
    Exit,
    /// Ease-in-out cubic. For what moves from one place on screen to another.
    Move,
    /// Constant speed. Only for continuous motion, such as a rotation.
    Linear,
}

impl Easing {
    pub const ALL: [Self; 4] = [Self::Enter, Self::Exit, Self::Move, Self::Linear];

    /// Progress along the curve at `t`, clamped to 0…1.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Enter => 1.0 - (1.0 - t).powi(3),
            Self::Exit => t.powi(3),
            Self::Move => {
                if t < 0.5 {
                    4.0 * t.powi(3)
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Self::Linear => t,
        }
    }

    /// The curve as a function, for `gpui::Animation::with_easing`.
    pub fn curve(self) -> impl Fn(f32) -> f32 + 'static {
        move |t| self.apply(t)
    }
}

/// Interruptible panel reveal. A reversal starts at the current position,
/// rather than restarting at either endpoint. Initial state never animates.
#[derive(Clone, Debug)]
pub struct PanelTransition {
    from: f32,
    target: f32,
    started: Instant,
}

impl PanelTransition {
    pub fn new(expanded: bool) -> Self {
        let value = if expanded { 1.0 } else { 0.0 };
        Self {
            from: value,
            target: value,
            started: Instant::now(),
        }
    }

    pub fn set_expanded(&mut self, expanded: bool, now: Instant) {
        let target = if expanded { 1.0 } else { 0.0 };
        if self.target != target {
            self.from = self.progress(now);
            self.target = target;
            self.started = now;
        }
    }

    pub fn progress(&self, now: Instant) -> f32 {
        let fraction = (now.saturating_duration_since(self.started).as_secs_f32()
            / FxAnimation::PANEL.as_secs_f32())
        .min(1.0);
        if fraction == 1.0 {
            return self.target;
        }
        self.from + (self.target - self.from) * Easing::Enter.apply(fraction)
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.from != self.target && now.saturating_duration_since(self.started) < FxAnimation::PANEL
    }
}

impl FxAnimation {
    // ── The duration scale ─────────────────────────────────────────────────

    /// Caret blinks, presses and other barely perceived changes.
    pub const MICRO: Duration = Duration::from_millis(100);
    /// Hover and state feedback.
    pub const QUICK: Duration = Duration::from_millis(150);
    /// The default for an element changing in place.
    pub const BASE: Duration = Duration::from_millis(200);
    /// Panels and sheets opening and closing.
    pub const PANEL: Duration = Duration::from_millis(250);
    /// The rare motion meant to be watched.
    pub const EMPHASIS: Duration = Duration::from_millis(600);

    /// The scale, shortest first.
    pub const SCALE: [Duration; 5] = [
        Self::MICRO,
        Self::QUICK,
        Self::BASE,
        Self::PANEL,
        Self::EMPHASIS,
    ];

    /// How far an entering element travels as it arrives: a rise, never a
    /// slide across the screen.
    pub const RISE: f32 = Spacing::XXS;

    // ── Continuous status motion ───────────────────────────────────────────

    /// One full breath of the pulsing status ring.
    pub const STATUS_PULSE: Duration = Duration::from_millis(1600);
    /// One full rotation of the activity spinner.
    pub const ACTIVITY_SPIN: Duration = Duration::from_millis(800);
    /// How often continuous status motion repaints, about 30 Hz.
    ///
    /// A spinner or pulse that repaints on every display frame redraws the whole
    /// window at 60–120 Hz for as long as a turn runs, since the sidebar is drawn
    /// by the root view. Neither needs more than this to read as motion.
    pub const ACTIVITY_FRAME: Duration = Duration::from_millis(33);
    /// How often motion that may last for hours repaints: the waiting pulse,
    /// which can sit on a thread waiting in another app all day, and any
    /// motion in a window that is not the key window.
    ///
    /// The pulse is a slow tint over 1.6 s, so ten steps a second still read
    /// as a breath, and a third of the whole-window redraws.
    pub const QUIET_ACTIVITY_FRAME: Duration = Duration::from_millis(100);
    /// How often a running clock repaints. Elapsed times are shown in whole
    /// seconds, so anything faster is a repaint that changes nothing.
    pub const CLOCK_TICK: Duration = Duration::from_secs(1);

    /// Opacity for the `index`-th dot of the typing indicator.
    pub fn typing_dot_opacity(index: usize, phase: usize, reduce_motion: bool) -> f32 {
        if reduce_motion {
            return 0.65;
        }
        if index == phase { 1.0 } else { 0.3 }
    }

    /// Scale for the `index`-th dot of the typing indicator.
    pub fn typing_dot_scale(index: usize, phase: usize, reduce_motion: bool) -> f32 {
        if reduce_motion {
            return 1.0;
        }
        if index == phase { 1.2 } else { 1.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_reversal_starts_at_its_current_position_and_settles() {
        let now = Instant::now();
        let mut panel = PanelTransition::new(true);
        assert_eq!(panel.progress(now), 1.0);
        assert!(!panel.is_animating(now));
        panel.set_expanded(false, now);
        let halfway = now + FxAnimation::PANEL / 2;
        let position = panel.progress(halfway);
        assert!(position > 0.0 && position < 1.0);
        panel.set_expanded(true, halfway);
        assert_eq!(panel.progress(halfway), position);
        assert!(panel.progress(halfway + FxAnimation::PANEL / 2) > position);
        assert_eq!(panel.progress(halfway + FxAnimation::PANEL), 1.0);
        assert!(!panel.is_animating(halfway + FxAnimation::PANEL));
        panel.set_expanded(false, halfway + FxAnimation::PANEL);
        assert_eq!(panel.progress(halfway + FxAnimation::PANEL * 2), 0.0);
    }

    #[test]
    fn the_duration_scale_holds_its_documented_values_in_order() {
        let millis: Vec<u128> = FxAnimation::SCALE.iter().map(|d| d.as_millis()).collect();
        assert_eq!(millis, [100, 150, 200, 250, 600]);
    }

    #[test]
    fn an_entering_element_rises_by_a_step_of_the_spacing_scale() {
        assert_eq!(FxAnimation::RISE, Spacing::XXS);
        assert!(Spacing::ALL.contains(&FxAnimation::RISE));
    }

    #[test]
    fn every_easing_starts_at_zero_ends_at_one_and_never_turns_back() {
        for easing in Easing::ALL {
            assert_eq!(easing.apply(0.0), 0.0, "{easing:?}");
            assert!((easing.apply(1.0) - 1.0).abs() < 1e-6, "{easing:?}");
            let mut last = 0.0;
            for i in 0..=200 {
                let v = easing.curve()(i as f32 / 200.0);
                assert!(v.is_finite() && v >= last, "{easing:?} turned back at {i}");
                last = v;
            }
            // Outside its domain a curve holds its ends.
            assert_eq!(easing.apply(-1.0), 0.0);
            assert!((easing.apply(2.0) - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn the_curves_are_the_shapes_their_names_promise() {
        // Enter is ahead of linear, exit behind it, and each mirrors the
        // other; move is symmetric about the midpoint and crosses it there.
        for i in 1..100 {
            let t = i as f32 / 100.0;
            let enter = Easing::Enter.apply(t);
            let exit = Easing::Exit.apply(t);
            assert!(enter > t && exit < t, "at {t}");
            assert!((enter - (1.0 - Easing::Exit.apply(1.0 - t))).abs() < 1e-5);
            let mv = Easing::Move.apply(t);
            assert!((mv - (1.0 - Easing::Move.apply(1.0 - t))).abs() < 1e-5);
            assert_eq!(Easing::Linear.apply(t), t);
        }
        assert!((Easing::Move.apply(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_four_curves_are_four_different_curves() {
        // `spring(0.0)` was `ease_out` under another name.
        for (i, a) in Easing::ALL.iter().enumerate() {
            for b in &Easing::ALL[i + 1..] {
                assert_ne!(a.apply(0.25), b.apply(0.25), "{a:?} and {b:?}");
            }
        }
    }

    #[test]
    fn typing_indicator_highlights_exactly_one_dot() {
        for phase in 0..3 {
            let lit = (0..3)
                .filter(|i| FxAnimation::typing_dot_opacity(*i, phase, false) > 0.5)
                .count();
            assert_eq!(lit, 1, "phase {phase}");
        }
    }
}
