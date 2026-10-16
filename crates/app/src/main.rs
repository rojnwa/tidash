//! Tidash desktop app.
//!
//! The app outlives its window (fastframe-shell): with Close to tray on, closing the window
//! leaves playback, the media controls and the tray item running headless until the tray asks
//! for a new window.
//!
//! Debug switches for measuring frame pacing (spec §10, §14), read once at start:
//! - `TIDASH_AUTOSCROLL=<pt/s>`: scrolls the current page this many points a second, every
//!   pass, repainting without pause; it starts down (up if negative) and bounces at either end.
//!   Unset, 0 or not a number is off.
//! - `TIDASH_REPAINT_LOG=1`: logs each pass's repaint causes at info.
//!
//! The OpenGL renderer in use is logged for each window either way.
//!
//! Switches for looking at the UI (design system spec §6.2), also read once at start; blank is
//! unset:
//! - `TIDASH_SCREENSHOT=<file.png>`: the window opens at 1440×900 on the launch link's page
//!   (Home without one); once the page has loaded and no artwork is still loading, at least
//!   1 s after start (at most 20 s), it saves a screenshot of the window there and quits.
//! - `TIDASH_THEME=<dark|light|system|filename>`: the theme for this run, never saved.

mod backend;
mod discord;
mod i18n;
mod images;
mod instance;
mod library;
mod link;
mod messages;
mod model;
mod now_playing;
mod pages;
mod playback;
mod queue;
mod scrobble;
mod settings;
mod theme;
mod tray;
mod ui;

use fastframe_shell::{Closed, Headless, Held, Resident, Waker};
use link::Link;
use model::{Action, Event, Model, Route};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use theme::Palette;
use tokio::sync::mpsc::UnboundedSender;
use tray::TrayAction;

/// The switches the crate docs list.
#[derive(Clone, Debug, PartialEq)]
struct Switches {
    autoscroll: Option<f32>,
    repaint_log: bool,
    screenshot: Option<PathBuf>,
    theme: Option<String>,
}

impl Switches {
    /// From `var`, which reads an environment variable.
    fn parse(var: impl Fn(&str) -> Option<String>) -> Self {
        let text = |k| Some(var(k)?.trim().to_owned()).filter(|v| !v.is_empty());
        Self {
            autoscroll: var("TIDASH_AUTOSCROLL")
                .and_then(|v| v.trim().parse().ok())
                .filter(|v: &f32| v.is_normal()),
            repaint_log: var("TIDASH_REPAINT_LOG").is_some_and(|v| v.trim() == "1"),
            screenshot: text("TIDASH_SCREENSHOT").map(PathBuf::from),
            theme: text("TIDASH_THEME"),
        }
    }
}

/// Tags `TIDASH_SCREENSHOT`'s screenshot, so it is not taken for anyone else's.
struct Shot;

/// What lives as long as the process: the model, the backend's channels, the media controls,
/// the Discord status and the tray. Windows come and go around it (`Window`).
struct App {
    model: Model,
    actions: UnboundedSender<Action>,
    events: Receiver<Event>,
    /// The current window's: its textures belong to that window's context, so each window
    /// gets its own (`new_ui`). The player carries over.
    ui: ui::UiState,
    media: now_playing::MediaControls,
    /// With a Discord application ID compiled in.
    presence: Option<discord::Presence>,
    /// `None` where the desktop has no tray at all.
    tray: Option<fastframe_tray::Tray>,
    /// What the tray's Play/Pause entry was last set for.
    tray_playing: bool,
    /// What later launches asked for (`instance`), and this launch's own link.
    inbox: instance::Inbox,
    /// Links that came in, waiting for a login (`open_links`).
    links: Vec<Link>,
    /// No window exists: the app runs headless.
    hidden: bool,
    /// The tray asked to close the window to it.
    hide: bool,
    /// Headless, a window was asked for.
    show: bool,
    /// Ctrl+Q, the tray or the media controls asked to quit.
    quit: bool,
    /// When `main` began, until the first frame logs the cold-start time (spec §10).
    started: Option<Instant>,
    /// With `TIDASH_SCREENSHOT`, when `main` began, until the screenshot is asked for.
    shot_since: Option<Instant>,
    switches: Switches,
    /// The palette files in the themes folder and the presets (`resolve_theme`).
    catalog: fastframe_theme::Catalog<Palette>,
    /// Wakes the app when a scan of the themes folder finishes.
    theme_waker: fastframe_theme::Waker,
}

/// Which OpenGL renderer eframe's glow painter runs on.
#[expect(unsafe_code)]
fn log_renderer(cc: &eframe::CreationContext<'_>) {
    if let Some(gl) = &cc.gl {
        use eframe::glow::{self, HasContext as _};
        // SAFETY: eframe creates the app on this thread with this context current (its painter
        // has just used it); these are read-only queries of strings every OpenGL version has.
        let (renderer, vendor, version) = unsafe {
            (
                gl.get_parameter_string(glow::RENDERER),
                gl.get_parameter_string(glow::VENDOR),
                gl.get_parameter_string(glow::VERSION),
            )
        };
        log::info!("renderer: glow, {renderer} ({vendor}), OpenGL {version}");
    }
}

impl App {
    /// Starts the backend, the media controls, the Discord status and the tray, all waking
    /// whichever window exists through `waker`.
    fn new(
        waker: &Waker,
        settings: settings::Settings,
        inbox: instance::Inbox,
        started: Instant,
        switches: Switches,
    ) -> Self {
        let wake = |w: &Waker| {
            let w = w.clone();
            move || w.wake()
        };
        // The images cache: the backend opens it off this thread, for both.
        let artwork = Arc::new(OnceLock::new());
        let (actions, events) = backend::start(wake(waker), artwork.clone(), settings.clone());
        let image_base = tidal::ClientConfig::default().image_base;
        let presence = discord::app_id().map(|id| discord::Presence::start(id, image_base.clone()));
        let media = now_playing::MediaControls::start(wake(waker), artwork, image_base);
        let tray = fastframe_tray::Tray::spawn(tray::config(), wake(waker));
        let mut model = Model::default();
        model.apply(Event::Settings(settings));
        let mut catalog = fastframe_theme::Catalog::default();
        catalog.enable_desktop_themes(fastframe_theme::DesktopThemes {
            slug: "tidash",
            omarchy_template: fastframe_theme::omarchy::BASE_TEMPLATE,
            omarchy_previous_templates: &[],
            presets: true,
        });
        let mut app = Self {
            model,
            actions,
            events,
            ui: ui::UiState::default(),
            media,
            presence,
            tray,
            tray_playing: false,
            inbox,
            links: Vec::new(),
            hidden: false,
            hide: false,
            show: false,
            quit: false,
            started: Some(started),
            shot_since: switches.screenshot.as_ref().map(|_| started),
            switches,
            catalog,
            theme_waker: fastframe_theme::Waker::new(wake(waker)),
        };
        app.scan_themes();
        app
    }

    /// Lists the themes folder in the background, the selected file first; nothing without a
    /// config dir.
    fn scan_themes(&mut self) {
        if let Some(dir) = settings::themes_dir() {
            let selected = self.model.settings.custom_theme.clone();
            self.catalog.start(dir, selected, &self.theme_waker);
        }
    }

    /// The catalog's news, each pass (spec §3.4): a scan to start when a watched file changed
    /// (Linux) or one that finished; the selected file's palette into the settings' cache,
    /// saved like any change; the picker's list and status line into the model.
    fn resolve_theme(&mut self, ctx: &egui::Context) {
        if self.catalog.needs_reload() {
            self.scan_themes();
        }
        self.catalog.poll();
        let settings = &self.model.settings;
        if let Some(file) = &settings.custom_theme
            && let Some(found) = self.catalog.find(file)
            && settings.custom_theme_cache.as_ref() != Some(found)
        {
            let mut new = settings.clone();
            new.custom_theme_cache = Some(found.clone());
            self.apply(ctx, Event::Settings(new.clone()));
            let _ = self.actions.send(Action::SetSettings(new));
        }
        let status = self
            .catalog
            .status(self.model.settings.custom_theme.as_deref());
        let listed = self.catalog.picker_themes().map(|t| &t.filename);
        if status != self.model.theme_status || !listed.eq(&self.model.themes) {
            let names = self.catalog.picker_themes();
            let names = names.map(|t| t.filename.clone()).collect();
            self.model.apply(Event::Themes { names, status });
        }
    }

    /// Readies a new window: its look and UI state; on macOS the tray item comes with the
    /// first.
    fn attach(&mut self, ctx: &egui::Context) {
        setup_look(ctx);
        ctx.set_theme(self.model.settings.theme);
        if let Some(speed) = self.switches.autoscroll {
            ui::autoscroll(ctx, speed);
        }
        self.new_ui();
        self.hidden = false;
        if let Some(tray) = &mut self.tray {
            tray.attach();
        }
    }

    /// Empty UI state for the next window; the player carries over.
    fn new_ui(&mut self) {
        let player = self.ui.player.take();
        self.ui = ui::UiState::default();
        self.ui.player = player;
    }

    fn tray_shown(&self) -> bool {
        self.tray
            .as_ref()
            .is_some_and(fastframe_tray::Tray::is_shown)
    }

    fn hides_to_tray(&self) -> bool {
        let close_to_tray = self.model.settings.behavior.close_to_tray;
        tray::hides_to_tray(self.tray_shown(), close_to_tray)
    }

    /// Applies `ev` to the model; a new theme restyles the window (`draw`), Quit quits, and
    /// Settings opening lists the themes folder again.
    fn apply(&mut self, ctx: &egui::Context, ev: Event) {
        match ev {
            Event::Quit => return self.quit(ctx),
            Event::RescanThemes => return self.scan_themes(),
            _ => {}
        }
        if let Event::Settings(s) = &ev {
            let old = &self.model.settings;
            if s.theme != old.theme {
                ctx.set_theme(s.theme);
            }
            if (s.theme, &s.custom_theme) != (old.theme, &old.custom_theme) {
                ctx.request_repaint(); // a change from the UI's pass shows in the next
            }
        }
        let settings = matches!(ev, Event::Settings(_));
        self.model.apply(ev);
        if settings {
            self.show_presence(self.position()); // turned off, it clears at once
        }
    }

    /// Ends the app: the window closes and `closed` says Quit, or headless, the next tick does.
    fn quit(&mut self, ctx: &egui::Context) {
        self.quit = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    /// The player's position, now.
    fn position(&self) -> Duration {
        let player = self.ui.player.as_ref();
        player.map_or(Duration::ZERO, player::Player::position)
    }

    /// The playback at `position` in the Discord status, if the settings say so.
    fn show_presence(&mut self, position: Duration) {
        if let Some(p) = &mut self.presence {
            let on = self.model.settings.integrations.discord;
            p.show(&self.model.playback, position, on);
        }
    }

    /// Asks the backend for the current route's page when the model says to.
    fn open_current(&mut self) {
        if let Some(route) = self.model.open_current(Instant::now()) {
            let _ = self.actions.send(Action::Open(route));
        }
    }

    /// Backend events, later launches, the media controls and the tray: each window pass (also
    /// while the window is minimized) and each headless tick.
    fn pump(&mut self, ctx: &egui::Context) {
        let position = self.position();
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Image { key, px, image } => self.ui.images.insert(ctx, key, px, image),
                Event::ImageFailed { key, px } => self.ui.images.forget(key, px),
                Event::Player(p) => self.ui.player = Some(p),
                Event::Seeked(at) => {
                    self.media.seeked(at);
                    self.show_presence(at);
                }
                Event::Playback(p) => {
                    self.model.apply(Event::Playback(p));
                    self.media.show(&self.model.playback, position);
                    self.show_presence(position);
                }
                ev => {
                    self.ui.apply(ctx, &ev);
                    self.apply(ctx, ev);
                }
            }
        }
        self.resolve_theme(ctx);
        let launches =
            std::mem::take(&mut *self.inbox.lock().unwrap_or_else(PoisonError::into_inner));
        for asked in launches {
            self.handle(ctx, asked);
        }
        for asked in self.media.poll(&self.model.playback, position) {
            self.handle(ctx, asked);
        }
        let clicks = self.tray.as_ref().map(fastframe_tray::Tray::events);
        for click in clicks.unwrap_or_default() {
            self.handle(ctx, tray::action_for(&click));
        }
        open_links(&mut self.model, &mut self.links, &self.actions);
        self.open_current(); // Home after `LoggedIn`, a link's page
        let playing = matches!(self.model.playback.state, Some(player::PlayState::Playing));
        if let Some(tray) = &mut self.tray
            && self.tray_playing != playing
        {
            self.tray_playing = playing;
            tray.set_label(tray::PLAY_PAUSE, tray::play_pause_label(playing));
        }
    }

    /// Does what the tray, the media controls or a later launch asked.
    fn handle(&mut self, ctx: &egui::Context, asked: TrayAction) {
        let minimized = ctx.input(|i| i.viewport().minimized == Some(true));
        match asked {
            TrayAction::Play(action) => {
                let _ = self.actions.send(action);
            }
            TrayAction::Quit => self.quit(ctx),
            TrayAction::Open(link) => {
                self.links.push(link);
                self.handle(ctx, TrayAction::Show);
            }
            TrayAction::Toggle if !self.hidden && !minimized => {
                self.hide = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            TrayAction::Show | TrayAction::Toggle if self.hidden => self.show = true,
            TrayAction::Show | TrayAction::Toggle => {
                // A minimized window comes back before it can take the focus.
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        if let Some(t) = self.started.take() {
            log::info!("first frame {} ms after start", t.elapsed().as_millis());
        }
        if self.switches.repaint_log {
            let ctx = ui.ctx();
            let (pass, causes) = (ctx.cumulative_pass_nr(), ctx.repaint_causes());
            log::info!("pass {pass}: repaint causes {causes:?}");
        }
        let ctx = ui.ctx();
        let switch = self.switches.theme.as_deref();
        // `TIDASH_THEME=system` follows the desktop whatever the setting; egui's theme
        // follows the setting.
        let system = if switch == Some("system") {
            ctx.system_theme()
        } else {
            None
        };
        let dark = system.unwrap_or_else(|| ctx.theme()) == egui::Theme::Dark;
        let catalog = &self.catalog;
        let find = |file: &str| Some(catalog.find(file)?.palette);
        let wanted = pick(&self.model.settings, dark, switch, find);
        self.ui.set_palette(ctx, wanted);
        // The root took egui's style as this pass began: a palette applied just now draws in
        // this pass too, not only the next (which nothing may ask for).
        ui.set_style(ui.ctx().global_style());
        for ev in ui::draw(ui, &self.model, &self.actions, &mut self.ui) {
            self.apply(ui.ctx(), ev);
        }
        self.open_current();
        self.ask_for_screenshot(ui.ctx());
    }

    /// With `TIDASH_SCREENSHOT`, once: asks for the screenshot when the page has loaded and no
    /// artwork is still loading, at least 1 s after start, or 20 s after it regardless.
    fn ask_for_screenshot(&mut self, ctx: &egui::Context) {
        let Some(since) = self.shot_since else {
            return;
        };
        let waited = since.elapsed();
        let route = self.model.route();
        let page = matches!(route, Route::Login | Route::Search | Route::Settings)
            || self
                .model
                .pages
                .get(route)
                .is_some_and(|s| s.data.is_some());
        let settled = page && !self.ui.images.loading() && waited >= Duration::from_secs(1);
        if settled || waited >= Duration::from_secs(20) {
            self.shot_since = None;
            let shot = egui::UserData::new(Shot);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(shot));
        } else {
            ctx.request_repaint_after(Duration::from_millis(100)); // the clock moves on
        }
    }

    /// Saves `TIDASH_SCREENSHOT`'s screenshot once it arrives, and quits, saved or not. Other
    /// screenshots are someone else's.
    fn save_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.switches.screenshot.clone() else {
            return;
        };
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot {
                    user_data, image, ..
                } if user_data.data.as_ref().is_some_and(|d| d.is::<Shot>()) => {
                    Some(Arc::clone(image))
                }
                _ => None,
            })
        });
        let Some(image) = image else {
            return;
        };
        let [w, h] = image.size.map(|n| n as u32);
        match image::save_buffer(&path, image.as_raw(), w, h, image::ColorType::Rgba8) {
            Ok(()) => log::info!("screenshot saved to {}", path.display()),
            Err(e) => log::error!("screenshot not saved to {}: {e}", path.display()),
        }
        self.quit(ctx);
    }
}

/// What the shell around `eframe::run_native` does with the app between windows.
impl Resident for App {
    /// See `on_close`.
    fn closed(&self) -> Closed {
        let to_tray = self.hides_to_tray();
        on_close(self.quit, self.hide, to_tray, self.model.logged_in)
    }

    /// The window's textures go with it.
    fn window_gone(&mut self) {
        (self.hidden, self.hide, self.show) = (true, false, false);
        self.new_ui();
    }

    /// Playback, the media controls, the Discord status and the tray keep going until Show or
    /// Quit.
    fn headless_frame(&mut self, ctx: &egui::Context) -> Headless {
        self.pump(ctx);
        if self.quit {
            Headless::Quit
        } else if self.show {
            Headless::Show
        } else {
            Headless::Wait
        }
    }

    /// Nothing waits for a first frame: the backend started with the app.
    fn start_hidden(&mut self) -> bool {
        if !starts_hidden(&self.model.settings.behavior, self.tray_shown()) {
            return false;
        }
        self.started = None; // no first frame to time
        if let Some(tray) = &mut self.tray {
            tray.create_item(); // macOS: else made with the first window
        }
        true
    }

    /// Closing the action channel stops the backend, which first reports the current play;
    /// give that a moment (best effort) before the process exits.
    fn shutdown(&mut self) {
        self.tray = None; // gone at once
        self.actions = tokio::sync::mpsc::unbounded_channel().0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Err(RecvTimeoutError::Disconnected) = self.events.recv_timeout(left) {
                break; // backend finished
            }
        }
    }
}

/// Whether a launch starts in the tray: asked for, and with a tray to come back from.
fn starts_hidden(behavior: &settings::Behavior, tray_shown: bool) -> bool {
    behavior.start_minimized && tray::hides_to_tray(tray_shown, behavior.close_to_tray)
}

/// What closing the window does. Quit wins; otherwise it hides when the tray asked for that
/// (`hide`) or, logged in, the setting says so (`to_tray`, `hides_to_tray`): a first-run user
/// closing the login page expects the app to end.
fn on_close(quit: bool, hide: bool, to_tray: bool, logged_in: bool) -> Closed {
    if !quit && (hide || (to_tray && logged_in)) {
        Closed::Hide
    } else {
        Closed::Quit
    }
}

/// Opens the links that wait, in order, once logged in: a track's album comes from the
/// backend (`Action::OpenTrack`), the rest are routes.
fn open_links(model: &mut Model, links: &mut Vec<Link>, actions: &UnboundedSender<Action>) {
    if !model.logged_in {
        return;
    }
    for link in links.drain(..) {
        let route = match link {
            Link::Track(id) => {
                let _ = actions.send(Action::OpenTrack(id));
                continue;
            }
            Link::Album(id) => Route::Album(id),
            Link::Artist(id) => Route::Artist(id),
            Link::Playlist(id) => Route::Playlist(id),
            Link::Mix(id) => Route::Mix(id),
        };
        model.navigate(route);
    }
}

/// One window around the app; dropping it with the window hands the app back to the shell.
struct Window(Held<App>);

impl eframe::App for Window {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.pump(ctx);
        self.0.save_screenshot(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.draw(ui);
    }
}

/// The palette to show (spec §3.4, ruling 4): `TIDASH_THEME` (`switch`) when it names a
/// built-in or a file `find` knows; else the selected file, as the catalog has it or else as
/// last cached; else Dark or Light as egui's theme is (`ctx_dark`).
fn pick(
    settings: &settings::Settings,
    ctx_dark: bool,
    switch: Option<&str>,
    find: impl Fn(&str) -> Option<Palette>,
) -> Palette {
    let by_ctx = if ctx_dark {
        Palette::dark()
    } else {
        Palette::light()
    };
    let switched = switch.and_then(|s| match s {
        "dark" => Some(Palette::dark()),
        "light" => Some(Palette::light()),
        "system" => Some(by_ctx),
        file => find(file),
    });
    let custom = || {
        let file = settings.custom_theme.as_deref()?;
        find(file).or_else(|| Some(settings.custom_theme_cache.as_ref()?.palette))
    };
    switched.or_else(custom).unwrap_or(by_ctx)
}

/// Fonts and text rendering, as fastframe's getting-started guide shows (`theme::apply` gives
/// the colours, sizes and scroll bars). Ctrl+Q is the app's (`ui::draw`): egui's own closes the
/// window bare, which hides it to the tray.
fn setup_look(ctx: &egui::Context) {
    let mut fonts = fastframe_fonts::FontSetup::default().definitions();
    theme::rendering().apply_to(&mut fonts);
    ctx.set_fonts(fonts);
    // SVG only (egui_extras' `svg` feature); the icons' bytes loader never forgets them.
    egui_extras::install_image_loaders(ctx);
    fastframe_icons::install::<theme::Icon>(ctx);
    ctx.options_mut(|o| o.quit_shortcuts.clear());
}

fn main() -> eframe::Result {
    let started = Instant::now();
    // A desktop hands a link over as an argument: the first that parses is this launch's.
    let link = std::env::args_os()
        .skip(1)
        .find_map(|a| link::parse(a.to_str()?));
    let waker = Waker::default();
    let inbox = Arc::new(Mutex::new(Vec::from_iter(
        link.clone().map(TrayAction::Open),
    )));
    // Before logging (a fresh file each run), the tray and the backend: a later launch only
    // hands its request to the running one, and ends.
    let Some(_instance) = instance::claim(link.as_ref(), &waker, Arc::clone(&inbox)) else {
        return Ok(());
    };
    let _ = fastframe_log::Logging::new("tidash", env!("CARGO_PKG_VERSION"))
        // symphonia's MP4 probe matches `ftyp` and reports every file's leading box size as
        // "4 bytes of junk".
        .filter("warn,tidash=info,symphonia_core::formats::probe=error")
        .init();
    let switches = Switches::parse(|k| std::env::var(k).ok());
    // Before any window: whether one opens at start depends on them. Blocks briefly.
    let settings = settings::path().as_deref().map(settings::Settings::load);
    let settings = settings.unwrap_or_default();
    // A screenshot needs a window.
    let screenshot = switches.screenshot.is_some();
    let hidden = settings.behavior.start_minimized && !screenshot;
    let app = App::new(&waker, settings, inbox, started, switches);
    let shell = fastframe_shell::Shell::new(app, &waker);
    // A status item answers only while AppKit's loop runs; elsewhere the shell sleeps on the waker.
    #[cfg(target_os = "macos")]
    let shell = shell.idle(fastframe_tray::idle);
    shell.start_hidden(hidden).run(|lease| {
        // The app ID names the Linux desktop entry and MPRIS bus, as in `now_playing`.
        let mut viewport = eframe::egui::ViewportBuilder::default()
            .with_app_id("tidash")
            .with_min_inner_size(theme::MIN_WINDOW);
        if screenshot {
            viewport = viewport.with_inner_size([1440.0, 900.0]); // as TIDAL's references
        }
        // Glow, chosen by measurement (spec §14); explicit, as eframe defaults to wgpu
        // whenever anything in the graph turns on its `wgpu` feature.
        let options = eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            viewport,
            ..Default::default()
        };
        eframe::run_native(
            "Tidash",
            options,
            Box::new(move |cc| {
                log_renderer(cc);
                let mut app = lease.take(&cc.egui_ctx);
                app.attach(&cc.egui_ctx);
                Ok(Box::new(Window(app)))
            }),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Action, Event, Link, Model, Route, Switches, on_close, open_links, pick, starts_hidden,
    };
    use crate::settings::{Behavior, Settings, Theme};
    use crate::theme::Palette;
    use fastframe_shell::Closed;
    use fastframe_theme::{CustomTheme, Status};
    use std::path::PathBuf;

    fn nord() -> CustomTheme<Palette> {
        let mut presets = fastframe_theme::presets::themes::<Palette>();
        presets
            .find(|t| t.filename == "Nord.json")
            .expect("Nord ships")
    }

    /// The switch, then the selected file (the catalog's copy, else the cache), then Dark or
    /// Light as egui's theme is (ruling 4).
    #[test]
    fn pick_order() {
        let nord = nord();
        let cached = Settings {
            custom_theme: Some("Nord.json".into()),
            custom_theme_cache: Some(nord.clone()),
            ..Settings::default()
        };
        let none = |_: &str| None;
        assert_eq!(pick(&cached, true, Some("light"), none), Palette::light());
        assert_eq!(pick(&cached, false, None, none), nord.palette, "the cache");
        let fresh = |f: &str| (f == "Nord.json").then_some(Palette::light());
        assert_eq!(
            pick(&cached, true, None, fresh),
            Palette::light(),
            "the file"
        );
        let stale = Settings {
            custom_theme: None,
            ..cached.clone()
        };
        assert_eq!(pick(&stale, true, None, none), Palette::dark(), "stale");
        let system = Settings {
            theme: Theme::System,
            ..Settings::default()
        };
        assert_eq!(pick(&system, false, None, none), Palette::light());
        let missing = pick(&cached, true, Some("missing.json"), none);
        assert_eq!(missing, nord.palette, "falls through");
        let mine = |f: &str| (f == "Mine.json").then_some(nord.palette);
        let switched = pick(&Settings::default(), true, Some("Mine.json"), mine);
        assert_eq!(switched, nord.palette);
        assert_eq!(pick(&cached, false, Some("system"), none), Palette::light());
        assert_eq!(pick(&stale, false, Some("dark"), none), Palette::dark());
    }

    /// Spec §3.5: a selected file the catalog no longer lists keeps its last colours, and
    /// Settings says why.
    #[test]
    fn a_selected_theme_that_vanishes_keeps_its_colours() {
        let palette = nord().palette;
        let settings = Settings {
            custom_theme: Some("Gone.json".into()),
            custom_theme_cache: Some(CustomTheme {
                filename: "Gone.json".into(),
                palette,
            }),
            ..Settings::default()
        };
        assert_eq!(pick(&settings, true, None, |_| None), palette);
        let said = crate::ui::settings::status_text(&Status::SelectedUnavailable, "Gone.json");
        assert_eq!(said, "Gone can't be read; its last colours stay.");
    }

    /// Closing the login page quits, Close to tray or not: a first-run user expects the app
    /// to end. Logged in, the setting hides; Quit and the tray's own hide win either way.
    #[test]
    fn closing_the_login_page_quits() {
        // on_close(quit, hide, to_tray, logged_in)
        assert_eq!(
            on_close(false, false, true, false),
            Closed::Quit,
            "login page"
        );
        assert_eq!(on_close(false, false, true, true), Closed::Hide);
        assert_eq!(
            on_close(false, false, false, true),
            Closed::Quit,
            "setting off"
        );
        assert_eq!(on_close(true, true, true, true), Closed::Quit, "quit asked");
        assert_eq!(
            on_close(false, true, false, false),
            Closed::Hide,
            "the tray's hide"
        );
    }

    /// Links that come in before the login open after it, in order: a track's album comes
    /// from the backend.
    #[test]
    fn a_link_waits_for_login() {
        let (actions, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let mut model = Model::default();
        let mut links = vec![
            Link::Artist(2),
            Link::Track(7),
            Link::Playlist("p".into()),
            Link::Mix("m".into()),
            Link::Album(1),
        ];
        open_links(&mut model, &mut links, &actions);
        assert_eq!(links.len(), 5, "kept until logged in");
        assert_eq!(model.history, [Route::Login]);
        assert!(sent.try_recv().is_err());
        model.apply(Event::LoggedIn(1));
        open_links(&mut model, &mut links, &actions);
        assert!(links.is_empty());
        let want = [
            Route::Home,
            Route::Artist(2),
            Route::Playlist("p".into()),
            Route::Mix("m".into()),
            Route::Album(1),
        ];
        assert_eq!(model.history, want);
        assert!(matches!(sent.try_recv(), Ok(Action::OpenTrack(7))));
        assert!(sent.try_recv().is_err());
    }

    #[test]
    fn start_hidden_only_with_a_tray() {
        let behavior = |close_to_tray, start_minimized| Behavior {
            close_to_tray,
            start_minimized,
        };
        assert!(starts_hidden(&behavior(true, true), true));
        assert!(!starts_hidden(&behavior(true, true), false), "no tray host");
        assert!(
            !starts_hidden(&behavior(false, true), true),
            "closing quits"
        );
        assert!(!starts_hidden(&behavior(true, false), true), "not asked");
    }

    #[test]
    fn switches_parse_and_default_off() {
        let parse = |vars: &[(&str, &str)]| {
            Switches::parse(|k| {
                let v = vars.iter().find(|(name, _)| *name == k);
                v.map(|(_, v)| v.to_string())
            })
        };
        let off = Switches {
            autoscroll: None,
            repaint_log: false,
            screenshot: None,
            theme: None,
        };
        assert_eq!(parse(&[]), off);
        let on = [
            ("TIDASH_AUTOSCROLL", "1500"),
            ("TIDASH_REPAINT_LOG", "1"),
            ("TIDASH_SCREENSHOT", r"C:\x\home.png"),
            ("TIDASH_THEME", " Nord.json "),
        ];
        let want = Switches {
            autoscroll: Some(1500.0),
            repaint_log: true,
            screenshot: Some(PathBuf::from(r"C:\x\home.png")),
            theme: Some("Nord.json".into()),
        };
        assert_eq!(parse(&on), want);
        for blank in ["", "  "] {
            assert_eq!(parse(&[("TIDASH_SCREENSHOT", blank)]), off, "{blank:?}");
            assert_eq!(parse(&[("TIDASH_THEME", blank)]), off, "{blank:?}");
        }
        assert_eq!(
            parse(&[("TIDASH_AUTOSCROLL", "-200.5")]).autoscroll,
            Some(-200.5)
        );
        for junk in ["0", "fast", "", "NaN", "inf"] {
            assert_eq!(parse(&[("TIDASH_AUTOSCROLL", junk)]), off, "{junk:?}");
        }
        assert_eq!(parse(&[("TIDASH_REPAINT_LOG", "0")]), off);
    }
}
