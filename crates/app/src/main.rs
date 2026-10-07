//! Tidash desktop app.

mod backend;
mod i18n;
mod images;
mod model;
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
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_look(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let (actions, events, player) = backend::start(move || ctx.request_repaint());
        let mut ui = ui::UiState::default();
        ui.player = player;
        Self { model: Model::default(), actions, events, ui }
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                Event::Image { cover, size, image } => self.ui.images.insert(ui.ctx(), cover, size, image),
                ev => self.model.apply(ev),
            }
        }
        for ev in ui::draw(ui, &self.model, &self.actions, &mut self.ui) {
            self.model.apply(ev);
        }
    }
}

fn main() -> eframe::Result {
    let _ = fastframe_log::Logging::new("tidash", env!("CARGO_PKG_VERSION"))
        .filter("warn,tidash=info")
        .init();
    eframe::run_native(
        "tidash",
        eframe::NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
