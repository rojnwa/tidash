//! Search view: query box, virtualized album grid, virtualized track list.

use super::{cover, tracks};
use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Event, Model, Route};
use egui::{Align2, FontId, Sense, vec2};
use tokio::sync::mpsc::UnboundedSender;

const COVER: f32 = 160.0;
const CELL_W: f32 = COVER + 12.0;
const CELL_H: f32 = COVER + 44.0;

#[derive(Default)]
pub struct State {
    query: String,
}

pub fn show(
    ui: &mut egui::Ui,
    model: &Model,
    tx: &UnboundedSender<Action>,
    state: &mut State,
    images: &mut TextureCache,
    events: &mut Vec<Event>,
) {
    let edit = ui.add(
        egui::TextEdit::singleline(&mut state.query)
            .hint_text(tr("Search TIDAL"))
            .desired_width(f32::INFINITY),
    );
    let q = state.query.trim();
    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !q.is_empty() {
        let _ = tx.send(Action::Search(q.to_string()));
    }
    let Some((_, results)) = &model.search else {
        return;
    };

    ui.add_space(6.0);
    ui.heading(tr("Albums"));
    let cols = ((ui.available_width() / CELL_W) as usize).max(1);
    let n_rows = results.albums.len().div_ceil(cols);
    let grid_h = (ui.available_height() * 0.5).max(CELL_H);
    egui::ScrollArea::vertical()
        .id_salt("albums")
        .max_height(grid_h)
        .show_rows(ui, CELL_H, n_rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for a in results.albums.iter().skip(r * cols).take(cols) {
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(CELL_W, CELL_H), Sense::click());
                        let img = egui::Rect::from_min_size(
                            rect.min + vec2(6.0, 4.0),
                            vec2(COVER, COVER),
                        );
                        cover(ui, images, tx, a.cover.as_deref(), COVER as u32, img);
                        let font = FontId::proportional(13.0);
                        let clip = ui.painter_at(egui::Rect::from_min_max(
                            rect.min + vec2(6.0, COVER + 6.0),
                            rect.max - vec2(6.0, 0.0),
                        ));
                        clip.text(
                            img.left_bottom() + vec2(0.0, 4.0),
                            Align2::LEFT_TOP,
                            &a.title,
                            font.clone(),
                            ui.visuals().text_color(),
                        );
                        let by = a
                            .artists
                            .first()
                            .map(|x| x.name.as_str())
                            .unwrap_or_default();
                        clip.text(
                            img.left_bottom() + vec2(0.0, 22.0),
                            Align2::LEFT_TOP,
                            by,
                            font,
                            ui.visuals().weak_text_color(),
                        );
                        if resp.clicked() {
                            events.push(Event::Navigate(Route::Album(a.id)));
                            if !model.albums.contains_key(&a.id) {
                                let _ = tx.send(Action::OpenAlbum(a.id));
                            }
                        }
                    }
                });
            }
        });

    ui.add_space(6.0);
    ui.heading(tr("Tracks"));
    tracks::show(ui, "search-tracks", &results.tracks, false, tx);
}
