//! The player bar (spec §5.7), in three columns, 1 : 1.5 : 1. Left, the cover, title and
//! artists; centre, shuffle, the transport and repeat over the seek row; right, the format
//! badge, the side panel's toggles (Queue, Lyrics, Credits) and the volume.

use super::widgets::{self, circle_button, icon_button};
use super::{Side, cover, tracks};
use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Model, OutputInfo};
use crate::queue::Repeat;
use crate::theme::{self, ICON_BUTTON, Icon, Palette, TRANSPORT, Tier};
use egui::{
    Align, Color32, Frame, Label, Layout, Panel, Rect, RichText, Slider, Stroke, Ui, UiBuilder,
    Vec2, pos2, vec2,
};
use player::{PlayState, Player};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// The seek row at its widest.
const SEEK_MAX: f32 = 800.0;
/// The volume slider's width, given the room.
const VOLUME: f32 = 90.0;
/// The format badge's narrowest slot, a few characters in its pill; with less room it is
/// left out.
const BADGE_MIN: f32 = 64.0;

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
        let [left, centre, right] = columns(ui.max_rect());
        now_playing(ui, p, images, tx, pb.track.as_ref(), left);
        transport(ui, p, model, tx, player, state, centre);
        extras(ui, p, model, tx, side, right);
    });
    let edge = shown.response.rect;
    let stroke = Stroke::new(1.0, p.outline);
    ui.painter().hline(edge.x_range(), edge.top() + 0.5, stroke);
}

/// The bar's columns, 1 : 1.5 : 1 inside `PLAYER_PAD` at either end, each the bar's height.
fn columns(bar: Rect) -> [Rect; 3] {
    let x = bar.shrink2(vec2(theme::PLAYER_PAD, 0.0));
    let unit = x.width() / 3.5;
    let (a, b) = (x.left() + unit, x.right() - unit);
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
/// `secondary`, each cut to the column.
fn now_playing(
    ui: &mut Ui,
    p: &Palette,
    images: &mut TextureCache,
    tx: &UnboundedSender<Action>,
    track: Option<&tidal::Track>,
    rect: Rect,
) {
    let size = Vec2::splat(theme::PLAYER_COVER);
    let art = Rect::from_min_size(pos2(rect.left(), rect.center().y - size.y / 2.0), size);
    let image = track.and_then(|t| t.album.image());
    cover(ui, images, tx, image.as_ref(), art, theme::R_ART);
    let Some(t) = track else { return };
    let x = art.right() + theme::COL_GAP;
    let width = rect.right() - x;
    let title = widgets::line(ui, &t.title, theme::label(), p.text, width);
    let artists = tracks::artists(t);
    let artists = widgets::line(ui, &artists, theme::small(), p.secondary, width);
    let gap = ui.spacing().item_spacing.y;
    let mut y = rect.center().y - (title.size().y + gap + artists.size().y) / 2.0;
    for galley in [title, artists] {
        let at = Rect::from_min_size(pos2(x, y), galley.size());
        y = at.bottom() + gap;
        ui.put(at, Label::new(galley));
    }
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
        (Icon::PauseFilled, tr("Pause"), Action::TogglePlay)
    } else {
        (Icon::PlayFilled, tr("Play"), Action::TogglePlay)
    };
    for (icon, label, action) in [
        (Icon::SkipBackFilled, tr("Previous"), Action::Previous),
        play,
        (Icon::SkipForwardFilled, tr("Next"), Action::Next),
    ] {
        // No disc (ruling 8): the glyph alone, in `text`.
        let bare = |ui: &mut Ui| {
            let clear = Color32::TRANSPARENT;
            circle_button(ui, p, icon, &label, TRANSPORT, clear, p.text)
        };
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
    row.visuals_mut().selection.bg_fill = p.text; // the played part (spec §4.6)
    let slider = Slider::new(&mut at, 0.0..=duration.max(1.0)).show_value(false);
    let seek = row.add_enabled(has, slider);
    if seek.dragged() {
        state.seek = Some(at);
    } else if seek.drag_stopped() || seek.changed() {
        state.seek = None;
        send(Action::Seek(Duration::from_secs_f32(at)));
    }
    time(row, duration, Layout::left_to_right(Align::Center));
}

/// Right-aligned: the format badge (cut to the room left, or left out), Queue, Lyrics and
/// Credits, then the volume's icon and slider.
fn extras(
    ui: &mut Ui,
    p: &Palette,
    model: &Model,
    tx: &UnboundedSender<Action>,
    side: &mut Option<Side>,
    rect: Rect,
) {
    let pb = &model.playback;
    let gap = ui.spacing().item_spacing.x;
    let buttons = 4.0 * (ICON_BUTTON + gap); // three toggles and the volume's icon
    let volume = VOLUME.min(rect.width() - buttons).max(0.0);
    let controls = rect.with_min_x(rect.right() - buttons - volume);
    let room = controls.left() - gap - rect.left();
    if let Some(o) = pb.output.as_ref().filter(|_| room >= BADGE_MIN) {
        let slot = rect.with_max_x(rect.left() + room);
        let slot = &mut child(ui, slot, Layout::right_to_left(Align::Center));
        widgets::badge(slot, p, &badge(o), p.tier(Tier::of(o))).on_hover_text(badge_hover(o));
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
        ui.spacing_mut().slider_width = volume;
        ui.visuals_mut().selection.bg_fill = p.text;
        let mut volume = pb.volume;
        let slider = ui.add(Slider::new(&mut volume, 0.0..=1.0).show_value(false));
        if slider
            .labelled_by(speaker.id)
            .on_disabled_hover_text(tr("Volume is fixed in exclusive mode"))
            .changed()
        {
            let _ = tx.send(Action::SetVolume(volume));
        }
    });
}

/// A rate in kHz, with no trailing ".0": "44.1", "96".
fn khz(rate: u32) -> String {
    (f64::from(rate) / 1000.0).to_string()
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
    let (bits, rate) = (o.format.bits, khz(o.format.rate));
    format!("{bits}/{rate} {} · {mode}", o.codec)
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

    /// At 800 px, a 400-letter title, or a right-to-left one, moves no transport control, and
    /// neither it nor the artists reach into the centre column; nor do the right column's
    /// controls, a format badge with them.
    #[test]
    fn a_long_title_keeps_the_transport_in_place() {
        use crate::theme::PLAYER_PAD;
        use crate::ui::testing;
        use egui::Pos2;
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let p = Palette::dark();
        let (mut state, mut side) = (State::default(), None);
        let mut images = TextureCache::new(1 << 20);
        let mut model = Model::default();
        model.playback.output = Some(output(176_400, 24, false, 48_000));
        // 1 : 1.5 : 1 inside the padding.
        let unit = (800.0 - 2.0 * PLAYER_PAD) / 3.5;
        let (centre, right) = (PLAYER_PAD + unit, 800.0 - PLAYER_PAD - unit);
        let mut play = |title: String| {
            let track = tidal::Track {
                title: title.clone(),
                ..testing::track(1)
            };
            model.playback.track = Some(track);
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let (images, side) = (&mut images, &mut side);
                show(ui, &model, &tx, None, &mut state, images, side, &p, p.panel);
            });
            out.textures_delta.clear(); // nothing renders them
            let left: Vec<Rect> = (out.shapes.iter())
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) if [&*title, "Artist 1"].contains(&t.galley.text()) => {
                        Some(t.visual_bounding_rect())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(left.len(), 2, "the title and the artists");
            for rect in left {
                assert!(rect.right() <= centre, "{title:.9}: {rect:?} past {centre}");
            }
            assert!(testing::bounds(&out, "Repeat off").right() <= right);
            assert!(testing::bounds(&out, "Queue").left() >= right);
            testing::bounds(&out, "Play")
        };
        let short = play("Short".into());
        assert_eq!(play("W".repeat(400)), short);
        assert_eq!(play("ع".repeat(400)), short);
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
