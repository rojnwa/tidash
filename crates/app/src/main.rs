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
mod tray;
mod ui;

use fastframe_shell::{Closed, Headless, Held, Resident, Waker};
use link::Link;
use model::{Action, Event, Model, Route};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tray::TrayAction;

/// The debug switches the crate docs list.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Switches {
    autoscroll: Option<f32>,
    repaint_log: bool,
}

impl Switches {
    /// From `var`, which reads an environment variable.
    fn parse(var: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            autoscroll: var("TIDASH_AUTOSCROLL")
                .and_then(|v| v.trim().parse().ok())
                .filter(|v: &f32| v.is_normal()),
            repaint_log: var("TIDASH_REPAINT_LOG").is_some_and(|v| v.trim() == "1"),
        }
    }
}

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
    switches: Switches,
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
        Self {
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
            switches,
        }
    }

    /// Readies a new window: its look and UI state; on macOS the tray item comes with the
    /// first.
    fn attach(&mut self, ctx: &egui::Context) {
        setup_look(ctx, self.model.settings.theme);
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

    /// Applies `ev` to the model; a new theme restyles the window, and Quit quits.
    fn apply(&mut self, ctx: &egui::Context, ev: Event) {
        if let Event::Quit = ev {
            return self.quit(ctx);
        }
        if let Event::Settings(s) = &ev
            && s.theme != self.model.settings.theme
        {
            ctx.set_theme(egui_theme(s.theme));
            ctx.request_repaint(); // a change from the UI's pass shows in the next
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
        for ev in ui::draw(ui, &self.model, &self.actions, &mut self.ui) {
            self.apply(ui.ctx(), ev);
        }
        self.open_current();
    }
}

/// What the shell around `eframe::run_native` does with the app between windows.
impl Resident for App {
    /// Quit wins; otherwise a closed window hides to the tray when the tray asked for that or
    /// the setting says so.
    fn closed(&self) -> Closed {
        if !self.quit && (self.hide || self.hides_to_tray()) {
            Closed::Hide
        } else {
            Closed::Quit
        }
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
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.draw(ui);
    }
}

fn egui_theme(theme: settings::Theme) -> egui::Theme {
    match theme {
        settings::Theme::Dark => egui::Theme::Dark,
        settings::Theme::Light => egui::Theme::Light,
    }
}

/// Fonts, text rendering and `theme`, as fastframe's getting-started guide shows; scroll bars
/// that keep their place beside the content. Ctrl+Q is the app's (`ui::draw`): egui's own
/// closes the window bare, which hides it to the tray.
fn setup_look(ctx: &egui::Context, theme: settings::Theme) {
    let mut fonts = fastframe_fonts::FontSetup::default().definitions();
    let rendering = fastframe_text::detect();
    rendering.apply_to(&mut fonts);
    ctx.set_fonts(fonts);
    ctx.options_mut(|o| o.quit_shortcuts.clear());
    ctx.set_theme(egui_theme(theme));
    ctx.all_styles_mut(|style| {
        rendering.apply_to_visuals(&mut style.visuals);
        style.spacing.scroll = egui::style::ScrollStyle::solid();
    });
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
    let hidden = settings.behavior.start_minimized;
    let app = App::new(&waker, settings, inbox, started, switches);
    fastframe_shell::Shell::new(app, &waker)
        .idle(fastframe_tray::idle)
        .start_hidden(hidden)
        .run(|lease| {
            // Glow, chosen by measurement (spec §14); explicit, as eframe defaults to wgpu
            // whenever anything in the graph turns on its `wgpu` feature.
            let options = eframe::NativeOptions {
                renderer: eframe::Renderer::Glow,
                ..Default::default()
            };
            eframe::run_native(
                "tidash",
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
    use super::{Action, Event, Link, Model, Route, Switches, open_links, starts_hidden};
    use crate::settings::Behavior;

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
        };
        assert_eq!(parse(&[]), off);
        let on = [("TIDASH_AUTOSCROLL", "1500"), ("TIDASH_REPAINT_LOG", "1")];
        let want = Switches {
            autoscroll: Some(1500.0),
            repaint_log: true,
        };
        assert_eq!(parse(&on), want);
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
