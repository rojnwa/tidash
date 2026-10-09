//! Left navigation: back and forward, then the top-level pages.

use crate::i18n::tr;
use crate::model::{CollectionTab, Event, Model, Route};

pub fn show(ui: &mut egui::Ui, model: &Model, events: &mut Vec<Event>) {
    egui::Panel::left("sidebar")
        .resizable(false)
        .show(ui, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(model.at > 0, egui::Button::new(tr("Back")))
                    .clicked()
                {
                    events.push(Event::Back);
                }
                let ahead = model.at + 1 < model.history.len();
                if ui
                    .add_enabled(ahead, egui::Button::new(tr("Forward")))
                    .clicked()
                {
                    events.push(Event::Forward);
                }
            });
            ui.add_space(8.0);
            let route = model.route();
            let explore = Route::Page("pages/explore".into());
            for (to, selected, label) in [
                (Route::Home, *route == Route::Home, tr("Home")),
                (explore.clone(), *route == explore, tr("Explore")),
                (Route::Search, *route == Route::Search, tr("Search")),
                (
                    Route::Collection(CollectionTab::Tracks),
                    matches!(route, Route::Collection(_)),
                    tr("Collection"),
                ),
                (Route::Settings, *route == Route::Settings, tr("Settings")),
            ] {
                if ui.selectable_label(selected, label).clicked() {
                    events.push(Event::Navigate(to));
                }
            }
        });
}
