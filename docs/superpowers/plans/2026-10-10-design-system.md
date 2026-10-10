# Design System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tidash looks like the official TIDAL client by default and takes Spotifast-style themes: Follow system, Dark, Light, fastframe's presets and user palette files, with an album-art tint, an animated theme change and a collapsible sidebar.

**Architecture:** A new `theme.rs` owns a `Copy` `Palette` with fastframe's 16 colour names (so `fastframe-theme`'s presets and files load unchanged), the TIDAL Dark and Light built-ins, the mapping onto egui's `Style` for stock widgets, and TIDAL's sizes and type scale as constants. A new `ui/widgets.rs` paints the controls stock egui cannot (icon and circle buttons, pills, nav rows, the search pill, badges, gradients). `main.rs` owns the theme catalog, resolves the palette each pass and hands it to views through `ui::Ctx`. Views are restyled one area per task, keeping every AccessKit label so the headless tests keep clicking what they click today.

**Tech Stack:** Rust 2024, egui/eframe 0.36 on fastframe v0.4.1 (adds `fastframe-theme`, `fastframe-icons`), `egui_extras` 0.36 (`svg`), Inter via the existing `fastframe-fonts`, Lucide icons.

**Spec:** `docs/superpowers/specs/2026-10-10-design-system-design.md` (every section). Token sources: TIDAL 2.43.2's live CSS and Spotifast `dd2d5e3`, summarized in the spec's §4; the raw research is `scratchpad/research/{tidal-tokens.md,spotifast-theming.md}` in this session's scratchpad (not kept), with Spotifast (MIT) and fastframe cloned beside them for reading.

## Global Constraints

- License `GPL-3.0-or-later`; edition 2024; `rust-version = "1.98"`; workspace lints unchanged. CI runs `cargo clippy --all-targets -- -D warnings` and `cargo test` on Windows, macOS and Linux.
- New dependencies, all in `app`, and nothing else:
  - `fastframe-theme`, `fastframe-icons` (`git = "https://github.com/crmne/fastframe", tag = "v0.4.1"`)
  - `egui_extras = { version = "0.36", default-features = false, features = ["svg"] }`, plus `egui_extras` in `[patch.crates-io]` at the same crmne/egui rev as the other egui crates
  - `image` gains the `png` feature; egui gains `serde` if `Color32` is not already serializable in the graph
- Colours come only from `theme::Palette` and `theme`'s tier colours: no `Color32` literal in `crates/app/src/ui/` (tests excepted). Sizes, radii, fonts and durations come from `theme`'s constants and font functions.
- Every control keeps its current AccessKit label, and icon buttons show it as their tooltip. New controls get labels through `tr`.
- All user-visible strings go through `tr`/`tr_n` and are added to `crates/app/po/tidash.pot`. Messages are whole sentences.
- Nothing animates at rest: with the pointer away, `TIDASH_REPAINT_LOG=1` logs no passes on an idle page. Scroll bars stay `ScrollStyle::solid()`.
- Soft limit ~800 lines per file.
- Look up every library API before using it: ctx7 for egui (`/emilk/egui`); the fastframe sources at `%USERPROFILE%\.cargo\git\checkouts\fastframe-*\23d87e0\crates\` (`fastframe-theme`, `fastframe-icons`, `fastframe-fonts`); the egui fork at `%USERPROFILE%\.cargo\git\checkouts\egui-*\*\crates\`; Spotifast's `src/theme.rs` and `src/ui/widgets.rs` in the scratchpad clone as the reference implementation.
- In the worktree, run git through PowerShell (the Bash tool's git is blocked by a worktree guard). Never synthesize input on the human's desktop. Implementers do not launch the app; each visual task ends with a controller step that does.

## Review Focus

1. **A selected palette file that is deleted, emptied or broken while Tidash runs.** The cached palette stays, the status line says why, nothing panics. Test: Task 3 `a_selected_theme_that_vanishes_keeps_its_colours`.
2. **Theme changes in quick succession, or with no screenshot ever arriving (headless, some drivers).** The window ends on the last palette chosen, within 0.25 s, never on an intermediate one. Test: Task 3 `quick_changes_end_on_the_last_palette`.
3. **Very long or right-to-left titles in rows, cards, the sidebar and the player bar.** Truncated inside their column, never overlapping the next. Tests: Task 7 `long_titles_stay_in_their_columns`; Task 10 `a_long_title_keeps_the_transport_in_place`.
4. **A narrow window with the side panel open.** The grid keeps at least one card, the search pill shrinks rather than pushing the gear off, the player bar's columns do not overlap. Tests: Task 2 `card_columns_clamp`; Task 5 `the_gear_stays_in_a_narrow_window`; Task 10 `a_long_title_keeps_the_transport_in_place` (at 800 px).
5. **Keyboard-only use.** Every new control takes the focus with Tab, shows the 2 px ring, and activates with Enter or Space. Test: Task 4 `widgets_take_focus_and_activate_from_the_keyboard`.

---

### Task 1: Screenshot switch, references and a baseline

**Files:**
- Modify: `crates/app/src/main.rs` (`Switches`, `App::draw`, `main`'s `NativeOptions`, crate docs)
- Modify: `crates/app/src/images.rs` (`TextureCache::loading`)
- Modify: `crates/app/Cargo.toml` (`image` `png`)

**Interfaces:**
- Produces: `Switches { autoscroll: Option<f32>, repaint_log: bool, screenshot: Option<PathBuf>, theme: Option<String> }` (now `Clone`, not `Copy`); `TextureCache::loading(&self) -> bool`. Task 3 reads `switches.theme`.

- [ ] **Step 1: Write the failing tests.** Extend `switches_parse_and_default_off`: `TIDASH_SCREENSHOT=C:\x\home.png` gives `screenshot: Some(PathBuf::from(..))`, blank gives `None`; `TIDASH_THEME= Nord.json ` gives `theme: Some("Nord.json")` (trimmed), blank `None`. In `images.rs`: `loading_until_inserted_or_forgotten`: a `get` that requests makes `loading()` true; `insert` of that key makes it false; another `get` then `forget` makes it false.
- [ ] **Step 2: Run them; they fail to compile.** `cargo test -p tidash switches_parse loading_until`
- [ ] **Step 3: Implement.**
  - `TextureCache::loading`: some requested key has no texture.
  - With `screenshot` set, `main` opens the window at 1440×900 (`ViewportBuilder::with_inner_size`). `App::draw`, after `ui::draw`, once per run: when the route's page has data or the route has none (Login, Settings, Search), `!self.ui.images.loading()`, and at least 1 s has passed since start, or 20 s have passed regardless, it sends `ViewportCommand::Screenshot(egui::UserData::new(Shot))` (`struct Shot;`, private). In `logic`, an `egui::Event::Screenshot` whose `user_data` downcasts to `Shot` is saved as PNG to the path (`image::save_buffer`, RGBA8) and the app quits (`self.quit(ctx)`); a save error is logged and it quits anyway. Screenshots with other user data are left alone (Task 3's transition takes its own).
  - Crate docs list both switches: `TIDASH_SCREENSHOT=<file.png>` (spec §6.2) and `TIDASH_THEME=<dark|light|system|filename>` (read by Task 3; never saved).
- [ ] **Step 4: Run the tests; they pass.** `cargo test -p tidash` and `cargo clippy --all-targets -- -D warnings`
- [ ] **Step 5: Commit.** `feat(app): a screenshot switch for looking at the UI`
- [ ] **Step 6 (controller): references and baseline.**
  - Pick ids from the stored session: one album, one playlist, one artist, one mix. Build release (`cargo build --release -p tidash`); for Home and each id run `TIDASH_SCREENSHOT=<scratchpad>\shots\before-<page>.png target\release\tidash.exe tidal://<kind>/<id>`.
  - TIDAL 2.43 references: launch `%LOCALAPPDATA%\TIDAL\TIDAL.exe tidal://<kind>/<id>` for the same pages (it navigates by itself), size its window to 1440×900 with `SetWindowPos`, and capture it with `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)` from PowerShell into `<scratchpad>\shots\tidal-<page>.png`. No keyboard or mouse input is sent. Also capture one with a track playing if the user is playing one; otherwise the player bar's reference is the CSS in the spec.
  - Look at the references: if TIDAL draws its player play button as a bare glyph rather than a disc, amend spec §5.7 and Task 10 before Task 10 starts. (Done: it does; spec and Tasks 4, 5, 6, 9, 10 amended from the references, ledger Rulings 6–10.)

### Task 2: `theme.rs` and the settings it needs

**Files:**
- Create: `crates/app/src/theme.rs`
- Modify: `crates/app/src/main.rs` (`mod theme;`), `crates/app/src/settings.rs`, `crates/app/Cargo.toml`

**Interfaces:**
- Produces, in `theme`:
  - `#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)] pub struct Palette { pub dark: bool, pub window, panel, surface, surface_hover, surface_active, outline, text, secondary, dim, accent, accent_hover, on_accent, danger, warning, overlay, shadow: Color32 }`; `Palette::dark()`, `Palette::light()` (spec §4.1 values exactly); `impl Default` = `dark()`; `impl fastframe_theme::Palette` (`base`, and `set` matching the 16 names of `fastframe_theme::BASE_COLORS`).
  - `pub fn style(style: &mut egui::Style, p: &Palette)` (spec §4.3 text styles, §4.6 mapping, `animation_time` = `HOVER`); `pub fn apply(ctx: &egui::Context, p: &Palette)` = `ctx.all_styles_mut(|s| { style(s, p); rendering().apply_to_visuals(&mut s.visuals) })`; `pub fn rendering() -> fastframe_text::TextRendering` (`detect()` once, `OnceLock`).
  - Fonts: `pub fn small() / body() / body_strong() / label() / heading() / title() -> egui::FontId` (spec §4.3 via `fastframe_fonts::Weight::X.font_id`).
  - Constants (`f32` unless noted): `SIDEBAR` 240, `SIDEBAR_COLLAPSED` 78, `TOP_BAR` 56, `PLAYER_BAR` 88, `PLAYER_PAD` 18, `SIDE_PANEL` 360, `SIDE_PANEL_MIN` 280, `SIDE_PANEL_MAX` 560, `PAGE_PAD` 20, `MODULE_GAP` 40, `ROW` 48, `ROW_HEADER` 36, `INDEX_COL` 42, `COVER_COL` 42, `ROW_COVER` 40, `CARD` 175, `CARD_GAP` 16, `CARDS_MAX: usize` 8, `HEADER_COVER` 240, `BACKDROP` 450, `TINT_HEADER` 340, `PLAYER_COVER` 52, `SIDEBAR_COVER` 36, `QUEUE_COVER` 40, `ICON_BUTTON` 32, `TRANSPORT` 40, `CARD_CONTROL` 36, `PILL` 40, `SEARCH_W` 280, `R_ART` 2, `R_ROW` 5, `R_BADGE` 4, `R_FIELD` 8, `R_CARD` 12, `R_DIALOG` 20, `HOVER` 0.15, `OVERLAY` 0.3, `TINT_FADE` 0.45, `TINT_PX: u32` 16.
  - `pub enum Tier { Max, Lossless, Low }`, `Tier::of(o: &crate::model::OutputInfo) -> Tier` (FLAC, case-insensitive, above 16 bit or 48 kHz → Max; FLAC otherwise → Lossless; any other codec → Low); `Palette::tier(&self, t: Tier) -> Color32` (spec §4.2; Low is `text`).
  - `pub fn blend(a: Color32, b: Color32, t: f32) -> Color32` (per channel, alpha too); `pub fn dominant(image: &egui::ColorImage) -> Option<Color32>` (spec §5.9's weighting; `None` when every pixel is transparent); `Palette::tint(&self, art: Color32) -> Color32` (lightness 0.30 dark / 0.72 light, hue and saturation kept); `pub fn card_columns(width: f32) -> (usize, f32)` (spec §4.4: `n` clamped to 1..=8 and the card width).
- Produces, in `settings`: `Theme { System, Dark, Light }` (default `Dark`); `Settings` gains `custom_theme: Option<String>`, `custom_theme_cache: Option<fastframe_theme::CustomTheme<theme::Palette>>` (`#[serde(deserialize_with = "fastframe_theme::read_cached_theme", skip_serializing_if = "Option::is_none")]`), `accent_from_art: bool` (default `true`), `sidebar_collapsed: bool` (default `false`); `Settings`' `Default` becomes a hand-written impl for that `true`; `pub fn themes_dir() -> Option<PathBuf>` (`<config>/themes`).

- [ ] **Step 1: Write the failing tests** (in `theme.rs` unless noted):
  - `every_base_colour_is_settable`: for each name in `BASE_COLORS`, `parse_palette::<Palette>(r#"{"colors":{"<name>":"#123456"}}"#)` succeeds and differs from `Palette::dark()`; `{"colors":{"chat":"#123456"}}` fails.
  - `presets_parse`: `fastframe_theme::presets::themes::<Palette>()` yields 8 themes; Nord's `window` is `#2e3440`.
  - `apply_maps_the_palette`: after `style(&mut s, &Palette::light())`: `panel_fill == window`, `window_fill == overlay`, `override_text_color == Some(text)`, `widgets.hovered.bg_fill` is opaque, `text_styles[Heading].size == 19.0`, `spacing.scroll.floating == false`.
  - `tiers`: lossy AAC 16/44.1 → Low; FLAC 16/44.1 → Lossless; FLAC 24/96 and 16/96 → Max.
  - `dominant_colour_prefers_the_vivid_patch`: 16×16 mid-grey with a 4×4 `#e02020` patch → red channel above green and blue by 100+; all transparent → `None`. `tint_reaches_its_lightness`: `Palette::dark().tint(red)` has HSL lightness 0.30 ± 0.02, `light()` 0.72 ± 0.02.
  - `card_columns_clamp`: width 0 and 100 → 1 card, the full width; 175×3 + 32 → 3; 5000 → 8.
  - In `settings.rs` `settings_load_save_and_defaults` gains: the default theme is Dark, `accent_from_art` true, `sidebar_collapsed` false; `{"theme":"Light"}` loads Light; `{"custom_theme":"Nord.json","custom_theme_cache":{"filename":7}}` loads with the cache `None` and the rest intact; a cache round-trips through save and load.
- [ ] **Step 2: Run them; they fail to compile.** `cargo test -p tidash theme:: settings::`
- [ ] **Step 3: Implement** to the interfaces above, `fastframe-theme` added. `main.rs` still sets egui's theme from `settings.theme` until Task 3 (map `System` to `ThemePreference::System`).
- [ ] **Step 4: Run the tests; they pass;** clippy clean.
- [ ] **Step 5: Commit.** `feat(app): a palette with TIDAL's colours, sizes and type`

### Task 3: Applying the palette; the theme catalog and the Appearance settings

**Files:**
- Modify: `crates/app/src/main.rs` (`App`: catalog, `resolve_theme`; `setup_look`; drop `egui_theme`)
- Modify: `crates/app/src/ui/mod.rs` (`UiState`, `Ctx`, `draw`), `crates/app/src/ui/testing.rs` (`Ctx` gets the palette)
- Modify: `crates/app/src/messages.rs`, `crates/app/src/model.rs`, `crates/app/src/backend.rs`, `crates/app/src/ui/settings.rs`

**Interfaces:**
- Consumes: Task 1 `Switches::theme`; Task 2 `Palette`, `apply`, settings fields, `themes_dir`.
- Produces:
  - `Ctx { .., pub palette: theme::Palette }` (by value, `Copy`).
  - `UiState { .., palette: Palette, applied: bool, transition: fastframe_theme::Transition }` and `UiState::set_palette(&mut self, ctx: &egui::Context, wanted: Palette)`: on a window's first call it applies at once; afterwards a different `wanted` begins the transition (`Reveal::Band`) and applies once `holding` is false; `ui::draw` ends with `state.transition.paint(ctx)`. `pub fn palette(&self) -> Palette`.
  - `fn pick(settings: &Settings, ctx_dark: bool, switch: Option<&str>, find: impl Fn(&str) -> Option<Palette>) -> Palette` in `main.rs`: the switch (`dark`, `light`, `system` → by `ctx_dark`, else a filename via `find`, falling through when not found), then `custom_theme_cache`'s palette when `custom_theme` is set, then `dark()`/`light()` by `ctx_dark`.
  - `Event::Themes { names: Vec<String>, status: Option<fastframe_theme::Status> }` → `Model { themes: Vec<String>, theme_status: Option<Status> }`; `Event::RescanThemes`; `Action::OpenThemesFolder`.
  - `settings::status_text(status: &Status, selected: &str) -> String` in `ui/settings.rs` (spec §3.5 copy).
- Behaviour (spec §3.4, §3.5):
  - `App::new` creates the catalog, `enable_desktop_themes(DesktopThemes { slug: "tidash", omarchy_template: fastframe_theme::omarchy::BASE_TEMPLATE, omarchy_previous_templates: &[], presets: true })`, and `start(themes_dir, settings.custom_theme, &Waker::new(wake))` when there is a config dir.
  - `pump` each pass: `catalog.poll()`; if `settings.custom_theme` is set and the catalog has that file with a palette other than the cache's (or the cache names another file), new settings with the cache refilled go through `Event::Settings` and `Action::SetSettings`; `Event::Themes` is applied when `(picker filenames, catalog.status(selected))` differ from the model's. `RescanThemes` restarts the scan.
  - `App::draw` calls `self.ui.set_palette(ctx, pick(..))` before `ui::draw`. `ctx.set_theme` follows `settings.theme` (`System` → `ThemePreference::System`) in `attach` and when the setting changes, as today.
  - `setup_look` keeps fonts and `quit_shortcuts`; `theme::apply` replaces its visuals code.
  - Settings › Appearance: a `ComboBox` labelled "Theme": "Follow system", "Dark", "Light", a separator, then `model.themes` by `fastframe_theme::display_name`; choosing a built-in clears `custom_theme` and the cache, choosing a file sets `custom_theme` (the cache fills itself). Beside it the "Open themes folder" button (`Action::OpenThemesFolder`: the backend `create_dir_all`s `themes_dir` and `open::that_detached`s it). Under it `status_text` in weak text when `theme_status` is set. Then the checkbox "Colour from album art" (`accent_from_art`). Settings emits `Event::RescanThemes` on the pass it lists itself afresh (`state.listed` false → true).

- [ ] **Step 1: Write the failing tests.**
  - `main.rs` `pick_order`: switch `"light"` beats a cached Nord; a cached Nord with `custom_theme` set beats `ctx_dark`; `custom_theme` `None` ignores a stale cache; `Theme::System` with `ctx_dark` false gives `light()`; switch `"missing.json"` falls through to the cache.
  - `ui/mod.rs` `quick_changes_end_on_the_last_palette`: two passes apply Dark; `set_palette` Light, then Nord on the next pass; run passes with `RawInput::time` advancing 0.1 s each up to 0.6 s (no screenshot arrives); `ctx.style().visuals.panel_fill` is Nord's `window` and `state.palette()` is Nord.
  - `ui/settings.rs` `picking_a_theme_file_sets_custom_theme`: model with `themes = ["Nord.json"]`; open the combo box by its "Theme" label, click "Nord"; the emitted settings have `custom_theme == Some("Nord.json")`; then clicking "Light" emits `theme == Light`, `custom_theme == None`, cache `None`.
  - `main.rs` `a_selected_theme_that_vanishes_keeps_its_colours`: settings with `custom_theme = Some("Gone.json")` and a cached palette; `pick` with a `find` that knows nothing returns the cached palette; `status_text(&Status::SelectedUnavailable, "Gone.json")` is "Gone can't be read; its last colours stay."
  - `settings_open_rescans_the_themes`: drawing Settings after another route emits `RescanThemes` once.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement** to the interfaces and behaviour above. Every `Ctx` built in `ui/mod.rs` and `testing.rs` carries `state.palette()` (tests: `Palette::dark()`).
- [ ] **Step 4: Run all tests; they pass;** clippy clean.
- [ ] **Step 5: Commit.** `feat(app): themes: the palette applied, presets and theme files, the picker`
- [ ] **Step 6 (controller):** release build; screenshots of Home with `TIDASH_THEME=dark`, `light`, `Nord.json`. Stock widgets must already show the palettes (black canvas in Dark). Copy a preset to `Broken.json` with invalid JSON, select it via settings.json, launch: the cached colours stay and Settings shows the status line.

### Task 4: Icons and the widget kit

**Files:**
- Create: `crates/app/src/ui/widgets.rs`, `crates/app/assets/icons/*.svg`, `crates/app/assets/icons/LICENSE.txt`
- Modify: `Cargo.toml` (workspace `egui_extras` + patch), `crates/app/Cargo.toml`, `crates/app/src/theme.rs` (`Icon`), `crates/app/src/main.rs` (`setup_look` installs loaders), `crates/app/src/ui/mod.rs` (`mod widgets;`)

**Interfaces:**
- Produces:
  - `theme::Icon` via `fastframe_icons::icons!` (prefix `"tidash-icon-"`, directory `../assets/icons/`). Lucide shared: `ChevronLeft, ChevronRight, ChevronDown, Search, Settings, Plus, Info, Pause, Play, VolumeX, Volume2, X, Check, Ellipsis, CircleAlert, User, Clock`. App SVGs (copied from Spotifast's `assets/icons/`, Lucide-derived, MIT; or from Lucide 0.4xx upstream, ISC): `House, Compass, Library, ListMusic, ListPlus, Disc3, Music, Radio, AudioLines, Heart, HeartFilled, PlayFilled, PauseFilled, SkipBackFilled, SkipForwardFilled, Shuffle, Repeat, Repeat1, Volume1`; from Lucide upstream: `MicVocal, PanelLeft, FolderPlus, Folder, FolderOpen`. `LICENSE.txt` carries Lucide's ISC and Spotifast's MIT notices.
  - `setup_look`: `egui_extras::install_image_loaders(ctx)` then `fastframe_icons::install::<theme::Icon>(ctx)`.
  - In `ui::widgets` (each allocates, paints from `p: &Palette`, sets `WidgetInfo` with `label`, shows `label` as tooltip where it has no text, draws `focus_ring` when focused, and returns the `Response`):
    - `icon_button(ui, p, icon: Icon, label: &str, active: bool) -> Response`: `ICON_BUTTON` square, 16 px glyph; `secondary`, `text` when hovered or `active`; an `active` one gets a `surface_active` disc; hover scale 1.1 and press 0.9, eased over `HOVER`.
    - `circle_button(ui, p, icon, label, diameter: f32, fill: Color32, glyph: Color32) -> Response`: a disc, glyph 0.55 of the diameter, same scaling.
    - `pill_button(ui, p, icon: Option<Icon>, label: &str, primary: bool) -> Response`: `PILL` tall, padding 20, `label()` font; primary `accent` / `on_accent` (`accent_hover` hovered), secondary `surface` / `text` (hovered `surface_hover` over `surface`).
    - `nav_row(ui, p, icon, label: &str, selected: bool, collapsed: bool) -> Response`: spec §5.2 (radius `R_CARD`, padding 8, 24 px icon, `label()` text; collapsed: icon only, full height).
    - `search_field(ui, p, text: &mut String, id: egui::Id, hint: &str, width: f32) -> Response`: a `TextEdit` with that `id` inside a painted 40 px pill; focused, the pill fills with `text` and the glyphs use `window`.
    - `badge(ui, p, text: &str, color: Color32) -> Response`: a pill (radius half its height), `small()` text in `color` on `color` at 10% alpha, 1 px stroke of `color` at 50%, padding 12×6.
    - `vertical_gradient(painter: &egui::Painter, rect: Rect, top: Color32, bottom: Color32)`: a four-vertex `Mesh`.
    - `focus_ring(ui, response: &Response, radius: f32)`: 2 px `accent` stroke expanded 2.
- [ ] **Step 1: Write the failing tests** (`widgets.rs`, headless with AccessKit, through `ui::testing`):
  - `widgets_take_focus_and_activate_from_the_keyboard`: a row of `icon_button` "Shuffle", `pill_button` "Play", `nav_row` "Home"; three Tab presses focus them in order (`ctx.memory(|m| m.focused())` matches each response id); Enter on each reports `clicked()`.
  - `icon_button_is_labelled`: the AccessKit tree has a button named "Shuffle"; `testing::click` on it reports `clicked()`.
  - `search_field_keeps_its_id`: typing "abc" into `search_field` with id `X` leaves "abc" in the string and `ctx.memory(|m| m.has_focus(X))` after a click.
  - `icons_load`: after `install_image_loaders` and `install::<Icon>`, `ctx.try_load_bytes(Icon::PlayFilled.uri())` succeeds for every `Icon::ALL`.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement** to the interfaces above.
- [ ] **Step 4: Run all tests; they pass;** clippy clean; `cargo build --release -p tidash` and note the binary's size before and after (resvg comes in here) in the commit body.
- [ ] **Step 5: Commit.** `feat(app): icons and the widget kit`

### Task 5: Top bar, search pill and the error strip

**Files:**
- Create: `crates/app/src/ui/top_bar.rs`
- Modify: `crates/app/src/ui/mod.rs` (`draw`, `UiState`), `crates/app/src/ui/search.rs`, `crates/app/src/ui/sidebar.rs` (Back/Forward and Search leave)

**Interfaces:**
- Consumes: Task 4 widgets; Task 2 constants.
- Produces: `top_bar::show(ui: &mut egui::Ui, ctx: &mut Ctx, search: &mut search::State)`; `search::State` keeps `query`, now drawn by the top bar; `search::show(ui, ctx)` draws results only. `search::query_id()` unchanged.
- Behaviour (spec §5.3): inside the `CentralPanel`, logged in, before the route: a `TOP_BAR` row with `PAGE_PAD` sides: on the left, Back and Forward `icon_button`s side by side on one `surface` pill (labels "Back", "Forward", disabled at the history's ends); right-aligned, the `search_field` (hint "Search TIDAL", width `SEARCH_W`, shrinking to the room left) and then the gear `icon_button` (`Settings` icon, label "Settings", active on the Settings route). The sidebar loses Back, Forward, Search and its Settings row (ledger Ruling 1). Enter with a non-blank query sends `Action::Search(q)` and navigates to `Route::Search`. The offline notice and errors move from the top panel into a strip under the bar (logged out it stays a top panel): `CircleAlert` icon in `warning`, text, an `X` `icon_button` labelled "Dismiss" per error. The central panel's frame has no inner margin; pages pad themselves by `PAGE_PAD` (`ui::page` adds it; Login and Settings, and anything drawn outside `ui::page`, add it themselves).
- [ ] **Step 1: Write the failing tests** (`top_bar.rs`):
  - `enter_searches_and_opens_the_search_page`: at Home, click the field (`testing::middle` of the "Search TIDAL" field), type "massive attack", Enter: an `Action::Search("massive attack")` is sent and `Event::Navigate(Route::Search)` is emitted; a blank query sends nothing.
  - `the_gear_stays_in_a_narrow_window`: at 640 px wide, the "Settings" button's rect lies inside the window and right of the field's.
  - The existing `ctrl_f_focuses_search_and_typing_mutes_shortcuts`, `the_query_leaves_with_the_session`, `two_dismissals_in_one_frame_remove_both` pass unchanged.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run all tests; they pass;** clippy clean.
- [ ] **Step 5: Commit.** `feat(app): TIDAL's top bar: history, the search pill, settings`
- [ ] **Step 6 (controller):** screenshot Home and the album; compare the top bar with `tidal-*.png`.

### Task 6: Sidebar: navigation, the collection, the library tree, collapsing

**Files:**
- Modify: `crates/app/src/ui/sidebar.rs`, `crates/app/src/ui/library.rs`, `crates/app/src/ui/collection.rs`

**Interfaces:**
- Consumes: Task 4 `nav_row`, `icon_button`; `settings.sidebar_collapsed` (Task 2).
- Produces: `sidebar::show(ui, ctx, library)` unchanged in signature; `library::show(ui, ctx, state, collapsed: bool)`.
- Behaviour (spec §5.2): `egui::Panel::show_switched` between `Panel::left("sidebar")` (`exact_size(SIDEBAR)`) and `Panel::left("sidebar-collapsed")` (`exact_size(SIDEBAR_COLLAPSED)`), not resizable, `window` fill, 1 px `outline` right edge, no egui separator. Top: the `PanelLeft` `icon_button` ("Collapse sidebar" / "Expand sidebar") flips `sidebar_collapsed` through `Event::Settings` + `Action::SetSettings`. Nav rows Home (`House`), Explore (`Compass`); "Collection" header (`small()`, `dim`, sentence case); rows Playlists (`ListMusic`), Albums (`Disc3`), Tracks (`Music`), Artists (`User`), Mixes (`Radio`) to `Route::Collection(tab)`, selected on that tab; "All playlists" header with `ListPlus` "New playlist" and `FolderPlus` "New folder" icon buttons; then the tree in its own scroll. Playlist rows: `SIDEBAR_COVER` cover (`R_ART`, `super::cover`), title `body_strong()`, subtitle `small()` `secondary` (`tr_n` "{n} track"/"{n} tracks" from `number_of_tracks`, or "Playlist"), hover `surface_hover`, selected `surface_active`, radius `R_CARD`; folders: `egui::collapsing_header::CollapsingState` with a custom header row (`Folder`/`FolderOpen` icon, name, chevron). Collapsed (spec §5.2): icons and covers only, label as tooltip and AccessKit name. Menus and dialogs as today. Collection drops its tab row and titles the page with the tab's name in `title()`.
- [ ] **Step 1: Write the failing tests.**
  - `sidebar.rs` `collection_rows_open_their_tab`: clicking "Albums" emits `Navigate(Collection(Albums))`; at `Collection(Albums)` the "Albums" row reports selected (`testing::shown` + AccessKit `is_selected`).
  - `sidebar.rs` `collapsing_flips_the_setting_and_rows_still_work`: clicking "Collapse sidebar" emits settings with `sidebar_collapsed: true` and sends `SetSettings`; with it collapsed, "Home" and the playlist "Playlist" are still clickable by label and navigate.
  - The `library.rs` tests (`expanding_a_folder_loads_it_once`, `new_playlist_dialog_sends_create`, `delete_folder_asks_first`) pass unchanged; `collection.rs` `views_render_every_state` passes with the tab names removed from its expected texts where they moved.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run all tests; they pass;** clippy clean.
- [ ] **Step 5: Commit.** `feat(app): TIDAL's sidebar, collapsible`
- [ ] **Step 6 (controller):** screenshots with the sidebar expanded and (settings.json edited, restored after) collapsed.

### Task 7: Track lists

**Files:**
- Modify: `crates/app/src/ui/tracks.rs`, `crates/app/src/ui/queue_panel.rs`

**Interfaces:**
- Consumes: Task 2 `ROW`, `ROW_HEADER`, `INDEX_COL`, `COVER_COL`, `ROW_COVER`, `R_ROW`, `Tier`, `Palette::tier`; `Ctx.palette`.
- Produces: `tracks::ROW_H` = `theme::ROW`; `paint_row(ui, ctx: &mut Ctx, row: &Response, num: String, t: &tidal::Track, album: bool, current: Option<Color32>)` — `current` is the playing row's tier colour; `album` draws the cover column and the album column; colours, images and requests come from `ctx`. `queue_panel::show(ui, ctx: &mut Ctx)` (the side panel already builds a `Ctx`).
- Behaviour (spec §5.5): a `ROW_HEADER` column header above every list but the queue ("#", "Title", "Artist", "Album" when `album`, a `Clock` icon over durations; `small()` `secondary`); rows: hover `surface_hover` radius `R_ROW` faded over `HOVER`; index: number in `secondary`, on hover a 16 px `PlayFilled` in `text`, playing an `AudioLines` in the tier colour; cover column `ROW_COVER` (`R_ART`) when `album`; title `body_strong()` `text` (tier colour when playing); artist and album `body()` `text`; duration `body()` `secondary`, right-aligned; unavailable rows at 50% alpha. Column split as today (title 50%, artist 25%, album 25% of the rest). Disc rows: `Disc3` + "Disc n". Drop line 2 px `accent`. Queue rows: the same painter, `QUEUE_COVER`, no column header, no album column.
- [ ] **Step 1: Write the failing tests** (`tracks.rs`):
  - `long_titles_stay_in_their_columns`: a track titled 300 × "W" and an Arabic title of 200 characters, at 800 px: every text shape of the row (from `out.shapes`) ends left of the next column's start.
  - `the_playing_row_takes_its_tier_colour`: with `playback.track` = row 2 and a 24/96 FLAC `output`, row 2's title text shape has `Palette::dark().tier(Tier::Max)`.
  - Existing tests in `tracks.rs`, `playlist.rs` (drag reorder uses `ROW_H`) and `queue_panel.rs` pass; `autoscroll_bounces_between_the_ends_of_the_page` passes (raise its 3000 pt/s if the taller rows need more passes, keeping its assertions).
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run all tests; they pass;** clippy clean.
- [ ] **Step 5: Commit.** `feat(app): TIDAL's track lists`
- [ ] **Step 6 (controller):** screenshot the playlist and the album; compare rows with the references.

### Task 8: Cards, shelves and grids

**Files:**
- Modify: `crates/app/src/ui/card.rs`, `crates/app/src/ui/shelf.rs`, `crates/app/src/ui/browse.rs`, `crates/app/src/ui/search.rs`

**Interfaces:**
- Consumes: Task 2 `card_columns`, `CARD_GAP`, `MODULE_GAP`, `OVERLAY`; Task 4 `icon_button`, `vertical_gradient`, `focus_ring`.
- Produces: `card::card(ui, p: &Palette, images, tx, image, title, subtitle, round, width: f32) -> Response`; `card::height(width: f32) -> f32` (`width + 8 + 2 × 22`).
- Behaviour (spec §5.4, §5.6): cards as spec §5.6 (cover at full width, radius `R_ART` or round; title `body_strong()`, subtitle `body()` `secondary`, one line each; hover/focus gradient over the cover's lower half, transparent → black 70%, faded over `OVERLAY`; no fill). Shelves: title `heading()`; right-aligned "View all" (`body()`, `secondary`, `text` hovered) and `ChevronLeft`/`ChevronRight` `icon_button`s ("Scroll left"/"Scroll right") that scroll the row by its visible width (`ScrollArea::horizontal` with `scroll_bar_visibility(AlwaysHidden)`, its offset set from the buttons), cards sized by `card_columns(row width)`; `MODULE_GAP` between shelves. Grids: `card_columns`, rows virtualized with `card::height`. Page titles (`browse.rs`) in `title()`.
- [ ] **Step 1: Write the failing tests.**
  - `shelf.rs` `cards_fill_the_row`: a Cards shelf of 12 albums at 800 px wide content: the first card's rect is `card_columns(width).1` wide and the fourth starts at `3 × (w + CARD_GAP)`.
  - `shelf.rs` `scroll_right_moves_the_row`: clicking "Scroll right" changes the shelf's horizontal offset by about the visible width over the next passes.
  - Existing `shelf.rs`, `browse.rs`, `search.rs`, `collection.rs`, `artist.rs` tests pass.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run all tests; they pass;** clippy clean.
- [ ] **Step 5: Commit.** `feat(app): TIDAL's cards, shelves and grids`
- [ ] **Step 6 (controller):** screenshot Home, Explore and Search; compare with the references.

### Task 9: Page headers and the blurred backdrop

**Files:**
- Modify: `crates/app/src/ui/header.rs`, `crates/app/src/ui/album.rs`, `crates/app/src/ui/artist.rs`, `crates/app/src/ui/playlist.rs`, `crates/app/src/ui/mix.rs`

**Interfaces:**
- Consumes: Task 4 `pill_button`, `circle_button`, `vertical_gradient`; Task 2 `HEADER_COVER`, `BACKDROP`, `TINT_PX`.
- Produces: `header::header(ui, ctx, kind: &str, image, title, details, play, fav, extra)` — `kind` is the type line ("Album", "Playlist", "Mix", "Artist"; `tr`, drawn upper-cased); `header::play_button` becomes a primary `pill_button` with `PlayFilled`; page actions (`extra`) use secondary `pill_button`s.
- Behaviour (spec §5.4): the artist page is a hero instead (spec §5.4's first bullet: the picture full width over the top `BACKDROP`, cropped by `super::cover`, requested at that width, fading into `window` over its lower 40%; name, details and actions over its bottom-left; no round cover). Album, playlist and mix: before the header, the cover at `TINT_PX` px is painted stretched over the page's full width (the scroll's clip rect, ignoring `PAGE_PAD`) and `BACKDROP` tall, under `vertical_gradient` from `window` at 60% alpha to `window`; it scrolls with the page. Then the `HEADER_COVER` cover (round for artists), the type line in `small()` `dim`, the title in `title()`, details in `body()` `secondary`, and the action row: Play pill, heart `circle_button` 40 (`Heart`/`HeartFilled`, labels as today, `surface` fill), page pills.
- [ ] **Step 1: Write the failing tests.**
  - `album.rs` `the_backdrop_asks_for_the_small_cover`: drawing an album page requests the cover at `TINT_PX` px as well as at the header's size (an `Action::LoadImage` with `px == 16` is sent).
  - Existing album, artist, playlist and mix tests pass (labels "Play", "Add to favorites", "Edit", "Delete", "Add to playlist", "Artist radio" unchanged).
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run all tests; they pass;** clippy clean.
- [ ] **Step 5: Commit.** `feat(app): page headers over a blurred cover`
- [ ] **Step 6 (controller):** screenshot the album, playlist, artist and mix pages; compare with the references.

### Task 10: Player bar

**Files:**
- Modify: `crates/app/src/ui/player_bar.rs`

**Interfaces:**
- Consumes: Task 4 widgets; Task 2 `Tier`, `PLAYER_BAR`, `PLAYER_PAD`, `PLAYER_COVER`, `TRANSPORT`.
- Produces: `player_bar::show(ui, model, tx, player, state, images, side, p: &Palette, fill: Color32)` — `fill` is the bar's colour (`p.panel` until Task 11 tints it).
- Behaviour (spec §5.7): `Panel::bottom("player")` `exact_size(PLAYER_BAR)`, `fill`, 1 px `outline` top edge, no separator; three columns 1 : 1.5 : 1 by `ui.columns`-free fixed rects from the bar's width. Left: cover, title `label()` and artists `small()` `secondary`, both truncated to the column. Centre: Shuffle and Repeat `icon_button`s (`active` when on; Repeat shows `Repeat1` for One), Previous/Next as `circle_button`s 40 with no fill (`SkipBackFilled`/`SkipForwardFilled`, `text` glyph), Play/Pause a bare 24 px `PlayFilled`/`PauseFilled` in `text` on a `TRANSPORT` hit box with no fill (a `circle_button` with a transparent fill; TIDAL 2.43 draws no disc); under them, at most 800 wide, times (`small()`, `secondary`) around the stock seek `Slider`. Right: the `badge` with `badge(o)`'s text and `p.tier(Tier::of(o))`, hover text as today; Queue (`ListMusic`), Lyrics (`MicVocal`), Credits (`Info`) `icon_button`s, `active` for the open panel; the volume icon (`VolumeX` at 0, `Volume1` below 0.5, else `Volume2`, label "Volume") and a 90 px stock slider. Every label as today.
- [ ] **Step 1: Write the failing tests.**
  - `a_long_title_keeps_the_transport_in_place`: at 800 px wide, the "Play" button's rect is the same with a 5-character title and a 400-character title, and no title text shape crosses into the centre column.
  - The badge test(s) and `player_bar_toggles_play_cycles_repeat_and_picks_the_side_panel` pass unchanged.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run all tests; they pass;** clippy clean.
- [ ] **Step 5: Commit.** `feat(app): TIDAL's player bar`
- [ ] **Step 6 (controller):** play a track, screenshot (a short script: launch with a `tidal://track/<id>` link, then press nothing; if playback needs a click, defer this shot to the acceptance session).

### Task 11: Album-art tint

**Files:**
- Modify: `crates/app/src/images.rs` (`TextureCache`), `crates/app/src/ui/mod.rs` (`draw`), `crates/app/src/ui/player_bar.rs` (caller side only)

**Interfaces:**
- Consumes: Task 2 `dominant`, `Palette::tint`, `blend`, `TINT_PX`, `TINT_HEADER`, `TINT_FADE`; Task 4 `vertical_gradient`; Task 10 `fill`.
- Produces: `TextureCache::tint(&self, key: &str) -> Option<Color32>` (the dominant colour of that image's `TINT_PX` texture, stored by `insert` when `px == TINT_PX`, dropped with it).
- Behaviour (spec §5.9): with `settings.accent_from_art` and a current track whose album has a cover, `draw` requests that cover at `TINT_PX` (through `super::cover`'s request path, off screen) and, once `tint` has it, `tint = p.tint(colour)`. Player bar `fill` = `blend(p.panel, tint, 0.12)`, each channel through `ctx.animate_value_with_time(id.with(channel), value, TINT_FADE)`; without a tint, `p.panel` the same way. Coverless routes (Home, `Page`, `ViewAll`, `Collection`, `Search`): before the route draws, `vertical_gradient` over the central panel's top `TINT_HEADER` from `blend(p.window, tint, 0.35)` to `p.window`.
- [ ] **Step 1: Write the failing tests.**
  - `images.rs` `a_small_image_keeps_its_tint`: inserting a 16 px red image stores a red tint under its key; a 64 px insert stores none; evicting the 16 px texture drops it.
  - `ui/mod.rs` `no_cover_no_tint`: a playing track without a cover, or `accent_from_art` off, leaves the player bar's fill `Palette::dark().panel` (read from the panel's shape) after the animation settles.
- [ ] **Step 2: Run them; they fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run all tests; they pass;** clippy clean; with `TIDASH_REPAINT_LOG=1` (controller, Step 6) a paused, settled window logs no passes.
- [ ] **Step 5: Commit.** `feat(app): colour from album art`
- [ ] **Step 6 (controller):** screenshot Home with a track playing; idle-pass check.

### Task 12: Side panel, Settings, login, dialogs

**Files:**
- Modify: `crates/app/src/ui/mod.rs` (side panel frame), `crates/app/src/ui/lyrics.rs`, `crates/app/src/ui/credits.rs`, `crates/app/src/ui/queue_panel.rs` (title), `crates/app/src/ui/settings.rs`, `crates/app/src/ui/login.rs`, `crates/app/src/ui/library.rs` (dialogs only)

**Interfaces:**
- Consumes: earlier tasks' widgets and constants.
- Behaviour (spec §5.8): side panel `default_size(SIDE_PANEL)`, `size_range(SIDE_PANEL_MIN..=SIDE_PANEL_MAX)`, `window` fill, 1 px `outline` left edge, `PAGE_PAD` margin, its title in `heading()`. Lyrics: synced lines in `label()` at 16 px, current `text`, others `dim`; unsynced in `body()`; provider in `small()` `dim`. Credits: role headers `small()` `secondary`, names as links. Settings: each `section` a `heading()` with `MODULE_GAP` above; rows with the label left and the control right-aligned; Apply and Log out as `pill_button`s (primary, secondary). Login: centred column, primary `pill_button` "Log in with TIDAL", the Settings link. Dialogs: titles in `heading()`, OK as a primary pill, Cancel secondary.
- [ ] **Step 1: Write the failing test.** `lyrics.rs` `the_current_line_is_brighter`: with synced lines and a position in line 3, line 3's text shape is `text` and line 2's is `dim`.
- [ ] **Step 2: Run it; it fails.**
- [ ] **Step 3: Implement;** existing lyrics, credits, settings, login and library dialog tests pass unchanged.
- [ ] **Step 4: Run all tests; they pass;** clippy clean; `grep -rn "Color32::from\|Color32::[A-Z]" crates/app/src/ui --include=*.rs` finds only tests.
- [ ] **Step 5: Commit.** `feat(app): side panel, settings and login in the new look`
- [ ] **Step 6 (controller):** screenshot Settings, the login page (`TIDASH_SCREENSHOT` with no session is not possible without logging out: skip, acceptance covers it), the queue and lyrics panels.

### Task 13: Docs, budgets and acceptance

**Files:**
- Modify: `docs/superpowers/specs/2026-10-07-tidash-design.md` (§7 Theme and shell lines, §12, §15, per the design-system spec §8), `docs/follow-ups.md`, `crates/app/po/tidash.pot`, `README.md` (themes folder, one line), this plan (outcome)

- [ ] **Step 1: Main spec and follow-ups.** Apply the design-system spec's §8 edits. Follow-ups gain: card play and favourite buttons (need a play-by-id backend action), the track row's "…" button, Inter's and Lucide's licence files in the packages (M5), and anything the controller's screenshot checks deferred.
- [ ] **Step 2: Strings.** Every new `tr` string is in `tidash.pot`; `cargo test -p tidash` (the i18n build check) passes.
- [ ] **Step 3 (controller): budgets.** Release build, three launches each: first frame time, working set on Home after 15 s, idle passes with `TIDASH_REPAINT_LOG=1` (pointer away), binary size; compare with spec §10's last numbers and record them in its §10.
- [ ] **Step 4: Commit.** `docs: design system outcome and follow-ups`
- [ ] **Step 5: Human acceptance** (one session, spec §6.3): Tidash beside TIDAL on Home, an album, a playlist, Search; switch Follow system / Dark / Light / Nord live and watch the reveal; edit a palette file, then break it; play tracks with different covers and watch the bar's tint follow, then turn "Colour from album art" off; collapse and expand the sidebar; Tab through the top bar, sidebar, a list and the player bar with the focus ring visible; hover and press cards, rows and buttons. Results go into this plan's outcome and the follow-ups.
