//! Playlist page: header with Play, then its tracks. The user's own also has Edit and Delete,
//! and its rows can be removed and dragged to reorder it.

use super::widgets::pill_button;
use super::{Ctx, header, library, tracks};
use crate::i18n::tr;
use crate::model::{PageData, Route};

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Some(PageData::Playlist {
        playlist: p,
        tracks: list,
        positions,
        etag,
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
        let edit = |ui: &mut egui::Ui, ctx: &mut Ctx| {
            if !own {
                return;
            }
            let pill =
                |ui: &mut egui::Ui, label: &str| pill_button(ui, &ctx.palette, None, label, false);
            if pill(ui, &tr("Edit")).clicked() {
                library::open(ui.ctx(), library::edit_playlist(p));
            }
            if pill(ui, &tr("Delete")).clicked() {
                library::open(ui.ctx(), library::delete_playlist(p));
            }
        };
        header::header(
            ui,
            ctx,
            &tr("Playlist"),
            p.image().as_ref(),
            &p.title,
            details,
            Some(list),
            fav,
            edit,
        );
        // Ruling R11: rows are edited by position only with the ETag of their listing.
        let rows = etag
            .as_deref()
            .filter(|_| own)
            .map(|etag| tracks::PlaylistRows {
                uuid: &p.uuid,
                etag,
                positions,
            });
        tracks::show(ui, ctx, "playlist-tracks", list, false, rows.as_ref());
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{Action, Event, Model, PageData, Route};
    use crate::ui::library;
    use crate::ui::testing::{
        at, click, clicks, context, every_state, key, middle, pass, pass_with, playlist, right,
        shown, tracks,
    };
    use egui::{Key, PointerButton, Pos2, vec2};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

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
        let egui = context();
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
        let egui = context();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        for input in [vec![], vec![key(Key::Tab)], vec![key(Key::Enter)]] {
            pass(&egui, &tx, &model, super::show, input);
        }
        let played = std::iter::from_fn(|| rx.try_recv().ok()).find_map(|a| match a {
            Action::PlayTracks { tracks, start, .. } => Some((tracks.len(), start)),
            _ => None, // the artwork
        });
        assert_eq!(played, Some((3, 0)));
    }

    /// The page drawn pass after pass, with the library's dialog, as the app draws them.
    struct Page {
        egui: egui::Context,
        tx: UnboundedSender<Action>,
        rx: UnboundedReceiver<Action>,
        model: Model,
    }

    impl Page {
        /// Playlist `p-1` of user `creator` (the user is 1): three tracks at positions 0, 2 and
        /// 3 of a listing tagged `etag`.
        fn new(creator: u64, etag: Option<&str>) -> Self {
            let route = Route::Playlist("p-1".into());
            let mut model = at(route.clone());
            let data = PageData::Playlist {
                playlist: tidal::Playlist {
                    creator_id: Some(creator),
                    ..playlist()
                },
                tracks: tracks(3),
                positions: vec![0, 2, 3],
                etag: etag.map(str::to_owned),
            };
            model.apply(Event::Page { route, data });
            let egui = context();
            egui.enable_accesskit();
            let (tx, rx) = unbounded_channel();
            Self {
                egui,
                tx,
                rx,
                model,
            }
        }

        fn pass(&mut self, input: Vec<egui::Event>) -> egui::FullOutput {
            let model = &self.model;
            let (_, out) = pass_with(&self.egui, &self.tx, model, input, |ui, c| {
                super::show(ui, c, model.route());
                library::dialog(ui.ctx(), c);
            });
            out
        }

        /// A few passes without input, for menus and dialogs to lay themselves out.
        fn settle(&mut self) -> egui::FullOutput {
            self.pass(vec![]);
            self.pass(vec![])
        }

        /// Right-clicks the node labelled `label`: the output with its menu open.
        fn menu(&mut self, label: &str) -> egui::FullOutput {
            let at = middle(&self.settle(), label);
            self.pass(vec![egui::Event::PointerMoved(at), right(at, true)]);
            self.pass(vec![right(at, false)]);
            self.settle()
        }

        /// Drags the row labelled `from` to just inside the top edge of the row labelled `to`.
        fn drag(&mut self, from: &str, to: &str) {
            let out = self.settle();
            let top = |label| middle(&out, label) - vec2(0.0, 12.0);
            let (from, to) = (top(from), top(to));
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let moved = |pos: Pos2| egui::Event::PointerMoved(pos);
            self.pass(vec![moved(from), button(from, true)]);
            self.pass(vec![moved(from + vec2(0.0, 10.0))]);
            self.pass(vec![moved(to)]);
            self.pass(vec![button(to, false)]);
        }

        /// The playlist edits sent since last asked.
        fn sent(&mut self) -> Vec<String> {
            std::iter::from_fn(|| self.rx.try_recv().ok())
                .filter(|a| matches!(a, Action::EditPlaylist(_))) // not the artwork
                .map(|a| format!("{a:?}"))
                .collect()
        }
    }

    fn row(n: usize) -> String {
        format!("{n}. Track {n}, Artist 1, 3:20")
    }

    /// The user's own playlist has Edit (its title and description, as they are) and Delete
    /// (asked first); TIDAL's and other users' have neither.
    #[test]
    fn own_playlist_has_edit_and_delete_others_not() {
        for creator in [0, 2] {
            let shown = shown(&Page::new(creator, Some("e1")).settle());
            let offered = ["Edit", "Delete"].map(|s| shown.contains(&s.into()));
            assert_eq!(offered, [false, false], "creator {creator}");
        }
        let mut page = Page::new(1, Some("e1"));
        let out = page.settle();
        page.pass(vec![click(&out, "Edit")]);
        let out = page.settle();
        assert!(shown(&out).contains(&"Edit playlist".into()));
        page.pass(vec![key(Key::Enter)]);
        let edit = r#"EditPlaylist(Edit { uuid: "p-1", title: "Playlist", description: "" })"#;
        assert_eq!(page.sent(), [edit]);
        let out = page.settle();
        page.pass(vec![click(&out, "Delete")]);
        let out = page.settle();
        assert!(page.sent().is_empty(), "asked first");
        let ask = "Delete the playlist Playlist? This cannot be undone.";
        assert!(shown(&out).contains(&ask.into()));
        // The header's Delete, under the dialog, only asks again.
        page.pass(clicks(&out, "Delete").collect());
        assert_eq!(page.sent(), [r#"EditPlaylist(Delete { uuid: "p-1" })"#]);
    }

    /// A row's "Remove from this playlist" removes its position in the playlist, guarded by
    /// the listing's ETag. Without that ETag (R11), or on another user's playlist, it is not
    /// offered.
    #[test]
    fn remove_sends_the_playlist_position() {
        for (creator, etag) in [(1, None), (2, Some("e1"))] {
            let out = Page::new(creator, etag).menu(&row(2));
            let shown = shown(&out);
            assert!(shown.contains(&"Play next".into()), "the menu is open");
            let remove = shown.contains(&"Remove from this playlist".into());
            assert!(!remove, "creator {creator}, {etag:?}");
        }
        let mut page = Page::new(1, Some("e1"));
        let out = page.menu(&row(2));
        page.pass(vec![click(&out, "Remove from this playlist")]);
        let remove = r#"EditPlaylist(Remove { uuid: "p-1", etag: "e1", positions: [2] })"#;
        assert_eq!(page.sent(), [remove]);
    }

    /// The page reloads under row 2's open menu (row 1 removed, a new ETag), so row 2 holds
    /// another track: the menu closes, and neither its Remove nor its "Add to playlist" acts.
    #[test]
    fn a_reload_closes_an_open_row_menu() {
        let mut page = Page::new(1, Some("e1"));
        let own = tidal::Playlist {
            uuid: "p-2".into(),
            title: "Road trip".into(),
            ..playlist()
        };
        page.model.apply(Event::OwnPlaylists(vec![own]));
        let out = page.menu(&row(2));
        page.pass(vec![click(&out, "Add to playlist")]);
        let out = page.settle();
        let data = PageData::Playlist {
            playlist: tidal::Playlist {
                creator_id: Some(1),
                ..playlist()
            },
            tracks: tracks(3)[1..].to_vec(),
            positions: vec![0, 1],
            etag: Some("e2".into()),
        };
        let route = page.model.route().clone();
        page.model.apply(Event::Page { route, data });
        let picks = ["Remove from this playlist", "Road trip"].map(|s| click(&out, s));
        page.pass(picks.into());
        assert_eq!(page.sent(), Vec::<String>::new());
        assert!(
            !shown(&page.settle()).contains(&"Play next".into()),
            "closed"
        );
    }

    /// Row 1 dropped on row 3's top edge goes before position 3; dropped on its own bottom
    /// edge, or over the header above the list, it stays. Without the listing's ETag (R11)
    /// rows do not move.
    #[test]
    fn dragging_a_row_sends_move() {
        let mut page = Page::new(1, Some("e1"));
        page.drag(&row(1), &row(3));
        let to = r#"EditPlaylist(Move { uuid: "p-1", etag: "e1", from: 0, to: 3 })"#;
        assert_eq!(page.sent(), [to]);
        page.drag(&row(1), &row(2));
        assert_eq!(page.sent(), Vec::<String>::new(), "its own edge");
        page.drag(&row(2), "Play");
        assert_eq!(page.sent(), Vec::<String>::new(), "over the header");
        let mut page = Page::new(1, None);
        page.drag(&row(1), &row(3));
        assert_eq!(page.sent(), Vec::<String>::new(), "no ETag");
    }
}
