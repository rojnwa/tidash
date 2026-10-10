//! Tidash desktop app.
//!
//! Debug switches for measuring frame pacing (spec §10, §14), read once at start:
//! - `TIDASH_AUTOSCROLL=<pt/s>`: scrolls the current page this many points a second, every
//!   pass, repainting without pause; it starts down (up if negative) and bounces at either end.
//!   Unset, 0 or not a number is off.
//! - `TIDASH_REPAINT_LOG=1`: logs each pass's repaint causes at info.
//!
//! The OpenGL renderer in use is logged at start either way.

mod backend;
mod i18n;
mod images;
mod library;
mod messages;
mod model;
mod now_playing;
mod pages;
mod playback;
mod queue;
mod settings;
mod ui;

use model::{Action, Event, Model};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

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

struct App {
    model: Model,
    actions: UnboundedSender<Action>,
    events: Receiver<Event>,
    ui: ui::UiState,
    media: now_playing::MediaControls,
    /// When `main` began, until the first frame logs the cold-start time (spec §10).
    started: Option<Instant>,
    repaint_log: bool,
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
    fn new(cc: &eframe::CreationContext<'_>, started: Instant, switches: Switches) -> Self {
        log_renderer(cc);
        setup_look(&cc.egui_ctx);
        // The images cache: the backend opens it off this thread, for both.
        let artwork = Arc::new(OnceLock::new());
        let ctx = cc.egui_ctx.clone();
        let (actions, events) = backend::start(move || ctx.request_repaint(), artwork.clone());
        let ctx = cc.egui_ctx.clone();
        let wake = move || ctx.request_repaint();
        let image_base = tidal::ClientConfig::default().image_base;
        let media = now_playing::MediaControls::start(wake, artwork, image_base);
        if let Some(speed) = switches.autoscroll {
            ui::autoscroll(&cc.egui_ctx, speed);
        }
        Self {
            model: Model::default(),
            actions,
            events,
            ui: ui::UiState::default(),
            media,
            started: Some(started),
            repaint_log: switches.repaint_log,
        }
    }

    /// Applies `ev` to the model; a new theme restyles the window.
    fn apply(&mut self, ctx: &egui::Context, ev: Event) {
        if let Event::Settings(s) = &ev
            && s.theme != self.model.settings.theme
        {
            ctx.set_theme(match s.theme {
                settings::Theme::Dark => egui::Theme::Dark,
                settings::Theme::Light => egui::Theme::Light,
            });
            ctx.request_repaint(); // a change from the UI's pass shows in the next
        }
        self.model.apply(ev);
    }

    /// Asks the backend for the current route's page when the model says to.
    fn open_current(&mut self) {
        if let Some(route) = self.model.open_current(Instant::now()) {
            let _ = self.actions.send(Action::Open(route));
        }
    }
}

impl Drop for App {
    /// Closing the action channel stops the backend, which first reports the current play;
    /// give that a moment (best effort) before the process exits.
    fn drop(&mut self) {
        self.actions = tokio::sync::mpsc::unbounded_channel().0;
        let deadline = Instant::now() + Duration::from_secs(3);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            if let Err(RecvTimeoutError::Disconnected) = self.events.recv_timeout(left) {
                break; // backend finished
            }
        }
    }
}

/// Fonts, text rendering and the dark theme (until the settings say otherwise), as fastframe's
/// getting-started guide shows; scroll bars that keep their place beside the content.
fn setup_look(ctx: &egui::Context) {
    let mut fonts = fastframe_fonts::FontSetup::default().definitions();
    let rendering = fastframe_text::detect();
    rendering.apply_to(&mut fonts);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        rendering.apply_to_visuals(&mut style.visuals);
        style.spacing.scroll = egui::style::ScrollStyle::solid();
    });
}

impl eframe::App for App {
    /// Backend events and media commands; also runs while the window is hidden or minimized.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let position = self
            .ui
            .player
            .as_ref()
            .map_or(Duration::ZERO, player::Player::position);
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Image { key, px, image } => self.ui.images.insert(ctx, key, px, image),
                Event::ImageFailed { key, px } => self.ui.images.forget(key, px),
                Event::Player(p) => self.ui.player = Some(p),
                Event::Seeked(at) => self.media.seeked(at),
                Event::Playback(p) => {
                    self.model.apply(Event::Playback(p));
                    self.media.show(&self.model.playback, position);
                }
                ev => {
                    self.ui.apply(ctx, &ev);
                    self.apply(ctx, ev);
                }
            }
        }
        self.open_current(); // Home after `LoggedIn`
        for action in self.media.poll(&self.model.playback, position) {
            let _ = self.actions.send(action);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if let Some(t) = self.started.take() {
            log::info!("first frame {} ms after start", t.elapsed().as_millis());
        }
        if self.repaint_log {
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

fn main() -> eframe::Result {
    let started = Instant::now();
    let _ = fastframe_log::Logging::new("tidash", env!("CARGO_PKG_VERSION"))
        // symphonia's MP4 probe matches `ftyp` and reports every file's leading box size as
        // "4 bytes of junk".
        .filter("warn,tidash=info,symphonia_core::formats::probe=error")
        .init();
    let switches = Switches::parse(|k| std::env::var(k).ok());
    // Glow, chosen by measurement (spec §14); explicit, as eframe defaults to wgpu whenever
    // anything in the graph turns on its `wgpu` feature.
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "tidash",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, started, switches)))),
    )
}

#[cfg(test)]
mod tests {
    use super::Switches;

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
