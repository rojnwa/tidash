//! Search view: one page scroll of shelves (artists, albums, playlists) and every track
//! found. The query box is the top bar's.

use super::{Ctx, shelf, tracks};
use crate::i18n::tr;
use crate::model::Route;
use std::borrow::Cow;
use tidal::{Item, Layout, Shelf};

/// The query, kept from page to page; the top bar draws it and sends it.
#[derive(Default)]
pub struct State {
    pub query: String,
}

/// The query box's id, fixed so that Ctrl+F can focus it from anywhere.
pub fn query_id() -> egui::Id {
    egui::Id::new("search-query")
}

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx) {
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
            tracks::show(ui, ctx, "search-tracks", &results.tracks, false, None);
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

#[cfg(test)]
mod tests {
    use super::super::{UiState, draw, testing};
    use crate::model::{Event, Model, Route};
    use egui::{Key, Modifiers};

    /// The session's end takes the query with it: the next account finds the box empty, and
    /// Undo there does not bring the query back.
    #[test]
    fn the_query_leaves_with_the_session() {
        for end in [Event::LoggedOut, Event::SessionExpired] {
            let ctx = egui::Context::default();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let (mut state, mut model) = (UiState::default(), testing::at(Route::Search));
            let mut time = 0.0;
            // The box focused; 2 s a pass, past the time text takes to become an undo point.
            let mut pass = |model: &Model, state: &mut UiState, events| {
                time += 2.0;
                let input = egui::RawInput {
                    events,
                    time: Some(time),
                    ..Default::default()
                };
                ctx.memory_mut(|m| m.request_focus(super::query_id()));
                let mut out = ctx.run_ui(input, |ui| {
                    draw(ui, model, &tx, state);
                });
                out.textures_delta.clear(); // nothing renders them
            };
            pass(&model, &mut state, vec![egui::Event::Text("secret".into())]);
            pass(&model, &mut state, vec![]);
            assert_eq!(state.search.query, "secret");
            state.apply(&ctx, &end);
            model.apply(end.clone());
            model.apply(Event::LoggedIn(2));
            model.navigate(Route::Search);
            pass(&model, &mut state, vec![]);
            assert_eq!(state.search.query, "", "after {end:?}");
            let undo = egui::Event::Key {
                key: Key::Z,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::COMMAND,
            };
            pass(&model, &mut state, vec![undo]);
            assert_eq!(state.search.query, "", "undone after {end:?}");
        }
    }
}
