//! Player bar: cover, title and artists, transport with shuffle and repeat, seek, volume, the
//! format badge and the queue panel's toggle.

use super::{cover, tracks};
use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Model, OutputInfo};
use crate::queue::Repeat;
use egui::{Button, Label, RichText, Slider, vec2};
use player::{PlayState, Player};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

const COVER: f32 = 56.0;
/// The format badge's width: "24/176.4 FLAC · Shared 48 kHz" fits.
const BADGE: f32 = 190.0;

#[derive(Default)]
pub struct State {
    /// The seek slider's value (seconds) while dragged; it seeks on release.
    seek: Option<f32>,
}

pub fn show(
    ui: &mut egui::Ui,
    model: &Model,
    tx: &UnboundedSender<Action>,
    player: Option<&Player>,
    state: &mut State,
    images: &mut TextureCache,
    queue_open: &mut bool,
) {
    let pb = &model.playback;
    let send = |a: Action| {
        let _ = tx.send(a);
    };
    // The position only moves while playing; nothing to repaint for otherwise, or when hidden.
    if pb.state == Some(PlayState::Playing)
        && ui.ctx().input(|i| i.viewport().visible()) != Some(false)
    {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    egui::Panel::bottom("player").show(ui, |ui| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(COVER, COVER), egui::Sense::hover());
            let art = pb.track.as_ref().and_then(|t| t.album.image());
            cover(ui, images, tx, art.as_ref(), rect, 4.0);
            ui.allocate_ui(vec2(220.0, COVER), |ui| {
                ui.set_width(220.0); // fixed, so the controls do not move with the title
                ui.vertical(|ui| {
                    if let Some(t) = &pb.track {
                        ui.add(Label::new(RichText::new(&t.title).strong()).truncate());
                        ui.add(Label::new(tracks::artists(t)).truncate());
                    }
                });
            });

            let (has, q) = (pb.track.is_some(), &model.queue);
            if ui.selectable_label(q.shuffle, tr("Shuffle")).clicked() {
                send(Action::SetShuffle(!q.shuffle));
            }
            if ui.add_enabled(has, Button::new(tr("Previous"))).clicked() {
                send(Action::Previous);
            }
            let busy = matches!(pb.state, Some(PlayState::Playing | PlayState::Buffering));
            if ui
                .add_enabled(
                    has,
                    Button::new(if busy { tr("Pause") } else { tr("Play") }),
                )
                .clicked()
            {
                send(Action::TogglePlay); // the backend's state decides, not this label's
            }
            if ui.add_enabled(has, Button::new(tr("Next"))).clicked() {
                send(Action::Next);
            }
            let (repeat, then) = match q.repeat {
                Repeat::Off => (tr("Repeat off"), Repeat::All),
                Repeat::All => (tr("Repeat all"), Repeat::One),
                Repeat::One => (tr("Repeat one"), Repeat::Off),
            };
            if ui
                .selectable_label(q.repeat != Repeat::Off, repeat)
                .clicked()
            {
                send(Action::SetRepeat(then));
            }

            let duration = pb.track.as_ref().map_or(0.0, |t| t.duration as f32);
            let pos = player
                .map_or(0.0, |p| p.position().as_secs_f32())
                .min(duration);
            let mut at = state.seek.unwrap_or(pos);
            ui.label(tracks::duration(at as u32));
            ui.spacing_mut().slider_width = (ui.available_width() - 300.0 - BADGE).max(80.0);
            let seek = ui.add_enabled(
                has,
                Slider::new(&mut at, 0.0..=duration.max(1.0)).show_value(false),
            );
            if seek.dragged() {
                state.seek = Some(at);
            } else if seek.drag_stopped() || seek.changed() {
                state.seek = None;
                send(Action::Seek(Duration::from_secs_f32(at)));
            }
            ui.label(tracks::duration(duration as u32));

            ui.label(tr("Volume"));
            ui.spacing_mut().slider_width = 90.0;
            let exclusive = pb.output.as_ref().is_some_and(|o| o.exclusive);
            let mut volume = pb.volume;
            let slider = Slider::new(&mut volume, 0.0..=1.0).show_value(false);
            if ui
                .add_enabled(!exclusive, slider)
                .on_disabled_hover_text(tr("Volume is fixed in exclusive mode"))
                .changed()
            {
                send(Action::SetVolume(volume));
            }
            // A fixed slot, so the Queue toggle stays put as the badge comes and goes.
            ui.allocate_ui(vec2(BADGE, ui.spacing().interact_size.y), |ui| {
                ui.set_width(BADGE);
                if let Some(o) = &pb.output {
                    ui.add(Label::new(badge(o)).truncate())
                        .on_hover_text(badge_hover(o));
                }
            });
            ui.toggle_value(queue_open, tr("Queue"));
        });
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

fn badge_hover(o: &OutputInfo) -> String {
    if o.exclusive {
        return tr("Exclusive: bit-perfect, volume fixed").into_owned();
    }
    let mut hover = tr("Shared: mixed with other apps").into_owned();
    if o.device_rate != o.format.rate {
        hover += " · ";
        hover += &tr("resampled to {rate} kHz").replace("{rate}", &khz(o.device_rate));
    }
    hover
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
}
