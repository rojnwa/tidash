//! A card (spec §5.6): square artwork (round for artists) at the card's full width, a title
//! line and a subtitle line. Hovered or focused, the cover's lower half darkens.

use super::widgets::{focus_ring, line, paint_disc, vertical_gradient};
use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::Action;
use crate::theme::{self, Icon, Palette, TRANSPORT};
use egui::{
    Color32, CursorIcon, Mesh, Painter, Rect, Sense, UiBuilder, Vec2, WidgetInfo, WidgetType, pos2,
    vec2,
};
use std::f32::consts::FRAC_PI_2;
use tokio::sync::mpsc::UnboundedSender;

/// Between the cover and the title.
const GAP: f32 = 8.0;
/// Each text line.
const LINE: f32 = 22.0;
/// The play disc's inset from the cover's bottom right corner, and how far it rises as it
/// fades in.
const PLAY_INSET: f32 = 8.0;
const RISE: f32 = 8.0;

/// A card `width` wide, from the cover's top to the subtitle's bottom.
pub fn height(width: f32) -> f32 {
    width + GAP + 2.0 * LINE
}

/// One card `width` wide; the caller acts on its clicks. Screen readers hear "title, subtitle".
/// With `play` (`Some(true)` while the card's tracks play), an `accent` disc rises into the
/// cover's bottom right corner on hover or focus, a Tab stop of its own: play, or pause while
/// they play. The card's response, and whether the disc was clicked.
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
    play: Option<bool>,
    width: f32,
) -> (egui::Response, bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height(width)), Sense::click());
    let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
    resp.widget_info(|| {
        let label = if subtitle.is_empty() {
            title.to_owned()
        } else {
            format!("{title}, {subtitle}")
        };
        WidgetInfo::labeled(WidgetType::Button, true, label)
    });
    let art = Rect::from_min_size(rect.min, Vec2::splat(width));
    // Over the card, so it takes the clicks there; after it, so Tab meets it next. It is
    // painted later, over the cover.
    let corner = art.max - Vec2::splat(PLAY_INSET + TRANSPORT / 2.0);
    let disc = Rect::from_center_size(corner, Vec2::splat(TRANSPORT));
    let button = play.map(|playing| {
        let b = ui.interact(disc, resp.id.with("play"), Sense::click());
        let (icon, label) = if playing {
            (Icon::PauseFilled, tr("Pause {title}"))
        } else {
            (Icon::PlayFilled, tr("Play {title}"))
        };
        let label = label.replace("{title}", title);
        b.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
        let b = b.on_hover_text(label);
        (icon, b.on_hover_cursor(CursorIcon::PointingHand))
    });
    let played = button.as_ref().is_some_and(|(_, b)| b.clicked());
    let radius = if round { width / 2.0 } else { theme::R_ART };
    focus_ring(ui, &resp.clone().with_new_rect(art), radius);
    if resp.gained_focus() {
        resp.scroll_to_me(None); // the lines too: Tab along a shelf or down a grid
    }
    if !ui.is_rect_visible(rect) {
        return (resp, played);
    }
    super::cover(ui, images, tx, image, art, radius);
    let on_disc = |(_, b): &(Icon, egui::Response)| b.hovered() || b.has_focus();
    let on = resp.hovered() || resp.has_focus() || button.as_ref().is_some_and(on_disc);
    let shade = ui.ctx().animate_bool_with_time(resp.id, on, theme::OVERLAY);
    if shade > 0.0 {
        lower_half(ui.painter(), art, round, theme::SCRIM.gamma_multiply(shade));
    }
    if let Some((icon, b)) = &button {
        let rise = b.id.with("rise"); // `b.id`'s is the hover fade's
        let risen = ui.ctx().animate_bool_with_time(rise, on, theme::OVERLAY);
        if risen > 0.0 {
            let mut ui = ui.new_child(UiBuilder::new().max_rect(art));
            ui.multiply_opacity(risen);
            let at = disc.translate(vec2(0.0, (1.0 - risen) * RISE));
            paint_disc(&ui, p, b, at, *icon, p.accent, p.on_accent);
            focus_ring(&ui, b, TRANSPORT / 2.0);
        }
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
    (resp, played)
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
