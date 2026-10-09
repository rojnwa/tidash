//! Shelves (a title, maybe "View all", then a sideways row of cards, a few tracks, or links)
//! and the card grid.

use super::{Ctx, card, rows, tracks};
use crate::i18n::tr;
use crate::model::{Action, Route};
use tidal::{Item, Layout, More, Shelf};

/// The track rows a Tracks shelf shows.
const SHELF_TRACKS: usize = 10;

pub fn shelves(ui: &mut egui::Ui, shelves: &[Shelf], ctx: &mut Ctx) {
    for (n, s) in shelves.iter().enumerate() {
        ui.push_id(n, |ui| shelf(ui, s, ctx));
    }
}

fn shelf(ui: &mut egui::Ui, s: &Shelf, ctx: &mut Ctx) {
    ui.add_space(16.0);
    // An untitled shelf without "View all" has no heading row.
    if !s.title.is_empty() || s.more.is_some() {
        ui.horizontal(|ui| {
            if !s.title.is_empty() {
                ui.heading(&s.title);
            }
            if let Some(more) = &s.more
                && ui.link(tr("View all")).clicked()
            {
                ctx.go(match more {
                    More::Page(path) => Route::Page(path.clone()),
                    More::ViewAll(path) => Route::ViewAll(path.clone()),
                });
            }
        });
    }
    match s.layout {
        // Not virtualized: a rail holds tens of cards, and `cover` skips those out of view.
        Layout::Cards => {
            egui::ScrollArea::horizontal()
                .id_salt("cards")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for item in &s.items {
                            item_card(ui, ctx, item);
                        }
                    });
                });
        }
        Layout::Tracks => {
            let list: Vec<_> = tracks_of(&s.items).take(SHELF_TRACKS).cloned().collect();
            tracks::show(ui, ctx, "tracks", &list, false);
        }
        Layout::Links => {
            ui.horizontal_wrapped(|ui| {
                for item in &s.items {
                    if let Item::Link { title, api_path } = item
                        && ui.button(title).clicked()
                    {
                        ctx.go(Route::Page(api_path.clone()));
                    }
                }
            });
        }
    }
}

/// The tracks among `items`.
pub fn tracks_of(items: &[Item]) -> impl Iterator<Item = &tidal::Track> {
    items.iter().filter_map(|i| match i {
        Item::Track(t) => Some(t),
        _ => None,
    })
}

/// Items as rows of cards, as many to a row as fit; only the rows in view are built.
pub fn grid(ui: &mut egui::Ui, items: &[Item], ctx: &mut Ctx) {
    let gap = ui.spacing().item_spacing.x;
    let cols = (((ui.available_width() + gap) / (card::WIDTH + gap)) as usize).max(1);
    rows::virtual_rows(ui, items.len().div_ceil(cols), card::HEIGHT, |ui, r| {
        ui.horizontal(|ui| {
            for item in items.chunks(cols).nth(r).unwrap_or_default() {
                item_card(ui, ctx, item);
            }
        });
    });
}

/// Where a card leads; a track has no page of its own.
pub fn route_of(item: &Item) -> Option<Route> {
    Some(match item {
        Item::Album(a) => Route::Album(a.id),
        Item::Artist(a) => Route::Artist(a.id),
        Item::Playlist(p) => Route::Playlist(p.uuid.clone()),
        Item::Mix(m) => Route::Mix(m.id.clone()),
        Item::Link { api_path, .. } => Route::Page(api_path.clone()),
        Item::Track(_) => return None,
    })
}

/// A card for `item`: a click opens its page, or plays a track.
fn item_card(ui: &mut egui::Ui, ctx: &mut Ctx, item: &Item) {
    let (image, title, subtitle, round) = match item {
        Item::Album(a) => (a.image(), &a.title, tracks::names(&a.artists), false),
        Item::Artist(a) => (a.image(), &a.name, tr("Artist").into_owned(), true),
        Item::Playlist(p) => {
            let by = p.creator.clone().unwrap_or_else(|| "TIDAL".into()); // a brand: not translated
            (p.image(), &p.title, by, false)
        }
        Item::Mix(m) => (m.image(), &m.title, m.subtitle.clone(), false),
        Item::Track(t) => (t.album.image(), &t.title, tracks::artists(t), false),
        Item::Link { title, .. } => (None, title, String::new(), false),
    };
    let resp = card::card(
        ui,
        ctx.images,
        ctx.tx,
        image.as_ref(),
        title,
        &subtitle,
        round,
    );
    if !resp.clicked() {
        return;
    }
    if let Some(r) = route_of(item) {
        ctx.go(r);
    } else if let Item::Track(t) = item {
        ctx.send(Action::PlayTracks {
            tracks: vec![t.clone()],
            start: 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::route_of;
    use crate::model::Route;
    use tidal::Item;

    #[test]
    fn item_routes() {
        let artist = tidal::Artist {
            id: 3,
            name: "n".into(),
            picture: None,
            selected_album_cover_fallback: None,
            mixes: Default::default(),
        };
        let album = tidal::Album {
            id: 1,
            title: "a".into(),
            cover: None,
            artists: vec![],
            number_of_tracks: 0,
            duration: 0,
            release_date: None,
        };
        let playlist = tidal::Playlist {
            uuid: "u-u".into(),
            title: "p".into(),
            description: None,
            creator: None,
            creator_id: None,
            number_of_tracks: 0,
            duration: 0,
            square_image: None,
            image: None,
        };
        let mix = tidal::Mix {
            id: "m1".into(),
            title: "m".into(),
            subtitle: String::new(),
            images: None,
        };
        let link = Item::Link {
            title: "Pop".into(),
            api_path: "pages/genre_pop".into(),
        };
        assert_eq!(route_of(&Item::Album(album)), Some(Route::Album(1)));
        assert_eq!(route_of(&Item::Artist(artist)), Some(Route::Artist(3)));
        assert_eq!(
            route_of(&Item::Playlist(playlist)),
            Some(Route::Playlist("u-u".into()))
        );
        assert_eq!(route_of(&Item::Mix(mix)), Some(Route::Mix("m1".into())));
        assert_eq!(route_of(&link), Some(Route::Page("pages/genre_pop".into())));
    }
}
