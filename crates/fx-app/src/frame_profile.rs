//! Frame timing, on request.
//!
//! Smoothness has to be measured before it can be improved, and GPUI exposes
//! no frame clock to the app. So the root wraps its element tree in an
//! element that times the four stages a frame spends on the UI thread:
//!
//! - **build**, the root's render function producing the element tree;
//! - **layout**, the tree's layout request and Taffy's pass over it;
//! - **prepaint**, where every element measures text and records hit regions;
//! - **paint**, where the scene is written.
//!
//! The GPU's share is not visible from here. What is visible is everything the
//! app itself can make slow, which is where a dropped frame starts.
//!
//! Set `FX_FRAME_PROFILE=<ms>` to log every frame that spends at least that
//! long on the UI thread, and a summary of all frames every few seconds. `0`
//! logs every frame. Unset, nothing here runs: the root is returned as it is.
//!
//! ```sh
//! FX_FRAME_PROFILE=4 ./fx run 2>&1 | grep fx_frame
//! ```
//!
//! A 120 Hz display gives a frame 8.3 ms, and the GPU needs some of that, so
//! 4 ms is a useful threshold for a frame that is about to be late.

use std::{
    cell::RefCell,
    sync::OnceLock,
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

/// The environment variable that turns profiling on.
const VARIABLE: &str = "FX_FRAME_PROFILE";

/// How often the running summary is logged.
const SUMMARY_EVERY: Duration = Duration::from_secs(5);

/// The threshold above which a frame is logged, or `None` when profiling is
/// off. Read once: the variable is a launch-time choice.
fn threshold() -> Option<Duration> {
    static THRESHOLD: OnceLock<Option<Duration>> = OnceLock::new();
    *THRESHOLD.get_or_init(|| {
        std::env::var(VARIABLE)
            .ok()
            .map(|value| parse_threshold(&value))
    })
}

/// The threshold a variable's value asks for, in milliseconds.
///
/// Anything that is not a number logs every frame: the variable being set at
/// all is the request, and a typo should not silently turn it off.
fn parse_threshold(value: &str) -> Duration {
    let millis: f64 = value.trim().parse().unwrap_or(0.0);
    Duration::from_secs_f64(millis.max(0.0) / 1000.0)
}

/// Whether frames are being timed.
pub fn enabled() -> bool {
    threshold().is_some()
}

/// When the root began building its element tree, if frames are being timed.
pub fn begin_build() -> Option<Instant> {
    enabled().then(Instant::now)
}

/// The root's tree, timed from `build_started` through its paint.
///
/// With profiling off, the tree itself: the wrapper is never in the way of a
/// frame that is not being measured.
pub fn profiled(root: impl IntoElement, build_started: Option<Instant>) -> AnyElement {
    match build_started {
        Some(started) => Profiled {
            build: started.elapsed(),
            child: Some(root.into_any_element()),
        }
        .into_any_element(),
        None => root.into_any_element(),
    }
}

/// One frame's time on the UI thread, by stage.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Frame {
    build: Duration,
    layout: Duration,
    prepaint: Duration,
    paint: Duration,
}

impl Frame {
    fn total(&self) -> Duration {
        self.build + self.layout + self.prepaint + self.paint
    }
}

/// The frames since the last summary.
struct Stats {
    since: Instant,
    frames: u32,
    slow: u32,
    total: Duration,
    worst: Frame,
    last_frame_at: Option<Instant>,
}

impl Stats {
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            frames: 0,
            slow: 0,
            total: Duration::ZERO,
            worst: Frame::default(),
            last_frame_at: None,
        }
    }
}

thread_local! {
    // Frames are painted on the main thread, so no lock is needed.
    static STATS: RefCell<Option<Stats>> = const { RefCell::new(None) };
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Account for a painted frame: log it when it was slow, and the summary when
/// one is due.
fn record(frame: Frame) {
    let Some(threshold) = threshold() else {
        return;
    };
    let now = Instant::now();
    STATS.with(|stats| {
        let mut stats = stats.borrow_mut();
        let stats = stats.get_or_insert_with(|| Stats::new(now));
        let total = frame.total();
        let since_last = stats.last_frame_at.map(|at| now.saturating_duration_since(at));
        stats.last_frame_at = Some(now);
        stats.frames += 1;
        stats.total += total;
        if total > stats.worst.total() {
            stats.worst = frame;
        }
        if total >= threshold {
            stats.slow += 1;
            tracing::info!(
                target: "fx_frame",
                total_ms = format_args!("{:.2}", millis(total)),
                build_ms = format_args!("{:.2}", millis(frame.build)),
                layout_ms = format_args!("{:.2}", millis(frame.layout)),
                prepaint_ms = format_args!("{:.2}", millis(frame.prepaint)),
                paint_ms = format_args!("{:.2}", millis(frame.paint)),
                since_last_ms = since_last.map(|d| format!("{:.1}", millis(d))).as_deref().unwrap_or("-"),
                "slow frame"
            );
        }
        if now.saturating_duration_since(stats.since) >= SUMMARY_EVERY {
            let mean = stats.total.checked_div(stats.frames).unwrap_or_default();
            tracing::info!(
                target: "fx_frame",
                frames = stats.frames,
                slow = stats.slow,
                mean_ms = format_args!("{:.2}", millis(mean)),
                worst_ms = format_args!("{:.2}", millis(stats.worst.total())),
                worst_build_ms = format_args!("{:.2}", millis(stats.worst.build)),
                worst_layout_ms = format_args!("{:.2}", millis(stats.worst.layout)),
                worst_prepaint_ms = format_args!("{:.2}", millis(stats.worst.prepaint)),
                worst_paint_ms = format_args!("{:.2}", millis(stats.worst.paint)),
                "frames"
            );
            let last_frame_at = stats.last_frame_at;
            *stats = Stats::new(now);
            stats.last_frame_at = last_frame_at;
        }
    });
}

/// The root's tree with a stopwatch around each stage.
struct Profiled {
    build: Duration,
    child: Option<AnyElement>,
}

/// When the layout request finished, so the prepaint can measure the layout
/// pass GPUI runs between the two.
struct LayoutRequested(Instant);

/// The stages measured by the time paint begins.
struct BeforePaint {
    layout: Duration,
    prepaint: Duration,
}

impl IntoElement for Profiled {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Profiled {
    type RequestLayoutState = LayoutRequested;
    type PrepaintState = BeforePaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, LayoutRequested) {
        let child = self.child.as_mut().expect("the root is laid out once");
        let id = child.request_layout(window, cx);
        (id, LayoutRequested(Instant::now()))
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        requested: &mut LayoutRequested,
        window: &mut Window,
        cx: &mut App,
    ) -> BeforePaint {
        let started = Instant::now();
        // Between the request returning and this call, GPUI computed the
        // layout. Nothing else of the app's runs in between.
        let layout = started.saturating_duration_since(requested.0);
        self.child
            .as_mut()
            .expect("the root is prepainted once")
            .prepaint(window, cx);
        BeforePaint {
            layout,
            prepaint: started.elapsed(),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut LayoutRequested,
        before: &mut BeforePaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        self.child
            .as_mut()
            .expect("the root is painted once")
            .paint(window, cx);
        record(Frame {
            build: self.build,
            layout: before.layout,
            prepaint: before.prepaint,
            paint: started.elapsed(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, div, prelude::*};

    #[test]
    fn the_threshold_is_milliseconds_and_a_typo_logs_everything() {
        assert_eq!(parse_threshold("4"), Duration::from_millis(4));
        assert_eq!(parse_threshold(" 2.5 "), Duration::from_micros(2_500));
        assert_eq!(parse_threshold("0"), Duration::ZERO);
        assert_eq!(parse_threshold("yes"), Duration::ZERO);
        assert_eq!(parse_threshold("-3"), Duration::ZERO);
    }

    #[test]
    fn a_frame_is_the_sum_of_its_stages() {
        let frame = Frame {
            build: Duration::from_millis(1),
            layout: Duration::from_millis(2),
            prepaint: Duration::from_millis(3),
            paint: Duration::from_millis(4),
        };
        assert_eq!(frame.total(), Duration::from_millis(10));
    }

    #[test]
    fn a_profiled_root_still_paints_its_tree() {
        // The wrapper passes layout, prepaint and paint through to its child;
        // a stage it forgot would leave the window blank.
        struct Probe;
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let started = Some(Instant::now());
                profiled(
                    div()
                        .size_full()
                        .child(div().debug_selector(|| "profiled-child".into()).size_full()),
                    started,
                )
            }
        }
        let mut test = gpui::TestAppContext::single();
        let window = test.add_window(|_, _| Probe);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
        test.refresh().unwrap();
        let bounds = visual
            .debug_bounds("profiled-child")
            .expect("the child painted");
        assert!(bounds.size.width > Pixels::ZERO && bounds.size.height > Pixels::ZERO);
    }

    #[test]
    fn an_unprofiled_root_is_returned_as_it_is() {
        // With profiling off, `begin_build` hands back nothing and the wrapper
        // is never built, so a frame that is not measured pays nothing.
        let mut test = gpui::TestAppContext::single();
        struct Probe;
        impl Render for Probe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                profiled(
                    div().debug_selector(|| "plain-root".into()).size_full(),
                    None,
                )
            }
        }
        let window = test.add_window(|_, _| Probe);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
        test.refresh().unwrap();
        assert!(visual.debug_bounds("plain-root").is_some());
    }
}
