//! Drawing: the player bar, the sidebar and the side panel; then the top bar, dismissible
//! errors under it, and the current route; a toast over it.

mod album;
mod artist;
mod browse;
mod card;
mod collection;
mod credits;
mod header;
mod library;
mod login;
mod lyrics;
mod mix;
mod player_bar;
mod playlist;
mod queue_panel;
mod rows;
mod search;
pub mod settings;
mod shelf;
mod sidebar;
mod toast;
mod top_bar;
mod tracks;
mod widgets;

use crate::i18n::tr;
use crate::images::TextureCache;
use crate::model::{Action, Event, Load, Model, PageData, Route, Source};
use crate::theme::{self, Palette};
use std::borrow::Cow;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// What the right-hand panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Queue,
    Lyrics,
    Credits,
}

pub struct UiState {
    login: login::State,
    search: search::State,
    bar: player_bar::State,
    pub images: TextureCache,
    /// Read only for the position; `None` until the output opened, or without a device.
    pub player: Option<player::Player>,
    scrolling: fastframe_scroll::Scrolling,
    /// The right-hand panel's view, if it shows; the player bar picks it.
    pub side: Option<Side>,
    settings: settings::State,
    library: library::State,
    /// The colours shown, once `applied` (`set_palette`).
    palette: Palette,
    applied: bool,
    /// Reveals a change of colours; `draw` paints it last.
    transition: fastframe_theme::Transition,
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
            side: None,
            settings: Default::default(),
            library: Default::default(),
            palette: Palette::dark(),
            applied: false,
            transition: fastframe_theme::Transition::new(fastframe_theme::Reveal::Band),
        }
    }
}

impl UiState {
    /// Shows `wanted` (spec §3.4): at once on this window's first call; after that, a change
    /// waits for the transition's picture of the old colours (at most 0.25 s), which `draw`
    /// then reveals away. Call it every pass, before `draw`.
    pub fn set_palette(&mut self, ctx: &egui::Context, wanted: Palette) {
        if self.applied {
            if wanted == self.palette {
                return;
            }
            self.transition.begin(ctx);
            if self.transition.holding(ctx) {
                return;
            }
        }
        theme::apply(ctx, &wanted);
        (self.palette, self.applied) = (wanted, true);
    }

    pub fn palette(&self) -> Palette {
        self.palette
    }

    /// What the UI keeps of a session ends with it (`LoggedOut`, `SessionExpired`): the next
    /// account finds the search box empty, with no undo history to bring the query back, and
    /// the library tree unasked, its dialog closed. A `Toast` shows.
    pub fn apply(&mut self, ctx: &egui::Context, ev: &Event) {
        if let Event::Toast(text) = ev {
            toast::show(ctx, text.clone());
        }
        if matches!(ev, Event::LoggedOut | Event::SessionExpired) {
            self.search = Default::default();
            self.library = Default::default();
            library::close(ctx);
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
    /// The colours shown.
    pub palette: Palette,
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

    /// `button`, which flips `item`'s heart now and has the backend write it (a failed write
    /// flips it back); disabled while that item's write is under way (ruling R19).
    pub fn favorite_button(
        &mut self,
        ui: &mut egui::Ui,
        item: tidal::Favorite,
        button: impl egui::Widget,
    ) -> egui::Response {
        let pending = self.model.pending_favorites.contains(&item);
        let resp = ui.add_enabled(!pending, button);
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
/// Right under the top bar, with nothing between (`draw`), its header paints up behind the bar
/// (`behind_bar`); nothing else on it goes there (ruling 31). It ends `MODULE_GAP` below its
/// content, clear of the player bar.
pub fn page<R>(ui: &mut egui::Ui, route: &Route, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let bar = ui.data(|d| d.get_temp::<Option<egui::Rect>>(bar_id()).flatten());
    let bar = bar.filter(|b| ui.cursor().top() <= b.bottom());
    ui.data_mut(|d| d.insert_temp(bar_id(), bar));
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
            let inner = padded(ui, |ui| ui.push_id(route, f).inner);
            gap(ui);
            inner
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

/// Fades `ui` in over `PAGE_FADE` (spec §3.11) from the pass `route`, or whether its data is
/// in (`loaded`), changes: a route change (Back and Forward too), or the data arriving after
/// the spinner. A context's first pass shows at once; the first page's data, coming in later,
/// still fades in.
/// Painting only: input works throughout.
fn fade_in(ui: &mut egui::Ui, route: &Route, loaded: bool) {
    let now = ui.input(|i| i.time);
    let last = ui.data(|d| d.get_temp::<(Route, bool, f64)>(page_fade_id()));
    let start = match last {
        Some((r, l, start)) if r == *route && l == loaded => start,
        Some(_) => now,
        None => now - f64::from(theme::PAGE_FADE),
    };
    ui.data_mut(|d| d.insert_temp(page_fade_id(), (route.clone(), loaded, start)));
    let opacity = ((now - start) as f32 / theme::PAGE_FADE).clamp(0.0, 1.0);
    if opacity < 1.0 {
        ui.ctx().request_repaint();
    }
    ui.multiply_opacity(opacity);
}

/// `f` inset `PAGE_PAD` from both sides, as `page` insets a page: the central panel has no
/// margin of its own.
fn padded<R>(ui: &mut egui::Ui, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let margin = egui::Margin::symmetric(theme::PAGE_PAD as i8, 0);
    egui::Frame::NONE.inner_margin(margin).show(ui, f).inner
}

fn autoscroll_id() -> egui::Id {
    egui::Id::new("autoscroll")
}

/// Where `draw` keeps the top bar this pass (an `Option<Rect>`, none logged out); `page` keeps
/// it only when it starts right under it, for its header (`behind_bar`).
fn bar_id() -> egui::Id {
    egui::Id::new("top bar")
}

/// Where `fade_in` keeps the route last drawn, whether its data was in, and its fade's start.
fn page_fade_id() -> egui::Id {
    egui::Id::new("page fade")
}

/// For what a page's header paints from the page's top (spec §5.4: the backdrop, the artist
/// hero): a child of `ui` and the top to paint from. On a page right under the top bar
/// (`page`), that is a bar's height above `top` (the header's), and the child's clip reaches up
/// to the bar, so at scroll 0 it starts at the window's top, behind the bar; otherwise `top`,
/// and the page's view.
fn behind_bar(ui: &mut egui::Ui, top: f32) -> (egui::Ui, f32) {
    let bar = ui.data(|d| d.get_temp::<Option<egui::Rect>>(bar_id()).flatten());
    let mut behind = ui.new_child(egui::UiBuilder::new());
    let Some(bar) = bar else {
        return (behind, top);
    };
    behind.set_clip_rect(behind.clip_rect().with_min_y(bar.top()));
    (behind, top - bar.height())
}

/// `MODULE_GAP` to the next module, the item spacing after the last one included.
fn gap(ui: &mut egui::Ui) {
    ui.add_space(theme::MODULE_GAP - ui.spacing().item_spacing.y);
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
        padded(ui, |ui| {
            if slot.failed {
                ui.label(tr("Couldn't load this page"));
                let retry = widgets::pill_button(ui, &ctx.palette, None, &tr("Retry"), false);
                if retry.clicked() {
                    ctx.go(route.clone()); // asks for it again
                }
            } else if slot.loading {
                ui.spinner();
            }
        });
    }
    slot.data.as_ref()
}

/// What playing from the current page starts from (spec §3.9): its route and title. None
/// before its data is in, or on a page with nothing to go back to (Home, Search, View all).
pub fn playing_from(model: &Model) -> Option<Source> {
    let route = model.route();
    let name = match (route, model.pages.get(route)?.data.as_ref()?) {
        (Route::Album(_), PageData::Album { album, .. }) => album.title.clone(),
        (Route::Playlist(_), PageData::Playlist { playlist, .. }) => playlist.title.clone(),
        (Route::Mix(_), PageData::Mix { mix, .. }) => mix.title.clone(),
        (Route::Artist(_), PageData::Artist(page)) => page.artist.name.clone(),
        (Route::Collection(tab), _) => collection::name(*tab).into_owned(),
        _ => return None,
    };
    let route = route.clone();
    Some(Source { route, name })
}

/// The current track's data in `slot` (lyrics or credits), once ready. A slot of another
/// track, or none, is asked for: `ask` gives the `Loading` event and the action. Until ready, a
/// spinner, or `failed` and Retry. Without a current track it says so.
fn track_data<'m, T>(
    ui: &mut egui::Ui,
    ctx: &mut Ctx<'m>,
    slot: &'m Option<(u64, Load<T>)>,
    failed: &str,
    ask: fn(u64) -> (Event, Action),
) -> Option<&'m T> {
    let Some(track) = &ctx.model.playback.track else {
        ui.label(tr("Nothing is playing"));
        return None;
    };
    let id = track.id;
    let asks = |ctx: &mut Ctx| {
        let (ev, action) = ask(id);
        ctx.events.push(ev);
        ctx.send(action);
    };
    match slot
        .as_ref()
        .filter(|(at, _)| *at == id)
        .map(|(_, load)| load)
    {
        Some(Load::Ready(data)) => return Some(data),
        Some(Load::Failed) => {
            ui.label(failed);
            let retry = widgets::pill_button(ui, &ctx.palette, None, &tr("Retry"), false);
            if retry.clicked() {
                asks(ctx);
            }
        }
        Some(Load::Loading) => {
            ui.spinner();
        }
        None => {
            asks(ctx);
            ui.spinner();
        }
    }
    None
}

/// Paints `image` over `rect` with corners of `radius`, requested as many pixels wide as it
/// is drawn (at most its largest rendition); or a flat placeholder, which the artwork fades in
/// over when it first comes (`COVER_FADE`). Nothing out of view is requested. Artwork of
/// another shape (a wide playlist image) is cropped around its centre, not stretched.
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
    let px = request_px(width * ui.ctx().pixels_per_point());
    let tex = image.and_then(|img| {
        // No wider than the largest rendition: `decode` never scales up, so a wider key would
        // be the same picture again (the artist hero, at 750 px).
        let px = px.min(img.max_px());
        images.get(ui.ctx(), img.key(), px, || {
            let _ = tx.send(Action::LoadImage {
                image: img.clone(),
                px,
            });
        })
    });
    // The opacity, by image, with the pass it was last drawn in. Not drawn last pass (new, out
    // of view, another page), it shows as it is now; only a texture coming while it is drawn
    // fades. Every copy in a pass shares the first's step; a size swap keeps it at 1.
    let a = image.map_or(0.0, |img| {
        let ctx = ui.ctx();
        let id = egui::Id::new(("cover", img.key()));
        let to = if tex.is_some() { 1.0 } else { 0.0 };
        let now = ctx.cumulative_pass_nr();
        // As egui's own: no step in a pass at the same time (a discarded one's rerun).
        let step = ctx.input(|i| i.stable_dt.min(i.unstable_dt)) / theme::COVER_FADE;
        let a = ctx.data_mut(|d| {
            let a = match d.get_temp::<(u64, f32)>(id) {
                Some((pass, a)) if pass == now => a,
                Some((pass, a)) if pass + 1 == now && to > a => (a + step).min(to),
                Some((pass, a)) if pass + 1 == now => (a - step).max(to),
                _ => to,
            };
            d.insert_temp(id, (now, a));
            a
        });
        if a != to {
            ctx.request_repaint();
        }
        a
    });
    let painter = ui.painter();
    if a < 1.0 {
        painter.rect_filled(rect, radius, ui.visuals().faint_bg_color);
    }
    if let Some(t) = tex {
        let uv = crop(t.aspect_ratio(), rect.aspect_ratio());
        let tint = egui::Color32::WHITE.gamma_multiply(a);
        let shape = egui::epaint::RectShape::filled(rect, radius, tint).with_texture(t.id(), uv);
        painter.add(shape);
    }
}

/// The width to ask for a cover drawn `width_px` pixels wide: exact up to 64 px (rows, the
/// sidebar, tint sources), else rounded up to a multiple of 32, so a card that widens with the
/// window keeps its texture.
fn request_px(width_px: f32) -> u32 {
    let px = width_px.ceil().max(1.0) as u32;
    if px > 64 { px.next_multiple_of(32) } else { px }
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

/// The playing cover's tint (spec §5.9): with `accent_from_art`, once that cover's `TINT_PX`
/// image is in, which it asks for meanwhile, only at that size (ruling 20).
fn tint(
    ctx: &egui::Context,
    model: &Model,
    tx: &UnboundedSender<Action>,
    images: &mut TextureCache,
    p: &Palette,
) -> Option<egui::Color32> {
    if !model.settings.accent_from_art {
        return None;
    }
    let image = model.playback.track.as_ref()?.album.image()?;
    let px = theme::TINT_PX;
    let cached = images.get_exact(ctx, image.key(), px, || {
        let image = image.clone();
        let _ = tx.send(Action::LoadImage { image, px });
    });
    cached.and(images.tint(image.key())).map(|c| p.tint(c))
}

/// The player bar's fill: `panel` blended 12% toward `tint`, each channel eased over
/// `TINT_FADE`; still once there, so nothing repaints at rest.
fn bar_fill(ctx: &egui::Context, p: &Palette, tint: Option<egui::Color32>) -> egui::Color32 {
    let to = tint.map_or(p.panel, |t| theme::blend(p.panel, t, 0.12));
    let id = egui::Id::new("player tint");
    // Channel by channel in place: a palette colour eased, never one written out here.
    let mut fill = to;
    for (i, c) in to.to_array().into_iter().enumerate() {
        let v = ctx.animate_value_with_time(id.with(i), f32::from(c), theme::TINT_FADE);
        fill[i] = v.round() as u8;
    }
    fill
}

/// Keyboard and mouse shortcuts but Quit (`draw`); Settings lists the keys
/// (`settings::shortcuts`). None while typing but the mouse's back and forward; Space also
/// leaves a focused widget alone (it clicks it).
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
        let find = model.logged_in && key(Modifiers::COMMAND, Key::F); // Search needs a session
        if key(Modifiers::COMMAND, Key::Comma) {
            events.push(Event::Navigate(Route::Settings));
        }
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
    let (mut events, palette) = (Vec::new(), state.palette());
    // Ctrl+Q (Cmd+Q) on every page, typing or not: the app's quit, never a bare window close.
    if ui
        .ctx()
        .input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Q))
    {
        events.push(Event::Quit);
    }
    if *model.route() != Route::Login {
        shortcuts(ui, model, tx, &mut events);
    }
    if *model.route() != Route::Settings {
        state.settings.listed = false; // the next visit starts afresh
    }
    // Logged in, they sit under the top bar.
    if !model.logged_in && (!model.errors.is_empty() || model.offline) {
        egui::Panel::top("top").show(ui, |ui| {
            let mut ctx = Ctx {
                model,
                tx,
                images: &mut state.images,
                events: &mut events,
                palette,
            };
            top_bar::notices(ui, &mut ctx);
        });
    }
    let tint = tint(ui.ctx(), model, tx, &mut state.images, &palette);
    // Logged out (Settings, from the login page) nothing leads to the session's pages.
    if model.logged_in {
        // Before the sidebar, so the bar spans the full width.
        player_bar::show(
            ui,
            model,
            tx,
            state.player.as_ref(),
            &mut state.bar,
            &mut state.images,
            &mut state.side,
            &palette,
            bar_fill(ui.ctx(), &palette, tint),
            &mut events,
        );
        let mut ctx = Ctx {
            model,
            tx,
            images: &mut state.images,
            events: &mut events,
            palette,
        };
        sidebar::show(ui, &mut ctx, &mut state.library);
        if let Some(side) = state.side {
            let at = state.player.as_ref();
            let at = at.map_or(Duration::ZERO, player::Player::position);
            let mut ctx = Ctx {
                model,
                tx,
                images: &mut state.images,
                events: &mut events,
                palette,
            };
            // Spec §5.8; egui's separator line is its 1 px `outline` left edge (`theme::style`).
            let frame = egui::Frame::NONE
                .fill(palette.window)
                .inner_margin(theme::PAGE_PAD);
            egui::Panel::right("side")
                .resizable(true)
                .default_size(theme::SIDE_PANEL)
                .size_range(theme::SIDE_PANEL_MIN..=theme::SIDE_PANEL_MAX)
                .frame(frame)
                .show(ui, |ui| {
                    // Short content (an empty queue, a spinner) would shrink it to its minimum.
                    ui.take_available_width();
                    match side {
                        Side::Queue => queue_panel::show(ui, &mut ctx),
                        Side::Lyrics => lyrics::show(ui, &mut ctx, at),
                        Side::Credits => credits::show(ui, &mut ctx),
                    }
                });
        }
    }

    // No margin: pages inset themselves (`page`, `padded`), their scroll bars at the edge.
    let frame = egui::Frame::central_panel(ui.style()).inner_margin(0);
    let central = egui::CentralPanel::default().frame(frame).show(ui, |ui| {
        let mut ctx = Ctx {
            model,
            tx,
            images: &mut state.images,
            events: &mut events,
            palette,
        };
        let route = model.route();
        // Pages without a cover of their own: the tint behind their top, under the top bar.
        let coverless = matches!(
            route,
            Route::Home | Route::Page(_) | Route::ViewAll(_) | Route::Collection(_) | Route::Search
        );
        if let Some(t) = tint.filter(|_| coverless) {
            let page = ui.max_rect();
            let top = page.with_max_y(page.top() + theme::TINT_HEADER);
            let from = theme::blend(palette.window, t, 0.35);
            widgets::vertical_gradient(ui.painter(), top, from, palette.window);
        }
        // The top bar first, for Tab and screen readers (on a layer over the page's, ruling 31),
        // then the notices under it.
        let bar = model.logged_in.then(|| {
            let bar = ui.available_rect_before_wrap();
            let bar = bar.with_max_y(bar.top() + theme::TOP_BAR);
            top_bar::show(ui, &mut ctx, &mut state.search, bar);
            ui.add_space(theme::TOP_BAR);
            top_bar::notices(ui, &mut ctx);
            bar
        });
        if bar.is_none() {
            ui.add_space(theme::PAGE_PAD);
        }
        ui.data_mut(|d| d.insert_temp(bar_id(), bar));
        // The route's content fades in, its title, spinner or failure too; all above stays put.
        // Login, Search and Settings have no page data to wait for; a Collection tab paints its
        // title before its data, so it fades on the switch only, lest the title blink.
        let loaded = matches!(
            route,
            Route::Login | Route::Search | Route::Settings | Route::Collection(_)
        ) || model.pages.get(route).is_some_and(|s| s.data.is_some());
        fade_in(ui, route, loaded);
        match route {
            Route::Login => login::show(ui, &mut ctx, &mut state.login),
            Route::Search => search::show(ui, &mut ctx),
            Route::Home | Route::Page(_) | Route::ViewAll(_) => browse::show(ui, &mut ctx, route),
            Route::Album(_) => album::show(ui, &mut ctx, route),
            Route::Artist(_) => artist::show(ui, &mut ctx, route),
            Route::Playlist(_) => playlist::show(ui, &mut ctx, route),
            Route::Mix(_) => mix::show(ui, &mut ctx, route),
            Route::Collection(_) => collection::show(ui, &mut ctx, route),
            Route::Settings => settings::show(ui, &mut ctx, &mut state.settings),
        }
    });
    // Centred on the window, not the content area beside the sidebar; above the player bar.
    let over = egui::Rect::from_x_y_ranges(
        ui.ctx().content_rect().x_range(),
        central.response.rect.y_range(),
    );
    toast::paint(ui.ctx(), &palette, over);
    state.transition.paint(ui.ctx()); // over everything
    events
}

#[cfg(test)]
pub(crate) mod testing;

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    /// Ctrl+F opens Search with its box focused; while typing there, Alt+← is not Back.
    #[test]
    fn ctrl_f_focuses_search_and_typing_mutes_shortcuts() {
        let ctx = testing::context();
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

    #[test]
    fn ctrl_comma_opens_settings() {
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Home);
        let comma = egui::Event::Key {
            key: Key::Comma,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };
        let input = egui::RawInput {
            events: vec![comma],
            ..Default::default()
        };
        let mut events = Vec::new();
        let mut out = ctx.run_ui(input, |ui| events = draw(ui, &model, &tx, &mut state));
        out.textures_delta.clear(); // nothing renders them
        for ev in events {
            model.apply(ev);
        }
        assert_eq!(model.route(), &Route::Settings);
    }

    /// Ctrl+Q (Cmd+Q on macOS) quits on every page, the login page too, and while typing. The
    /// key never reaches egui's own Ctrl+Q, a bare window close, which would hide to the tray.
    #[test]
    fn ctrl_q_quits() {
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = Model::default();
        let ctrl = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        };
        // The events, and whether the window was told to close.
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
            let commands = &full.viewport_output[&egui::ViewportId::ROOT].commands;
            let close = commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Close));
            (out, close)
        };
        let quits = |(events, close): (Vec<Event>, bool), page: &str| {
            assert!(matches!(events[..], [Event::Quit]), "{page}: {events:?}");
            assert!(!close, "{page}: a bare close");
        };
        assert_eq!(model.route(), &Route::Login);
        quits(pass(&model, vec![ctrl(Key::Q)]), "login");
        model.apply(Event::LoggedIn(1));
        quits(pass(&model, vec![ctrl(Key::Q)]), "logged in");
        for ev in pass(&model, vec![ctrl(Key::F)]).0 {
            model.apply(ev);
        }
        pass(&model, vec![]); // the search box appears and takes the focus
        assert!(ctx.text_edit_focused());
        quits(pass(&model, vec![ctrl(Key::Q)]), "typing");
    }

    /// As a screen reader finds them: Space and Play send `TogglePlay` (the backend decides),
    /// repeat cycles Off → All → One, Queue and then Lyrics take the side panel.
    #[test]
    fn player_bar_toggles_play_cycles_repeat_and_picks_the_side_panel() {
        use crate::queue::Repeat;
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Home);
        model.playback.track = Some(testing::track(1));
        // The output, and the side panel after it.
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
            (out, state.side)
        };
        let mut sent = || -> Vec<String> {
            std::iter::from_fn(|| rx.try_recv().ok())
                .map(|a| format!("{a:?}"))
                .collect()
        };
        let (out, _) = pass(&model, vec![testing::key(egui::Key::Space)]);
        pass(&model, vec![testing::click(&out, "Play")]);
        assert_eq!(sent(), ["TogglePlay", "TogglePlay"]);
        for (repeat, label, then) in [
            (Repeat::Off, "Repeat off", "SetRepeat(All)"),
            (Repeat::All, "Repeat all", "SetRepeat(One)"),
            (Repeat::One, "Repeat one", "SetRepeat(Off)"),
        ] {
            model.queue.repeat = repeat;
            let (out, _) = pass(&model, vec![]);
            pass(&model, vec![testing::click(&out, label)]);
            assert_eq!(sent(), [then]);
        }
        let (out, _) = pass(&model, vec![]);
        pass(&model, vec![testing::click(&out, "Shuffle")]);
        assert_eq!(sent(), ["SetShuffle(true)"]);
        let (_, side) = pass(&model, vec![testing::click(&out, "Queue")]);
        assert_eq!(side, Some(Side::Queue));
        let (out, _) = pass(&model, vec![]);
        let (_, side) = pass(&model, vec![testing::click(&out, "Lyrics")]);
        assert_eq!(side, Some(Side::Lyrics));
    }

    /// Both errors' Dismiss clicked in one frame: both go, not one and then nothing.
    #[test]
    fn two_dismissals_in_one_frame_remove_both() {
        let ctx = testing::context();
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
        let ctx = testing::context();
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

    /// Light and then Nord in the next pass, and no screenshot ever arrives (headless, some
    /// drivers): the window ends on Nord once the transition stops waiting, never on Light.
    #[test]
    fn quick_changes_end_on_the_last_palette() {
        use crate::theme::Palette;
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = Model::default();
        let mut presets = fastframe_theme::presets::themes::<Palette>();
        let nord = presets.find(|t| t.filename == "Nord.json");
        let nord = nord.expect("Nord ships").palette;
        let (dark, light) = (Palette::dark(), Palette::light());
        let mut pass = |tenths: u8, wanted: Palette| {
            let input = egui::RawInput {
                time: Some(f64::from(tenths) / 10.0),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                state.set_palette(ui.ctx(), wanted);
                draw(ui, &model, &tx, &mut state);
            });
            out.textures_delta.clear(); // nothing renders them
            state.palette()
        };
        assert_eq!(pass(0, dark), dark, "the first applies at once");
        assert_eq!(ctx.global_style().visuals.panel_fill, dark.window);
        assert_eq!(pass(1, dark), dark);
        let shown: Vec<Palette> = [(2, light), (3, nord), (4, nord), (5, nord), (6, nord)]
            .into_iter()
            .map(|(t, wanted)| pass(t, wanted))
            .collect();
        assert!(!shown.contains(&light), "{shown:?}");
        assert_eq!(shown.last(), Some(&nord));
        assert_eq!(ctx.global_style().visuals.panel_fill, nord.window);
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

    /// Spec §5.9: a playing track without a cover, or `accent_from_art` off, leaves the player
    /// bar plain `panel` once the fade settles; a cover's 16 px image tints it, and it eases
    /// back. Settled, nothing repaints.
    #[test]
    fn no_cover_no_tint() {
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Home);
        model.playback.track = Some(testing::track(1)); // no cover
        let mut time = 0.0;
        // The bar's fill after a second of passes, past `TINT_FADE`; still by then.
        let mut settled = |model: &Model, state: &mut UiState| {
            let mut fill = None;
            for i in 0..10 {
                time += 0.1;
                let input = egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| {
                    draw(ui, model, &tx, state);
                });
                out.textures_delta.clear(); // nothing renders them
                let repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
                assert!(i < 6 || repaint == Duration::MAX, "repaints at rest");
                fill = out.shapes.iter().find_map(|s| match &s.shape {
                    egui::Shape::Rect(r)
                        if r.rect.bottom() == 600.0 && r.rect.height() == theme::PLAYER_BAR =>
                    {
                        Some(r.fill)
                    }
                    _ => None,
                });
            }
            fill.expect("a player bar")
        };
        let panel = Palette::dark().panel;
        assert_eq!(settled(&model, &mut state), panel, "no cover");
        let mut track = testing::track(1);
        track.album.cover = Some("ab-cd".into());
        model.playback.track = Some(track);
        let (px, red) = (theme::TINT_PX, egui::Color32::RED);
        let red = egui::ColorImage::filled([px as usize; 2], red);
        state.images.insert(&ctx, "ab-cd".into(), px, red);
        let tinted = settled(&model, &mut state);
        assert!(tinted.r() > tinted.g() + 8, "toward red: {tinted:?}");
        model.settings.accent_from_art = false;
        assert_eq!(settled(&model, &mut state), panel, "accent_from_art off");
    }

    /// One pass of `draw` in an 800 by 600 window at `time` (egui's own clock if `None`), fed
    /// `events`: egui's output.
    fn frame(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        state: &mut UiState,
        model: &Model,
        time: Option<f64>,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            events,
            time,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            draw(ui, model, tx, state);
        });
        out.textures_delta.clear(); // nothing renders them
        out
    }

    /// Spec §3.4: scrolled to its end, a page keeps `MODULE_GAP` above the player bar; Settings'
    /// last control, Check now, too.
    #[test]
    fn every_page_ends_with_room() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let model = testing::at(Route::Settings);
        // Down until it turns round at the end; that last step lands, then the page is still.
        autoscroll(&ctx, 6000.0);
        for _ in 0..100 {
            if ctx.data(|d| d.get_temp::<f32>(autoscroll_id())) < Some(0.0) {
                break;
            }
            frame(&ctx, &tx, &mut state, &model, None, vec![]);
        }
        autoscroll(&ctx, 0.0);
        frame(&ctx, &tx, &mut state, &model, None, vec![]);
        let out = frame(&ctx, &tx, &mut state, &model, None, vec![]);
        let check = testing::bounds(&out, "Check now");
        let bar = 600.0 - theme::PLAYER_BAR;
        assert!(check.bottom() + theme::MODULE_GAP <= bar + 1.0, "{check:?}");
    }

    /// How opaque `out` paints the text `text` right of the sidebar (which lists Home and the
    /// Collection tabs too), from its first glyph's colour: 0 if it paints none there (egui
    /// drops a shape at opacity 0).
    fn opacity(out: &egui::FullOutput, text: &str) -> f32 {
        let painted = out.shapes.iter().find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == text && t.pos.x >= theme::SIDEBAR => {
                let rows = t.galley.rows.iter();
                let v = rows.flat_map(|r| &r.row.visuals.mesh.vertices).next()?;
                let c = if v.color == egui::Color32::PLACEHOLDER {
                    t.fallback_color
                } else {
                    v.color
                };
                Some(t.opacity_factor * f32::from(c.a()) / 255.0)
            }
            _ => None,
        });
        painted.unwrap_or(0.0)
    }

    /// Spec §3.11: the route's content fades in over `PAGE_FADE` from the very pass the route
    /// changes between loaded pages, on Back, on a Collection tab switch (its title is drawn
    /// outside `page`) but not when that tab's data comes in, and when other data comes in
    /// after the spinner; then it rests at 1 with no repaints. The first page a context draws shows at once. Input works mid-fade: the
    /// first pass, unseen, already takes the pointer.
    #[test]
    fn pages_fade_in() {
        use crate::model::CollectionTab;
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut pass = |model: &Model, time: f64, events: Vec<egui::Event>| {
            frame(&ctx, &tx, &mut state, model, Some(time), events)
        };
        let (route, title) = (Route::Album(1), "Album 1");
        let data = || PageData::Album {
            album: testing::album(1),
            tracks: testing::tracks(3),
        };
        let mut model = testing::at(Route::Home);
        let home = tidal::Page {
            title: String::new(),
            shelves: vec![],
        };
        model.apply(Event::Page {
            route: Route::Home,
            data: PageData::Shelves(home),
        });
        let out = pass(&model, 0.0, vec![]);
        assert_eq!(opacity(&out, "Home"), 1.0, "the first page shows at once");
        model.navigate(route.clone());
        model.apply(Event::Page {
            route: route.clone(),
            data: data(),
        });
        assert_eq!(opacity(&pass(&model, 0.1, vec![]), title), 0.0, "next pass");
        let mid = opacity(&pass(&model, 0.175, vec![]), title);
        assert!(0.0 < mid && mid < 1.0, "{mid}");
        assert_eq!(opacity(&pass(&model, 0.3, vec![]), title), 1.0, "faded in");
        let out = pass(&model, 0.4, vec![]);
        assert_eq!(opacity(&out, title), 1.0, "settled");
        let repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert_eq!(repaint, Duration::MAX, "repaints at rest");
        model.apply(Event::Back);
        assert_eq!(opacity(&pass(&model, 0.5, vec![]), "Home"), 0.0, "Back");
        model.navigate(Route::Collection(CollectionTab::Tracks));
        pass(&model, 1.0, vec![]);
        model.navigate(Route::Collection(CollectionTab::Albums));
        assert_eq!(opacity(&pass(&model, 1.5, vec![]), "Albums"), 0.0, "tab");
        let mid = opacity(&pass(&model, 1.575, vec![]), "Albums");
        assert!(0.0 < mid && mid < 1.0, "the tab's title: {mid}");
        let albums = Route::Collection(CollectionTab::Albums);
        model.apply(Event::Loading(albums.clone()));
        assert_eq!(opacity(&pass(&model, 1.7, vec![]), "Albums"), 1.0, "in");
        model.apply(Event::Page {
            route: albums,
            data: PageData::Items(vec![tidal::Item::Album(testing::album(1))]),
        });
        let out = pass(&model, 1.75, vec![]);
        assert_eq!(opacity(&out, "Albums"), 1.0, "no second fade");
        let out = pass(&model, 1.8, vec![]);
        let repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert_eq!(repaint, Duration::MAX, "the tab at rest");
        // Loading, then the data.
        let mut model = testing::at(route.clone());
        model.apply(Event::Loading(route.clone()));
        pass(&model, 2.0, vec![]); // the spinner
        model.apply(Event::Page {
            route,
            data: data(),
        });
        let out = pass(&model, 2.5, vec![]);
        assert_eq!(opacity(&out, title), 0.0, "the data in");
        // The album's Play pill (the player bar's is disabled), placed by that unseen pass.
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let play = nodes
            .iter()
            .find(|(_, n)| n.label() == Some("Play") && !n.is_disabled());
        let b = play.and_then(|(_, n)| n.bounds());
        let b = b.expect("the album's Play");
        let at = egui::pos2(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let press = vec![egui::Event::PointerMoved(at), button(true)];
        pass(&model, 2.55, press);
        let out = pass(&model, 2.6, vec![button(false)]);
        assert!(opacity(&out, title) < 1.0, "still fading");
        let played = std::iter::from_fn(|| rx.try_recv().ok())
            .any(|a| matches!(a, Action::PlayTracks { .. }));
        assert!(played, "the click is delivered mid-fade");
    }

    /// Each textured shape in `out`: its texture and its tint's alpha.
    fn covers(out: &egui::FullOutput) -> Vec<(egui::TextureId, u8)> {
        let shapes = out.shapes.iter();
        let textured = shapes.filter_map(|s| match &s.shape {
            egui::Shape::Rect(r) => r.brush.as_ref().map(|b| (b.fill_texture_id, r.fill.a())),
            _ => None,
        });
        textured.collect()
    }

    /// Track 1, its album's cover "ab-cd".
    fn covered() -> tidal::Track {
        let mut track = testing::track(1);
        track.album.cover = Some("ab-cd".into());
        track
    }

    /// Caches "ab-cd" `px` wide.
    fn arrives(ctx: &egui::Context, state: &mut UiState, px: f32) {
        let grey = egui::ColorImage::filled([px as usize; 2], egui::Color32::GRAY);
        state.images.insert(ctx, "ab-cd".into(), px as u32, grey);
    }

    /// Spec §3.11: a cover first drawn without its texture fades in over its placeholder when
    /// the texture comes, over `COVER_FADE`, then rests with no repaints. The player bar's,
    /// outside the page's own fade.
    #[test]
    fn a_cover_fades_in_when_it_arrives() {
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Home);
        model.settings.accent_from_art = false; // no tint easing in meanwhile
        model.playback.track = Some(covered());
        let pass = |state: &mut UiState, t| frame(&ctx, &tx, state, &model, Some(t), vec![]);
        assert_eq!(covers(&pass(&mut state, 0.0)), [], "the placeholder only");
        arrives(&ctx, &mut state, theme::PLAYER_COVER);
        let fading = covers(&pass(&mut state, 0.05));
        assert!(
            matches!(fading[..], [(_, a)] if 0 < a && a < 255),
            "{fading:?}"
        );
        let shown = covers(&pass(&mut state, 0.3));
        assert!(matches!(shown[..], [(_, 255)]), "{shown:?}");
        let out = pass(&mut state, 0.4);
        let repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert_eq!(repaint, Duration::MAX, "repaints at rest");
    }

    /// Spec §3.11: a cover already cached when first drawn shows at once; the larger size
    /// replacing the one shown meanwhile swaps in without a fade.
    #[test]
    fn a_cached_cover_shows_at_once() {
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Home);
        model.settings.accent_from_art = false;
        model.playback.track = Some(covered());
        let pass = |state: &mut UiState, t| frame(&ctx, &tx, state, &model, Some(t), vec![]);
        arrives(&ctx, &mut state, 32.0); // another size, shown while the bar's loads
        let small = covers(&pass(&mut state, 0.0));
        assert!(matches!(small[..], [(_, 255)]), "first pass: {small:?}");
        arrives(&ctx, &mut state, theme::PLAYER_COVER);
        let large = covers(&pass(&mut state, 0.05));
        assert!(matches!(large[..], [(_, 255)]), "the swap: {large:?}");
        assert_ne!(small[0].0, large[0].0, "the larger size shown");
    }

    /// Spec §3.11: a cover whose texture came while it was not drawn (scrolled away, another
    /// page; here nothing playing for a while) shows at once when drawn again, and rests.
    #[test]
    fn a_cover_loaded_out_of_view_shows_at_once() {
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let mut model = testing::at(Route::Home);
        model.settings.accent_from_art = false;
        model.playback.track = Some(covered());
        let out = frame(&ctx, &tx, &mut state, &model, Some(0.0), vec![]);
        assert_eq!(covers(&out), [], "the placeholder only");
        model.playback.track = None;
        for t in [0.1, 0.2] {
            frame(&ctx, &tx, &mut state, &model, Some(t), vec![]);
        }
        arrives(&ctx, &mut state, theme::PLAYER_COVER);
        model.playback.track = Some(covered());
        let out = frame(&ctx, &tx, &mut state, &model, Some(0.3), vec![]);
        let shown = covers(&out);
        assert!(matches!(shown[..], [(_, 255)]), "drawn again: {shown:?}");
        let repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert_eq!(repaint, Duration::MAX, "nothing pending");
    }

    /// Spec §3.11: one cover drawn in many places (five rows of one album, and the player bar
    /// playing one of them) fades in once, every copy in step over the whole `COVER_FADE`, and
    /// stays.
    #[test]
    fn one_cover_in_many_places_fades_once() {
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = UiState::default();
        let route = Route::Playlist("p-1".into());
        let mut model = testing::at(route.clone());
        model.settings.accent_from_art = false;
        let tracks: Vec<_> = (0..5).map(|_| covered()).collect();
        model.playback.track = Some(covered());
        let data = PageData::Playlist {
            playlist: testing::playlist(),
            tracks,
            positions: (0..5).collect(),
            etag: None,
        };
        model.apply(Event::Page { route, data });
        // Tall enough for every row.
        let pass = |state: &mut UiState, time| {
            let input = egui::RawInput {
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 1200.0),
                )),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                draw(ui, &model, &tx, state);
            });
            out.textures_delta.clear(); // nothing renders them
            out
        };
        pass(&mut state, 0.0);
        // The page still; the playlist's own image never comes.
        assert_eq!(covers(&pass(&mut state, 1.0)), [], "placeholders only");
        arrives(&ctx, &mut state, theme::ROW_COVER);
        arrives(&ctx, &mut state, theme::PLAYER_COVER);
        let alphas = |out: &egui::FullOutput| -> Vec<u8> {
            covers(out).into_iter().map(|(_, a)| a).collect()
        };
        pass(&mut state, 1.05); // it comes
        // Mid-fade, every copy as far along: drawn six times, it fades no faster.
        let mid = alphas(&pass(&mut state, 1.15));
        let even = mid.len() == 6 && mid.iter().all(|a| *a == mid[0]);
        assert!(even && 0 < mid[0] && mid[0] < 255, "mid-fade: {mid:?}");
        for time in [1.3, 1.4] {
            assert_eq!(alphas(&pass(&mut state, time)), [255; 6], "at {time}");
        }
        let out = pass(&mut state, 1.5);
        assert_eq!(alphas(&out), [255; 6], "still");
        let repaint = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        assert_eq!(repaint, Duration::MAX, "repaints at rest");
    }

    /// Small covers are asked for exactly; larger ones in steps of 32 px, so a card that
    /// widens a little keeps its texture.
    #[test]
    fn cover_sizes_round_up_above_64_px() {
        let cases = [(16.0, 16), (40.0, 40), (64.0, 64), (65.0, 96), (172.3, 192)];
        for (width, px) in cases {
            assert_eq!(request_px(width), px, "{width}");
        }
    }

    /// Spec §3.9: playing from a page names it by its title; a page with nothing to return to,
    /// or whose data is not in yet, names nothing.
    #[test]
    fn playing_from_names_the_page() {
        use crate::model::CollectionTab;
        let from = |route: Route, data: Option<PageData>| {
            let mut model = testing::at(route.clone());
            if let Some(data) = data {
                model.apply(Event::Page { route, data });
            }
            playing_from(&model)
        };
        let named = |route: Route, data, name: &str| {
            let want = Source {
                route: route.clone(),
                name: name.into(),
            };
            assert_eq!(from(route, Some(data)), Some(want));
        };
        let tracks = testing::tracks(2);
        let album = testing::album(1);
        let album = PageData::Album {
            album,
            tracks: tracks.clone(),
        };
        named(Route::Album(1), album, "Album 1");
        let playlist = PageData::Playlist {
            playlist: testing::playlist(),
            tracks: tracks.clone(),
            positions: vec![0, 1],
            etag: None,
        };
        named(Route::Playlist("p-1".into()), playlist, "Playlist");
        let mix = PageData::Mix {
            mix: testing::mix(),
            tracks: tracks.clone(),
        };
        named(Route::Mix("m1".into()), mix, "Mix");
        let artist = tidal::ArtistPage {
            artist: testing::artist(1),
            bio: None,
            shelves: vec![],
        };
        named(Route::Artist(1), PageData::Artist(artist), "Artist 1");
        let tab = CollectionTab::Tracks;
        let favorites = PageData::Tracks(tracks.clone());
        named(Route::Collection(tab), favorites, &collection::name(tab));
        let home = PageData::Shelves(Default::default());
        assert_eq!(from(Route::Home, Some(home)), None);
        assert_eq!(from(Route::Search, None), None);
        let all = Some(PageData::Tracks(tracks));
        assert_eq!(from(Route::ViewAll("x".into()), all), None);
        assert_eq!(from(Route::Album(2), None), None, "no data yet");
    }
}
