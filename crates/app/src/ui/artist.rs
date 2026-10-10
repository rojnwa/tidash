//! Artist page: picture, name and the start of the biography, its radio, then the shelves.

use super::{Ctx, header, shelf};
use crate::i18n::tr;
use crate::model::{PageData, Route};
use std::borrow::Cow;

/// The most characters of the biography shown.
const BIO: usize = 300;

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Some(PageData::Artist(page)) = super::page_data(ui, ctx, route) else {
        return;
    };
    let artist = &page.artist;
    super::page(ui, route, |ui| {
        let bio = |ui: &mut egui::Ui, _: &mut Ctx| {
            if let Some(bio) = &page.bio {
                ui.label(excerpt(bio, BIO));
            }
        };
        let radio = |ui: &mut egui::Ui, ctx: &mut Ctx| {
            if let Some(mix) = &artist.mixes.artist
                && ui.button(tr("Artist radio")).clicked()
            {
                ctx.go(Route::Mix(mix.clone()));
            }
        };
        header::header(
            ui,
            ctx,
            artist.image().as_ref(),
            &artist.name,
            bio,
            None,
            Some(tidal::Favorite::Artist(artist.id)),
            radio,
        );
        shelf::shelves(ui, &page.shelves, ctx);
    });
}

/// The first `max` characters of `text`, and "…" if it goes on.
fn excerpt(text: &str, max: usize) -> Cow<'_, str> {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", text[..end].trim_end()).into(),
        None => text.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{Event, Model, PageData, Route};
    use crate::ui::testing::{album, artist, at, click, every_state, key, pass, shown};
    use egui::Key;
    use std::time::Instant;
    use tidal::{ArtistPage, Item, Layout, Shelf};

    #[test]
    fn views_render_every_state() {
        let page = ArtistPage {
            artist: artist(1),
            bio: Some("Born in 1970.".into()),
            shelves: vec![Shelf {
                title: "Albums".into(),
                layout: Layout::Cards,
                items: vec![Item::Album(album(2))],
                more: None,
            }],
        };
        let texts = ["Artist 1", "Born in 1970.", "Albums"];
        every_state(
            super::show,
            Route::Artist(1),
            PageData::Artist(page),
            &texts,
        );
    }

    /// Ruling R12: Retry navigates to the failed page, which asks for it again.
    #[test]
    fn retry_sends_open() {
        let now = Instant::now();
        let route = Route::Artist(1);
        let mut model = at(route.clone());
        assert_eq!(model.open_current(now), Some(route.clone()));
        model.apply(Event::PageFailed(route.clone()));
        assert_eq!(model.open_current(now), None, "a failed page waits");
        let egui = egui::Context::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        for input in [vec![], vec![key(Key::Tab)], vec![key(Key::Enter)]] {
            let (events, _) = pass(&egui, &tx, &model, super::show, input);
            for ev in events {
                model.apply(ev);
            }
        }
        assert_eq!(model.open_current(now), Some(route));
    }

    /// Artist 1's page, `mix` its radio.
    fn with_mix(mix: Option<&str>) -> Model {
        let route = Route::Artist(1);
        let mut model = at(route.clone());
        let mut artist = artist(1);
        artist.mixes.artist = mix.map(Into::into);
        let page = ArtistPage {
            artist,
            bio: None,
            shelves: vec![],
        };
        let data = PageData::Artist(page);
        model.apply(Event::Page { route, data });
        model
    }

    /// "Artist radio" opens the artist's mix.
    #[test]
    fn artist_radio_button_opens_the_mix() {
        let model = with_mix(Some("m"));
        let egui = egui::Context::default();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, super::show, vec![]);
        let radio = click(&out, "Artist radio");
        let (events, _) = pass(&egui, &tx, &model, super::show, vec![radio]);
        assert!(
            matches!(&events[..], [Event::Navigate(Route::Mix(id))] if id == "m"),
            "{events:?}"
        );
    }

    #[test]
    fn no_artist_mix_no_button() {
        let model = with_mix(None);
        let egui = egui::Context::default();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (_, out) = pass(&egui, &tx, &model, super::show, vec![]);
        let shown = shown(&out);
        assert!(shown.contains(&"Artist 1".into()), "{shown:?}");
        assert!(!shown.contains(&"Artist radio".into()), "{shown:?}");
    }

    #[test]
    fn bio_is_cut_at_300_chars() {
        assert_eq!(super::excerpt("short", 300), "short");
        let long = "é".repeat(301);
        assert_eq!(super::excerpt(&long, 300), format!("{}…", "é".repeat(300)));
        assert_eq!(super::excerpt(&long[..600], 300), long[..600]);
    }
}
