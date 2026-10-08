//! Album page: header (artists link to their pages) with Play, then the track list with disc
//! headers.

use super::{Ctx, header, tracks};
use crate::model::{PageData, Route};

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Some(PageData::Album {
        album,
        tracks: list,
    }) = super::page_data(ui, ctx, route)
    else {
        return;
    };
    super::page(ui, route, |ui| {
        let details = |ui: &mut egui::Ui, ctx: &mut Ctx| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (n, a) in album.artists.iter().enumerate() {
                    if n > 0 {
                        ui.label(", ");
                    }
                    if ui.link(&a.name).clicked() {
                        ctx.go(Route::Artist(a.id));
                    }
                }
            });
            if let Some(year) = album.release_date.as_deref().and_then(|d| d.get(..4)) {
                ui.label(year);
            }
            ui.label(tracks::summary(list));
        };
        header::header(
            ui,
            ctx,
            album.image().as_ref(),
            &album.title,
            details,
            Some(list),
        );
        tracks::show(ui, ctx, "album-tracks", list, true);
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{PageData, Route};
    use crate::ui::testing::{album, every_state, tracks};

    #[test]
    fn views_render_every_state() {
        let mut list = tracks(2);
        list[1].volume_number = 2;
        let data = PageData::Album {
            album: album(1),
            tracks: list,
        };
        let texts = [
            "Album 1",
            "Artist 1",
            "Artist 2",
            "2020",
            "2 tracks · 6:40",
            "Play",
            "Disc 2",
        ];
        every_state(super::show, Route::Album(1), data, &texts);
    }
}
