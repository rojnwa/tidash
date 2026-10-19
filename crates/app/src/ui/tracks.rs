//! Track list, virtualized inside the caller's page scroll; shared by every view with tracks.

use super::widgets::{
    context_menu, fade, focus_ring, line, menu_button, menu_separator, paint_icon,
};
use super::{Ctx, library, queue_panel, rows};
use crate::i18n::{tr, tr_n};
use crate::model::{Action, PlaylistEdit, Route};
use crate::theme::{
    self, COL_GAP, COVER_COL, GLYPH, INDEX_COL, Icon, Palette, ROW_COVER, TIME_COL, Tier,
};
use egui::text::CCursor;
use egui::{
    Align, Color32, DragAndDrop, FontId, Galley, PointerButton, Pos2, Rangef, Rect, Sense, Stroke,
    Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};
use std::ops::Range;
use std::sync::Arc;

pub const ROW_H: f32 = theme::ROW;

#[derive(Clone, Copy)]
enum Row {
    Disc(u32),
    Track(usize),
}

/// The user's own playlist `uuid`, whose rows can be removed and dragged to reorder: each
/// row's position in it, and the `etag` of the listing those came from (ruling R11).
pub struct PlaylistRows<'a> {
    pub uuid: &'a str,
    pub etag: &'a str,
    pub positions: &'a [u32],
}

/// The playlist row being dragged, and the ETag of the listing it is a row of: one reloaded
/// mid-drag has other rows.
struct PlaylistDrag {
    from: usize,
    etag: String,
}

/// The `(from, to)` playlist positions a drop of row `from_row` at `slot` (`0..=len`) moves:
/// `to` is the position of the row dropped before, or past the last. None onto the row's own
/// edges.
fn move_for(positions: &[u32], from_row: usize, slot: usize) -> Option<(u32, u32)> {
    if slot == from_row || slot == from_row + 1 {
        return None;
    }
    let to = match positions.get(slot) {
        Some(&to) => to,
        None => positions.last()? + 1,
    };
    Some((*positions.get(from_row)?, to))
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn duration(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// "{n} tracks · {duration}" of `tracks`.
pub fn summary(tracks: &[tidal::Track]) -> String {
    let n = tr_n("{n} track", "{n} tracks", tracks.len() as u32);
    format!(
        "{n} · {}",
        duration(tracks.iter().map(|t| t.duration).sum())
    )
}

pub fn names(artists: &[tidal::Artist]) -> String {
    artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn artists(t: &tidal::Track) -> String {
    names(&t.artists)
}

/// Double-clicking an available row, or activating it from the keyboard or a screen reader,
/// sends `PlayTracks` for the whole list; right-clicking opens its menu. With `discs`,
/// multi-disc lists get a header row per disc; otherwise the album title is a column. `id`
/// tells apart two lists on one page. With `edit` (and no `discs`), rows can be removed from
/// the playlist and dragged to reorder it.
pub fn show(
    ui: &mut egui::Ui,
    ctx: &mut Ctx,
    id: &str,
    tracks: &[tidal::Track],
    discs: bool,
    edit: Option<&PlaylistRows>,
) {
    let multi = discs && tracks.iter().any(|t| t.volume_number > 1);
    // Only a multi-disc album (short) needs a row list, for its disc headers; in any other
    // list row i is track i, and nothing is built per frame.
    let mut rows = Vec::new();
    if multi {
        let mut disc = 0;
        for (i, t) in tracks.iter().enumerate() {
            if t.volume_number != disc {
                disc = t.volume_number;
                rows.push(Row::Disc(disc));
            }
            rows.push(Row::Track(i));
        }
    }
    let n = if multi { rows.len() } else { tracks.len() };
    let playing = ctx.model.playback.track.as_ref().map(|t| t.id);
    let tier = playing_colour(ctx);
    ui.push_id(id, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0; // the rows right under the header
        if n > 0 {
            header(ui, &ctx.palette, !discs);
        }
        let top = ui.cursor().top();
        rows::virtual_rows(ui, n, ROW_H, |ui, r| {
            let row = if multi { rows[r] } else { Row::Track(r) };
            let sense = match row {
                Row::Disc(_) => Sense::hover(),
                Row::Track(_) if edit.is_some() => Sense::click_and_drag(),
                Row::Track(_) => Sense::click(),
            };
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
            match row {
                Row::Disc(n) => {
                    let (p, mid) = (ctx.palette, rect.center().y);
                    let label = format!("{} {n}", tr("Disc"));
                    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &label));
                    let [index, title, ..] = columns(rect, false, false);
                    let at = pos2(index.center(), mid);
                    paint_icon(ui, Icon::Disc3, at, GLYPH, p.secondary);
                    let x = Rangef::new(title.min, rect.right());
                    cell(ui, x, mid, &label, theme::small(), p.secondary, Align::Min);
                }
                Row::Track(i) => {
                    let resp = hand(resp);
                    let t = &tracks[i];
                    let num = if discs {
                        t.track_number.to_string()
                    } else {
                        (i + 1).to_string()
                    };
                    let current = (playing == Some(t.id)).then_some(tier);
                    paint_row(ui, ctx, &resp, num, t, !discs, !discs, current);
                    if t.stream_ready && (resp.double_clicked() || activated(&resp)) {
                        ctx.send(Action::PlayTracks {
                            tracks: tracks.to_vec(),
                            start: i,
                            source: super::playing_from(ctx.model),
                        });
                    }
                    if let Some(e) = edit
                        && resp.drag_started_by(PointerButton::Primary)
                    {
                        let etag = e.etag.to_owned();
                        DragAndDrop::set_payload(ui.ctx(), PlaylistDrag { from: i, etag });
                    }
                    menu(&resp, ctx, t, edit.map(|e| (e, i)));
                }
            }
        });
        if let Some(e) = edit
            && let Some((drag, slot)) = queue_panel::dragging::<PlaylistDrag>(ui, n, top)
            && drag.etag == e.etag
            && let Some((from, to)) = move_for(e.positions, drag.from, slot)
        {
            ctx.send(Action::EditPlaylist(PlaylistEdit::Move {
                uuid: e.uuid.to_owned(),
                etag: e.etag.to_owned(),
                from,
                to,
            }));
        }
    });
}

/// Enter, Space or a screen reader "click" on `row`, without the mouse.
pub fn activated(row: &egui::Response) -> bool {
    row.clicked() && !row.clicked_by(PointerButton::Primary)
}

/// The hand over `row`, but not while it is dragged: egui's drag and drop shows its grab then,
/// and only over the default cursor.
pub fn hand(row: egui::Response) -> egui::Response {
    if row.dragged() {
        row
    } else {
        row.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// The colour of the playing row: the stream's tier's, `text` until it is heard.
pub fn playing_colour(ctx: &Ctx) -> Color32 {
    let output = ctx.model.playback.output.as_ref();
    ctx.palette.tier(output.map_or(Tier::Low, Tier::of))
}

/// Where a row across `rect` draws, left to right: the index, then the title (past the cover
/// with `cover`), the artists, the album (empty without `album`) and the duration. The title
/// takes half of what the index, cover and duration leave, the artists and album a quarter
/// each (the artists half without `album`); each text column ends `COL_GAP` short of the next.
fn columns(rect: Rect, cover: bool, album: bool) -> [Rangef; 5] {
    let index = rect.left() + INDEX_COL;
    let title = index + if cover { COVER_COL + COL_GAP } else { 0.0 };
    let time = rect.right() - COL_GAP - TIME_COL;
    let rest = (time - title).max(0.0);
    let artist = title + rest * 0.5;
    let album_x = if album { artist + rest * 0.25 } else { time };
    let text = |from: f32, to: f32| Rangef::new(from, (to - COL_GAP).max(from));
    [
        Rangef::new(rect.left(), index),
        text(title, artist),
        text(artist, album_x),
        text(album_x, time),
        Rangef::new(time, rect.right() - COL_GAP),
    ]
}

/// `text` on one line in `font` and `ink`, vertically centred on `mid`, inside `x`: cut with
/// "…" to fit, and clipped to it. `align` places it at `x`'s start, centre or end. Where it
/// went and its layout, if it was drawn.
fn cell(
    ui: &Ui,
    x: Rangef,
    mid: f32,
    text: &str,
    font: FontId,
    ink: Color32,
    align: Align,
) -> Option<(Pos2, Arc<Galley>)> {
    if text.is_empty() || x.span() <= 0.0 {
        return None;
    }
    let galley = line(ui, text, font, ink, x.span());
    let size = galley.size();
    let left = match align {
        Align::Min => x.min,
        Align::Center => x.center() - size.x / 2.0,
        Align::Max => x.max - size.x,
    };
    let at = pos2(left, mid - size.y / 2.0);
    let clip = Rect::from_x_y_ranges(x, ui.clip_rect().y_range());
    ui.painter_at(clip).galley(at, galley.clone(), ink);
    Some((at, galley))
}

/// `names` (each a char range of the text `galley` lays out at `at`, and where it leads) as
/// pointer links through `row` (spec §3.8): underlined in `ink` under the pointer (the row
/// shows the hand), opened by a primary click. No Tab stops: the row's Enter and its menu's
/// Go to album / Go to artist serve the keyboard. A name the "…" cuts ends at it; one wholly
/// past the cut is no link.
fn links(
    ui: &Ui,
    ctx: &mut Ctx,
    row: &egui::Response,
    (at, galley): (Pos2, Arc<Galley>),
    names: impl IntoIterator<Item = (Range<usize>, Route)>,
    ink: Color32,
) {
    let Some(pointer) = row.hover_pos().filter(|_| !row.dragged()) else {
        return;
    };
    let end = galley.end().index.0; // past the "…" when cut
    let shown = end.saturating_sub(usize::from(galley.elided));
    let x = |i| at.x + galley.pos_from_cursor(CCursor::new(i)).min.x;
    let y = Rangef::new(at.y, at.y + galley.size().y);
    for (chars, to) in names {
        if chars.start >= shown {
            return;
        }
        let last = if chars.end <= shown { chars.end } else { end };
        let rect = Rect::from_x_y_ranges(x(chars.start)..=x(last), y);
        if rect.contains(pointer) {
            let foot = rect.bottom() - 0.5;
            ui.painter()
                .hline(rect.x_range(), foot, Stroke::new(1.0, ink));
            if row.clicked_by(PointerButton::Primary) {
                ctx.go(to);
            }
            return;
        }
    }
}

/// The column header (spec §5.5), `ROW_HEADER` tall: "#", "Title" (over the cover too),
/// "Artist", and "Album" with `album`, in `small()` `secondary`, upper case; a clock over the
/// durations.
fn header(ui: &mut Ui, p: &Palette, album: bool) {
    let size = vec2(ui.available_width(), theme::ROW_HEADER);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let [index, title, artist, album_x, time] = columns(rect, album, album);
    let title = Rangef::new(index.max, title.max);
    let (mid, ink) = (rect.center().y, p.secondary);
    let head = |x, text: &str, align| {
        cell(ui, x, mid, &text.to_uppercase(), theme::small(), ink, align);
    };
    head(index, "#", Align::Center);
    head(title, &tr("Title"), Align::Min);
    head(artist, &tr("Artist"), Align::Min);
    if album {
        head(album_x, &tr("Album"), Align::Min);
    }
    let clock = pos2(time.max - GLYPH / 2.0, mid);
    paint_icon(ui, Icon::Clock, clock, GLYPH, ink);
}

/// Paints track `t`'s row (`row` its response, `ui` its own): `num`, or a play glyph hovered
/// and a playing one when `current` (the playing row's tier colour, which its title takes
/// too); the cover with `cover`; the title, artists, the album with `album`, and the duration,
/// each cut to its column. An unavailable track's row is at half opacity.
#[expect(clippy::too_many_arguments)] // two callers, each part named at the call
pub fn paint_row(
    ui: &mut Ui,
    ctx: &mut Ctx,
    row: &egui::Response,
    num: String,
    t: &tidal::Track,
    cover: bool,
    album: bool,
    current: Option<Color32>,
) {
    row.widget_info(|| {
        let label = format!(
            "{num}. {}, {}, {}",
            t.title,
            artists(t),
            duration(t.duration)
        );
        WidgetInfo::labeled(WidgetType::Button, t.stream_ready, label)
    });
    focus_ring(ui, row, theme::R_ROW); // and scrolls a row Tab reaches into view
    let (rect, p) = (row.rect, ctx.palette);
    if !ui.is_rect_visible(rect) {
        return;
    }
    if !t.stream_ready {
        ui.multiply_opacity(0.5);
    }
    let hover = fade(ui, row, row.hovered() || row.has_focus());
    ui.painter()
        .rect_filled(rect, theme::R_ROW, p.surface_hover.gamma_multiply(hover));
    let mid = rect.center().y;
    let [index, title, artist, album_x, time] = columns(rect, cover, album);
    let put = |x, text: &str, font, ink, align| cell(ui, x, mid, text, font, ink, align);
    let glyph = pos2(index.center(), mid);
    if row.hovered() && t.stream_ready {
        paint_icon(ui, Icon::PlayFilled, glyph, GLYPH, p.text);
    } else if let Some(tier) = current {
        paint_icon(ui, Icon::AudioLines, glyph, GLYPH, tier);
    } else {
        put(index, &num, theme::body(), p.secondary, Align::Center);
    }
    if cover {
        let at = pos2(index.max, mid - ROW_COVER / 2.0);
        let square = Rect::from_min_size(at, Vec2::splat(ROW_COVER));
        let art = t.album.image();
        super::cover(ui, ctx.images, ctx.tx, art.as_ref(), square, theme::R_ART);
    }
    let ink = current.unwrap_or(p.text);
    put(title, &t.title, theme::body_strong(), ink, Align::Min);
    if let Some(laid) = put(artist, &artists(t), theme::body(), p.text, Align::Min) {
        let mut from = 0;
        let names = t.artists.iter().map(|a| {
            let chars = from..from + a.name.chars().count();
            from = chars.end + ", ".len(); // as `names` joins them
            (chars, Route::Artist(a.id))
        });
        links(ui, ctx, row, laid, names, p.text);
    }
    if album && let Some(laid) = put(album_x, &t.album.title, theme::body(), p.text, Align::Min) {
        let name = (0..t.album.title.chars().count(), Route::Album(t.album.id));
        links(ui, ctx, row, laid, [name], p.text);
    }
    let length = duration(t.duration);
    put(time, &length, theme::body(), p.secondary, Align::Max);
}

/// A track row's context menu; with `edit`, the playlist's rows and this one's index in them,
/// it can remove the row. `ui.close()` also closes it when an entry is picked from the
/// keyboard or a screen reader, which no pointer click does.
///
/// A row's ids are its index's, so a list replaced under an open menu (a playlist reloaded
/// after an edit) would hand it another track and listing: it closes instead.
fn menu(
    row: &egui::Response,
    ctx: &mut Ctx,
    t: &tidal::Track,
    edit: Option<(&PlaylistRows, usize)>,
) {
    let id = row.id.with("menu-for");
    let now = || (t.id, edit.map(|(e, _)| e.etag.to_owned()));
    if row.secondary_clicked() {
        row.ctx.data_mut(|d| d.insert_temp(id, now())); // as the menu opens
    }
    let p = ctx.palette;
    context_menu(row, &p, |ui| {
        if ui.data(|d| d.get_temp(id)) != Some(now()) {
            ui.close();
            return;
        }
        if ui
            .add(menu_button(&p, Icon::ListStart, tr("Play next")))
            .clicked()
        {
            ctx.send(Action::PlayNext(vec![t.clone()]));
            ui.close();
        }
        if ui
            .add(menu_button(&p, Icon::ListPlus, tr("Add to queue")))
            .clicked()
        {
            ctx.send(Action::AddToQueue(vec![t.clone()]));
            ui.close();
        }
        library::add_to_playlist(ui, ctx, std::slice::from_ref(t));
        menu_separator(ui);
        if ui
            .add(menu_button(&p, Icon::Disc3, tr("Go to album")))
            .clicked()
        {
            ctx.go(Route::Album(t.album.id));
            ui.close();
        }
        if let Some(artist) = t.artists.first()
            && ui
                .add(menu_button(&p, Icon::User, tr("Go to artist")))
                .clicked()
        {
            ctx.go(Route::Artist(artist.id));
            ui.close();
        }
        let radio = menu_button(&p, Icon::Radio, tr("Track radio"));
        if ui.add_enabled(t.mixes.track.is_some(), radio).clicked()
            && let Some(mix) = &t.mixes.track
        {
            ctx.go(Route::Mix(mix.clone()));
            ui.close();
        }
        menu_separator(ui);
        let fav = tidal::Favorite::Track(t.id);
        let heart = if ctx.model.favorites.contains(&fav) {
            Icon::HeartFilled
        } else {
            Icon::Heart
        };
        let item = menu_button(&p, heart, ctx.favorite_label(&fav));
        if ctx.favorite_button(ui, fav, item).clicked() {
            ui.close();
        }
        if ui
            .add(menu_button(&p, Icon::Link, tr("Copy link")))
            .clicked()
        {
            ui.ctx()
                .copy_text(format!("https://tidal.com/browse/track/{}", t.id));
            ui.close();
        }
        if let Some((e, i)) = edit
            && let Some(&position) = e.positions.get(i)
        {
            menu_separator(ui);
            let remove = menu_button(&p, Icon::X, tr("Remove from this playlist"));
            if ui.add(remove).clicked() {
                ctx.send(Action::EditPlaylist(PlaylistEdit::Remove {
                    uuid: e.uuid.to_owned(),
                    etag: e.etag.to_owned(),
                    positions: vec![position],
                }));
                ui.close();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use crate::model::{Action, Event, Model, Route};
    use crate::ui::testing::{self, click, middle};
    use crate::ui::{library, page};
    use egui::text::CCursor;
    use egui::{Key, Pos2, vec2};
    use std::sync::Arc;
    use tokio::sync::mpsc::UnboundedSender;

    fn track() -> tidal::Track {
        tidal::Track {
            id: 1,
            title: "t".into(),
            duration: 60,
            track_number: 1,
            volume_number: 1,
            artists: vec![],
            album: tidal::AlbumRef {
                id: 2,
                title: "a".into(),
                cover: None,
            },
            stream_ready: true,
            ..Default::default()
        }
    }

    /// One pass of a page holding `list`, with the library's dialog, fed `input`: the events it
    /// gave, and its output.
    fn pass(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
        input: Vec<egui::Event>,
    ) -> (Vec<Event>, egui::FullOutput) {
        pass_at(ctx, tx, model, list, 800.0, input)
    }

    /// As `pass`, in a window `width` wide.
    fn pass_at(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
        width: f32,
        input: Vec<egui::Event>,
    ) -> (Vec<Event>, egui::FullOutput) {
        let input = egui::RawInput {
            events: input,
            screen_rect: Some(egui::Rect::from_min_size(Pos2::ZERO, vec2(width, 600.0))),
            ..Default::default()
        };
        testing::pass_raw(ctx, tx, model, input, |ui, c| {
            page(ui, &Route::Search, |ui| {
                super::show(ui, c, "t", list, false, None)
            });
            library::dialog(ui.ctx(), c);
        })
    }

    /// A primary button press at `at` if `pressed`, else its release.
    fn primary(at: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// The pointer moves to `at` and clicks there, a pass each, in a window `width` wide: the
    /// events those passes gave.
    fn click_at(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
        width: f32,
        at: Pos2,
    ) -> Vec<Event> {
        let steps = [
            vec![egui::Event::PointerMoved(at)],
            vec![primary(at, true)],
            vec![primary(at, false)],
        ];
        let pass = |input| pass_at(ctx, tx, model, list, width, input).0;
        steps.into_iter().flat_map(pass).collect()
    }

    /// The painted text `text` in `out`: where it is drawn, and its layout.
    fn painted(out: &egui::FullOutput, text: &str) -> (Pos2, Arc<egui::Galley>) {
        let shape = out.shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.text() == text => Some((t.pos, t.galley.clone())),
            _ => None,
        });
        shape.unwrap_or_else(|| panic!("no {text:?} painted"))
    }

    /// The middle of char `n` of the painted `text` in `out` (the "…" past what shows of a
    /// cut one), where a pointer would click it.
    fn char_at(out: &egui::FullOutput, text: &str, n: usize) -> Pos2 {
        let (at, galley) = painted(out, text);
        let x = |i: usize| galley.pos_from_cursor(CCursor::new(i)).min.x;
        at + vec2((x(n) + x(n + 1)) / 2.0, galley.size().y / 2.0)
    }

    /// Track 1 by `artists`, on "Album 1".
    fn by(artists: &[u64]) -> tidal::Track {
        tidal::Track {
            artists: artists.iter().map(|&id| testing::artist(id)).collect(),
            ..testing::track(1)
        }
    }

    /// Spec §3.8: a primary click on an artist or the album in a row opens it; on the title
    /// it does not, and a double click there still plays. Hovered, a name is underlined (1 px
    /// `text` along its foot, under it alone) under the row's hand.
    #[test]
    fn names_in_a_row_navigate() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![by(&[1, 2])]);
        let names = "Artist 1, Artist 2";
        let out = pass(&testing::context(), &tx, &model, &list, vec![]).1;
        // A context each: no click left over to make the next a double one.
        let click = |at| click_at(&testing::context(), &tx, &model, &list, 800.0, at);
        let events = click(char_at(&out, names, 10));
        assert!(
            matches!(events[..], [Event::Navigate(Route::Artist(2))]),
            "{events:?}"
        );
        let events = click(char_at(&out, "Album 1", 3));
        assert!(
            matches!(events[..], [Event::Navigate(Route::Album(1))]),
            "{events:?}"
        );
        let title = char_at(&out, "Track 1", 2);
        let ctx = testing::context();
        let events = click_at(&ctx, &tx, &model, &list, 800.0, title);
        assert!(events.is_empty(), "{events:?}");
        assert!(rx.try_recv().is_err(), "one click plays nothing");
        let again = [vec![primary(title, true)], vec![primary(title, false)]];
        for input in again {
            pass(&ctx, &tx, &model, &list, input);
        }
        assert!(matches!(
            rx.try_recv(),
            Ok(Action::PlayTracks { start: 0, .. })
        ));

        let p = crate::theme::Palette::dark();
        let (at, galley) = painted(&out, names);
        let x = |i: usize| at.x + galley.pos_from_cursor(CCursor::new(i)).min.x;
        let foot = at.y + galley.size().y - 0.5;
        let underlines = |out: &egui::FullOutput| -> Vec<egui::Rangef> {
            let line = |s: &egui::epaint::ClippedShape| match s.shape {
                egui::Shape::LineSegment {
                    points: [a, b],
                    stroke,
                } if stroke == egui::Stroke::new(1.0, p.text) && a.y == foot && b.y == foot => {
                    Some(egui::Rangef::new(a.x.min(b.x), a.x.max(b.x)))
                }
                _ => None,
            };
            out.shapes.iter().filter_map(line).collect()
        };
        assert!(underlines(&out).is_empty(), "at rest");
        let ctx = testing::context();
        pass(&ctx, &tx, &model, &list, vec![]); // the rows the pointer finds next pass
        let moved = vec![egui::Event::PointerMoved(char_at(&out, names, 3))];
        let out = pass(&ctx, &tx, &model, &list, moved).1;
        assert_eq!(
            out.platform_output.cursor_icon,
            egui::CursorIcon::PointingHand
        );
        assert_eq!(
            underlines(&out),
            [egui::Rangef::new(x(0), x(8))],
            "Artist 1"
        );
    }

    /// Spec §3.8: a name is the row's: right-clicked, the row's menu opens; in an editable
    /// playlist, a primary drag from it drags the row, opening nothing.
    #[test]
    fn links_leave_the_row_its_menu_and_drag() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![by(&[1, 2])]);
        let rows = super::PlaylistRows {
            uuid: "p-1",
            etag: "e",
            positions: &[0],
        };
        let pass = |ctx: &egui::Context, input| {
            testing::pass_with(ctx, &tx, &model, input, |ui, c| {
                page(ui, &Route::Search, |ui| {
                    super::show(ui, c, "t", &list, false, Some(&rows))
                });
            })
        };
        let ctx = testing::context();
        ctx.enable_accesskit();
        let out = pass(&ctx, vec![]).1;
        let at = char_at(&out, "Artist 1, Artist 2", 3);
        let mut events = Vec::new();
        for input in [
            vec![egui::Event::PointerMoved(at), testing::right(at, true)],
            vec![testing::right(at, false)],
            vec![],
        ] {
            events.extend(pass(&ctx, input).0);
        }
        let out = pass(&ctx, vec![]).1;
        assert!(
            testing::shown(&out).contains(&"Play next".into()),
            "no menu"
        );
        let ctx = testing::context();
        for input in [
            vec![egui::Event::PointerMoved(at)],
            vec![primary(at, true)],
            vec![egui::Event::PointerMoved(at + vec2(0.0, 10.0))],
        ] {
            events.extend(pass(&ctx, input).0);
        }
        let drag = egui::DragAndDrop::payload::<super::PlaylistDrag>(&ctx);
        assert!(matches!(drag.as_deref(), Some(d) if d.from == 0 && d.etag == "e"));
        assert!(events.is_empty(), "nothing opened: {events:?}");
    }

    /// Spec §3.8: in a window `NARROW` wide the artist column cuts "Artist 1, Artist 2,
    /// Artist 3" to "Artist 1, Art…": the "…" still opens artist 2, and no point of the
    /// column opens artist 3, which does not show.
    #[test]
    fn a_cut_name_links_only_what_shows() {
        const NARROW: f32 = 610.0;
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![by(&[1, 2, 3])]);
        let names = "Artist 1, Artist 2, Artist 3";
        let out = pass_at(&ctx, &tx, &model, &list, NARROW, vec![]).1;
        let (at, galley) = painted(&out, names);
        let shown: String = galley.rows[0].glyphs.iter().map(|g| g.chr).collect();
        assert_eq!(shown, "Artist 1, Art…");
        let click = |at| click_at(&ctx, &tx, &model, &list, NARROW, at);
        let events = click(char_at(&out, names, 13));
        assert!(
            matches!(events[..], [Event::Navigate(Route::Artist(2))]),
            "the …: {events:?}"
        );
        // Every 2 px from the column's start to the album's.
        let (album, _) = painted(&out, "Album 1");
        let y = at.y + galley.size().y / 2.0;
        let mut opened = Vec::new();
        let mut x = at.x;
        while x < album.x {
            opened.extend(click(Pos2::new(x, y)));
            x += 2.0;
        }
        let to = |id| {
            let artist = |e: &Event| matches!(e, Event::Navigate(Route::Artist(a)) if *a == id);
            opened.iter().any(artist)
        };
        assert!(to(1) && to(2) && !to(3), "{opened:?}");
    }

    /// Tab to the first row and press Enter: it plays, as a double click would.
    #[test]
    fn enter_on_a_focused_row_plays() {
        let ctx = testing::context();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let list = vec![track()];
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        for input in [vec![], vec![key(Key::Tab)], vec![key(Key::Enter)]] {
            pass(&ctx, &tx, &Model::default(), &list, input);
        }
        assert!(matches!(
            rx.try_recv(),
            Ok(Action::PlayTracks { start: 0, .. })
        ));
    }

    /// Spec §3.9: a row double-clicked on a playlist plays from that playlist; on Search, from
    /// nowhere to return to.
    #[test]
    fn a_row_plays_from_its_page() {
        use crate::model::{PageData, Source};
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let list = vec![track()];
        let mut source = |model: &Model| {
            let ctx = testing::context(); // no click left over from the last double click
            let at = Pos2::new(100.0, crate::theme::ROW_HEADER + super::ROW_H / 2.0);
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            pass(&ctx, &tx, model, &list, vec![egui::Event::PointerMoved(at)]);
            for pressed in [true, false, true, false] {
                pass(&ctx, &tx, model, &list, vec![button(pressed)]);
            }
            match rx.try_recv() {
                Ok(Action::PlayTracks { source, .. }) => source,
                other => panic!("{other:?}"),
            }
        };
        let route = Route::Playlist("p-1".into());
        let mut playlist = testing::at(route.clone());
        let data = PageData::Playlist {
            playlist: testing::playlist(),
            tracks: list.clone(),
            positions: vec![0],
            etag: None,
        };
        playlist.apply(Event::Page {
            route: route.clone(),
            data,
        });
        let want = Source {
            route,
            name: "Playlist".into(),
        };
        assert_eq!(source(&playlist), Some(want));
        assert_eq!(source(&testing::at(Route::Search)), None);
    }

    /// Right-clicks the first row of `list`: the output with its menu open.
    fn open_menu(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
    ) -> egui::FullOutput {
        // The first row, under the column header.
        let at = Pos2::new(100.0, crate::theme::ROW_HEADER + super::ROW_H / 2.0);
        let right = |pressed| testing::right(at, pressed);
        pass(ctx, tx, model, list, vec![]);
        let press = vec![egui::Event::PointerMoved(at), right(true)];
        pass(ctx, tx, model, list, press);
        pass(ctx, tx, model, list, vec![right(false)]);
        settle(ctx, tx, model, list) // the menu lays itself out
    }

    /// A few passes without input, for menus and dialogs to lay themselves out: the last output.
    fn settle(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
    ) -> egui::FullOutput {
        pass(ctx, tx, model, list, vec![]);
        pass(ctx, tx, model, list, vec![]).1
    }

    /// Right-click a row, then pick "Go to album" (as a screen reader would): its album opens.
    #[test]
    fn context_menu_go_to_album_navigates() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![track()]);
        let out = open_menu(&ctx, &tx, &model, &list);
        click(&out, "Add to favorites"); // panics unless the menu offers it too
        let go = vec![click(&out, "Go to album")];
        let (events, _) = pass(&ctx, &tx, &model, &list, go);
        assert!(
            matches!(events[..], [Event::Navigate(Route::Album(2))]),
            "{events:?}"
        );
    }

    /// Spec §3.7: a row's menu has `MENU_ITEM`-tall items, its groups split by 1 px `outline`
    /// rules, and a frame at least `MENU_W` wide; its "Add to playlist" submenu's items too.
    #[test]
    fn the_row_menu_is_roomy() {
        use crate::theme::{MENU_ITEM, MENU_W, Palette};
        let ctx = testing::context();
        ctx.enable_accesskit();
        let p = Palette::dark();
        crate::theme::apply(&ctx, &p);
        ctx.all_styles_mut(|s| s.animation_time = 0.0); // the menu shows at once, not fading in
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let t = tidal::Track {
            artists: vec![testing::artist(1)],
            ..track()
        };
        let (model, list) = (Model::default(), vec![t]);
        let out = open_menu(&ctx, &tx, &model, &list);
        let b = |label: &str| testing::bounds(&out, label);
        for label in [
            "Play next",
            "Add to queue",
            "Go to album",
            "Go to artist",
            "Copy link",
        ] {
            assert!(
                b(label).height() >= MENU_ITEM - 0.5,
                "{label}: {:?}",
                b(label)
            );
        }
        fn flat(out: &egui::FullOutput) -> Vec<egui::Shape> {
            let one = |c: &egui::epaint::ClippedShape| match &c.shape {
                egui::Shape::Vec(v) => v.clone(),
                s => vec![s.clone()],
            };
            out.shapes.iter().flat_map(one).collect()
        }
        let shapes = flat(&out);
        let rule_between = |above: &str, below: &str| {
            let y = egui::Rangef::new(b(above).bottom(), b(below).top());
            shapes.iter().any(|s| match s {
                egui::Shape::LineSegment {
                    points: [a, z],
                    stroke,
                } => a.y == z.y && y.contains(a.y) && *stroke == egui::Stroke::new(1.0, p.outline),
                _ => false,
            })
        };
        assert!(rule_between("Add to playlist", "Go to album"));
        assert!(rule_between("Track radio", "Add to favorites"));
        let frame = shapes.iter().find_map(|s| match s {
            egui::Shape::Rect(r) if r.fill == p.overlay => Some(r.rect),
            _ => None,
        });
        let frame = frame.expect("the menu's frame");
        assert!(frame.width() >= MENU_W, "{frame:?}");
        pass(
            &ctx,
            &tx,
            &model,
            &list,
            vec![click(&out, "Add to playlist")],
        );
        let out = settle(&ctx, &tx, &model, &list);
        let new = testing::bounds(&out, "New playlist…");
        assert!(new.height() >= MENU_ITEM - 0.5, "{new:?}");
        // Hovered, an item (a submenu's too) is `surface_hover` over `overlay`, radius 8, and
        // reads against the frame: before, it was `surface`'s hover, 6 off `overlay`.
        let hover = p.overlay.blend(p.surface_hover);
        assert!(hover.r().abs_diff(p.overlay.r()) >= 16, "{hover:?}");
        let hovered = |at: egui::Pos2| {
            pass(
                &ctx,
                &tx,
                &model,
                &list,
                vec![egui::Event::PointerMoved(at)],
            );
            let out = pass(&ctx, &tx, &model, &list, vec![]).1;
            flat(&out).iter().any(|s| match s {
                egui::Shape::Rect(r) => {
                    r.fill == hover && r.rect.contains(at) && r.corner_radius == 8.into()
                }
                _ => false,
            })
        };
        assert!(hovered(new.center()), "New playlist…");
        assert!(hovered(b("Copy link").center()), "Copy link");
    }

    /// "Play next" and "Add to queue" in a row's menu send that track to the queue.
    #[test]
    fn track_menu_play_next_sends_action() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![track()]);
        let mut pick = |entry| {
            let out = open_menu(&ctx, &tx, &model, &list);
            pass(&ctx, &tx, &model, &list, vec![click(&out, entry)]);
            rx.try_recv().expect("an action")
        };
        let ids = |t: &[tidal::Track]| t.iter().map(|t| t.id).collect::<Vec<_>>();
        assert!(matches!(pick("Play next"), Action::PlayNext(t) if ids(&t) == [1]));
        assert!(matches!(pick("Add to queue"), Action::AddToQueue(t) if ids(&t) == [1]));
    }

    /// "Track radio" opens the track's mix.
    #[test]
    fn track_radio_opens_the_mix() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut t = track();
        t.mixes.track = Some("m".into());
        let (model, list) = (Model::default(), vec![t]);
        let out = open_menu(&ctx, &tx, &model, &list);
        let radio = vec![click(&out, "Track radio")];
        let (events, _) = pass(&ctx, &tx, &model, &list, radio);
        assert!(
            matches!(&events[..], [Event::Navigate(Route::Mix(id))] if id == "m"),
            "{events:?}"
        );
    }

    /// Without a mix, "Track radio" is shown but disabled, and a click does nothing.
    #[test]
    fn no_mix_disables_track_radio() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (Model::default(), vec![track()]);
        let out = open_menu(&ctx, &tx, &model, &list);
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let radio = nodes.iter().find(|(_, n)| n.label() == Some("Track radio"));
        assert!(radio.expect("Track radio shown").1.is_disabled());
        let radio = vec![click(&out, "Track radio")];
        let (events, _) = pass(&ctx, &tx, &model, &list, radio);
        assert!(events.is_empty(), "{events:?}");
    }

    /// Logged in, with two own playlists, newest updated first (as the backend sorts them).
    fn with_playlists() -> Model {
        let mut model = testing::at(Route::Search);
        let p = |uuid: &str, title: &str| tidal::Playlist {
            uuid: uuid.into(),
            title: title.into(),
            ..testing::playlist()
        };
        let own = vec![p("p-new", "Newer"), p("p-old", "Older")];
        model.apply(Event::OwnPlaylists(own));
        model
    }

    /// Opens the first row's menu and its "Add to playlist" submenu: the output showing it.
    fn add_menu(
        ctx: &egui::Context,
        tx: &UnboundedSender<Action>,
        model: &Model,
        list: &[tidal::Track],
    ) -> egui::FullOutput {
        let out = open_menu(ctx, tx, model, list);
        pass(ctx, tx, model, list, vec![click(&out, "Add to playlist")]);
        settle(ctx, tx, model, list)
    }

    /// "Add to playlist" lists "New playlist…" and then the own playlists, newest updated
    /// first; picking one adds the track to it.
    #[test]
    fn add_to_playlist_lists_own_playlists_and_sends_add() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (with_playlists(), vec![track()]);
        let out = add_menu(&ctx, &tx, &model, &list);
        let y = |label| middle(&out, label).y;
        assert!(y("New playlist…") < y("Newer") && y("Newer") < y("Older"));
        pass(&ctx, &tx, &model, &list, vec![click(&out, "Older")]);
        let sent: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|a| format!("{a:?}"))
            .collect();
        let add = r#"EditPlaylist(Add { uuid: "p-old", tracks: [1] })"#;
        assert_eq!(sent, [add]);
        let out = settle(&ctx, &tx, &model, &list);
        assert!(!testing::shown(&out).contains(&"Newer".into()), "closed");
    }

    /// "New playlist…" asks for a title; the playlist made starts with the track.
    #[test]
    fn new_playlist_from_a_track_sends_create_with_it() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (model, list) = (with_playlists(), vec![track()]);
        let out = add_menu(&ctx, &tx, &model, &list);
        pass(&ctx, &tx, &model, &list, vec![click(&out, "New playlist…")]);
        settle(&ctx, &tx, &model, &list); // the dialog opens, its title focused
        assert!(rx.try_recv().is_err(), "asked first");
        let text = egui::Event::Text("Road trip".into());
        pass(&ctx, &tx, &model, &list, vec![text]);
        pass(&ctx, &tx, &model, &list, vec![testing::key(Key::Enter)]);
        let create = concat!(
            r#"EditPlaylist(Create { title: "Road trip", description: "", "#,
            "folder: None, tracks: [1] })"
        );
        assert_eq!(format!("{:?}", rx.try_recv().expect("sent")), create);
    }

    /// A drop on a row's own edges moves nothing; elsewhere, the dragged row's position goes
    /// before the position of the row the drop lands on, or after the last.
    #[test]
    fn drag_reorder_sends_positions() {
        let positions = [0, 2, 3];
        let at = |from, slot| super::move_for(&positions, from, slot);
        assert_eq!(at(1, 1), None, "its top edge");
        assert_eq!(at(1, 2), None, "its bottom edge");
        assert_eq!(at(0, 2), Some((0, 3)));
        assert_eq!(at(2, 0), Some((3, 0)));
        assert_eq!(at(1, 0), Some((2, 0)));
        assert_eq!(at(0, 3), Some((0, 4)), "past the last");
    }

    /// Each text shape in `out`: its text, where it is drawn, and its colour.
    fn texts(out: &egui::FullOutput) -> Vec<(String, egui::Rect, egui::Color32)> {
        let text = |t: &egui::epaint::TextShape| {
            let section = t.galley.job.sections.first();
            let colour = section.map_or(t.fallback_color, |s| s.format.color);
            (t.galley.text().to_owned(), t.visual_bounding_rect(), colour)
        };
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(text(t)),
                _ => None,
            })
            .collect()
    }

    /// A 300-letter title and a 200-letter Arabic one (their artist and album as long), at
    /// 800 px: each of a row's texts ends left of where the next column's begins.
    #[test]
    fn long_titles_stay_in_their_columns() {
        let ctx = testing::context();
        ctx.enable_accesskit();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let long = "W".repeat(300);
        let row = |id, title: String| tidal::Track {
            id,
            title,
            artists: vec![tidal::Artist {
                name: long.clone(),
                ..testing::artist(1)
            }],
            album: tidal::AlbumRef {
                id: 2,
                title: long.clone(),
                cover: None,
            },
            ..track()
        };
        let list = vec![row(1, long.clone()), row(2, "ع".repeat(200))];
        let model = Model::default();
        let (_, out) = pass(&ctx, &tx, &model, &list, vec![]);
        let shapes = texts(&out);
        for (n, t) in list.iter().enumerate() {
            let label = format!("{}. {}, {long}, 1:00", n + 1, t.title);
            let row = testing::bounds(&out, &label);
            let mut cells: Vec<egui::Rect> = shapes
                .iter()
                .map(|(_, rect, _)| *rect)
                .filter(|r| row.y_range().contains(r.center().y))
                .collect();
            cells.sort_by(|a, b| a.left().total_cmp(&b.left()));
            assert_eq!(
                cells.len(),
                5,
                "index, title, artist, album, time: {cells:?}"
            );
            for pair in cells.windows(2) {
                assert!(pair[0].right() < pair[1].left(), "row {n}: {pair:?}");
            }
            assert!(cells[4].right() <= row.right(), "row {n}: {cells:?}");
        }
    }

    /// Row 2 plays, a 24/96 FLAC stream: its title takes Max's colour; row 1's stays `text`.
    #[test]
    fn the_playing_row_takes_its_tier_colour() {
        use crate::model::OutputInfo;
        use crate::theme::{Palette, Tier};
        let ctx = testing::context();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let list = testing::tracks(3);
        let mut model = Model::default();
        model.playback.track = Some(list[1].clone());
        model.playback.output = Some(OutputInfo {
            codec: "FLAC".into(),
            format: player::Format {
                rate: 96_000,
                bits: 24,
                channels: 2,
            },
            exclusive: false,
            device_rate: 96_000,
        });
        let (_, out) = pass(&ctx, &tx, &model, &list, vec![]);
        let shapes = texts(&out);
        let colour = |text: &str| shapes.iter().find(|(t, ..)| t == text).map(|s| s.2);
        let p = Palette::dark();
        assert_eq!(colour("Track 2"), Some(p.tier(Tier::Max)));
        assert_eq!(colour("Track 1"), Some(p.text));
    }

    #[test]
    fn duration_is_m_ss_or_h_mm_ss() {
        assert_eq!(super::duration(0), "0:00");
        assert_eq!(super::duration(65), "1:05");
        assert_eq!(super::duration(3600), "1:00:00");
        assert_eq!(super::duration(36_125), "10:02:05");
    }
}
