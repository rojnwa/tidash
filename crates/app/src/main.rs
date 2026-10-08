//! Tidash desktop app.

mod backend;
mod i18n;
mod images;
mod model;
mod now_playing;
mod pages;
mod playback;
mod queue;
mod ui;

use model::{Action, Event, Model};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

struct App {
    model: Model,
    actions: UnboundedSender<Action>,
    events: Receiver<Event>,
    ui: ui::UiState,
    media: now_playing::MediaControls,
    /// When `main` began, until the first frame logs the cold-start time (spec §10).
    started: Option<Instant>,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, started: Instant) -> Self {
        setup_look(&cc.egui_ctx);
        // The images cache: the backend opens it off this thread, for both.
        let artwork = Arc::new(OnceLock::new());
        let ctx = cc.egui_ctx.clone();
        let (actions, events) = backend::start(move || ctx.request_repaint(), artwork.clone());
        let ctx = cc.egui_ctx.clone();
        let media = now_playing::MediaControls::start(move || ctx.request_repaint(), artwork);
        Self {
            model: Model::default(),
            actions,
            events,
            ui: ui::UiState::default(),
            media,
            started: Some(started),
        }
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

/// Fonts, text rendering and the dark theme, as fastframe's getting-started guide shows;
/// scroll bars that keep their place beside the content.
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
                ev => self.model.apply(ev),
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
        for ev in ui::draw(ui, &self.model, &self.actions, &mut self.ui) {
            self.model.apply(ev);
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
    eframe::run_native(
        "tidash",
        eframe::NativeOptions::default(),
        Box::new(move |cc| Ok(Box::new(App::new(cc, started)))),
    )
}
