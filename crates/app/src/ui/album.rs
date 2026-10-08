//! Album page: header with Play, then the track list with disc headers.

use super::{Ctx, header, tracks};
use crate::i18n::{tr, tr_n};
use crate::model::PageData;

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx) {
    let model = ctx.model;
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
                ctx.go(route.clone()); // asks for it again
            }
        } else if slot.is_some_and(|s| s.loading) {
            ui.spinner();
        }
        return;
    };
    let mut lines = vec![tracks::names(&album.artists)];
    lines.extend(
        album
            .release_date
            .as_deref()
            .and_then(|d| d.get(..4))
            .map(str::to_owned),
    );
    lines.push(tr_n("{n} track", "{n} tracks", list.len() as u32));
    super::page(ui, route, |ui| {
        let art = album.image();
        header::header(ui, ctx, art.as_ref(), &album.title, &lines, Some(list));
        tracks::show(ui, ctx, "album-tracks", list, true);
    });
}
