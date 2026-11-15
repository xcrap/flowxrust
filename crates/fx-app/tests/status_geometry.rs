//! Native layout regressions for layered design-system controls.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use fx_design::components::{
    ActivityState, ButtonStyle, ContextUsage, MenuItem, MenuLayer, MenuModel, MenuSection,
    ProviderIdentity, TYPING_DOT_CELL, activity_dot, activity_wakes_scheduled, button_with_icon,
    context_meter, context_ring_icon, entrance_frames_requested, menu_layer_overlay, menu_surface,
    provider_status, typing_dots,
};
use fx_design::icon::Icon;
use fx_design::theme::{
    Colors, Entrance, FxAnimation, IconSize, Theme, ThemeGuard, Typography, global,
};
use gpui::prelude::*;
use gpui::{Context, Entity, Render, SharedString, Window, div, px};

struct StatusProbe;

impl Render for StatusProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = global::current();
        div()
            .flex()
            .gap(px(20.0))
            .child(context_ring_icon(
                Icon::ARROW_UP,
                Some(0.4),
                Colors::accent(&theme),
                36.0,
                true,
                false,
                |_, _, _| {},
            ))
            .child(activity_dot(Colors::accent(&theme), ActivityState::Idle))
    }
}

#[test]
fn send_ring_layers_keep_their_dimensions_and_share_one_center() {
    let mut test = gpui::TestAppContext::single();
    let window = test.add_window(|_, _| StatusProbe);
    test.refresh().unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    let outer = visual.debug_bounds("context-ring").unwrap();
    let track = visual.debug_bounds("context-ring-track").unwrap();
    let fill = visual.debug_bounds("context-ring-fill").unwrap();
    let glyph = visual.debug_bounds("context-ring-glyph").unwrap();
    assert_eq!(outer.size, gpui::size(px(36.0), px(36.0)));
    assert_eq!(track, outer);
    assert_eq!(fill.size, gpui::size(px(28.0), px(28.0)));
    let edge = px(Typography::icon(IconSize::Control, &global::current()).size);
    assert_eq!(glyph.size, gpui::size(edge, edge));
    assert_eq!(fill.center(), outer.center());
    assert_eq!(glyph.center(), outer.center());
}

#[test]
fn activity_ring_and_identity_dot_overlay_instead_of_becoming_neighbors() {
    let mut test = gpui::TestAppContext::single();
    let window = test.add_window(|_, _| StatusProbe);
    test.refresh().unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    let outer = visual.debug_bounds("activity-dot").unwrap();
    let ring = visual.debug_bounds("activity-ring").unwrap();
    let dot = visual.debug_bounds("activity-inner").unwrap();
    assert_eq!(outer.size, gpui::size(px(16.0), px(16.0)));
    assert_eq!(ring.size, gpui::size(px(13.0), px(13.0)));
    assert_eq!(dot.size, gpui::size(px(7.0), px(7.0)));
    assert_eq!(ring.center(), outer.center());
    assert_eq!(dot.center(), outer.center());
}

struct RecheckProbe;

impl Render for RecheckProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().flex().items_start().child(button_with_icon(
            "Re-check",
            Some(Icon::ARROW_CLOCKWISE),
            ButtonStyle::Secondary,
            true,
            |_, _, _| {},
        ))
    }
}

#[test]
fn labelled_controls_keep_icon_and_label_in_one_compact_row() {
    let mut test = gpui::TestAppContext::single();
    let window = test.add_window(|_, _| RecheckProbe);
    test.refresh().unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    let button = visual.debug_bounds("button-content").unwrap();
    let icon = visual.debug_bounds("button-icon").unwrap();
    let label = visual.debug_bounds("button-label").unwrap();
    assert!(
        button.size.height >= px(28.0) && button.size.height <= px(32.0),
        "{button:?}"
    );
    assert_eq!(icon.center().y, label.center().y);
    assert!(icon.right() < label.left());
    assert!(label.right() < button.right());
}

struct MenuProbe;

impl Render for MenuProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        menu_surface(
            &MenuModel::new(vec![MenuSection::new(
                "choices",
                vec![MenuItem::new("one", "Supervised").subtitle("Ask before making changes")],
            )]),
            |_, _, _, _| {},
            |_, _, _| {},
        )
    }
}

#[test]
fn menu_rows_grow_to_fit_their_supporting_label() {
    let mut test = gpui::TestAppContext::single();
    let window = test.add_window(|_, _| MenuProbe);
    test.refresh().unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    let row = visual.debug_bounds("menu-row").unwrap();
    let title = visual.debug_bounds("menu-title").unwrap();
    let subtitle = visual.debug_bounds("menu-subtitle").unwrap();
    assert!(title.bottom() < subtitle.top());
    assert!(subtitle.bottom() < row.bottom());
    assert!(row.size.height > px(28.0));
}

struct ProviderProbe(ActivityState, ContextUsage);

impl Render for ProviderProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .child(provider_status(
                "test-status",
                ProviderIdentity::Claude,
                self.0,
                "Status",
            ))
            .child(context_meter(
                self.1,
                &fx_design::components::ContextDetail::default(),
                ProviderIdentity::Claude,
            ))
    }
}

#[test]
fn live_provider_states_paint_without_moving_or_growing_the_context_control() {
    // Every activity against every kind of label: unreported, short, and the
    // widest. A fixed usage could not catch a label that wraps or pushes the
    // tile, and a fixed-size tile cannot grow on its own.
    let usages = [
        ("—", None, Some(200_000)),
        ("5%", Some(10_000), Some(200_000)),
        ("100%", Some(250_000), Some(200_000)),
    ];
    let mut test = gpui::TestAppContext::single();
    let window =
        test.add_window(|_, _| ProviderProbe(ActivityState::Idle, ContextUsage::default()));
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    let mut first_tile = None;
    let mut first_meter = None;
    for (label, used, limit) in usages {
        let usage = ContextUsage { used, limit };
        assert_eq!(usage.label(), label);
        let mut same_usage = None;
        for activity in ActivityState::ALL {
            window
                .update(&mut test, |probe, _, cx| {
                    *probe = ProviderProbe(activity, usage);
                    cx.notify();
                })
                .unwrap();
            test.refresh().unwrap();
            let tile = visual.debug_bounds("provider-status").unwrap();
            let meter = visual.debug_bounds("composer-context").unwrap();
            assert_eq!(tile.size, gpui::size(px(20.0), px(20.0)));
            assert!(tile.right() <= meter.left(), "{label} {activity:?}");
            assert!(meter.size.width < px(65.0), "{label}: {meter:?}");
            // The activity never moves or resizes either control.
            if let Some(previous) = same_usage {
                assert_eq!((tile, meter), previous, "{label} {activity:?}");
            }
            same_usage = Some((tile, meter));
            // The label only widens the meter to its right: the tile and the
            // meter's leading edge, height, and vertical place all hold.
            let tile_origin = *first_tile.get_or_insert(tile);
            assert_eq!(tile, tile_origin, "{label} {activity:?}");
            let (origin, height) = *first_meter.get_or_insert((meter.origin, meter.size.height));
            assert_eq!(
                (meter.origin, meter.size.height),
                (origin, height),
                "{label} {activity:?}"
            );
        }
    }
}

/// Draws `tiles` provider tiles, all in one activity, and counts its renders.
struct CountingProbe {
    activity: ActivityState,
    tiles: usize,
    renders: Rc<Cell<usize>>,
}

impl Render for CountingProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().flex().children((0..self.tiles).map(|index| {
            provider_status(
                SharedString::from(format!("counted-{index}")),
                ProviderIdentity::Codex,
                self.activity,
                "Working",
            )
        }))
    }
}

/// A key window drawing `tiles` tiles in `activity`, settled onto the beat.
fn counting(
    activity: ActivityState,
    tiles: usize,
) -> (
    gpui::TestAppContext,
    gpui::WindowHandle<CountingProbe>,
    Rc<Cell<usize>>,
) {
    let mut test = gpui::TestAppContext::single();
    let renders = Rc::new(Cell::new(0));
    let window = test.add_window({
        let renders = renders.clone();
        move |_, _| CountingProbe {
            activity,
            tiles,
            renders,
        }
    });
    // The key window: test windows start in the background.
    window
        .update(&mut test, |_, window, _| window.activate_window())
        .unwrap();
    test.run_until_parked();
    test.refresh().unwrap();
    test.run_until_parked();
    // The first wake was asked for before activation landed, on the quiet
    // cadence; two beats later the window is on the beat.
    test.executor().advance_clock(FxAnimation::BEAT * 2);
    test.run_until_parked();
    (test, window, renders)
}

/// How many times the window rendered over `span` of the executor's clock.
fn renders_over(test: &mut gpui::TestAppContext, renders: &Cell<usize>, span: Duration) -> usize {
    let before = renders.get();
    test.executor().advance_clock(span);
    test.run_until_parked();
    renders.get() - before
}

#[test]
fn a_running_tile_repaints_once_a_beat_and_a_waiting_one_every_other() {
    let second = Duration::from_secs(1);
    let (mut test, window, renders) = counting(ActivityState::Running, 1);
    // Twelve positions a second, one repaint each: the spinner's whole cost.
    assert_eq!(renders_over(&mut test, &renders, second), 12);
    // A beat not yet reached asks for nothing more.
    assert!(renders_over(&mut test, &renders, FxAnimation::BEAT / 3) <= 1);

    // Waiting can last all day, so it breathes at half the beat.
    window
        .update(&mut test, |probe, _, cx| {
            probe.activity = ActivityState::Waiting;
            cx.notify();
        })
        .unwrap();
    test.run_until_parked();
    renders_over(&mut test, &renders, FxAnimation::BEAT * 2);
    assert_eq!(renders_over(&mut test, &renders, second), 6);

    // Settled: the pending wake lands once, then nothing.
    window
        .update(&mut test, |probe, _, cx| {
            probe.activity = ActivityState::Idle;
            cx.notify();
        })
        .unwrap();
    test.run_until_parked();
    renders_over(&mut test, &renders, FxAnimation::BEAT * 2);
    let wakes = activity_wakes_scheduled();
    assert_eq!(renders_over(&mut test, &renders, second * 10), 0);
    assert_eq!(
        activity_wakes_scheduled(),
        wakes,
        "a settled tile kept a clock"
    );
}

#[test]
fn every_spinner_on_screen_shares_one_repaint() {
    // Three running threads, three spinners: still twelve rebuilds a
    // second, not thirty-six, because they wake on one beat.
    let (mut test, _window, renders) = counting(ActivityState::Running, 3);
    assert_eq!(
        renders_over(&mut test, &renders, Duration::from_secs(1)),
        12
    );
}

#[test]
fn a_window_nobody_is_watching_spins_at_half_the_beat() {
    let (mut test, window, renders) = counting(ActivityState::Running, 1);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    visual.deactivate_window();
    renders_over(&mut test, &renders, FxAnimation::BEAT * 2);
    assert_eq!(renders_over(&mut test, &renders, Duration::from_secs(1)), 6);
}

#[test]
fn reduced_motion_holds_the_spinner_and_the_pulse_still() {
    let _theme = ThemeGuard::install(Theme {
        reduce_motion: true,
        ..Theme::default()
    });
    for activity in [ActivityState::Running, ActivityState::Waiting] {
        let (mut test, _window, renders) = counting(activity, 2);
        let wakes = activity_wakes_scheduled();
        assert_eq!(
            renders_over(&mut test, &renders, Duration::from_secs(2)),
            0,
            "{activity:?} moved under reduced motion"
        );
        assert_eq!(activity_wakes_scheduled(), wakes);
    }
}

/// The typing dots, lit at `phase`.
struct TypingProbe(Option<usize>);

impl Render for TypingProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .child(typing_dots(self.0))
            .child(
                div()
                    .debug_selector(|| "typing-caption".into())
                    .child("Thinking"),
            )
    }
}

#[test]
fn the_typing_dots_keep_one_cell_whatever_dot_is_lit() {
    // The lit dot grows by a fifth. Without a fixed cell it pushed the dots
    // after it, and the caption beside them, on every step.
    let mut test = gpui::TestAppContext::single();
    let window = test.add_window(|_, _| TypingProbe(None));
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    let mut first = None;
    for phase in [None, Some(0), Some(1), Some(2)] {
        window
            .update(&mut test, |probe, _, cx| {
                probe.0 = phase;
                cx.notify();
            })
            .unwrap();
        test.refresh().unwrap();
        let cells: Vec<_> = ["typing-dot:0", "typing-dot:1", "typing-dot:2"]
            .into_iter()
            .map(|selector| visual.debug_bounds(selector).unwrap())
            .collect();
        let caption = visual.debug_bounds("typing-caption").unwrap();
        // The cell holds the lit dot, to the half pixel the layout rounds
        // its edges to.
        for cell in &cells {
            for edge in [cell.size.width, cell.size.height] {
                assert!((f32::from(edge) - TYPING_DOT_CELL).abs() <= 0.5, "{cell:?}");
            }
        }
        let layout = (cells, caption.origin);
        let first = first.get_or_insert_with(|| layout.clone());
        assert_eq!(&layout, first, "lit at {phase:?} moved the row");
    }
}

/// The app-wide menu layer, drawn as the shell draws it.
struct LayerProbe {
    layer: Entity<MenuLayer>,
}

impl Render for LayerProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(menu_layer_overlay(
            &self.layer,
            cx,
            |_, _, _, _| {},
            |_, _| {},
            |_, _, _| {},
        ))
    }
}

fn layer_model() -> MenuModel {
    MenuModel::new(vec![MenuSection::new(
        "choices",
        vec![MenuItem::new("one", "One"), MenuItem::new("two", "Two")],
    )])
}

fn redraw(test: &mut gpui::TestAppContext) {
    test.refresh().unwrap();
    test.update(|_| {});
}

#[test]
fn a_menu_rises_into_place_over_its_entrance_and_then_asks_for_nothing() {
    let mut test = gpui::TestAppContext::single();
    let layer = test.update(|cx| cx.new(|_| MenuLayer::default()));
    let window = test.add_window({
        let layer = layer.clone();
        move |_, _| LayerProbe { layer }
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    redraw(&mut test);
    let closed = entrance_frames_requested();
    redraw(&mut test);
    assert_eq!(
        entrance_frames_requested(),
        closed,
        "a closed layer asked for frames"
    );

    let open = |test: &mut gpui::TestAppContext| {
        test.update(|cx| {
            layer.update(cx, |layer, cx| {
                layer.dismiss();
                layer.open(layer_model(), "probe");
                cx.notify();
            })
        });
        redraw(test);
    };
    let top = |visual: &mut gpui::VisualTestContext| {
        f32::from(visual.debug_bounds("menu-panel").unwrap().top())
    };

    open(&mut test);
    let opened = entrance_frames_requested();
    assert!(opened > closed, "an opening menu asked for no frame");
    let first = top(&mut visual);

    test.executor().advance_clock(FxAnimation::QUICK / 2);
    redraw(&mut test);
    let halfway = top(&mut visual);
    assert!(
        entrance_frames_requested() > opened,
        "the entrance stopped early"
    );

    test.executor().advance_clock(FxAnimation::QUICK);
    redraw(&mut test);
    let settled = top(&mut visual);
    // It rose the whole rise, most of it in the first half (ease-out).
    assert!(
        (first - settled - FxAnimation::RISE).abs() < 0.01,
        "{first} → {settled}"
    );
    let rise = Entrance::POPOVER.at(FxAnimation::QUICK / 2, false).rise;
    assert!(
        (halfway - settled - rise).abs() < 0.01,
        "{halfway} → {settled}"
    );
    let requested = entrance_frames_requested();
    for _ in 0..3 {
        redraw(&mut test);
    }
    assert_eq!(
        entrance_frames_requested(),
        requested,
        "a settled menu asked for frames"
    );

    // Each open enters again, even one replacing a menu already up.
    open(&mut test);
    assert!(entrance_frames_requested() > requested);
    assert!((top(&mut visual) - settled - FxAnimation::RISE).abs() < 0.01);
}

#[test]
fn under_reduced_motion_a_menu_only_fades() {
    let _theme = ThemeGuard::install(Theme {
        reduce_motion: true,
        ..Theme::default()
    });
    let mut test = gpui::TestAppContext::single();
    let layer = test.update(|cx| cx.new(|_| MenuLayer::default()));
    let window = test.add_window({
        let layer = layer.clone();
        move |_, _| LayerProbe { layer }
    });
    let mut visual = gpui::VisualTestContext::from_window(window.into(), &test);
    test.update(|cx| {
        layer.update(cx, |layer, cx| {
            layer.open(layer_model(), "probe");
            cx.notify();
        })
    });
    let before = entrance_frames_requested();
    redraw(&mut test);
    let first = visual.debug_bounds("menu-panel").unwrap().top();
    assert!(
        entrance_frames_requested() > before,
        "the fade was dropped too"
    );
    test.executor().advance_clock(FxAnimation::QUICK * 2);
    redraw(&mut test);
    assert_eq!(
        visual.debug_bounds("menu-panel").unwrap().top(),
        first,
        "it travelled"
    );
}
