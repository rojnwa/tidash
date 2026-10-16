//! Settings view: account, appearance, audio, behavior, integrations, the keyboard shortcuts,
//! and updates; the login page leads here too, logged out. A change shows at once
//! (`Event::Settings`) and goes to the backend (`Action::SetSettings`), which saves it and sends
//! the settings back. The scrobbling services are the backend's to set: Connect and Disconnect
//! are actions. The Discord toggle and the behavior are settings like the others.

use super::Ctx;
use super::widgets::{heading, pill_button};
use crate::i18n::tr;
use crate::messages::{Secret, UpdateStep};
use crate::model::{Action, Event, Model, Route, UpdateState};
use crate::scrobble::Keys;
use crate::settings::{Account, Audio, Normalization, Settings, Theme, Updates, nonblank};
use crate::theme::{self, Palette};
use egui::{
    Align, Layout, Response, RichText, Sense, Ui, UiBuilder, WidgetInfo, WidgetText, WidgetType,
    vec2,
};
use fastframe_theme::Status;
use fastframe_update::{PackageManager, Unsupported};
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
        section(ui, &p, tr("Updates"));
        if let Some(action) = updates(ui, &p, model.update.as_ref(), &mut new.updates) {
            ctx.send(action);
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

/// The version running, Automatic updates, then where an update stands: Restart to update
/// once one is ready, else Check now, disabled while a check or a download runs. What was
/// clicked, to send.
fn updates(
    ui: &mut Ui,
    p: &Palette,
    update: Option<&UpdateState>,
    updates: &mut Updates,
) -> Option<Action> {
    let version = tr("Tidash {version}").replace("{version}", env!("CARGO_PKG_VERSION"));
    row(ui, tr("Version"), |ui| ui.label(version));
    row(ui, tr("Automatic updates"), |ui| {
        let automatic = tr("Check daily and download new versions");
        ui.checkbox(&mut updates.automatic, automatic)
    });
    let (_, clicked) = row(ui, "", |ui| {
        if let Some(update) = update {
            update_status(ui, update);
        }
        if let Some(UpdateState::Ready { .. }) = update {
            let restart = pill_button(ui, p, None, &tr("Restart to update"), true);
            return restart.clicked().then_some(Action::InstallUpdate);
        }
        let busy = matches!(
            update,
            Some(UpdateState::Checking | UpdateState::Downloading { .. })
        );
        // Not `add_enabled_ui`: its child area cannot wrap, so the pill would stay on the
        // sentence's last line even past the edge. This `ui` is the row's, and the pill its last.
        if busy {
            ui.disable();
        }
        let check = pill_button(ui, p, None, &tr("Check now"), false);
        check.clicked().then_some(Action::CheckForUpdates)
    });
    clicked
}

/// Where `update` stands, in a sentence; for a release this copy cannot install, a second
/// says how to get it.
fn update_status(ui: &mut Ui, update: &UpdateState) {
    let text = match update {
        UpdateState::Checking => tr("Checking for updates…"),
        UpdateState::UpToDate => tr("Tidash is up to date."),
        UpdateState::Downloading { version, percent } => {
            tr("Downloading Tidash {version} ({percent}%)…")
                .replace("{version}", version)
                .replace("{percent}", &percent.to_string())
                .into()
        }
        UpdateState::Ready { version } => tr("Tidash {version} is ready.")
            .replace("{version}", version)
            .into(),
        UpdateState::Unsupported {
            version,
            reason: why,
        } => {
            ui.label(tr("Tidash {version} is available.").replace("{version}", version));
            reason(why)
        }
        UpdateState::Failed(UpdateStep::Check) => tr("Tidash could not check for updates."),
        UpdateState::Failed(UpdateStep::Download) => tr("Tidash could not download the update."),
        UpdateState::Failed(UpdateStep::Install) => tr("Tidash could not start the update."),
    };
    ui.label(text);
}

/// How a copy that cannot update itself gets a new release.
fn reason(reason: &Unsupported) -> Cow<'static, str> {
    let manager = match reason {
        Unsupported::Flatpak => "Flatpak",
        Unsupported::Snap => "Snap",
        Unsupported::Nix => "Nix",
        Unsupported::Homebrew => "Homebrew",
        Unsupported::SystemPackage(PackageManager::Apt) => "apt",
        Unsupported::SystemPackage(PackageManager::Dnf) => "dnf",
        Unsupported::SystemPackage(PackageManager::Pacman) => "pacman",
        Unsupported::Cargo => {
            return tr("This copy was installed with cargo; run cargo install again to update it.");
        }
        Unsupported::SystemDirectory => {
            return tr("This copy is in a system folder; the system's package manager updates it.");
        }
        Unsupported::MoveToApplications => return tr("Move Tidash to Applications to update it."),
        _ => return tr("This copy cannot update itself; download the new version from GitHub."),
    };
    tr("{manager} updates this copy.")
        .replace("{manager}", manager)
        .into()
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
#[path = "settings/tests.rs"]
mod tests;
