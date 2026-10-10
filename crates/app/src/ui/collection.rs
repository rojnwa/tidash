//! Collection: a tab each for the favorite tracks (a list with Play), albums, artists,
//! playlists and mixes (card grids).

use super::{Ctx, header, shelf, tracks};
use crate::i18n::tr;
use crate::model::{CollectionTab, PageData, Route};

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Route::Collection(at) = route else { return };
    super::padded(ui, |ui| {
        ui.heading(tr("Collection"));
        ui.horizontal(|ui| {
            for (tab, label) in [
                (CollectionTab::Tracks, tr("Tracks")),
                (CollectionTab::Albums, tr("Albums")),
                (CollectionTab::Artists, tr("Artists")),
                (CollectionTab::Playlists, tr("Playlists")),
                (CollectionTab::Mixes, tr("Mixes")),
            ] {
                if ui.selectable_label(*at == tab, label).clicked() {
                    ctx.go(Route::Collection(tab));
                }
            }
        });
    });
    ui.add_space(8.0);
    let Some(data) = super::page_data(ui, ctx, route) else {
        return;
    };
    super::page(ui, route, |ui| match data {
        PageData::Tracks(list) => {
            ui.horizontal(|ui| {
                header::play_button(ui, ctx, list);
                ui.label(tracks::summary(list));
            });
            ui.add_space(8.0);
            tracks::show(ui, ctx, "collection-tracks", list, false, None);
        }
        PageData::Items(items) => shelf::grid(ui, items, ctx),
        _ => {} // the loader fills a collection tab with nothing else
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{CollectionTab, PageData, Route};
    use crate::ui::testing::{album, every_state, mix, tracks};
    use tidal::Item;

    #[test]
    fn views_render_every_state() {
        let route = Route::Collection(CollectionTab::Tracks);
        let data = PageData::Tracks(tracks(2));
        let texts = ["Collection", "Tracks", "Mixes", "Play", "2 tracks · 6:40"];
        every_state(super::show, route, data, &texts);
        let route = Route::Collection(CollectionTab::Albums);
        let data = PageData::Items(vec![Item::Album(album(1))]);
        every_state(super::show, route, data, &["Album 1, Artist 1, Artist 2"]);
        let route = Route::Collection(CollectionTab::Mixes);
        let data = PageData::Items(vec![Item::Mix(mix())]);
        every_state(super::show, route, data, &["Mix, Daily"]);
    }
}
