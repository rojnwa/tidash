//! Tidash desktop app.

mod backend;
mod i18n;
mod images;
mod model;
mod now_playing;
mod playback;
mod queue;
mod ui;

use model::{Action, Event, Model};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

struct App {
    model: Model,
    actions: UnboundedSender<Action>,
    events: Receiver<Event>,
    ui: ui::UiState,
    media: now_playing::MediaControls,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_look(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let (actions, events, player) = backend::start(move || ctx.request_repaint());
        let mut ui = ui::UiState::default();
        ui.player = player;
        let ctx = cc.egui_ctx.clone();
        let media = now_playing::MediaControls::start(move || ctx.request_repaint());
        Self {
            model: Model::default(),
            actions,
            events,
            ui,
            media,
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

/// Fonts, text rendering and the dark theme, as fastframe's getting-started guide shows.
fn setup_look(ctx: &egui::Context) {
    let mut fonts = fastframe_fonts::FontSetup::default().definitions();
    let rendering = fastframe_text::detect();
    rendering.apply_to(&mut fonts);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals));
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
                Event::Image { cover, size, image } => {
                    self.ui.images.insert(ctx, cover, size, image)
                }
                Event::Seeked(at) => self.media.seeked(at),
                Event::Playback(p) => {
                    self.model.apply(Event::Playback(p));
                    self.media.show(&self.model.playback, position);
                }
                ev => self.model.apply(ev),
            }
        }
        for action in self.media.poll(&self.model.playback, position) {
            let _ = self.actions.send(action);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        for ev in ui::draw(ui, &self.model, &self.actions, &mut self.ui) {
            self.model.apply(ev);
        }
    }
}

fn main() -> eframe::Result {
    let _ = fastframe_log::Logging::new("tidash", env!("CARGO_PKG_VERSION"))
        // symphonia's MP4 probe matches `ftyp` and reports every file's leading box size as
        // "4 bytes of junk".
        .filter("warn,tidash=info,symphonia_core::formats::probe=error")
        .init();
    eframe::run_native(
        "tidash",
        eframe::NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
