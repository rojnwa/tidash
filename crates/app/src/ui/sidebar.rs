//! Left navigation (spec §5.2): the top-level pages, the collection's tabs, then the
//! playlists; collapsible to icons. History, Search and Settings are the top bar's.

use super::widgets::{icon_button, nav_row};
use super::{Ctx, collection, library};
use crate::i18n::tr;
use crate::model::{Action, CollectionTab, Event, Route};
use crate::theme::{self, Icon, Palette};
use egui::{
    Align, Direction, Frame, Layout, Margin, Panel, RichText, Sense, Stroke, Ui, UiBuilder, vec2,
};

/// `SIDEBAR` wide, or `SIDEBAR_COLLAPSED` as the settings say, sliding between the two, with
/// a 1 px `outline` on its right edge.
pub fn show(ui: &mut Ui, ctx: &mut Ctx, library: &mut library::State) {
    let p = ctx.palette;
    let frame = Frame::NONE
        .fill(p.window)
        .inner_margin(Margin::symmetric(theme::SIDEBAR_PAD as i8, 0));
    let panel = |id: &'static str, size| {
        Panel::left(id)
            .exact_size(size)
            .resizable(false)
            .show_separator_line(false)
            .frame(frame)
    };
    let mut expanded = !ctx.model.settings.sidebar_collapsed;
    let shown = Panel::show_switched(
        ui,
        &mut expanded,
        panel("sidebar-collapsed", theme::SIDEBAR_COLLAPSED),
        panel("sidebar", theme::SIDEBAR),
        |ui, expanded| contents(ui, ctx, library, !expanded),
    );
    let edge = shown.response.rect;
    let stroke = Stroke::new(1.0, p.outline);
    ui.painter()
        .vline(edge.right() - 0.5, edge.y_range(), stroke);
}

/// The collapse button, the pages, the collection's tabs and the library; `collapsed`, icons
/// and covers only.
fn contents(ui: &mut Ui, ctx: &mut Ctx, library: &mut library::State, collapsed: bool) {
    let (p, model) = (ctx.palette, ctx.model);
    // The setting, not the form drawn: mid-slide the button already says what a click does.
    let folded = model.settings.sidebar_collapsed;
    let label = if folded {
        tr("Expand sidebar")
    } else {
        tr("Collapse sidebar")
    };
    let layout = if collapsed {
        Layout::centered_and_justified(Direction::TopDown)
    } else {
        Layout::right_to_left(Align::Center)
    };
    let size = vec2(ui.available_width(), theme::TOP_BAR);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let mut top = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    if icon_button(&mut top, &p, Icon::PanelLeft, &label, None).clicked() {
        let mut new = model.settings.clone();
        new.sidebar_collapsed = !folded;
        ctx.events.push(Event::Settings(new.clone()));
        ctx.send(Action::SetSettings(new));
    }
    let route = model.route();
    let explore = Route::Page("pages/explore".into());
    for (icon, label, to) in [
        (Icon::House, tr("Home"), Route::Home),
        (Icon::Compass, tr("Explore"), explore),
    ] {
        if nav_row(ui, &p, icon, &label, *route == to, collapsed).clicked() {
            ctx.go(to);
        }
    }
    ui.add_space(theme::SIDEBAR_PAD);
    heading(ui, &p, &tr("Collection"), collapsed);
    for (tab, icon) in [
        (CollectionTab::Playlists, Icon::ListMusic),
        (CollectionTab::Albums, Icon::Disc3),
        (CollectionTab::Tracks, Icon::Music),
        (CollectionTab::Artists, Icon::User),
        (CollectionTab::Mixes, Icon::Radio),
    ] {
        let to = Route::Collection(tab);
        let name = collection::name(tab);
        if nav_row(ui, &p, icon, &name, *route == to, collapsed).clicked() {
            ctx.go(to);
        }
    }
    library::show(ui, ctx, library, collapsed);
}

/// A section's header: `text` in `small()` `dim`, sentence case; collapsed, a 1 px `outline`
/// rule in its place.
pub(super) fn heading(ui: &mut Ui, p: &Palette, text: &str, collapsed: bool) {
    if collapsed {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        let stroke = Stroke::new(1.0, p.outline);
        ui.painter().hline(rect.x_range(), rect.center().y, stroke);
    } else {
        ui.label(RichText::new(text).font(theme::small()).color(p.dim));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::ui::testing::{self, at, click, pass_with, playlist, shown};
    use tidal::FolderItem;

    /// Whether the node labelled `label` in `out` tells a screen reader it is selected: egui
    /// says so as AccessKit's toggled state, as for its own selectable labels.
    fn selected(out: &egui::FullOutput, label: &str) -> bool {
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let node = nodes.iter().find(|(_, n)| n.label() == Some(label));
        let node = node
            .unwrap_or_else(|| panic!("no {label:?} shown"))
            .1
            .clone();
        node.toggled() == Some(egui::accesskit::Toggled::True)
    }

    #[test]
    fn collection_rows_open_their_tab() {
        let egui = testing::context();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut library = library::State::default();
        let mut pass =
            |m: &Model, input| pass_with(&egui, &tx, m, input, |ui, c| show(ui, c, &mut library));
        let m = at(Route::Home);
        let (_, out) = pass(&m, vec![]);
        let (events, _) = pass(&m, vec![click(&out, "Albums")]);
        assert!(
            matches!(
                &events[..],
                [Event::Navigate(Route::Collection(CollectionTab::Albums))]
            ),
            "{events:?}"
        );
        let m = at(Route::Collection(CollectionTab::Albums));
        let (_, out) = pass(&m, vec![]);
        assert!(shown(&out).contains(&"Albums".into()));
        for (label, on) in [("Albums", true), ("Tracks", false), ("Home", false)] {
            assert_eq!(selected(&out, label), on, "{label}");
        }
    }

    /// "Collapse sidebar" saves the setting; collapsed, the rows keep their names and work.
    #[test]
    fn collapsing_flips_the_setting_and_rows_still_work() {
        let egui = testing::context();
        egui.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut library = library::State::default();
        let mut pass =
            |m: &Model, input| pass_with(&egui, &tx, m, input, |ui, c| show(ui, c, &mut library));
        let mut m = at(Route::Collection(CollectionTab::Tracks));
        let items = vec![FolderItem::Playlist(playlist())];
        m.apply(Event::Folder {
            id: "root".into(),
            items,
        });
        let (_, out) = pass(&m, vec![]);
        let (events, _) = pass(&m, vec![click(&out, "Collapse sidebar")]);
        let [Event::Settings(settings)] = &events[..] else {
            panic!("{events:?}")
        };
        assert!(settings.sidebar_collapsed);
        let sent: Vec<Action> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|a| !matches!(a, Action::LoadImage { .. })) // the playlist's cover
            .collect();
        assert!(
            matches!(&sent[..], [Action::SetSettings(s)] if s.sidebar_collapsed),
            "{sent:?}"
        );
        m.apply(Event::Settings(settings.clone()));
        let mut out = pass(&m, vec![]).1;
        for _ in 0..30 {
            out = pass(&m, vec![]).1; // the slide ends
        }
        assert!(shown(&out).contains(&"Expand sidebar".into()));
        for label in ["Home", "Playlist"] {
            let rect = testing::bounds(&out, label);
            assert!(
                rect.right() <= theme::SIDEBAR_COLLAPSED,
                "{label}: {rect:?}"
            );
        }
        let (events, _) = pass(&m, vec![click(&out, "Home")]);
        assert!(
            matches!(&events[..], [Event::Navigate(Route::Home)]),
            "{events:?}"
        );
        let (events, _) = pass(&m, vec![click(&out, "Playlist")]);
        assert!(
            matches!(&events[..], [Event::Navigate(Route::Playlist(p))] if p == "p-1"),
            "{events:?}"
        );
    }
}
