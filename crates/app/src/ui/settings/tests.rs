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
        let inside = centre.contains(&field_rect.left()) && centre.contains(&field_rect.right());
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
