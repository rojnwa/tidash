//! Left navigation: Search entry and back button.

use crate::i18n::tr;
use crate::model::{Event, Model, Route};

pub fn show(ui: &mut egui::Ui, model: &Model, events: &mut Vec<Event>) {
    egui::Panel::left("sidebar").resizable(false).show(ui, |ui| {
        ui.add_space(8.0);
        if ui.add_enabled(!model.history.is_empty(), egui::Button::new(tr("Back"))).clicked() {
            events.push(Event::Back);
        }
        ui.add_space(8.0);
        if ui.selectable_label(model.route == Route::Search, tr("Search")).clicked() {
            events.push(Event::Navigate(Route::Search));
        }
    });
}
