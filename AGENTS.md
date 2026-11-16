# FlowX (Rust)

A native macOS AI workspace for orchestrating conversations, browser previews,
git inspection, and terminals — a port of the Swift original at
`../_flowx` to Rust and [GPUI](https://gpui.rs).

## Toolchain

- Rust 1.98.1, pinned in `rust-toolchain.toml` to match the version Zed builds
  against.
- `gpui = "0.2.2"` from crates.io. Deliberately the published release rather
  than a git dependency: the API does not churn, which matters when tens of
  thousands of lines sit on top of it.
- macOS with Metal. GPUI needs `font-kit` for glyph rasterization; that is
  already a default feature.
- Zig 0.17.0, only for phase 2 (`libghostty-vt` compiles the Ghostty VT core
  from Zig source at build time).

## Build

```sh
./fx build      # compile the whole workspace, tests included
./fx run        # run the app in place
./fx launch     # assemble a .app bundle and open it
./fx test       # run every test
./fx bench      # run the benchmarks
./fx lint       # fmt check + clippy with warnings denied
./fx clean      # remove target/
./fx clean-probes  # archive the Codex threads a live probe left (see Live probes)
```

`./fx launch` produces `target/debug/bundle/FlowX-Dev.app`. A real bundle
matters: the app needs one to own a Dock icon, a menu bar, and an
application-support directory.

A debug build compiles GPUI and every other dependency at opt-level 0, so
`./fx run` is a slow build of the app, not the app. Judge smoothness on
`FX_CONFIG=release`, never on a debug build.

To see where a frame's time goes, set `FX_FRAME_PROFILE=<ms>`:

```sh
FX_FRAME_PROFILE=4 ./fx run 2>&1 | grep fx_frame
```

Every frame that spends at least that long on the UI thread is logged with its
build, layout, prepaint and paint times, and a summary of all frames follows
every five seconds. `0` logs every frame. The wrapper that measures is
`fx-app/src/frame_profile.rs`; unset, it is not in the tree at all.

## Crate layout

Dependencies point one way only. Nothing lower in this list may depend on
anything above it.

| Crate | Ported from | Depends on | Responsibility |
|---|---|---|---|
| `fx-core` | `Packages/FXCore` | — | Domain models, pure policy services, git parsing, runtime discovery, image preparation. No UI, no GPUI. |
| `fx-design` | `Packages/FXDesign` | gpui | The design system: tokens, typography, icons, shared controls. The source of truth for how FlowX looks. |
| `fx-agent` | `Packages/FXAgent` | fx-core | Providers (Codex, Claude), the conversation engine, streaming, approvals. |
| `fx-store` | `FlowX/Services` | fx-core | Persistence. Owns the on-disk format and nothing else. |
| `fx-bridge` | — | fx-core | FlowX's own tools for the agents it runs: `flowx --mcp-server`, the app's socket, tokens. |
| `fx-term` | `Packages/FXTerminal` | fx-design | Terminal sessions and rendering. Phase 2. |
| `fx-app` | `FlowX/State`, `FlowX/Views` | all of the above | Application state, the window shell, and every view. |
| `flowx` | — | fx-app | A `main` that opens the window. Nothing else. |

Two rules keep this structure intact:

- **`fx-core` never imports gpui.** Anything that needs to draw belongs in
  `fx-design` or `fx-app`. This is what keeps the pure logic testable without a
  window.
- **`fx-design` never imports `fx-core`.** The design system must stay usable by
  any crate, including the terminal.

## Design system

`fx-design` is the code-level source of truth for FlowX interface styling and
components. Implement UI by extending it first, then consume from app views.

### Color architecture

The system works like CSS custom properties — semantic tokens that resolve from
the active theme at draw time:

```
Colors::fg(&theme)          what views read
       ↓
Theme { base_tone, appearance, accent, text_size }
       ↓
Palette::get(tone, dark)    the backing layer
       ↓
ToneScale                   11 shades per tone, 50 → 950
```

- **Views never hardcode a color.** If a token is missing, add it to
  `theme::color::Colors` first.
- Base tones are slate, zinc, neutral, and stone. Dark reads top-down (900 =
  background, 800 = elevated, 700 = surface); light reads bottom-up (50, 100,
  200). All eight combinations are generated once into a static table.
- Accents (violet, blue, emerald, orange, rose) are independent of the tone.
  `on_accent` is white, or the tone's darkest ink for emerald and orange, which
  cannot carry white text.
- Semantic, muted, and diff colors adapt per mode.

### Invariants that must keep holding

`crates/fx-design/tests/design_tokens.rs` enforces these. Rerun it after
touching any foreground, background, or accent token.

- `fg` clears 7:1 on `bg` and `bg_elevated`; `fg_secondary` and `fg_tertiary`
  clear 4.5:1 there, and stay in that order.
- `fg_tertiary` clears 3:1 on `bg_surface`.
- `on_accent` clears 3:1 on every accent, in every tone and mode.
- Banner text (`fg`, `fg_secondary`) clears 4.5:1 on every `*_muted` fill.
- Diff foregrounds clear 3:1 on their own diff backgrounds.
- Status glyphs clear 3:1 on their provider tile, the error glyph on the
  provider tint included, and the context meter's arc clears 3:1 on the
  composer at every fill level. Light-mode `warning` is too pale for that arc,
  so it uses `warning_ink`.
- Selected transcript text stays readable over `text_selection`.
- Checklist text clears 4.5:1 and every checklist mark 3:1 on its surface, with
  every accent. Light-mode emerald and orange fall short, so the active mark
  falls back to `fg` there, as a badge's label does.
- A badge's label clears 4.5:1 on its own fill over `bg`, `bg_elevated` *and*
  `bg_surface`, with every accent. The fill is translucent and badges sit on
  cards as well as panels; measured over the panel alone, dark "Ready" on a
  provider card was 4.2:1. In dark mode only neutral and warning keep their
  hue on both, so the others' labels are `fg` on their tint.
- The title bar's change counts clear 4.5:1 on the title bar at rest, hovered
  and selected. They use the diff inks, not `success`/`error` (light `success`
  was 3.4:1); dark `diff_removed_fg` is 4.2:1 on the selected fill, so it falls
  back to `fg` while the inspector is open.
- A menu's checkmark clears 3:1 on `bg_surface`, hovered or not, with every
  accent. Where the accent falls short (dark violet is 2.15:1), it is `fg`.

All four "hue where legible, `fg` where not" rules go through
`theme::legible_or`, given every fill the element can be drawn on — its color
does not change when the pointer or the surface under it does. So do
`error_ink`, `accent_secondary_ink` and `success_ink`, which are what text in
those hues uses on any surface: plain `error` on `bg_surface` was 3.7:1.

Keyboard focus is drawn with `Colors::focus_ring` (≥3:1 on every fill, an
invariant) through `fx_focus_ring()`, and shows once a view calls
`track_focus`. A hover never sets an edge colour: hover is applied after
focus and would hide the ring. A Danger button's label is `fg` on its tint,
as a badge's is; even `error_ink` misses 4.5:1 there.

One documented shortfall, recorded rather than hidden: where a status hue is
used as an *icon on its own tint*, light-mode `warning` bottoms out at 2.29:1
and `success` at 2.67:1, against the 3:1 non-text guideline. Dark mode clears
3:1 everywhere. The floor is pinned by
`status_ink_meets_its_documented_floor_on_its_own_tint` so it cannot get worse.
Raising it means darkening the light-mode values — a palette change, not a port.

Run the measurement yourself after any retune:

```sh
cargo run -p fx-design --example contrast_report
```

### Typography

SF Pro Rounded for UI text, SF Mono for code, terminal output, and diff line
numbers. Sizes come from `TextRole` and are multiplied by `TextSizePreset`; no
view writes a point size. Every role also carries a line-height multiplier
because GPUI has no automatic line height.

### Spacing, radii, borders, elevation

8px baseline grid (`Spacing`), `Radii`, `BorderWidth` (hairline 0.5 by default),
and `Shadow` recipes. Apply elevation with `.fx_shadow(...)`; never write a raw
`shadow(...)`.

### Components

`fx-design/src/components/` holds the shared control vocabulary. Extend it
before styling a screen ad hoc.

| Module | Provides |
|---|---|
| `button` | Labelled button (primary/secondary/ghost/danger), icon button, compact action button (with an on/disabled variant, `compact_action_toggle`), and `glyph_button` for rows too dense for an icon button's 28 pt hit target. Hover and press are declarative `.hover`/`.active` refinements, so a button repaints without re-rendering. |
| `checkbox` | A labelled checkbox whose whole row toggles. |
| `checklist` | A read-only checklist: done, active (spinning while live) and pending rows on one surface. |
| `surface` | Card, panel, divider, banner, empty state, section header, and `floating`/`floating_scroll`: a popover's shadow, hairline edge and corner around content that may scroll. |
| `status` | Badge, activity dot, typing indicator, context-usage ring, and `status_glyph` (running, done, failed, stopped): the finished marks share one Small slot, and running is the spinner at its own size. |
| `segmented` | Segmented control for exclusive choices. |
| `settings` | The preferences vocabulary: a section heading over a group card whose rows (title, description, trailing control) are split by hairlines, the navigation's group label and search field, a breadcrumb, and an on/off `switch`. |
| `tooltip` | Hover tooltip body. |
| `menu` | Dropdown and context-menu panels, plus `MenuLayer`, the one-open-menu-at-a-time owner. |
| `menu_model` | `MenuItem` / `MenuSection` / `MenuModel`, and the highlight and keyboard-navigation rules. A `MenuItem::submenu` row opens another menu in place, at `MenuLayer::last_position`, since only one menu is open at a time. |
| `dialog` | Every modal: the dialog shadow on the surface (never on a padded wrapper), one title role, a ghost close, a footer; its `capture_key_down` calls `cycle_focus` first, so Tab and Shift-Tab move between its controls and Space presses the focused one, once (`focusable` blocks the click GPUI adds on the key's release). Choice rows (`choice_row`, given a focus handle) are in that order, and Space chooses one. |
| `loading` | `loading_spinner(id)` for work whose end FlowX cannot see coming, on the shared beat. |
| `motion` | `request_activity_frame` and the activity `Beat`: every continuous phase is read from one `Instant`, 12 beats a second (half that in a window that is not key), and an element asks for its next beat only while it is drawn. |

Color decisions live in the `*_palette` and `*_ink` helpers rather than inline
in the element tree, so they can be asserted without a window. Prefer those
functions; the element builders just apply them.

A few layout rules the views keep:

- **GPUI's overflow mask is a plain rectangle**, so a filled row inside a
  rounded panel takes the panel's corners itself (`fx_inner_corners`).
- **A control that shows on hover changes its opacity**
  (`revealed_on_hover`), never its visibility: a hidden element gets no
  hitbox, and the row's actions must stay reachable.
- **A path is drawn with `path_label`**, which gives up folders before the
  file name; GPUI 0.2.2 can only truncate at the end.
- **A selected row keeps its fill under the pointer** (`list_row_fill`).
- **A turn's rows share one rhythm.** Prose, a group of calls, a plan and
  the live tail are `Layout::TRANSCRIPT_ROW_GAP` apart, the same as a
  paragraph inside one reply (markdown's block gap is that constant), and
  the reader's prompt keeps `TRANSCRIPT_TURN_GAP` either side. No row adds
  vertical padding of its own: each once padded itself and then added 24pt
  below, and replies read 30 to 38pt apart against Claude's 16. A group's
  header lines its words up with the prose and gives its hover padding back
  on the leading side.
- **A prompt's images are thumbnails above its words** (`image_thumbnail`,
  the Swift original's 176pt at 1.6, cropped), never previews inside the
  bubble, and the bubble takes the composer's fill (`user_message_bg`).
- **A finished call's status mark is Small (11pt).** At Medium (14pt) it made
  GPUI's list reserve about 5,300pt for an activity group once it was opened,
  in the real app on two threads: the group sat at the top of that slot and
  the prompt above it was pushed out of view, so the transcript read as
  blank above the reply. Bisected to that one size, and gone at 11pt. The
  test platform's text system does not reproduce it, so check a change to a
  row's marks in the running app.

### Motion

Continuous motion (spinners, the waiting pulse, typing dots, the caret) asks
for its next beat from the element that draws it, through
`request_activity_frame`; it never counts render passes and never uses GPUI's
`with_animation`, which asks for a frame on every refresh and loses its clock
when its element is not drawn. Timers that are not motion use `on_the_beat`,
so they share a frame with the spinner. A one-shot entrance (menus, the
palette, the @ picker, approval and question cards) stores its own start and
asks for frames only until it settles; closing is instant. A panel slide lays
the centre out at its widest width for the whole slide and clips it, because
`gpui::list` drops every row height when its width changes. Reduce Motion is
read from `NSWorkspace` in `fx-app` (`motion_preference.rs`) at first draw and
on each activation and stored on the theme: offsets and slides go, the
sidebar's collapsible sections included, spinners still, fades stay. State a timer reads back belongs in an app `Global`, never a
thread-local: two test apps on one thread share thread-locals and window ids.

### Menus

The Swift original rejected `NSMenu` and built flat panels on an `NSPanel`,
which meant every open panel also owned a separate window, a pair of event
monitors, and an `NSHashTable` hit-test tree for nested context menus.

GPUI makes all of that unnecessary: `anchored()` positions a panel against its
anchor and keeps it inside the window, so a popup is one absolutely positioned
element in the same window as its content. What that leaves behind is the
*decision* half, which is split in two:

- **`MenuModel`** is pure state — sections, items, and the highlight key.
  Keyboard movement, wrapping, disabled-item skipping, and activation are all
  methods on it, so they are tested without a window.
- **`MenuLayer`** is the app-wide owner of *which* menu is open. Only one at a
  time, so opening a second closes the first. It stores no callbacks: opening
  and closing return what happened, and the caller reacts.

Dismissal needs no global monitors. A transparent backdrop covers the window and
closes on mouse-down; the panel is stacked above it.

Two things are deliberately preserved from the original:

- The panel is sized to its content, with a hairline edge and the popover
  shadow (`floating`). GPUI 0.2.2 paints an element's shadow before its own
  overflow mask, so a panel that scrolls keeps it; the shadow is clipped only
  by an ancestor's mask.
- Pointer and keyboard share one highlight, so pressing a row and then pressing
  an arrow key continues from the pointer rather than from the top of the menu.
  A row's fill comes from that highlight alone, never a hover fill of its own:
  a submenu opened under the pointer lit two rows, and Return chose the one
  the pointer was not on.

Two behaviours are deliberate departures from the Swift original, both because
GPUI cannot express the original:

- A pressed button dims to 0.94 opacity instead of scaling to 0.97. GPUI's
  style refinement has no transform, and animating a scale would re-run layout.
- The activity spinner is drawn as twelve rotated capsules rather than a
  trimmed stroke, for the same reason.

One accessibility improvement: `BadgeTone::text_ink` uses the tone's hue for
the label only where it clears AA against its own fill, over both a panel and a
card, and falls back to `fg` where it does not. Light-mode `warning` over its
own tint is 2.6:1, which is unreadable at 12pt. The tint still carries the
tone, so the badge keeps its identity without shipping an illegible label.

A selected row keeps its selection fill under the pointer — a thread row, a
settings destination, the title bar's change counts — as the segmented control,
`choice_row` and `compact_action_toggle` always did. GPUI's hover refinement
replaces the background rather than layering on it, so an unconditional
`.hover(bg_hover)` turned the 8% selection into a 4% wash.

### Icons

`fx-design/src/icon.rs` exposes glyphs under the **same SF Symbols names the
Swift app used**, so call sites port without renaming — `Icon::CHEVRON_DOWN`,
`Icon::EXCLAMATIONMARK_TRIANGLE_FILL`, and so on.

Glyphs are Lucide (ISC), vendored in `crates/fx-design/assets/icons/` and embedded
by the build script. **GPUI's `Svg::path` takes an asset key, not SVG drawing
instructions.** `Icon::render` supplies `icons/<name>.svg`, and the application
installs `IconAssets` with `Application::with_assets`. GPUI parses and caches the
original SVG, preserving its stroke geometry without runtime filesystem reads.
Flattened path data remains available for geometry checks and `Icon::path`
compatibility; it must never be passed directly to `Svg::path`.

To add an icon: drop `<sf-symbol-name>.svg` into `assets/icons/`, add a constant
to the `icons!` macro at the bottom of `icon.rs`. The tests check that the table
stays sorted (lookup binary-searches), that names stay unique, and that no
coordinate escapes the 24×24 viewBox. They also resolve every embedded asset and
rasterize it through GPUI's SVG engine to verify that it paints visible pixels.

Lucide is stroke-only, so a few Swift `.fill` variants render as outlines. That
is consistent with FlowX's flat treatment, not a regression.

## GPUI 0.2.2 API notes

The published crate's API differs from the current gpui.rs documentation. Where
they disagree, this crate follows 0.2.2. The differences that actually bite:

- Colors are `Rgba` (and `Hsla`). There is no `Color` type.
- `WindowOptions` takes `window_bounds: Option<WindowBounds>`, `titlebar:
  Option<TitlebarOptions>`, and `window_background:
  WindowBackgroundAppearance`. There are no `bounds` or `title` fields.
- `Bounds::new` takes `Point` and `Size`, not four numbers. Construct with
  `gpui::point`, `gpui::size`, and `gpui::px`.
- `Pixels` has a private field; always go through `gpui::px`.
- `Application::run` takes a single `&mut App` closure.
- `Render::render` takes `&mut Context<Self>`, not `&mut App`.
- Shadows are `shadow(Vec<BoxShadow>)`, not `box_shadow(offset, blur, color)`.

### Interactive elements need an id

This is the one that costs the most time to rediscover. GPUI splits
interactivity across two traits:

| Available on a plain `div()` | Requires `div().id(..)` first |
|---|---|
| `hover`, `on_mouse_down`, `on_mouse_move`, `on_key_down`, `group_hover`, `occlude`, `on_drop` | `on_click`, `active`, `tooltip`, `hoverable_tooltip`, `overflow_scroll`, `track_scroll`, `focusable` |

`.id(..)` returns `Stateful<Div>`, which is what implements
`StatefulInteractiveElement`. So a button reads:

```rust
div().id("send").hover(..).active(..).on_click(..)
```

Both traits come from `gpui::prelude::*`, which every component file imports.

### Style refinements

`.hover(..)` and `.active(..)` take a callback returning a `StyleRefinement`,
where every field is an `Option` and the nested `text` style is itself an
`Option`. Use the `FxRefinement` helpers from `fx-design::styled` rather than
touching that shape directly:

```rust
.hover(|s| s.fx_background(hover_bg).fx_text_color(hover_fg))
```

### Other gotchas worth knowing

- There is no `scale`, `rotate`, or transform on a styled element. An element
  can only be laid out and painted. Animated emphasis has to come from opacity,
  size, or a custom paint pass.
- A tooltip body must be a real view (`Entity<V>` converted with `AnyView::from`),
  not an element tree.
- `ElementId` converts from `SharedString` and `&'static str`, not from `String`.
- **A `uniform_list` counts its own vertical padding twice**: once in where
  its rows start and again in how far it scrolls, so a padded list scrolls
  with everything in view. Put vertical insets on a wrapper. The list needs
  `ListSizingBehavior::Infer` to fit its rows under a `max_h`; the default,
  `Auto`, sizes it from its style alone, which under only a `max_h` is zero.
- **GPUI clicks a focused element again when Return or Space is released.**
  A dialog button pressed on key-down was pressed twice, and Return on a
  focused Cancel withdrew the provider's question; `focusable` in
  `dialog.rs` blocks the second press.
- **Any `.cursor()` makes an element re-render as the pointer enters and
  leaves it**, so counting renders proves nothing about hover.

## Persistence

`fx-store` owns the on-disk format and nothing else. Three rules follow from that,
and each one exists because its alternative was tried first.

- **The format depends on `fx-core`, never on `fx-agent`.** A persisted record is
  built from `Vec<ConversationMessage>`, not from an engine conversation, so the
  format outlives any particular engine.
- **Every write is atomic and leaves a `.backup` sibling.** The primary lands
  before the copy is replaced, so a crash between the two leaves the previous
  good copy rather than losing both.
- **A read falls back on a parse failure, not only a read failure.** An
  interrupted write leaves a truncated file that opens fine and then does not
  parse; a fallback that only triggers on `read` would never fire for the damage
  it was added to survive.
- **One unreadable entry costs only that entry.** Lists decode element by
  element and unknown enum values read as the default — every one, the
  inspector tab and a goal's status included. Access levels are the exception:
  an unknown one reads as `Supervised`, because the default and the inherited
  app default are both Full Access. Lists that refer to entries (archived
  threads, pending titles) keep the ids of entries carried as raw JSON. An entry this build
  cannot read is kept as raw JSON and written back unchanged, so a newer build's
  data survives a round trip through an older one.

Both copies are written to a unique, owner-only temp file, flushed with
`F_FULLFSYNC` before the rename, and the directory is flushed after both
renames. A fixed temp name let two instances truncate each other's file.

Files under the application-support directory are created `0700`. Everything
there can contain prompts and code, so it does not inherit a wider mode.

An empty transcript is **deleted** on quit rather than written as a stub: an
unopened provider thread has nothing to save, and writing it anyway would
truncate every other thread's cache.

A queued prompt is saved with its transcript, images included, because
queuing clears the composer: a queue held only in memory was lost to every
quit. It comes back in the composer rather than as a queue. After a relaunch
nothing is running for it to wait behind, and FlowX never sends a prompt on
its own.

A composer's unsent images are saved with its draft, including an image-only
draft. The saved image list distinguishes an absent legacy field from an
explicit empty list, so removing a failed prompt's image stays removed after
relaunch. Image ids participate in the save version; checkpoints share the bytes
with the writer instead of copying them on the UI thread.

Native discovery keeps a missing thread visible while it has local input,
including image-only drafts and queued prompts. Check both the live composer
and an unopened thread's saved `has_pending_input`, or a saved image can remain
on disk with no chat left in the sidebar to recover it from.

Images imported while a draft loads are merged with its saved images. The save
version remembers only the disk's image ids, so that merge is saved even without
a text edit. Each import owns a ticket; an in-place Reset, Delete and project
removal revoke its tickets and release its composer buffers. A late result cannot
attach to a reset thread or consume a newer import's ticket.

A goal belongs to its provider session. Restart clears the retired session's
goal and counters; Resume keeps them. Queued prompts keep their images when a
caption is removed, so saving an image-only edit is valid.

Recovering a queue keeps every saved character and image, even when several
valid prompts exceed one composer's ordinary limits. Recovery reports that
case and allows the draft to be edited without growing until it is back under
the normal cap; it never silently truncates already saved work. Workspace
metadata saves measure and write the same encoded bytes before replacing either
recovery file.

A binding records where its session starts in the transcript (`local_start`)
when FlowX opened that session in a thread that already had turns, as Restart
does. Native sync compares and replaces only from there. Compared whole, a
restarted thread's history could never match again, and the sidebar said
"Thread sync needs attention" on every pass. A prompt that replaces one the
provider never accepted takes that prompt's row for the same reason.

One cost of the crate boundary: `fx-store` cannot import `fx-design`, so the
inspector's geometry is declared in both `PersistedApp` and `Layout`.
`the_persisted_defaults_match_the_design_metrics` in `fx-app` is the only test
that sees both, and is therefore the only place the duplication can be caught.

## Chat worktrees

An agent's optional `ChatWorkspace` binds it to a linked Git checkout. Missing
means the project folder; malformed metadata stays unreadable rather than
silently selecting another folder. `AppState::project_root_for` is the execution
root for prompts, recovery and native history; `active_workspace_root` also
routes the inspector and context references. Project-level native discovery
must not mark worktree bindings missing or clear their external-waiting status.

New worktrees use a new branch at the current commit. Creation never copies,
stages or stashes source edits, never forces an existing path or branch, and
runs on the Git worker. The chat is selected only after its binding is saved.
If saving fails, the dialog retains the prepared checkout for another save.
Archive, Delete and project removal keep checkout files and branches.

Before a provider starts or the inspector commits/pushes, validate the saved
path, common Git directory and worktree registration. Never fall back to the
project when a worktree is missing. Reset keeps the checkout while retiring the
provider session. Worktrees live in the private app data directory, independent
of conversation cache deletion.

## Spaces

A space is a named, coloured group of a project's threads, drawn as a heading
in the sidebar with its threads under it; a thread in none is listed after
them. The project owns its spaces (`PersistedProject::spaces`, in draw order)
and a thread names its own (`Agent::space`), so moving a thread is one field
and deleting a space only clears that field on its threads. Spaces are
grouping, not order: `RecordIndex::visible` stays one flat list in thread
order, so ⌘1–9 and the palette number threads as before, and the sidebar
groups them as it draws. A space's lead is its header, not a row, so
`visible` leaves leads out and `RecordIndex::listed`, which search reads,
keeps them. The open thread always has a row: moving it into a
closed space opens that space, and so does opening it from elsewhere (⌘1–9,
the palette, a restore), as its project already did. `Space::COLORS` is the
accent names, because `fx-core` cannot see the design system; a sidebar test
checks the two agree.

Codex files threads in account-wide sections (`thread/list`'s `section {id,
name, appearance}`), and a Claude desktop project (Projects, beta) starts
Claude Code threads on the Mac from a chat on claude.ai. Each project index
pass mirrors both, read-only, as one space per section or project in each
project that has threads in it (`Space::provider_section`, saved under its
first name `codex_section`; a Claude project's id is behind
`Space::CLAUDE_PROJECT`). Four rules:

- **The provider moves a thread only when its section changed.** The binding
  records the section the provider last listed (`PersistedBinding::section`);
  a listing that differs from it files the thread in that section's space,
  and one that does not leaves the reader's own placement alone, as an
  archive does.
- **A mirror no thread names is removed**; a space the reader made is never
  removed by a pass.
- **Nothing is written back.** Codex's sections and Claude's projects are
  their accounts', and FlowX does not move or rename them.
- **A Claude project is named "Claude project" until the reader renames
  it.** Claude keeps neither the project's name nor its chat on the Mac (the
  chat is the coordinator, a session on claude.ai; the app's Local Storage
  holds only analytics about it), so the project is known by the id its
  threads' messages carry, and two in one folder are numbered. A pass names
  a mirror only when it makes one, so a rename holds.

## Orchestrators

Every space has a lead: a thread with `Agent::lead_of` naming the space,
which its header opens and which runs the space's other threads through
FlowX's tools. It is made the first time the header is opened
(`AppState::ensure_space_lead`), so no empty thread is made up front, with
`fx_core::policy::orchestrator::BRIEF` as its system prompt and FlowX's
tools on.

- **One lead per space, by one rule:** the first listed thread whose
  `lead_of` names it, in `agents` order (`snapshot_index::space_leads`).
  Renaming the space renames its lead. Deleting the space keeps the lead as
  an ordinary thread, its brief dropped and its read tools kept. Archiving
  it or Reset hands the role to the next. A Codex-mirrored space is kept
  while it has a lead. Every call is checked by that rule too: a second
  thread naming the space is an ordinary row and no lead. A lead stays in
  the space it leads, so neither "New space…" from search nor a Codex
  section moves it, and another space's lead is never a worker.
- **Only the chevron folds a space.** The rest of the header opens the lead,
  takes the selected fill while it is open, and shows its activity; opening
  a lead never unfolds its space.
- **A lead acts only inside a turn the reader started.** `start_thread`,
  `send_to_thread`, `wait_for_threads` and `stop_thread` are offered to a
  lead alone and checked as one on every call: the server's role hint
  (`FLOWX_MCP_ROLE`) only decides what is listed, and proves nothing. FlowX
  never prompts a lead by itself, and a lead's Stop stops only the lead.
  Every call also needs the lead's own turn running, and not stopping: a
  start still making its worktree when the reader pressed Stop was
  finished afterwards, and ran a thread nobody asked for.
- **A worker is an ordinary thread in the lead's space** (`spawned_by`),
  with the lead's access, none of FlowX's tools, its own title kept (never
  auto-named), and not selected when it starts. `worktree: true` gives it a
  linked checkout on a `flowx/` branch, as a worktree chat gets.
- **Starts are bounded.** At most `MAX_RUNNING_WORKERS` (4) of a space's
  threads run at once, sends included, checked again on the UI thread when
  the thread is made, so two starts cannot race for the last slot. No new
  work starts once the provider's usage passes `LIMIT_WARNING_AT`, or while
  a refusal for the limit holds; it holds until the window behind it (a
  full one, or the first to reset) has reset. A start may take 240 s,
  which every step's own bound fits inside: cut at 150 s, its worktree or
  thread was made after the lead was told to try again. Making a worker
  saves the thread and waits on no transcript save, and a prompt to a
  worker nothing has loaded starts its transcript's read in the
  background and asks the lead to send again.
- **A wait is bounded and watches.** At most 240 s, polled off the UI
  thread; a worker blocked on the reader counts as settled, as "waiting",
  so the lead can say where to answer. A thread running in the Claude app
  is running, and takes no prompt. Listing, reading and waiting are
  reads: answered without a card at any level that asks. Starting, sending
  and stopping are each a card there, and Read Only refuses them.
- **A tool timeout is set per server, never through the environment.**
  Codex 0.167's default MCP tool timeout is 300 s (`tool_timeout_sec`,
  float seconds) and Claude 2.1.325's is 1e8 ms (`MCP_TOOL_TIMEOUT`); a
  lead's server entry states 600 s in each one's unit (`timeout` in ms for
  Claude), so a user's own low setting cannot cut a wait short.
- `orchestration.rs` is declared from `state.rs` with `#[path]` to read the
  state's private fields; a `#[path]` module's own `mod x;` resolves beside
  `src/`, so its tests stay inline.

## Native history

`fx-agent::claude::native` reads Claude's own transcripts, read-only, for the
sidebar and for a thread's history. Five rules, each of which hid real threads
first:

- **A transcript is the workspace's whose history folder it is in.** That is
  how Claude lists and resumes it. Deciding by where the session started
  refused 73 of one renamed project's 129 threads (`_azoresgame` became
  `azoresgame`, so every record named a path no longer there), and every
  desktop session that began in a scratch folder and moved into its project.
- **A recorded directory only breaks a collision.** The folder's name is an
  encoded path and two paths can encode the same, so the first `cwd` that
  encodes to this folder's name is asked one thing: is it this workspace, or
  another that still exists (`claim`)? A directory that encodes differently
  settles nothing, and one that is gone is what a rename leaves behind.
- **The folder is named as the CLI names it**, read out of the binary
  (2.1.322), not its documentation: every UTF-16 unit outside `[a-zA-Z0-9]`
  becomes a dash, and past 200 characters the name is cut and closed with
  `Math.abs(hash).toString(36)`. Keeping Unicode letters looked for `-Açores`
  where Claude wrote `-A-ores`: a path with an accent had no threads at all.
- **A refusal is remembered like a summary**, by the file's fingerprint. A
  transcript refused at its last record was otherwise read to that record on
  every five-second pass. Two things are asked again each time: whose
  workspace it is, which the folders around it decide, and a transcript
  accepted because a directory was missing, once that directory is back.
- **One warning per reason, and none for an empty file.** A warning per thread
  was a line per thread, and a tooltip taller than the window is drawn at the
  window's top, over the pointer and the Retry button under it; the sidebar
  also caps what it shows (`sync_detail`). Claude creates a session's file
  before writing to it, so a zero-byte one is not a thread and not a warning.
  Any warning stops the project's deleted threads being hidden, so a
  permanent one is never free.

A provider that is not installed is nothing to sync: no warning, and its
empty listing is never proof that a thread is gone. A project root is
resolved once a pass and compared as written and resolved, never with a
`realpath` per thread. Claude keeps a folder beside each session, so only
`.jsonl` files count toward the folder limit, and the summary cache, shared by
every project, never evicts the folders being listed. A transcript over the
history limit is still listed, from what fits under it; only importing its
history is refused, when the thread is opened. New threads, made here or found
by a pass, are listed first: appended, they fell below "Show more". A FlowX
archive or restore gives way once the provider's own flag changes.

Claude desktop's archive flag is mirrored, so a project worked on there can
have every thread archived. The sidebar then says "No open threads" over a row
that opens the archive, not "No threads yet". So is its title
(`claude-code-sessions/…/local_*.json`'s `title`), which names the thread
over the transcript's own title and its first prompt: it is what the reader
knows the thread by there.

A thread a Claude desktop project started carries the project's traffic as
user records in three shapes (`claude::project`, read from 2.1.325's files):
the coordinator's brief, a meta `<relay reason="spawn">`; the reader's own
message, a `<wake>` whose `<message from="human">` is verbatim and whose
`<project id="chan_…">` names the project; and a relayed message, a task
notification (`subkind: projects-relay`) wrapping `<cited author="user">`
words and the coordinator's `<note>`. The reader's words are a prompt; the
coordinator's are `MessageContent::ProjectNote`, a System row drawn as a
compaction is ("Note from the project chat", the note behind a disclosure),
which `as_text()` does not read. Read as plain prompts, the brief and every
relay were hidden as meta and task notifications, and the wake was the
thread's prompt and title as raw markup (`<wake reason="mention" cu…`). A
wake that carries none of the reader's words is shown as written.

## Runtime discovery

Neither provider CLI is bundled. Both are the user's own install, found at launch
by `fx-core::services::runtime`, which makes it less about spawning a process and
more about deciding whether a file on disk is safe to execute.

Three rules, each of which is a bug that was written first:

- **A GUI app cannot trust its own `PATH`.** Launched from Finder, a process
  inherits launchd's minimal `PATH`. The search therefore has its own list, and
  every launch rebuilds `PATH` around the directory the binary was found in — an
  npm-installed provider's script needs the interpreter beside it. Empty `PATH`
  entries are dropped, because an empty component means the working directory.
- **Quarantine is never bypassed.** A downloaded binary carries
  `com.apple.quarantine` until a human clears it. FlowX reports it and stops,
  and never removes the attribute. The assessor and the attribute reader are
  injectable, because the policy cannot be tested if setting up the condition
  needs `xattr` or a signed binary.
- **Indirection is resolved before the quarantine check**, so a clean-looking
  symlink cannot hide a quarantined target. The reported path is the resolved one,
  so swapping a link later cannot redirect a launch.

A quarantined binary is skipped, not fatal: if a working one exists later in the
search it is used. That is a real case — on the machine this was written on, the
Homebrew cask `codex` is quarantined while a `~/.local/bin` install is not.

What counts as quarantined is narrower than "has the attribute", for two reasons:

- **A notarized binary passes.** The default assessor is Swift's
  `SecStaticCodeCheckValidity` requirement (Apple, or Developer ID + notarized),
  remembered per file identity. That same Homebrew `codex` is quarantined and
  notarized, and is accepted.
- **An approval keeps the attribute.** Approving a binary in Finder sets the
  user-approved flag (`0x40`) in the value rather than removing it, so the value
  is parsed. Checking presence alone meant the approval FlowX asks for could
  never clear it.

The extra directories are directories: the candidate is `<dir>/<name>`. Checking
the directory itself always missed, and the search silently fell through to a
login shell that does not read `~/.zshrc`.

The search paths are the Swift app's, in its order: Codex prefers the copy
bundled in ChatGPT.app or Codex.app, then Homebrew, `~/.local/bin`, `~/.npm/bin`
and nvm. A glob may sit mid-path (`~/.nvm/versions/node/*/bin/codex`), and
versions sort by number, so v22 wins over v9. The launch `PATH` leads with the
directory the binary was *found* in, then the resolved one: an nvm link resolves
into `lib/node_modules`, which has no `node` beside it.

Discovery tests run with `without_extra_directories`. Without it a "not found"
assertion can be satisfied, or defeated, by whatever happens to be installed on
the machine running the suite.

Two of these were found by running the stack against the real `codex
app-server` rather than by testing, and both are now pinned by tests:

- **A directory is not a binary.** Checking only the execute bit selects
  `/opt/homebrew/bin` as the `codex` binary — every preferred search directory
  carries that bit — and the launch then fails with a permission error on a
  directory.
- **Codex sends thread timestamps in seconds, not milliseconds.** Read as
  milliseconds, a same-day thread lands in the year 58656. The live payload also
  calls the title `name`, not `title`.

An invented test fixture gets both of these wrong and asserts the wrong thing
confidently. The captured real payload is in `a_live_thread_shape_is_read_correctly`.

## Provider transport

`codex app-server` and `claude` both speak newline-delimited JSON-RPC on stdio.
The Swift original ran this as one 4,000-line actor holding the process, the
pending-response map, the interrupt watchdog, and every protocol method. It is
split here so the failure paths are reachable from a test:

- `LineFramer` — pure, no IO.
- `PendingRequests` — who is waiting on what, and how a failure is broadcast.
- `Transport` — the process and its two reader tasks.

The process tests spawn a real `python3` fake server. A mocked stream cannot reach
the things most likely to break: handshake order, response correlation over an
actual pipe, and what a caller sees when the process dies mid-request.

Four rules cost real time to get right, and each has a test:

- **`initialize` carries the reserved id 0**; dynamic ids start at 10. The server
  treats low ids as handshake traffic.
- **`initialized` is sent only after `initialize` succeeds.** Sending it earlier
  is a race the server loses.
- **An exit and a rejection are different errors.** `Outcome::Answered` and
  `Outcome::Dead` exist because funnelling both through one error type made a
  dead server report itself as a rejected method.
- **A request against a dead or closed process fails immediately.** Otherwise
  every request after a crash waits out the full 30-second control timeout with
  the composer disabled. That includes `initialize`: the exit watcher starts
  before the handshake, so a server that dies answering it fails at once, with
  its stderr.

Two more, for how a turn ends:

- **Silence is never a failure.** A turn ends on completion, a provider or
  process error, the stream closing, or Stop. A quiet `docker build` or a long
  think is not a hang, and a silence timeout killed exactly those turns.
- **An interrupted turn was stopped, not completed.** Another client, or Codex
  itself, can end a turn with `turn/completed` status `interrupted`; read as a
  completion it showed the Ready check and the unseen-completion dot for a turn
  that never answered. FlowX's own Stop never sees that status, because it
  drops the turn first.
- **Thinking is asked for, or it is not there.** A print-mode `claude` run
  defaults to `thinking_display: updates`, under which every `thinking_delta`
  carries empty text and only `system` `thinking_tokens` estimates arrive.
  Every turn passes `--thinking-display summarized`, which streams the
  model's summarized thinking in the same frames and records it in the
  history, so a restored thread reads the same as it did live. The assembled
  `assistant` frame repeats the thought whole, so it is dropped once a delta
  carried text, as the reply's is. Codex's reasoning is its summary
  (`item/reasoning/summaryTextDelta`, a `summaryPartAdded` per paragraph);
  the raw reasoning is encrypted, and `reasoning_text` in `codex/session.rs`
  reads the live item and the rollout's `summary_text` parts alike. It is
  asked for too: every `turn/start` sends `summary: "auto"`. Without it,
  codex-cli 0.167 on gpt-6-astra completes each reasoning item with
  `summary: []` and streams none of it.
- **A tool call opens at its block start.** Claude's `content_block_start`
  for a `tool_use` is the call, with no arguments yet; `input_json_delta`
  frames grow them, and the assembled frame replaces them whole
  (`ToolInput`). A plan call (`TodoWrite`) waits for its assembled frame: half
  a plan is not a plan card. Codex streams a command's output as
  `item/commandExecution/outputDelta` and a patch as `fileChange/patchUpdated`,
  which is the call's arguments in the completed item's shape; both are
  `ToolOutputDelta` / `ToolInput`, and the live output is never recorded,
  because the completed item carries the whole.
- **A compaction is a boundary, and its summary belongs to it.** Claude's
  `system` `compact_boundary` (`compact_metadata`: the trigger, `pre_tokens`,
  `post_tokens`) is the `Compacted` event. The synthetic user message after it
  ("This session is being continued…", `isSynthetic` live, `isCompactSummary`
  in the history) is what the model read in place of the dropped history,
  never a prompt: live it is `CompactionSummary`, in the history it is
  attached to the last boundary. Read as a prompt it was the thread's latest
  prompt in the sidebar and split the turns around it. Codex reports its own
  twice, as a `contextCompaction` item and the deprecated `thread/compacted`,
  so `TurnNotifications` drops the second, and `compact()` synthesizes a
  boundary only when neither arrived.
- **What fills the context is asked for as the turn's input closes.** The CLI
  reads stdin in order, so a `get_context_usage` control request (`detail:
  full`, `CONTEXT_REQUEST_ID`) written just before `Close` is answered before
  the exit, and its `control_response` is the `/context` breakdown
  (`ContextBreakdown`) the gauge's tooltip lists. The answer comes after the
  `result` it was written at (0.6 s later on 2.1.322), so a turn that ends at
  that `result` reads on for it inside the moment the process is given to
  exit, and a turn that reads to the exit must not take it for the turn
  saying something: counted as content, it marked a follow-up as under way,
  and every steered or background-task turn ended as a failure. The fakes
  answer the request as the CLI does; one that said nothing let both of those
  ship. The idle probe
  (`claude::catalog`) sends the same request and carries the same breakdown.
  The report's `totalTokens` is the fill; a `used` category is in the window,
  while `deferred`, `buffer` and `free` are not.
- **Usage limits are the account's, not the thread's.** Claude's
  `rate_limit_event` (`unifiedWindows`: five-hour and seven-day, absent for an
  API-key account) and Codex's `account/rateLimits/updated` (`primary` and
  `secondary`, named by `windowDurationMins`) both become
  `ProviderEvent::RateLimits`, which the conversation ignores and the app
  keeps per provider (`AppState::rate_limits`: live, never saved). Codex says
  nothing before a turn, so `run_codex` reads `account/rateLimits/read` once
  the thread is open, bounded to two seconds and never waited on by the
  prompt; the fake answers it as an unknown method, which is the API-key
  case. A window past `LIMIT_WARNING_AT` badges the composer beside the gauge.
- **Codex's context fill is the last turn's total minus its reasoning.**
  `thread/tokenUsage/updated`'s `last.totalTokens` counts
  `reasoningOutputTokens`, which the next turn does not carry; the TUI
  subtracts them, and so does `context_tokens` in `codex/session.rs`, live
  and from a rollout's `last_token_usage`.
- **Stop ends the tools too.** Each provider runs in its own process group
  (`fx-agent::process`), but its tools do not stay in it: `claude` starts each
  `Bash` call in a session of its own and `codex app-server` gives each command
  a group of its own. So Stop works on the provider's *tree* — its
  descendants, and every group and session one of them leads, which still
  finds a tool's children once launchd has adopted them. Stop sends the
  provider's interrupt, then SIGTERM to the tree, a short grace, then SIGKILL.
  A turn that finished normally only stops the server, so a dev server the
  agent started on purpose keeps running. The fakes start their tools the same
  way; a fake whose child stayed in the provider's group passed against a Stop
  that reached no real tool.
- **A finished turn's process is left to exit.** Claude Code 2.1.322 takes
  about 1.3 s from a `result` to its exit: it answers the context question
  and runs the user's SessionEnd hooks first. `CLOSE_GRACE` is 2.5 s for
  that reason; at 750 ms every completed turn ended with a SIGTERM into
  those hooks. Stop keeps its own 750 ms (`STOP_GRACE`). And a process that
  left with status 0 did not fail because of what it printed on the way out:
  `Exit::reason` quotes stderr only for a non-zero status, or a stale hook in
  the user's settings explains a turn it had nothing to do with.
- **The tree is watched, not taken at Stop.** Taken at Stop, it missed every
  tool of a Claude that had already died, or been killed for an oversized
  line: with Claude gone, nothing in the table leads to a session it started.
  `process::Watched` records both providers' trees from spawn and re-reads them
  every 500 ms, at Claude's `task_started`, and at Codex command lifecycle
  notifications. Stop, a failed turn and Quit all end
  what it last saw joined with what the table shows now. A group or session is
  forgotten once empty, because a record that lives for hours would otherwise
  claim the next unrelated process to be handed its id.
- **Quit cannot await any of that.** GPUI waits a tenth of a second for quit
  handlers, so every transport registers its provider while it runs, and the
  quit handler calls `fx_agent::stop_all_now`, which ends every registered tree
  on the main thread.

Claude's background tasks (`Agent` or `Bash` with `run_in_background`) change
what "completion" means, and each rule here was a lost result first:

- **A `result` with a task running is not the end.** Claude reports the task's
  end as `task_notification`, then answers it as a turn of its own (`init`, text,
  a `result` whose `origin.kind` is `task-notification`). stdin stays open while
  a task runs *or an ended task's answer is still owed*, so the wait can be
  steered and the follow-up's permissions answered; it closes at the first
  `result` with nothing outstanding, before that line's events are delivered,
  and the turn then reads to the process's exit. A zero-turn acknowledgement
  after the prompt was answered counts as that answer. `BackgroundWait` in
  `session.rs` holds the turn's half of these rules.
- **Only an ending Claude tells the model is owed an answer.** A task that
  completed or failed is. One that was stopped (TaskStop, KillShell, a
  Monitor's timeout) is not: Claude marks a shell or monitor told as it kills
  it, and an agent the turn stopped is told inside that turn. Nor is one
  whose `task_started` says `owned_by_subagent`: its subagent is told. And one
  that ends while a turn still works is answered in that turn when a request
  follows it (`system` `status: requesting`, which Claude sends after gathering
  what is queued), and as a turn of its own only when none does. Each of those
  was once owed regardless, and held stdin open for a follow-up that never
  came: the turn spun until Stop and its finished answer was recorded as
  cancelled.
- **A task holds stdin from its patch to its notification.** `task_updated`
  with a terminal status comes a line before the `task_notification` that owes
  the answer. Read as the end of the wait, it closed stdin there, and the
  follow-up's permissions then failed inside Claude ("Stream closed") and a
  steer was refused. A stopped task's patch holds nothing.
- **A steer may be answered after the `result` it raced.** Once guidance has
  been written, a `result` reads to the process's exit rather than stopping it,
  or the answer to a steer reported as accepted is killed 750 ms later. Claude
  says when it still holds one: every `result` carries `queued_turn_count`,
  and stdin stays open until a later `result` reports zero. Closed at the
  first, the steer's answer ran with no way to ask for a permission.
- **Background tasks run until they end or Stop.** The CLI's
  `CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS` defaults to 10 minutes because a `-p`
  run has to end unattended; applied by default here, it stopped six working
  agents mid-edit. FlowX applies a ceiling only when the variable is set in the
  environment. Then it is measured from each idle `result` and again from each
  task's end, is off while a steer is being answered, and at the ceiling the
  turn completes with a note and the process group is stopped.
- **A resumed session's lost tasks are not this turn's.** Resuming a session
  whose tasks were killed opens with a `stopped` notification per task and an
  `init` plus a zero-turn `result` per batch, before the prompt is read. Those
  neither end the turn nor prove the prompt was read; only tasks started in this
  process are tracked.
- **Claude's history file holds bookkeeping FlowX never shows.** A task
  notification is a user record (`origin.kind` `task-notification`), and an
  interruption writes a `[Request interrupted by user…]` user record and a
  `<synthetic>` "No response requested." reply. The history reader drops all
  three. Kept, they split the turns around them, FlowX's copy of an
  interrupted or task-heavy thread was never found in its history, and the
  sidebar said "Thread sync needs attention" on every pass.
- **…and records a steer as something other than a user message.** A message
  sent while a turn runs is read into that turn after the current tool's
  result and recorded as an `attachment` of type `queued_command`. The reader
  keeps the user's own — `commandMode` `prompt`, not meta, origin `human` or
  none — as a user message, between the replies FlowX's live row sits between
  when the steer went in during a tool call. Read only as user records, a
  steered thread's history never caught up with FlowX's copy.

How a turn ends, around its input closing and Stop:

- **The input can close at a line that is not a `result`** (a task ending
  with nothing owed), so the exit path asks the transport whether it closed,
  not only `BackgroundWait`. Read only from `result()`, an answered turn
  ended as "exited before finishing" and its tree was killed.
- **Once the input has closed, an answer that cannot be written is Claude's
  to fail, not the turn's.**
- **Stop interrupts first and settles steers after**; a Codex steer sent
  before `turn/start` is acknowledged waits for the turn id rather than being
  refused. The account's limits are read beside the turn, never before it.
- **A stopped or failed turn ends its unanswered calls** with an error result
  (`end_unanswered_calls`), or a settled group drew them with a green check.
  Native sync compares prose only, so these rows never stop a history
  matching.
- **A `<synthetic>` reply's usage is not context**: an API error's zero usage
  emptied the meter, live and restored.

Read-only command checks for both providers go through one classifier,
`fx-agent::shell`, which fails closed on control operators, substitutions and
the write flags of otherwise read-only tools (`find -delete`, `sort -o`,
`git diff --output`).
**A refusal of a Claude tool call always says why.** Its message becomes the
call's error result, and Claude Code 2.1.325 sends an empty one to the API as
it is; the API refuses it (`content cannot be empty if is_error is true`) and
the turn fails at the first refused call. Every Deny the reader clicked, and
every refusal by policy, ended the turn that way while the denial carried
`""`. The reader's Deny says the user declined; a policy's refusal carries the
policy's reason (`ClaudeTransport::deny`). Both fakes now answer an empty
denial the way the API does.
Claude's `AskUserQuestion` answers are an object keyed by the original question
text, with string values. An array of question/answer pairs fails the CLI's
`updatedInput` validation; the fake CLI validates that wire shape too.
`LineFramer` remembers how far it has scanned. Without that, a line arriving in
many chunks is rescanned from the front each time — quadratic, which for a 64 MB
message is billions of comparisons. The `scan_offset` field in the Swift original
existed for the same reason, and its test asserts a time bound rather than only
correctness.

A Codex request names its thread and turn, and one that is not the running
turn's is refused — unless a sub-agent of this thread sent it. A sub-agent is a
thread of its own, so its approvals carry its own ids; refused as foreign, every
command it asked to run failed. Codex names each child as it starts it (a
`subAgentActivity` item's `agentThreadId`, a collab call's `receiverThreadIds`,
`thread/started`'s `parentThreadId`), and `SpawnedThreads` in `codex/turn.rs`
accepts only a thread named that way; nothing is guessed. A request that is
refused still gets a row: the call it asked about, with the refusal as its
result, because its own item never reaches this transcript and a silent
refusal failed the call with nothing on screen to say why. That holds for a
sub-agent's MCP approval, which arrives as an elicitation, too. A call FlowX
refused has one result: Codex still completes its item as `declined`, and
read as well, every refused command showed "Tool failed" under the refusal.

### FlowX's tools

A thread can be given FlowX's own tools (`AgentConfiguration::flowx_tools`,
off unless chosen, from the access menu): `list_threads` lists the asking
thread's project and `read_thread` reads one of its threads' latest prose,
loaded or from its saved transcript. They are an MCP server, `flowx
--mcp-server`, which `main` checks for before anything else starts, since
stdout is the MCP channel in that mode. The server forwards each call over a
Unix socket to the running app, which answers on the UI thread from app
state; a thread nothing has loaded is read from disk off it. Checked live
against Claude Code 2.1.325 and codex-cli 0.167 at Read Only: both found the
tools, read a sibling thread and answered from it, with no card shown.

- **Each provider is told its own way.** Claude takes
  `--mcp-config=<json>`, with `=` because the flag takes a list, and never
  `--strict-mcp-config`, which would drop the user's own servers;
  `--restricted` still loads it. Codex takes `config` on both `thread/start`
  and `thread/resume` (FlowX opens a fresh app-server per turn), as one dotted
  key per server, `mcp_servers.flowx`: the whole `mcp_servers` table would be
  a claim on every server the user configured.
- **A call proves which thread asks.** The socket is in a fresh `0700`
  directory under the temporary directory (a socket path must fit in 104
  bytes, which app support does not), is `0600`, and is removed on quit. A
  call is accepted only from the same user and only with the HMAC of the
  thread it names under a secret made fresh each launch, so one thread's
  token cannot speak for another and a relaunch retires them all. The token
  is in Claude's argv, which the same user can read; that user can read the
  transcripts on disk anyway.
- **The opt-in and the project are checked on every call**, not at launch:
  a thread whose tools were turned off is refused although its server still
  runs, and `read_thread` refuses a thread outside the asker's project.
- **The tools' names are in one place**, `fx_core::policy::flowx_tools`,
  which both approval policies read: only those exact names are reads, so
  Read Only answers them without a card and a lookalike from another server
  is not one. They are reads only in a turn FlowX launched with its own
  server (`mcp::FlowxServer`), because a user's own server named `flowx`
  answers to the same names. A Codex approval whose message names no tool
  is `mcp__<server>__`, never a name a server could compose into one of
  FlowX's.
- **The server notices a socket when it starts, not at its first tick**:
  one removed before that tick was never seen, and the server never exited.

### What a provider can ask the user

Every request either provider can block on, checked against the real CLIs
(Claude Code 2.1.283's binary and a live stream-json run; `codex app-server
generate-ts` and a live run of codex-cli 0.164, both against a probe MCP server
that elicits). Each is answered in the UI, or answered on purpose for the reason
given.

| Provider | Request | What FlowX does |
|---|---|---|
| Claude | `can_use_tool` (any tool, MCP tools `mcp__server__tool` included, `ExitPlanMode`, a sandbox's network ask) | Approval card. Full Access and a non-`Ask` policy answer it. |
| Claude | `can_use_tool` for `AskUserQuestion` | Question dialog. Never answered by policy. |
| Claude | `elicitation` (an MCP server's form, confirmation or link) | Question dialog. Never answered by policy. |
| Claude | `control_cancel_request` (a request withdrawn: its call aborted, or another client answered) | The card or dialog is removed and nothing is sent; Claude reads no answer to it. An id FlowX does not hold is ignored. |
| Claude | `request_user_dialog` | Refused. Sent only for `dialog_kind`s the host declared in `initialize.supportedDialogKinds`; FlowX sends no `initialize`, so the CLI falls back to its no-dialog path. |
| Claude | `hook_callback`, `mcp_message` | Refused. Sent only for hooks and SDK MCP servers a host registers at `initialize`; FlowX registers none. |
| Claude | `oauth_token_refresh`, `host_auth_token_refresh`, `remote_control_work_secret`, device-hook requests | Refused. Host-managed auth and remote control, which FlowX does not enable. |
| Codex | `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval`, legacy `execCommandApproval` / `applyPatchApproval` | Approval card, or the approval policy. |
| Codex | `item/tool/requestUserInput` | Question dialog. |
| Codex | `mcpServer/elicitation/request` with `_meta.codex_approval_kind` | Approval card for an MCP tool call (`mcp__server__tool`), or the approval policy. |
| Codex | any other `mcpServer/elicitation/request` | Question dialog. With no reader, `{action: cancel}`. |
| Codex | `currentTime/read` | Answered with the clock. |
| Codex | `item/tool/call` | Refused. Dynamic tools only; FlowX registers none on `thread/start`. |
| Codex | `account/chatgptAuthTokens/refresh`, `attestation/generate` | Refused. Externally managed auth, and `initialize.capabilities.requestAttestation`, neither of which FlowX uses. |

Three of those rows were bugs found by that live run:

- **An MCP elicitation was refused.** Claude turns the refusal into
  `{action: cancel}` and Codex into a failed request, so the server's tool
  failed and nothing on screen said it had asked. Both providers relay the same
  MCP request, and `fx-agent::elicitation` is the one reader for both: a form
  becomes one question per field (a yes-or-no and a single choice are chosen,
  not typed; a field outside `required` may be left blank), a form with no
  fields is a confirmation, and `url` mode is a link. The answer goes back typed
  as the schema says, and a value that does not parse is sent as written,
  because a reply that never leaves keeps the server waiting.
- **Codex asks for MCP tool approvals as elicitations.** Under any access level
  that asks, every MCP tool call opens with `mcpServer/elicitation/request`, an
  empty form and `_meta.codex_approval_kind: "mcp_tool_call"`. Refused as an
  unknown method, every MCP tool call failed. The tool's name is only in the
  message (`…run tool "ask_name"?`).
- **The reply shapes differ.** Claude's schema takes `content` absent but never
  null; Codex names `content` and `_meta`, null when empty.

Opening a link is consent, so the dialog opens it only when the reader accepts,
in their own browser, and only `http`/`https`: a `file:` or app-scheme URL from
a server is never offered.

A thread FlowX only mirrors is a different case: the request belongs to the
process that asked, and only that process can hear the answer. FlowX shows that
the thread is waiting and does not offer buttons that reach nobody:

- **Claude.** Every Claude process keeps `<config>/sessions/<pid>.json`
  current, and while one is blocked it reads `status: "waiting"` with
  `waitingFor` (`"permission prompt"`, `"input needed"`, `"dialog open"`). That
  is the record `claude agents --json` reports. The Claude index pass reads it
  for every listed thread and ignores a record whose process is gone, since a
  crash leaves its file behind.
- **Codex.** `thread/list` reports `status: {type: active, activeFlags:
  [waitingOnApproval | waitingOnUserInput]}` only for a thread loaded in the
  app-server answering the listing. A thread running in another Codex process
  reads `notLoaded`, so FlowX cannot see that one wait and says nothing rather
  than guessing.

The same record says `status: "busy"` while that process runs a turn, and
the thread then shows as running: the sidebar row and its space's header,
the title bar's dot, and a quiet line under the transcript whose rows grow
as the other app writes them. Without it a thread being worked on in the
Claude app read as idle here. It is never a turn FlowX runs, so Stop,
steering and the composer's busy state ignore it; waiting outranks it, and
a turn FlowX runs on the thread answers for itself. Codex cannot say this
(a thread another process runs reads `notLoaded`), so only Claude does.

The mark is live state, never saved. Each index pass replaces it for its
provider and project, a failed pass clears it, and it is reported for the
projects that keep polling: the active one every five seconds and every
expanded one every 30 (`BACKGROUND_INDEX_INTERVAL`). Polling only the active
project left a thread archived in Codex or Claude listed under every other
project until it was clicked. Read for the active project alone, a thread
in another expanded project never showed either. A Claude killed mid-turn
leaves its record saying busy, and macOS reuses pids, so a record older
than the process now holding its pid is not that process's
(`record_is_its_process`). While the mark holds, the transcript's last
group is live ("Working", its unanswered calls spinning), never a turn
FlowX runs. Coming back to the window
(`refresh_native_projects`) and expanding a project read it at once. A turn FlowX runs on the thread
answers for itself. The sidebar row shows Waiting, the title bar's dot turns to
Waiting for the active thread, and the transcript says where to answer.

### Live probes

The fakes are not the CLIs, so a change to how a turn runs is also checked
against the installed ones: a temporary `#[ignore]` test that drives
`TurnRunner::launch` with `RuntimeDiscovery::default()`, removed before the
commit. It runs with the user's own provider homes, because pointing
`CODEX_HOME` at a copy means copying the ChatGPT login, whose token refreshes
would then land in the copy. So everything a probe starts is the user's
history, and it cleans up after itself:

- **Codex lists every thread, whatever folder it ran in.** Six probe threads
  ("Run `sleep 8`, then reply red" / "Change of plan: green", "is 2027 prime")
  were in the user's Codex sidebar on every launch until they asked why. A
  probe runs in a temporary folder and ends with `./fx clean-probes`, which
  archives every unarchived thread FlowX started in one, through
  `thread/archive`, so Codex's archived list can bring it back. A thread open
  in the Codex app has a writer and is refused; the script lists it and exits
  non-zero. `--dry-run` only lists.
- **Claude files a session under the folder it ran in**, so a probe's
  transcripts are under that temporary folder's name in `~/.claude/projects`.
  The script leaves them, since removing them cannot be undone.

## The transcript

The transcript carries tool calls as first-class rows, not only as the activity
strip above it. Five rules, each of which is a bug that was written first:

- **`Done` commits the streaming buffer, never clears it.** Clearing discards the
  turn's last — often only — sentence, so the final answer never appears.
- **A tool call splits the prose.** The streamed text is committed, the call is
  appended, and a new segment starts. That is the whole interleaving mechanism:
  "let me check" / call / result / "found it" is four rows, not one row with the
  call buried in it.
- **A successful result saying nothing is not recorded.** A tool that returned
  empty, or the literal `Completed`, would otherwise add a row per call and bury
  the prose. Errors are always recorded.
- **`claim_tool_id` must not gate the append.** The `ToolUse` and the matching
  `ToolResult` carry the *same* id, so a dedup gate on it rejects every legitimate
  first result. It tracks completion for the spinner; that is all — so only
  the result claims it. Claiming the call as it started made every running call
  read as finished.
- **A steer lands where the provider reads it.** Both providers accept a steer
  at once and read it at their next request, after whatever they were saying
  or running, which is where their histories record it. So the row is held
  (`hold_steer`), drawn after the live reply, and placed when the provider
  names the steer as read: Claude's `command_lifecycle` frame with state
  `started`, sent only for a message that carries a `uuid`, and Codex's
  `userMessage` item, whose `clientId` echoes the steer's
  `clientUserMessageId`. Both were captured from the real CLIs (Claude Code
  2.1.283, codex-cli 0.164). Placed on acceptance, a steer sent mid-sentence
  split the sentence around itself and the history never matched that thread
  again. Placed at the next `requesting` frame instead, a steer written just
  as a request went out sat a step early. That frame, and a steer item that
  comes back without an id, are the fallback for a CLI that names nothing; a
  steer nothing names lands when the turn ends. An id read before its steer
  is held (Codex's record can overtake its reply to `turn/steer`) places the
  steer as soon as it is.

Three more, for what a turn shows of itself while it works:

- **A thought is a block of its own, and never text.** `MessageContent::
  Thinking` is what both providers' reasoning becomes; `as_text()` does not
  read it, so search, previews and native sync (`native_contains_local_tail`,
  `carry_message_ids`) never see it, and the grouping policy counts it as
  activity. The conversation streams it apart from the reply
  (`streaming_thinking`), commits it as a message when the provider ends it,
  and commits it anyway before the reply's first delta, a call or `Done`, so a
  provider that names no end still reads right. The first thinking delta is a
  step; the first text delta commits the thought but keeps its token estimate,
  which Claude reports before the first word.
- **Thoughts sit among their calls.** An activity group places each recorded
  thought before the call that followed it (`Thought::position`), dimmed
  (`THOUGHT_OPACITY`) rather than recoloured, since Markdown draws in `fg`. A
  group of nothing but thinking is "Thought", not "Worked". The live thought
  is drawn in the tail while nothing is recorded and inside the live group
  afterwards; closed, the group's live step shows the thought's latest line,
  or its size in tokens where the text is withheld. `RowStamp::Activity::live`
  carries the streaming revision only while live text is drawn inside a
  group, so a tick still re-measures nothing.
- **A call's row opens to what it is.** An edit tool's row opens to the
  change it described (`fx_core::policy::EditPreview`: Claude's `Edit`
  old/new as a line diff, `Write` as additions, `MultiEdit` a section per
  edit, Codex's `fileChange` from its unified diffs, and a recorded edit's
  `_flowxStructuredPatch` over its intent), drawn with the inspector's
  `line_palette`; any other call's to its arguments; both to the output once
  it has one. While a call runs, what it has printed so far is under its row,
  the last few lines closed and more open, and the live step's detail ends
  with the latest line, so a closed group still shows a build going by. The
  buffer behind it is bounded in calls and bytes (`MAX_LIVE_OUTPUTS`,
  `MAX_LIVE_OUTPUT_BYTES`) and dropped at the result. A row's detail is read
  from the arguments as they are: `partial_json_string` finds a path in JSON
  that has not finished arriving. Every panel a row draws is in
  `group_paragraphs` too, in draw order, so an off-screen selection copies
  what the row would have shown.
- **A compaction is a row of its own.** `MessageContent::Compaction` is a
  System message between the work either side, and the grouping policy splits
  the groups there: the model that did the second never saw the first, only
  the summary. The row is a rule across the column with a caption ("Context
  compacted automatically · 610k → 22k tokens") and, where the provider gave
  one, a disclosure to the summary the model read, dimmed as a thought is. It
  is not text: `as_text()` does not read it, so search, sync, previews and
  copy never see it, and `message_paragraphs` reads the caption and the open
  summary so an off-screen selection copies what the row shows.

Four more, for what stays put while a turn runs:

- **A disclosure key nests under its group's** (`{group}:{pos}:call`), so
  pruning keeps any key a live group's key prefixes at a colon; keeping group
  keys only closed an opened call row on the next message.
- **A landed steer splits the transcript like a prompt but does not end the
  turn**: the plan looks past `landed_steers`, so it stays live and is
  updated in place rather than duplicated.
- **What an open row derives is cached by its call's arguments** (the edit
  preview), never rebuilt per frame; folding is one sweep, and a `Write`
  builds only the lines it shows.
- **Live output reads like a terminal**: each line is what follows its last
  `\r`. `carry_message_ids` keys thoughts by their text and compactions by
  their counts, so a group a thought heads keeps its key through a sync.

Tool output is read from `aggregatedOutput` first, then `output`, `result`,
`error`, `contentItems`, `changes`, `results`, `agentsStates` — that order is the
server's, and live turns and restored history share one reader
(`codex::session::tool_outcome`). Three of those are envelopes and are read
for their text: an MCP call's `result` is the MCP result (`{content,
structuredContent, _meta}`), its `error` is `{message}`, and a dynamic
call's `contentItems` are blocks. Shown as JSON, every Codex MCP row was the
envelope around its answer. A function call's `output` may be a list of
`input_*` blocks, read the same way; encrypted content is never shown. An MCP call is named `mcp__<server>__<tool>`, as
Claude names one, so both read `server · tool`. Reading only `output`/`result` yields an empty
result for every real tool call, which then looks trivial and is dropped, so the
transcript silently loses every tool output. An explicit `null` is not output:
Codex sends `"error": null` on every successful MCP call.

Which items become rows at all is one decision too, `codex::session::item_kind`,
shared the same way. History once kept a list of its own and wrote every item
the live turn skipped — sub-agent activity, hook prompts, review markers — as a
System row of raw JSON, and each compaction as a row the live turn never had, so
a thread that used sub-agents came back after a restart as hundreds of blobs.
Plan mode's proposed plan is an item of its own, and both paths show it as
prose.

Whether a row is prose or a collapsed activity is a *view* decision, made by
`TranscriptActivityGroupingPolicy` in `fx-core`. It reads content only, never
role; the caller excludes user messages before asking it to segment.

A running turn shows its progress in three places, all read from the
conversation rather than kept by the view:

- **The live group's step.** Under the header, open or closed: the latest
  unanswered call with its elapsed time, or "Thinking…" / "Writing…" between
  calls. A call starts at its message's timestamp; the turn and the stretches
  between calls start at `turn_started_at` and `step_started_at`.
- **One tick a second**, from the executor's timer, aligned to the turn's own
  seconds. A tick changes no row's stamp and the clocks sit on fixed-height
  lines, so it re-measures nothing.
- **The plan.** `fx-core::policy::TurnPlan` reads Claude's `TodoWrite` and
  Codex's `update_plan` alike. codex-cli 0.167 offers `update_plan` only
  when its config asks, so every Codex thread is opened with
  `tools.update_plan` `{enabled: true}` and its developer instructions end
  with `codex::session::PLAN_INSTRUCTION`; without them no Codex turn
  showed a plan. Codex reports the plan only as
  `turn/plan/updated`, so the conversation records it as the `update_plan`
  call it stands for, and one checklist card per turn follows the turn's last
  burst of work. Plan calls are not rows or counted actions. Claude Code
  2.1.325 offers newer models no task list at all unless asked, so every
  Claude launch sets `CLAUDE_CODE_ENABLE_TODO_TOOLS=1` and
  `CLAUDE_CODE_ENABLE_TASKS=0` (`PLAN_TOOL_ENVIRONMENT`, unless the user set
  either): the first offers the list, the second makes it `TodoWrite`, the
  whole list per call, rather than the `Task*` tools, which change one task
  by an id only their results carry. Checked live: with them `TodoWrite` is
  offered, deferred; without them it is not. Offered is not used: deferred,
  it was never loaded for a five-step task, so every Claude turn's appended
  system prompt ends with `PLAN_INSTRUCTION`, after the thread's own
  instructions, in one `--append-system-prompt`. With it, the same task kept
  a list updated after each step. Read Only keeps `TodoWrite`: it writes
  only Claude's own list. Claude records a conversation's system prompt at
  its first request and replays it on every resume, whatever a later launch
  passes, so every turn passes `--system-prompt-snapshot=off`: live, a
  thread's instructions edited from ALPHA to BETA were answered as ALPHA
  until then. An unchanged prompt still caches. The Claude desktop app keeps a
  session's plan with its own `mcp__ccd_turn__update_plan`, mostly steps on
  from the last list (`advance`, `done`/`start`, 1-based); the history
  reader folds those into the whole list each left
  (`TurnPlan::apply_desktop_update`), so a thread run there shows its plan.

## Two queues, and why they are bounded

Both transports cap their channels rather than letting them grow. The reason is
the same on each side: a session outlives its turn, and nothing drains the queue
between turns, so an unbounded one grows for as long as a thread sits idle.

When the queue fills, the reader stops reading and the server's pipe fills, so it
waits. That is deliberate: a stall is a truthful signal that FlowX is not
keeping up, where unbounded growth is silent and shows up later as an
out-of-memory.

One trap worth writing down, because it costs an hour: **`mpsc::Sender::send` on
a *bounded* channel is a future.** Converting `unbounded_channel` to `channel`
leaves every `let _ = sender.send(..)` compiling — and silently delivering
nothing. Both `send` sites were found that way, by tests that had passed all
along.

Claude's stream reader is exclusive by construction: `reader()` moves the
receiver out, so there is exactly one consumer. Answering an approval stays on
the transport, because that genuinely happens from another task while the stream
is being read.

## The shell

`fx-app` is three columns — sidebar, centre, inspector — and it has exactly two
app-wide owners, both held by `AppState`:

- **`MenuLayer`** holds at most one open menu. Drawing it is a separate concern
  from owning it: `menu_layer_overlay` is called once at the root, which is why a
  context menu opened from a row appears above the same content as a dropdown
  opened from a toolbar button, and why neither can be clipped by the column it
  came from.
- **`ContextRegistry`** holds the regions that offer a context menu. The
  right-click handler is attached to the **root**, not per region, so
  right-clicking the background does nothing instead of opening an empty panel.

The deepest registered region is authoritative. A region with no menu blocks its
ancestors rather than falling through, because falling through would offer an
ancestor's actions for a target the ancestor does not own.

`AppHandle` is a `gpui::Global` wrapping the state entity. A menu row's callback
receives only a `Window` and an `App`, so the state has to be reachable without
being captured.

### Per-frame work

No view is cached, so every notify, caret blink and streamed delta rebuilds
the whole window. Anything a frame does once per thread is paid once per
thread per frame, and the store is tested at 2,000 threads. The sidebar used
to copy the whole snapshot, scan every thread for every row, and under a
search lock and fold every loaded transcript, on every frame. Three rules keep
that work proportional to what is on screen:

- **Borrow the snapshot, never clone it, and ask the index.**
  `AppState::snapshot` is a `Versioned<PersistedApp>`: every `&mut` access goes
  through `DerefMut`, which moves its generation. `snapshot_index()` rebuilds
  the per-project visible lists and the id → thread / project / binding maps
  only when that generation has moved. `native_defaults`,
  `native_binding` and `project_for_agent` answer from the index when it is
  current and scan otherwise, so a loop that edits and asks on each pass never
  rebuilds it. A plain repaint must not borrow the snapshot mutably at all.
  `a_repaint_does_no_work_that_grows_with_the_threads` checks that across the
  whole shell; a mutable borrow taken on every frame would rebuild the index
  every frame.
- **A sync pass costs the project it is about, once.** Native-sync passes
  land every couple of seconds on the UI thread. What a pass changed is told
  by the snapshot's generation, not by copying the workspace before and
  after; an index pass copies its own project's record, reads everything it
  looks up per thread into maps first (`index_bindings`), and encodes the
  workspace only when that record changed in memory. Scanned per thread, a
  pass was quadratic in the project's threads.
- **A search is remembered, not rerun.** The sidebar folds each thread's text
  once, re-reads only threads whose loaded transcript has moved, and
  re-matches only when the query, the snapshot's generation or a folded text
  changes.
- **Long lists of uniform rows are `uniform_list`s.** The palette's results
  and the commit composer's file list build only the rows in view. Each row
  carries the list's gap as a bottom inset so that every row is one height,
  and the list gives the last one back.

Every key binding, the menu bar, and Settings' Shortcuts page come from one
table, `commands::key_bindings()`. Add a shortcut there, never with a separate
`bind_keys` call, or the page and the menus stop telling the truth. A menu item
is enabled exactly when something on the focused path handles its action, so
commands that need a project or an idle thread register their handlers only
then.

### Text input

GPUI 0.2.2 ships no text-input element. `crates/fx-app/src/text_field.rs` is
an `Entity<TextField>` implementing `gpui::platform::InputHandler`, registered
during paint through `Window::handle_input`. Of that trait's methods only
`apple_press_and_hold_enabled` has a default; the other seven are required.

Two decisions there are load-bearing:

- **Printable characters are not claimed in `on_key_down`.** They arrive through
  `NSInputContext` → `replace_text_in_range`, which is what makes dead keys and
  IME composition work. Claiming them would insert the character twice.
- **`Return` inserts a newline in a multi-line field and only submits in a
  single-line one.** Cmd/Ctrl-Return submits in both, and is claimed explicitly
  because macOS reports `key_char: Some("\n")` for it.
- **IME composition is a marked range, not committed text.** While one is
  marked, GPUI routes keys to the input context first, so Return confirms the
  composition rather than submitting. Text with no replacement range replaces
  the marked range. Undo groups never split a composition.

Five more, each of which was a bug first:

- **A character is a grapheme cluster.** Arrows, Backspace, Delete, word
  movement and the column a vertical move keeps all step by
  `unicode-segmentation` clusters. By code point they stopped inside a flag, a
  skin tone, a ZWJ family or a letter with a combining accent, and deleting
  left half of one behind. Offsets are still UTF-8 bytes inside the field and
  UTF-16 at the `InputHandler`; only the stepping changed.
- **Line breaks are normalised in `edit`.** GPUI shapes lines only at `\n`, so a
  kept `\r` is an invisible character that costs a keypress and a Backspace
  with no visible effect. A multi-line field turns `\r\n` and `\r` into `\n`; a
  single-line one turns each break inside the text into a space and drops breaks
  at either end, so a path copied with its newline still opens.
- **A paste the cap cuts short says so.** `edit` returns how many characters
  `max_len` left out, `paste` emits `TextFieldEvent::PasteTruncated`, and the
  composer's draft shows it as its notice. Typing at the cap stays quiet.
- **Hit-testing adds back the scroll that was painted.** Paint subtracts the
  scroll, so `index_at` adds back the offset prepaint recorded in
  `FieldGeometry::painted_scroll` — not the scroll of the layout it rebuilds
  for the click, which follows the caret as it is now rather than as drawn. The
  caret row scrolls into view by its bottom, not its top.
- **GPUI's wrapped hit-test adds the row's start column itself.**
  `WrappedLine::closest_index_for_position` takes x relative to the visual row;
  adding the column too put every click on a later row at its end. It also
  truncates `y / line_height`, so ask for a row at its middle.

Logical lines cache their vertical origins when rows are built. Painting looks
up those origins directly and visits only lines and selection rectangles that
intersect the viewport. Scanning preceding rows for every painted line made a
large multiline draft quadratic, even when almost all of it was clipped.

### @-mentions

Typing `@` in the composer opens a picker over the files under the thread's
execution root (`AppState::project_root_for`); a row is taken with Return,
Tab or a click, and lands in the draft as `@path ` in one undoable step
(`TextField::replace_range`). The pure halves are in `fx-core`:
`policy::mention` finds the token at the caret (`mention_at`) and ranks paths
for it (`rank`), and `services::file_listing` lists a folder. What the two
CLIs do with a mention was checked live, not read from documentation, and
each rule below came from that run:

- **Claude reads `@path` out of the text itself.** In print mode and through
  stream-json user messages alike, Claude Code 2.1.283 expanded `@AGENTS.md`
  before the model saw the prompt: it answered the file's first heading with
  no tool call. So the draft is the whole message, and FlowX adds nothing.
- **Codex does not.** Given the same text, codex-cli 0.164 ran `head` on the
  file. Given a `{type: "mention", name, path}` input item beside the text, it
  answered from the file with no command. `turn_input` in `codex/session.rs`
  adds one per `@path` that names a file, relative to the thread's `cwd` or
  absolute, with trailing punctuation dropped (`see @src/lib.rs,`); the text
  keeps its `@path`. Codex records the item in the `userMessage`'s content,
  and the history reader turns one back into `@path` only where the text does
  not already name it — a prompt from the Codex app — so FlowX's own prompts
  still match their history.
- **Neither reads a mention past whitespace**, so a path with a space in it
  cannot be mentioned and is not offered. `@` after a letter is an address or
  a scoped package, not a mention; after whitespace or an opening bracket it
  is one.

The listing is `git ls-files --cached --others --exclude-standard` minus the
deleted files, so the repository's ignore rules apply, and for a folder that
is not a checkout a walk bounded in depth with the Files panel's skip list.
It runs on the background executor, is kept per root for `INDEX_TTL` (a turn
writes files for as long as it runs) and is capped at `MAX_INDEXED_FILES`,
past which the popup says the list is a prefix. A stale listing keeps serving
until its replacement lands, so the picker never empties while a root is
re-read. Codex's `fuzzyFileSearch` is not used: the app-server only lives
during a turn, and a picker that worked only while Codex was busy would be
two pickers.

The popup is drawn by the composer, never by a layer of its own:
`deferred(anchored().anchor(Corner::BottomLeft))` as a child of a wrapper
around the card, so it grows upward over the transcript, paints above the
rows, and stays inside the window with the menus' margin. Its rows are a
`MenuModel`, so pointer and keyboard share one highlight and movement wraps
as in every menu. Its keys are caught by the composer's `capture_key_down`
on the way down to the field, only while the picker is open under a focused
draft and only unmodified: the arrows and Return are the field's again as
soon as there is no row to take, and ⌘↩ still sends. Escape closes the
picker for that token — the state remembers the dismissed token, because the
next repaint would otherwise reopen it — and typing on opens it again.

The ranking lowercases each path once, when the listing is built, and sorts
only the rows it shows (`MentionPaths::rank`); sorting every match per
keystroke cost 30–58 ms at 50,000 paths. A completion that would cross the
composer's cap is refused whole, never cut. A slash command's text comes back
to an empty composer when the command fails, and a queued prompt that cannot
start joins the draft after a blank line while the queue moves on.

One GPUI detail that bit the tests: blur events fire only in an active
window, so a focus test calls `activate_window()` and `run_until_parked()`.
Another: an observer registered in the same
update as the first `notify` it should see misses it. `cx.observe` activates
through a deferred effect, and `notify` is deduplicated while one is pending,
so a field created and typed into in one update merges both notifies ahead
of the activation. Create the field in one update and type in the next.

### Running a turn

The runner owns a `CodexSession` for a turn's lifetime and writes events into
the conversation from a background thread, so the conversation is behind a
`Mutex` — the render path holds it only for the read, never across an await.

The runner's callback **may not capture `App`**: it fires on another thread and
`App` is neither `Send` nor reachable from there. It sends to an `mpsc` channel
instead, and rings a wake the root awaits on the foreground executor, so an
update is drained and drawn on the frame it arrives. Waiting for a poll
instead put every streamed delta up to a tick late, which is what made
streaming look like it came in steps. Wake-driven drains are spaced one
display frame apart, so a stream that writes faster than that is drawn in
bulk. A 33 ms poller still runs while anything is busy: it notices a
revision that moved without an update, runs the checkpoint and the queues,
and is what a test clock drives. Both go through one `drain_and_repaint`,
which repaints only when the drain applied something or a running thread's
revision moved, so a quiet turn costs no frames, and a finished turn's
`Finished` update is what repaints it without anything asking. What the last
pass saw is kept on the root, so a wake and the poll after it draw a change
once.

The window is opaque. Nothing in it is translucent, and a transparent
`NSWindow` is composited against whatever is behind it on every frame, so
`WindowBackgroundAppearance::Transparent` cost a blend per frame for nothing.

Two things stay off the UI thread. Transcripts are saved by one writer thread
(`conversation_writer.rs`): at most one waiting save per thread, a newer save
replaces it, and an unchanged file is not rewritten. Transcripts are read in
the background on first view (`request_conversation`); a render never reads
one, and the centre column shows a loading state until it lands. Tests that
read the store right after a save call `flush_saves()` first.

`DenyUnlessReadOnly` only bites when the thread is opened at a level that asks:
`AgentAccess::FullAccess` maps to `("never", "danger-full-access")`, so Codex
never asks and FlowX never gets to refuse. The access level has to be
`ReadOnly`, `Supervised` or `AcceptEdits` for the policy to mean anything.

Read Only is that policy as an access level, and `TurnRunner::launch` applies
it whatever policy the caller passed: a read goes ahead, anything else is
refused with the refusal as the call's result, and no approval card is ever
shown. Questions still reach the reader. Each provider is also held back on
its own side, because FlowX only hears about what a provider asks:

- **Codex runs `("on-request", "read-only")`**, its own read-only pairing:
  anything may run inside a sandbox that cannot write, and only a command
  that asks to leave it reaches FlowX. Paired with `untrusted`, Codex asked
  about every command off its short safe list and FlowX refused reads such as
  `nl -ba file`; a live review spent four calls on refusals. A write inside
  the sandbox fails with `Operation not permitted`, and codex-cli 0.167
  reports no item for that command at all.
- **Claude runs `manual --restricted`, with `--tools` naming a reader's set
  and the edit tools denied.** FlowX hears only what Claude asks, and an
  allow rule in any settings file runs a command without asking: live
  against 2.1.325, a checkout's `.claude/settings.local.json` allowing
  `Bash(touch:*)` created the file under Read Only, and a reviewed diff can
  add that file. `--restricted` reads no user, project or local settings, so
  no rule or settings hook applies; it also drops Bash, which `--tools` names
  back (`READ_ONLY_TOOLS`; `default,Bash` leaves Bash alone). WebFetch stays
  out, because a fetch can carry away what was read. Lists are written with
  `=`, because the flags take lists. 2.1.325 has no `MultiEdit` and warns
  about a rule naming it. Every tool the list offers that Claude asks about
  passes the read-only classifier (WebSearch, ToolSearch and Agent among
  them), and a test checks the two agree: offered and then refused, every
  search failed. A sub-agent's own calls are still asked of FlowX.
- **Only Read Only shuts out the checkout's own Claude config.** The other
  levels read user, project and local settings and the checkout's
  `.claude/agents`, as the CLI does in that folder: live, an agent file
  there with `permissionMode: acceptEdits` ran `touch` under Supervised
  with no card. Under `--restricted` the agent was not offered.
- **`GIT_OPTIONAL_LOCKS=0` is in the environment** (`launch_environment`),
  so a reviewer's `git status` refreshes the index in memory instead of
  writing it beside the author's own `git add`. Porcelain `git diff` ignores
  the variable; Codex's sandbox stops that write, and Claude has none, so a
  Claude review run during the author's commit can still collide with it.

A Codex thread recorded with a read-only sandbox reads as Supervised, not
Read Only: Codex asks before a command leaves the sandbox and the reader may
approve, where Read Only refuses. A saved level this build cannot read is
Supervised too: it may be a level that only asked.

### Scrolling

Conversation Find preserves the selected message id when history refreshes,
including navigation made while the search is pending. Only a changed query
starts at the first match; an unchanged match must not request another scroll.

GPUI 0.2.2's scrollable elements have **no keyboard handling at all**. The
Swift original got arrows and page keys from `NSScrollView` for free, so the
transcript adds `.track_focus()` plus an `on_key_down` that pages the list —
claiming only the keys it moves, and stopping propagation for those alone.

The transcript is a `gpui::list` aligned to the bottom: only the rows in view
are built, laid out and painted, and every other row keeps the height it was
last measured at. With no scroll position of its own the list shows its last
rows and stays there as they grow, which is what following the bottom is. Four
rules, each of which was a bug first:

- **Splice every row that changed.** Each pass stamps the rows and splices only
  the stretch whose stamps moved, so a delta re-measures the tail and nothing
  else. A reader scrolled into a spliced row is put back at the same offset;
  GPUI otherwise moves them to the row's top on every delta.
- **A pinned list's position is "the end", a viewport below the screen.** A
  step taken from there barely moves, so paging starts from what is on screen.
- **A row keeps its id across a sync.** The provider's copy of a turn has ids
  of its own, so native sync carries each local message's id onto the matching
  message (`carry_message_ids`), and a splice that finds none of the reader's
  rows keeps their place counted from the stretch's end. A call is matched by
  its id wherever it is, because the history orders calls as they finished,
  or call and result in turn; a prompt of images alone, by its images.
  Taking the provider's
  ids sent a reader in the turn just finished to the top of the thread and
  closed every disclosure in it. The live reply is keyed per segment for the
  same reason: under one key, a selection in one reply came back over the
  next.
- **What is off screen is not layout.** Selection reads the rows it does not
  see from the messages, through `selection::Document`, which has to produce the
  keys and text the rows would draw.
  `the_document_reads_the_same_paragraphs_the_rows_draw` pins that.

The list's paint, like `track_scroll`, reaches `Window::current_view()`, which
unwraps a stack that only `View::paint` populates. That means **a transcript
cannot be painted through `VisualTestContext::draw`** — a bare element is not a
view. Mount a one-line `Render` probe and refresh instead; that is what
`paint()` in the `fx-app` tests does.

`Window::request_animation_frame()` is unsafe from a render function for the
same reason — it calls `current_view()` and asserts the draw phase. Use
`window.on_next_frame(…)`, and key it on the caret's blink phase rather than on
every pass, so a fast stream cannot turn it into a frame per delta.

### View tests

GPUI's test platform is behind the `test-support` feature, which is enabled from
`fx-app`'s and `fx-design`'s **dev-dependencies** rather than from the workspace. Adding it to the
workspace dependency would pull leak detection and a fake platform into the
shipped binary.

Each view test builds its own `TestAppContext` and its own `TempDir`. Sharing one
store directory across tests is a race: they run concurrently in one process and
overwrite each other's snapshot, which surfaces as a test that fails only in a
full run.

Test windows are not key, so motion in them runs at half the beat unless a
test activates the window, and an update and a refresh each draw a frame:
compare debug bounds taken from the same frame. `debug_bounds` takes only a
`&'static str`, so a selector built with `format!` is found by a literal.

`VisualTestContext::debug_bounds` proves something was drawn, never that it
went away: GPUI 0.2.2's `Frame::clear` does not clear the frame's debug
selectors, so a selector drawn once stays visible to it in later frames.
Assert a removal on the state that decides it.

## The inspector

The inspector shows the working tree's diff and is the one place FlowX writes
to a repository. `inspector.rs` is the shell; its child modules are the Git
layer (`git_inspector.rs`), the shared poller (`repository_monitor.rs`), the
diff viewer (`diff_view.rs`) and the commit composer (`commit_composer.rs`).
The pure halves live in `fx-core::git`: `diff` parses, `operations` plans
commits and pushes, `revision` hashes a snapshot.

Seven rules, each of which is a bug that was written first:

- **A read never writes the index.** Agents run `git add` and `git commit` in
  the same repositories, and a read holding `index.lock` fails theirs with
  "File exists". Status honours `GIT_OPTIONAL_LOCKS=0` by refreshing in memory
  only; porcelain `git diff` ignores it and rewrote the index on exit whenever
  a file was stat-dirty (`touch`, a formatter, an edit and its revert). So
  every diff is plumbing: `diff-files`, and `diff-index` against `HEAD` or, on
  an unborn branch, the empty tree. Plumbing reads none of porcelain's diff
  configuration, so two defaults are restated: `--ita-invisible-in-index`, and
  `-M` or `-C` exactly when status reported a rename or copy, so the diff pairs
  files as the list does. `diff-files --numstat` still lists a stat-dirty
  binary as `-\t-`, so counts are kept only for paths status reports changed
  on that side. Nothing FlowX runs refreshes the index, so a stat-dirty file
  is read again by each poll until the user's own git refreshes it.
- **One poller.** The title-bar count and the inspector both observe the one
  `RepositoryMonitor`, which reads every five seconds and notifies only when
  the read differs. Two timers ran `git status` and both numstat passes twice.
- **A snapshot has a real revision.** `content_revision` hashes the status and
  numstat text, every changed path's metadata (nanoseconds, inode, ctime) and
  the index. A constant revision made every poll look like a change.
- **Diff lines are classified by hunk state, never by prefix.** Inside a hunk
  the `@@` counts say how many lines follow, and every one is content: a
  removed `-- comment` arrives as `--- comment`, an added `++i` as `+++i`. A
  conflicted file's combined diff (`diff --cc`) has a marker column per
  parent, and each is stripped. `diff-index --cached` prints only
  `* Unmerged path <path>` for one, which opens a section of its own; read as
  metadata, it joined the previous file's section.
- **A refresh never blanks the diff.** The selection is a path, not a status
  record (its line counts change on every edit). The shown document stays
  until its replacement is parsed, which is then scrolled back to the same
  file and line. A failed load is not retried until something changes or
  Refresh is pressed, because the check runs on every render.
- **Untracked directories are not files.** A nested repository is reported as
  `?? vendor/lib/`; counting its lines always failed, which pinned "Line totals
  are partial" on forever.
- **A write outlives a project switch.** Writes in flight are held per project
  and report to the project they wrote: its draft is removed on success, and
  the banner waits until it is shown. Dropping the task on a switch cancelled
  it after git had already committed, so the message came back as a draft,
  with no banner, and Commit was enabled for a duplicate.

A successful commit clears only the exact draft submitted. The field remains
editable while Git runs, so edits made during a hook, including ones saved by a
project switch, must remain available after the older commit completes.

A chosen-files commit removes before it adds, because with macOS's
`core.ignorecase` the add kept HEAD's entry for a case-only rename and the
removal then deleted it. It adds with `--force`, so a force-added ignored file
still commits; a plain folder now at a chosen path is a deletion, not its new
files; and a chosen `D ` row is committed untracked, never re-added. A path
can be listed twice (`D ` and `??` after `git rm --cached`), so counts and
labels are per row and per side, never by path. Rename detection is per side:
Base pairs only the renames status reported. A changed submodule's HEAD is
part of the revision, and Refresh reads again at the same revision. Push is
offered only when git's own `%(push)` resolves.

Every Git job runs on one shared multi-thread runtime whose workers have an
8 MiB stack. Jobs are *built* there (`spawn_git` takes a closure), because
debug-build futures overflow a dispatch worker's small stack.

Writes follow `fx-core::git::operations`: paths after `--` under
`--literal-pathspecs`, no plan ever contains a force flag, and a failure shows
git's whole combined output. A chosen-files commit is built in a temporary index
beside the repository's own: chosen paths from the working tree, a rename's
source as removed, then the user's index is reset to the commit for those
paths only. `--only` took the source from the working tree too, so after
`git mv old new` and a fresh `old`, the unchosen file was committed. The
temporary index starts as a copy of the user's, its modification time
included, merged with `HEAD` by `read-tree -m -i`, so every entry `HEAD`
matches keeps its stat data. `read-tree HEAD` alone kept none, and the commit
then hashed every tracked file through its clean filter (LFS, git-crypt):
314 ms for 4,000 files where `commit -- path` takes 29. The copy keeps the
time because git rereads an entry only while it is as new as the index
holding it; stamped now, a same-size rewrite made in the second the index was
written passed as unchanged. `-i` keeps the working tree out of the merge,
which otherwise refuses any file staged and then edited again. Only a
rename has a source to remove: a copy (`status.renames=copies`) names one
too, and treating it as a rename committed the source as deleted. That commit
names no pathspec, so git's own "cannot do a partial commit during a merge"
never runs; FlowX refuses first, in git's words, while `MERGE_HEAD`,
`CHERRY_PICK_HEAD` or `REVERT_HEAD` resolves or any index entry is unmerged.
Unchecked, a merge became a two-parent commit of `HEAD` plus the chosen
files, and the other branch counted as merged with its changes dropped. Push
runs plain `git push` when `branch.<name>.merge` is set (it survives a gone
upstream, which is how an empty clone starts) and otherwise
`push --set-upstream -- <remote> HEAD`.

Open in Editor never hands an agent-written file to its default handler, which
for `.app` or `.command` means running it. Only a passive viewer type keeps its
viewer, only when the path itself is a regular file (a `shot.png` symlink to
`evil.command` is not), and the viewer is named with `open -b`, because a bare
`open` honours a per-file "Open With" attribute. Everything else goes to the
registered source-code editor, then `open -t`.

### Reviews

The Review button beside the inspector's change count starts a review of the
checkout's uncommitted changes by either provider, the one that did not write
them listed first (`review.rs`; the pure half is `fx_core::policy::review`).
Four rules:

- **A review is an ordinary thread.** It runs at Read Only in the reviewed
  thread's project and folder (its worktree binding included), with
  `review::BRIEF` as its system prompt so it holds for follow-ups, opened with
  `review::PROMPT`. `Agent::review_of` links it to the thread it reviews; a
  review started from a review reviews that thread again, and one whose
  thread was deleted reviews for nobody rather than for itself. It is named
  after what it reviews, and the name is recorded as a rename is: without the
  override, the first sync pass after its turn replaced it with the
  provider's title, the first prompt.
- **It is a prompt, not Codex's `review/start`.** Run against codex-cli 0.167,
  a native review streamed nothing of what it wrote: the whole review arrived
  in one `exitedReviewMode` item and one completed `agentMessage` at the end,
  with no deltas and no reasoning summary, under a second turn id
  (`rootTurnId`), and it left a thread of its own behind. A prompt streams like
  any turn and reads the same on both providers.
- **The findings are the last reply to the last prompt** (`review::findings`),
  and only of a turn that completed. A turn's earlier prose is the reviewer
  talking between calls; a stopped or failed turn's last words are exactly
  that, so the saved `execution_state` must be `Success`. The row asks
  `review_ready` on every frame, which reads the transcript from its end and
  builds nothing.
- **Nothing is sent for the reader.** The row above a review's composer adds
  the findings to the reviewed thread's draft (`append_prompt_context`, after
  whatever the reader had typed), shows that thread and focuses its composer.
  FlowX never sends a prompt on its own. The reviewed thread must be loaded
  first, or its saved draft is still on its way; a review shown after a
  relaunch is usually opened first, so showing one starts that read.

## Porting conventions

- Swift `FXColors.foo` becomes `Colors::foo(&theme)`.
- Swift `FXSpacing.sm` becomes `Spacing::SM`. Same for `Radii`, `BorderWidth`,
  `FXLayout` → `Layout`, `FXAnimation` → `FxAnimation`.
- Swift `FXTypography.body` becomes `Typography::style(TextRole::Body, &theme)`,
  applied with `.fx_body()`.
- Swift `FXShadow.popover` becomes `Shadow::popover(Colors::overlay(&theme))`.
- Pure logic ports almost verbatim, keeping its tests. These are the cheapest and
  highest-value parts of the port.
- `enum` cases become `PascalCase`; acronyms keep theirs (`CHEVRON_DOWN`,
  `is_dark`).
- Errors are `thiserror` enums in the crate that owns the failure, surfaced
  with `anyhow` only at the binary edge.

## Deferred to phase 2

- **Terminal.** SwiftTerm is a Ghostty fork. Phase 2 uses `libghostty-vt`, safe
  bindings over the same VT core, plus a GPUI renderer. `Xuanwo/gpui-ghostty`
  is a working reference. Requires Zig at build time.
- **Browser split.** Swift owned a real `WKWebView`; nothing in Rust embeds
  WebKit. Deferred pending a decision between an `objc2` bridge and dropping the
  embedded preview for an external browser.
