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
    /// Read only for the position; `None` until the output opened, or without a device.
    pub player: Option<player::Player>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            login: Default::default(),
            search: Default::default(),
            bar: Default::default(),
            images: TextureCache::new(64 << 20),
            player: None,
        }
    }
}

/// Paints `image` over `rect`, requested as wide as `rect` is in pixels; or a flat placeholder.
fn cover(
    ui: &egui::Ui,
    images: &mut TextureCache,
    tx: &UnboundedSender<Action>,
    image: Option<&tidal::Image>,
    rect: egui::Rect,
) {
    let px = (rect.width() * ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
    let tex = image.and_then(|img| {
        images.get(ui.ctx(), img.key(), px, || {
            let _ = tx.send(Action::LoadImage {
                image: img.clone(),
                px,
            });
        })
    });
    match tex {
        Some(t) => {
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(t.id(), rect, uv, egui::Color32::WHITE);
        }
        None => {
            ui.painter()
                .rect_filled(rect, 4.0, ui.visuals().faint_bg_color);
        }
    }
}

/// Keyboard and mouse shortcuts. None while typing but the mouse's back and forward; Space
/// also leaves a focused widget alone (it clicks it).
fn shortcuts(ui: &egui::Ui, model: &Model, tx: &UnboundedSender<Action>, events: &mut Vec<Event>) {
    use egui::{Key, Modifiers, PointerButton};
    let ctx = ui.ctx();
    let typing = ctx.text_edit_focused();
    let focused = ctx.memory(|m| m.focused().is_some());
    let pb = &model.playback;
    let send = |a: Action| {
        let _ = tx.send(a);
    };
    let find = ctx.input_mut(|i| {
        let mut key = |mods: Modifiers, k: Key| !typing && i.consume_key(mods, k);
        let find = key(Modifiers::COMMAND, Key::F);
        let back = key(Modifiers::ALT, Key::ArrowLeft);
        let forward = key(Modifiers::ALT, Key::ArrowRight);
        if pb.track.is_some() {
            if key(Modifiers::COMMAND, Key::ArrowLeft) {
                send(Action::Previous);
            }
            if key(Modifiers::COMMAND, Key::ArrowRight) {
                send(Action::Next);
            }
            if !focused && key(Modifiers::NONE, Key::Space) {
                // ponytail: Play/Pause by state until Task 12's TogglePlay
                let busy = matches!(
                    pb.state,
                    Some(player::PlayState::Playing | player::PlayState::Buffering)
                );
                send(if busy { Action::Pause } else { Action::Play });
            }
        }
        if back || i.pointer.button_pressed(PointerButton::Extra1) {
            events.push(Event::Back);
        }
        if forward || i.pointer.button_pressed(PointerButton::Extra2) {
            events.push(Event::Forward);
        }
        find
    });
    if find {
        events.push(Event::Navigate(Route::Search));
        ctx.memory_mut(|m| m.request_focus(search::query_id()));
    }
}

/// Draws the model; returns events for the caller to apply (the UI never mutates `Model`).
pub fn draw(
    ui: &mut egui::Ui,
    model: &Model,
    tx: &UnboundedSender<Action>,
    state: &mut UiState,
) -> Vec<Event> {
    let mut events = Vec::new();
    if *model.route() != Route::Login {
        shortcuts(ui, model, tx, &mut events);
    }
    let mut logout = false;
    if !model.errors.is_empty() || model.logged_in {
        egui::Panel::top("top").show(ui, |ui| {
            if model.offline {
                let warn = ui.visuals().warn_fg_color;
                ui.colored_label(warn, tr("Offline — showing saved pages"));
            }
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
    if *model.route() != Route::Login {
        // Before the sidebar, so the bar spans the full width.
        player_bar::show(
            ui,
            model,
            tx,
            state.player.as_ref(),
            &mut state.bar,
            &mut state.images,
        );
        sidebar::show(ui, model, &mut events);
    }

    egui::CentralPanel::default().show(ui, |ui| match model.route() {
        Route::Login => login::show(ui, model, tx, &mut state.login),
        Route::Search => search::show(
            ui,
            model,
            tx,
            &mut state.search,
            &mut state.images,
            &mut events,
        ),
        Route::Album(_) => album::show(ui, model, tx, &mut state.images, &mut events),
        // ponytail: the views of Tasks 9–11 replace this
        _ => {
            ui.label(tr("Coming soon"));
        }
    });
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    /// Ctrl+F opens Search with its box focused; while typing there, Alt+← is not Back.
    #[test]
    fn ctrl_f_focuses_search_and_typing_mutes_shortcuts() {
        let ctx = egui::Context::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = Model::default();
        model.apply(Event::LoggedIn);
        let mut pass = |model: &Model, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut out = Vec::new();
            let mut full = ctx.run_ui(input, |ui| out = draw(ui, model, &tx, &mut state));
            full.textures_delta.clear(); // nothing renders them
            out
        };
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let alt_left = || vec![key(Key::ArrowLeft, Modifiers::ALT)];
        assert!(matches!(pass(&model, alt_left())[..], [Event::Back]));
        for ev in pass(&model, vec![key(Key::F, Modifiers::COMMAND)]) {
            model.apply(ev);
        }
        assert_eq!(model.route(), &Route::Search);
        pass(&model, vec![]); // the box appears and takes the focus
        assert!(ctx.text_edit_focused());
        let events = pass(&model, alt_left());
        assert!(events.is_empty(), "{events:?}");
    }
}
