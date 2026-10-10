//! A page's header (spec §5.4). Album, playlist and mix pages: the cover blurred behind the top
//! of the page, the cover beside the type line, title and lines of detail, and under them the
//! action row (Play, the favorite heart, the page's own pills). Artist pages: a hero, the
//! picture across the page with the name, details and actions over its bottom-left.

use super::{Ctx, widgets};
use crate::i18n::tr;
use crate::model::Action;
use crate::theme::{self, BACKDROP, HEADER_COVER, Icon, PAGE_PAD};
use egui::{Align, Color32, Layout, Rect, RichText, UiBuilder, Vec2, pos2};

/// `kind` is the type line ("Album"), shown upper-cased; `details` draws the lines under the
/// title, in `secondary`. Under the cover (ruling 22): with `play`, a Play pill sends all of it
/// from the start (disabled when empty); with `fav`, a heart adds it to the favorites or removes
/// it. `extra` draws the page's own pills after them, in the same row.
#[expect(clippy::too_many_arguments)] // one call per page, each part named at the call
pub fn header(
    ui: &mut egui::Ui,
    ctx: &mut Ctx,
    kind: &str,
    image: Option<&tidal::Image>,
    title: &str,
    details: impl FnOnce(&mut egui::Ui, &mut Ctx),
    play: Option<&[tidal::Track]>,
    fav: Option<tidal::Favorite>,
    extra: impl FnOnce(&mut egui::Ui, &mut Ctx),
) {
    let top = ui.cursor().top();
    backdrop(ui, ctx, image, top);
    let x = ui.max_rect().x_range();
    let cover = Rect::from_min_size(pos2(x.min, top + PAGE_PAD), Vec2::splat(HEADER_COVER));
    super::cover(ui, ctx.images, ctx.tx, image, cover, theme::R_ART);
    let left = cover.right() + PAGE_PAD;
    let beside = Rect::from_x_y_ranges(left..=x.max.max(left), cover.y_range());
    let drawn = column(ui, beside, 0.5, |ui| {
        info(ui, ctx, Some(kind), title, details)
    });
    end(ui, top, cover.bottom().max(drawn.bottom()));
    super::gap(ui);
    actions(ui, ctx, play, fav, extra);
    super::gap(ui);
}

/// An artist page's header (ruling 9): the picture across the page and its top `BACKDROP`
/// (from behind the top bar, `ui::behind_bar`), sharp, fading into `window` over its lower
/// 40%; over its bottom-left the name, `details`, the heart and `extra`. No type line, and
/// without a picture nothing behind them (ruling 23). The shelves after it keep their own gap.
pub fn hero(
    ui: &mut egui::Ui,
    ctx: &mut Ctx,
    image: Option<&tidal::Image>,
    title: &str,
    details: impl FnOnce(&mut egui::Ui, &mut Ctx),
    fav: Option<tidal::Favorite>,
    extra: impl FnOnce(&mut egui::Ui, &mut Ctx),
) {
    let top = ui.cursor().top();
    let (behind, from) = super::behind_bar(ui, top);
    let picture = Rect::from_x_y_ranges(behind.clip_rect().x_range(), from..=from + BACKDROP);
    if image.is_some() {
        super::cover(&behind, ctx.images, ctx.tx, image, picture, 0.0);
        let fade = picture.with_min_y(picture.bottom() - 0.4 * BACKDROP);
        let window = ctx.palette.window;
        widgets::vertical_gradient(behind.painter(), fade, Color32::TRANSPARENT, window);
    }
    let x = ui.max_rect().x_range();
    let over = Rect::from_x_y_ranges(x, top + PAGE_PAD..=picture.bottom() - PAGE_PAD);
    let drawn = column(ui, over, 1.0, |ui| {
        info(ui, ctx, None, title, details);
        ui.add_space(ui.spacing().item_spacing.y);
        actions(ui, ctx, None, fav, extra);
    });
    end(ui, top, picture.bottom().max(drawn.bottom()));
}

/// Plays `list` from the start; disabled when it is empty.
pub fn play_button(ui: &mut egui::Ui, ctx: &Ctx, list: &[tidal::Track]) {
    let p = ctx.palette;
    let pill =
        |ui: &mut egui::Ui| widgets::pill_button(ui, &p, Some(Icon::PlayFilled), &tr("Play"), true);
    if ui.add_enabled(!list.is_empty(), pill).clicked() {
        ctx.send(Action::PlayTracks {
            tracks: list.to_vec(),
            start: 0,
        });
    }
}

/// The cover at `TINT_PX` stretched over the page's full width from the header's `top` (or from
/// behind the top bar, `ui::behind_bar`), `BACKDROP` tall, under a fade from `window` at 60% to
/// `window`: TIDAL's blurred backdrop, scrolling with the page. Only that size is painted
/// (ruling 20): nothing until it loads.
fn backdrop(ui: &mut egui::Ui, ctx: &mut Ctx, image: Option<&tidal::Image>, top: f32) {
    let (ui, top) = super::behind_bar(ui, top);
    // The scroll's clip, not the page's padded width.
    let rect = Rect::from_x_y_ranges(ui.clip_rect().x_range(), top..=top + BACKDROP);
    if !ui.is_rect_visible(rect) {
        return;
    }
    let (px, tx) = (theme::TINT_PX, ctx.tx);
    let tex = image.and_then(|img| {
        ctx.images.get_exact(ui.ctx(), img.key(), px, || {
            let image = img.clone();
            let _ = tx.send(Action::LoadImage { image, px });
        })
    });
    let Some(tex) = tex else { return };
    let uv = super::crop(tex.aspect_ratio(), rect.aspect_ratio());
    ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
    let window = ctx.palette.window;
    widgets::vertical_gradient(ui.painter(), rect, window.gamma_multiply(0.6), window);
}

/// Draws `add` top-down across `rect`, `align` of the way down it (0.5 centred, 1 at the
/// bottom) by its height last pass; a new height has the pass drawn again (`request_discard`),
/// so it never shows out of place. Where it was drawn.
fn column(ui: &mut egui::Ui, rect: Rect, align: f32, add: impl FnOnce(&mut egui::Ui)) -> Rect {
    let id = ui.id().with("header height");
    let height = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    let top = rect.top() + ((rect.height() - height) * align).max(0.0);
    let max = Rect::from_x_y_ranges(rect.x_range(), top..=rect.bottom().max(top));
    let layout = Layout::top_down(Align::Min);
    let mut child = ui.new_child(UiBuilder::new().max_rect(max).layout(layout));
    add(&mut child);
    let drawn = child.min_rect();
    if (drawn.height() - height).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(id, drawn.height()));
        ui.ctx().request_discard("a page header's height changed");
    }
    drawn
}

/// Takes the block's place, from `top` to `bottom`.
fn end(ui: &mut egui::Ui, top: f32, bottom: f32) {
    ui.advance_cursor_after_rect(Rect::from_x_y_ranges(ui.max_rect().x_range(), top..=bottom));
}

/// The type line (if any) in `small()` `dim`, the title in `title()`, `details` in
/// `secondary`.
fn info(
    ui: &mut egui::Ui,
    ctx: &mut Ctx,
    kind: Option<&str>,
    title: &str,
    details: impl FnOnce(&mut egui::Ui, &mut Ctx),
) {
    let p = ctx.palette;
    if let Some(kind) = kind {
        let line = RichText::new(kind.to_uppercase()).font(theme::small());
        ui.label(line.color(p.dim));
    }
    ui.label(RichText::new(title).font(theme::title()));
    ui.scope(|ui| {
        ui.visuals_mut().override_text_color = Some(p.secondary);
        details(ui, ctx);
    });
}

/// Play, the heart, then the page's own pills, in a row that wraps.
fn actions(
    ui: &mut egui::Ui,
    ctx: &mut Ctx,
    play: Option<&[tidal::Track]>,
    fav: Option<tidal::Favorite>,
    extra: impl FnOnce(&mut egui::Ui, &mut Ctx),
) {
    ui.horizontal_wrapped(|ui| {
        if let Some(list) = play {
            play_button(ui, ctx, list);
        }
        if let Some(item) = fav {
            heart(ui, ctx, item);
        }
        extra(ui, ctx);
    });
}

/// A 40 px `surface` disc with a heart, filled for a favorite; a screen reader hears, and the
/// tooltip shows, what a click does.
fn heart(ui: &mut egui::Ui, ctx: &mut Ctx, item: tidal::Favorite) {
    let p = ctx.palette;
    let label = ctx.favorite_label(&item);
    let icon = if ctx.model.favorites.contains(&item) {
        Icon::HeartFilled
    } else {
        Icon::Heart
    };
    let disc = |ui: &mut egui::Ui| {
        widgets::circle_button(ui, &p, icon, &label, theme::PILL, p.surface, p.text)
    };
    ctx.favorite_button(ui, item, disc);
}
