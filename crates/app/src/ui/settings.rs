//! Settings view: the audio output and stream choices. A change goes to the backend
//! (`Action::SetAudio`), which saves it and sends the settings back.

use super::Ctx;
use crate::i18n::tr;
use crate::model::{Action, Route};
use crate::settings::Normalization;

/// `listed`: the devices were asked for on this visit (`ui::draw` clears it on any other route).
pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, listed: &mut bool) {
    if !std::mem::replace(listed, true) {
        ctx.send(Action::ListDevices);
    }
    let model = ctx.model;
    // Edited here, sent to the backend; the model changes when it sends the settings back.
    let mut audio = model.settings.audio.clone();
    super::page(ui, &Route::Settings, |ui| {
        ui.heading(tr("Settings"));
        ui.add_space(12.0);
        ui.label(egui::RichText::new(tr("Audio")).strong().size(16.0));
        ui.add_space(8.0);
        let system = tr("System default");
        // A saved device that is not plugged in shows by name.
        let selected = audio.device.clone().unwrap_or_else(|| system.to_string());
        egui::ComboBox::from_label(tr("Output device"))
            .selected_text(selected)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut audio.device, None, system);
                for name in &model.devices {
                    ui.selectable_value(&mut audio.device, Some(name.clone()), name.as_str());
                }
            });
        ui.add_space(8.0);
        ui.checkbox(&mut audio.exclusive, tr("Exclusive mode"));
        ui.weak(tr(
            "Bit-perfect: each track plays at its own sample rate and bit depth, without other \
             apps' sound and without volume control. Falls back to shared mode when the device \
             refuses.",
        ));
        ui.add_space(8.0);
        ui.label(tr("Quality"));
        ui.radio_value(
            &mut audio.hi_res,
            true,
            tr("Hi-Res (up to 24-bit, 192 kHz)"),
        );
        ui.radio_value(&mut audio.hi_res, false, tr("Lossless (16-bit, 44.1 kHz)"));
        ui.add_space(8.0);
        ui.label(tr("Normalization"));
        ui.horizontal(|ui| {
            ui.radio_value(&mut audio.normalization, Normalization::Off, tr("Off"));
            ui.radio_value(&mut audio.normalization, Normalization::Track, tr("Track"));
            ui.radio_value(&mut audio.normalization, Normalization::Album, tr("Album"));
        });
    });
    if audio != model.settings.audio {
        ctx.send(Action::SetAudio(audio));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{UiState, draw, testing};
    use crate::model::{Action, Model, Route};
    use crate::settings::Audio;

    /// One pass of the whole UI: the actions it sent, and its output.
    fn pass(
        ctx: &egui::Context,
        state: &mut UiState,
        model: &Model,
        events: Vec<egui::Event>,
    ) -> (Vec<Action>, egui::FullOutput) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 600.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            draw(ui, model, &tx, state);
        });
        out.textures_delta.clear(); // nothing renders them
        (std::iter::from_fn(|| rx.try_recv().ok()).collect(), out)
    }

    fn listed(sent: &[Action]) -> usize {
        let devices = |a: &&Action| matches!(a, Action::ListDevices);
        sent.iter().filter(devices).count()
    }

    #[test]
    fn asks_for_devices_once_per_visit() {
        let ctx = egui::Context::default();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        let (first, _) = pass(&ctx, &mut state, &model, vec![]);
        let (second, _) = pass(&ctx, &mut state, &model, vec![]);
        assert_eq!((listed(&first), listed(&second)), (1, 0));
        model.navigate(Route::Home);
        let (away, _) = pass(&ctx, &mut state, &model, vec![]);
        assert_eq!(listed(&away), 0);
        model.navigate(Route::Settings);
        let (back, _) = pass(&ctx, &mut state, &model, vec![]);
        assert_eq!(listed(&back), 1, "a new visit");
    }

    #[test]
    fn exclusive_checkbox_sends_set_audio() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let model = testing::at(Route::Settings);
        let (_, out) = pass(&ctx, &mut state, &model, vec![]);
        let click = testing::click(&out, "Exclusive mode");
        let (sent, _) = pass(&ctx, &mut state, &model, vec![click]);
        let set: Vec<Audio> = sent
            .into_iter()
            .filter_map(|a| match a {
                Action::SetAudio(audio) => Some(audio),
                _ => None,
            })
            .collect();
        let want = Audio {
            exclusive: true,
            ..Audio::default()
        };
        assert_eq!(set, [want]);
    }
}
