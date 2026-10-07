//! tidalfast desktop app.

mod backend;
mod i18n;
mod model;
mod ui;

use model::{Action, Event, Model};
use std::sync::mpsc::Receiver;
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
        let (actions, events) = backend::start(move || ctx.request_repaint());
        Self { model: Model::default(), actions, events, ui: ui::UiState::default() }
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
            self.model.apply(ev);
        }
        for ev in ui::draw(ui, &self.model, &self.actions, &mut self.ui) {
            self.model.apply(ev);
        }
    }
}

fn main() -> eframe::Result {
    let _ = fastframe_log::Logging::new("tidalfast", env!("CARGO_PKG_VERSION"))
        .filter("warn,tidalfast=info")
        .init();
    eframe::run_native(
        "tidalfast",
        eframe::NativeOptions::default(),
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}
