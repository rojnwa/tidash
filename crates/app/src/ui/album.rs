//! Album page: cover, header, play button, track list with disc headers.

use super::{cover, tracks};
use crate::i18n::{tr, tr_n};
use crate::images::TextureCache;
use crate::model::{Action, Event, Model, PageData};
use egui::vec2;
use tokio::sync::mpsc::UnboundedSender;

const COVER: f32 = 320.0;

pub fn show(
    ui: &mut egui::Ui,
    model: &Model,
    tx: &UnboundedSender<Action>,
    images: &mut TextureCache,
    events: &mut Vec<Event>,
) {
    let route = model.route();
    let slot = model.pages.get(route);
    let Some(PageData::Album {
        album,
        tracks: list,
    }) = slot.and_then(|s| s.data.as_ref())
    else {
        if slot.is_some_and(|s| s.failed) {
            ui.label(tr("Couldn't load this album"));
            if ui.button(tr("Retry")).clicked() {
                events.push(Event::Navigate(route.clone())); // asks for it again
            }
        } else if slot.is_some_and(|s| s.loading) {
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
