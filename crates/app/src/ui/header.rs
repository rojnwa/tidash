//! A page's header: artwork, title, lines of detail, Play, the favorite heart and the page's
//! own buttons.

use super::Ctx;
use crate::i18n::tr;
use crate::model::Action;
use egui::{Sense, WidgetInfo, WidgetType, vec2};

const COVER: f32 = 200.0;

/// `details` draws the lines under the title. With `play`, a Play button sends all of it from
/// the start (disabled when empty); with `fav`, a heart adds it to the favorites or removes it.
/// `extra` draws the page's own buttons after them, in the same row.
#[expect(clippy::too_many_arguments)] // one call per page, each part named at the call
pub fn header(
    ui: &mut egui::Ui,
    ctx: &mut Ctx,
    image: Option<&tidal::Image>,
    title: &str,
    details: impl FnOnce(&mut egui::Ui, &mut Ctx),
    play: Option<&[tidal::Track]>,
    fav: Option<tidal::Favorite>,
    extra: impl FnOnce(&mut egui::Ui, &mut Ctx),
) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(COVER, COVER), Sense::hover());
        super::cover(ui, ctx.images, ctx.tx, image, rect, 6.0);
        ui.vertical(|ui| {
            ui.heading(title);
            details(ui, ctx);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if let Some(list) = play {
                    play_button(ui, ctx, list);
                }
                if let Some(item) = fav {
                    heart(ui, ctx, item);
                }
                extra(ui, ctx);
            });
        });
    });
    ui.add_space(12.0);
}

/// Plays `list` from the start; disabled when it is empty.
pub fn play_button(ui: &mut egui::Ui, ctx: &Ctx, list: &[tidal::Track]) {
    let button = egui::Button::new(tr("Play"));
    if ui.add_enabled(!list.is_empty(), button).clicked() {
        ctx.send(Action::PlayTracks {
            tracks: list.to_vec(),
            start: 0,
        });
    }
}

/// ♥ for a favorite, else ♡; a screen reader hears what a click does.
fn heart(ui: &mut egui::Ui, ctx: &mut Ctx, item: tidal::Favorite) {
    let label = ctx.favorite_label(&item);
    let glyph = if ctx.model.favorites.contains(&item) {
        "♥"
    } else {
        "♡"
    };
    let resp = ctx.favorite_button(ui, item, glyph);
    let enabled = resp.enabled();
    let resp = resp.on_hover_text(label.as_ref());
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label.as_ref()));
}
