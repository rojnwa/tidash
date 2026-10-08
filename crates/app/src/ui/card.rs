//! A card: square artwork (round for artists), a title line and a subtitle line.

use crate::images::TextureCache;
use crate::model::Action;
use egui::text::{LayoutJob, TextWrapping};
use egui::{FontId, Sense, WidgetInfo, WidgetType, pos2, vec2};
use tokio::sync::mpsc::UnboundedSender;

pub const WIDTH: f32 = 172.0;
/// Artwork, two lines and the padding around them.
pub const HEIGHT: f32 = 222.0;
const COVER: f32 = 148.0;
const PAD: f32 = (WIDTH - COVER) / 2.0;
const LINE: f32 = 20.0;

/// One card; the caller acts on its clicks. Screen readers hear "title, subtitle".
pub fn card(
    ui: &mut egui::Ui,
    images: &mut TextureCache,
    tx: &UnboundedSender<Action>,
    image: Option<&tidal::Image>,
    title: &str,
    subtitle: &str,
    round: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(WIDTH, HEIGHT), Sense::click());
    resp.widget_info(|| {
        let label = if subtitle.is_empty() {
            title.to_owned()
        } else {
            format!("{title}, {subtitle}")
        };
        WidgetInfo::labeled(WidgetType::Button, true, label)
    });
    if resp.gained_focus() {
        resp.scroll_to_me(None); // Tab along a shelf or down a grid
    }
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let v = ui.visuals();
    if resp.hovered() || resp.has_focus() {
        ui.painter()
            .rect_filled(rect, 6.0, v.widgets.hovered.bg_fill);
    }
    let art = egui::Rect::from_min_size(rect.min + vec2(PAD, PAD), vec2(COVER, COVER));
    let radius = if round { COVER / 2.0 } else { 4.0 };
    super::cover(ui, images, tx, image, art, radius);
    let lines = [
        (title, 14.0, v.strong_text_color()),
        (subtitle, 13.0, v.weak_text_color()),
    ];
    for (n, (text, size, color)) in lines.into_iter().enumerate() {
        let job = LayoutJob {
            wrap: TextWrapping::truncate_at_width(COVER),
            ..LayoutJob::simple_singleline(text.to_owned(), FontId::proportional(size), color)
        };
        let at = pos2(art.left(), art.bottom() + 8.0 + n as f32 * LINE);
        ui.painter().galley(at, ui.painter().layout_job(job), color);
    }
    resp
}
