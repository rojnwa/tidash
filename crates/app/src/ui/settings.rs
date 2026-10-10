//! Settings view: account, appearance, audio, and the keyboard shortcuts; the login page leads
//! here too, logged out. A change shows at once (`Event::Settings`) and goes to the backend
//! (`Action::SetSettings`), which saves it and sends the settings back.

use super::Ctx;
use crate::i18n::tr;
use crate::model::{Action, Event, Route};
use crate::settings::{Account, Audio, Normalization, Theme, nonblank};
use std::borrow::Cow;

#[derive(Default)]
pub struct State {
    /// The devices were asked for, and the fields filled in, on this visit (`ui::draw` clears
    /// it on any other route).
    pub listed: bool,
    /// The account fields as typed; Apply makes them the settings.
    client_id: String,
    redirect_uri: String,
}

pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, state: &mut State) {
    let model = ctx.model;
    if !std::mem::replace(&mut state.listed, true) {
        ctx.send(Action::ListDevices);
        let saved = &model.settings.account;
        state.client_id = saved.client_id.clone().unwrap_or_default();
        state.redirect_uri = saved.redirect_uri.clone().unwrap_or_default();
    }
    let mut new = model.settings.clone(); // edited here
    let (mut logout, mut back) = (false, false);
    super::page(ui, &Route::Settings, |ui| {
        // Logged out there is no sidebar: this is the way back to the login page.
        if !model.logged_in {
            back = ui.button(tr("Back")).clicked();
        }
        ui.heading(tr("Settings"));
        section(ui, tr("Account"));
        if model.logged_in {
            logout = ui.button(tr("Log out")).clicked();
            ui.add_space(8.0);
        }
        account(ui, state, &mut new.account);
        section(ui, tr("Appearance"));
        ui.horizontal(|ui| {
            ui.label(tr("Theme"));
            ui.radio_value(&mut new.theme, Theme::Dark, tr("Dark"));
            ui.radio_value(&mut new.theme, Theme::Light, tr("Light"));
        });
        section(ui, tr("Audio"));
        audio(ui, &model.devices, &mut new.audio);
        section(ui, tr("Keyboard shortcuts"));
        egui::Grid::new("shortcuts").num_columns(2).show(ui, |ui| {
            for (keys, what) in shortcuts() {
                ui.label(keys);
                ui.label(what);
                ui.end_row();
            }
        });
    });
    if logout {
        ctx.send(Action::Logout);
    }
    if back {
        ctx.events.push(Event::Back);
    }
    if new != model.settings {
        // Shown at once, so a second change before the backend answers builds on this one.
        ctx.events.push(Event::Settings(new.clone()));
        ctx.send(Action::SetSettings(new));
    }
}

/// The keyboard shortcuts `ui::shortcuts` handles: the keys, and what they do.
pub fn shortcuts() -> Vec<(String, Cow<'static, str>)> {
    let cmd = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    vec![
        ("Space".into(), tr("Play or pause")),
        (format!("{cmd}+←"), tr("Previous track")),
        (format!("{cmd}+→"), tr("Next track")),
        ("Alt+←".into(), tr("Back")),
        ("Alt+→".into(), tr("Forward")),
        (format!("{cmd}+F"), tr("Search")),
        (format!("{cmd}+,"), tr("Settings")),
    ]
}

/// A section's title, spaced from what comes before.
fn section(ui: &mut egui::Ui, title: Cow<'static, str>) {
    ui.add_space(16.0);
    ui.label(egui::RichText::new(title).strong().size(16.0));
    ui.add_space(8.0);
}

/// The client fields; Apply makes what was typed the `account`.
fn account(ui: &mut egui::Ui, state: &mut State, account: &mut Account) {
    egui::Grid::new("account").num_columns(2).show(ui, |ui| {
        let fields = [
            (tr("Client ID"), &mut state.client_id),
            (tr("Redirect URL"), &mut state.redirect_uri),
        ];
        for (label, text) in fields {
            let label = ui.label(label);
            ui.text_edit_singleline(text).labelled_by(label.id);
            ui.end_row();
        }
    });
    let typed = Account {
        client_id: nonblank(&state.client_id),
        redirect_uri: nonblank(&state.redirect_uri),
    };
    let apply = egui::Button::new(tr("Apply"));
    if ui.add_enabled(typed != *account, apply).clicked() {
        *account = typed;
    }
    ui.weak(tr(
        "Leave empty for the built-in client. Changing it logs you out.",
    ));
}

/// The output device and the stream choices.
fn audio(ui: &mut egui::Ui, devices: &[String], audio: &mut Audio) {
    let system = tr("System default");
    // A saved device that is not plugged in shows by name.
    let selected = audio.device.clone().unwrap_or_else(|| system.to_string());
    egui::ComboBox::from_label(tr("Output device"))
        .selected_text(selected)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut audio.device, None, system);
            for name in devices {
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
}

#[cfg(test)]
mod tests {
    use super::super::{UiState, draw, testing};
    use crate::model::{Action, Model, Route};
    use crate::settings::{Audio, Normalization, Settings};

    /// One pass of the whole UI, its events applied to `model`: the actions it sent, and its
    /// output.
    fn pass(
        ctx: &egui::Context,
        state: &mut UiState,
        model: &mut Model,
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
        let mut events = Vec::new();
        let mut out = ctx.run_ui(input, |ui| events = draw(ui, model, &tx, state));
        out.textures_delta.clear(); // nothing renders them
        for ev in events {
            model.apply(ev);
        }
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
        let (first, _) = pass(&ctx, &mut state, &mut model, vec![]);
        let (second, _) = pass(&ctx, &mut state, &mut model, vec![]);
        assert_eq!((listed(&first), listed(&second)), (1, 0));
        model.navigate(Route::Home);
        let (away, _) = pass(&ctx, &mut state, &mut model, vec![]);
        assert_eq!(listed(&away), 0);
        model.navigate(Route::Settings);
        let (back, _) = pass(&ctx, &mut state, &mut model, vec![]);
        assert_eq!(listed(&back), 1, "a new visit");
    }

    /// The labels and texts `out` shows a screen reader.
    fn labels(out: &egui::FullOutput) -> Vec<String> {
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = tree.expect("AccessKit is on").nodes.iter();
        let text = |n: &egui::accesskit::Node| n.label().or(n.value()).map(str::to_owned);
        nodes.filter_map(|(_, n)| text(n)).collect()
    }

    /// Spec §9: a refused client ID points to a setting the login page leads to. Logged out,
    /// Settings has no session pages around it nor Log out, and Back returns to the login.
    #[test]
    fn settings_is_reachable_from_the_login_page() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = Model::default();
        assert_eq!(model.route(), &Route::Login);
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        let click = testing::click(&out, "Settings");
        pass(&ctx, &mut state, &mut model, vec![click]);
        assert_eq!(model.route(), &Route::Settings);
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        let shown = labels(&out);
        for (label, want) in [("Home", false), ("Log out", false), ("Back", true)] {
            assert_eq!(shown.iter().any(|l| l == label), want, "{label}: {shown:?}");
        }
        assert!(shown.iter().any(|l| l == "Client ID"), "{shown:?}");
        pass(
            &ctx,
            &mut state,
            &mut model,
            vec![testing::click(&out, "Back")],
        );
        assert_eq!(model.route(), &Route::Login);
    }

    fn set_settings(sent: Vec<Action>) -> Vec<Settings> {
        let set = |a| match a {
            Action::SetSettings(s) => Some(s),
            _ => None,
        };
        sent.into_iter().filter_map(set).collect()
    }

    /// Clicked in consecutive passes, before the backend sends anything back: the second
    /// change builds on the first, which shows at once.
    #[test]
    fn two_quick_edits_both_land() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        let click = testing::click(&out, "Exclusive mode");
        let (sent, out) = pass(&ctx, &mut state, &mut model, vec![click]);
        let exclusive = Settings {
            audio: Audio {
                exclusive: true,
                ..Audio::default()
            },
            ..Settings::default()
        };
        assert_eq!(set_settings(sent), std::slice::from_ref(&exclusive));
        assert_eq!(model.settings, exclusive, "shown at once");
        let click = testing::click(&out, "Album");
        let (sent, _) = pass(&ctx, &mut state, &mut model, vec![click]);
        let mut both = exclusive;
        both.audio.normalization = Normalization::Album;
        assert_eq!(set_settings(sent), [both]);
    }

    #[test]
    fn shortcut_list_names_every_shortcut() {
        let cmd = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        let list = super::shortcuts();
        let keys: Vec<&str> = list.iter().map(|(k, _)| k.as_str()).collect();
        let want = [
            "Space".to_owned(),
            format!("{cmd}+←"),
            format!("{cmd}+→"),
            "Alt+←".into(),
            "Alt+→".into(),
            format!("{cmd}+F"),
            format!("{cmd}+,"),
        ];
        for k in &want {
            assert!(keys.contains(&k.as_str()), "no {k} in {keys:?}");
        }
        assert_eq!(list.len(), want.len(), "{keys:?}");
        assert!(list.iter().all(|(_, what)| !what.is_empty()), "{list:?}");
    }
}
