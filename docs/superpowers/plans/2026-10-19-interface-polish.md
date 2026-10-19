# Interface Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the reported visual bugs (sliders, window minimum, sidebar toggle, spacing, thin fields) and add the
touches that make Tidash feel like a modern app (hand cursor, links, roomy menus with icons, toasts, fades).

**Architecture:** Everything but the window minimum is UI code in `crates/app/src/ui` and `theme.rs`, built from
the house widgets in `ui/widgets.rs` and egui 0.36's own hooks (`interact_cursor`, `Popup::style`, opacity). The
playback source travels with `Action::PlayTracks` into the backend's `Queue` and back in `QueueSnapshot`. The
window minimum is a one-line fix in winit, carried in a fork through `[patch.crates-io]`.

**Tech Stack:** Rust 2024 (1.98), egui/eframe 0.36.2 from crmne/egui `6ce633b`, fastframe v0.13.0
(`fastframe-icons` for Lucide SVGs), winit 0.30 from crmne/winit `ed7caa9` (to be repointed in Task 12).

**Spec:** `docs/superpowers/specs/2026-10-19-interface-polish-design.md`. The design-system spec
(`docs/superpowers/specs/2026-10-10-design-system-design.md`) still governs everything this one does not change.

## Global Constraints

- No new crates. `unsafe_code = "deny"` stays.
- Every user-facing string goes through `tr()` and is added to `crates/app/po/tidash.pot` like the existing ones.
- Accessibility does not regress: every control keeps its AccessKit name and role, focus rings stay, keyboard paths
  stay. Track-list links take no Tab stop; player-bar links do.
- 0 repaints at rest: every new animation asks for repaints only while it runs (tests assert
  `repaint_delay == Duration::MAX` once settled, as `no_cover_no_tint` does).
- Durations: slider hover 0.12 s, cover fade 0.2 s, page fade 0.15 s, toast fade 0.15 s, toast shown 2.5 s.
- Sizes: control height `theme::PILL` (40); menu item 34 tall, 12 text inset, 16 icon, 12 icon gap, hover radius 8,
  menu margin 6, menu min width 200; separator 1 px `outline` with 5 px above and below; slider rail 4 at rest, 6
  hovered, thumb 12 across.
- Checks before every commit: `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked -p tidash` (the whole suite, not only the new test).
- Git in this worktree runs through the PowerShell tool.

## Review Focus

1. Right-clicking an artist or album name inside a track row still opens the row's menu, and dragging a playlist
   row by its artist name still reorders it. Test in Task 9.
2. A name cut with "…" in a narrow column: clicking its visible part opens that name; a name wholly cut off is no
   link. Test in Task 9.
3. One cover shown in several places at once (the player bar and rows of a playlist from one album) fades in once
   and ends fully opaque. Test in Task 11.
4. A disabled slider (exclusive mode's volume, the seek bar with nothing playing) hovered shows no thumb, no
   accent and the arrow cursor. Test in Task 2.
5. At the minimum width (960) with the side panel open, the top bar still fits the sidebar toggle, Back/Forward,
   the search pill and the gear, none overlapping. Test in Task 3.

---

### Task 1: Hand cursor, unselectable text

**Files:**
- Modify: `crates/app/src/theme.rs` (`style`)
- Modify: `crates/app/src/ui/widgets.rs` (`icon_button`, `circle_button`, `glyph_button`, `pill_button`, `nav_row`)
- Modify: `crates/app/src/ui/tracks.rs` (row response in `show`), `ui/queue_panel.rs` (row), `ui/card.rs`
  (`card`), `ui/library.rs` (`row`, the folder header), `ui/shelf.rs` ("View all"), `ui/lyrics.rs` (line rows),
  `ui/settings.rs` (checkboxes, `combo`)
- Test: `crates/app/src/ui/widgets.rs` (tests module)

**Interfaces:**
- Produces: every enabled clickable shows `egui::CursorIcon::PointingHand` on hover. Later tasks' new clickables
  call `.on_hover_cursor(egui::CursorIcon::PointingHand)` the same way; stock `egui::Button`s (menu items included)
  get it from `Visuals::interact_cursor`.

- [ ] **Step 1: Write the failing test** `clickables_show_the_hand` in `widgets.rs` tests: with
  `theme::apply(&ctx, &Palette::dark())`, draw one at a time and hover each (`PointerMoved` to its middle, second
  pass); read `out.platform_output.cursor_icon`.

```rust
assert_eq!(hovered(icon_button(.., "Shuffle", None)), CursorIcon::PointingHand);
assert_eq!(hovered(pill_button(.., "Play", true)), CursorIcon::PointingHand);
assert_eq!(hovered(nav_row(.., "Home", false, false)), CursorIcon::PointingHand);
assert_eq!(hovered(ui.button("Retry")), CursorIcon::PointingHand); // stock, via interact_cursor
assert_eq!(hovered(ui.add_enabled(false, egui::Button::new("Off"))), CursorIcon::Default);
assert_eq!(hovered(ui.label("Plain text")), CursorIcon::Default); // no I-beam
assert!(!ctx.global_style().interaction.selectable_labels);
```

- [ ] **Step 2: Run it and see it fail**
  Run: `cargo test --locked -p tidash clickables_show_the_hand`. Expected: FAIL (`Default` for the house widgets,
  `Text` for the label).
- [ ] **Step 3: Implement.** In `theme::style`: `v.interact_cursor = Some(egui::CursorIcon::PointingHand)` and
  `s.interaction.selectable_labels = false`. In each house widget and each listed site, chain
  `.on_hover_cursor(egui::CursorIcon::PointingHand)` on the clickable response (egui's `hovered()` is false for
  disabled widgets, so disabled ones keep the arrow; confirm with the test). Checkboxes and the combo box get the
  same in `settings.rs`.
- [ ] **Step 4: Run the test, then the whole suite.** Expected: PASS; no other test changes.
- [ ] **Step 5: Commit** `feat(ui): hand cursor on clickables, no selectable labels`.

---

### Task 2: Player-bar sliders: thumb on hover, one centre line

**Files:**
- Modify: `crates/app/src/ui/player_bar.rs` (`slider`, `transport`, `extras`)
- Modify: `crates/app/src/theme.rs` (constant)
- Test: `crates/app/src/ui/player_bar.rs` (tests module; replaces `slider_knobs_are_small`)

**Interfaces:**
- Produces: `pub const SLIDER_HOVER: f32 = 0.12;` in `theme.rs`. `slider(ui, p, value, range) -> Response` keeps
  its signature.

- [ ] **Step 1: Write the failing tests** (a `bar_at(ctx, model, width, time, events)` helper beside `bar` that
  also sets `RawInput::time`; the rail is the `Shape::Rect` filled `surface_active`, the times are the text shapes
  "0:00" and "3:20" of `testing::track(1)`):

```rust
#[test]
fn slider_thumb_only_on_hover() {
    // at rest, playing: no Shape::Circle anywhere in the bar
    // pointer over the seek rail's middle, passes at t = 0.0..=0.3: exactly one circle, radius 6.0 (±0.5),
    //   no stroke, centred on the rail's centre y (±0.5); the rail's rect is 6 tall (±0.5)
    // pointer moved away, t + 0.3: no circle again; repaint_delay == Duration::MAX
}

#[test]
fn times_sit_on_the_seek_rail() {
    // the seek rail's centre y == the "0:00" text's centre y == the "3:20" text's centre y (±0.5)
    // the volume rail's centre y == the volume icon's centre y (±0.5)
}

#[test]
fn a_disabled_slider_shows_no_thumb() {
    // model.settings.audio.exclusive = true (volume fixed): hover the volume rail for 0.3 s:
    // no circle, cursor_icon == CursorIcon::Default
    // no track (seek disabled): hover the seek rail: no circle
}
```

- [ ] **Step 2: Run them and see them fail** (`cargo test --locked -p tidash slider_thumb times_sit
  a_disabled_slider`): circles at rest, times about 6 pt off the rail.
- [ ] **Step 3: Implement.** Keep egui's `Slider` for input, keys and AccessKit, but let it paint nothing visible:
  in the scope set its height to the row's (`interact_size.y` = the row height, so the slider's rect is the row and
  its centre the row's), `handle_shape = HandleShape::Rect { aspect_ratio: 0.0 }` (so value-to-x spans the whole
  width), transparent `bg_fill`s and `selection.bg_fill`, no strokes. Then paint from the response: `t =
  animate_bool_with_time(r.id, enabled && (hovered || dragged || has_focus), SLIDER_HOVER)`; rail
  `lerp(4..=6, t)` tall in `surface_active` on `r.rect.center().y` across `r.rect.x_range()`; played part in
  `blend(text, accent, t)`; thumb radius `6 * t` in `text` (skip at 0); faded with the rest when disabled. Place
  the seek times and the slider in the same row rect so they share its centre; the volume slider in the icon row.
  Hand cursor on the enabled slider (Task 1's rule).
- [ ] **Step 4: Run the three tests, then the whole suite.** Expected: PASS. `slider_knobs_are_small` is removed
  (superseded).
- [ ] **Step 5:** Add "Slider hover | 120 ms" to the motion table (design-system spec §4.5) and fix §4.6's slider
  line to describe the new look. **Commit** `fix(player bar): centred sliders, thumb only on hover`.

---

### Task 3: Sidebar toggle in the top bar

**Files:**
- Modify: `crates/app/src/ui/sidebar.rs` (`contents`, tests), `crates/app/src/ui/top_bar.rs` (`show`, tests)

**Interfaces:**
- Produces: `fn sidebar_toggle(ui: &mut Ui, ctx: &mut Ctx)` in `top_bar.rs`: the `Icon::PanelLeft` `icon_button`
  labelled "Collapse sidebar" / "Expand sidebar" (by the setting, as now), pushing `Event::Settings` and sending
  `Action::SetSettings` exactly as `sidebar.rs` does today. `top_bar::show` calls it before `history`.

- [ ] **Step 1: Move and write the failing tests.** Move `collapsing_flips_the_setting_and_rows_still_work` and
  `the_collapse_button_keeps_the_focus` from `sidebar.rs` to `top_bar.rs`, driving `draw` (the top bar's `pass`
  helper) instead of the sidebar alone. Add:

```rust
#[test]
fn the_toggle_leads_the_top_bar() {
    // at 1000 wide: bounds("Collapse sidebar").right() <= bounds("Back").left();
    // their centres' y within 1.0 of TOP_BAR / 2; bounds("Home") centre y within 1.0 of TOP_BAR / 2
}

#[test]
fn the_top_bar_fits_at_the_minimum_width() {
    // MIN_WINDOW wide, UiState { side: Some(Side::Queue) }: "Collapse sidebar", "Back", "Forward",
    // "Search TIDAL" and "Settings" are all inside the window and pairwise non-overlapping
}
```

  In `the_top_bar_comes_first_in_tab_order`, `bar` becomes
  `["Collapse sidebar", "Back", "Forward", "Search TIDAL", "Settings"]`.
- [ ] **Step 2: Run them and see them fail** (the toggle is in the sidebar; Home sits a bar's height down).
- [ ] **Step 3: Implement.** Delete the toggle row from `sidebar::contents`; start the sidebar's scroll with
  `(TOP_BAR - nav row height) / 2` of space so Home's centre lines up with the bar's. Call `sidebar_toggle` first in
  `top_bar::show`. Update `contents`' and the module docs.
- [ ] **Step 4: Run the moved and new tests, then the whole suite.** Expected: PASS.
- [ ] **Step 5:** Update design-system spec §5.2's "Collapse" bullet to the new place. **Commit**
  `feat(ui): sidebar toggle in the top bar`.

---

### Task 4: Pages: space at the end, fade in

**Files:**
- Modify: `crates/app/src/ui/mod.rs` (`page`, new `fade_in`), `crates/app/src/theme.rs` (constant)
- Test: `crates/app/src/ui/mod.rs` (tests module)

**Interfaces:**
- Produces: `pub const PAGE_FADE: f32 = 0.15;`; `fn fade_in(ui: &mut egui::Ui, route: &Route)` multiplies `ui`'s
  opacity by the page's fade.

- [ ] **Step 1: Write the failing tests:**

```rust
#[test]
fn every_page_ends_with_room() {
    // Settings at 800×600, logged in, autoscrolled to the bottom (as `scrolled` in top_bar's tests):
    // bounds("Check now").bottom() + theme::MODULE_GAP <= the player bar's top + 1.0
}

#[test]
fn pages_fade_in() {
    // album page (top_bar tests' `album_page(3)`), passes with RawInput::time:
    // t = 0.0: the title's TextShape ("Album 1") is painted at opacity < 1
    // t = 0.2: at opacity 1; t = 0.3: repaint_delay == Duration::MAX
    // navigate to Home and back (new model routes): t restarts — opacity < 1 on the first pass back
    // the page loading (Event::Loading, no data) then the data arriving: opacity < 1 on the first pass with data
    // a pointer click on the album's Play pill on the very first pass is delivered (input works mid-fade)
}
```

  (Opacity: the text shape's `opacity_factor` times its colour's alpha, or whatever egui 0.36's `TextShape` carries;
  write one small helper.)
- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement.** `page()` adds `theme::MODULE_GAP` of space after `f`'s content, inside the scroll.
  `fade_in` keeps `(Route, last pass drawn, start time)` in egui temp data under one id: when the route differs or
  the last pass drawn is not the previous pass (`ctx.cumulative_pass_nr()`), the start becomes now. Opacity
  `((now - start) / PAGE_FADE).clamp(0, 1)`; below 1 it calls `request_repaint()`. `page()` calls it on its content
  `Ui` (pages only reach `page()` once their data is in, so arrival after the spinner restarts it). The top bar,
  sidebar and player bar are outside `page()` and don't fade.
- [ ] **Step 4: Run the tests, then the whole suite.** Expected: PASS.
- [ ] **Step 5:** Motion table row "Page fade-in | 150 ms". **Commit** `feat(ui): room at each page's end, pages
  fade in`.

---

### Task 5: Text fields and buttons share one height

**Files:**
- Modify: `crates/app/src/ui/widgets.rs` (new `text_field`), `ui/settings.rs` (`field`, buttons, `combo`),
  `ui/login.rs` (fields, Continue), `ui/library.rs` (dialog `field`), `ui/mod.rs` (Retry ×2), `ui/shelf.rs`
  (`Layout::Links` buttons)
- Test: `crates/app/src/ui/settings/tests.rs`, `crates/app/src/ui/widgets.rs` tests

**Interfaces:**
- Produces: `pub fn text_field(ui: &mut Ui, p: &Palette, edit: egui::TextEdit<'_>) -> Response` in `widgets.rs`:
  adds `edit` with a `PILL`-tall fully round `surface` frame, no stroke, 16 pt side padding, text vertically centred,
  and draws `focus_ring` around it. Callers keep their own `desired_width`, `hint_text`, `password`, `id`.

- [ ] **Step 1: Write the failing tests:**

```rust
// widgets.rs
#[test]
fn text_fields_are_a_pill_tall() {
    // text_field(ui, &p, TextEdit::singleline(&mut s).hint_text("Name")) → response.rect.height() == theme::PILL
    // typing after a pointer click still edits `s` (as search_field_keeps_its_id does)
}

// settings/tests.rs
#[test]
fn a_rows_field_and_button_line_up() {
    // Settings, logged in, ListenBrainz not connected: bounds("User token").height() == theme::PILL (±0.5),
    // bounds("Connect").height() == theme::PILL (±0.5), their centres' y within 0.5
}
```

- [ ] **Step 2: Run them and see them fail** (field about 21 tall, Connect 32).
- [ ] **Step 3: Implement** `text_field`; route `settings::field`, both login fields and `library::field` through
  it. Replace every stock `ui.button` / `egui::Button` outside menus with `pill_button(ui, &p, None, label, false)`
  (login's Continue: `true`, the step's main action), keeping each `add_enabled` and its hover text. In `combo`,
  scope `interact_size.y = theme::PILL` around the `ComboBox`.
- [ ] **Step 4: Run the tests, then the whole suite.** Fix any test that looked for the old sizes only if the
  change is the intended one.
- [ ] **Step 5: Commit** `feat(ui): one control height for fields and buttons`.

---

### Task 6: Roomy menus with icons

**Files:**
- Create: `crates/app/assets/icons/list-start.svg`, `link.svg`, `folder-input.svg` (latest Lucide release; add them
  to `crates/app/assets/icons/LICENSE.txt`'s list)
- Modify: `crates/app/src/theme.rs` (`Icon` variants, `menu_style`, constants), `crates/app/src/ui/widgets.rs`
  (`menu_button`, `context_menu`, `menu_separator`), `ui/tracks.rs` (`menu`), `ui/library.rs` (folder and playlist
  menus, `add_to_playlist`), `ui/queue_panel.rs` (row menu)
- Test: `crates/app/src/ui/tracks.rs` tests, `crates/app/src/ui/widgets.rs` `icons_load` (covers the new icons)

**Interfaces:**
- Produces in `theme.rs`: `Icon::{ListStart, Link, FolderInput}` (app files) and `Icon::{Plus, Pencil, Trash2}`
  (`lucide "plus"`, `"pencil"`, `"trash-2"`, shared in fastframe-icons); `pub const MENU_ITEM: f32 = 34.0;`,
  `pub const MENU_W: f32 = 200.0;`; `pub fn menu_style(s: &mut egui::Style)` (egui's menu style with
  `button_padding` x 12, `icon_spacing` 12, `item_spacing.y` 0, `interact_size.y` `MENU_ITEM`, hovered corner
  radius 8, `menu_margin` 6, inactive fill transparent).
- Produces in `widgets.rs`: `pub fn menu_button<'a>(p: &Palette, icon: Icon, label: impl Into<egui::WidgetText>)
  -> egui::Button<'a>` (16 pt `secondary` icon then the label, `min_size` `MENU_ITEM` tall); `pub fn
  context_menu(resp: &Response, add: impl FnOnce(&mut Ui))` (`Popup::context_menu(resp).style(theme::menu_style)`,
  at least `MENU_W` wide); `pub fn menu_separator(ui: &mut Ui)` (1 px `outline`, 5 above and below). Task 10 adds
  toasts at the call sites this task edits.

- [ ] **Step 1: Write the failing test** next to `context_menu_go_to_album_navigates`:

```rust
#[test]
fn the_row_menu_is_roomy() {
    // right-click a row (testing::right), next pass:
    // for "Play next", "Add to queue", "Go to album", "Go to artist", "Copy link":
    //     bounds(label).height() >= theme::MENU_ITEM - 0.5
    // separators: a 1 px outline hline lies between bounds("Add to playlist").bottom() and
    //     bounds("Go to album").top(), and between bounds("Track radio") and the favourites item
    // the menu's frame is at least MENU_W wide
    // opening "Add to playlist": "New playlist…" is at least MENU_ITEM tall too
}
```

- [ ] **Step 2: Run it and see it fail** (items about 32 tall, no separators).
- [ ] **Step 3: Implement.** Swap every `row.context_menu` / `header.context_menu` / `resp.context_menu` for
  `widgets::context_menu`, each item for `ui.add(menu_button(..))` (or `add_enabled`, or
  `ctx.favorite_button(ui, fav, menu_button(..))`), and the submenus' `ui.menu_button(label, ..)` for
  `ui.menu_button((icon image, label), ..)` keeping `add_to_playlist`'s AccessKit relabel. Its page pill's
  `Popup::menu(&pill)` gets `.style(theme::menu_style)` too. Icons per the spec §3.7: Play next `ListStart`, Add to
  queue `ListPlus`, Add to playlist `ListMusic`, New playlist… `Plus`, each own playlist `ListMusic`, Go to album
  `Disc3`, Go to artist `User`, Track radio `Radio`, favourites `Heart` / `HeartFilled` (by the current state),
  Copy link `Link`, Remove from this playlist and the queue's Remove `X`, Rename `Pencil`, Delete `Trash2`, Move to
  folder `FolderInput`, each destination `Folder` (the app's `folder.svg`; add the variant if missing). Track menu
  groups split by `menu_separator`: (Play next, Add to queue, Add to playlist) | (Go to album, Go to artist, Track
  radio) | (favourites, Copy link) | (Remove from this playlist, when shown; replaces today's `ui.separator()`).
  Closing rules and the stale-row guard stay as they are.
- [ ] **Step 4: Run the test, `icons_load`, then the whole suite.** Expected: PASS.
- [ ] **Step 5:** Update design-system spec §5.8's menu line. **Commit** `feat(ui): roomy menus with icons`.

---

### Task 7: The playback source

**Files:**
- Modify: `crates/app/src/model.rs` (`Source`, `QueueSnapshot`), `crates/app/src/messages.rs` (`PlayTracks`),
  `crates/app/src/queue.rs` (`Queue`), `crates/app/src/playback.rs` (`PlayTracks` handler),
  `crates/app/src/ui/mod.rs` (new `playing_from`), `ui/header.rs` (`play_button`), `ui/tracks.rs` (`show`),
  `ui/shelf.rs` (track card), every test that builds `Action::PlayTracks`
- Test: `crates/app/src/ui/mod.rs` tests, `crates/app/src/playback/tests.rs`

**Interfaces:**
- Produces: `#[derive(Clone, Debug, PartialEq)] pub struct Source { pub route: Route, pub name: String }` in
  `model.rs`; `QueueSnapshot::source: Option<Source>`; `Action::PlayTracks { tracks, start, source:
  Option<Source> }`; `Queue::source: Option<Source>` (set by the `PlayTracks` handler, included in `snapshot()`,
  untouched by `insert_next`, `append`, `move_item`, `remove`, shuffle and repeat); `pub fn playing_from(model:
  &Model) -> Option<Source>` in `ui/mod.rs`.

- [ ] **Step 1: Write the failing tests:**

```rust
// ui/mod.rs
#[test]
fn playing_from_names_the_page() {
    // at(Route::Album(1)) with its PageData::Album → Some(Source { route: Album(1), name: "Album 1" })
    // Playlist "p-1" with its data → name "Playlist"; Mix "m1" → "Mix"; Artist(1) → "Artist 1";
    // Collection(Tracks) → collection::name(Tracks); Home, Search, ViewAll(_) → None; a page without data → None
}

// playback/tests.rs
#[test]
fn the_queue_keeps_its_source() {
    // PlayTracks { source: Some(s) } → snapshot().source == Some(s); PlayNext / AddToQueue / MoveInQueue /
    // RemoveFromQueue (not the last) / SetShuffle leave it; a second PlayTracks { source: None } clears it
}
```

  Plus, in `tracks.rs` tests: double-clicking a row on `at(Route::Playlist("p-1"))` with its data sends
  `PlayTracks` whose `source` is `Some(Source { route: Playlist("p-1"), name: "Playlist" })`; on Search it is `None`.
- [ ] **Step 2: Run them and see them fail** (compile errors first: add `source: None` to every existing
  `PlayTracks` construction in tests).
- [ ] **Step 3: Implement.** `playing_from` maps `model.route()` and its `PageData` to a name: Album → album title,
  Playlist → playlist title, Mix → mix title, Artist → artist name, Collection(tab) → `collection::name(tab)`;
  anything else, or no data, `None`. `header::play_button` and `tracks::show` send `source: playing_from(ctx.model)`;
  the shelf's track card sends `None`. The handler sets `self.queue.source` before `publish_queue()`.
- [ ] **Step 4: Run the tests, then the whole suite.** Expected: PASS.
- [ ] **Step 5: Commit** `feat: playback remembers where it started`.

---

### Task 8: Player-bar links

**Files:**
- Modify: `crates/app/src/ui/player_bar.rs` (`now_playing`; `show` gains a last parameter `events: &mut
  Vec<Event>`, and the tests' `bar` helper passes one), `crates/app/src/ui/mod.rs` (`draw` passes `&mut events`)
- Test: `crates/app/src/ui/player_bar.rs` tests

**Interfaces:**
- Consumes: `QueueSnapshot::source` (Task 7).
- Produces: the cover, the title and each artist in the player bar are focusable link widgets (`Sense::click()`,
  AccessKit `WidgetType::Link`) named "Playing from {name}" (cover, with a source) or "Go to album {title}" (cover
  without one, and the title), and "Go to artist {name}"; clicked, they push `Event::Navigate(route)`.

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn the_now_playing_column_links_out() {
    // model.playback.track = Some(testing::track(1)) (album 1, artist 1); model.queue.source =
    //   Some(Source { route: Route::Playlist("p-1".into()), name: "Playlist".into() })
    // AccessKit click on "Playing from Playlist" → [Event::Navigate(Route::Playlist("p-1"))]
    // AccessKit click on "Go to album Album 1" → [Event::Navigate(Route::Album(1))]
    // AccessKit click on "Go to artist Artist 1" → [Event::Navigate(Route::Artist(1))]
    // source None: the cover is named "Go to album Album 1" too (two nodes with that name) and opens Album(1)
    // hovering the title: cursor PointingHand, and a hline under the title text (the underline)
    // Tab reaches the cover, the title and the artist before "Shuffle"
}
```

- [ ] **Step 2: Run it and see it fail.**
- [ ] **Step 3: Implement.** In `now_playing`, allocate the cover rect and each text line's galley rect with
  `Sense::click()`; give each `widget_info` as above, `focus_ring`, the hand cursor, and on hover an underline (1 px
  in the text's colour under the galley) or, for the cover, a `SCRIM` at 30 % over it; tooltips "Playing from
  {name}" / "Go to album". Artists are laid out one galley per name with ", " between, cut with "…" when the
  column runs out (a name wholly past the cut gets no link). New strings: "Playing from {name}", "Go to album
  {title}", "Go to artist {name}".
- [ ] **Step 4: Run the test, then the whole suite** (the player bar's layout tests must still pass).
- [ ] **Step 5: Commit** `feat(player bar): cover, title and artists link out`.

---

### Task 9: Artist and album links in track lists

**Files:**
- Modify: `crates/app/src/ui/tracks.rs` (`paint_row`, `cell`)
- Test: `crates/app/src/ui/tracks.rs` tests

**Interfaces:**
- Produces: in every list `paint_row` draws (track lists and the queue panel), each artist name and the album name
  is a pointer link: underlined and hand cursor while the pointer is over it, a primary click on it pushes
  `Event::Navigate(Route::Artist(id))` / `Route::Album(id)`. No new focusable widgets.

- [ ] **Step 1: Write the failing tests** (locate a name's x from the painted `TextShape` of the joined artists via
  its galley, e.g. `galley.pos_from_cursor(CCursor::new(n))`):

```rust
#[test]
fn names_in_a_row_navigate() {
    // a playlist row whose track has artists [artist(1), artist(2)], album "Album 1":
    // click (press+release, primary) on "Artist 2" → [Event::Navigate(Route::Artist(2))]
    // click on "Album 1" in the album column → [Event::Navigate(Route::Album(1))]
    // click on the title → no Navigate; double-click on the title → PlayTracks sent
    // hover "Artist 1" → cursor PointingHand and an underline hline under it
}

#[test]
fn links_leave_the_row_its_menu_and_drag() {
    // right-click on "Artist 1" → the row's menu shows ("Play next" in shown())
    // in an editable playlist, a primary drag started on "Artist 1" sets the PlaylistDrag payload
}

#[test]
fn a_cut_name_links_only_what_shows() {
    // a narrow window where the artist column cuts "Artist 1, Artist 2, Artist 3" after "Artist 1, Art…":
    // clicking the shown "Art…" opens Artist(2); no point of the column opens Artist(3)
}
```

- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement.** In `paint_row`, lay out the artists cell and the album cell as today, keep each name's
  char range in the galley, and turn the ranges into rects with the galley's cursor positions, clipped to what was
  laid out (an elided name ends at the "…"). With `row.hover_pos()` inside a name's rect: underline it and set the
  hand cursor. With `row.clicked_by(PointerButton::Primary)` and the press inside a rect: `ctx.go(route)`.
  Hit-testing through the row's own response keeps its right-click menu, double-click and drag as they are.
- [ ] **Step 4: Run the tests, then the whole suite.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(tracks): artist and album names link out`.

---

### Task 10: Toasts

**Files:**
- Create: `crates/app/src/ui/toast.rs`
- Modify: `crates/app/src/ui/mod.rs` (`mod toast;`, `draw` paints it last before the transition),
  `crates/app/src/theme.rs` (constants), `ui/tracks.rs` (`menu`), `ui/library.rs` (`add_to_playlist`)
- Test: `crates/app/src/ui/toast.rs`

**Interfaces:**
- Produces: `pub const TOAST_FADE: f32 = 0.15;`, `pub const TOAST_SHOW: f32 = 2.5;`; `pub fn toast::show(ctx:
  &egui::Context, text: String)` (replaces any toast showing); `pub fn toast::paint(ctx: &egui::Context, p:
  &Palette, over: egui::Rect)` (bottom-centre of `over`, `PAGE_PAD` above its bottom, on a foreground layer that
  takes no input).

- [ ] **Step 1: Write the failing test:**

```rust
#[test]
fn a_toast_comes_and_goes() {
    // passes of `draw` with RawInput::time; at t = 0 `toast::show(ctx, "Link copied")`:
    // t = 0.0: "Link copied" in shown(); its AccessKit node is a polite live region
    // t = 1.0: still shown; repaint_delay > 0 and <= the time left (no repaint every frame mid-show)
    // t = 2.9 (past TOAST_SHOW + TOAST_FADE): gone; the next pass repaint_delay == Duration::MAX
    // a second show at t = 1.0 replaces the first (one toast at a time)
    // a click where the toast sits reaches the page under it
}
```

  And in `tracks.rs` tests: picking "Copy link" from the row menu shows "Link copied"; "Play next" shows "Playing
  next"; "Add to queue" shows "Added to queue".
- [ ] **Step 2: Run it and see it fail.**
- [ ] **Step 3: Implement** `toast.rs`: `(text, shown_at)` in egui temp data; opacity in over `TOAST_FADE`, out
  over the last `TOAST_FADE` of `TOAST_SHOW + TOAST_FADE`; `request_repaint()` only while fading,
  `request_repaint_after(time left until the fade-out)` while still; the pill: `overlay` fill, radius `R_CARD`, the
  popup shadow, `body()` text, padded like a menu item (12 × 8); AccessKit node with role `Status`, live `Polite`,
  label the text. `draw` passes the central panel's rect. Call `toast::show` where each action is sent: Copy link
  "Link copied", Play next "Playing next", Add to queue "Added to queue", a playlist picked in `add_to_playlist`
  "Added to {playlist}", the favourites menu item "Added to favorites" / "Removed from favorites" (the menu item
  only, not page hearts).
- [ ] **Step 4: Run the tests, then the whole suite.** Expected: PASS.
- [ ] **Step 5:** Motion table rows "Toast fade | 150 ms in and out, shown 2.5 s". **Commit** `feat(ui): toasts
  for actions with no visible result`.

---

### Task 11: Covers fade in on first arrival

**Files:**
- Modify: `crates/app/src/ui/mod.rs` (`cover`), `crates/app/src/theme.rs` (constant)
- Test: `crates/app/src/ui/mod.rs` tests

**Interfaces:**
- Produces: `pub const COVER_FADE: f32 = 0.2;`. `cover(..)` keeps its signature.

- [ ] **Step 1: Write the failing tests** (a card or a row with an image whose texture the test inserts into
  `state.images` with `TextureCache::insert`, as `the_backdrop_starts_behind_the_top_bar` does; read the textured
  shape's tint alpha):

```rust
#[test]
fn a_cover_fades_in_when_it_arrives() {
    // t = 0.0 no texture: the placeholder only; insert the texture; t = 0.05: textured shape alpha < 255;
    // t = 0.3: alpha 255; t = 0.4: repaint_delay == Duration::MAX
}

#[test]
fn a_cached_cover_shows_at_once() {
    // texture inserted before the first pass: alpha 255 on the first pass
    // a larger size inserted later for the same image: still alpha 255 (no fade on the swap)
}

#[test]
fn one_cover_in_many_places_fades_once() {
    // a playlist of 5 tracks from album 1 (same cover) plus the player bar playing one of them:
    // after insert, every textured shape of that image reaches alpha 255 by t = 0.3 and stays there
}
```

- [ ] **Step 2: Run them and see them fail.**
- [ ] **Step 3: Implement.** In `cover`, `let a = ctx.animate_bool_with_time(Id::new(("cover", img.key())),
  tex.is_some(), theme::COVER_FADE)`: its first call returns the target at once, so a cover first drawn with a
  texture shows at once and one first drawn without fades when the texture comes; a size swap keeps it `true`.
  Paint the placeholder, then the texture tinted `Color32::WHITE.gamma_multiply(a)` (placeholder only while `a <
  1`).
- [ ] **Step 4: Run the tests, then the whole suite.** Expected: PASS.
- [ ] **Step 5:** Motion table row "Cover fade-in | 200 ms, first arrival only". **Commit** `feat(ui): covers fade
  in when they arrive`.

---

### Task 12: Window minimum: winit fix (controller task, not a subagent)

**Files:**
- Modify: `Cargo.toml` (`[patch.crates-io] winit`), `Cargo.lock`, `docs/follow-ups.md`

- [ ] **Step 1:** Clone crmne/winit at `ed7caa9023f10b397f5b6ec8284a840cbd8a6f65` into a scratch directory; on a
  branch `fix/minmax-on-programmatic-resize`, change `src/platform_impl/windows/event_loop.rs`'s
  `WM_WINDOWPOSCHANGING` arm: keep `ProcResult::Value(0)` when the window is fullscreen, else
  `ProcResult::DefWindowProc(wparam)`. Write the commit message and the PR text (what, why, how to reproduce with
  `SetWindowPos`, the fullscreen path unchanged).
- [ ] **Step 2: Stop for the human.** Show the diff and the PR text; publish only on their go-ahead: `gh repo
  fork crmne/winit` (as rojnwa/winit), push the branch, open the PR on crmne/winit (`apps-0.30`), and the same fix
  against rust-windowing/winit `v0.30.x` and `master`.
- [ ] **Step 3:** Point `[patch.crates-io] winit` at `https://github.com/rojnwa/winit` and the fix's commit
  (comment: carried until crmne merges the PR, then back to crmne's rev); `cargo update -p winit`.
- [ ] **Step 4: Verify.** `cargo build --release --locked -p tidash`; launch the built exe; from PowerShell call
  `SetWindowPos` on its window to 200×150 and read `GetWindowRect`/client size: expected at least 960×540 client.
  Close it. (The controller's own launched window only; no input is sent.)
- [ ] **Step 5:** Add to `docs/follow-ups.md`: "winit: patch points at rojnwa/winit `<sha>` for the min-size fix;
  repoint at crmne/winit once `<PR url>` merges." **Commit** `fix: enforce the window minimum on programmatic
  resizes (winit patch)`.

---

### Task 13: Screenshots, docs, acceptance list

**Files:**
- Modify: `docs/superpowers/specs/2026-10-19-interface-polish-design.md` (status), this plan (acceptance results)

- [ ] **Step 1:** Screenshots with `TIDASH_SCREENSHOT=<png>` and `TIDASH_THEME=dark`, `light`, and a preset, on a
  `tidal://` album link and Settings (scrolled to the end); check the slider line, the top bar, the fields and the
  page's end by eye.
- [ ] **Step 2:** Set the spec's status to "built"; confirm the design-system spec's §4.5 rows and §5.2 / §5.8 lines
  from Tasks 2, 3, 4, 6, 10 and 11 are in.
- [ ] **Step 3: Human acceptance list** (one session, at the end):
  1. Hover the seek and volume bars: thumb grows in, bar thickens, accent; leave: back to a plain bar.
  2. Times sit on the seek bar's line; volume bar on the icons' line.
  3. GlazeWM: shrink Tidash's tile: the window stops at its minimum. FancyZones: same.
  4. The sidebar toggle sits before ← → and slides with the sidebar; Home lines up with the top bar.
  5. Settings: "Check now" has room above the player bar; the ListenBrainz field and Connect line up.
  6. Hand cursor on buttons, rows, cards, links, menu items; plain text no longer selects.
  7. Right-click menus: roomy, icons, separators, fade in.
  8. Player bar: cover opens the playlist you played from (tooltip says so), title the album, each artist theirs.
  9. Track lists: artist and album names underline and open; double-click still plays; right-click on a name
     still opens the menu.
  10. Toasts after Copy link, Play next, Add to queue, Add to playlist, favourites from a menu.
  11. Covers fade in once on a fresh page; scrolling back shows them at once. Pages fade in on navigation.
- [ ] **Step 4: Commit** `docs: interface polish built`.
