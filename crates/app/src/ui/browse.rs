//! Home, Explore and the other pages (a title, then shelves). A page of one shelf (a v2 "View
//! all", or a v1 `single-module-page`) shows it in full: every track, or a grid of cards
//! (ruling R17); but not a shelf of links, nor one with a View all of its own (R18).

use super::{Ctx, shelf, tracks};
use crate::i18n::tr;
use crate::model::{PageData, Route};
use tidal::Layout;

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Some(PageData::Shelves(page)) = super::page_data(ui, ctx, route) else {
        return;
    };
    let one = match &page.shelves[..] {
        [s] if s.layout != Layout::Links && s.more.is_none() => Some(s),
        _ => None,
    };
    super::page(ui, route, |ui| {
        let home = tr("Home"); // the home feed has no title
        let title: &str = if *route == Route::Home {
            &home
        } else if page.title.is_empty() {
            one.map_or("", |s| &s.title)
        } else {
            &page.title
        };
        if !title.is_empty() {
            ui.heading(title);
        }
        let Some(s) = one else {
            shelf::shelves(ui, &page.shelves, ctx);
            return;
        };
        if s.layout == Layout::Tracks {
            // ponytail: up to 500 tracks cloned per frame (~135 µs measured); keep them as
            // tracks on arrival if this ever shows in a profile.
            let list: Vec<_> = shelf::tracks_of(&s.items).cloned().collect();
            tracks::show(ui, ctx, "all-tracks", &list, false);
        } else {
            shelf::grid(ui, &s.items, ctx);
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{Event, PageData, Route};
    use crate::ui::testing::{album, artist, at, every_state, mix, pass, playlist, tracks};
    use tidal::{Item, Layout, More, Page, Shelf};

    /// A v1 "View all": a page of one untitled-page shelf, as `Client::page` fills it.
    fn single_module(title: &str, layout: Layout, items: Vec<Item>) -> (Route, Page) {
        let shelf = Shelf {
            title: title.into(),
            layout,
            items,
            more: None,
        };
        let route = Route::Page("pages/single-module-page/x".into());
        (
            route,
            Page {
                title: String::new(),
                shelves: vec![shelf],
            },
        )
    }

    /// The text and top edge of each AccessKit node `show` draws for `page` at `route`.
    fn nodes(route: Route, page: Page) -> Vec<(String, f64)> {
        let mut model = at(route.clone());
        model.apply(Event::Page {
            route,
            data: PageData::Shelves(page),
        });
        let egui = egui::Context::default();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, super::show, vec![]);
        let tree = out
            .platform_output
            .accesskit_update
            .expect("AccessKit is on");
        tree.nodes
            .iter()
            .filter_map(|(_, n)| {
                let text = n.label().or(n.value())?.to_owned();
                Some((text, n.bounds()?.y0))
            })
            .collect()
    }

    /// Ruling R17: a one-shelf page lists every track, not a shelf's first 10.
    #[test]
    fn a_single_module_page_lists_every_track() {
        let items = tracks(12).into_iter().map(Item::Track).collect();
        let (route, page) = single_module("New Tracks", Layout::Tracks, items);
        let nodes = nodes(route, page);
        let shows = |t: &str| nodes.iter().any(|(s, _)| s == t);
        assert!(shows("New Tracks"), "the shelf's title heads the page");
        assert!(shows("11. Track 11, Artist 1, 3:20"), "{nodes:?}");
    }

    /// Ruling R17: a one-shelf page of cards is a grid that wraps, not one sideways row.
    #[test]
    fn a_single_module_page_of_cards_is_a_grid() {
        let items = (1..=8).map(|i| Item::Album(album(i))).collect();
        let (route, page) = single_module("New Albums", Layout::Cards, items);
        let nodes = nodes(route, page);
        let top = |t: &str| nodes.iter().find(|(s, _)| s == t).map(|n| n.1);
        let (first, fifth) = (
            top("Album 1, Artist 1, Artist 2"),
            top("Album 5, Artist 1, Artist 2"),
        );
        assert!(top("New Albums").is_some(), "{nodes:?}");
        assert!(
            first.zip(fifth).is_some_and(|(a, b)| b > a),
            "card 5 starts a row below card 1: {nodes:?}"
        );
    }

    /// Ruling R18: a one-shelf page of links stays a row of buttons, not a grid of cards.
    #[test]
    fn a_one_shelf_page_of_links_stays_buttons() {
        let link = |i| Item::Link {
            title: format!("Genre {i}"),
            api_path: format!("pages/genre_{i}"),
        };
        let (route, page) = single_module("Genres", Layout::Links, (1..=5).map(link).collect());
        let nodes = nodes(route, page);
        let top = |t: &str| nodes.iter().find(|(s, _)| s == t).map(|n| n.1);
        assert!(top("Genre 1").is_some(), "{nodes:?}");
        assert_eq!(top("Genre 1"), top("Genre 5"), "one row: {nodes:?}");
    }

    /// Ruling R18: a one-shelf page whose shelf has its own View all keeps the link and the
    /// sideways rail: the shelf is not the full list.
    #[test]
    fn a_one_shelf_page_with_view_all_keeps_its_rail() {
        let items = (1..=8).map(|i| Item::Album(album(i))).collect();
        let (route, mut page) = single_module("New Albums", Layout::Cards, items);
        page.shelves[0].more = Some(More::ViewAll("home/pages/x/view-all".into()));
        let nodes = nodes(route, page);
        let top = |t: &str| nodes.iter().find(|(s, _)| s == t).map(|n| n.1);
        assert!(top("View all").is_some(), "{nodes:?}");
        let card = |i| top(&format!("Album {i}, Artist 1, Artist 2"));
        assert!(
            card(1).is_some() && card(1) == card(5),
            "one row: {nodes:?}"
        );
    }

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
