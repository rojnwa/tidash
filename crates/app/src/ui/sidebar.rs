//! Left navigation: the top-level pages, then the playlists. History, Search and Settings are
//! the top bar's.

use super::{Ctx, library};
use crate::i18n::tr;
use crate::model::{CollectionTab, Route};

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, library: &mut library::State) {
    let model = ctx.model;
    egui::Panel::left("sidebar")
        .resizable(false)
        .show(ui, |ui| {
            ui.add_space(8.0);
            let route = model.route();
            let explore = Route::Page("pages/explore".into());
            for (to, selected, label) in [
                (Route::Home, *route == Route::Home, tr("Home")),
                (explore.clone(), *route == explore, tr("Explore")),
                (
                    Route::Collection(CollectionTab::Tracks),
                    matches!(route, Route::Collection(_)),
                    tr("Collection"),
                ),
            ] {
                if ui.selectable_label(selected, label).clicked() {
                    ctx.go(to);
                }
            }
            ui.add_space(16.0);
            library::show(ui, ctx, library);
        });
}
