//! Settings view: account, appearance, audio, behavior, integrations, and the keyboard
//! shortcuts; the login page leads here too, logged out. A change shows at once
//! (`Event::Settings`) and goes to the backend (`Action::SetSettings`), which saves it and sends
//! the settings back. The scrobbling services are the backend's to set: Connect and Disconnect
//! are actions. The Discord toggle and the behavior are settings like the others.

use super::Ctx;
use super::widgets::{heading, pill_button};
use crate::i18n::tr;
use crate::messages::Secret;
use crate::model::{Action, Event, Model, Route};
use crate::scrobble::Keys;
use crate::settings::{Account, Audio, Normalization, Settings, Theme, nonblank};
use crate::theme::{self, Palette};
use egui::{
    Align, Layout, Response, RichText, Sense, Ui, UiBuilder, WidgetInfo, WidgetText, WidgetType,
    vec2,
};
use fastframe_theme::Status;
use std::borrow::Cow;

pub struct State {
    /// The devices were asked for, and the fields filled in, on this visit (`ui::draw` clears
    /// it on any other route).
    pub listed: bool,
    /// The account fields as typed; Apply makes them the settings.
    client_id: String,
    redirect_uri: String,
    /// The ListenBrainz token as typed, until Connect sends it; never in the model.
    listenbrainz_token: String,
    /// Last.fm's keys are compiled in: its row shows.
    lastfm: bool,
    /// A Discord application ID is compiled in: its toggle shows.
    discord: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            listed: false,
            client_id: String::new(),
            redirect_uri: String::new(),
            listenbrainz_token: String::new(),
            lastfm: Keys::compiled().is_some(),
            discord: crate::discord::app_id().is_some(),
        }
    }
}

/// Spec §5.8: the page's title, then a `heading()` per section, one row per setting.
pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, state: &mut State) {
    let (model, p) = (ctx.model, ctx.palette);
    if !std::mem::replace(&mut state.listed, true) {
        ctx.send(Action::ListDevices);
        ctx.events.push(Event::RescanThemes); // a file just dropped in shows
        let saved = &model.settings.account;
        state.client_id = saved.client_id.clone().unwrap_or_default();
        state.redirect_uri = saved.redirect_uri.clone().unwrap_or_default();
    }
    let mut new = model.settings.clone(); // edited here
    let (mut logout, mut back, mut connect) = (false, false, None);
    super::page(ui, &Route::Settings, |ui| {
        // Logged out there is no sidebar: this is the way back to the login page.
        if !model.logged_in {
            back = ui.button(tr("Back")).clicked();
        }
        ui.label(
            RichText::new(tr("Settings"))
                .font(theme::title())
                .color(p.text),
        );
        section(ui, &p, tr("Account"));
        account(ui, &p, state, &mut new.account);
        if model.logged_in {
            logout = pill_button(ui, &p, None, &tr("Log out"), false).clicked();
        }
        section(ui, &p, tr("Appearance"));
        if appearance(ui, model, &mut new) {
            ctx.send(Action::OpenThemesFolder);
        }
        section(ui, &p, tr("Audio"));
        audio(ui, &model.devices, &mut new.audio);
        section(ui, &p, tr("Behavior"));
        ui.checkbox(&mut new.behavior.close_to_tray, tr("Close to tray"));
        // Without Close to tray there would be no window to come back to (`starts_hidden`).
        let start_minimized =
            egui::Checkbox::new(&mut new.behavior.start_minimized, tr("Start minimized"));
        ui.add_enabled(new.behavior.close_to_tray, start_minimized);
        ui.weak(tr("Only where the desktop shows a tray icon."));
        section(ui, &p, tr("Integrations"));
        if state.lastfm {
            connect = lastfm(ui, model);
        }
        if let Some(action) = listenbrainz(ui, model, state) {
            connect = Some(action);
        }
        if state.discord {
            let toggle = tr("Show what you play in your Discord status");
            ui.checkbox(&mut new.integrations.discord, toggle);
            ui.weak(tr("Discord must be running on this computer."));
        }
        section(ui, &p, tr("Keyboard shortcuts"));
        for (keys, what) in shortcuts() {
            row(ui, what, |ui| ui.weak(keys));
        }
    });
    if logout {
        ctx.send(Action::Logout);
    }
    if let Some(action) = connect {
        ctx.send(action);
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
        (format!("{cmd}+Q"), tr("Quit")),
    ]
}

/// A section's title in `heading()`, `MODULE_GAP` under what comes before.
fn section(ui: &mut Ui, p: &Palette, title: Cow<'static, str>) {
    ui.add_space(theme::MODULE_GAP);
    heading(ui, p, &title);
}

/// A setting's row (ruling 30), left to right in reading order: `label` in a `SETTINGS_LABEL`
/// column, cut with "…", then what `controls` adds, wrapping. With less room than that column
/// and a field, the label stacks above the controls.
fn row<R>(
    ui: &mut Ui,
    label: impl Into<WidgetText>,
    controls: impl FnOnce(&mut Ui) -> R,
) -> (Response, R) {
    if ui.available_width() < theme::SETTINGS_LABEL + theme::SETTINGS_FIELD {
        let label = ui.label(label);
        return (label, ui.horizontal_wrapped(controls).inner);
    }
    ui.horizontal(|ui| {
        let size = vec2(theme::SETTINGS_LABEL, ui.spacing().interact_size.y);
        let (column, _) = ui.allocate_exact_size(size, Sense::hover());
        let layout = Layout::left_to_right(Align::Center);
        let mut column = ui.new_child(UiBuilder::new().max_rect(column).layout(layout));
        let label = column.add(egui::Label::new(label).truncate());
        (label, ui.horizontal_wrapped(controls).inner)
    })
    .inner
}

/// A one-line field for a row, `SETTINGS_FIELD` wide or the room left.
fn field(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).desired_width(theme::SETTINGS_FIELD)
}

/// A combo box showing `selected`, its choices drawn by `list`, named `label` for screen
/// readers: egui would only link it to the label beside it.
fn combo(ui: &mut Ui, label: &str, selected: &str, list: impl FnOnce(&mut Ui)) {
    let combo = egui::ComboBox::from_id_salt(label).selected_text(selected);
    let shown = combo.show_ui(ui, list);
    shown.response.widget_info(|| {
        let mut info = WidgetInfo::labeled(WidgetType::ComboBox, ui.is_enabled(), label);
        info.current_text_value = Some(selected.to_owned());
        info
    });
}

/// The theme picker (spec §5.8): the built-ins, then the catalog's files; a built-in clears
/// the file and its cache, a file leaves the cache for `main` to fill. Beside it "Open themes
/// folder", under it the status line, then the album-art tint. Whether "Open themes folder"
/// was clicked.
fn appearance(ui: &mut Ui, model: &Model, new: &mut Settings) -> bool {
    let builtins = [
        (Theme::System, tr("Follow system")),
        (Theme::Dark, tr("Dark")),
        (Theme::Light, tr("Light")),
    ];
    let selected = match &new.custom_theme {
        Some(file) => fastframe_theme::display_name(file).to_owned(),
        None => {
            let builtin = builtins.iter().find(|(t, _)| *t == new.theme);
            builtin
                .map(|(_, label)| label.to_string())
                .unwrap_or_default()
        }
    };
    let title = tr("Theme");
    let (_, open) = row(ui, title.as_ref(), |ui| {
        combo(ui, &title, &selected, |ui| {
            // Closed by hand: egui closes it on a pointer's click only, not on Enter, Space
            // or a screen reader's.
            let item = |ui: &mut egui::Ui, on, label: &str| {
                let clicked = ui.selectable_label(on, label).clicked();
                if clicked {
                    ui.close();
                }
                clicked
            };
            for (theme, label) in &builtins {
                let on = new.custom_theme.is_none() && new.theme == *theme;
                if item(ui, on, label) {
                    new.theme = *theme;
                    (new.custom_theme, new.custom_theme_cache) = (None, None);
                }
            }
            if !model.themes.is_empty() {
                ui.separator();
            }
            for file in &model.themes {
                let on = new.custom_theme.as_ref() == Some(file);
                if item(ui, on, fastframe_theme::display_name(file)) {
                    new.custom_theme = Some(file.clone());
                }
            }
        });
        ui.button(tr("Open themes folder")).clicked()
    });
    if let Some(status) = &model.theme_status {
        ui.weak(status_text(
            status,
            new.custom_theme.as_deref().unwrap_or_default(),
        ));
    }
    ui.checkbox(&mut new.accent_from_art, tr("Colour from album art"));
    open
}

/// What the theme picker says under it (spec §3.5), `selected` being the chosen file.
pub fn status_text(status: &Status, selected: &str) -> String {
    use fastframe_theme::Problem;
    let text = match status {
        Status::Loading => tr("Loading themes…"),
        Status::SelectedUnavailable => {
            let name = fastframe_theme::display_name(selected);
            return tr("{name} can't be read; its last colours stay.").replace("{name}", name);
        }
        Status::Problem(Problem::Unreadable) => tr("The themes folder can't be read."),
        Status::Problem(Problem::TooManyEntries | Problem::TooManyThemes) => {
            tr("The themes folder holds too many files; some themes aren't listed.")
        }
        Status::Problem(_) => tr("Themes couldn't be listed."),
    };
    text.into_owned()
}

/// The client fields; Apply, a primary pill, makes what was typed the `account`.
fn account(ui: &mut Ui, p: &Palette, state: &mut State, account: &mut Account) {
    let fields = [
        (tr("Client ID"), &mut state.client_id),
        (tr("Redirect URL"), &mut state.redirect_uri),
    ];
    for (label, text) in fields {
        let (label, edit) = row(ui, label, |ui| ui.add(field(text)));
        edit.labelled_by(label.id);
    }
    let typed = Account {
        client_id: nonblank(&state.client_id),
        redirect_uri: nonblank(&state.redirect_uri),
    };
    let apply = ui.add_enabled_ui(typed != *account, |ui| {
        pill_button(ui, p, None, &tr("Apply"), true)
    });
    if apply.inner.clicked() {
        *account = typed;
    }
    ui.weak(tr(
        "Leave empty for the built-in client. Changing it logs you out.",
    ));
}

/// Who Last.fm is connected as, with Disconnect; while a connect waits for the user, Cancel;
/// else Connect. What was clicked, to send.
fn lastfm(ui: &mut Ui, model: &Model) -> Option<Action> {
    let user = &model.settings.integrations.lastfm_user;
    let (_, clicked) = row(ui, tr("Last.fm"), |ui| {
        if let Some(user) = user {
            ui.label(tr("Connected as {user}").replace("{user}", user));
            let disconnect = ui.button(tr("Disconnect")).clicked();
            disconnect.then_some(Action::DisconnectLastfm)
        } else if model.lastfm_waiting {
            ui.label(tr("Waiting for you to allow Tidash on Last.fm…"));
            let cancel = ui.button(tr("Cancel")).clicked();
            cancel.then_some(Action::CancelLastfm)
        } else {
            let connect = ui.button(tr("Connect")).clicked();
            connect.then_some(Action::ConnectLastfm)
        }
    });
    if user.is_none() && !model.lastfm_waiting {
        ui.weak(tr(
            "Scrobbles what you play. Connect opens Last.fm in the browser, where you allow Tidash.",
        ));
    }
    clicked
}

/// Who ListenBrainz is connected as, with Disconnect; or the token field, called "User token",
/// with Connect. What was clicked, to send.
fn listenbrainz(ui: &mut Ui, model: &Model, state: &mut State) -> Option<Action> {
    let name = tr("ListenBrainz");
    if let Some(user) = &model.settings.integrations.listenbrainz_user {
        let (_, disconnect) = row(ui, name, |ui| {
            ui.label(tr("Connected as {user}").replace("{user}", user));
            ui.button(tr("Disconnect")).clicked()
        });
        return disconnect.then_some(Action::DisconnectListenBrainz);
    }
    let (_, connect) = row(ui, name, |ui| {
        let hint = tr("User token");
        let token = field(&mut state.listenbrainz_token)
            .password(true)
            .hint_text(hint.as_ref());
        let token = ui.add(token);
        // The hint is AccessKit's placeholder, not its name; `widget_info` would also make it
        // a plain text input.
        ui.ctx()
            .accesskit_node_builder(token.id, |node| node.set_label(hint));
        let token = nonblank(&state.listenbrainz_token);
        let button = egui::Button::new(tr("Connect"));
        let clicked = ui.add_enabled(token.is_some(), button).clicked();
        clicked.then_some(token).flatten()
    });
    ui.weak(tr(
        "Scrobbles what you play. Copy your user token from listenbrainz.org/settings.",
    ));
    connect.map(|token| {
        state.listenbrainz_token.clear();
        Action::ConnectListenBrainz(Secret(token))
    })
}

/// The output device and the stream choices.
fn audio(ui: &mut Ui, devices: &[String], audio: &mut Audio) {
    let system = tr("System default");
    // A saved device that is not plugged in shows by name.
    let selected = audio.device.clone().unwrap_or_else(|| system.to_string());
    let label = tr("Output device");
    row(ui, label.as_ref(), |ui| {
        combo(ui, &label, &selected, |ui| {
            ui.selectable_value(&mut audio.device, None, system);
            for name in devices {
                ui.selectable_value(&mut audio.device, Some(name.clone()), name.as_str());
            }
        });
    });
    ui.checkbox(&mut audio.exclusive, tr("Exclusive mode"));
    ui.weak(tr(
        "Bit-perfect: each track plays at its own sample rate and bit depth, without other \
         apps' sound and without volume control. Falls back to shared mode when the device \
         refuses.",
    ));
    row(ui, tr("Quality"), |ui| {
        ui.radio_value(
            &mut audio.hi_res,
            true,
            tr("Hi-Res (up to 24-bit, 192 kHz)"),
        );
        ui.radio_value(&mut audio.hi_res, false, tr("Lossless (16-bit, 44.1 kHz)"));
    });
    row(ui, tr("Normalization"), |ui| {
        ui.radio_value(&mut audio.normalization, Normalization::Off, tr("Off"));
        ui.radio_value(&mut audio.normalization, Normalization::Track, tr("Track"));
        ui.radio_value(&mut audio.normalization, Normalization::Album, tr("Album"));
    });
}

#[cfg(test)]
mod tests {
    use super::super::{UiState, draw, testing};
    use crate::model::{Action, Event, Model, Route};
    use crate::settings::{Audio, Normalization, Settings, Theme};

    /// One pass of the whole UI, its events applied to `model`: the actions it sent, and its
    /// output.
    fn pass(
        ctx: &egui::Context,
        state: &mut UiState,
        model: &mut Model,
        events: Vec<egui::Event>,
    ) -> (Vec<Action>, egui::FullOutput) {
        pass_at(ctx, state, model, events, 1000.0)
    }

    /// `pass` in a window `width` wide, 600 tall.
    fn pass_at(
        ctx: &egui::Context,
        state: &mut UiState,
        model: &mut Model,
        events: Vec<egui::Event>,
        width: f32,
    ) -> (Vec<Action>, egui::FullOutput) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 600.0),
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
        let ctx = testing::context();
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
        let ctx = testing::context();
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
        let ctx = testing::context();
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

    /// Connect sends the token typed, trimmed, and empties the field; connected, the user
    /// shows with Disconnect. The token never reaches the model.
    #[test]
    fn listenbrainz_connect_and_disconnect() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        state.settings.lastfm = false; // its Connect aside, whatever the build's keys
        state.settings.listenbrainz_token = " tok \n".into(); // as pasted
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        let (sent, _) = pass(
            &ctx,
            &mut state,
            &mut model,
            vec![testing::click(&out, "Connect")],
        );
        let token = sent.iter().find_map(|a| match a {
            Action::ConnectListenBrainz(t) => Some(t.0.as_str()),
            _ => None,
        });
        assert_eq!(token, Some("tok"), "{sent:?}");
        assert!(state.settings.listenbrainz_token.is_empty());
        assert!(!format!("{:?}", model.settings).contains("tok"));
        model.settings.integrations.listenbrainz_user = Some("rob".into());
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        assert!(labels(&out).iter().any(|l| l == "Connected as rob"));
        let (sent, _) = pass(
            &ctx,
            &mut state,
            &mut model,
            vec![testing::click(&out, "Disconnect")],
        );
        let disconnect = |a: &Action| matches!(a, Action::DisconnectListenBrainz);
        assert_eq!(sent.iter().filter(|a| disconnect(a)).count(), 1, "{sent:?}");
    }

    /// Hidden without keys. With them: Connect; while it waits, Cancel; connected, the user
    /// with Disconnect.
    #[test]
    fn lastfm_connect_cancel_and_disconnect() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        state.settings.lastfm = false;
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        assert!(!labels(&out).iter().any(|l| l == "Last.fm"), "hidden");
        state.settings.lastfm = true;
        // ListenBrainz connected: the one Connect is Last.fm's.
        model.settings.integrations.listenbrainz_user = Some("rob".into());
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        let click = vec![testing::click(&out, "Connect")];
        let (sent, _) = pass(&ctx, &mut state, &mut model, click);
        assert!(matches!(sent[..], [Action::ConnectLastfm]), "{sent:?}");
        model.lastfm_waiting = true;
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        let waiting = "Waiting for you to allow Tidash on Last.fm…";
        assert!(labels(&out).iter().any(|l| l == waiting));
        let click = vec![testing::click(&out, "Cancel")];
        let (sent, _) = pass(&ctx, &mut state, &mut model, click);
        assert!(matches!(sent[..], [Action::CancelLastfm]), "{sent:?}");
        // Last.fm connected, ListenBrainz not: the one Disconnect is Last.fm's.
        model.lastfm_waiting = false;
        model.settings.integrations.lastfm_user = Some("ann".into());
        model.settings.integrations.listenbrainz_user = None;
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        assert!(labels(&out).iter().any(|l| l == "Connected as ann"));
        let click = vec![testing::click(&out, "Disconnect")];
        let (sent, _) = pass(&ctx, &mut state, &mut model, click);
        assert!(matches!(sent[..], [Action::DisconnectLastfm]), "{sent:?}");
    }

    /// Hidden without an application ID; with one, a setting like the others.
    #[test]
    fn discord_toggle_is_a_setting() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        let toggle = "Show what you play in your Discord status";
        state.settings.discord = false;
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        assert!(!labels(&out).iter().any(|l| l == toggle), "hidden");
        state.settings.discord = true;
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        let click = vec![testing::click(&out, toggle)];
        let (sent, _) = pass(&ctx, &mut state, &mut model, click);
        let mut on = Settings::default();
        on.integrations.discord = true;
        assert_eq!(set_settings(sent), [on]);
        assert!(model.settings.integrations.discord, "shown at once");
    }

    /// Start minimized needs Close to tray: off, its checkbox shows disabled and a click
    /// changes nothing.
    #[test]
    fn start_minimized_needs_close_to_tray() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        model.settings.behavior.close_to_tray = false; // on by default
        let start = "Start minimized";
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        assert!(testing::disabled(&out, start));
        let click = vec![testing::click(&out, start)];
        let (sent, _) = pass(&ctx, &mut state, &mut model, click);
        assert_eq!(set_settings(sent), []);
        model.settings.behavior.close_to_tray = true;
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        assert!(!testing::disabled(&out, start));
    }

    /// A file from the catalog becomes `custom_theme`; a built-in clears it and its cache.
    #[test]
    fn picking_a_theme_file_sets_custom_theme() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        model.themes = vec!["Nord.json".into()];
        // Opens the combo box (its list shows the pass after, once sized), then picks
        // `choice` from it: the settings sent.
        let mut choose = |model: &mut Model, choice: &str| {
            let (_, out) = pass(&ctx, &mut state, model, vec![]);
            pass(&ctx, &mut state, model, vec![testing::click(&out, "Theme")]);
            let (_, out) = pass(&ctx, &mut state, model, vec![]);
            let (sent, _) = pass(&ctx, &mut state, model, vec![testing::click(&out, choice)]);
            set_settings(sent)
        };
        let sent = choose(&mut model, "Nord");
        let custom: Vec<_> = sent.iter().map(|s| s.custom_theme.as_deref()).collect();
        assert_eq!(custom, [Some("Nord.json")]);
        let mut presets = fastframe_theme::presets::themes();
        model.settings.custom_theme_cache = presets.find(|t| t.filename == "Nord.json");
        let sent = choose(&mut model, "Light");
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].theme, Theme::Light);
        assert_eq!(sent[0].custom_theme, None);
        assert_eq!(sent[0].custom_theme_cache, None);
    }

    /// Each visit asks for a fresh list of the themes folder, once.
    #[test]
    fn settings_open_rescans_the_themes() {
        let ctx = testing::context();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Home);
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut rescans = |model: &Model| {
            let mut events = Vec::new();
            // Every pass's: egui may run more than one (a new grid sizes itself first).
            let mut out = ctx.run_ui(Default::default(), |ui| {
                events.extend(draw(ui, model, &tx, &mut state));
            });
            out.textures_delta.clear(); // nothing renders them
            let rescan = |e: &&Event| matches!(e, Event::RescanThemes);
            events.iter().filter(rescan).count()
        };
        assert_eq!(rescans(&model), 0);
        model.navigate(Route::Settings);
        assert_eq!((rescans(&model), rescans(&model)), (1, 0));
    }

    /// The AccessKit tree's nodes in `out`.
    fn nodes(out: &egui::FullOutput) -> &[(egui::accesskit::NodeId, egui::accesskit::Node)] {
        let tree = out.platform_output.accesskit_update.as_ref();
        &tree.expect("AccessKit is on").nodes
    }

    /// Ruling 30: Tab follows the reading order, a row's controls left to right.
    #[test]
    fn settings_tab_order_reads_left_to_right() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        let (hi_res, lossless) = (
            "Hi-Res (up to 24-bit, 192 kHz)",
            "Lossless (16-bit, 44.1 kHz)",
        );
        let ends = ["Open themes folder", lossless];
        pass(&ctx, &mut state, &mut model, vec![]);
        // The labels the focus visits, Tab after Tab.
        let mut order: Vec<String> = Vec::new();
        for _ in 0..200 {
            let tab = vec![testing::key(egui::Key::Tab)];
            let (_, out) = pass(&ctx, &mut state, &mut model, tab);
            let id = ctx.memory(|m| m.focused()).map(|id| id.accesskit_id());
            let node = nodes(&out).iter().find(|(n, _)| Some(*n) == id);
            order.extend(node.and_then(|(_, n)| n.label()).map(str::to_owned));
            if ends.iter().all(|e| order.iter().any(|l| l == e)) {
                break;
            }
        }
        let at = |label: &str| {
            let at = order.iter().position(|l| l == label);
            at.unwrap_or_else(|| panic!("{label:?} never focused: {order:?}"))
        };
        assert!(at("Theme") < at("Open themes folder"), "{order:?}");
        assert!(at(hi_res) < at(lossless), "{order:?}");
    }

    /// Review focus 4, ruling 30: at the 960 px minimum with the side panel open at its
    /// default, every control lies between the sidebar and the side panel, and a label does
    /// not run into its field.
    #[test]
    fn settings_fit_at_960_with_the_side_panel() {
        use crate::theme::{SIDE_PANEL, SIDEBAR};
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut state = UiState {
            side: Some(crate::ui::Side::Queue),
            ..Default::default()
        };
        let mut model = testing::at(Route::Settings);
        for _ in 0..2 {
            pass_at(&ctx, &mut state, &mut model, vec![], 960.0);
        }
        let (_, out) = pass_at(&ctx, &mut state, &mut model, vec![], 960.0);
        let centre = SIDEBAR..=960.0 - SIDE_PANEL;
        let controls = [
            "Apply",
            "Log out",
            "Theme",
            "Open themes folder",
            "Colour from album art",
            "Output device",
            "Exclusive mode",
            "Hi-Res (up to 24-bit, 192 kHz)",
            "Lossless (16-bit, 44.1 kHz)",
            "Off",
            "Track",
            "Album",
            "Close to tray",
            "Start minimized",
            "Connect",
        ];
        for label in controls {
            let r = testing::bounds(&out, label);
            let inside = centre.contains(&r.left()) && centre.contains(&r.right());
            assert!(inside, "{label}: {r:?} outside {centre:?}");
        }
        let rect = |n: &egui::accesskit::Node| {
            let b = n.bounds().expect("placed");
            let (min, max) = ((b.x0 as f32, b.y0 as f32), (b.x1 as f32, b.y1 as f32));
            egui::Rect::from_min_max(min.into(), max.into())
        };
        for label in ["Client ID", "Redirect URL"] {
            let text = Some(label);
            let (id, node) = nodes(&out)
                .iter()
                .find(|(_, n)| n.role() == egui::accesskit::Role::Label && n.value() == text)
                .expect(label);
            let field = nodes(&out)
                .iter()
                .find(|(_, n)| n.labelled_by().contains(id));
            let (_, field) = field.unwrap_or_else(|| panic!("no field for {label}"));
            let (label_rect, field_rect) = (rect(node), rect(field));
            assert!(
                !label_rect.intersects(field_rect),
                "{label}: {label_rect:?} {field_rect:?}"
            );
            let inside =
                centre.contains(&field_rect.left()) && centre.contains(&field_rect.right());
            assert!(inside, "{label}'s field: {field_rect:?} outside {centre:?}");
        }
    }

    /// The ListenBrainz field is called what it asks for, and stays a password field.
    #[test]
    fn the_token_field_is_called_user_token() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Settings);
        let (_, out) = pass(&ctx, &mut state, &mut model, vec![]);
        let field = nodes(&out)
            .iter()
            .find(|(_, n)| n.label() == Some("User token"));
        let (_, field) = field.expect("a field called User token");
        assert_eq!(field.role(), egui::accesskit::Role::PasswordInput);
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
            format!("{cmd}+Q"),
        ];
        for k in &want {
            assert!(keys.contains(&k.as_str()), "no {k} in {keys:?}");
        }
        assert_eq!(list.len(), want.len(), "{keys:?}");
        assert!(list.iter().all(|(_, what)| !what.is_empty()), "{list:?}");
    }
}
