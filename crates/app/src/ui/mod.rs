//! Drawing: dismissible errors on top, then the current route.

mod album;
mod login;
mod player_bar;
mod search;
mod sidebar;
mod tracks;

use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Event, Model, Route};
use tokio::sync::mpsc::UnboundedSender;

pub struct UiState {
    login: login::State,
    search: search::State,
    bar: player_bar::State,
    pub images: TextureCache,
    /// Read only for the position; `None` without an audio device.
    pub player: Option<player::Player>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            login: Default::default(),
            search: Default::default(),
            bar: Default::default(),
            images: TextureCache::new(512),
            player: None,
        }
    }
}

/// Paints a square cover at `size` points (requested at `size` pixels), or a flat placeholder.
fn cover(ui: &egui::Ui, images: &mut TextureCache, tx: &UnboundedSender<Action>, cover: Option<&str>, size: u32, rect: egui::Rect) {
    let tex = cover.and_then(|c| {
        images.get(ui.ctx(), c, size, || {
            let _ = tx.send(Action::LoadImage { cover: c.to_string(), size });
        })
    });
    match tex {
        Some(t) => {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(t.id(), rect, uv, egui::Color32::WHITE);
        }
        None => {
            ui.painter().rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
        }
    }
}

/// Draws the model; returns events for the caller to apply (the UI never mutates `Model`).
pub fn draw(ui: &mut egui::Ui, model: &Model, tx: &UnboundedSender<Action>, state: &mut UiState) -> Vec<Event> {
    let mut events = Vec::new();
    let mut logout = false;
    if !model.errors.is_empty() || model.logged_in {
        egui::Panel::top("top").show(ui, |ui| {
            for (i, e) in model.errors.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, e);
                    if ui.small_button(tr("Dismiss")).clicked() {
                        events.push(Event::DismissError(i));
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
    if model.route != Route::Login {
        // Before the sidebar, so the bar spans the full width.
        player_bar::show(ui, model, tx, state.player.as_ref(), &mut state.bar, &mut state.images);
        sidebar::show(ui, model, &mut events);
    }

    egui::CentralPanel::default().show(ui, |ui| match model.route {
        Route::Login => login::show(ui, model, tx, &mut state.login),
        Route::Search => search::show(ui, model, tx, &mut state.search, &mut state.images, &mut events),
        Route::Album(id) => album::show(ui, model, id, tx, &mut state.images),
    });
    events
}
