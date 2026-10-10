//! The player bar (spec §5.7), in three columns, about 1 : 1.5 : 1. Left, the cover, title
//! and artists; centre, shuffle, the transport and repeat over the seek row; right, the
//! format pill, the side panel's toggles (Queue, Lyrics, Credits) and the volume.

use super::widgets::{self, glyph_button, icon_button};
use super::{Side, cover, tracks};
use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Model, OutputInfo};
use crate::queue::Repeat;
use crate::theme::{self, ICON_BUTTON, Icon, PLAY_GLYPH, Palette, SKIP_GLYPH, TRANSPORT, Tier};
use egui::{
    Align, Color32, Frame, Label, Layout, Panel, Rect, RichText, Slider, Stroke, Ui, UiBuilder,
    Vec2, pos2, vec2,
};
use player::{PlayState, Player};
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
        // The right column fits its controls and the whole pill (ruling 25).
        let gap = ui.spacing().item_spacing.x;
        let pill = pb.output.as_ref().map(short_badge);
        let pill = pill.map_or(0.0, |text| gap + widgets::badge_width(ui, &text));
        let [left, centre, right] = columns(ui.max_rect(), controls_width(ui) + pill);
        now_playing(ui, p, images, tx, pb.track.as_ref(), left);
        transport(ui, p, model, tx, player, state, centre);
        extras(ui, p, model, tx, side, right);
    });
    let edge = shown.response.rect;
    let stroke = Stroke::new(1.0, p.outline);
    ui.painter().hline(edge.x_range(), edge.top() + 0.5, stroke);
}

/// The bar's columns inside `PLAYER_PAD` at either end, each the bar's height: the centre
/// 1.5 / 3.5 of the width, the right 1 / 3.5 or `right` if wider, the left the rest.
fn columns(bar: Rect, right: f32) -> [Rect; 3] {
    let x = bar.shrink2(vec2(theme::PLAYER_PAD, 0.0));
    let unit = x.width() / 3.5;
    let b = x.right() - right.max(unit);
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
        ui.ctx()
            .accesskit_node_builder(pill.id, |node| node.set_label(full.as_str()));
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
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let p = Palette::dark();
        let (mut state, mut side) = (State::default(), None);
        let mut images = TextureCache::new(1 << 20);
        let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 600.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let (images, side) = (&mut images, &mut side);
            show(ui, model, &tx, None, &mut state, images, side, &p, p.panel);
        });
        out.textures_delta.clear(); // nothing renders them
        out
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

    /// From the 960 px minimum up, a 24/176.4 stream shared at 48 kHz shows its whole short
    /// pill in the right column, before Queue; screen readers hear the full format (ruling 25).
    #[test]
    fn the_format_pill_shows_whole_from_960_px() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let o = output(176_400, 24, false, 48_000);
        let (full, short) = (badge(&o), short_badge(&o));
        let mut model = Model::default();
        model.playback.track = Some(testing::track(1));
        model.playback.output = Some(o);
        for width in [960.0, 1280.0] {
            let out = bar(&ctx, &model, width);
            let pill = testing::bounds(&out, &full);
            let (_, end) = centre(&out, width);
            assert!(pill.left() >= end - 0.5, "{width}: {pill:?} before {end}");
            let queue = testing::bounds(&out, "Queue");
            assert!(pill.right() <= queue.left(), "{width}: {pill:?} {queue:?}");
            let text = painted(&out, &short);
            assert!(matches!(text[..], [t] if !t.galley.elided), "{width}: cut");
        }
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
