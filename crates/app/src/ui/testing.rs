//! Fixtures and a headless pass of one view, for the views' tests.

use super::Ctx;
use crate::images::TextureCache;
use crate::model::{Action, Event, Model, PageData, Route};
use egui::{Pos2, Rect, vec2};
use tokio::sync::mpsc::UnboundedSender;

pub type View = fn(&mut egui::Ui, &mut Ctx<'_>, &Route);

/// A context with the app's fonts: a palette's text styles (`theme::apply`) name weights a
/// bare context lacks.
pub fn context() -> egui::Context {
    let ctx = egui::Context::default();
    let fonts = fastframe_fonts::FontSetup::default().system_fallbacks(false);
    fonts.install(&ctx);
    ctx
}

pub fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Default::default(),
    }
}

/// A screen reader's click on the node labelled `label` in `out`.
pub fn click(out: &egui::FullOutput, label: &str) -> egui::Event {
    clicks(out, label)
        .next()
        .unwrap_or_else(|| panic!("no {label:?} shown"))
}

/// A screen reader's click on each node labelled `label` in `out`.
pub fn clicks(out: &egui::FullOutput, label: &str) -> impl Iterator<Item = egui::Event> {
    use egui::accesskit;
    let tree = out
        .platform_output
        .accesskit_update
        .as_ref()
        .expect("AccessKit is on");
    let nodes = tree
        .nodes
        .iter()
        .filter(move |(_, n)| n.label() == Some(label));
    nodes.map(|(id, _)| {
        egui::Event::AccessKitActionRequest(accesskit::ActionRequest {
            action: accesskit::Action::Click,
            target_tree: accesskit::TreeId::ROOT,
            target_node: *id,
            data: None,
        })
    })
}

/// Whether the node labelled `label` in `out` is shown disabled.
pub fn disabled(out: &egui::FullOutput, label: &str) -> bool {
    let tree = out.platform_output.accesskit_update.as_ref();
    let nodes = &tree.expect("AccessKit is on").nodes;
    let node = nodes.iter().find(|(_, n)| n.label() == Some(label));
    node.unwrap_or_else(|| panic!("no {label:?} shown"))
        .1
        .is_disabled()
}

/// The text of every node in `out`, as a screen reader finds it.
pub fn shown(out: &egui::FullOutput) -> Vec<String> {
    let tree = out.platform_output.accesskit_update.as_ref();
    let text = |n: &egui::accesskit::Node| n.label().or(n.value()).map(str::to_owned);
    let nodes = &tree.expect("AccessKit is on").nodes;
    nodes.iter().filter_map(|(_, n)| text(n)).collect()
}

/// Logged in as user 1, at `route`.
pub fn at(route: Route) -> Model {
    let mut m = Model::default();
    m.apply(Event::LoggedIn(1));
    m.navigate(route);
    m
}

/// Where the node labelled `label` in `out` is drawn.
pub fn bounds(out: &egui::FullOutput, label: &str) -> Rect {
    let tree = out.platform_output.accesskit_update.as_ref();
    let nodes = &tree.expect("AccessKit is on").nodes;
    let node = nodes.iter().find(|(_, n)| n.label() == Some(label));
    let bounds = node.and_then(|(_, n)| n.bounds());
    let b = bounds.unwrap_or_else(|| panic!("no {label:?} placed"));
    Rect::from_min_max(
        Pos2::new(b.x0 as f32, b.y0 as f32),
        Pos2::new(b.x1 as f32, b.y1 as f32),
    )
}

/// The middle of the node labelled `label` in `out`, where a pointer would click it.
pub fn middle(out: &egui::FullOutput, label: &str) -> Pos2 {
    bounds(out, label).center()
}

/// A secondary (right) button press at `at` if `pressed`, else its release.
pub fn right(at: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Secondary,
        pressed,
        modifiers: Default::default(),
    }
}

/// A primary (left) button press at `at` if `pressed`, else its release.
pub fn primary(at: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    }
}

/// One pass of `view` drawing `model`'s route, fed `input`: the events it gave, and its
/// output.
pub fn pass(
    egui: &egui::Context,
    tx: &UnboundedSender<Action>,
    model: &Model,
    view: View,
    input: Vec<egui::Event>,
) -> (Vec<Event>, egui::FullOutput) {
    pass_with(egui, tx, model, input, |ui, c| view(ui, c, model.route()))
}

/// One pass of `draw`, fed `input`: the events it gave, and its output.
pub fn pass_with(
    egui: &egui::Context,
    tx: &UnboundedSender<Action>,
    model: &Model,
    input: Vec<egui::Event>,
    draw: impl FnMut(&mut egui::Ui, &mut Ctx<'_>),
) -> (Vec<Event>, egui::FullOutput) {
    let input = egui::RawInput {
        events: input,
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
        ..Default::default()
    };
    pass_raw(egui, tx, model, input, draw)
}

/// As `pass_with`, fed `input` whole: another window size or scale.
pub fn pass_raw(
    egui: &egui::Context,
    tx: &UnboundedSender<Action>,
    model: &Model,
    input: egui::RawInput,
    mut draw: impl FnMut(&mut egui::Ui, &mut Ctx<'_>),
) -> (Vec<Event>, egui::FullOutput) {
    let mut images = TextureCache::new(1 << 20);
    let mut events = Vec::new();
    let mut out = egui.run_ui(input, |ui| {
        let mut c = Ctx {
            model,
            tx,
            images: &mut images,
            events: &mut events,
            palette: crate::theme::Palette::dark(),
        };
        draw(ui, &mut c);
    });
    out.textures_delta.clear(); // nothing renders them
    (events, out)
}

/// `view` draws `route` loading, failed (with Retry), and loaded with `data`, then showing
/// each of `texts`.
pub fn every_state(view: View, route: Route, data: PageData, texts: &[&str]) {
    let egui = context();
    egui.enable_accesskit();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let shown = |model: &Model| shown(&pass(&egui, &tx, model, view, vec![]).1);
    let mut model = at(route.clone());
    model.apply(Event::Loading(route.clone()));
    assert!(
        !shown(&model).contains(&"Retry".into()),
        "{route:?} loading"
    );
    model.apply(Event::PageFailed(route.clone()));
    assert!(shown(&model).contains(&"Retry".into()), "{route:?} failed");
    let r = route.clone();
    model.apply(Event::Page { route: r, data });
    let shown = shown(&model);
    for t in texts {
        assert!(
            shown.iter().any(|s| s == t),
            "{route:?}: no {t:?} in {shown:?}"
        );
    }
}

pub fn artist(id: u64) -> tidal::Artist {
    tidal::Artist {
        id,
        name: format!("Artist {id}"),
        picture: Some("12-34".into()),
        selected_album_cover_fallback: None,
        mixes: Default::default(),
    }
}

pub fn album(id: u64) -> tidal::Album {
    tidal::Album {
        id,
        title: format!("Album {id}"),
        cover: Some("ab-cd".into()),
        artists: vec![artist(1), artist(2)],
        number_of_tracks: 2,
        duration: 400,
        release_date: Some("2020-01-02".into()),
    }
}

/// 200 s long, on disc 1.
pub fn track(id: u64) -> tidal::Track {
    tidal::Track {
        id,
        title: format!("Track {id}"),
        duration: 200,
        track_number: id as u32,
        volume_number: 1,
        artists: vec![artist(1)],
        album: tidal::AlbumRef {
            id: 1,
            title: "Album 1".into(),
            cover: None,
        },
        stream_ready: true,
        ..Default::default()
    }
}

pub fn tracks(n: u64) -> Vec<tidal::Track> {
    (1..=n).map(track).collect()
}

/// TIDAL's own (no creator name, creator id 0).
pub fn playlist() -> tidal::Playlist {
    tidal::Playlist {
        uuid: "p-1".into(),
        title: "Playlist".into(),
        description: None,
        creator: None,
        creator_id: Some(0),
        number_of_tracks: 3,
        duration: 600,
        square_image: None,
        image: Some("wi-de".into()),
    }
}

pub fn mix() -> tidal::Mix {
    tidal::Mix {
        id: "m1".into(),
        title: "Mix".into(),
        subtitle: "Daily".into(),
        images: None,
    }
}
