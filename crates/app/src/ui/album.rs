//! Album page: header (artists link to their pages) with Play and "Add to playlist", then the
//! track list with disc headers.

use super::{Ctx, header, library, tracks};
use crate::i18n::tr;
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
            &tr("Album"),
            album.image().as_ref(),
            &album.title,
            details,
            Some(list),
            Some(tidal::Favorite::Album(album.id)),
            |ui, ctx| library::add_to_playlist(ui, ctx, list),
        );
        tracks::show(ui, ctx, "album-tracks", list, true, None);
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{Action, Event, Model, PageData, Route};
    use crate::theme;
    use crate::ui::testing::{album, at, click, context, every_state, pass, shown, tracks};
    use tidal::Favorite;

    fn data() -> PageData {
        let mut list = tracks(2);
        list[1].volume_number = 2;
        PageData::Album {
            album: album(1),
            tracks: list,
        }
    }

    /// The heart flips at once (the model applies `FavoriteSet`) and the backend writes it;
    /// it then offers the way back, but only once that write is done (ruling R19).
    #[test]
    fn heart_favorites_at_once() {
        let route = Route::Album(1);
        let mut model = at(route.clone());
        model.apply(Event::Page {
            route,
            data: data(),
        });
        let egui = context();
        egui.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, super::show, vec![]);
        let heart = click(&out, "Add to favorites");
        let (events, _) = pass(&egui, &tx, &model, super::show, vec![heart]);
        let album = Favorite::Album(1);
        let set = matches!(&events[..], [Event::FavoriteSet { item, on: true }] if *item == album);
        assert!(set, "{events:?}");
        let sent = std::iter::from_fn(|| rx.try_recv().ok())
            .any(|a| matches!(a, Action::SetFavorite { item, on: true } if item == album));
        assert!(sent, "the write");
        for ev in events {
            model.apply(ev);
        }
        let mut unfavorite = |model: &Model| {
            let (_, out) = pass(&egui, &tx, model, super::show, vec![]);
            let heart = click(&out, "Remove from favorites");
            let (events, _) = pass(&egui, &tx, model, super::show, vec![heart]);
            let sent = std::iter::from_fn(|| rx.try_recv().ok())
                .any(|a| matches!(a, Action::SetFavorite { on: false, .. }));
            (events.len(), sent)
        };
        let pending = unfavorite(&model);
        assert_eq!(pending, (0, false), "pending: the heart is disabled");
        model.apply(Event::FavoriteSaved { item: album });
        assert_eq!(unfavorite(&model), (1, true), "saved: it toggles again");
    }

    /// The backdrop asks for the cover at the tint's 16 px, beside the header's own size.
    #[test]
    fn the_backdrop_asks_for_the_small_cover() {
        let route = Route::Album(1);
        let mut model = at(route.clone());
        model.apply(Event::Page {
            route,
            data: data(),
        });
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        pass(&context(), &tx, &model, super::show, vec![]);
        let sizes: Vec<u32> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter_map(|a| match a {
                Action::LoadImage { px, .. } => Some(px),
                _ => None,
            })
            .collect();
        let header = crate::ui::request_px(theme::HEADER_COVER);
        assert!(sizes.contains(&theme::TINT_PX), "{sizes:?}");
        assert!(sizes.contains(&header), "{sizes:?}");
    }

    /// Ruling 22: the action row sits under the cover, at the page's padding, as TIDAL's.
    #[test]
    fn the_actions_sit_under_the_cover() {
        let route = Route::Album(1);
        let mut model = at(route.clone());
        model.apply(Event::Page {
            route,
            data: data(),
        });
        let egui = context();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, super::show, vec![]);
        let play = crate::ui::testing::bounds(&out, "Play");
        let cover_bottom = theme::PAGE_PAD + theme::HEADER_COVER; // the page starts at 0
        assert!(play.top() > cover_bottom, "{play:?}");
        assert!((play.left() - theme::PAGE_PAD).abs() < 1.0, "{play:?}");
    }

    /// The header's "Add to playlist" pill opens the playlists to pick from.
    #[test]
    fn add_to_playlist_opens_its_menu() {
        let route = Route::Album(1);
        let mut model = at(route.clone());
        model.apply(Event::Page {
            route,
            data: data(),
        });
        let egui = context();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, super::show, vec![]);
        let pill = click(&out, "Add to playlist");
        pass(&egui, &tx, &model, super::show, vec![pill]);
        let (_, out) = pass(&egui, &tx, &model, super::show, vec![]);
        assert!(shown(&out).contains(&"New playlist…".into()));
    }

    #[test]
    fn views_render_every_state() {
        let data = data();
        let texts = [
            "Album 1",
            "Artist 1",
            "Artist 2",
            "2020",
            "2 tracks · 6:40",
            "Play",
            "Add to playlist",
            "Disc 2",
        ];
        every_state(super::show, Route::Album(1), data, &texts);
    }
}
