//! Motion, as elements: the activity clock's repaints and an entrance.
//!
//! The phases and curves are pure and live in
//! [`theme::animation`](crate::theme::animation). This module is the half
//! that needs a window: asking for the next repaint, and only while something
//! that moves is actually drawn.
//!
//! - **The activity clock.** An element that draws continuous motion — a
//!   spinner, the waiting pulse, the typing dots, the caret — calls
//!   [`request_activity_frame`] as it renders. A window holds at most one
//!   pending wake, on the next beat its fastest motion changes, so every
//!   spinner on screen shares one repaint, and the chain stops by itself on
//!   the first frame that draws nothing moving: nothing asks again.
//! - **An entrance.** [`entering`] fades a surface in and lifts it the last
//!   few points to where it settles, frame by frame on the display's clock,
//!   for as long as the entrance runs and not a frame longer.
//!
//! Both read the executor's clock, which is the system's in the app and the
//! one a test advances, so a test drives motion the way it drives timers.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Instant;

use gpui::prelude::*;
use gpui::{AnyElement, App, ElementId, IntoElement, Rgba, Window, WindowId, div, px};

use crate::theme::animation::until_beat;
use crate::theme::{Beat, Entrance, Motion, global};

/// The clock motion reads: the executor's, so a test can move it.
pub fn now(cx: &App) -> Instant {
    cx.background_executor().now()
}

/// A window's one pending wake.
#[derive(Clone, Copy)]
struct Wake {
    due: Instant,
    generation: u64,
}

/// Every window's pending wake. A global of the app rather than a
/// thread-local, so a wake belongs to the executor whose clock it was timed
/// on: two apps on one thread, as two tests in a row are, cannot hold each
/// other's.
#[derive(Default)]
struct Wakes {
    pending: HashMap<WindowId, Wake>,
    generation: u64,
}

impl gpui::Global for Wakes {}

thread_local! {
    static WAKES_SCHEDULED: Cell<u64> = const { Cell::new(0) };
    static ENTRANCE_FRAMES: Cell<u64> = const { Cell::new(0) };
}

/// Ask for one repaint of this window on the next beat `motion` changes.
///
/// Called by an element as it draws continuous motion, never by a view on
/// its own initiative: the request is what keeps the motion going, so a
/// motion scrolled out of view, settled or replaced stops costing frames on
/// the next pass. A window that is not the key window repaints at
/// [`Motion::quiet_beats`].
///
/// One pending wake per window. A request that the pending wake already
/// covers schedules nothing; an earlier one replaces it, and the replaced
/// timer lands without repainting. A wake already due is stale, whether its
/// timer is late or will never fire, and is replaced rather than trusted.
pub fn request_activity_frame(motion: Motion, window: &Window, cx: &mut App) {
    let now = now(cx);
    let beats = if window.is_window_active() {
        motion.beats()
    } else {
        motion.quiet_beats()
    };
    let delay = until_beat(beats, now);
    let due = now + delay;
    let id = window.window_handle().window_id();
    let wakes = cx.default_global::<Wakes>();
    if wakes
        .pending
        .get(&id)
        .is_some_and(|wake| wake.due > now && wake.due <= due)
    {
        return;
    }
    wakes.generation += 1;
    let generation = wakes.generation;
    wakes.pending.insert(id, Wake { due, generation });
    WAKES_SCHEDULED.with(|count| count.set(count.get() + 1));
    window
        .spawn(cx, async move |cx| {
            // The platform's timer, which a test clock can advance.
            cx.background_executor().timer(delay).await;
            let _ = cx.update(|window, cx| {
                let wakes = cx.default_global::<Wakes>();
                let current = wakes
                    .pending
                    .get(&id)
                    .is_some_and(|wake| wake.generation == generation);
                if current {
                    wakes.pending.remove(&id);
                    window.refresh();
                }
            });
        })
        .detach();
}

/// How many activity wakes this thread has scheduled. A diagnostic for
/// tests that assert motion stops: a count that holds still is a clock that
/// stopped.
pub fn activity_wakes_scheduled() -> u64 {
    WAKES_SCHEDULED.with(Cell::get)
}

/// How many display frames entrances on this thread have asked for. A
/// diagnostic, as [`activity_wakes_scheduled`] is.
pub fn entrance_frames_requested() -> u64 {
    ENTRANCE_FRAMES.with(Cell::get)
}

/// The beat a motion drawn now is at, having asked for the frame that
/// changes it. `None` under reduced motion, where it holds still and asks
/// for nothing.
pub fn activity_beat(motion: Motion, window: &Window, cx: &mut App) -> Option<Beat> {
    if global::current().reduce_motion {
        return None;
    }
    request_activity_frame(motion, window, cx);
    Some(Beat::at(now(cx)))
}

// ── The spinner ────────────────────────────────────────────────────────────

/// The activity spinner, turning on the activity clock: twelve positions a
/// second, and a still ring under reduced motion.
pub fn spinner(color: Rgba) -> Spinner {
    Spinner { color }
}

#[derive(IntoElement)]
pub struct Spinner {
    color: Rgba,
}

impl RenderOnce for Spinner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        match activity_beat(Motion::Spin, window, cx) {
            Some(beat) => super::spinner_arc(self.color, beat.spin_degrees()).into_any_element(),
            None => super::static_ring(self.color).into_any_element(),
        }
    }
}

// ── Entrances ──────────────────────────────────────────────────────────────

/// Which way an entering surface travels to where it settles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approach {
    /// Rises into place: a menu below its trigger, a card above the
    /// composer, the palette.
    FromBelow,
    /// Settles down into place: a popup anchored above what opened it, such
    /// as the `@` picker over the composer.
    FromAbove,
}

/// When an entrance began.
enum Start {
    /// When the owner says, for a surface whose owner keeps an `Instant`.
    At(Instant),
    /// The first frame an element with this id was drawn in. Kept in element
    /// state, which lives exactly as long as the element is drawn in
    /// consecutive frames, so a surface that closes and opens again enters
    /// again.
    FirstDrawn(ElementId),
}

/// A surface that fades in over `entrance`, travelling the last
/// [`RISE`](crate::theme::FxAnimation::RISE) to where it settles, measured
/// from the first frame an element with `id` was drawn in.
pub fn entering(
    id: impl Into<ElementId>,
    entrance: Entrance,
    approach: Approach,
    content: impl IntoElement,
) -> Entering {
    Entering {
        start: Start::FirstDrawn(id.into()),
        entrance,
        approach,
        content: content.into_any_element(),
    }
}

/// [`entering`], measured from an `Instant` the surface's owner keeps, on
/// the executor's clock ([`now`]).
pub fn entering_since(
    started: Instant,
    entrance: Entrance,
    approach: Approach,
    content: impl IntoElement,
) -> Entering {
    Entering {
        start: Start::At(started),
        entrance,
        approach,
        content: content.into_any_element(),
    }
}

#[derive(IntoElement)]
pub struct Entering {
    start: Start,
    entrance: Entrance,
    approach: Approach,
    content: AnyElement,
}

impl RenderOnce for Entering {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let now = now(cx);
        let started = match self.start {
            Start::At(started) => started,
            Start::FirstDrawn(id) => *window.use_keyed_state(id, cx, |_, _| now).read(cx),
        };
        let frame = self.entrance.at(
            now.saturating_duration_since(started),
            global::current().reduce_motion,
        );
        if !frame.settled {
            // On the display's clock, for the few frames the entrance runs.
            // A settled surface asks for nothing.
            ENTRANCE_FRAMES.with(|count| count.set(count.get() + 1));
            window.on_next_frame(|window, _| window.refresh());
        }
        let rise = match self.approach {
            Approach::FromBelow => frame.rise,
            Approach::FromAbove => -frame.rise,
        };
        // Offset by `top` on a relatively positioned box, which moves what is
        // painted and nothing around it, so the travel lays nothing else out.
        div()
            .relative()
            .max_w_full()
            .top(px(rise))
            .opacity(frame.opacity)
            .child(self.content)
    }
}
