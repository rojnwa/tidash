//! The top bar (spec §5.3): history on the left; the search pill and the gear on the right.
//! Under it, the offline notice and the errors.

use super::widgets::{icon_button, search_field};
use super::{Ctx, search};
use crate::i18n::tr;
use crate::model::{Action, Event, Route};
use crate::theme::{self, Icon};
use egui::{Align, Key, Layout, Rect, Sense, Ui, UiBuilder, vec2};

/// In `bar`, inset `PAGE_PAD`, over the page drawn before it (`ui::page` runs behind it), so
/// its controls take the clicks; so does the rest of it, so nothing scrolled under it is hit.
/// Enter in the search pill with a query searches and opens Search.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, search: &mut search::State, bar: Rect) {
    let p = ctx.palette;
    // Not focusable: Tab and screen readers pass it by.
    ui.interact(bar, ui.id().with("top bar"), Sense::CLICK | Sense::DRAG);
    let row = bar.shrink2(vec2(theme::PAGE_PAD, 0.0));
    let layout = Layout::left_to_right(Align::Center);
    let mut bar = ui.new_child(UiBuilder::new().max_rect(row).layout(layout));
    history(&mut bar, ctx);
    // The gear first, at the right end: in a narrow window the search pill shrinks, the gear
    // stays.
    bar.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if icon_button(ui, &p, Icon::Settings, &tr("Settings"), None).clicked() {
            ctx.go(Route::Settings);
        }
        let width = ui
            .available_width()
            .clamp(theme::SEARCH_MIN, theme::SEARCH_W);
        let id = search::query_id();
        let hint = tr("Search TIDAL");
        let field = search_field(ui, &p, &mut search.query, id, &hint, width);
        let q = search.query.trim();
        if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !q.is_empty() {
            ctx.send(Action::Search(q.to_owned()));
            ctx.go(Route::Search);
        }
    });
}

/// Back and Forward on a `surface` pill `PILL` tall around them, each disabled at its end of
/// the history.
fn history(ui: &mut Ui, ctx: &mut Ctx) {
    let (p, model) = (ctx.palette, ctx.model);
    let inset = (theme::PILL - theme::ICON_BUTTON) / 2.0;
    let size = vec2(2.0 * (theme::ICON_BUTTON + inset), theme::PILL);
    // Painted by hand: a `Frame` here would take the bar's whole height.
    let (_, pill) = ui.allocate_space(size);
    ui.painter()
        .rect_filled(pill, pill.height() / 2.0, p.surface);
    let layout = Layout::left_to_right(Align::Center);
    let ui = &mut ui.new_child(UiBuilder::new().max_rect(pill.shrink(inset)).layout(layout));
    ui.spacing_mut().item_spacing.x = 0.0;
    let ahead = model.at + 1 < model.history.len();
    for (icon, label, enabled, event) in [
        (Icon::ChevronLeft, tr("Back"), model.at > 0, Event::Back),
        (Icon::ChevronRight, tr("Forward"), ahead, Event::Forward),
    ] {
        let button = ui.add_enabled_ui(enabled, |ui| icon_button(ui, &p, icon, &label, None));
        if button.inner.clicked() {
            ctx.events.push(event);
        }
    }
}

/// The offline notice and each error with its Dismiss, a `warning` alert icon before each,
/// inset `PAGE_PAD`; nothing without either.
pub fn notices(ui: &mut Ui, ctx: &mut Ctx) {
    let (p, model) = (ctx.palette, ctx.model);
    if !model.offline && model.errors.is_empty() {
        return;
    }
    let alert = |ui: &mut Ui| ui.add(Icon::CircleAlert.image(p.warning, theme::GLYPH));
    super::padded(ui, |ui| {
        if model.offline {
            ui.horizontal(|ui| {
                alert(ui);
                ui.label(tr("Offline — showing saved pages"));
            });
        }
        for (id, e) in &model.errors {
            ui.horizontal(|ui| {
                alert(ui);
                ui.label(e);
                if icon_button(ui, &p, Icon::X, &tr("Dismiss"), None).clicked() {
                    ctx.events.push(Event::DismissError(*id));
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::super::{Side, UiState, draw, testing};
    use crate::model::{Action, Event, Model, PageData, Route};
    use crate::theme::{self, Palette};
    use egui::{Pos2, Rect, vec2};
    use tokio::sync::mpsc::UnboundedSender;

    /// One pass of `draw` in a window `width` by 600, fed `events`: what it gave, and egui's
    /// output.
    fn pass(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        state: &mut UiState,
        model: &Model,
        width: f32,
        events: Vec<egui::Event>,
    ) -> (Vec<Event>, egui::FullOutput) {
        let input = egui::RawInput {
            events,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 600.0))),
            ..Default::default()
        };
        let mut events = Vec::new();
        let mut out = ctx.run_ui(input, |ui| events = draw(ui, model, tx, state));
        out.textures_delta.clear(); // nothing renders them
        (events, out)
    }

    /// Typed and Enter: a blank query does nothing; any other searches and opens Search.
    #[test]
    fn enter_searches_and_opens_the_search_page() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = testing::at(Route::Home);
        let mut pass = |events| pass(&ctx, &tx, &mut state, &model, 800.0, events);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        for (typed, searches) in [("  ", false), ("massive attack", true)] {
            // A pointer's click: a text field takes no focus from a screen reader's.
            let at = testing::middle(&pass(vec![]).1, "Search TIDAL");
            pass(vec![egui::Event::PointerMoved(at), button(at, true)]);
            pass(vec![button(at, false)]);
            let enter = testing::key(egui::Key::Enter);
            let (events, _) = pass(vec![egui::Event::Text(typed.into()), enter]);
            let opened = events
                .iter()
                .any(|e| matches!(e, Event::Navigate(Route::Search)));
            let sent: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok())
                .map(|a| format!("{a:?}"))
                .collect();
            assert_eq!(opened, searches, "{typed:?}: {events:?}");
            let want: &[&str] = if searches {
                &[r#"Search("massive attack")"#]
            } else {
                &[]
            };
            assert_eq!(sent, want, "{typed:?}");
        }
    }

    /// A narrow window, the side panel shut and open: the search pill gives way, not the gear.
    #[test]
    fn the_gear_stays_in_a_narrow_window() {
        let window = Rect::from_min_size(Pos2::ZERO, vec2(640.0, 600.0));
        for side in [None, Some(Side::Queue)] {
            let ctx = testing::context();
            ctx.enable_accesskit();
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let mut state = UiState {
                side,
                ..Default::default()
            };
            let model = testing::at(Route::Home);
            let (_, out) = pass(&ctx, &tx, &mut state, &model, window.width(), vec![]);
            let gear = testing::bounds(&out, "Settings");
            let field = testing::bounds(&out, "Search TIDAL");
            assert!(window.contains_rect(gear), "{side:?}: {gear:?}");
            assert!(gear.left() >= field.right(), "{side:?}: {gear:?} {field:?}");
        }
    }

    /// Where in the paint order `out` first paints a `fill` rect that `keep` keeps, and the rect.
    fn filled(
        out: &egui::FullOutput,
        fill: egui::Color32,
        keep: impl Fn(&Rect) -> bool,
    ) -> Option<(usize, Rect)> {
        out.shapes
            .iter()
            .enumerate()
            .find_map(|(i, s)| match &s.shape {
                egui::Shape::Rect(r) if r.fill == fill && keep(&r.rect) => Some((i, r.rect)),
                _ => None,
            })
    }

    /// Back and Forward sit on a pill that hugs them, in the middle of the bar's height.
    #[test]
    fn the_history_pill_is_centred_in_the_bar() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        theme::apply(&ctx, &Palette::dark());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = testing::at(Route::Home);
        let (_, out) = pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]);
        let back = testing::bounds(&out, "Back");
        let mid = theme::TOP_BAR / 2.0;
        assert!((back.center().y - mid).abs() <= 1.0, "{back:?}");
        let surface = Palette::dark().surface;
        let (_, pill) = filled(&out, surface, |r| r.contains(back.center())).expect("a pill");
        assert!(pill.height() <= theme::ICON_BUTTON + 8.0, "{pill:?}");
    }

    /// `route`'s page, with `data`, after Home: Back goes somewhere.
    fn after_home(route: Route, data: PageData) -> Model {
        let mut model = testing::at(Route::Home);
        model.navigate(route.clone());
        model.apply(Event::Page { route, data });
        model
    }

    fn album_page(tracks: u64) -> Model {
        let data = PageData::Album {
            album: testing::album(1),
            tracks: testing::tracks(tracks),
        };
        after_home(Route::Album(1), data)
    }

    /// The textured shapes' rects in `out`: covers, backdrops, pictures.
    fn textured(out: &egui::FullOutput) -> Vec<Rect> {
        let shapes = out.shapes.iter().map(|s| &s.shape);
        shapes
            .filter_map(|s| match s {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                    Some(m.calc_bounds())
                }
                egui::Shape::Rect(r) if r.brush.is_some() => Some(r.rect),
                _ => None,
            })
            .collect()
    }

    /// At scroll 0 the album's backdrop and the artist's hero start at the window's top, behind
    /// the bar, and Back, drawn over them, still takes a pointer's click.
    #[test]
    fn the_backdrop_starts_behind_the_top_bar() {
        let artist = PageData::Artist(tidal::ArtistPage {
            artist: testing::artist(1),
            bio: None,
            shelves: vec![],
        });
        let pages = [
            (album_page(3), "ab-cd"),
            (after_home(Route::Artist(1), artist), "12-34"),
        ];
        for (model, key) in pages {
            let ctx = testing::context();
            ctx.enable_accesskit();
            theme::apply(&ctx, &Palette::dark());
            let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
            let mut state = UiState::default();
            let px = theme::TINT_PX;
            let grey = egui::ColorImage::filled([px as usize; 2], egui::Color32::GRAY);
            state.images.insert(&ctx, key.into(), px, grey);
            let (_, out) = pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]);
            let wide = textured(&out)
                .into_iter()
                .filter(|r| r.width() > theme::HEADER_COVER);
            let top = wide.map(|r| r.top()).reduce(f32::min).expect("a backdrop");
            assert!(top <= 1.0, "{key}: {top}");
            let at = testing::middle(&out, "Back");
            let mut pass = |events| pass(&ctx, &tx, &mut state, &model, 1000.0, events).0;
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            pass(vec![egui::Event::PointerMoved(at), button(true)]);
            let events = pass(vec![button(false)]);
            assert!(matches!(events[..], [Event::Back]), "{key}: {events:?}");
        }
    }

    /// At its top the page shows through the bar; scrolled, `window` fills the bar's height
    /// across it, under the history pill.
    #[test]
    fn the_bar_is_opaque_once_scrolled() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        theme::apply(&ctx, &Palette::dark());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = album_page(30);
        let p = Palette::dark();
        let mut pass = || pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]).1;
        // The fill over the bar's height from the top, across Back to the gear, and its place
        // in the paint order.
        let fill = |out: &egui::FullOutput| {
            let (back, gear) = (
                testing::bounds(out, "Back"),
                testing::bounds(out, "Settings"),
            );
            let over = |r: &Rect| {
                r.top() <= 0.5
                    && (r.height() - theme::TOP_BAR).abs() < 0.5
                    && r.left() <= back.left()
                    && r.right() >= gear.right()
            };
            filled(out, p.window, over)
        };
        assert_eq!(fill(&pass()), None, "at the top");
        super::super::autoscroll(&ctx, 3000.0);
        let mut out = pass();
        for _ in 0..3 {
            out = pass();
        }
        let (at, _) = fill(&out).expect("a fill once scrolled");
        // Not the album's cover scrolled under it, a placeholder of the same colour.
        let back = testing::bounds(&out, "Back").center();
        let pill = |r: &Rect| r.contains(back) && r.height() <= theme::PILL;
        let (pill, _) = filled(&out, p.surface, pill).expect("the pill");
        assert!(at < pill, "the fill is under the pill");
    }
}
