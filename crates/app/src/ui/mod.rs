//! Drawing: dismissible errors on top, then the current route.

mod login;

use crate::i18n::tr;
use crate::model::{Action, Event, Model, Route};
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default)]
pub struct UiState {
    login: login::State,
}

/// Draws the model; returns events for the caller to apply (the UI never mutates `Model`).
pub fn draw(ui: &mut egui::Ui, model: &Model, tx: &UnboundedSender<Action>, state: &mut UiState) -> Vec<Event> {
    let mut dismiss = None;
    let mut logout = false;
    if !model.errors.is_empty() || model.logged_in {
        egui::Panel::top("top").show(ui, |ui| {
            for (i, e) in model.errors.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, e);
                    if ui.small_button(tr("Dismiss")).clicked() {
                        dismiss = Some(i);
                    }
                });
            }
            if model.logged_in {
                logout = ui.small_button(tr("Log out")).clicked();
            }
        });
    }
    if logout {
        let _ = tx.send(Action::Logout);
    }

    egui::CentralPanel::default().show(ui, |ui| match model.route {
        Route::Login => login::show(ui, model, tx, &mut state.login),
        // ponytail: real views arrive in tasks 18-20.
        Route::Search | Route::Album(_) => {
            ui.label(tr("Logged in. Search and playback arrive in the next tasks."));
        }
    });
    dismiss.map(Event::DismissError).into_iter().collect()
}
