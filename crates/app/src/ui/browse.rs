//! Home, Explore and the other v1 pages (a title, then shelves), and a "View all" (its one
//! shelf in full: every track, or a grid of cards).

use super::{Ctx, shelf, tracks};
use crate::i18n::tr;
use crate::model::{PageData, Route};
use tidal::Layout;

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Some(PageData::Shelves(page)) = super::page_data(ui, ctx, route) else {
        return;
    };
    super::page(ui, route, |ui| {
        if *route == Route::Home {
            ui.heading(tr("Home")); // the home feed has no title
        } else if !page.title.is_empty() {
            ui.heading(&page.title);
        }
        if !matches!(route, Route::ViewAll(_)) {
            shelf::shelves(ui, &page.shelves, ctx);
            return;
        }
        for s in &page.shelves {
            if s.layout == Layout::Tracks {
                // ponytail: up to 500 tracks cloned per frame; keep them as tracks on arrival
                // if this ever shows in a profile.
                let list: Vec<_> = shelf::tracks_of(&s.items).cloned().collect();
                tracks::show(ui, ctx, "view-all-tracks", &list, false);
            } else {
                shelf::grid(ui, &s.items, ctx);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{PageData, Route};
    use crate::ui::testing::{album, artist, every_state, mix, playlist, tracks};
    use tidal::{Item, Layout, More, Page, Shelf};

    #[test]
    fn views_render_every_state() {
        let cards = Shelf {
            title: "New".into(),
            layout: Layout::Cards,
            items: vec![
                Item::Album(album(1)),
                Item::Artist(artist(2)),
                Item::Playlist(playlist()),
                Item::Mix(mix()),
            ],
            more: Some(More::ViewAll("home/pages/x/view-all".into())),
        };
        let songs = Shelf {
            title: String::new(), // draws no heading
            layout: Layout::Tracks,
            items: tracks(3).into_iter().map(Item::Track).collect(),
            more: None,
        };
        let links = Shelf {
            title: "Genres".into(),
            layout: Layout::Links,
            items: vec![Item::Link {
                title: "Pop".into(),
                api_path: "pages/genre_pop".into(),
            }],
            more: None,
        };
        let shelves = vec![cards.clone(), songs.clone(), links];
        let home = Page {
            title: String::new(), // as the home feed comes
            shelves: shelves.clone(),
        };
        every_state(
            super::show,
            Route::Home,
            PageData::Shelves(home),
            &["Home", "New", "Genres", "Pop"],
        );
        let explore = Page {
            title: "Explore".into(),
            shelves,
        };
        let route = Route::Page("pages/explore".into());
        every_state(super::show, route, PageData::Shelves(explore), &["Explore"]);
        // A view-all shows its items as cards, or as track rows.
        let all = [
            (cards, "Album 1, Artist 1, Artist 2"),
            (songs, "3. Track 3, Artist 1, 3:20"),
        ];
        for (shelf, item) in all {
            let page = Page {
                title: "All".into(),
                shelves: vec![Shelf {
                    title: String::new(),
                    more: None,
                    ..shelf
                }],
            };
            let route = Route::ViewAll("home/pages/x/view-all".into());
            every_state(super::show, route, PageData::Shelves(page), &["All", item]);
        }
    }
}
