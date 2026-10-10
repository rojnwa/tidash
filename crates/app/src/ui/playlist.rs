//! Playlist page: header with Play, then its tracks.

use super::{Ctx, header, tracks};
use crate::model::{PageData, Route};

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Some(PageData::Playlist {
        playlist: p,
        tracks: list,
        ..
    }) = super::page_data(ui, ctx, route)
    else {
        return;
    };
    super::page(ui, route, |ui| {
        let details = |ui: &mut egui::Ui, _: &mut Ctx| {
            ui.label(p.creator.as_deref().unwrap_or("TIDAL")); // a brand: not translated
            ui.label(tracks::summary(list));
        };
        // Ruling R19: no heart on the user's own, whose folder remove could drop it.
        let own = p.creator_id.is_some() && p.creator_id == ctx.model.user_id;
        let fav = (!own).then(|| tidal::Favorite::Playlist(p.uuid.clone()));
        header::header(
            ui,
            ctx,
            p.image().as_ref(),
            &p.title,
            details,
            Some(list),
            fav,
            |_, _| {},
        );
        tracks::show(ui, ctx, "playlist-tracks", list, false);
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{Action, Event, PageData, Route};
    use crate::ui::testing::{at, every_state, key, pass, playlist, tracks};
    use egui::Key;

    /// The first track is unavailable: Enter on its row plays nothing, so only Play can.
    fn data() -> PageData {
        let mut list = tracks(3);
        list[0].stream_ready = false;
        PageData::Playlist {
            playlist: playlist(),
            tracks: list,
            positions: vec![0, 1, 2],
            etag: None,
        }
    }

    #[test]
    fn views_render_every_state() {
        let route = Route::Playlist("p-1".into());
        let texts = ["Playlist", "TIDAL", "3 tracks · 10:00", "Play"];
        every_state(super::show, route, data(), &texts);
    }

    /// Ruling R19: the user's own playlist (they are user 1) has no heart, whose remove could
    /// take it out of their library; TIDAL's and other users' have one.
    #[test]
    fn no_heart_on_the_users_own_playlist() {
        let egui = egui::Context::default();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        for (creator, heart) in [(Some(1), false), (Some(2), true), (Some(0), true)] {
            let route = Route::Playlist("p-1".into());
            let mut model = at(route.clone());
            let data = PageData::Playlist {
                playlist: tidal::Playlist {
                    creator_id: creator,
                    ..playlist()
                },
                tracks: tracks(1),
                positions: vec![0],
                etag: None,
            };
            model.apply(Event::Page { route, data });
            let (_, out) = pass(&egui, &tx, &model, super::show, vec![]);
            let tree = out
                .platform_output
                .accesskit_update
                .expect("AccessKit is on");
            let shown = tree
                .nodes
                .iter()
                .any(|(_, n)| n.label() == Some("Add to favorites"));
            assert_eq!(shown, heart, "creator {creator:?}");
        }
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
