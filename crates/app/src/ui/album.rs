//! Album page: cover, header, play button, track list with disc headers.

use super::{cover, tracks};
use crate::i18n::{tr, tr_n};
use crate::images::TextureCache;
use crate::model::{Action, Model};
use egui::vec2;
use tokio::sync::mpsc::UnboundedSender;

const COVER: f32 = 320.0;

pub fn show(
    ui: &mut egui::Ui,
    model: &Model,
    id: u64,
    tx: &UnboundedSender<Action>,
    images: &mut TextureCache,
) {
    let Some((album, list)) = model.albums.get(&id) else {
        if model.album_errors.contains(&id) {
            ui.label(tr("Couldn't load this album"));
            if ui.button(tr("Retry")).clicked() {
                let _ = tx.send(Action::OpenAlbum(id));
            }
        } else {
            ui.spinner();
        }
        return;
    };
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(COVER, COVER), egui::Sense::hover());
        cover(ui, images, tx, album.cover.as_deref(), COVER as u32, rect);
        ui.vertical(|ui| {
            ui.heading(&album.title);
            ui.label(
                album
                    .artists
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            if let Some(year) = album.release_date.as_deref().and_then(|d| d.get(..4)) {
                ui.label(year);
            }
            ui.label(tr_n("{n} track", "{n} tracks", list.len() as u32));
            ui.add_space(8.0);
            if ui
                .add_enabled(!list.is_empty(), egui::Button::new(tr("Play")))
                .clicked()
            {
                let _ = tx.send(Action::PlayTracks {
                    tracks: list.clone(),
                    start: 0,
                });
            }
        });
    });
    ui.add_space(8.0);
    tracks::show(ui, "album-tracks", list, true, tx);
}
