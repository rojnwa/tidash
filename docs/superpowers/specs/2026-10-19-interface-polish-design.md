# Tidash interface polish: design

**Date:** 2026-10-19. **Status:** built (2026-10-20); human acceptance pending (plan Task 13). **Builds on:** `2026-10-10-design-system-design.md`
(its palette, sizes, motion table and rulings stay; this changes only what is named here).

## 1. Why

The human's words: Tidash "does not feel like an app"; it has visual bugs and lacks the small touches that make
software feel "smooth, nice, fast and modern". They reported, with screenshots:

1. The seek and volume sliders are misaligned: the times sit about 6 px above the seek rail's centre line.
2. The sliders' thumb looks bad.
3. The window can be shrunk to about 1×1 px.
4. The sidebar's collapse button takes a row of its own, which looks odd when the sidebar is expanded.
5. Spacing is missing in places: Settings' last control ("Check now") sits flush on the player bar.
6. It doesn't feel like an app: no hand cursor on clickable things; little motion; the player bar's cover, title
   and artists don't navigate; artist and album names in track lists don't navigate; the right-click menu is
   cramped.
7. Text fields are thinner than the controls beside them (Settings' ListenBrainz token field next to Connect).

They handed the "modern feel" choices to the builder ("that's your job… orient on highly praised software"), then
picked from mockups (`.superpowers/brainstorm/`, `polish-choices.html`) the four options marked below as theirs.

**Success:**
- Each item above is fixed as §3 describes, in Dark, Light and custom themes.
- The budgets hold (design-system spec §1): 0 repaints at idle (every new animation repaints only while it runs),
  working set unchanged within noise. The headless tests keep passing, updated only where §3 changes behaviour on
  purpose.

**Non-goals:** a now-playing view; new settings (no "reduce motion" switch); new keyboard shortcuts; egui 0.37 (the
fork has no 0.37 branch yet); palette or type changes; a time tooltip on the seek bar; scroll-wheel volume.

**Anti-goals** (failure even if it works): motion that delays input or content (no transition blocks a click or
holds back a page); covers that re-fade when scrolled back into view or when a larger rendition replaces a smaller
one; anything that repaints at rest; extra Tab stops on every track row.

## 2. Decisions

| Item | Decision | Whose |
|---|---|---|
| Slider thumb | Hidden at rest; on hover/drag the rail thickens, the played part turns `accent`, the thumb appears | human (mockup B) |
| Sidebar toggle | Moves to the top bar, left of the Back/Forward pill; the sidebar starts with Home | human (mockup A) |
| Right-click menus | Roomier rows, a 16 px icon per item, separators between groups | human (mockup B) |
| Player-bar cover | Opens where playback started (playlist, mix, album, page); else the album | human (mockup B) |
| Title / artist links | Player-bar title → album; each artist name → that artist; same for artist and album names in track lists | human |
| Hand cursor | On everything clickable | human |
| Text cursor | Only in text fields; plain labels stop being selectable | builder's call |
| Control height | Buttons and text fields outside menus are one height, the pill's (40) | builder's call |
| Bottom spacing | Every page ends with `MODULE_GAP` of space | builder's call |
| Motion | Covers fade in on first arrival; pages fade in on navigation; buttons fade on hover | builder's call, shown to the human |
| Toasts | A short confirmation for actions with no visible result | builder's call, shown to the human |
| Window minimum | Root cause in winit (§3.2); fix carried in a fork now, PR'd upstream (§6) | human |

## 3. What changes

### 3.1 Player-bar sliders (`ui/player_bar.rs` `slider`)

- At rest: a 4 px `surface_active` rail with the played part in `text`, no thumb (no disc, no bump from egui's
  own handle).
- Hovered or dragged: over about 120 ms the rail grows to 6 px, the played part turns `accent`, and a 12 px `text`
  thumb grows in at the value. It returns the same way when the pointer leaves.
- Keyboard focus shows the hover look too, plus the focus ring. (Builder's call.)
- Disabled (no track; volume in exclusive mode): the rest look, faded as today.
- The times, the rail and the volume icon share one centre line: the seek row's times are centred on the rail, and
  the volume rail on the icon row. Clicking anywhere on the rail still seeks/sets; keyboard and screen-reader
  behaviour (arrow keys, AccessKit value) stay as egui's `Slider` gives them today.
- `slider_knobs_are_small` changes to: no disc at rest; one ≤ 7 px stroke-less disc while hovered.

### 3.2 Window minimum

The app already asks for 960×540 (`main.rs` `with_min_inner_size(theme::MIN_WINDOW)`) and Windows enforces it for
border drags. Programmatic resizes bypass it: winit's `WM_WINDOWPOSCHANGING` handler
(`src/platform_impl/windows/event_loop.rs` ~1299–1376 in crmne/winit `ed7caa9`, same code in upstream winit
0.30.x and master) returns 0 instead of passing the message to `DefWindowProc`, which is what applies
`WM_GETMINMAXINFO` to `SetWindowPos`. Tiling managers (GlazeWM, FancyZones, both running on the human's machine)
resize that way, so a small tile shrinks Tidash to nothing. Reproduced on the running app and in a bare Win32
window.

Fix: in the non-fullscreen case return `ProcResult::DefWindowProc(wparam)`. No app code changes; it reaches
Tidash's build as §6 says. An app-side "snap back to the minimum" was rejected: it fights the window
manager (resize loops, flicker).

### 3.3 Sidebar toggle (`ui/sidebar.rs`, `ui/top_bar.rs`)

- The `panel-left` icon button (same labels "Collapse sidebar" / "Expand sidebar", same setting) is the top bar's
  first control, left of the Back/Forward pill. It moves with the sidebar's edge as it slides.
- The sidebar loses its toggle row; its scroll starts at the top, with the first nav row level with the top bar's
  controls (same centre line).
- Keyboard: the toggle keeps its focus across the slide (today's `the_collapse_button_keeps_the_focus`), and comes
  first in the top bar's Tab order. Tests that look for it in the sidebar move with it.

### 3.4 Spacing (`ui/mod.rs` `page`)

- `page()` ends every route's content with `MODULE_GAP` of space, so nothing sits on the player bar (Settings'
  "Check now" included). One change, in the shared wrapper.
- Settings rows keep ruling 30's layout; their controls get the heights of §3.6.

### 3.5 Cursor and selection (`theme.rs`, `ui/widgets.rs`, the custom clickables)

- `style.interaction.selectable_labels = false`: plain text no longer shows the I-beam or selects on drag. Text
  fields keep the I-beam.
- The hand cursor (`CursorIcon::PointingHand`) on every enabled clickable: egui's stock `Button` (and so menu items)
  through `Visuals::interact_cursor`; the house widgets (`icon_button`, `circle_button`, `glyph_button`,
  `pill_button`, `nav_row`), track and queue rows, cards, library rows, shelf "View all", the links of §3.8, the
  slider rails, checkboxes, radio buttons and combo boxes, each with `on_hover_cursor`. Disabled controls keep the
  arrow.

### 3.6 Text fields and buttons

- One house text field, `widgets::text_field` (or the like), used by every `TextEdit` outside the search pill:
  Settings (`field()`), login (the URL and the paste box), and the playlist and folder dialogs (`library.rs`). It
  is 40 tall (`theme::PILL`), fully round like the search pill, `surface` fill, no stroke, text vertically centred,
  16 px inner padding, the focus ring as elsewhere. Width and hint behaviour stay per call site.
- Stock `ui.button`s outside menus become the house secondary `pill_button` (40 tall, fades on hover, hand
  cursor): Retry (`mod.rs`), Settings' Back, Open themes folder, Connect/Disconnect/Cancel, login's button, shelf
  "View all" if it is a stock button. Primary/secondary roles stay as they are.
- Combo boxes in Settings are 40 tall too, so a row's controls line up.

### 3.7 Right-click menus

One shared style for every context menu and its submenus: track rows (`tracks.rs`), library folders and playlists
(`library.rs`), queue rows (`queue_panel.rs`), and the "Add to playlist" / "Move to folder" submenus.

- Frame: `overlay` fill, 1 px `outline`, radius 12 (`R_CARD`), the popup shadow, 6 px inner margin, at least
  200 px wide. egui's own fade-in on open stays.
- Items: 34 px tall, 12 px text inset, a 16 px `secondary` icon then a 12 px gap, label in `body()` `text`; hover
  fill `surface_hover` with radius 8, inset by the frame margin; disabled items faded. A submenu item ends in a
  `secondary` chevron.
- Separators: 1 px `outline`, 5 px above and below, between groups. Track menu groups: (Play next, Add to queue, Add
  to playlist) | (Go to album, Go to artist, Track radio) | (Add to/Remove from favorites, Copy link) | (Remove from
  this playlist, when shown).
- Icons (Lucide; add the ones missing as app SVGs from the latest Lucide release, licence file updated): Play next
  `list-start`, Add to queue `list-plus`, Add to playlist `list-music`, New playlist… `plus`, Go to album `disc-3`,
  Go to artist `user`, Track radio `radio`, favorites `heart` / `heart-filled`, Copy link `link`, Remove from this
  playlist / Remove (queue) `x`, Rename `pencil`, Delete `trash-2`, Move to folder `folder-input`, folder entries
  `folder`.
- Behaviour (closing rules, the stale-row guard, keyboard/screen-reader activation) is unchanged.

### 3.8 Links: player bar and track lists

- Player bar: the title opens the album; each artist name opens that artist; the cover opens the playback source
  (§3.9). Hovered, a link underlines and shows the hand; the cover darkens slightly (like a card's scrim, lighter).
  Tooltips: the cover "Playing from {name}" (or "Go to album" without a source). The title and artists keep their
  "…" cut to the column.
- Track lists (`tracks.rs` rows, every list that uses them): each artist name in the Artist column and the album
  name in the Album column is its own link, with the same hover. The rest of the row keeps today's behaviour
  (hover fill, double-click to play, drag to reorder, right-click menu). Names cut with "…" stay clickable for the
  part shown.
- In track lists, links are pointer conveniences: they take no Tab stop (the row, its Enter/Space and its menu's Go
  to album / Go to artist cover keyboard and screen-reader users). The menu's "Go to artist" is unchanged.
- In the player bar, which has no row or menu to fall back on, the cover, the title and each artist are focusable
  links in the Tab order (after the sidebar toggle's place in the order is settled, they come before the transport),
  with AccessKit names "Playing from {name}" / "Go to album {title}" / "Go to artist {name}" and the focus ring.
  (Builder's call, from the builder check.)
- Queue panel rows get the same links: they are painted by the same `tracks::paint_row`. (Builder's call.)

### 3.9 Playback source

- `Action::PlayTracks` carries an optional source: the route playback started from and a human-readable name.
  Set by: the page header's Play (`header.rs`: the page's route and title) and a track row's double-click/Enter
  (`tracks.rs`: the current page's route and its title: album, playlist, mix, artist, collection tab). No source
  (the cover falls back to the album) for a single-track card on a shelf (`shelf.rs`; album/playlist/mix cards
  only navigate, their play buttons stay deferred per `docs/follow-ups.md`) and for search results
  (`Route::Search` keeps no query to return to). (Builder's calls, from the builder check.)
- The queue keeps it with its tracks and exposes it in its snapshot. Each `PlayTracks` replaces it; Play next, Add
  to queue, remove and reorder leave it. It lives as long as the queue (the queue is not saved across restarts, so
  neither is the source; the mockup's "survives restarts" was wrong).
- The cover navigates to the source's route, or to the track's album without one.

### 3.10 Toasts

- One toast at a time, bottom-centre of the content area, just above the player bar: `overlay` fill, radius 12,
  the popup shadow, `body()` text, padded like a menu item. Fades in (150 ms), stays about 2.5 s, fades out
  (150 ms); a new one replaces the old. Never takes focus or blocks clicks. AccessKit polite live region, so screen
  readers announce it.
- Shown for actions with no visible result: Copy link ("Link copied"), Play next ("Playing next"), Add to queue
  ("Added to queue"), favorites from a menu ("Added to favorites" / "Removed from favorites"), shown when the
  action is sent. Adding to a playlist shows the backend's answer as a toast instead ("Added {n} track(s) to
  {playlist}" or "Already in this playlist", formerly warn-coloured notices), so the toast never contradicts it.
  A failure still shows today's error notice. (Builder's call during the build.)
- Repaints only while fading or until its expiry (`request_repaint_after`), none at rest.

### 3.11 Motion

- **Covers** (`ui/mod.rs` `cover`): when a cover's first texture arrives, it fades over the placeholder in 200 ms.
  A cover already loaded when it comes into view shows at once; a larger rendition replacing a smaller one swaps
  without a fade. Built as a per-image opacity with the pass it was last drawn in: a cover not drawn in the
  previous pass shows its current state at once.
- **Pages** (`ui::draw`, around the route's content): on every route change (Back/Forward too), and when a page's
  data arrives after its spinner, the content below the top bar fades in over 150 ms (opacity only, no slide),
  keyed by the route and whether its data is in. Titles, spinners and failure messages fade with it. The first page
  a window draws shows at once. Input works from the first frame; the top bar, sidebar and player bar don't fade.
  Collection tabs fade on the switch only: their title is drawn before their data, so their list appears after the
  spinner without a second fade and the title never blinks. (Builder's call.)
- **Buttons**: covered by §3.6 (house pills already fade on hover over `HOVER`).
- Durations join the motion table (design-system spec §4.5): cover fade 200 ms, page fade 150 ms, toast fade
  150 ms, slider hover 120 ms.

### 3.12 Strings

Every new user-facing string goes through `tr()` and into `crates/app/po/tidash.pot` as the existing ones do.

## 4. Constraints

- Platform: Rust 2024, egui 0.36.2 from crmne/egui `6ce633b` and fastframe v0.13.0, both the newest that fit
  (fastframe's guide prescribes that egui rev; v0.13.0 is fastframe's newest tag). No new crates.
- `unsafe_code = "deny"` stays.
- Accessibility does not regress: every control keeps its AccessKit name and role, focus rings stay, keyboard paths
  stay (see §3.8's Tab rule).
- The headless view tests (`ui/testing.rs` helpers) cover new behaviour: the hand cursor on a hovered control
  (`platform_output.cursor_icon`), no selectable labels, the page's bottom space, slider thumb at rest vs hovered,
  toggle in the top bar, menu item height, `PlayTracks`' source and the cover's navigation (source and fallback),
  row links navigating without breaking double-click, a toast appearing and expiring with no idle repaint, a cover
  fading on first arrival but not when cached, the page fade ending with no repaint.
- GUI checks the builder can't do (real hover feel, GlazeWM resize) go into one human acceptance list at the end.
  Screenshots via `TIDASH_SCREENSHOT` are fine.

## 5. Left to the builder

The slider's mechanics (styling egui's `Slider` with a hidden handle vs a small custom widget, as long as keys and
AccessKit behave as today); how row links are hit-tested (per-name rects from the laid-out text); the toast's
storage and API; exact ids and helper names; ordering of the work.

## 6. How the window-minimum fix ships (the human's decision, 2026-10-19)

Fix it upstream and carry it meanwhile:

- Fork crmne/winit under the human's GitHub account (rojnwa/winit), commit the one-line fix on top of `ed7caa9`,
  and point Tidash's `[patch.crates-io] winit` at that commit. Tidash is fixed with this work.
- Open the fix as a PR on crmne/winit, and on upstream rust-windowing/winit (both 0.30.x and master carry the same
  code). Public text follows the repo's rule: Tidash is "a TIDAL client on fastframe I'm currently working on",
  not linked.
- When crmne merges, repoint the patch at crmne's rev and drop the fork; record that in `docs/follow-ups.md`.

Publishing (the fork, the PRs) needs the human's go-ahead at the moment it happens; the plan stops for it.

Rejected: waiting for the merge with no fix in the meantime; a GlazeWM float rule instead of a fix; an app-side
snap-back (§3.2).
