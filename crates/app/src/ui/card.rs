//! A card (spec §5.6): square artwork (round for artists) at the card's full width, a title
//! line and a subtitle line. Hovered or focused, the cover's lower half darkens.

use super::widgets::{focus_ring, line, vertical_gradient};
use crate::images::TextureCache;
use crate::model::Action;
use crate::theme::{self, Palette};
use egui::{Color32, Mesh, Painter, Rect, Sense, Vec2, WidgetInfo, WidgetType, pos2, vec2};
use std::f32::consts::FRAC_PI_2;
use tokio::sync::mpsc::UnboundedSender;

/// Between the cover and the title.
const GAP: f32 = 8.0;
/// Each text line.
const LINE: f32 = 22.0;

/// A card `width` wide, from the cover's top to the subtitle's bottom.
pub fn height(width: f32) -> f32 {
    width + GAP + 2.0 * LINE
}

/// One card `width` wide; the caller acts on its clicks. Screen readers hear "title, subtitle".
#[expect(clippy::too_many_arguments)] // one caller, each part named at the call
pub fn card(
    ui: &mut egui::Ui,
    p: &Palette,
    images: &mut TextureCache,
    tx: &UnboundedSender<Action>,
    image: Option<&tidal::Image>,
    title: &str,
    subtitle: &str,
    round: bool,
    width: f32,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height(width)), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| {
        let label = if subtitle.is_empty() {
            title.to_owned()
        } else {
            format!("{title}, {subtitle}")
        };
        WidgetInfo::labeled(WidgetType::Button, true, label)
    });
    let art = Rect::from_min_size(rect.min, Vec2::splat(width));
    let radius = if round { width / 2.0 } else { theme::R_ART };
    focus_ring(ui, &resp.clone().with_new_rect(art), radius);
    if resp.gained_focus() {
        resp.scroll_to_me(None); // the lines too: Tab along a shelf or down a grid
    }
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    super::cover(ui, images, tx, image, art, radius);
    let on = resp.hovered() || resp.has_focus();
    let shade = ui.ctx().animate_bool_with_time(resp.id, on, theme::OVERLAY);
    if shade > 0.0 {
        lower_half(ui.painter(), art, round, theme::SCRIM.gamma_multiply(shade));
    }
    let lines = [
        (title, theme::body_strong(), p.text),
        (subtitle, theme::body(), p.secondary),
    ];
    for (n, (text, font, color)) in lines.into_iter().enumerate() {
        let at = pos2(art.left(), art.bottom() + GAP + n as f32 * LINE);
        ui.painter()
            .galley(at, line(ui, text, font, color, width), color);
    }
    resp
}

/// `color` over `art`'s lower half, clear at its middle to full at its bottom, inside the
/// cover's shape: a disc's half as strips between chords, closer together towards the bottom.
fn lower_half(painter: &Painter, art: Rect, round: bool, color: Color32) {
    if !round {
        let half = Rect::from_min_max(pos2(art.left(), art.center().y), art.max);
        vertical_gradient(painter, half, Color32::TRANSPARENT, color);
        return;
    }
    const STRIPS: u32 = 16;
    let (c, r) = (art.center(), art.width() / 2.0);
    let mut mesh = Mesh::default();
    for i in 0..=STRIPS {
        let (down, across) = (FRAC_PI_2 * i as f32 / STRIPS as f32).sin_cos();
        let ink = color.gamma_multiply(down);
        mesh.colored_vertex(pos2(c.x - r * across, c.y + r * down), ink);
        mesh.colored_vertex(pos2(c.x + r * across, c.y + r * down), ink);
        if i > 0 {
            let k = 2 * i;
            mesh.add_triangle(k - 2, k - 1, k);
            mesh.add_triangle(k - 1, k + 1, k);
        }
    }
    painter.add(mesh);
}
