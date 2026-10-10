//! The top bar (spec §5.3): history on the left; the search pill and the gear on the right.
//! Under it, the offline notice and the errors.

use super::widgets::{icon_button, search_field};
use super::{Ctx, search};
use crate::i18n::tr;
use crate::model::{Action, Event, Route};
use crate::theme::{self, Icon};
use egui::{Align, Key, Layout, Sense, Ui, UiBuilder, vec2};

/// `TOP_BAR` tall, inset `PAGE_PAD`, then the notices. Enter in the search pill with a
/// query searches and opens Search.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, search: &mut search::State) {
    let p = ctx.palette;
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), theme::TOP_BAR), Sense::hover());
    let row = rect.shrink2(vec2(theme::PAGE_PAD, 0.0));
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
    notices(ui, ctx);
}

/// Back and Forward on one `surface` pill, each disabled at its end of the history.
fn history(ui: &mut Ui, ctx: &mut Ctx) {
    let (p, model) = (ctx.palette, ctx.model);
    let inset = (theme::PILL - theme::ICON_BUTTON) / 2.0;
    let pill = egui::Frame::NONE
        .fill(p.surface)
        .corner_radius(theme::PILL / 2.0)
        .inner_margin(inset);
    pill.show(ui, |ui| {
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
    });
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
    use crate::model::{Action, Event, Model, Route};
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
}
