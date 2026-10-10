//! The top bar (spec §5.3): history on the left; the search pill and the gear on the right.
//! Under it, the offline notice and the errors.

use super::widgets::{icon_button, search_field};
use super::{Ctx, search};
use crate::i18n::tr;
use crate::model::{Action, Event, Route};
use crate::theme::{self, Icon};
use egui::{Align, Key, LayerId, Layout, Rect, Ui, UiBuilder, vec2};

/// In `bar`, inset `PAGE_PAD`. Drawn before the page, so Tab and screen readers meet it first,
/// on a layer right above the page's, so it paints over and takes the clicks from what a page's
/// header paints up behind it (ruling 31). Enter in the search pill with a query searches and
/// opens Search.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, search: &mut search::State, bar: Rect) {
    let p = ctx.palette;
    let layer = LayerId::new(ui.layer_id().order, egui::Id::new("top bar"));
    ui.ctx().set_sublayer(ui.layer_id(), layer);
    let row = bar.shrink2(vec2(theme::PAGE_PAD, 0.0));
    let layout = Layout::left_to_right(Align::Center);
    let builder = UiBuilder::new()
        .layer_id(layer)
        .max_rect(row)
        .layout(layout);
    let mut bar = ui.new_child(builder);
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

    /// The textured shapes in `out` (covers, backdrops, pictures): each one's rect and clip.
    fn textured(out: &egui::FullOutput) -> Vec<(Rect, Rect)> {
        (out.shapes.iter())
            .filter_map(|s| match &s.shape {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => {
                    Some((m.calc_bounds(), s.clip_rect))
                }
                egui::Shape::Rect(r) if r.brush.is_some() => Some((r.rect, s.clip_rect)),
                _ => None,
            })
            .collect()
    }

    /// At scroll 0 the album's backdrop and the artist's hero start at the window's top, behind
    /// the bar, and show there (their clip reaches up too); Back, over them, still takes a
    /// pointer's click.
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
            // The backdrop takes only the tint's size; the hero any other while its own loads.
            for px in [theme::TINT_PX, 64] {
                let grey = egui::ColorImage::filled([px as usize; 2], egui::Color32::GRAY);
                state.images.insert(&ctx, key.into(), px, grey);
            }
            let (_, out) = pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]);
            let wide = textured(&out)
                .into_iter()
                .filter(|(r, _)| r.width() > theme::HEADER_COVER);
            let top = wide.map(|(r, clip)| r.top().max(clip.top()));
            let top = top.reduce(f32::min).expect("a backdrop");
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

    /// `model`'s page scrolled down `passes` steps of 100 pt, and still: the last step lands
    /// in the pass after it.
    fn scrolled(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        state: &mut UiState,
        model: &Model,
        passes: usize,
    ) {
        super::super::autoscroll(ctx, 6000.0);
        for _ in 0..passes {
            pass(ctx, tx, state, model, 1000.0, vec![]);
        }
        super::super::autoscroll(ctx, 0.0);
        pass(ctx, tx, state, model, 1000.0, vec![]);
    }

    /// Scrolled, the track list goes under no part of the bar: rows reach up there (a row's
    /// AccessKit bounds are its whole rect, cut or not, and `virtual_rows` builds one above the
    /// view), but what they paint is cut at the bar's bottom.
    #[test]
    fn the_page_scrolls_below_the_bar() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        theme::apply(&ctx, &Palette::dark());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = album_page(30);
        scrolled(&ctx, &tx, &mut state, &model, 8);
        let (_, out) = pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]);
        let tree = out.platform_output.accesskit_update.as_ref();
        let reached = (tree.expect("AccessKit is on").nodes.iter()).any(|(_, n)| {
            let row = n.label().is_some_and(|l| l.contains(". Track "));
            row && n.bounds().is_some_and(|b| (b.y0 as f32) < theme::TOP_BAR)
        });
        assert!(reached, "no row reached the bar");
        let titles: Vec<_> = (out.shapes.iter())
            .filter(|s| {
                let text = |t: &egui::epaint::TextShape| t.galley.text().starts_with("Track ");
                matches!(&s.shape, egui::Shape::Text(t) if text(t))
            })
            .collect();
        assert!(!titles.is_empty());
        for s in titles {
            assert!(
                s.clip_rect.top() >= theme::TOP_BAR - 0.5,
                "{:?}",
                s.clip_rect
            );
        }
    }

    /// The node `out` has focused: its label and where it is drawn.
    fn focused(out: &egui::FullOutput) -> Option<(String, Rect)> {
        let tree = out.platform_output.accesskit_update.as_ref();
        let tree = tree.expect("AccessKit is on");
        let node = tree.nodes.iter().find(|(id, _)| *id == tree.focus)?;
        let b = node.1.bounds()?;
        let at = |x: f64, y: f64| Pos2::new(x as f32, y as f32);
        let label = node.1.label()?.to_owned();
        Some((label, Rect::from_min_max(at(b.x0, b.y0), at(b.x1, b.y1))))
    }

    /// Tab from nothing focused reaches the bar's controls before the page's (ruling 31).
    #[test]
    fn the_top_bar_comes_first_in_tab_order() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        theme::apply(&ctx, &Palette::dark());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = album_page(3);
        pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]);
        let bar = ["Back", "Forward", "Search TIDAL", "Settings"];
        // The album's own controls: its Play pill (the player's is disabled), heart and pill.
        let page = ["Play", "Add to favorites", "Add to playlist"];
        for _ in 0..200 {
            let tab = vec![testing::key(egui::Key::Tab)];
            let (_, out) = pass(&ctx, &tx, &mut state, &model, 1000.0, tab);
            let Some((label, _)) = focused(&out) else {
                continue;
            };
            assert!(!page.contains(&label.as_str()), "{label} before the bar");
            if bar.contains(&label.as_str()) {
                return;
            }
        }
        panic!("Tab never reached the bar");
    }

    /// Scrolled down, Shift+Tab from the first row wholly in view to the one above it, cut by
    /// the bar's edge: the list scrolls it into view below the bar, not under it.
    #[test]
    fn focus_stays_clear_of_the_bar() {
        use egui::accesskit::{Action as Act, ActionRequest, TreeId};
        let ctx = testing::context();
        ctx.enable_accesskit();
        theme::apply(&ctx, &Palette::dark());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = album_page(30);
        scrolled(&ctx, &tx, &mut state, &model, 8);
        let (_, out) = pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]);
        let tree = out.platform_output.accesskit_update.as_ref();
        let rows = (tree.expect("AccessKit is on").nodes.iter()).filter_map(|(id, n)| {
            let row = n.label()?.contains(". Track ");
            let top = n.bounds()?.y0 as f32;
            (row && top >= theme::TOP_BAR).then_some((top, *id))
        });
        let (_, first) = rows
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .expect("rows in view");
        let focus = egui::Event::AccessKitActionRequest(ActionRequest {
            action: Act::Focus,
            target_tree: TreeId::ROOT,
            target_node: first,
            data: None,
        });
        let (_, out) = pass(&ctx, &tx, &mut state, &model, 1000.0, vec![focus]);
        let (from, _) = focused(&out).expect("a focused row");
        let back = egui::Event::Key {
            key: egui::Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        pass(&ctx, &tx, &mut state, &model, 1000.0, vec![back]);
        let mut out = pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]).1;
        for _ in 0..30 {
            out = pass(&ctx, &tx, &mut state, &model, 1000.0, vec![]).1;
        }
        let (to, at) = focused(&out).expect("a focused row");
        assert_ne!(to, from, "Shift+Tab moved");
        assert!(at.top() >= theme::TOP_BAR - 0.5, "{to}: {at:?}");
    }
}
