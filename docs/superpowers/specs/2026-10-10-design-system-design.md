# Tidash design system: design

**Date:** 2026-10-10. **Status:** draft for review. **Comes before:** M5 (ship).
**Builds on:** `2026-10-07-tidash-design.md` §7 ("Theme: dark default plus light (fastframe-theme)").

## 1. Goal

Tidash looks like the official TIDAL desktop client by default, and takes themes the way Spotifast does. Today it
draws stock egui: default Dark/Light visuals, text buttons ("Play", "Previous"), sizes hard-coded per view.

Success:

- Side by side with TIDAL 2.43 on the same pages (Home, an album, a playlist, search), Tidash reads as the same
  family: black canvas, white accent, the same shell, sizes and hover behaviour.
- Themes switch live: Follow system, Dark, Light, fastframe's 8 presets, and any palette file the user drops in the
  themes folder; a broken or deleted file falls back without breaking the look.
- The §10 budgets hold (0% CPU idle, working set under 150 MB on Home); the headless view tests keep passing.

## 2. Decisions

| Question | Decision |
|---|---|
| How far theming goes | Spotifast parity: Follow system / Dark / Light, fastframe's 8 presets, user JSON palettes in `<config>/themes/`. Colours only. |
| Layout | TIDAL's shell: back/forward and a search pill in a top bar over the content; an icon sidebar with the collection and the playlists; Settings behind a gear. |
| Architecture | Spotifast's pattern: the app owns a `Palette` with fastframe's 16 colour names, maps it onto egui's `Style` for stock widgets, and paints its own controls from it; sizes and type are constants. |
| Font | Inter (already bundled by `fastframe-fonts`, 400/500/600/700). TIDAL draws Cash Sans and bundles Nationale; both are proprietary. |
| Icons | SVG through `fastframe-icons` (Lucide, ISC), tinted at draw time; filled play/pause/skip/heart variants as app SVGs. |
| Frosted glass | egui has no backdrop blur: the player bar is opaque (`#1e1e1e` is TIDAL's `#282828bf` over black). The page header's blurred cover is a 16 px texture stretched with linear filtering. |

## 3. Architecture

### 3.1 Files

- **`crates/app/src/theme.rs`** (new)
  - `Palette`: `Copy`, `Serialize`/`Deserialize`; `dark: bool` and the 16 colours of `fastframe_theme::BASE_COLORS`
    (`window, panel, surface, surface_hover, surface_active, outline, text, secondary, dim, accent, accent_hover,
    on_accent, danger, warning, overlay, shadow`).
  - `Palette::dark()` / `Palette::light()`: the built-ins of §4.1 (Dark is the TIDAL look).
  - `impl fastframe_theme::Palette` (`base` and a 16-arm `set`), so presets and user files load unchanged.
  - `apply(ctx, &Palette)`: the egui `Style` mapping of §4.6, applied with `ctx.all_styles_mut`, then
    `fastframe_text`'s `apply_to_visuals` again (replacing visuals resets the text gamma).
  - The size, radius, type and motion constants of §4.3–4.5; `Tier` and its colours (§4.2).
  - `Icon`: `fastframe_icons::icons!` over the Lucide set plus app SVGs in `crates/app/assets/icons/`.
- **`crates/app/src/ui/widgets.rs`** (new): the controls stock egui cannot draw in TIDAL's style, each a free function
  that allocates, paints from the palette, sets its AccessKit `WidgetInfo` and draws the focus ring:
  `icon_button`, `circle_button`, `pill_button` (primary or secondary), `nav_row`, `search_field`, `badge`,
  `vertical_gradient`, `focus_ring`. Sliders, checkboxes, combo boxes, text fields and menus stay stock egui,
  styled through §4.6.
- Views (`ui/*.rs`) lose their local sizes and colours and draw with `theme` and `widgets`.

### 3.2 Dependencies

- `fastframe-theme` and `fastframe-icons` at tag v0.4.1 (Spotifast builds against the same code).
- `egui_extras` with the `svg` feature, patched to the same crmne/egui rev as the rest of egui (as Spotifast does);
  `egui_extras::install_image_loaders` then `fastframe_icons::install::<Icon>` in `setup_look`.
- egui's `serde` feature if `Color32` is not already serializable in the build (the settings cache stores a palette).
- `image` gains the `png` feature (the screenshot switch, §6.2).
- Inter's licence (`Inter-LICENSE.txt`) and Lucide's ship with the app (packaging, M5).

### 3.3 Settings

- `theme: Theme` gains `System`: `System | Dark | Light`, default **Dark**. Existing `"Dark"` / `"Light"` values parse
  unchanged.
- `custom_theme: Option<String>`: a filename in the themes folder; `None` uses `theme`.
- `custom_theme_cache: Option<fastframe_theme::CustomTheme<Palette>>`: the last palette read for `custom_theme`,
  deserialized with `fastframe_theme::read_cached_theme` (a damaged cache is `None`, never a failed load).
- Picking System, Dark or Light clears `custom_theme` and the cache.

### 3.4 Data flow

- `main.rs` owns a `fastframe_theme::Catalog<Palette>`. At launch it starts a scan of `<config>/themes/` with the
  presets enabled (installed there as files on the first scan); its waker is the app's repaint waker.
- Each pass, before drawing, `resolve_theme(ctx)`:
  1. If the catalog finished a scan (`poll()`), refresh `custom_theme_cache` from the selected file (a changed palette
     is saved like any settings change).
  2. Pick the palette: `TIDASH_THEME` (§6.2) if set, else the cached custom palette, else `Palette::dark()` or
     `light()` by `ctx.theme()`, with `ctx.set_theme` given `ThemePreference::System`, `Dark` or `Light` (a custom
     palette sets its own `dark`).
  3. Only if the result differs from the last applied one: `theme::apply` and store it.
- The palette reaches views as a field of `ui::Ctx` (it is `Copy`); `player_bar::show` and `queue_panel::show`, which
  take no `Ctx`, get it as a parameter.
- A Settings change (`Event::Settings`) needs nothing extra: the next pass resolves the new palette.

### 3.5 Failure handling

- A selected file that is missing or invalid keeps the cached palette; with no cache, Dark.
- Settings › Appearance shows the catalog's `status` for the selection in one line (fastframe's `Problem` worded:
  "Couldn't read <name>: <reason>"), so a user editing a theme sees why it did not take.
- An icon that fails to load paints nothing rather than egui's red placeholder (the fastframe loader never forgets
  bytes, which is the known cause).

## 4. Tokens

### 4.1 Built-in palettes

Dark is TIDAL's dark mode (its only mode). Light is ours, built with the same rules over white. Alpha colours are
`#RRGGBBAA`, as fastframe's format allows.

| Token | Dark (TIDAL) | Light | Used for |
|---|---|---|---|
| window | `#000000` | `#ffffff` | content, sidebar, side panel, top bar |
| panel | `#1e1e1e` | `#f5f5f5` | player bar |
| surface | `#1c1c1c` | `#f0f0f0` | search pill, text fields, secondary pills, artwork placeholder, dialogs |
| surface_hover | `#ffffff1a` | `#0000000f` | hover on rows, nav rows, menu items, secondary pills |
| surface_active | `#ffffff26` | `#0000001f` | selected nav row, pressed, active toggle disc |
| outline | `#ffffff14` | `#00000014` | sidebar, side panel and player dividers |
| text | `#ffffff` | `#000000` | primary text and icons |
| secondary | `#ffffff8c` | `#0000008c` | subtitles, idle icons, index and duration columns, times |
| dim | `#ffffff4d` | `#00000059` | section headers, placeholders, disabled |
| accent | `#ffffff` | `#000000` | primary pill, play disc, slider fill, focus ring, selection |
| accent_hover | `#e5e5e5` | `#333333` | hovered primary pill and play disc |
| on_accent | `#000000` | `#ffffff` | glyphs and labels on accent |
| danger | `#ff4242` | `#d92d2d` | destructive menu items, failures |
| warning | `#ff3269` | `#d4145a` | offline and error strip |
| overlay | `#2d2d2d` | `#ffffff` | menus, popovers, tooltips |
| shadow | `#0000004d` | `#00000026` | menu and dialog shadows |

### 4.2 Quality tiers

Not themed; one set per base. Tier from the playing stream's `OutputInfo`: lossy codec → Low; FLAC at ≤ 16 bit and
≤ 48 kHz → Lossless; FLAC above either → Max.

| Tier | Dark | Light |
|---|---|---|
| Max | `#ffd432` | `#9c7a00` |
| Lossless | `#33ffee` | `#00897b` |
| Low | `text` | `text` |

The format badge draws the tier colour on a 10% tint of it, radius 4. The playing row's title takes the tier colour.

### 4.3 Typography

Inter through the existing `FontSetup::default()`; `fastframe_fonts::Weight::X.font_id(size)`.

| Style | Size / weight | Used for |
|---|---|---|
| Small | 12 Regular | times, badge, player artist line, sidebar row subtitle, column header |
| Body | 14 Regular | secondary lines, settings text, card subtitle |
| Body strong | 14 Medium | track title, card title, sidebar playlist title |
| Label | 14 SemiBold | nav rows, player title, pill labels |
| Heading | 19 Bold | shelf titles, side panel titles, settings sections |
| Title | 32 Bold | page header title |

egui text styles: Small 12, Body 14, Button 14, Heading 19, Monospace 13.

### 4.4 Sizes, spacing, radii

| Constant | Value |
|---|---|
| Spacing scale | 4, 8, 12, 16, 20, 24, 40 |
| Sidebar | 240 wide, not resizable |
| Top bar | 56 |
| Player bar | 88 tall, padding 18 |
| Side panel | default 360, 280–560, resizable |
| Page padding / gap between modules | 20 / 40 |
| Track row / column header | 48 / 36; index column 42; cover column 42 (40 px cover) |
| Card | target 175 wide, gap 16, `n = clamp(floor(W / 175), 1, 8)` per row, `width = (W − (n−1)·16) / n` |
| Covers | page header 240, player 52, sidebar 36, queue 40 |
| Buttons | icon 32 (16 glyph), transport 40, play disc 40 (22 glyph), card play/overlay controls 36, pill 40 tall |
| Radii | artwork 2, row 5, badge 4, field and stock widget 8, nav row / menu / card hover 12, dialog 20, pills and discs full |

### 4.5 Motion

| Use | Value |
|---|---|
| Hover colour on rows, nav rows, icons | 150 ms (`ctx.animate_bool_with_time`) |
| Card cover overlay | 300 ms fade |
| Discs and icon buttons | hover scale 1.1, press 0.9 |
| egui `animation_time` | 0.15 s |

Animations repaint only while running; nothing animates at rest, so idle stays at 0 passes.

### 4.6 egui `Style` mapping (stock widgets)

From `Visuals::dark()` or `light()` by `dark`, then:

- `panel_fill` window; `window_fill` overlay; `extreme_bg_color`, `faint_bg_color`, `code_bg_color` surface;
  `override_text_color` text; `weak_text_color` secondary; `hyperlink_color` text;
  `warn_fg_color` warning; `error_fg_color` danger.
- `selection`: fill accent at 25% alpha, stroke 2 px accent (egui's focus outline).
- `window_stroke` outline; `window_corner_radius` 20; `menu_corner_radius` 12; `window_shadow` and `popup_shadow`
  offset (0, 4), blur 20, shadow colour.
- Widgets: `corner_radius` 8, `bg_stroke` none, `expansion` 0; fills inactive surface, hovered surface_hover and
  active surface_active, each composited over surface into an opaque colour (Dark's are translucent: on their own
  they would be darker than the idle fill); `weak_bg_fill` the same.
- Text styles of §4.3; `item_spacing` (8, 8); `button_padding` (16, 8); `interact_size` (40, 32); `menu_margin` 4.
- Sliders: `slider_rail_height` 4, circle handle, trailing fill in text colour.
- Scroll bars stay `ScrollStyle::solid()` (the floating bars' fade repaints while idle, S3): 6 px wide, invisible
  track, handle text at 35%, 55% hovered, text while dragged.

## 5. Shell and components

### 5.1 Shell

```
┌──────────────┬──────────────────────────────────────┬───────────┐
│ ⌂ Home       │ (‹)(›)  [⌕ Search          ]     (⚙) │ Queue     │
│ ◎ Explore    │ ─ offline / error strip, if any ─    │           │
│              │                                      │           │
│ COLLECTION   │  page (scrolls)                      │           │
│ ♫ Tracks     │                                      │           │
│ ◉ Albums ... │                                      │           │
│ PLAYLISTS  + │                                      │           │
│ ▣ Chill      │                                      │           │
├──────────────┴──────────────────────────────────────┴───────────┤
│ ▣ Title     │   ⇄  ⏮  (▶)  ⏭  ↻                │ MAX  ≡ ♪ ⓘ 🔈── │
│   Artist    │   0:42 ━━━━━━━━──────── 3:58      │                  │
└─────────────────────────────────────────────────────────────────┘
```

Panel order stays: player bar, sidebar, side panel, central. Logged out, only the login page and Settings show, with
neither sidebar nor top bar (as today).

### 5.2 Sidebar

- Window fill, 1 px outline on its right edge, 16 px side margins.
- Nav rows (`nav_row`): 24 px icon, Label text, radius 12, padding 8; idle secondary, hovered surface_hover and text,
  selected surface_active and text. Home (`house`), Explore (`compass`).
- "COLLECTION" section header (Small, dim): Playlists, Albums, Tracks, Artists, Mixes, each a nav row to
  `Route::Collection(tab)`. The Collection page drops its own tab row and titles itself with the tab's name.
- "PLAYLISTS" section header with a `+` icon button (New playlist / New folder, the existing menu), then the library
  tree: rows with a 36 px cover (radius 2) or a folder icon, title Body strong, subtitle Small secondary; folders keep
  their open/closed chevron. Rename, move and delete menus are unchanged.
- Back and Forward leave the sidebar for the top bar. Search and Settings leave it too.

### 5.3 Top bar and search

- 56 tall, window fill, inside the central panel above the page scroll; page padding on both sides.
- Back and Forward: 32 px `icon_button`s (`chevron-left` / `chevron-right`), disabled at the history's ends, labels
  "Back" / "Forward".
- `search_field`: a 40 px pill, 280 wide, surface fill, `search` icon and placeholder "Search TIDAL" in dim; focused
  it inverts (text fill, window glyphs), as TIDAL does. It keeps `search::query_id()`, so Ctrl+F focuses it. Enter
  with a non-blank query sends `Action::Search` and navigates to Search. The query state moves from the Search page
  to `UiState` and still leaves with the session.
- The Search page shows only results.
- Right end: a 32 px gear `icon_button` to Settings, label "Settings".
- Below the bar, the offline notice and errors as one strip: `circle-alert` icon in warning, the text, and an `x`
  `icon_button` labelled "Dismiss" per error.

### 5.4 Pages, headers, shelves

- Album, playlist, mix and artist headers: the cover requested at 16 px and painted stretched behind the top 450 px of
  the page, under a `vertical_gradient` from window at 40% to window at 100%, so it scrolls with the page like TIDAL's
  blurred backdrop. Over it: the 240 px cover (round for artists), a Small dim type line ("ALBUM", "PLAYLIST"),
  the Title, the detail lines in secondary, then an action row: a primary `pill_button` Play with the `play-filled`
  icon, a 40 px heart `circle_button` (`heart` / `heart-filled`, label "Add to favorites" / "Remove from favorites"),
  then the page's own actions as secondary pills (Add to playlist, Edit, Delete, Artist radio).
- Shelves: Heading title; on the right "View all" (Body, secondary, text on hover) and ‹ › 32 px icon buttons that
  scroll the row by its visible width. The row's own scroll bar is hidden; wheel, touchpad and Tab still scroll it.
  Modules are 40 apart.
- Grids (Collection, View all): rows of `n` cards from §4.4, virtualized as today with the row height derived from the
  card width.

### 5.5 Track lists

- Column header row (36): "#", "Title", "Artist", "Album" (lists spanning albums), and a `clock` icon over durations;
  Small, secondary.
- Rows (48): hover surface_hover, radius 5, 150 ms fade. Index column: the number in secondary; on hover a 16 px
  `play-filled` in text; the playing row an `audio-lines` icon in the tier colour. Lists spanning albums (today's
  `album` flag) add a 40 px cover column. Title Body strong in text (the playing row's in the tier colour); artist and
  album Body in text; duration Body in secondary, tabular, right-aligned. Unavailable rows at 50% opacity. Disc rows:
  `disc-3` icon and "Disc n" in Small secondary.
- Context menu, drag to reorder and keyboard activation are unchanged; the drop line is 2 px accent.

### 5.6 Cards

- Cover square (round for artists), radius 2, at the card's full width; 8 below it the title (Body strong, one line)
  and the subtitle (Body, secondary, one line).
- Hover or focus fades in a gradient over the cover's lower half (transparent to black at 70%) in 300 ms; focus also
  draws the 2 px ring. No card background fill.
- No play button on the card: playing an album or playlist by id needs a backend action that does not exist yet
  (follow-up).

### 5.7 Player bar

- Panel fill, 88 tall, 1 px outline on its top edge; three columns 1 : 1.5 : 1.
- Left: 52 px cover (radius 2); title Label; artists Small secondary.
- Centre: Shuffle and Repeat (32, 16 px glyphs; active ones get a surface_active disc), Previous and Next (40,
  `skip-back-filled` / `skip-forward-filled`), Play/Pause a 40 px accent `circle_button` with a 22 px on_accent glyph.
  Below, the seek row (max 800 wide): times in Small secondary, tabular; a stock slider styled per §4.6.
- Right: the format badge (`badge`, tier colours), then Queue (`list-music`), Lyrics (`mic-vocal`), Credits (`info`)
  icon buttons, text when their panel shows and secondary otherwise, then a volume icon and a 90 px slider.
- Every control keeps its current AccessKit label: "Play"/"Pause", "Previous", "Next", "Shuffle", "Repeat off" /
  "Repeat all" / "Repeat one", "Queue", "Lyrics", "Credits", "Volume"; tooltips show the same text.

### 5.8 Side panel, menus, dialogs, Settings, login

- Side panel: window fill, 1 px outline on its left edge, Heading title. Queue rows reuse the track-row painter with a
  40 px cover. Lyrics: lines in 16 SemiBold, the current one in text and the rest in dim. Credits: role headers in
  Small secondary, names as links.
- Menus, popovers, tooltips and dialogs are stock egui, styled by §4.6 (overlay fill, radius 12 and 20, shadow).
- Settings: Heading per section, one row per setting (label left, control right). Appearance becomes a combo box:
  Follow system, Dark, Light, a separator, then the catalog's themes by `display_name`; beside it "Open themes folder";
  under it the catalog status line. Primary actions (Apply, Log in) are primary pills; the rest stock buttons.
- Login: a centred column with the primary pill "Log in with TIDAL" and a Settings link.

## 6. Verification and testing

### 6.1 Tests (`cargo test`)

- `theme`: each of the 16 names sets its colour through `parse_palette`; every fastframe preset parses into
  `Palette`; `apply` sets `panel_fill`, `window_fill` and `override_text_color` from the palette.
- Settings: old `"Dark"` / `"Light"` files load; the default is Dark; a damaged `custom_theme_cache` loads as `None`.
- Tier from `OutputInfo` (lossy, 16/44.1 FLAC, 24/96 FLAC, 16/96 FLAC).
- Views (headless, by AccessKit label): the existing player bar, shortcut and dismiss tests pass unchanged; Enter in
  the top-bar search sends `Search` and opens the Search page; Collection nav rows open their tab; picking a theme in
  Appearance emits settings with `custom_theme` set.

### 6.2 Looking at it

- `TIDASH_SCREENSHOT=<file.png>`: the window opens at 1440×900 on the route of the launch's TIDAL link (Home without
  one); once the page has loaded and no artwork is still loading (at most 20 s), it saves a screenshot of the window
  (`ViewportCommand::Screenshot`) and quits.
- `TIDASH_THEME=<dark|light|system|filename>`: overrides the theme for that run without saving it.
- Reference screenshots of TIDAL 2.43 on the same pages, captured once by window capture with no input sent, kept in
  the session scratchpad and not committed (TIDAL's UI and artwork).
- Each restyled view is checked against its reference in Dark, then once in Light and one preset.
- Budgets: idle passes with `TIDASH_REPAINT_LOG=1` (0 with the pointer away), working set on Home, binary size before
  and after (`resvg` comes in with the SVG loader).

### 6.3 Human acceptance

One session at the end: Tidash beside TIDAL; theme switching live, including a hand-edited palette file and a broken
one; keyboard focus visible on every control; hover and press feel.

## 7. Not in this design

- Album-art tint of the player bar and headers (declined); the theme-change reveal animation; Omarchy.
- Theming sizes, spacing or fonts; a collapsible sidebar; search as you type.
- Card play and favourite buttons, and the track row's "…" button (the context menu covers them); playing by id
  needs a backend action.
- A now-playing full-screen view.

## 8. Changes to the main spec

- §7 Theme line: "Dark (TIDAL look, default), Light and Follow system, plus palette files: fastframe's presets and the
  user's own in `<config>/themes/` (fastframe-theme). See the design-system design."
- §7 Shell line: back/forward and search move to a top bar; Settings to a gear; the collection's tabs to the sidebar.
- §12: the screenshot switch is a manual aid, not a screenshot test; "no screenshot tests in v1" stands.
- §15 (dependencies): `fastframe-theme`, `fastframe-icons`, `egui_extras` (svg).
