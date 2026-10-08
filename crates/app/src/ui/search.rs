//! Search view: the query box, then one page scroll of shelves (artists, albums, playlists)
//! and every track found.

use super::{Ctx, shelf, tracks};
use crate::i18n::tr;
use crate::model::{Action, Route};
use std::borrow::Cow;
use tidal::{Item, Layout, Shelf};

#[derive(Default)]
pub struct State {
    query: String,
}

/// The query box's id, fixed so that Ctrl+F can focus it from anywhere.
pub fn query_id() -> egui::Id {
    egui::Id::new("search-query")
}

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, state: &mut State) {
    let edit = ui.add(
        egui::TextEdit::singleline(&mut state.query)
            .id(query_id())
            .hint_text(tr("Search TIDAL"))
            .desired_width(f32::INFINITY),
    );
    let q = state.query.trim();
    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !q.is_empty() {
        ctx.send(Action::Search(q.to_string()));
    }
    let model = ctx.model;
    let Some((_, results)) = &model.search else {
        return;
    };
    super::page(ui, &Route::Search, |ui| {
        let shelves: Vec<Shelf> = [
            cards(tr("Artists"), &results.artists, Item::Artist),
            cards(tr("Albums"), &results.albums, Item::Album),
            cards(tr("Playlists"), &results.playlists, Item::Playlist),
        ]
        .into_iter()
        .flatten()
        .collect();
        shelf::shelves(ui, &shelves, ctx);
        if !results.tracks.is_empty() {
            ui.add_space(16.0);
            ui.heading(tr("Tracks"));
            tracks::show(ui, ctx, "search-tracks", &results.tracks, false);
        }
    });
}

/// A shelf of cards for `list`, or none when it is empty.
// ponytail: up to 90 results are cloned into items each frame; convert them once on arrival
// if this ever shows in a profile.
fn cards<T: Clone>(title: Cow<'_, str>, list: &[T], item: fn(T) -> Item) -> Option<Shelf> {
    (!list.is_empty()).then(|| Shelf {
        title: title.into_owned(),
        layout: Layout::Cards,
        items: list.iter().cloned().map(item).collect(),
        more: None,
    })
}
