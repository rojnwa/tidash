//! Playlist page: header with Play, then its tracks.

use super::{Ctx, header, tracks};
use crate::model::{PageData, Route};

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Some(PageData::Playlist {
        playlist: p,
        tracks: list,
    }) = super::page_data(ui, ctx, route)
    else {
        return;
    };
    super::page(ui, route, |ui| {
        let details = |ui: &mut egui::Ui, _: &mut Ctx| {
            ui.label(p.creator.as_deref().unwrap_or("TIDAL")); // a brand: not translated
            ui.label(tracks::summary(list));
        };
        header::header(ui, ctx, p.image().as_ref(), &p.title, details, Some(list));
        tracks::show(ui, ctx, "playlist-tracks", list, false);
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{Action, Event, PageData, Route};
    use crate::ui::testing::{at, every_state, key, pass, playlist, tracks};
    use egui::Key;

    fn data() -> PageData {
        PageData::Playlist {
            playlist: playlist(),
            tracks: tracks(3),
        }
    }

    #[test]
    fn views_render_every_state() {
        let route = Route::Playlist("p-1".into());
        let texts = ["Playlist", "TIDAL", "3 tracks · 10:00", "Play"];
        every_state(super::show, route, data(), &texts);
    }

    #[test]
    fn play_sends_all_tracks_from_start() {
        let route = Route::Playlist("p-1".into());
        let mut model = at(route.clone());
        model.apply(Event::Page {
            route,
            data: data(),
        });
        let egui = egui::Context::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for input in [vec![], vec![key(Key::Tab)], vec![key(Key::Enter)]] {
            pass(&egui, &tx, &model, super::show, input);
        }
        let played = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|a| match a {
            Action::PlayTracks { tracks, start } => Some((tracks.len(), start)),
            _ => None, // the artwork
        });
        assert_eq!(played, Some((3, 0)));
    }
}
