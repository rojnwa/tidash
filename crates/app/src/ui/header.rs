//! A page's header: artwork, title, lines of detail and Play.

use super::Ctx;
use crate::i18n::tr;
use crate::model::Action;
use egui::{Sense, vec2};

const COVER: f32 = 200.0;

/// `details` draws the lines under the title. With `play`, a Play button sends all of it from
/// the start (disabled when empty).
pub fn header(
    ui: &mut egui::Ui,
    ctx: &mut Ctx,
    image: Option<&tidal::Image>,
    title: &str,
    details: impl FnOnce(&mut egui::Ui, &mut Ctx),
    play: Option<&[tidal::Track]>,
) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(COVER, COVER), Sense::hover());
        super::cover(ui, ctx.images, ctx.tx, image, rect, 6.0);
        ui.vertical(|ui| {
            ui.heading(title);
            details(ui, ctx);
            if let Some(list) = play {
                ui.add_space(8.0);
                let button = egui::Button::new(tr("Play"));
                if ui.add_enabled(!list.is_empty(), button).clicked() {
                    ctx.send(Action::PlayTracks {
                        tracks: list.to_vec(),
                        start: 0,
                    });
                }
            }
        });
    });
    ui.add_space(12.0);
}
