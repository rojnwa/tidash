//! Drawing: dismissible errors on top, then the current route.

mod album;
mod card;
mod header;
mod login;
mod player_bar;
mod rows;
mod search;
mod shelf;
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
    scrolling: fastframe_scroll::Scrolling,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            login: Default::default(),
            search: Default::default(),
            bar: Default::default(),
            images: TextureCache::new(64 << 20),
            player: None,
            scrolling: Default::default(),
        }
    }
}

/// What every view draws with.
pub struct Ctx<'a> {
    pub model: &'a Model,
    pub tx: &'a UnboundedSender<Action>,
    pub images: &'a mut TextureCache,
    pub events: &'a mut Vec<Event>,
}

impl Ctx<'_> {
    pub fn send(&self, action: Action) {
        let _ = self.tx.send(action);
    }

    pub fn go(&mut self, route: Route) {
        self.events.push(Event::Navigate(route));
    }
}

/// A route's one vertical scroll; each route keeps its own offset, and the ids inside (a
/// shelf's sideways offset, say) are the route's own: egui does not salt them by the scroll's.
pub fn page<R>(ui: &mut egui::Ui, route: &Route, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::ScrollArea::vertical()
        .id_salt(("page", route))
        .auto_shrink([false, false])
        .show(ui, |ui| ui.push_id(route, f).inner)
        .inner
}

/// Paints `image` over `rect` with corners of `radius`, requested as many pixels wide as it
/// is drawn; or a flat placeholder. Nothing out of view is requested. Artwork of another
/// shape (a wide playlist image) is cropped around its centre, not stretched.
fn cover(
    ui: &egui::Ui,
    images: &mut TextureCache,
    tx: &UnboundedSender<Action>,
    image: Option<&tidal::Image>,
    rect: egui::Rect,
    radius: f32,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    // Wide renditions are 3:2: cropped, they need as many pixels as they are tall.
    let wide = matches!(
        image,
        Some(tidal::Image::Tidal {
            kind: tidal::ImageKind::PlaylistWide,
            ..
        })
    );
    let width = if wide {
        rect.width().max(rect.height() * 1.5)
    } else {
        rect.width()
    };
    let px = (width * ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
    let tex = image.and_then(|img| {
        images.get(ui.ctx(), img.key(), px, || {
            let _ = tx.send(Action::LoadImage {
                image: img.clone(),
                px,
            });
        })
    });
    let painter = ui.painter();
    match tex {
        Some(t) => {
            let uv = crop(t.aspect_ratio(), rect.aspect_ratio());
            let shape = egui::epaint::RectShape::filled(rect, radius, egui::Color32::WHITE)
                .with_texture(t.id(), uv);
            painter.add(shape);
        }
        None => {
            painter.rect_filled(rect, radius, ui.visuals().faint_bg_color);
        }
    }
}

/// The centred part of a texture `tex` wide per unit of height that fills a rect `rect` wide
/// per unit of height, in texture coordinates.
fn crop(tex: f32, rect: f32) -> egui::Rect {
    let size = if tex > rect {
        egui::vec2(rect / tex, 1.0)
    } else {
        egui::vec2(1.0, tex / rect)
    };
    egui::Rect::from_center_size(egui::pos2(0.5, 0.5), size)
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
    // First, before any scroll area reads the input.
    state.scrolling.apply(ui.ctx());
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

    egui::CentralPanel::default().show(ui, |ui| {
        let mut ctx = Ctx {
            model,
            tx,
            images: &mut state.images,
            events: &mut events,
        };
        match model.route() {
            Route::Login => login::show(ui, model, tx, &mut state.login),
            Route::Search => search::show(ui, &mut ctx, &mut state.search),
            Route::Album(_) => album::show(ui, &mut ctx),
            // ponytail: the views of Tasks 10–11 replace this
            _ => {
                ui.label(tr("Coming soon"));
            }
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

    /// A 3:2 image in a square keeps its middle two thirds; a square one is shown whole.
    #[test]
    fn wide_art_is_cropped_not_stretched() {
        let r = |x0: f32, x1: f32, y0: f32, y1: f32| egui::Rect::from_x_y_ranges(x0..=x1, y0..=y1);
        assert_eq!(
            crop(1.5, 1.0),
            r(0.5 - 1.0 / 3.0, 0.5 + 1.0 / 3.0, 0.0, 1.0)
        );
        assert_eq!(
            crop(1.0, 1.5),
            r(0.0, 1.0, 0.5 - 1.0 / 3.0, 0.5 + 1.0 / 3.0)
        );
        assert_eq!(crop(1.0, 1.0), r(0.0, 1.0, 0.0, 1.0));
    }
}
