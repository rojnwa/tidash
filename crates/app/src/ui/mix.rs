//! Mix page: header with Play, then its tracks.

use super::{Ctx, header, tracks};
use crate::model::{PageData, Route};

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, route: &Route) {
    let Some(PageData::Mix { mix, tracks: list }) = super::page_data(ui, ctx, route) else {
        return;
    };
    super::page(ui, route, |ui| {
        let subtitle = |ui: &mut egui::Ui, _: &mut Ctx| {
            ui.label(&mix.subtitle);
        };
        header::header(
            ui,
            ctx,
            mix.image().as_ref(),
            &mix.title,
            subtitle,
            Some(list),
            Some(tidal::Favorite::Mix(mix.id.clone())),
        );
        tracks::show(ui, ctx, "mix-tracks", list, false);
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{PageData, Route};
    use crate::ui::testing::{every_state, mix, tracks};

    #[test]
    fn views_render_every_state() {
        let data = PageData::Mix {
            mix: mix(),
            tracks: tracks(2),
        };
        let texts = ["Mix", "Daily", "Play", "2. Track 2, Artist 1, 3:20"];
        every_state(super::show, Route::Mix("m1".into()), data, &texts);
    }
}
