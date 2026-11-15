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
//! Three kinds of motion, three mechanisms, none of them `gpui::Animation`:
//! `with_animation` asks for a frame on every display refresh for as long as
//! it runs, which rebuilds the whole window at 120 Hz, and keeps its clock in
//! element state that is dropped for any element not drawn that frame.
//!
//! - **Continuous motion** — the spinner, the waiting pulse, the typing dots,
//!   the caret — is a phase of one clock, [`Beat`]: twelve beats a second,
//!   counted from one process-wide origin. Each motion changes on a multiple
//!   of the beat ([`Motion::beats`]), so everything that moves wakes together,
//!   a running turn costs at most twelve rebuilds a second, and a screen with
//!   nothing moving schedules nothing at all.
//! - **An entrance** — a menu, the palette, an approval card — is a short
//!   fade and rise measured from an `Instant` ([`Entrance`]), drawn frame by
//!   frame until it settles and never again.
//! - **A panel** is a [`PanelTransition`], which preserves position when its
//!   direction changes before it finishes.
//!
//! Reduced motion, read from the platform by the app and kept on the theme,
//! turns every travel into none: an entrance only fades, a panel jumps, and
//! the spinner and pulse hold still.

use std::sync::OnceLock;
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

    /// End any motion at the target: what a panel does under reduced motion,
    /// where a slide becomes an instant change of width.
    pub fn settle(&mut self) {
        self.from = self.target;
    }

    /// The least and the most of the panel the current transition shows, from
    /// where it started to where it is going. At rest both are the target.
    ///
    /// What lets a neighbour lay itself out once for the whole slide: at the
    /// widest it will be, rather than at every width in between.
    pub fn range(&self) -> (f32, f32) {
        (self.from.min(self.target), self.from.max(self.target))
    }
}

// ── The activity clock ─────────────────────────────────────────────────────

/// A count of beats on the activity clock, which every continuous motion
/// reads its phase from.
///
/// Counted from one process-wide origin rather than from when each element
/// first drew, so two spinners started a second apart turn together and
/// their repaints land in one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Beat(pub i64);

/// A continuous motion, and how often it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// The activity spinner: one of twelve positions a beat, a turn a second.
    Spin,
    /// The waiting pulse: a slow breath that can last all day, so it steps
    /// every other beat.
    Pulse,
    /// The typing indicator: the lit dot moves every third of a second.
    Typing,
    /// The streaming caret: solid half a second, faded half a second.
    Caret,
}

impl Motion {
    pub const ALL: [Self; 4] = [Self::Spin, Self::Pulse, Self::Typing, Self::Caret];

    /// Beats between two changes of this motion.
    pub const fn beats(self) -> u32 {
        match self {
            Self::Spin => 1,
            Self::Pulse => 2,
            Self::Typing => 4,
            Self::Caret => 6,
        }
    }

    /// Beats between two repaints of this motion in a window that is not the
    /// key window, which nobody is watching closely.
    pub const fn quiet_beats(self) -> u32 {
        let beats = self.beats();
        if beats < FxAnimation::QUIET_BEATS {
            FxAnimation::QUIET_BEATS
        } else {
            beats
        }
    }
}

/// The process-wide instant beat zero began at.
fn origin(now: Instant) -> Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *ORIGIN.get_or_init(|| now)
}

/// Nanoseconds from `origin` to `now`, negative for an instant before it. A
/// test's executor clock can start before the origin a previous test set.
fn signed_nanos(origin: Instant, now: Instant) -> i128 {
    match now.checked_duration_since(origin) {
        Some(after) => after.as_nanos() as i128,
        None => -(origin.duration_since(now).as_nanos() as i128),
    }
}

impl Beat {
    /// Positions of the spinner, one per beat.
    pub const SPIN_STEPS: i64 = 12;
    /// Dots of the typing indicator.
    pub const TYPING_DOTS: i64 = 3;

    /// The beat `now` falls in.
    pub fn at(now: Instant) -> Self {
        Self::since(origin(now), now)
    }

    /// The beat `now` falls in, counted from `origin`.
    pub fn since(origin: Instant, now: Instant) -> Self {
        let beat = FxAnimation::BEAT.as_nanos() as i128;
        Self(signed_nanos(origin, now).div_euclid(beat) as i64)
    }

    /// How far the spinner has turned, in degrees: twelve steps of 30°, so a
    /// turn a second and the arc lands where it started.
    pub fn spin_degrees(self) -> f32 {
        self.0.rem_euclid(Self::SPIN_STEPS) as f32 * (360.0 / Self::SPIN_STEPS as f32)
    }

    /// Which dot of the typing indicator is lit.
    pub fn typing_dot(self) -> usize {
        (self.0.div_euclid(Motion::Typing.beats() as i64)).rem_euclid(Self::TYPING_DOTS) as usize
    }

    /// Whether the caret is drawn solid rather than faded.
    pub fn caret_solid(self) -> bool {
        self.0.div_euclid(Motion::Caret.beats() as i64) % 2 == 0
    }

    /// Where the waiting pulse is in its breath, 0 to 1 and back, stepped on
    /// the pulse's own beats. Beat zero is the peak.
    pub fn pulse(self) -> f32 {
        let step = Motion::Pulse.beats() as i64;
        let period = FxAnimation::STATUS_PULSE_BEATS as i64;
        let at = (self.0.div_euclid(step) * step).rem_euclid(period) as f32 / period as f32;
        0.5 + 0.5 * (at * std::f32::consts::TAU).cos()
    }
}

/// How long from `now` until the next beat on which a motion changing every
/// `beats` beats changes, counted from `origin`. Never zero: a beat that has
/// just begun is in the past.
pub fn until_beat_since(origin: Instant, beats: u32, now: Instant) -> Duration {
    let period = FxAnimation::BEAT.as_nanos() as i128 * beats.max(1) as i128;
    let into = signed_nanos(origin, now).rem_euclid(period);
    Duration::from_nanos((period - into) as u64)
}

/// How long from `now` until the next beat on which a motion changing every
/// `beats` beats changes.
pub fn until_beat(beats: u32, now: Instant) -> Duration {
    until_beat_since(origin(now), beats, now)
}

/// `delay` from `now`, pushed on to the beat that follows it, so a timer
/// that is not itself motion — a clock that ticks once a second — lands on
/// the same frame as the motion around it rather than a frame of its own.
pub fn on_the_beat(delay: Duration, now: Instant) -> Duration {
    on_the_beat_since(origin(now), delay, now)
}

/// [`on_the_beat`], counted from `origin`.
pub fn on_the_beat_since(origin: Instant, delay: Duration, now: Instant) -> Duration {
    let period = FxAnimation::BEAT.as_nanos() as i128;
    let into = signed_nanos(origin, now + delay).rem_euclid(period);
    if into == 0 {
        delay
    } else {
        delay + Duration::from_nanos((period - into) as u64)
    }
}

// ── Entrances ──────────────────────────────────────────────────────────────

/// How a surface arrives: a fade over a duration of the scale, rising
/// [`FxAnimation::RISE`] toward where it settles. Leaving is instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entrance {
    pub duration: Duration,
}

/// One frame of an entrance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntranceFrame {
    pub opacity: f32,
    /// How far the surface still is from where it settles, in points, away
    /// from its anchor. Zero once settled, and always under reduced motion.
    pub rise: f32,
    /// Whether the entrance is over, and nothing more needs drawing.
    pub settled: bool,
}

impl EntranceFrame {
    pub const SETTLED: Self = Self {
        opacity: 1.0,
        rise: 0.0,
        settled: true,
    };
}

impl Entrance {
    /// A menu, a context menu, the `@` picker or the palette: quick, because
    /// the reader asked for it and is already looking where it lands.
    pub const POPOVER: Self = Self {
        duration: FxAnimation::QUICK,
    };
    /// An approval or a question: the turn stopped for it, and it arrives
    /// unasked, so it takes the default duration.
    pub const CARD: Self = Self {
        duration: FxAnimation::BASE,
    };

    /// The entrance `elapsed` after it began.
    ///
    /// Reduced motion keeps the fade, which is not travel, and drops the rise.
    pub fn at(self, elapsed: Duration, reduce_motion: bool) -> EntranceFrame {
        let duration = self.duration.as_secs_f32();
        let t = if duration > 0.0 {
            (elapsed.as_secs_f32() / duration).min(1.0)
        } else {
            1.0
        };
        if t >= 1.0 {
            return EntranceFrame::SETTLED;
        }
        let eased = Easing::Enter.apply(t);
        EntranceFrame {
            opacity: eased,
            rise: if reduce_motion {
                0.0
            } else {
                FxAnimation::RISE * (1.0 - eased)
            },
            settled: false,
        }
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

    /// One beat of the activity clock: a twelfth of a second.
    ///
    /// Every continuous motion repaints on a multiple of it, and the whole
    /// window rebuilds on a repaint, since the sidebar and the transcript are
    /// drawn by the root view. Twelve a second is the most anything that
    /// moves costs: a spinner of twelve positions needs no more to turn.
    pub const BEAT: Duration = Duration::from_nanos(1_000_000_000 / 12);
    /// Beats between repaints of any motion in a window that is not the key
    /// window: six a second.
    pub const QUIET_BEATS: u32 = 2;
    /// One full breath of the waiting pulse, in beats: two seconds, stepped
    /// every other beat. The pulse can sit on a thread waiting in another app
    /// all day, so it is the slowest motion and repaints at half the beat.
    pub const STATUS_PULSE_BEATS: u32 = 24;
    /// One full breath of the waiting pulse.
    pub const STATUS_PULSE: Duration =
        Duration::from_nanos(1_000_000_000 / 12 * Self::STATUS_PULSE_BEATS as u64);
    /// How often a running clock repaints. Elapsed times are shown in whole
    /// seconds, so anything faster is a repaint that changes nothing.
    pub const CLOCK_TICK: Duration = Duration::from_secs(1);

    /// The typing indicator's lit dot, as a multiple of a resting dot's size.
    /// Each dot sits in a cell this much larger than a resting one, so the
    /// lit dot growing moves nothing beside it.
    pub const TYPING_DOT_EMPHASIS: f32 = 1.2;

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
        if index == phase {
            Self::TYPING_DOT_EMPHASIS
        } else {
            1.0
        }
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
    fn a_settled_panel_jumps_and_its_range_spans_the_slide() {
        let now = Instant::now();
        let mut panel = PanelTransition::new(true);
        assert_eq!(panel.range(), (1.0, 1.0));
        panel.set_expanded(false, now);
        assert_eq!(panel.range(), (0.0, 1.0), "a slide spans both ends");
        assert!(panel.is_animating(now));
        // Reduced motion: the width changes at once.
        panel.settle();
        assert!(!panel.is_animating(now));
        assert_eq!(panel.progress(now), 0.0);
        assert_eq!(panel.range(), (0.0, 0.0));
    }

    /// The instant `beats` beats and `nanos` past an origin.
    fn at(origin: Instant, beats: i64, nanos: i64) -> Instant {
        let offset = beats as i128 * FxAnimation::BEAT.as_nanos() as i128 + nanos as i128;
        if offset >= 0 {
            origin + Duration::from_nanos(offset as u64)
        } else {
            origin - Duration::from_nanos((-offset) as u64)
        }
    }

    #[test]
    fn every_phase_is_read_from_an_instant_on_the_shared_beat() {
        // An origin far enough from the start of the clock to step back from.
        let origin = Instant::now() + Duration::from_secs(10);
        let beat = |beats, nanos| Beat::since(origin, at(origin, beats, nanos));
        assert_eq!(beat(0, 0), Beat(0));
        assert_eq!(beat(0, 83_000_000), Beat(0), "still the first beat");
        assert_eq!(beat(1, 0), Beat(1));
        assert_eq!(beat(13, 1), Beat(13));
        assert_eq!(beat(0, -1), Beat(-1), "before the origin counts down");

        // The spinner: one step of 30° a beat, a whole turn a second.
        assert_eq!(Beat(0).spin_degrees(), 0.0);
        assert_eq!(Beat(1).spin_degrees(), 30.0);
        assert_eq!(Beat(11).spin_degrees(), 330.0);
        assert_eq!(Beat(12).spin_degrees(), 0.0);
        assert_eq!(Beat(-1).spin_degrees(), 330.0);
        assert_eq!(FxAnimation::BEAT * 12, Duration::from_nanos(999_999_996));

        // The typing dots: each lit for four beats, round in a second.
        let dots: Vec<usize> = (0..13).map(|b| Beat(b).typing_dot()).collect();
        assert_eq!(dots, [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 0]);

        // The caret: solid half a second, faded half a second.
        let caret: Vec<bool> = (0..13).map(|b| Beat(b).caret_solid()).collect();
        assert_eq!(caret[..6], [true; 6]);
        assert_eq!(caret[6..12], [false; 6]);
        assert!(caret[12]);

        // The pulse: a two-second breath, peak to trough and back, stepped
        // every other beat.
        assert_eq!(Beat(0).pulse(), 1.0);
        assert_eq!(
            Beat(1).pulse(),
            Beat(0).pulse(),
            "the pulse moved off its beat"
        );
        assert!(Beat(12).pulse() < 1e-6, "halfway is the trough");
        assert_eq!(Beat(24).pulse(), 1.0);
        assert!(Beat(6).pulse() > Beat(8).pulse());
        assert_eq!(
            FxAnimation::STATUS_PULSE,
            FxAnimation::BEAT * FxAnimation::STATUS_PULSE_BEATS
        );
    }

    #[test]
    fn a_wake_lands_on_the_next_change_of_its_motion_and_never_now() {
        let origin = Instant::now();
        let beat = FxAnimation::BEAT;
        // Exactly on a beat, the next change is a whole period away.
        assert_eq!(until_beat_since(origin, 1, origin), beat);
        assert_eq!(
            until_beat_since(origin, 1, at(origin, 3, 1_000)),
            beat - Duration::from_nanos(1_000)
        );
        for motion in Motion::ALL {
            let beats = motion.beats();
            for offset in [0, 1, beats as i64 - 1] {
                let now = at(origin, offset, 5);
                let wake = now + until_beat_since(origin, beats, now);
                let landed = Beat::since(origin, wake);
                assert_eq!(landed.0 % beats as i64, 0, "{motion:?} woke off its beat");
                assert!(
                    landed > Beat::since(origin, now),
                    "{motion:?} woke in the past"
                );
            }
        }
        // A window nobody is looking at repaints its spinner at half the beat.
        assert_eq!(Motion::Spin.quiet_beats(), 2);
        assert_eq!(Motion::Caret.quiet_beats(), Motion::Caret.beats());
    }

    #[test]
    fn a_clock_tick_is_pushed_onto_the_beat_it_falls_in() {
        let origin = Instant::now();
        let now = at(origin, 0, 10_000_000);
        let second = Duration::from_secs(1);
        let tick = on_the_beat_since(origin, second, now);
        assert!(tick >= second && tick < second + FxAnimation::BEAT);
        assert_eq!(
            signed_nanos(origin, now + tick) % FxAnimation::BEAT.as_nanos() as i128,
            0,
            "a tick between beats is a frame of its own"
        );
        // Already on a beat, it is not moved.
        let on = at(origin, 4, 0);
        assert_eq!(
            on_the_beat_since(origin, FxAnimation::BEAT * 8, on),
            FxAnimation::BEAT * 8
        );
    }

    #[test]
    fn an_entrance_fades_in_rising_toward_its_place_and_then_settles() {
        let quick = Entrance::POPOVER;
        assert_eq!(quick.duration, FxAnimation::QUICK);
        assert_eq!(Entrance::CARD.duration, FxAnimation::BASE);

        let first = quick.at(Duration::ZERO, false);
        assert_eq!(first.opacity, 0.0, "the first frame is invisible");
        assert_eq!(first.rise, FxAnimation::RISE, "it starts a rise away");
        assert!(!first.settled);

        let halfway = quick.at(FxAnimation::QUICK / 2, false);
        // Ease-out: well past half its opacity at half its time.
        assert!((halfway.opacity - 0.875).abs() < 1e-3, "{halfway:?}");
        assert!((halfway.rise - FxAnimation::RISE * 0.125).abs() < 1e-3);
        assert!(!halfway.settled);

        let mut last = first;
        for ms in (0..=150).step_by(10) {
            let frame = quick.at(Duration::from_millis(ms), false);
            assert!(frame.opacity >= last.opacity && frame.rise <= last.rise);
            last = frame;
        }
        assert_eq!(quick.at(FxAnimation::QUICK, false), EntranceFrame::SETTLED);
        assert_eq!(
            quick.at(Duration::from_secs(60), false),
            EntranceFrame::SETTLED
        );

        // Reduced motion keeps the fade and drops the travel.
        let reduced = quick.at(FxAnimation::QUICK / 2, true);
        assert_eq!(reduced.opacity, halfway.opacity);
        assert_eq!(reduced.rise, 0.0);
        assert_eq!(quick.at(Duration::ZERO, true).rise, 0.0);
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
