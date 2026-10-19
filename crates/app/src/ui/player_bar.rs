//! The player bar (spec §5.7), in three columns, about 1 : 1.5 : 1. Left, the cover, title
//! and artists; centre, shuffle, the transport and repeat over the seek row; right, the
//! format pill, the side panel's toggles (Queue, Lyrics, Credits) and the volume.

use super::widgets::{self, glyph_button, icon_button};
use super::{Side, cover, tracks};
use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Event, Model, OutputInfo, Route};
use crate::queue::Repeat;
use crate::theme::{self, ICON_BUTTON, Icon, PLAY_GLYPH, Palette, SKIP_GLYPH, TRANSPORT, Tier};
use egui::style::HandleShape;
use egui::{
    Align, Color32, CursorIcon, Frame, Galley, Layout, Panel, Pos2, Rect, Response, RichText,
    Sense, Slider, Stroke, Ui, UiBuilder, Vec2, WidgetInfo, WidgetType, lerp, pos2, remap_clamp,
    vec2,
};
use player::{PlayState, Player};
use std::ops::RangeInclusive;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// The seek row at its widest.
const SEEK_MAX: f32 = 800.0;
const VOLUME: f32 = 90.0;

#[derive(Default)]
pub struct State {
    /// The seek slider's value (seconds) while dragged; it seeks on release.
    seek: Option<f32>,
}

/// `PLAYER_BAR` tall across the window on `fill`, a 1 px `outline` along its top.
#[expect(clippy::too_many_arguments)] // one call, each part named at it
pub fn show(
    ui: &mut Ui,
    model: &Model,
    tx: &UnboundedSender<Action>,
    player: Option<&Player>,
    state: &mut State,
    images: &mut TextureCache,
    side: &mut Option<Side>,
    p: &Palette,
    fill: Color32,
    events: &mut Vec<Event>,
) {
    let pb = &model.playback;
    // The position only moves while playing; nothing to repaint for otherwise, or when hidden.
    // This also moves the lyrics' highlight.
    if pb.state == Some(PlayState::Playing)
        && ui.ctx().input(|i| i.viewport().visible()) != Some(false)
    {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    let panel = Panel::bottom("player")
        .exact_size(theme::PLAYER_BAR)
        .show_separator_line(false)
        .frame(Frame::NONE.fill(fill));
    let shown = panel.show(ui, |ui| {
        // The right column fits its controls and the widest pill the exclusive setting can
        // show, whether anything plays or not, so the transport never moves (rulings 25, 28).
        let pill = widgets::badge_width(ui, &widest_pill(model.settings.audio.exclusive));
        let right = controls_width(ui) + ui.spacing().item_spacing.x + pill;
        let [left, centre, right] = columns(ui.max_rect(), right);
        now_playing(ui, p, images, tx, model, left, events);
        transport(ui, p, model, tx, player, state, centre);
        extras(ui, p, model, tx, side, right);
    });
    let edge = shown.response.rect;
    let stroke = Stroke::new(1.0, p.outline);
    ui.painter().hline(edge.x_range(), edge.top() + 0.5, stroke);
}

/// The bar's columns inside `PLAYER_PAD` at either end, each the bar's height: the centre
/// 1.5 / 3.5 of the width, the right 1 / 3.5 or `right` if wider, the left the rest. Below
/// the window's minimum width, the left keeps the cover and a glyph's width of title, and the
/// right gives way (its pill is cut).
fn columns(bar: Rect, right: f32) -> [Rect; 3] {
    let x = bar.shrink2(vec2(theme::PLAYER_PAD, 0.0));
    let unit = x.width() / 3.5;
    let left = theme::PLAYER_COVER + theme::COL_GAP + theme::GLYPH;
    let b = x.right() - right.min(x.width() - 1.5 * unit - left).max(unit);
    let a = b - 1.5 * unit;
    [
        x.with_max_x(a),
        x.with_min_x(a).with_max_x(b),
        x.with_min_x(b),
    ]
}

/// A child of `ui` laid out by `layout` in `rect`.
fn child(ui: &mut Ui, rect: Rect, layout: Layout) -> Ui {
    ui.new_child(UiBuilder::new().max_rect(rect).layout(layout))
}

/// The cover, and beside it the title in `label()` over the artists in `small()`
/// `secondary`, each cut to the column. All links (spec §3.8): the cover to where playback
/// started (the album without a source), the title to the album, each artist to theirs.
/// Nothing without a track.
fn now_playing(
    ui: &mut Ui,
    p: &Palette,
    images: &mut TextureCache,
    tx: &UnboundedSender<Action>,
    model: &Model,
    rect: Rect,
    events: &mut Vec<Event>,
) {
    let Some(t) = &model.playback.track else {
        return;
    };
    let size = Vec2::splat(theme::PLAYER_COVER);
    let art = Rect::from_min_size(pos2(rect.left(), rect.center().y - size.y / 2.0), size);
    cover(ui, images, tx, t.album.image().as_ref(), art, theme::R_ART);
    let album = tr("Go to album {title}").replace("{title}", &t.album.title);
    let (name, tip, to) = match &model.queue.source {
        Some(s) => {
            let name = tr("Playing from {name}").replace("{name}", &s.name);
            (name.clone(), name, s.route.clone())
        }
        None => {
            let tip = tr("Go to album").into_owned();
            (album.clone(), tip, Route::Album(t.album.id))
        }
    };
    let r = link(ui, art, &name, theme::R_ART, to, events);
    if r.hovered() {
        let scrim = theme::SCRIM.gamma_multiply(0.3); // a card's, lighter
        ui.painter().rect_filled(art, theme::R_ART, scrim);
    }
    r.on_hover_text(tip);

    let (x, right) = (art.right() + theme::COL_GAP, rect.right());
    let title = widgets::line(ui, &t.title, theme::label(), p.text, right - x);
    let line = ui.fonts_mut(|f| f.row_height(&theme::small()));
    let gap = ui.spacing().item_spacing.y;
    let top = rect.center().y - (title.size().y + gap + line) / 2.0;
    let y = top + title.size().y + gap;
    let to = Route::Album(t.album.id);
    let cut = title.elided;
    let r = text_link(ui, title, pos2(x, top), p.text, &album, to, events);
    if cut {
        r.on_hover_text(t.title.as_str()); // as a cut label shows it
    }
    // One galley per name, ", " between; the column's end cuts the last one shown with "…".
    let (mut left, mut cut) = (x, false);
    for (i, a) in t.artists.iter().enumerate() {
        let small = |text: &str, left: f32| {
            widgets::line(ui, text, theme::small(), p.secondary, right - left)
        };
        if i > 0 {
            let sep = small(", ", left);
            let width = sep.size().x;
            cut = sep.elided;
            ui.painter().galley(pos2(left, y), sep, p.secondary);
            left += width;
            if cut {
                break;
            }
        }
        let text = small(&a.name, left);
        let width = text.size().x;
        cut = text.elided;
        // Cut to "…" alone, no part of the name shows: no link.
        if cut && text.rows.iter().all(|r| r.glyphs.len() <= 1) {
            ui.painter().galley(pos2(left, y), text, p.secondary);
        } else {
            let name = tr("Go to artist {name}").replace("{name}", &a.name);
            let to = Route::Artist(a.id);
            text_link(ui, text, pos2(left, y), p.secondary, &name, to, events);
        }
        left += width;
        if cut {
            break;
        }
    }
    // Over the names, as a label: screen readers read the whole list, names past the cut too,
    // and cut, the line shows it on hover, over the links as well. No Tab stop.
    let all = tracks::artists(t);
    let row = Rect::from_min_max(pos2(x, y), pos2(left, y + line));
    let row = ui.allocate_rect(row, Sense::hover());
    row.widget_info(|| WidgetInfo::labeled(WidgetType::Label, ui.is_enabled(), &all));
    if cut {
        row.on_hover_text(all);
    }
}

/// A link over `rect` (spec §3.8): `name` to screen readers, the hand, the focus ring with
/// corners of `radius`; clicked, it opens `to`.
fn link(
    ui: &mut Ui,
    rect: Rect,
    name: &str,
    radius: f32,
    to: Route,
    events: &mut Vec<Event>,
) -> Response {
    let r = ui.allocate_rect(rect, Sense::click());
    r.widget_info(|| WidgetInfo::labeled(WidgetType::Link, ui.is_enabled(), name));
    widgets::focus_ring(ui, &r, radius);
    if r.clicked() {
        events.push(Event::Navigate(to));
    }
    r.on_hover_cursor(CursorIcon::PointingHand)
}

/// `text` in `color` at `at`, a `link`, underlined while hovered: 1 px of `color` along its
/// foot. As a label's, its text is its node's value.
fn text_link(
    ui: &mut Ui,
    text: Arc<Galley>,
    at: Pos2,
    color: Color32,
    name: &str,
    to: Route,
    events: &mut Vec<Event>,
) -> Response {
    let rect = Rect::from_min_size(at, text.size());
    let r = link(ui, rect, name, theme::R_BADGE, to, events);
    ui.ctx()
        .accesskit_node_builder(r.id, |node| node.set_value(text.text()));
    if r.hovered() {
        let foot = rect.bottom() - 0.5;
        ui.painter()
            .hline(rect.x_range(), foot, Stroke::new(1.0, color));
    }
    ui.painter().galley(at, text, color);
    r
}

/// Shuffle, Previous, Play/Pause, Next and Repeat, centred over the seek row: the slider
/// between the position and the duration, at most `SEEK_MAX` wide.
fn transport(
    ui: &mut Ui,
    p: &Palette,
    model: &Model,
    tx: &UnboundedSender<Action>,
    player: Option<&Player>,
    state: &mut State,
    rect: Rect,
) {
    let (pb, q) = (&model.playback, &model.queue);
    let send = |a: Action| {
        let _ = tx.send(a);
    };
    let has = pb.track.is_some();
    let gap = ui.spacing().item_spacing.x;
    let seek_h = ui.spacing().interact_size.y;
    let top = rect.center().y - (TRANSPORT + seek_h) / 2.0;

    let width = 2.0 * ICON_BUTTON + 3.0 * TRANSPORT + 4.0 * gap;
    let at = pos2(rect.center().x, top + TRANSPORT / 2.0);
    let row = Rect::from_center_size(at, vec2(width, TRANSPORT));
    let row = &mut child(ui, row, Layout::left_to_right(Align::Center));
    if icon_button(row, p, Icon::Shuffle, &tr("Shuffle"), Some(q.shuffle)).clicked() {
        send(Action::SetShuffle(!q.shuffle));
    }
    // Play/Pause sends `TogglePlay`: the backend's state decides, not this label's.
    let busy = matches!(pb.state, Some(PlayState::Playing | PlayState::Buffering));
    let play = if busy {
        (
            Icon::PauseFilled,
            tr("Pause"),
            PLAY_GLYPH,
            Action::TogglePlay,
        )
    } else {
        (Icon::PlayFilled, tr("Play"), PLAY_GLYPH, Action::TogglePlay)
    };
    let back = (
        Icon::SkipBackFilled,
        tr("Previous"),
        SKIP_GLYPH,
        Action::Previous,
    );
    let next = (
        Icon::SkipForwardFilled,
        tr("Next"),
        SKIP_GLYPH,
        Action::Next,
    );
    for (icon, label, glyph, action) in [back, play, next] {
        let bare = |ui: &mut Ui| glyph_button(ui, p, icon, &label, glyph);
        if row.add_enabled(has, bare).clicked() {
            send(action);
        }
    }
    let (icon, label, then) = match q.repeat {
        Repeat::Off => (Icon::Repeat, tr("Repeat off"), Repeat::All),
        Repeat::All => (Icon::Repeat, tr("Repeat all"), Repeat::One),
        Repeat::One => (Icon::Repeat1, tr("Repeat one"), Repeat::Off),
    };
    let on = Some(q.repeat != Repeat::Off);
    if icon_button(row, p, icon, &label, on).clicked() {
        send(Action::SetRepeat(then));
    }

    let at = pos2(rect.center().x, top + TRANSPORT + seek_h / 2.0);
    let seek = Rect::from_center_size(at, vec2(rect.width().min(SEEK_MAX), seek_h));
    let row = &mut child(ui, seek, Layout::left_to_right(Align::Center));
    let time = |ui: &mut Ui, secs: f32, layout| {
        let text = tracks::duration(secs as u32);
        let text = RichText::new(text).font(theme::small()).color(p.secondary);
        ui.allocate_ui_with_layout(vec2(theme::TIME_COL, seek_h), layout, |ui| ui.label(text));
    };
    let duration = pb.track.as_ref().map_or(0.0, |t| t.duration as f32);
    let pos = player
        .map_or(0.0, |p| p.position().as_secs_f32())
        .min(duration);
    let mut at = state.seek.unwrap_or(pos);
    time(row, at, Layout::right_to_left(Align::Center));
    row.spacing_mut().slider_width = (seek.width() - 2.0 * (theme::TIME_COL + gap)).max(0.0);
    let range = 0.0..=duration.max(1.0);
    let seek = row
        .add_enabled_ui(has, |ui| slider(ui, p, &mut at, range))
        .inner;
    if seek.dragged() {
        state.seek = Some(at);
    } else if seek.drag_stopped() || seek.changed() {
        state.seek = None;
        send(Action::Seek(Duration::from_secs_f32(at)));
    }
    time(row, duration, Layout::left_to_right(Align::Center));
}

/// The right column's controls: three toggles, the volume's icon and its slider.
fn controls_width(ui: &Ui) -> f32 {
    4.0 * (ICON_BUTTON + ui.spacing().item_spacing.x) + VOLUME
}

/// Right-aligned: the format pill, Queue, Lyrics and Credits, then the volume's icon and
/// slider. The pill shows the short format (ruling 25); its hover and its name for screen
/// readers, the whole.
fn extras(
    ui: &mut Ui,
    p: &Palette,
    model: &Model,
    tx: &UnboundedSender<Action>,
    side: &mut Option<Side>,
    rect: Rect,
) {
    let pb = &model.playback;
    let controls = rect.with_min_x(rect.right() - controls_width(ui));
    if let Some(o) = &pb.output {
        let slot = rect.with_max_x(controls.left() - ui.spacing().item_spacing.x);
        let slot = &mut child(ui, slot, Layout::right_to_left(Align::Center));
        let pill = widgets::badge(slot, p, &short_badge(o), p.tier(Tier::of(o)));
        let full = badge(o);
        // A label's text is its node's value; screen readers name it from that.
        ui.ctx()
            .accesskit_node_builder(pill.id, |node| node.set_value(full.as_str()));
        pill.on_hover_text(format!("{full}\n{}", badge_hover(o)));
    }
    let row = &mut child(ui, controls, Layout::left_to_right(Align::Center));
    for (view, icon, label) in [
        (Side::Queue, Icon::ListMusic, tr("Queue")),
        (Side::Lyrics, Icon::MicVocal, tr("Lyrics")),
        (Side::Credits, Icon::Info, tr("Credits")),
    ] {
        let on = *side == Some(view);
        if icon_button(row, p, icon, &label, Some(on)).clicked() {
            *side = (!on).then_some(view);
        }
    }
    let icon = match pb.volume {
        v if v <= 0.0 => Icon::VolumeX,
        v if v < 0.5 => Icon::Volume1,
        _ => Icon::Volume2,
    };
    row.add_enabled_ui(!volume_fixed(model), |ui| {
        let label = tr("Volume");
        let image = icon
            .image(p.secondary, theme::GLYPH)
            .alt_text(label.clone());
        let speaker = ui.add_sized(Vec2::splat(ICON_BUTTON), image);
        let speaker = speaker.on_hover_text(label);
        ui.spacing_mut().slider_width = VOLUME;
        let mut volume = pb.volume;
        if slider(ui, p, &mut volume, 0.0..=1.0)
            .labelled_by(speaker.id)
            .on_disabled_hover_text(tr("Volume is fixed in exclusive mode"))
            .changed()
        {
            let _ = tx.send(Action::SetVolume(volume));
        }
    });
}

/// egui's `Slider` over `range` for its input, keys and AccessKit node, painted as TIDAL's
/// (spec §5.7). It is `interact_size.y` tall, the seek row's height, so its rail sits on the
/// row's centre line, the times' and the volume icon's: 4 px `surface_active`, `text` up to
/// the value. Hovered, dragged or focused, over `SLIDER_HOVER` the rail grows to 6 px, the
/// played part turns `accent` and a 12 px `text` thumb grows in at the value; focused, the
/// focus ring too. Faded with the rest when disabled.
fn slider(ui: &mut Ui, p: &Palette, value: &mut f32, range: RangeInclusive<f32>) -> Response {
    ui.scope(|ui| {
        let v = &mut ui.style_mut().visuals;
        v.handle_shape = HandleShape::Rect { aspect_ratio: 0.0 }; // the value spans the width
        v.selection.bg_fill = Color32::TRANSPARENT; // egui paints nothing seen
        let w = &mut v.widgets;
        for state in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
        ] {
            state.bg_fill = Color32::TRANSPARENT;
            state.fg_stroke = Stroke::NONE;
        }
        let ends = *range.start()..=*range.end();
        let r = ui.add(Slider::new(value, range).show_value(false));
        let on = r.enabled() && (r.hovered() || r.dragged() || r.has_focus());
        let t = ui
            .ctx()
            .animate_bool_with_time(r.id, on, theme::SLIDER_HOVER);
        let y = r.rect.center().y;
        let half = lerp(4.0..=6.0, t) / 2.0;
        let rail = Rect::from_x_y_ranges(r.rect.x_range(), y - half..=y + half);
        let at = lerp(rail.x_range(), remap_clamp(*value, ends, 0.0..=1.0));
        let (painter, round) = (ui.painter(), rail.height() / 2.0);
        painter.rect_filled(rail, round, p.surface_active);
        let played = theme::blend(p.text, p.accent, t);
        painter.rect_filled(rail.with_max_x(at), round, played);
        if t > 0.0 {
            painter.circle_filled(pos2(at, y), 6.0 * t, p.text);
        }
        widgets::focus_ring(ui, &r, r.rect.height() / 2.0);
        r.on_hover_cursor(CursorIcon::PointingHand)
    })
    .inner
}

/// A rate in kHz, with no trailing ".0": "44.1", "96".
fn khz(rate: u32) -> String {
    (f64::from(rate) / 1000.0).to_string()
}

/// The stream's format: "24/96 FLAC".
fn format_of(o: &OutputInfo) -> String {
    format!("{}/{} {}", o.format.bits, khz(o.format.rate), o.codec)
}

/// The format and the output in short: "24/96 FLAC · Exclusive", "24/192 FLAC · Shared 48 kHz".
pub fn badge(o: &OutputInfo) -> String {
    let mode = if o.exclusive {
        tr("Exclusive").into_owned()
    } else if o.device_rate != o.format.rate {
        tr("Shared {rate} kHz").replace("{rate}", &khz(o.device_rate))
    } else {
        tr("Shared").into_owned()
    };
    format!("{} · {mode}", format_of(o))
}

/// The pill's text (ruling 25): the format, and the mode only when exclusive. "24/96 FLAC",
/// "24/96 FLAC · Exclusive".
fn short_badge(o: &OutputInfo) -> String {
    if o.exclusive { badge(o) } else { format_of(o) }
}

/// The widest pill text there can be, exclusive or not: "24/176.4 FLAC", the longest rate
/// TIDAL streams (Inter's figures are all one width, ruling 18).
fn widest_pill(exclusive: bool) -> String {
    short_badge(&OutputInfo {
        codec: "FLAC".into(),
        format: player::Format {
            rate: 176_400,
            bits: 24,
            channels: 2,
        },
        exclusive,
        device_rate: 176_400,
    })
}

/// Exclusive output plays at full scale; until the output is known, exclusive is assumed when
/// set.
fn volume_fixed(model: &Model) -> bool {
    let exclusive = model.settings.audio.exclusive;
    model
        .playback
        .output
        .as_ref()
        .map_or(exclusive, |o| o.exclusive)
}

fn badge_hover(o: &OutputInfo) -> String {
    if o.exclusive {
        tr("Exclusive: bit-perfect, volume fixed").into_owned()
    } else if o.device_rate != o.format.rate {
        tr("Shared: mixed with other apps, resampled to {rate} kHz")
            .replace("{rate}", &khz(o.device_rate))
    } else {
        tr("Shared: mixed with other apps").into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Source;
    use crate::ui::testing;

    fn output(rate: u32, bits: u16, exclusive: bool, device_rate: u32) -> OutputInfo {
        OutputInfo {
            codec: "FLAC".into(),
            format: player::Format {
                rate,
                bits,
                channels: 2,
            },
            exclusive,
            device_rate,
        }
    }

    #[test]
    fn badge_formats() {
        let badge =
            |rate, bits, exclusive, device_rate| badge(&output(rate, bits, exclusive, device_rate));
        assert_eq!(badge(96_000, 24, true, 96_000), "24/96 FLAC · Exclusive");
        assert_eq!(badge(44_100, 16, false, 44_100), "16/44.1 FLAC · Shared");
        assert_eq!(
            badge(192_000, 24, false, 48_000),
            "24/192 FLAC · Shared 48 kHz"
        );
        assert_eq!(
            badge(176_400, 24, true, 176_400),
            "24/176.4 FLAC · Exclusive"
        );
    }

    #[test]
    fn badge_hover_is_whole_sentences() {
        let resampled = "Shared: mixed with other apps, resampled to {rate} kHz";
        let pot = include_str!("../../po/tidash.pot");
        assert!(pot.contains(&format!("msgid \"{resampled}\"")), "one msgid");
        assert_eq!(
            badge_hover(&output(192_000, 24, false, 48_000)),
            resampled.replace("{rate}", "48")
        );
        assert_eq!(
            badge_hover(&output(44_100, 16, false, 44_100)),
            "Shared: mixed with other apps"
        );
    }

    #[test]
    fn short_badge_formats() {
        let short = |rate, bits, exclusive, device_rate| {
            short_badge(&output(rate, bits, exclusive, device_rate))
        };
        assert_eq!(short(44_100, 16, false, 44_100), "16/44.1 FLAC");
        assert_eq!(short(192_000, 24, false, 48_000), "24/192 FLAC");
        assert_eq!(short(96_000, 24, true, 96_000), "24/96 FLAC · Exclusive");
    }

    /// One pass of the bar alone, in a window `width` by 600.
    fn bar(ctx: &egui::Context, model: &Model, width: f32) -> egui::FullOutput {
        bar_at(ctx, model, width, 0.0, vec![])
    }

    /// `bar` at `time` seconds, fed `events`.
    fn bar_at(
        ctx: &egui::Context,
        model: &Model,
        width: f32,
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        pass(ctx, model, width, time, events).1
    }

    /// `bar_at`: the events it gave, and its output.
    fn pass(
        ctx: &egui::Context,
        model: &Model,
        width: f32,
        time: f64,
        events: Vec<egui::Event>,
    ) -> (Vec<Event>, egui::FullOutput) {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let p = Palette::dark();
        let (mut state, mut side, mut gave) = (State::default(), None, Vec::new());
        let mut images = TextureCache::new(1 << 20);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 600.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let (images, side, gave) = (&mut images, &mut side, &mut gave);
            show(
                ui, model, &tx, None, &mut state, images, side, &p, p.panel, gave,
            );
        });
        out.textures_delta.clear(); // nothing renders them
        (gave, out)
    }

    /// The text shapes in `out` reading `text`.
    fn painted<'a>(out: &'a egui::FullOutput, text: &str) -> Vec<&'a egui::epaint::TextShape> {
        (out.shapes.iter())
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t),
                _ => None,
            })
            .collect()
    }

    /// Where the centre column starts and ends in a bar `width` wide: 1.5 / 3.5 of the padded
    /// width, Play in its middle.
    fn centre(out: &egui::FullOutput, width: f32) -> (f32, f32) {
        let half = 0.75 * (width - 2.0 * theme::PLAYER_PAD) / 3.5;
        let mid = testing::bounds(out, "Play").center().x;
        (mid - half, mid + half)
    }

    /// At 800 px, a 400-letter title, or a right-to-left one, moves no transport control, and
    /// neither it nor the artists reach into the centre column; nor do the right column's
    /// controls.
    #[test]
    fn a_long_title_keeps_the_transport_in_place() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut model = Model::default();
        let mut play = |title: String| {
            let track = tidal::Track {
                title: title.clone(),
                ..testing::track(1)
            };
            model.playback.track = Some(track);
            let out = bar(&ctx, &model, 800.0);
            let (start, end) = centre(&out, 800.0);
            let left = [painted(&out, &title), painted(&out, "Artist 1")].concat();
            assert_eq!(left.len(), 2, "the title and the artists");
            for t in left {
                let rect = t.visual_bounding_rect();
                assert!(rect.right() <= start, "{title:.9}: {rect:?} past {start}");
            }
            assert!(testing::bounds(&out, "Repeat off").right() <= end);
            assert!(testing::bounds(&out, "Queue").left() >= end);
            testing::bounds(&out, "Play")
        };
        let short = play("Short".into());
        assert_eq!(play("W".repeat(400)), short);
        assert_eq!(play("ع".repeat(400)), short);
    }

    /// Where the node whose value is `value` is drawn: a label's text is its value.
    fn valued(out: &egui::FullOutput, value: &str) -> Rect {
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let node = nodes.iter().find(|(_, n)| n.value() == Some(value));
        let b = node.and_then(|(_, n)| n.bounds());
        let b = b.unwrap_or_else(|| panic!("no {value:?} placed"));
        let at = |x: f64, y: f64| pos2(x as f32, y as f32);
        Rect::from_min_max(at(b.x0, b.y0), at(b.x1, b.y1))
    }

    /// From the 960 px minimum up, the format pill shows whole in the right column, before
    /// Queue: a 24/176.4 stream shared at 48 kHz, and the widest, an exclusive 24/176.4 one
    /// with exclusive mode on. Screen readers hear the full format (ruling 25).
    #[test]
    fn the_format_pill_shows_whole_from_960_px() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut model = Model::default();
        model.playback.track = Some(testing::track(1));
        let shared = output(176_400, 24, false, 48_000);
        let exclusive = output(176_400, 24, true, 176_400);
        for (o, width) in [(&shared, 960.0), (&shared, 1280.0), (&exclusive, 960.0)] {
            model.settings.audio.exclusive = o.exclusive;
            model.playback.output = Some(o.clone());
            let out = bar(&ctx, &model, width);
            let pill = valued(&out, &badge(o));
            let (_, end) = centre(&out, width);
            assert!(pill.left() >= end - 0.5, "{width}: {pill:?} before {end}");
            let queue = testing::bounds(&out, "Queue");
            assert!(pill.right() <= queue.left(), "{width}: {pill:?} {queue:?}");
            let text = painted(&out, &short_badge(o));
            assert!(matches!(text[..], [t] if !t.galley.elided), "{width}: cut");
        }
    }

    /// At 960 and 1440 px, Play stays put whether anything plays and whatever the format;
    /// exclusive mode on, whether an exclusive stream plays or not (ruling 28).
    #[test]
    fn the_transport_stays_put_whatever_plays() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut model = Model::default();
        model.playback.track = Some(testing::track(1));
        let cases = [
            (false, vec![None, Some(output(44_100, 16, false, 44_100))]),
            (false, vec![Some(output(176_400, 24, false, 48_000))]),
            (true, vec![None, Some(output(176_400, 24, true, 176_400))]),
        ];
        for width in [960.0, 1440.0] {
            let mut play = None;
            for (exclusive, outputs) in &cases {
                model.settings.audio.exclusive = *exclusive;
                if *exclusive {
                    play = None; // the setting widens the pill there can be
                }
                for o in outputs {
                    model.playback.output = o.clone();
                    let at = testing::bounds(&bar(&ctx, &model, width), "Play");
                    assert_eq!(*play.get_or_insert(at), at, "{width} {o:?}");
                }
            }
        }
    }

    /// The discs in `out`.
    fn circles(out: &egui::FullOutput) -> Vec<&egui::epaint::CircleShape> {
        (out.shapes.iter())
            .filter_map(|s| match &s.shape {
                egui::Shape::Circle(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    /// The enabled sliders' rails in `out`, in paint order: the seek's, then the volume's.
    fn rails(out: &egui::FullOutput) -> Vec<Rect> {
        let rail = Palette::dark().surface_active;
        (out.shapes.iter())
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.fill == rail => Some(r.rect),
                _ => None,
            })
            .collect()
    }

    /// Where the sliders are, enabled or not, left to right: the seek, then the volume.
    fn sliders(out: &egui::FullOutput) -> Vec<Rect> {
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let at = |x: f64, y: f64| pos2(x as f32, y as f32);
        let mut found: Vec<_> = (nodes.iter())
            .filter(|(_, n)| n.role() == egui::accesskit::Role::Slider)
            .filter_map(|(_, n)| n.bounds())
            .map(|b| Rect::from_min_max(at(b.x0, b.y0), at(b.x1, b.y1)))
            .collect();
        found.sort_by(|a, b| a.left().total_cmp(&b.left()));
        found
    }

    /// The pointer moved to `at` at `from`, then passes every 0.1 s for 0.3 s: the last.
    fn hover(ctx: &egui::Context, model: &Model, at: egui::Pos2, from: f64) -> egui::FullOutput {
        let moved = || vec![egui::Event::PointerMoved(at)];
        let mut out = bar_at(ctx, model, 1000.0, from, moved());
        for i in 1..=3 {
            out = bar_at(ctx, model, 1000.0, from + f64::from(i) * 0.1, vec![]);
        }
        out
    }

    /// At rest the sliders show no thumb on a 4 px rail; hovered, the seek rail grows to 6 px,
    /// a 12 px stroke-less disc sits on its centre line, under the hand; the pointer gone, it
    /// shrinks back and the bar is still.
    #[test]
    fn slider_thumb_only_on_hover() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        theme::apply(&ctx, &Palette::dark());
        let mut model = Model::default();
        model.playback.track = Some(testing::track(1));
        let out = bar_at(&ctx, &model, 1000.0, 0.0, vec![]);
        assert!(circles(&out).is_empty(), "at rest: {:?}", circles(&out));
        let seek = rails(&out)[0];
        assert!((seek.height() - 4.0).abs() <= 0.5, "{seek:?}");
        let out = hover(&ctx, &model, seek.center(), 0.0);
        let rail = rails(&out)[0];
        assert!((rail.height() - 6.0).abs() <= 0.5, "{rail:?}");
        let thumb = circles(&out);
        assert!(matches!(thumb[..], [_]), "{thumb:?}");
        let c = thumb[0];
        assert!(
            (c.radius - 6.0).abs() <= 0.5 && c.stroke.is_empty(),
            "{c:?}"
        );
        assert!(
            (c.center.y - rail.center().y).abs() <= 0.5,
            "{c:?} {rail:?}"
        );
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::PointingHand);
        let out = hover(&ctx, &model, pos2(10.0, 10.0), 0.4);
        assert!(circles(&out).is_empty(), "left: {:?}", circles(&out));
        let repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert_eq!(repaint, Duration::MAX, "repaints at rest");
    }

    /// The times sit on the seek rail's centre line, the volume rail on its icon's.
    #[test]
    fn times_sit_on_the_seek_rail() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        theme::apply(&ctx, &Palette::dark());
        let mut model = Model::default();
        model.playback.track = Some(testing::track(1));
        let out = bar(&ctx, &model, 1000.0);
        let [seek, volume] = rails(&out)[..] else {
            panic!("two rails: {:?}", rails(&out));
        };
        for text in ["0:00", "3:20"] {
            let t = painted(&out, text);
            let t = t.first().unwrap_or_else(|| panic!("no {text:?}"));
            let y = t.galley.rect.translate(t.pos.to_vec2()).center().y;
            assert!(
                (y - seek.center().y).abs() <= 0.5,
                "{text}: {y} vs {seek:?}"
            );
        }
        let icon = testing::bounds(&out, "Volume");
        assert!(
            (volume.center().y - icon.center().y).abs() <= 0.5,
            "{volume:?} {icon:?}"
        );
    }

    /// A disabled slider shows no thumb when hovered, and the arrow: the volume in exclusive
    /// mode, the seek with nothing playing.
    #[test]
    fn a_disabled_slider_shows_no_thumb() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        theme::apply(&ctx, &Palette::dark());
        let mut model = Model::default();
        model.playback.track = Some(testing::track(1));
        model.settings.audio.exclusive = true; // the volume fixed
        let volume = sliders(&bar(&ctx, &model, 1000.0))[1];
        let out = hover(&ctx, &model, volume.center(), 0.0);
        assert!(circles(&out).is_empty(), "volume: {:?}", circles(&out));
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::Default);
        model.settings.audio.exclusive = false;
        model.playback.track = None;
        let seek = sliders(&bar_at(&ctx, &model, 1000.0, 0.4, vec![]))[0];
        let out = hover(&ctx, &model, seek.center(), 0.5);
        assert!(circles(&out).is_empty(), "seek: {:?}", circles(&out));
    }

    /// Nothing playing, the left column is empty: no cover placeholder.
    #[test]
    fn no_track_no_cover() {
        let ctx = testing::context();
        let out = bar(&ctx, &Model::default(), 1000.0);
        let cover = Vec2::splat(theme::PLAYER_COVER);
        let placeholder = out.shapes.iter().any(|s| match &s.shape {
            egui::Shape::Rect(r) => r.rect.size() == cover,
            _ => false,
        });
        assert!(!placeholder);
    }

    /// The label of the node with the keyboard focus in `out`.
    fn focused(out: &egui::FullOutput) -> Option<String> {
        let tree = out.platform_output.accesskit_update.as_ref();
        let tree = tree.expect("AccessKit is on");
        let node = tree.nodes.iter().find(|(id, _)| *id == tree.focus)?;
        node.1.label().map(str::to_owned)
    }

    /// Spec §3.8: the cover opens where playback started (the album without a source), the
    /// title the album, each artist theirs. Hovered, a link shows the hand and an underline,
    /// the cover a light scrim; Tab reaches them before the transport.
    #[test]
    fn the_now_playing_column_links_out() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let p = Palette::dark();
        theme::apply(&ctx, &p);
        let mut model = Model::default();
        model.playback.track = Some(testing::track(1));
        let from = Route::Playlist("p-1".into());
        let source = Source {
            route: from.clone(),
            name: "Playlist".into(),
        };
        model.queue.source = Some(source.clone());
        let tab = || vec![testing::key(egui::Key::Tab)];
        let order: Vec<_> = (0..4)
            .map(|_| focused(&bar_at(&ctx, &model, 1000.0, 0.0, tab())))
            .collect();
        let want = [
            "Playing from Playlist",
            "Go to album Album 1",
            "Go to artist Artist 1",
            "Shuffle",
        ];
        assert_eq!(order, want.map(|l| Some(l.to_owned())));

        // Where the `n`th node labelled `label` leads, clicked by a screen reader.
        let opens = |model: &Model, label: &str, n: usize| {
            let out = bar(&ctx, model, 1000.0);
            let click = testing::clicks(&out, label).nth(n);
            let click = click.unwrap_or_else(|| panic!("no {label:?} #{n}"));
            match &pass(&ctx, model, 1000.0, 0.0, vec![click]).0[..] {
                [Event::Navigate(to)] => to.clone(),
                gave => panic!("{label} #{n}: {gave:?}"),
            }
        };
        assert_eq!(opens(&model, "Playing from Playlist", 0), from);
        assert_eq!(opens(&model, "Go to album Album 1", 0), Route::Album(1));
        assert_eq!(opens(&model, "Go to artist Artist 1", 0), Route::Artist(1));
        model.queue.source = None;
        let out = bar(&ctx, &model, 1000.0);
        let albums = testing::clicks(&out, "Go to album Album 1").count();
        assert_eq!(albums, 2, "the cover and the title");
        for n in 0..2 {
            assert_eq!(opens(&model, "Go to album Album 1", n), Route::Album(1));
        }

        // The underline: 1 px `text` along the foot of `title`.
        let underlined = |out: &egui::FullOutput, title: Rect| {
            out.shapes.iter().any(|s| match &s.shape {
                egui::Shape::LineSegment {
                    points: [a, b],
                    stroke,
                } => {
                    stroke.color == p.text
                        && stroke.width == 1.0
                        && (a.y - title.bottom()).abs() <= 1.0
                        && a.x.min(b.x) <= title.left() + 0.5
                        && a.x.max(b.x) >= title.right() - 0.5
                }
                _ => false,
            })
        };
        let scrim = theme::SCRIM.gamma_multiply(0.3);
        let scrimmed = |out: &egui::FullOutput| {
            (out.shapes.iter()).any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.fill == scrim))
        };
        model.queue.source = Some(source);
        let out = bar(&ctx, &model, 1000.0);
        let title = testing::bounds(&out, "Go to album Album 1");
        assert!(!underlined(&out, title) && !scrimmed(&out), "at rest");
        let out = hover(&ctx, &model, title.center(), 0.0);
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::PointingHand);
        assert!(underlined(&out, title), "no underline under {title:?}");
        let art = testing::bounds(&out, "Playing from Playlist");
        let out = hover(&ctx, &model, art.center(), 0.4);
        assert_eq!(out.platform_output.cursor_icon, CursorIcon::PointingHand);
        assert!(
            scrimmed(&out) && !underlined(&out, title),
            "the cover hovered"
        );
    }

    /// The column's end cuts the artists with "…": a name partly shown is still a link, one
    /// past the cut is not shown, and nothing reaches into the centre column; screen readers
    /// and the line's hover still give the whole list. At 800 px the column fits "…" alone of
    /// the first name: no link.
    #[test]
    fn artists_are_cut_at_the_column() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let long = "W".repeat(100);
        let wide = tidal::Artist {
            name: long.clone(),
            ..testing::artist(2)
        };
        let mut model = Model::default();
        model.playback.track = Some(tidal::Track {
            artists: vec![testing::artist(1), wide, testing::artist(3)],
            ..testing::track(1)
        });
        let out = bar(&ctx, &model, 1280.0);
        let shown = testing::shown(&out);
        let link = |name: &str| shown.contains(&format!("Go to artist {name}"));
        assert!(link("Artist 1") && link(&long), "{shown:?}");
        assert!(!link("Artist 3") && painted(&out, "Artist 3").is_empty());
        let (start, _) = centre(&out, 1280.0);
        let cut = testing::bounds(&out, &format!("Go to artist {long}"));
        assert!(cut.right() <= start, "{cut:?} past {start}");
        // Screen readers still read every name.
        let all = format!("Artist 1, {long}, Artist 3");
        assert!(shown.contains(&all), "no whole list in {shown:?}");
        let out = bar(&ctx, &model, 800.0);
        let first = painted(&out, "Artist 1");
        assert!(matches!(first[..], [t] if t.galley.elided), "cut");
        let shown = testing::shown(&out);
        assert!(
            !shown.iter().any(|s| s.starts_with("Go to artist")),
            "{shown:?}"
        );
        // Resting on the cut name, the line's tooltip gives the whole list.
        let moved = vec![egui::Event::PointerMoved(cut.center())];
        bar_at(&ctx, &model, 1280.0, 1.0, moved);
        bar_at(&ctx, &model, 1280.0, 2.0, vec![]); // still now: the tooltip's next pass
        let out = bar_at(&ctx, &model, 1280.0, 3.0, vec![]);
        assert!(!painted(&out, &all).is_empty(), "no whole list on hover");
    }

    #[test]
    fn volume_is_disabled_until_the_output_is_known_in_exclusive_mode() {
        let mut m = Model::default();
        assert!(!volume_fixed(&m), "shared, no output yet");
        m.settings.audio.exclusive = true;
        assert!(volume_fixed(&m), "exclusive, no output yet");
        m.playback.output = Some(output(96_000, 24, false, 48_000));
        assert!(!volume_fixed(&m), "fell back to shared");
        m.playback.output = Some(output(96_000, 24, true, 96_000));
        assert!(volume_fixed(&m), "exclusive");
    }
}
