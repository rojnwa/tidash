//! Shelves (a title, maybe "View all", then a sideways row of cards, a few tracks, or links)
//! and the card grid.

use super::widgets::{fade, focus_ring, icon_button, line, pill_button};
use super::{Ctx, card, rows, tracks};
use crate::i18n::tr;
use crate::model::{Action, Route};
use crate::theme::{self, Icon};
use egui::scroll_area::ScrollBarVisibility;
use egui::{Sense, WidgetInfo, WidgetType, vec2};
use tidal::{Item, Layout, More, Shelf};

/// The track rows a Tracks shelf shows.
const SHELF_TRACKS: usize = 10;

pub fn shelves(ui: &mut egui::Ui, shelves: &[Shelf], ctx: &mut Ctx) {
    for (n, s) in shelves.iter().enumerate() {
        ui.push_id(n, |ui| shelf(ui, s, ctx));
    }
}

fn shelf(ui: &mut egui::Ui, s: &Shelf, ctx: &mut Ctx) {
    super::gap(ui);
    let cards = s.layout == Layout::Cards;
    // An untitled shelf without "View all" or a row of cards to page has no heading row.
    let pages = if !s.title.is_empty() || s.more.is_some() || cards {
        heading(ui, ctx, s, cards)
    } else {
        0.0
    };
    match s.layout {
        // Not virtualized: a rail holds tens of cards, and `cover` skips those out of view. Its
        // scroll bar is hidden; the buttons, the wheel and Tab move it.
        Layout::Cards => {
            let (n, w) = theme::card_columns(ui.available_width());
            egui::ScrollArea::horizontal()
                .id_salt("cards")
                .scroll_bar_visibility(ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    if pages != 0.0 {
                        // A row's worth of cards, so the next lands flush with the left edge.
                        let row = n as f32 * (w + theme::CARD_GAP);
                        ui.scroll_with_delta(vec2(-pages * row, 0.0));
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = theme::CARD_GAP;
                        for item in &s.items {
                            item_card(ui, ctx, item, w);
                        }
                    });
                });
        }
        Layout::Tracks => {
            let list: Vec<_> = tracks_of(&s.items).take(SHELF_TRACKS).cloned().collect();
            tracks::show(ui, ctx, "tracks", &list, false, None);
        }
        Layout::Links => {
            let p = ctx.palette;
            ui.horizontal_wrapped(|ui| {
                for item in &s.items {
                    if let Item::Link { title, api_path } = item
                        && pill_button(ui, &p, None, title, false).clicked()
                    {
                        ctx.go(Route::Page(api_path.clone()));
                    }
                }
            });
        }
    }
}

/// A shelf's heading row: its title, then at the right end ‹ › for a row of `cards` and "View
/// all". How many rows the cards should move: -1, 0 or 1.
fn heading(ui: &mut egui::Ui, ctx: &mut Ctx, s: &Shelf, cards: bool) -> f32 {
    let p = ctx.palette;
    ui.horizontal(|ui| {
        let gap = ui.spacing().item_spacing.x;
        let view_all = tr("View all");
        let more = s.more.as_ref().map(|more| {
            let text = line(ui, &view_all, theme::body(), p.secondary, f32::INFINITY);
            (more, text)
        });
        // The right end's width, laid out left to right so Tab meets ‹ › before View all.
        let mut right = more.as_ref().map_or(-gap, |(_, text)| text.size().x);
        if cards {
            right += 2.0 * (theme::ICON_BUTTON + gap);
        }
        if !s.title.is_empty() {
            let room = (ui.available_width() - right - gap).max(0.0);
            ui.label(line(ui, &s.title, theme::heading(), p.text, room));
        }
        ui.add_space((ui.available_width() - right).max(0.0));
        let mut pages = 0.0;
        if cards {
            if icon_button(ui, &p, Icon::ChevronLeft, &tr("Scroll left"), None).clicked() {
                pages -= 1.0;
            }
            if icon_button(ui, &p, Icon::ChevronRight, &tr("Scroll right"), None).clicked() {
                pages += 1.0;
            }
        }
        if let Some((more, text)) = more {
            let (rect, r) = ui.allocate_exact_size(text.size(), Sense::click());
            let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
            r.widget_info(|| WidgetInfo::labeled(WidgetType::Link, ui.is_enabled(), &*view_all));
            let ink = theme::blend(p.secondary, p.text, fade(ui, &r, r.hovered()));
            ui.painter()
                .galley_with_override_text_color(rect.min, text, ink);
            focus_ring(ui, &r, theme::R_BADGE);
            if r.clicked() {
                ctx.go(match more {
                    More::Page(path) => Route::Page(path.clone()),
                    More::ViewAll(path) => Route::ViewAll(path.clone()),
                });
            }
        }
        pages
    })
    .inner
}

/// The tracks among `items`.
pub fn tracks_of(items: &[Item]) -> impl Iterator<Item = &tidal::Track> {
    items.iter().filter_map(|i| match i {
        Item::Track(t) => Some(t),
        _ => None,
    })
}

/// Items as rows of cards sharing the width (`card_columns`); only the rows in view are built.
pub fn grid(ui: &mut egui::Ui, items: &[Item], ctx: &mut Ctx) {
    let (cols, w) = theme::card_columns(ui.available_width());
    let row_h = card::height(w) + theme::CARD_GAP;
    rows::virtual_rows(ui, items.len().div_ceil(cols), row_h, |ui, r| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = theme::CARD_GAP;
            for item in items.chunks(cols).nth(r).unwrap_or_default() {
                item_card(ui, ctx, item, w);
            }
        });
        ui.add_space(theme::CARD_GAP);
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

/// A card `width` wide for `item`: a click opens its page, or plays a track. An album's,
/// playlist's or mix's play disc plays its tracks.
fn item_card(ui: &mut egui::Ui, ctx: &mut Ctx, item: &Item, width: f32) {
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
    let list = match item {
        Item::Album(_) | Item::Playlist(_) | Item::Mix(_) => route_of(item),
        _ => None,
    };
    let play = list
        .as_ref()
        .map(|r| super::under_way(ctx.model, r) == Some(true));
    let (resp, played) = card::card(
        ui,
        &ctx.palette,
        ctx.images,
        ctx.tx,
        image.as_ref(),
        title,
        &subtitle,
        round,
        play,
        width,
    );
    if played && let Some(r) = &list {
        super::play_list(ctx, r, title);
    }
    if !resp.clicked() {
        return;
    }
    if let Some(r) = route_of(item) {
        ctx.go(r);
    } else if let Item::Track(t) = item {
        ctx.send(Action::PlayTracks {
            tracks: vec![t.clone()],
            start: 0,
            source: None, // a lone track: its album is where it leads (spec §3.9)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::route_of;
    use crate::model::Route;
    use crate::theme::{CARD_GAP, card_columns};
    use crate::ui::testing::{album, at, bounds, click, context, pass_with};
    use egui::vec2;
    use tidal::{Item, Layout, Shelf};

    /// A rail of `n` albums.
    fn rail(n: u64) -> Shelf {
        Shelf {
            title: "New".into(),
            layout: Layout::Cards,
            items: (1..=n).map(|i| Item::Album(album(i))).collect(),
            more: None,
        }
    }

    /// Where album `i`'s card is drawn in `out`.
    fn card(out: &egui::FullOutput, i: u64) -> egui::Rect {
        bounds(out, &format!("Album {i}, Artist 1, Artist 2"))
    }

    /// Cards share the row's width, `card_columns` of them in view, `CARD_GAP` apart.
    #[test]
    fn cards_fill_the_row() {
        let egui = context();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, shelves) = (at(Route::Home), [rail(12)]);
        let mut width = 0.0;
        let (_, out) = pass_with(&egui, &tx, &model, vec![], |ui, ctx| {
            width = ui.available_width();
            super::shelves(ui, &shelves, ctx);
        });
        let (_, w) = card_columns(width);
        assert!((card(&out, 1).width() - w).abs() < 0.5, "{w}");
        let fourth = card(&out, 4).left() - card(&out, 1).left();
        assert!((fourth - 3.0 * (w + CARD_GAP)).abs() < 0.5, "{fourth}");
    }

    /// "Scroll right" moves the cards left by about the row's width, over the next passes.
    #[test]
    fn scroll_right_moves_the_row() {
        let egui = context();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, shelves) = (at(Route::Home), [rail(12)]);
        let mut width = 0.0;
        let mut pass = |input| {
            let (_, out) = pass_with(&egui, &tx, &model, input, |ui, ctx| {
                width = ui.available_width();
                super::shelves(ui, &shelves, ctx);
            });
            out
        };
        let out = pass(vec![]);
        let before = card(&out, 1).left();
        let mut out = pass(vec![click(&out, "Scroll right")]);
        for _ in 0..60 {
            out = pass(vec![]);
        }
        let moved = before - card(&out, 1).left();
        assert!(
            (moved - width).abs() <= CARD_GAP + 0.5,
            "{moved} for {width}"
        );
    }

    /// From the first shelf's cards to the second's heading row (its ‹ › buttons fill it):
    /// `MODULE_GAP`.
    #[test]
    fn modules_are_module_gap_apart() {
        let egui = context();
        egui.enable_accesskit();
        crate::theme::apply(&egui, &crate::theme::Palette::dark()); // its item spacing
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let old = Shelf {
            title: "Old".into(),
            items: (4..=6).map(|i| Item::Album(album(i))).collect(),
            ..rail(0)
        };
        let (model, shelves) = (at(Route::Home), [rail(3), old]);
        let (_, out) = pass_with(&egui, &tx, &model, vec![], |ui, ctx| {
            super::shelves(ui, &shelves, ctx);
        });
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let second = (nodes.iter())
            .filter(|(_, n)| n.label() == Some("Scroll left"))
            .filter_map(|(_, n)| n.bounds().map(|b| b.y0 as f32))
            .reduce(f32::max)
            .expect("two shelves");
        let gap = second - card(&out, 1).bottom();
        assert!((gap - crate::theme::MODULE_GAP).abs() < 0.5, "{gap}");
    }

    /// A playlist card's disc, under the pointer, plays the playlist from its page; the rest
    /// of the card still opens it. While the playlist plays, the disc says Pause and toggles.
    /// An artist's card has none.
    #[test]
    fn a_cards_disc_plays_its_list() {
        use crate::model::{Action, Event, Model};
        use crate::ui::testing::{artist, playing, playlist, primary, shown};
        let egui = context();
        egui.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut sent = || -> Vec<String> {
            std::iter::from_fn(|| rx.try_recv().ok())
                .filter(|a| !matches!(a, Action::LoadImage { .. }))
                .map(|a| format!("{a:?}"))
                .collect()
        };
        let shelves = [Shelf {
            items: vec![Item::Playlist(playlist()), Item::Artist(artist(3))],
            ..rail(0)
        }];
        let pass = |m: &Model, input| {
            pass_with(&egui, &tx, m, input, |ui, ctx| {
                super::shelves(ui, &shelves, ctx)
            })
        };
        let click_at = |m: &Model, at| {
            let moved = vec![egui::Event::PointerMoved(at)];
            [moved, vec![primary(at, true)], vec![primary(at, false)]]
                .into_iter()
                .flat_map(|input| pass(m, input).0)
                .collect::<Vec<_>>()
        };
        let mut m = at(Route::Home);
        let out = pass(&m, vec![]).1;
        assert!(!shown(&out).contains(&"Play Artist 3".into()));
        let events = click_at(&m, bounds(&out, "Play Playlist").center());
        assert!(events.is_empty(), "the card stays shut: {events:?}");
        let play = r#"PlaySource(Source { route: Playlist("p-1"), name: "Playlist" })"#;
        assert_eq!(sent(), [play]);
        let events = click_at(
            &m,
            bounds(&out, "Playlist, TIDAL").left_top() + vec2(10.0, 10.0),
        );
        assert!(
            matches!(&events[..], [Event::Navigate(Route::Playlist(u))] if u == "p-1"),
            "{events:?}"
        );
        assert_eq!(sent(), Vec::<String>::new());

        playing(&mut m, Route::Playlist("p-1".into()), "Playlist");
        let out = pass(&m, vec![]).1;
        pass(&m, vec![click(&out, "Pause Playlist")]);
        assert_eq!(sent(), ["TogglePlay"]);
    }

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
