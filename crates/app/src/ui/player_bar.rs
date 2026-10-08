//! Player bar: cover, title and artists, transport, seek and volume.

use super::{cover, tracks};
use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Model};
use egui::{Button, Label, RichText, Slider, vec2};
use player::{PlayState, Player};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

const COVER: f32 = 56.0;

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

            let has = pb.track.is_some();
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
                send(if busy { Action::Pause } else { Action::Play });
            }
            if ui.add_enabled(has, Button::new(tr("Next"))).clicked() {
                send(Action::Next);
            }

            let duration = pb.track.as_ref().map_or(0.0, |t| t.duration as f32);
            let pos = player
                .map_or(0.0, |p| p.position().as_secs_f32())
                .min(duration);
            let mut at = state.seek.unwrap_or(pos);
            ui.label(tracks::duration(at as u32));
            ui.spacing_mut().slider_width = (ui.available_width() - 230.0).max(80.0);
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
            let mut volume = pb.volume;
            if ui
                .add(Slider::new(&mut volume, 0.0..=1.0).show_value(false))
                .changed()
            {
                send(Action::SetVolume(volume));
            }
        });
    });
}
