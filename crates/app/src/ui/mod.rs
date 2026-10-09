//! Drawing: dismissible errors on top, then the current route.

mod album;
mod artist;
mod browse;
mod card;
mod collection;
mod header;
mod login;
mod mix;
mod player_bar;
mod playlist;
mod queue_panel;
mod rows;
mod search;
mod shelf;
mod sidebar;
mod tracks;

use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Event, Model, PageData, Route};
use std::borrow::Cow;
use tokio::sync::mpsc::UnboundedSender;

pub struct UiState {
    login: login::State,
    search: search::State,
    bar: player_bar::State,
    pub images: TextureCache,
    /// Read only for the position; `None` until the output opened, or without a device.
    pub player: Option<player::Player>,
    scrolling: fastframe_scroll::Scrolling,
    /// The queue panel shows; the player bar toggles it.
    pub queue_open: bool,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            login: Default::default(),
            search: Default::default(),
            bar: Default::default(),
            images: TextureCache::new(64 << 20),
            player: None,
            scrolling: Default::default(),
            queue_open: false,
        }
    }
}

impl UiState {
    /// What the UI keeps of a session ends with it (`LoggedOut`, `SessionExpired`): the next
    /// account finds the search box empty, with no undo history to bring the query back.
    pub fn apply(&mut self, ctx: &egui::Context, ev: &Event) {
        if matches!(ev, Event::LoggedOut | Event::SessionExpired) {
            self.search = Default::default();
            let id = search::query_id();
            ctx.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(id));
        }
    }
}

/// What every view draws with.
pub struct Ctx<'a> {
    pub model: &'a Model,
    pub tx: &'a UnboundedSender<Action>,
    pub images: &'a mut TextureCache,
    pub events: &'a mut Vec<Event>,
}

impl Ctx<'_> {
    pub fn send(&self, action: Action) {
        let _ = self.tx.send(action);
    }

    pub fn go(&mut self, route: Route) {
        self.events.push(Event::Navigate(route));
    }

    /// What toggling `item` does: "Add to favorites" or "Remove from favorites".
    pub fn favorite_label(&self, item: &tidal::Favorite) -> Cow<'static, str> {
        if self.model.favorites.contains(item) {
            tr("Remove from favorites")
        } else {
            tr("Add to favorites")
        }
    }

    /// A button showing `text` that flips `item`'s heart now and has the backend write it (a
    /// failed write flips it back); disabled while that item's write is under way (ruling R19).
    pub fn favorite_button(
        &mut self,
        ui: &mut egui::Ui,
        item: tidal::Favorite,
        text: &str,
    ) -> egui::Response {
        let pending = self.model.pending_favorites.contains(&item);
        let resp = ui.add_enabled(!pending, egui::Button::new(text));
        if resp.clicked() {
            let on = !self.model.favorites.contains(&item);
            self.events.push(Event::FavoriteSet {
                item: item.clone(),
                on,
            });
            self.send(Action::SetFavorite { item, on });
        }
        resp
    }
}

/// A route's one vertical scroll; each route keeps its own offset, and the ids inside (a
/// shelf's sideways offset, say) are the route's own: egui does not salt them by the scroll's.
pub fn page<R>(ui: &mut egui::Ui, route: &Route, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let speed = ui.ctx().data(|d| d.get_temp::<f32>(autoscroll_id()));
    let step = speed.map(|v| v * ui.input(|i| i.stable_dt));
    let out = egui::ScrollArea::vertical()
        .id_salt(("page", route))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(step) = step {
                let animation = egui::style::ScrollAnimation::none();
                ui.scroll_with_delta_animation(egui::vec2(0.0, -step), animation);
                ui.ctx().request_repaint();
            }
            ui.push_id(route, f).inner
        });
    if let (Some(v), Some(step)) = (speed, step) {
        // This step lands next pass; turn round now if it reaches an end (give or take float
        // error), so no pass stands still there.
        let to = out.state.offset.y + step;
        let max = out.content_size.y - out.inner_rect.height();
        if (v > 0.0 && to >= max - 0.5) || (v < 0.0 && to <= 0.5) {
            ui.ctx().data_mut(|d| d.insert_temp(autoscroll_id(), -v));
        }
    }
    out.inner
}

fn autoscroll_id() -> egui::Id {
    egui::Id::new("autoscroll")
}

/// `TIDASH_AUTOSCROLL`: the current page scrolls `speed` points a second down (up if
/// negative) every pass, turning round at either end.
pub fn autoscroll(ctx: &egui::Context, speed: f32) {
    ctx.data_mut(|d| d.insert_temp(autoscroll_id(), speed));
}

/// `route`'s page; without one, a spinner while it loads, or a message and Retry after it
/// failed.
fn page_data<'m>(ui: &mut egui::Ui, ctx: &mut Ctx<'m>, route: &Route) -> Option<&'m PageData> {
    let model = ctx.model;
    let slot = model.pages.get(route)?;
    if slot.data.is_none() {
        if slot.failed {
            ui.label(tr("Couldn't load this page"));
            if ui.button(tr("Retry")).clicked() {
                ctx.go(route.clone()); // asks for it again
            }
        } else if slot.loading {
            ui.spinner();
        }
    }
    slot.data.as_ref()
}

/// Paints `image` over `rect` with corners of `radius`, requested as many pixels wide as it
/// is drawn; or a flat placeholder. Nothing out of view is requested. Artwork of another
/// shape (a wide playlist image) is cropped around its centre, not stretched.
fn cover(
    ui: &egui::Ui,
    images: &mut TextureCache,
    tx: &UnboundedSender<Action>,
    image: Option<&tidal::Image>,
    rect: egui::Rect,
    radius: f32,
) {
    if !ui.is_rect_visible(rect) {
        return;
    }
    // Wide renditions are 3:2: cropped, they need as many pixels as they are tall.
    let wide = matches!(
        image,
        Some(tidal::Image::Tidal {
            kind: tidal::ImageKind::PlaylistWide,
            ..
        })
    );
    let width = if wide {
        rect.width().max(rect.height() * 1.5)
    } else {
        rect.width()
    };
    let px = (width * ui.ctx().pixels_per_point()).ceil().max(1.0) as u32;
    let tex = image.and_then(|img| {
        images.get(ui.ctx(), img.key(), px, || {
            let _ = tx.send(Action::LoadImage {
                image: img.clone(),
                px,
            });
        })
    });
    let painter = ui.painter();
    match tex {
        Some(t) => {
            let uv = crop(t.aspect_ratio(), rect.aspect_ratio());
            let shape = egui::epaint::RectShape::filled(rect, radius, egui::Color32::WHITE)
                .with_texture(t.id(), uv);
            painter.add(shape);
        }
        None => {
            painter.rect_filled(rect, radius, ui.visuals().faint_bg_color);
        }
    }
}

/// The centred part of a texture `tex` wide per unit of height that fills a rect `rect` wide
/// per unit of height, in texture coordinates.
fn crop(tex: f32, rect: f32) -> egui::Rect {
    let size = if tex > rect {
        egui::vec2(rect / tex, 1.0)
    } else {
        egui::vec2(1.0, tex / rect)
    };
    egui::Rect::from_center_size(egui::pos2(0.5, 0.5), size)
}

/// Keyboard and mouse shortcuts. None while typing but the mouse's back and forward; Space
/// also leaves a focused widget alone (it clicks it).
fn shortcuts(ui: &egui::Ui, model: &Model, tx: &UnboundedSender<Action>, events: &mut Vec<Event>) {
    use egui::{Key, Modifiers, PointerButton};
    let ctx = ui.ctx();
    let typing = ctx.text_edit_focused();
    let focused = ctx.memory(|m| m.focused().is_some());
    let pb = &model.playback;
    let send = |a: Action| {
        let _ = tx.send(a);
    };
    let find = ctx.input_mut(|i| {
        let mut key = |mods: Modifiers, k: Key| !typing && i.consume_key(mods, k);
        let find = key(Modifiers::COMMAND, Key::F);
        let back = key(Modifiers::ALT, Key::ArrowLeft);
        let forward = key(Modifiers::ALT, Key::ArrowRight);
        if pb.track.is_some() {
            if key(Modifiers::COMMAND, Key::ArrowLeft) {
                send(Action::Previous);
            }
            if key(Modifiers::COMMAND, Key::ArrowRight) {
                send(Action::Next);
            }
            if !focused && key(Modifiers::NONE, Key::Space) {
                send(Action::TogglePlay);
            }
        }
        if back || i.pointer.button_pressed(PointerButton::Extra1) {
            events.push(Event::Back);
        }
        if forward || i.pointer.button_pressed(PointerButton::Extra2) {
            events.push(Event::Forward);
        }
        find
    });
    if find {
        events.push(Event::Navigate(Route::Search));
        ctx.memory_mut(|m| m.request_focus(search::query_id()));
    }
}

/// Draws the model; returns events for the caller to apply (the UI never mutates `Model`).
pub fn draw(
    ui: &mut egui::Ui,
    model: &Model,
    tx: &UnboundedSender<Action>,
    state: &mut UiState,
) -> Vec<Event> {
    // First, before any scroll area reads the input.
    state.scrolling.apply(ui.ctx());
    let mut events = Vec::new();
    if *model.route() != Route::Login {
        shortcuts(ui, model, tx, &mut events);
    }
    let mut logout = false;
    if !model.errors.is_empty() || model.logged_in {
        egui::Panel::top("top").show(ui, |ui| {
            if model.offline {
                let warn = ui.visuals().warn_fg_color;
                ui.colored_label(warn, tr("Offline — showing saved pages"));
            }
            for (id, e) in &model.errors {
                ui.horizontal(|ui| {
                    ui.colored_label(ui.visuals().warn_fg_color, e);
                    if ui.small_button(tr("Dismiss")).clicked() {
                        events.push(Event::DismissError(*id));
                    }
                });
            }
            if model.logged_in {
                logout = ui.small_button(tr("Log out")).clicked();
            }
        });
    }
    if logout {
        let _ = tx.send(Action::Logout);
    }
    if *model.route() != Route::Login {
        // Before the sidebar, so the bar spans the full width.
        player_bar::show(
            ui,
            model,
            tx,
            state.player.as_ref(),
            &mut state.bar,
            &mut state.images,
            &mut state.queue_open,
        );
        sidebar::show(ui, model, &mut events);
        if state.queue_open {
            queue_panel::show(ui, model, tx);
        }
    }

    egui::CentralPanel::default().show(ui, |ui| {
        let mut ctx = Ctx {
            model,
            tx,
            images: &mut state.images,
            events: &mut events,
        };
        let route = model.route();
        match route {
            Route::Login => login::show(ui, model, tx, &mut state.login),
            Route::Search => search::show(ui, &mut ctx, &mut state.search),
            Route::Home | Route::Page(_) | Route::ViewAll(_) => browse::show(ui, &mut ctx, route),
            Route::Album(_) => album::show(ui, &mut ctx, route),
            Route::Artist(_) => artist::show(ui, &mut ctx, route),
            Route::Playlist(_) => playlist::show(ui, &mut ctx, route),
            Route::Mix(_) => mix::show(ui, &mut ctx, route),
            Route::Collection(_) => collection::show(ui, &mut ctx, route),
        }
    });
    events
}

/// Fixtures and a headless pass of one view, for the views' tests.
#[cfg(test)]
mod testing {
    use super::Ctx;
    use crate::images::TextureCache;
    use crate::model::{Action, Event, Model, PageData, Route};
    use egui::{Pos2, Rect, vec2};
    use tokio::sync::mpsc::UnboundedSender;

    pub type View = fn(&mut egui::Ui, &mut Ctx<'_>, &Route);

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

    /// Logged in as user 1, at `route`.
    pub fn at(route: Route) -> Model {
        let mut m = Model::default();
        m.apply(Event::LoggedIn(1));
        m.navigate(route);
        m
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
        let mut images = TextureCache::new(1 << 20);
        let mut events = Vec::new();
        let input = egui::RawInput {
            events: input,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            ..Default::default()
        };
        let mut out = egui.run_ui(input, |ui| {
            let mut c = Ctx {
                model,
                tx,
                images: &mut images,
                events: &mut events,
            };
            view(ui, &mut c, model.route());
        });
        out.textures_delta.clear(); // nothing renders them
        (events, out)
    }

    /// `view` draws `route` loading, failed (with Retry), and loaded with `data`, then showing
    /// each of `texts`.
    pub fn every_state(view: View, route: Route, data: PageData, texts: &[&str]) {
        let egui = egui::Context::default();
        egui.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let shown = |model: &Model| -> Vec<String> {
            let (_, out) = pass(&egui, &tx, model, view, vec![]);
            let tree = out
                .platform_output
                .accesskit_update
                .expect("AccessKit is on");
            let text = |n: &egui::accesskit::Node| n.label().or(n.value()).map(str::to_owned);
            tree.nodes.iter().filter_map(|(_, n)| text(n)).collect()
        };
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    /// Ctrl+F opens Search with its box focused; while typing there, Alt+← is not Back.
    #[test]
    fn ctrl_f_focuses_search_and_typing_mutes_shortcuts() {
        let ctx = egui::Context::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = Model::default();
        model.apply(Event::LoggedIn(1));
        let mut pass = |model: &Model, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut out = Vec::new();
            let mut full = ctx.run_ui(input, |ui| out = draw(ui, model, &tx, &mut state));
            full.textures_delta.clear(); // nothing renders them
            out
        };
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let alt_left = || vec![key(Key::ArrowLeft, Modifiers::ALT)];
        assert!(matches!(pass(&model, alt_left())[..], [Event::Back]));
        for ev in pass(&model, vec![key(Key::F, Modifiers::COMMAND)]) {
            model.apply(ev);
        }
        assert_eq!(model.route(), &Route::Search);
        pass(&model, vec![]); // the box appears and takes the focus
        assert!(ctx.text_edit_focused());
        let events = pass(&model, alt_left());
        assert!(events.is_empty(), "{events:?}");
    }

    /// As a screen reader finds them: Space and Play send `TogglePlay` (the backend decides),
    /// repeat cycles Off → All → One, Queue opens the panel.
    #[test]
    fn player_bar_toggles_play_cycles_repeat_and_opens_the_queue() {
        use crate::queue::Repeat;
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Home);
        model.playback.track = Some(testing::track(1));
        let mut pass = |model: &Model, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                draw(ui, model, &tx, &mut state);
            });
            out.textures_delta.clear(); // nothing renders them
            out
        };
        let mut sent = || -> Vec<String> {
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|a| format!("{a:?}"))
                .collect()
        };
        let out = pass(&model, vec![testing::key(egui::Key::Space)]);
        pass(&model, vec![testing::click(&out, "Play")]);
        assert_eq!(sent(), ["TogglePlay", "TogglePlay"]);
        for (repeat, label, then) in [
            (Repeat::Off, "Repeat off", "SetRepeat(All)"),
            (Repeat::All, "Repeat all", "SetRepeat(One)"),
            (Repeat::One, "Repeat one", "SetRepeat(Off)"),
        ] {
            model.queue.repeat = repeat;
            let out = pass(&model, vec![]);
            pass(&model, vec![testing::click(&out, label)]);
            assert_eq!(sent(), [then]);
        }
        let out = pass(&model, vec![]);
        pass(&model, vec![testing::click(&out, "Shuffle")]);
        assert_eq!(sent(), ["SetShuffle(true)"]);
        pass(&model, vec![testing::click(&out, "Queue")]);
        assert!(state.queue_open);
    }

    /// Both errors' Dismiss clicked in one frame: both go, not one and then nothing.
    #[test]
    fn two_dismissals_in_one_frame_remove_both() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = Model::default();
        for e in ["a", "b"] {
            model.apply(Event::Error(e.into()));
        }
        let mut pass = |model: &Model, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let mut events = Vec::new();
            let mut out = ctx.run_ui(input, |ui| events = draw(ui, model, &tx, &mut state));
            out.textures_delta.clear(); // nothing renders them
            (events, out)
        };
        let (_, out) = pass(&model, vec![]);
        let (events, _) = pass(&model, testing::clicks(&out, "Dismiss").collect());
        for ev in events {
            model.apply(ev);
        }
        assert!(model.errors.is_empty(), "{:?}", model.errors);
    }

    /// `TIDASH_AUTOSCROLL` scrolls the page down first and turns round at either end without
    /// a pass standing still there; each pass asks for the next.
    #[test]
    fn autoscroll_bounces_between_the_ends_of_the_page() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        autoscroll(&ctx, 3000.0); // 50 pt a pass at egui's default 1/60 s
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let route = Route::Collection(crate::model::CollectionTab::Tracks);
        let mut model = testing::at(route.clone());
        let data = PageData::Tracks(testing::tracks(30));
        model.apply(Event::Page { route, data });
        let mut pass = || {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                draw(ui, &model, &tx, &mut state);
            });
            out.textures_delta.clear(); // nothing renders them
            let repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            let tree = out
                .platform_output
                .accesskit_update
                .expect("AccessKit is on");
            let text = |n: &egui::accesskit::Node| n.label().or(n.value()).map(str::to_owned);
            let summary = |n: &egui::accesskit::Node| text(n).is_some_and(|t| t.starts_with("30 "));
            let (_, summary) = tree
                .nodes
                .iter()
                .find(|(_, n)| summary(n))
                .expect("summary");
            assert_eq!(repaint, std::time::Duration::ZERO);
            summary.bounds().expect("placed").y0
        };
        // The page is about 360 pt longer than its view: 60 passes go to the bottom and back
        // more than once.
        let ys: Vec<f64> = (0..60).map(|_| pass()).collect();
        let moves: Vec<f64> = ys.windows(2).map(|w| w[1] - w[0]).collect();
        // Past a new scroll's first pass, every pass moves.
        assert!(moves[1..].iter().all(|m| *m != 0.0), "{ys:?}");
        // The content moves up (the page scrolls down) first, then down, then up again.
        let up: Vec<bool> = moves[1..].iter().map(|m| *m < 0.0).collect();
        assert!(up[0], "{ys:?}");
        assert!(
            up.windows(2).filter(|w| w[0] != w[1]).count() >= 2,
            "{ys:?}"
        );
    }

    /// A 3:2 image in a square keeps its middle two thirds; a square one is shown whole.
    #[test]
    fn wide_art_is_cropped_not_stretched() {
        let r = |x0: f32, x1: f32, y0: f32, y1: f32| egui::Rect::from_x_y_ranges(x0..=x1, y0..=y1);
        assert_eq!(
            crop(1.5, 1.0),
            r(0.5 - 1.0 / 3.0, 0.5 + 1.0 / 3.0, 0.0, 1.0)
        );
        assert_eq!(
            crop(1.0, 1.5),
            r(0.0, 1.0, 0.5 - 1.0 / 3.0, 0.5 + 1.0 / 3.0)
        );
        assert_eq!(crop(1.0, 1.0), r(0.0, 1.0, 0.0, 1.0));
    }
}
